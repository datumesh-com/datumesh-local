//! A fake of the production mesh (datumesh.com) for the datumesh-local 0.5.1 tests: it serves the
//! responses captured from production (read-only GETs, `tools/capture_production_fixtures.py`,
//! kept in `tests/data/production/routes.jsonl`) on 127.0.0.1. Nothing here signs: every receipt
//! is production's own, so the tests run offline with the production key document.
//!
//! * Exact routes: a recorded path and query answers as recorded (the query's parameters in any
//!   order, spaces and quotes ignored).
//! * Cells (`/cell/<call>`): the call may be written in the URL form, the call form, positional or
//!   labelled; the function's name and the argument values decide. The register's cells
//!   (`datumesh.com/register/dataset(<source>, <code>).<method>()`) are answered from the
//!   recorded slices of each method.
//! * `describe_order`: true serves `/describe/<function>` with an `order` member (what
//!   datumesh-http 0.5.3 H1 adds), false as production answers today (no `order`).
//! * Every request is kept (`requests()`, `POSTs` included, which are answered 405).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use datumesh_core::json::{self, Value};

use crate::common::data;

/// The Treasury licence (a receipt's `licence` for `par_yield`).
pub const TREASURY_LICENCE: &str = "https://www.law.cornell.edu/uscode/text/17/105";
/// Bank Rate's licence as production's receipt carries it (an open licence, SPDX).
pub const BANK_RATE_LICENCE: &str = "OGL-UK-3.0";
/// The ECB's licence (`fx_reference`'s receipt; the register has two datasets under it).
pub const ECB_LICENCE: &str = "https://www.ecb.europa.eu/services/using-our-site/disclaimer/html/index.en.html";
/// The register's licence hash of the Treasury dataset (`par_yield_curve`).
pub const TREASURY_REGISTER_HASH: &str = "e0f425493dd80314c4777e99d29a229f1e72ffedd433891c958dec0500fed6d5";
/// The register's licence hash of the two ECB datasets (`policy_rates`, `reference_rates`).
pub const ECB_REGISTER_HASH: &str = "5c467e3b05d8e84acda31b952c7db2ced96c18bad746119039cb03da96fbf6db";
/// A sentence of the Treasury dataset's quoted clauses (the register's `licence_clauses` blob).
pub const TREASURY_CLAUSE: &str =
    "Copyright protection under this title is not available for any work of the United States Government";

pub const PAR_YIELD: &str = "datumesh.com/relay/treasury.gov/par_yield";
pub const BANK_RATE: &str = "datumesh.com/relay/bankofengland.co.uk/bank_rate";
pub const FX_REFERENCE: &str = "datumesh.com/relay/ecb.europa.eu/fx_reference";
/// The 10-year par yield and Bank Rate on 2026-11-20 (full names, labelled, canonical).
pub const PAR_10Y: &str = r#"datumesh.com/relay/treasury.gov/par_yield(date: 2026-11-20, tenor: "10Y")"#;
pub const BANK_RATE_DAY: &str = "datumesh.com/relay/bankofengland.co.uk/bank_rate(date: 2026-11-20)";
pub const FX_USD: &str = r#"datumesh.com/relay/ecb.europa.eu/fx_reference(currency: "USD", date: 2026-11-20)"#;

const METHODS: [&str; 6] = ["licence_clauses", "licence_hash", "licence_page", "name", "retrieved", "status"];

#[derive(Clone)]
struct Route {
    status: u16,
    ct: String,
    body: Vec<u8>,
}

/// One request the fake answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Req {
    pub method: String,
    /// The request target as sent (path and query, as written).
    pub target: String,
    pub answered: u16,
}

pub struct ProdMesh {
    pub origin: String,
    log: Arc<Mutex<Vec<Req>>>,
    describe_order: Arc<AtomicBool>,
    tamper: Arc<AtomicBool>,
    bank: Arc<Bank>,
    faults: Arc<Faults>,
}

/// What the fake can be made to fail (local 0.5.2, items 37 and 39), and the plain-words find.
///
/// * `/find?summary=1` answers as recorded, or 503 while `summary_down` is set.
/// * `/find?q=<query>` (any other `/find` request) answers production's shape for no match,
///   `{"query": <query>, "results": [], "vector": false}` (ITEM32-find-sources.md), or an empty error
///   answer with `find_status` when it is not 0.
#[derive(Default)]
pub struct Faults {
    summary_down: AtomicBool,
    find_status: std::sync::atomic::AtomicU16,
}

impl Faults {
    fn respond(&self, target: &str) -> Option<Route> {
        let d = decode(target);
        let (path, query) = d.split_once('?').unwrap_or((&d, ""));
        if path != "/find" {
            return None;
        }
        let error = |status: u16| Route {
            status,
            ct: "application/json".into(),
            body: format!(r#"{{"error":"E_TEST_FAULT","outcome":"error","status":{status}}}"#).into_bytes(),
        };
        if query == "summary=1" {
            return self.summary_down.load(Ordering::SeqCst).then(|| error(503));
        }
        let status = self.find_status.load(Ordering::SeqCst);
        if status != 0 {
            return Some(error(status));
        }
        let q = param(query, "q").unwrap_or_default();
        let body = Value::Object(vec![
            ("query".into(), Value::String(q)),
            ("results".into(), Value::Array(Vec::new())),
            ("vector".into(), Value::Bool(false)),
        ]);
        Some(Route { status: 200, ct: "application/json".into(), body: body.to_jcs() })
    }
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A request target without spaces and quotes, decoded, its query's parameters sorted.
fn canon(target: &str) -> String {
    let d = decode(target).replace([' ', '"'], "");
    match d.split_once('?') {
        Some((p, q)) => {
            let mut ps: Vec<&str> = q.split('&').collect();
            ps.sort();
            format!("{p}?{}", ps.join("&"))
        }
        None => d,
    }
}

/// `(function name, sorted argument values)` of a cell reference written after `/cell/`: the URL
/// form (`name?k=v&k=v`) or the call form (`name(args)` and `name(args).method()`), labelled or not.
fn cell_key(rest: &str) -> String {
    let d = decode(rest).replace([' ', '"'], "");
    if let Some(open) = d.find('(') {
        let name = &d[..open];
        let close = d[open..].find(')').map(|c| open + c).unwrap_or(d.len());
        let inner = &d[open + 1..close];
        let after = &d[(close + 1).min(d.len())..];
        let mut full = name.to_string();
        if let Some(m) = after.strip_prefix('.') {
            full = format!("{full}/{}", m.split('(').next().unwrap());
        }
        let mut vals: Vec<String> =
            inner.split(',').filter(|a| !a.is_empty()).map(|a| a.rsplit(':').next().unwrap().to_string()).collect();
        vals.sort();
        format!("{full}|{}", vals.join(","))
    } else {
        let (name, q) = d.split_once('?').unwrap_or((&d, ""));
        let mut vals: Vec<String> =
            q.split('&').filter(|a| !a.is_empty()).map(|a| a.split_once('=').map_or(a, |x| x.1).to_string()).collect();
        vals.sort();
        format!("{name}|{}", vals.join(","))
    }
}

fn load() -> (BTreeMap<String, Route>, BTreeMap<String, Route>) {
    let text = std::fs::read_to_string(data("production/routes.jsonl")).unwrap();
    let mut routes = BTreeMap::new();
    let mut cells = BTreeMap::new();
    for line in text.lines() {
        let v = json::parse(line.as_bytes()).unwrap();
        let path = v.get("path").and_then(Value::as_str).unwrap().to_string();
        let status: u16 = v.get("status").and_then(Value::as_number_text).unwrap().parse().unwrap();
        let ct = v.get("ct").and_then(Value::as_str).unwrap_or("application/json").to_string();
        let body = v.get("body").and_then(Value::as_str).unwrap().as_bytes().to_vec();
        if let Some(rest) = path.strip_prefix("/cell/") {
            cells.insert(cell_key(rest), Route { status, ct: ct.clone(), body: body.clone() });
        }
        // The register's cells: the members of each method's slice.
        if path.starts_with("/slice/datumesh.com/register/dataset/") && status == 200 {
            if let Some(Value::Array(members)) = json::parse(&body).unwrap().get("members") {
                for m in members {
                    let r = m.get("receipt").unwrap();
                    let c = r.get("cell").unwrap();
                    let arg = |k: &str| {
                        c.get("args").unwrap().get(k).unwrap().get("value").and_then(Value::as_str).unwrap().to_string()
                    };
                    let f = c.get("fn").and_then(Value::as_str).unwrap();
                    let mut vals = [arg("source"), arg("code")];
                    vals.sort();
                    cells.insert(
                        format!("datumesh.com/register/dataset/{f}|{}", vals.join(",")),
                        Route { status: 200, ct: "application/json".into(), body: m.to_jcs() },
                    );
                }
            }
        }
        routes.insert(canon(&path), Route { status, ct, body });
    }
    (routes, cells)
}

fn with_order(body: &[u8], order: &[&str]) -> Vec<u8> {
    let Value::Object(mut m) = json::parse(body).unwrap() else { panic!("describe is an object") };
    m.push(("order".into(), Value::Array(order.iter().map(|o| Value::String((*o).into())).collect())));
    m.sort_by(|a, b| a.0.cmp(&b.0));
    Value::Object(m).to_jcs()
}

/// An interval answer whose second member's receipt signature is broken (one hex digit changed).
fn break_second_member(body: &[u8]) -> Vec<u8> {
    let Value::Object(mut m) = json::parse(body).unwrap() else { panic!("an object") };
    for (k, v) in m.iter_mut() {
        if k != "members" {
            continue;
        }
        let Value::Array(ms) = v else { continue };
        let Some(Value::Object(member)) = ms.get_mut(1) else { continue };
        for (mk, mv) in member.iter_mut() {
            let Value::Object(r) = mv else { continue };
            if mk != "receipt" {
                continue;
            }
            for (rk, rv) in r.iter_mut() {
                if let (true, Value::String(sig)) = (rk == "sig", &mut *rv) {
                    let flipped = if sig.starts_with('0') { '1' } else { '0' };
                    sig.replace_range(0..1, &flipped.to_string());
                }
            }
        }
    }
    Value::Object(m).to_jcs()
}

fn answer(
    routes: &BTreeMap<String, Route>,
    cells: &BTreeMap<String, Route>,
    describe_order: bool,
    tamper: bool,
    target: &str,
) -> Route {
    let not_found = Route {
        status: 404,
        ct: "application/json".into(),
        body: br#"{"error":"E_NAME_UNBOUND","outcome":"not_covered"}"#.to_vec(),
    };
    let c = canon(target);
    if let Some(rest) = target.strip_prefix("/cell/") {
        return cells.get(&cell_key(rest)).cloned().unwrap_or(not_found);
    }
    // The register's slices, in whatever form of the slice call.
    if target.starts_with("/slice/") && c.contains("register/dataset") && c.contains('*') {
        for m in METHODS {
            if c.contains(&format!("dataset/{m}"))
                || c.contains(&format!("dataset(source:*,code:*).{m}"))
                || c.contains(&format!(").{m}"))
            {
                let key = format!("/slice/datumesh.com/register/dataset/{m}(source:*,code:*)");
                if let Some(r) = routes.get(&canon(&key)) {
                    return r.clone();
                }
            }
        }
    }
    let Some(r) = routes.get(&c) else { return not_found };
    if tamper && target.starts_with("/x/interval/") && r.status == 200 {
        return Route { status: r.status, ct: r.ct.clone(), body: break_second_member(&r.body) };
    }
    if describe_order && target.starts_with("/describe/") && r.status == 200 {
        let order: &[&str] = if c.ends_with("/par_yield") {
            &["tenor", "date"]
        } else if c.ends_with("/bank_rate") {
            &["date"]
        } else if c.ends_with("/fx_reference") {
            &["currency", "date"]
        } else {
            return r.clone();
        };
        return Route { status: r.status, ct: r.ct.clone(), body: with_order(&r.body, order) };
    }
    r.clone()
}

/// Bank Rate as a slice (local 0.5.2, item 12). The fake's whole slice of
/// `datumesh.com/relay/bankofengland.co.uk/bank_rate` is the recorded window `2026-09-01..2026-11-20`
/// (59 business days, production's own receipts, captured by `/x/interval`); production's is
/// 13,118 members from 1975, which no test needs.
///
/// * `/slice/<bank_rate with date open>`: JSON pages in production's form (`fn_hash`, `members`,
///   `next`, `outcome`, `slice`), page size `limit` (production's default is 1,000; the fake's is
///   20, so that paging is exercised), and `next` written as production writes it:
///   `https://datumesh.com/slice/<fn>?date=*&asof=<instant>&limit=<n>&after=<hex of the JSON array
///   of the last key's typed scalars>`.
/// * `/slice/<bank_rate with a date range>`: production has no URI for a ranged slice (SPEC 4.4:
///   slice objects "have no cell URI in this revision") and answers 400 `E_PARSE_URI`, as recorded;
///   the fake answers the same.
/// * `/x/interval/<bank_rate>?from=&to=`: the members of the window between the two dates, in
///   production's `/x/interval` form.
pub struct Bank {
    /// (date, the `/slice` member envelope, the `/x/interval` member).
    members: Vec<(String, Value, Value)>,
    /// Production's answer to a ranged slice (recorded).
    ranged: Route,
    served: std::sync::atomic::AtomicUsize,
}

const BANK_FN: &str = "datumesh.com/relay/bankofengland.co.uk/bank_rate";
const BANK_WINDOW: &str = "/x/interval/datumesh.com/relay/bankofengland.co.uk/bank_rate?from=2026-09-01&to=2026-11-20";
const BANK_RANGED: &str = "/slice/datumesh.com/relay/bankofengland.co.uk/bank_rate(date:[2026-10-06..2026-11-05])";
const BANK_ASOF: &str = "2026-11-24T20:40:59.817138Z";

fn recorded(path: &str) -> Route {
    let text = std::fs::read_to_string(data("production/routes.jsonl")).unwrap();
    for line in text.lines() {
        let v = json::parse(line.as_bytes()).unwrap();
        if v.get("path").and_then(Value::as_str) == Some(path) {
            return Route {
                status: v.get("status").and_then(Value::as_number_text).unwrap().parse().unwrap(),
                ct: v.get("ct").and_then(Value::as_str).unwrap().to_string(),
                body: v.get("body").and_then(Value::as_str).unwrap().as_bytes().to_vec(),
            };
        }
    }
    panic!("{path} is not in routes.jsonl: run tools/capture_production_fixtures.py")
}

fn bank_load() -> Vec<(String, Value, Value)> {
    let body = json::parse(&recorded(BANK_WINDOW).body).unwrap();
    let Some(Value::Array(ms)) = body.get("members") else { panic!("the window has members") };
    ms.iter()
        .map(|m| {
            let r = m.get("receipt").unwrap().clone();
            let date = m.get("keys").unwrap().get("date").and_then(Value::as_str).unwrap().to_string();
            let fn_hash = r.get("cell").unwrap().get("fn_hash").cloned().unwrap();
            let env = Value::Object(vec![
                ("fn_hash".into(), fn_hash),
                ("outcome".into(), Value::String("ok".into())),
                ("receipt".into(), r),
                ("trust".into(), Value::String("relayed".into())),
            ]);
            (date, env, m.clone())
        })
        .collect()
}

/// Whether `s` names a date range (`lo..hi`) anywhere.
fn has_range(s: &str) -> bool {
    let b = s.as_bytes();
    let is_date = |i: usize| {
        i + 10 <= b.len()
            && b[i..i + 10]
                .iter()
                .enumerate()
                .all(|(k, c)| if k == 4 || k == 7 { *c == b'-' } else { c.is_ascii_digit() })
    };
    (0..b.len()).any(|i| is_date(i) && s[i + 10..].starts_with("..") && is_date(i + 12))
}

/// A JSON number (numbers come only from parsing).
fn num(n: usize) -> Value {
    json::parse(n.to_string().as_bytes()).unwrap()
}

fn param(q: &str, k: &str) -> Option<String> {
    q.split('&').find_map(|p| p.strip_prefix(&format!("{k}=")).map(str::to_string))
}

fn unhex(h: &str) -> Option<Vec<u8>> {
    (0..h.len()).step_by(2).map(|i| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok())).collect()
}

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The date of production's `after` cursor (hex of `[{"type":"date","value":"<date>"}]`).
fn after_date(h: &str) -> Option<String> {
    let v = json::parse(&unhex(h)?).ok()?;
    let Value::Array(a) = v else { return None };
    a.first()?.get("value").and_then(Value::as_str).map(str::to_string)
}

impl Bank {
    fn respond(&self, target: &str) -> Option<Route> {
        let d = decode(target);
        if !d.contains(BANK_FN) {
            return None;
        }
        let (path, query) = d.split_once('?').unwrap_or((&d, ""));
        let json_ok = |body: Value| Route { status: 200, ct: "application/json".into(), body: body.to_jcs() };
        if path.starts_with("/x/interval/") {
            let (from, to) = (param(query, "from")?, param(query, "to")?);
            let picked: Vec<&(String, Value, Value)> =
                self.members.iter().filter(|m| m.0 >= from && m.0 <= to).collect();
            self.served.fetch_add(picked.len(), Ordering::SeqCst);
            return Some(json_ok(Value::Object(vec![
                ("asof".into(), Value::String(BANK_ASOF.into())),
                ("count".into(), num(picked.len())),
                ("from".into(), Value::String(from)),
                ("function".into(), Value::String(BANK_FN.into())),
                ("members".into(), Value::Array(picked.iter().map(|m| m.2.clone()).collect())),
                ("to".into(), Value::String(to)),
            ])));
        }
        if !path.starts_with("/slice/") {
            return None;
        }
        if has_range(&d) {
            return Some(self.ranged.clone());
        }
        let limit: usize = param(query, "limit").and_then(|l| l.parse().ok()).filter(|n| *n > 0).unwrap_or(20);
        let after = param(query, "after").and_then(|h| after_date(&h));
        let rest: Vec<&(String, Value, Value)> =
            self.members.iter().filter(|m| after.as_ref().is_none_or(|a| &m.0 > a)).collect();
        let page = &rest[..rest.len().min(limit)];
        self.served.fetch_add(page.len(), Ordering::SeqCst);
        let fn_hash = self.members[0].1.get("fn_hash").cloned().unwrap();
        let mut o = vec![
            ("fn_hash".to_string(), fn_hash.clone()),
            ("members".to_string(), Value::Array(page.iter().map(|m| m.1.clone()).collect())),
        ];
        if rest.len() > limit {
            let key = format!(r#"[{{"type":"date","value":"{}"}}]"#, page.last().unwrap().0);
            let next = format!(
                "https://datumesh.com/slice/{BANK_FN}?date=*&asof={BANK_ASOF}&limit={limit}&after={}",
                hex_of(key.as_bytes())
            );
            o.push(("next".into(), Value::String(next)));
        }
        o.push(("outcome".into(), Value::String("ok".into())));
        let (ns, f) = BANK_FN.rsplit_once('/').unwrap();
        o.push((
            "slice".into(),
            Value::Object(vec![
                ("args".into(), Value::Object(vec![("date".into(), Value::String("*".into()))])),
                ("fn".into(), Value::String(f.into())),
                ("fn_hash".into(), fn_hash),
                ("ns".into(), Value::String(ns.into())),
            ]),
        ));
        Some(json_ok(Value::Object(o)))
    }
}

#[allow(clippy::too_many_arguments)]
fn serve(
    mut s: TcpStream,
    routes: &BTreeMap<String, Route>,
    cells: &BTreeMap<String, Route>,
    log: &Mutex<Vec<Req>>,
    order: &AtomicBool,
    tamper: &AtomicBool,
    bank: &Bank,
    faults: &Faults,
) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let line = head.lines().next().unwrap_or("");
    let mut parts = line.split(' ');
    let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let banked = if method == "GET" { faults.respond(&target).or_else(|| bank.respond(&target)) } else { None };
    let r = if let Some(r) = banked {
        r
    } else if method == "GET" {
        answer(routes, cells, order.load(Ordering::SeqCst), tamper.load(Ordering::SeqCst), &target)
    } else {
        Route {
            status: 405,
            ct: "application/json".into(),
            body: br#"{"error":"the fake production mesh answers GET only"}"#.to_vec(),
        }
    };
    log.lock().unwrap().push(Req { method, target, answered: r.status });
    let reason = if r.status == 200 { "OK" } else { "Other" };
    let _ = write!(
        s,
        "HTTP/1.1 {} {reason}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        r.status,
        r.ct,
        r.body.len()
    );
    let _ = s.write_all(&r.body);
}

impl ProdMesh {
    /// Starts the fake; `/describe` of a function carries `order` when `describe_order`.
    pub fn start(describe_order: bool) -> ProdMesh {
        let (routes, cells) = load();
        let (routes, cells) = (Arc::new(routes), Arc::new(cells));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let order = Arc::new(AtomicBool::new(describe_order));
        let tamper = Arc::new(AtomicBool::new(false));
        let bank = Arc::new(Bank {
            members: bank_load(),
            ranged: recorded(BANK_RANGED),
            served: std::sync::atomic::AtomicUsize::new(0),
        });
        let faults = Arc::new(Faults::default());
        let (l2, o2, t2, b2, f2) = (log.clone(), order.clone(), tamper.clone(), bank.clone(), faults.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let (routes, cells, log, order, tamper, bank, faults) =
                    (routes.clone(), cells.clone(), l2.clone(), o2.clone(), t2.clone(), b2.clone(), f2.clone());
                std::thread::spawn(move || serve(conn, &routes, &cells, &log, &order, &tamper, &bank, &faults));
            }
        });
        ProdMesh { origin, log, describe_order: order, tamper, bank, faults }
    }

    /// Bank Rate members served so far, by `/slice` pages and `/x/interval` answers.
    pub fn bank_served(&self) -> usize {
        self.bank.served.load(Ordering::SeqCst)
    }

    /// The decoded targets of the requests for Bank Rate members (`/slice/...` and `/x/interval/...`).
    pub fn bank_requests(&self) -> Vec<String> {
        self.targets()
            .into_iter()
            .filter(|t| (t.starts_with("/slice/") || t.starts_with("/x/interval/")) && t.contains("bank_rate"))
            .collect()
    }

    /// Break the signature of the second member of every interval answer.
    /// `/find?summary=1` answers 503 while `down` (item 37).
    pub fn fail_summary(&self, down: bool) {
        self.faults.summary_down.store(down, Ordering::SeqCst);
    }

    /// `/find?q=...` answers `status` (0: the empty answer of no match) (item 39).
    pub fn set_find_status(&self, status: u16) {
        self.faults.find_status.store(status, Ordering::SeqCst);
    }

    /// The `/find?summary=1` requests so far.
    pub fn summary_requests(&self) -> usize {
        self.targets().iter().filter(|t| t.as_str() == "/find?summary=1").count()
    }

    pub fn tamper_interval(&self, on: bool) {
        self.tamper.store(on, Ordering::SeqCst);
    }

    pub fn set_describe_order(&self, on: bool) {
        self.describe_order.store(on, Ordering::SeqCst);
    }

    /// Every request answered so far.
    pub fn requests(&self) -> Vec<Req> {
        self.log.lock().unwrap().clone()
    }

    /// The decoded targets of the requests so far.
    pub fn targets(&self) -> Vec<String> {
        self.requests().iter().map(|r| decode(&r.target)).collect()
    }

    pub fn posts(&self) -> Vec<Req> {
        self.requests().into_iter().filter(|r| r.method != "GET").collect()
    }

    /// Requests that found nothing recorded (a path the fixtures lack), for a failure message.
    pub fn unknown(&self) -> Vec<String> {
        self.requests().into_iter().filter(|r| r.answered == 404).map(|r| decode(&r.target)).collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Homes and the command line
// ---------------------------------------------------------------------------------------------

use crate::common::{in_dir_050, TempDir};
use datumesh_local::Config;

fn patch_peers(dir: &TempDir, peers: &[(&str, &str)]) {
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let list: Vec<String> = peers.iter().map(|(h, o)| format!("\"{h}\": \"{o}\"")).collect();
    assert!(
        text.contains("\"peers\": {}"),
        "the installed config.json has a `\"peers\": {{}}` member to point at the fakes"
    );
    std::fs::write(&path, text.replace("\"peers\": {}", &format!("\"peers\": {{{}}}", list.join(", ")))).unwrap();
}

/// A new home as 0.5.1 installs it (production defaults), its `datumesh.com` (and any other host
/// of `peers`) reached at the fakes' origins.
pub fn fresh_home(peers: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new("l051");
    Config::in_dir(dir.path()).unwrap();
    patch_peers(&dir, peers);
    dir
}

/// A home as 0.5.0 installed it (the test mesh as forward mesh), peers as `fresh_home`.
pub fn old_home(peers: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new("l051-old");
    in_dir_050(dir.path()).unwrap();
    patch_peers(&dir, peers);
    dir
}

/// One command line in process: (exit status, stdout, stderr).
pub fn cli(dir: &TempDir, args: &[&str]) -> (i32, String, String) {
    let mut all = vec!["--home".to_string(), dir.path().to_string_lossy().into_owned()];
    all.extend(args.iter().map(|a| a.to_string()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&all, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

/// The hash of the receipt of the recorded production cell whose `/cell/` path contains `needle`.
pub fn fixture_receipt_hash(needle: &str) -> String {
    let text = std::fs::read_to_string(data("production/routes.jsonl")).unwrap();
    for line in text.lines() {
        let v = json::parse(line.as_bytes()).unwrap();
        let path = v.get("path").and_then(Value::as_str).unwrap();
        if path.starts_with("/cell/") && path.contains(needle) {
            let body = json::parse(v.get("body").and_then(Value::as_str).unwrap().as_bytes()).unwrap();
            let r = body.get("receipt").unwrap();
            return datumesh_core::Receipt::from_json(&r.to_jcs()).unwrap().hash().to_string();
        }
    }
    panic!("no recorded cell {needle}");
}

/// Every object anywhere in `v` that has a member `key`.
pub fn objects_with<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            if m.iter().any(|(k, _)| k == key) {
                out.push(v);
            }
            for (_, x) in m {
                objects_with(x, key, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| objects_with(x, key, out)),
        _ => {}
    }
}

/// Whether `t` is a canonical instant (six fractional digits, `Z`).
pub fn is_canonical_instant(t: &str) -> bool {
    let b = t.as_bytes();
    t.len() == 27
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'.'
        && b[26] == b'Z'
        && t[..4]
            .bytes()
            .chain(t[5..7].bytes())
            .chain(t[8..10].bytes())
            .chain(t[11..13].bytes())
            .chain(t[14..16].bytes())
            .chain(t[17..19].bytes())
            .chain(t[20..26].bytes())
            .all(|c| c.is_ascii_digit())
}

/// What an agent is told when a tool refuses: the text of a tool result with `isError`, or the
/// message of a JSON-RPC error (either way is a refusal the agent reads).
pub fn agent_refusal(v: &Value) -> String {
    if let Some(r) = v.get("result") {
        assert_eq!(
            r.get("isError"),
            Some(&Value::Bool(true)),
            "a refusal, not an answer: {}",
            String::from_utf8_lossy(&v.to_jcs())
        );
        return String::from_utf8_lossy(&v.to_jcs()).into_owned();
    }
    let e = v
        .get("error")
        .unwrap_or_else(|| panic!("neither a result nor an error: {}", String::from_utf8_lossy(&v.to_jcs())));
    String::from_utf8_lossy(&e.to_jcs()).into_owned()
}

/// The Bank Rate window the fake serves as its whole slice: (date, value), ascending.
pub fn bank_window() -> Vec<(String, String)> {
    bank_load().iter().map(|m| (m.0.clone(), m.2.get("value").and_then(Value::as_str).unwrap().to_string())).collect()
}

/// The value of the last member of the window at or before `day`.
pub fn bank_value_on_or_before(day: &str) -> String {
    bank_window().into_iter().rfind(|(d, _)| d.as_str() <= day).expect("a member").1
}

/// `fresh_home`, with `"slice_fetch_max": <max>` in `config.json` (item 12's bound): its value
/// replaced when the installed file states it, else the member put after `"offline": false,`.
/// (`config.json` has `//` comment lines, so it is edited as text.)
pub fn fresh_home_bounded(peers: &[(&str, &str)], max: u64) -> TempDir {
    let dir = fresh_home(peers);
    let p = dir.path().join("config.json");
    let text = std::fs::read_to_string(&p).unwrap();
    let key = "\"slice_fetch_max\":";
    let patched = match text.find(key) {
        Some(at) => {
            let start = at + key.len();
            let rest = &text[start..];
            let skip = rest.len() - rest.trim_start().len();
            let digits = rest.trim_start().bytes().take_while(u8::is_ascii_digit).count();
            format!("{} {max}{}", &text[..start], &text[start + skip + digits..])
        }
        None => {
            assert!(
                text.contains("\"offline\": false,"),
                "config.json has `\"offline\": false,` to put the bound after"
            );
            text.replacen("\"offline\": false,", &format!("\"offline\": false,\n  \"slice_fetch_max\": {max},"), 1)
        }
    };
    std::fs::write(&p, patched).unwrap();
    dir
}
