//! datumesh-local 0.5.5, the agent tools (CONTRACT-patch-0.6.1 item 8a.8: NEXT-PATCH-QUEUE items 17 to 27 and
//! the user's item 1 of 26 Nov, the 4 KiB small profile). Written before the implementation. The readings these
//! tests take, item by item, are in CONTRACT-local-0.5.5.md ("Test writer's readings").
//!
//! The worlds:
//! * `find_world`: the test mesh of `tests/common` (ECB-shaped `fx_rate(base, quote, date)`, business days from
//!   2026-09-21 to 2026-10-30, quotes CHF, GBP, JPY and USD) behind a stub `/find` (`AnyFind`) that answers every
//!   query with the same fixed results, whatever the query's text: the test does not decide what the implementation
//!   asks find. The test mesh's own find holds nothing (its catalogue is empty), so a stub is needed for any match.
//! * `prod_world`: the production fake (`tests/support/prod_mesh.rs`), for the interval, unlicensed and
//!   names-not-held cases; its find answers no match for every query.
//! * `level_world`: `local/level(day)`, a local stored function with 60 cells (as `local_053_agent_answers.rs`).
//!
//! The answer budgets: an answer is the whole JSON-RPC response line, as `local_053_agent_answers.rs` measures it.
//! The small profile is held to "about 4 KiB": `SMALL_BUDGET` is 4 KiB plus an eighth (4,608 bytes). The full
//! profile keeps 64 KiB (`BUDGET`).
//!
//! Nothing reaches the network.

#![cfg(unix)]

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_core::Receipt;
use datumesh_local::mcp::{McpServer, Profile, Transport};
use datumesh_local::{Config, Resolver};
use tools::{arr, is_error, items, jcs, q, result, rpc_on, structured, M};

const SMALL_BUDGET: usize = 4 * 1024 + 512;
const BUDGET: usize = 64 * 1024;

const FX: &str = "mesh-test.datumesh.com/ecb/fx_rate";
/// A Sunday: no cell. The nearest business days are the Friday before and the Monday after.
const SUNDAY: &str = "2026-09-27";
const FRIDAY: &str = "2026-09-25";
const MONDAY: &str = "2026-09-28";
const QUOTES: [&str; 4] = ["CHF", "GBP", "JPY", "USD"];
/// A label as agents gave one (C/analysis/refused_calls.jsonl, E_PARSE_URI and E_KEY_ENCODING), in the test
/// mesh's terms.
const LABEL: &str = "Euro foreign exchange reference rate (US dollar)";
/// A name in another namespace, among the stub's find results: never a near name of `ecb/...`.
const ESTR: &str = "datumesh.com/relay/ecb.europa.eu/estr";

/// Production's unlicensed spread (the Treasury's licence is restricted, Bank Rate's open).
const SPREAD: &str = "datumesh.com/relay/treasury.gov/par_yield(10Y, 2026-11-20) - datumesh.com/relay/bankofengland.co.uk/bank_rate(2026-11-20)";
const BANK_SLICE: &str = "datumesh.com/relay/bankofengland.co.uk/bank_rate(date: *)";
/// The fake's Bank Rate window: 59 business days.
const BANK_MEMBERS: usize = 59;

fn usd(date: &str) -> String {
    format!(r#"ecb/fx_rate(base: "EUR", quote: "USD", date: {date})"#)
}

// =============================================================================== the calls

/// A `tools/call`: the response as parsed, and its bytes (the whole JSON-RPC line).
fn call(srv: &McpServer, name: &str, args: &str) -> (Value, String) {
    let line = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{M},"name":{},"arguments":{args}}}}}"#,
        q(name)
    );
    let out = srv.handle(&line, Transport::Stdio).expect("a response");
    (json::parse(out.as_bytes()).unwrap_or_else(|e| panic!("{out}: {e:?}")), out)
}

fn get(srv: &McpServer, request: &str) -> (Value, String) {
    call(srv, "get", &format!(r#"{{"request":{}}}"#, q(request)))
}

fn calc(srv: &McpServer, formula: &str) -> (Value, String) {
    call(srv, "calc", &format!(r#"{{"formula":{}}}"#, q(formula)))
}

fn describe(srv: &McpServer, id: &str) -> (Value, String) {
    call(srv, "describe", &format!(r#"{{"id":{}}}"#, q(id)))
}

/// The text an agent reads: the content items, together.
fn text(v: &Value) -> String {
    items(v).join("\n")
}

fn within(what: &str, line: &str, budget: usize) {
    assert!(line.len() <= budget, "{what}: {} bytes, over the budget of {budget}: {line}", line.len());
}

fn number(v: Option<&Value>) -> u64 {
    v.and_then(Value::as_number_text).and_then(|t| t.parse().ok()).unwrap_or_else(|| panic!("a number: {v:?}"))
}

fn tools_of(srv: &McpServer) -> Vec<Value> {
    let v = rpc_on(srv, Transport::Stdio, "tools/list", "");
    arr(result(&v), "tools").to_vec()
}

fn tool_def(srv: &McpServer, name: &str) -> Value {
    tools_of(srv).into_iter().find(|t| s(t, "name") == name).unwrap_or_else(|| panic!("no {name} tool"))
}

/// The date of a member's call (`date: YYYY-MM-DD`).
fn date_of_call(call: &str) -> String {
    let at = call.find("date: ").unwrap_or_else(|| panic!("a date in {call}")) + 6;
    call[at..at + 10].to_string()
}

/// A compact row (item 18): its call, value and receipt hash (its trust allowed), and nothing else: no whole
/// receipt, no `asof` (the answer's own), no `keys` (already in the call).
fn assert_compact_rows(what: &str, members: &[Value]) {
    assert!(!members.is_empty(), "{what}: no rows");
    for m in members {
        let Value::Object(pairs) = m else { panic!("{what}: a row is an object: {}", jcs(m)) };
        for (k, _) in pairs {
            assert!(
                matches!(k.as_str(), "call" | "value" | "receipt_hash" | "trust"),
                "{what}: a row carries only its call, value and receipt hash, not {k}: {}",
                jcs(m)
            );
        }
        assert!(!s(m, "call").is_empty(), "{what}: its call: {}", jcs(m));
        assert!(m.get("value").is_some(), "{what}: its value: {}", jcs(m));
        let h = s(m, "receipt_hash");
        assert!(h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit()), "{what}: its receipt hash: {}", jcs(m));
    }
}

// =============================================================================== the find stub

/// The test mesh with `/find` answering every query with `results` (and `resolution: none`), and the catalogue
/// summary with a 500. Every other path is the test mesh's own.
struct AnyFind {
    origin: String,
    queries: Arc<Mutex<Vec<String>>>,
    _rt: tokio::runtime::Runtime,
}

impl AnyFind {
    fn start(up: &Upstream, results: Vec<Value>) -> AnyFind {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let queries = Arc::new(Mutex::new(Vec::new()));
        let kept = queries.clone();
        let results = Arc::new(Value::Array(results));
        let handler = move |req: axum::extract::Request| {
            let kept = kept.clone();
            let results = results.clone();
            async move {
                let raw = req.uri().query().unwrap_or("").to_string();
                let (status, body) = if raw.split('&').any(|p| p.starts_with("summary=")) {
                    (500, r#"{"error":"no summary"}"#.to_string())
                } else {
                    kept.lock().unwrap().push(raw.clone());
                    let body = format!(
                        r#"{{"query":"x","vector":false,"results":{},"resolution":{{"status":"none"}}}}"#,
                        jcs(&results)
                    );
                    (200, body)
                };
                axum::http::Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap()
            }
        };
        let router = axum::Router::new().route("/find", axum::routing::get(handler)).fallback_service(up.mesh.router());
        rt.spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        AnyFind { origin, queries, _rt: rt }
    }
}

/// A find result in datumesh-http's shape.
fn found(name: &str, fn_hash: &str, label: &str, description: &str, descriptor: &Value) -> Value {
    json::parse(
        format!(
            r#"{{"kind":"function","name":{},"fn_hash":{},"label":{},"description":{},"descriptor":{}}}"#,
            q(name),
            q(fn_hash),
            q(label),
            q(description),
            jcs(descriptor)
        )
        .as_bytes(),
    )
    .unwrap()
}

/// The upstream's binding of `name`: its hash and descriptor.
fn bound(up: &Upstream, name: &str) -> (String, Value) {
    let b = up.store.resolve_binding(name, END).unwrap().unwrap_or_else(|| panic!("{name} is bound"));
    let d = up.store.descriptor(&b.fn_hash).expect("its descriptor");
    (b.fn_hash.clone(), json::parse(&d).unwrap())
}

/// The find results of the test mesh's namespace: `fx_rate` first (the top match of `LABEL`), `holiday`, and
/// `estr` of another namespace.
fn ecb_results(up: &Upstream) -> Vec<Value> {
    let (fx_hash, fx_d) = bound(up, FX);
    let (hol_hash, hol_d) = bound(up, "mesh-test.datumesh.com/ecb/holiday");
    let estr_d = json::parse(br#"{"args":{"date":{"type":"date"}},"order":["date"],"returns":"number"}"#).unwrap();
    vec![
        found(
            FX,
            &fx_hash,
            "Euro foreign exchange reference rate",
            "The ECB's euro foreign exchange reference rate of a currency on a day",
            &fx_d,
        ),
        found("mesh-test.datumesh.com/ecb/holiday", &hol_hash, "TARGET holiday", "A TARGET2 closing day", &hol_d),
        found(ESTR, &"e".repeat(64), "Euro short-term rate (€STR)", "The euro short-term rate of a day", &estr_d),
    ]
}

struct FW {
    up: Upstream,
    stub: AnyFind,
    _r: Arc<Resolver>,
    full: McpServer,
    small: McpServer,
    _dir: TempDir,
}

/// The test mesh behind the find stub; `results` (else `ecb_results`) is every find answer.
fn find_world(results: Option<Vec<Value>>) -> FW {
    let up = upstream();
    let results = results.unwrap_or_else(|| ecb_results(&up));
    let stub = AnyFind::start(&up, results);
    let dir = TempDir::new("l055");
    let mut c = config(&dir, &up);
    c.peers.insert(HOST.to_string(), stub.origin.clone());
    c.find_meshes = vec![HOST.to_string()];
    let r = Arc::new(Resolver::open(c).unwrap());
    let full = McpServer::new(r.clone());
    let small = McpServer::with_profile(r.clone(), Profile::Small);
    FW { up, stub, _r: r, full, small, _dir: dir }
}

struct PW {
    prod: prod_mesh::ProdMesh,
    full: McpServer,
    small: McpServer,
    _dir: TempDir,
}

fn prod_world() -> PW {
    let prod = prod_mesh::ProdMesh::start(true);
    let dir = prod_mesh::fresh_home(&[("datumesh.com", &prod.origin)]);
    let r = Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap());
    let full = McpServer::new(r.clone());
    let small = McpServer::with_profile(r, Profile::Small);
    PW { prod, full, small, _dir: dir }
}

// =============================================================================== item 17

#[test]
fn i17_get_s_description_points_to_interval_and_to_a_calc_over_a_range_in_both_profiles() {
    let w = find_world(None);
    for (profile, srv) in [("full", &w.full), ("small", &w.small)] {
        let d = s(&tool_def(srv, "get"), "description");
        assert!(d.contains("interval"), "{profile}: get names interval for many dates: {d}");
        assert!(d.contains("calc"), "{profile}: get names a calc over a range: {d}");
        // One example of each: a range of keys is written with `*` (a slice) or `..` (a range).
        assert!(d.contains('*') || d.contains(".."), "{profile}: the calc example is over a range: {d}");
    }
}

/// The content lines of an answer that mention interval.
fn interval_lines(v: &Value) -> Vec<String> {
    text(v).lines().filter(|l| l.contains("interval")).map(str::to_string).collect()
}

#[test]
fn i17_a_get_after_two_gets_of_the_same_function_at_other_dates_says_so_in_one_line() {
    let w = find_world(None);
    for (n, date) in ["2026-10-28", "2026-10-29"].iter().enumerate() {
        let (v, _) = get(&w.full, &usd(date));
        assert!(!is_error(&v), "{}", jcs(&v));
        assert!(interval_lines(&v).is_empty(), "get {}: nothing said yet: {}", n + 1, text(&v));
    }
    // The third of the same function, at a third date: one line says many dates are read with interval or calc.
    let (v, _) = get(&w.full, &usd("2026-10-30"));
    assert!(!is_error(&v), "{}", jcs(&v));
    let lines = interval_lines(&v);
    assert_eq!(lines.len(), 1, "one line: {}", text(&v));
    assert!(lines[0].contains("calc"), "it names interval and calc: {}", lines[0]);
    // The answer is still the answer: its value is there.
    assert!(!s(structured(&v), "receipt_hash").is_empty(), "{}", jcs(&v));
    // A get of another function says nothing of it.
    let (v, _) = get(&w.full, CHRISTMAS);
    assert!(!is_error(&v), "{}", jcs(&v));
    assert!(interval_lines(&v).is_empty(), "another function: {}", text(&v));
}

#[test]
fn i17_gets_of_one_date_again_are_not_several_dates() {
    let w = find_world(None);
    for _ in 0..3 {
        let (v, _) = get(&w.full, &usd("2026-10-30"));
        assert!(!is_error(&v), "{}", jcs(&v));
        assert!(interval_lines(&v).is_empty(), "the same date again: {}", text(&v));
    }
}

// =============================================================================== item 18

#[test]
fn i18_an_interval_answer_is_compact_rows_in_both_profiles() {
    let w = prod_world();
    let args = r#"{"function":"treasury/par_yield","keys":{"tenor":"10Y"},"from":"2026-11-16","to":"2026-11-24"}"#;
    for (profile, srv, budget) in [("full", &w.full, BUDGET), ("small", &w.small, SMALL_BUDGET)] {
        let (v, line) = call(srv, "interval", args);
        assert!(!is_error(&v), "{profile}: {}", jcs(&v));
        within(profile, &line, budget);
        let members = arr(structured(&v), "members");
        assert_eq!(members.len(), 6, "{profile}: {}", jcs(structured(&v)));
        assert_compact_rows(profile, members);
        assert_eq!(date_of_call(&s(&members[0], "call")), "2026-11-16");
    }
}

#[test]
fn i18_a_long_interval_is_compact_rows_within_each_budget() {
    let w = prod_world();
    let args = r#"{"function":"ecb/fx_reference","keys":{"currency":"USD"},"from":"2024-01-01","to":"2024-06-30"}"#;
    for (profile, srv, budget) in [("full", &w.full, BUDGET), ("small", &w.small, SMALL_BUDGET)] {
        let (v, line) = call(srv, "interval", args);
        assert!(!is_error(&v), "{profile}: {}", jcs(&v));
        within(&format!("{profile} interval (126 members)"), &line, budget);
        assert_compact_rows(profile, arr(structured(&v), "members"));
        assert_eq!(number(structured(&v).get("count")), 126, "{profile}: the count of all");
    }
}

#[test]
fn i18_a_slice_answer_is_compact_rows_within_each_budget() {
    let w = prod_world();
    for (profile, srv, budget) in [("full", &w.full, BUDGET), ("small", &w.small, SMALL_BUDGET)] {
        let (v, line) = get(srv, BANK_SLICE);
        assert!(!is_error(&v), "{profile}: {}", jcs(&v));
        within(&format!("{profile} slice"), &line, budget);
        assert_compact_rows(profile, arr(structured(&v), "members"));
    }
}

// =============================================================================== items 19 and 25

#[test]
fn i19_i25_an_unlicensed_answer_names_the_command_and_remote_in_both_profiles() {
    let w = prod_world();
    for (profile, srv) in [("full", &w.full), ("small", &w.small)] {
        let (v, _) = calc(srv, SPREAD);
        assert!(is_error(&v), "{profile}: {}", jcs(&v));
        let t = text(&v);
        assert!(t.contains("unlicensed"), "{profile}: {t}");
        // Item 19 (already so in 0.5.2 to 0.5.4).
        assert!(
            t.contains(&format!("datumesh licence accept '{}'", prod_mesh::TREASURY_LICENCE)),
            "{profile}: the command the user runs: {t}"
        );
        // Item 25: the calculation can be run on the mesh with remote.
        assert!(t.contains("remote"), "{profile}: says remote runs it on the mesh: {t}");
    }
    assert!(w.prod.posts().is_empty(), "nothing was sent for the user");
}

#[test]
fn i25_the_small_profile_s_calc_declares_remote() {
    let w = prod_world();
    let t = tool_def(&w.small, "calc");
    let remote = t
        .get("inputSchema")
        .and_then(|x| x.get("properties"))
        .and_then(|p| p.get("remote"))
        .unwrap_or_else(|| panic!("calc declares remote: {}", jcs(&t)));
    assert_eq!(s(remote, "type"), "boolean", "{}", jcs(remote));
    // The full profile's calc already does.
    let full = tool_def(&w.full, "calc");
    assert!(full.get("inputSchema").and_then(|x| x.get("properties")).and_then(|p| p.get("remote")).is_some());
}

// =============================================================================== items 20 and 24

/// The receipt hash of a get answer (the answer block's).
fn hash_of(v: &Value) -> String {
    assert!(!is_error(v), "{}", jcs(v));
    let h = s(structured(v), "receipt_hash");
    assert_eq!(h.len(), 64, "a receipt hash: {}", jcs(v));
    h
}

#[test]
fn i20_get_takes_a_date_as_canonical_text() {
    let w = find_world(None);
    let want = hash_of(&get(&w.full, &usd("2026-10-30")).0);
    for (profile, srv) in [("full", &w.full), ("small", &w.small)] {
        let (v, _) = get(srv, &usd(r#""2026-10-30""#));
        assert_eq!(hash_of(&v), want, "{profile}: the same cell as date: 2026-10-30");
    }
    // Another function that declares a date: holiday.
    let (v, _) = get(&w.full, r#"ecb/holiday(calendar: "TARGET", date: "2026-12-25")"#);
    assert_eq!(hash_of(&v), hash_of(&get(&w.full, CHRISTMAS).0));
}

#[test]
fn i20_calc_takes_a_date_as_canonical_text() {
    let w = find_world(None);
    let full = |date: &str| format!(r#"{FX}(base: "EUR", quote: "USD", date: {date}) * 2"#);
    let plain = calc(&w.full, &full("2026-10-30")).0;
    assert!(!is_error(&plain), "{}", jcs(&plain));
    let (v, _) = calc(&w.full, &full(r#""2026-10-30""#));
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_eq!(s(structured(&v), "value"), s(structured(&plain), "value"), "{}", jcs(&v));
}

#[test]
fn i20_a_date_text_not_in_the_canonical_form_is_still_refused() {
    let w = find_world(None);
    for bad in [r#""2026-10-3""#, r#""30 Oct 2026""#, r#""2026/10/30""#] {
        let (v, _) = get(&w.full, &usd(bad));
        assert!(is_error(&v), "{bad}: {}", jcs(&v));
        assert!(text(&v).contains("E_CANON_TYPE"), "{bad}: {}", text(&v));
    }
}

#[test]
fn i24_get_takes_the_date_form_calc_takes() {
    let w = find_world(None);
    let want = hash_of(&get(&w.full, &usd("2026-10-30")).0);
    for form in ["date(2026, 10, 30)", "date(2026,10,30)"] {
        for (profile, srv) in [("full", &w.full), ("small", &w.small)] {
            let (v, _) = get(srv, &usd(form));
            assert_eq!(hash_of(&v), want, "{profile}: {form}");
        }
    }
}

/// `datumesh get <request>` on a home as 0.5.0 made it, reaching the test mesh: (exit, the receipt's hash or "").
fn cli_get(dir: &TempDir, request: &str) -> (i32, String, String) {
    let (code, out, err) = prod_mesh::cli(dir, &["get", request]);
    let hash = json::parse(out.trim().as_bytes())
        .ok()
        .and_then(|v| v.get("receipt").cloned())
        .and_then(|r| Receipt::from_value(&r).ok())
        .map(|r| r.hash().to_string())
        .unwrap_or_default();
    (code, hash, format!("{out}\n{err}"))
}

#[test]
fn i20_i24_the_command_line_get_takes_the_date_as_text_and_as_date() {
    // Item 20 asks whether the released CLI already accepts the text: this test is the answer (0.5.4: no, exit 3
    // with E_CANON_TYPE).
    let up = upstream();
    let dir = prod_mesh::old_home(&[(HOST, &up.origin)]);
    let (code, want, all) = cli_get(&dir, &usd("2026-10-30"));
    assert_eq!(code, 0, "{all}");
    assert_eq!(want.len(), 64, "{all}");
    let (code, got, all) = cli_get(&dir, &usd(r#""2026-10-30""#));
    assert_eq!((code, got), (0, want.clone()), "item 20, canonical text: {all}");
    let (code, got, all) = cli_get(&dir, &usd("date(2026, 10, 30)"));
    assert_eq!((code, got), (0, want), "item 24, date(...): {all}");
}

// =============================================================================== item 21

/// The first lookup formula a text names: `last_before(`, `first_after(` or `nearest(` up to its closing paren.
fn lookup_in(t: &str) -> Option<String> {
    let at = ["last_before(", "first_after(", "nearest("].iter().filter_map(|f| t.find(f)).min()?;
    let mut depth = 0;
    for (i, c) in t[at..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(t[at..=at + i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn assert_names_the_missing_cell(what: &str, v: &Value) {
    assert!(is_error(v), "{what}: {}", jcs(v));
    let t = text(v);
    assert!(t.contains("fx_rate") && t.contains(SUNDAY), "{what}: names the cell: {t}");
    assert!(t.contains(FRIDAY) && t.contains(MONDAY), "{what}: its nearest dates, before and after: {t}");
    let lookup = lookup_in(&t).unwrap_or_else(|| panic!("{what}: the lookup that would answer: {t}"));
    assert!(lookup.contains("fx_rate") && lookup.contains("date: *"), "{what}: a lookup over the series: {lookup}");
    assert!(lookup.contains(SUNDAY), "{what}: at the date asked for: {lookup}");
    assert!(lookup.contains("USD"), "{what}: of the same currency: {lookup}");
}

#[test]
fn i21_a_missing_cell_is_named_with_its_nearest_dates_and_a_lookup_get() {
    let w = find_world(None);
    for (profile, srv) in [("full", &w.full), ("small", &w.small)] {
        let (v, _) = get(srv, &usd(SUNDAY));
        assert_names_the_missing_cell(&format!("{profile} get"), &v);
    }
}

#[test]
fn i21_a_missing_cell_is_named_with_its_nearest_dates_and_a_lookup_calc() {
    let w = find_world(None);
    let formula = format!(r#"{FX}(base: "EUR", quote: "USD", date: {SUNDAY}) * 2"#);
    for (profile, srv) in [("full", &w.full), ("small", &w.small)] {
        let (v, _) = calc(srv, &formula);
        assert!(text(&v).contains("E_FORMULA_ABSENT"), "{profile}: the code stays: {}", text(&v));
        assert_names_the_missing_cell(&format!("{profile} calc"), &v);
    }
}

#[test]
fn i21_a_wrong_key_value_lists_the_values_the_function_takes() {
    // As par_yield(tenor: "1 year") for 1Y: here the currency's name for its code.
    let w = find_world(None);
    let request = r#"ecb/fx_rate(base: "EUR", quote: "US Dollar", date: 2026-10-30)"#;
    let formula = format!(r#"{FX}(base: "EUR", quote: "US Dollar", date: 2026-10-30) * 2"#);
    for (profile, srv) in [("full", &w.full), ("small", &w.small)] {
        for (tool, v) in [("get", get(srv, request).0), ("calc", calc(srv, &formula).0)] {
            assert!(is_error(&v), "{profile} {tool}: {}", jcs(&v));
            let t = text(&v);
            assert!(t.contains("quote"), "{profile} {tool}: names the key: {t}");
            for code in QUOTES {
                assert!(t.contains(code), "{profile} {tool}: lists {code}: {t}");
            }
        }
    }
}

#[test]
fn i21_a_guessed_name_gets_the_near_names_in_its_namespace() {
    let w = find_world(None);
    let request = r#"ecb/fx_price(base: "EUR", quote: "USD", date: 2026-10-30)"#;
    let formula = r#"mesh-test.datumesh.com/ecb/fx_price(base: "EUR", quote: "USD", date: 2026-10-30) * 2"#;
    for (profile, srv) in [("full", &w.full), ("small", &w.small)] {
        for (tool, v) in [("get", get(srv, request).0), ("calc", calc(srv, formula).0)] {
            assert!(is_error(&v), "{profile} {tool}: {}", jcs(&v));
            let t = text(&v);
            assert!(t.contains(FX), "{profile} {tool}: the near name, in full: {t}");
            assert!(!t.contains(ESTR), "{profile} {tool}: only names of its namespace: {t}");
        }
    }
}

#[test]
fn i21_an_unqualified_name_in_calc_gets_the_full_name_from_find() {
    let w = find_world(None);
    for (profile, srv) in [("full", &w.full), ("small", &w.small)] {
        let (v, _) = calc(srv, r#"fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#);
        assert!(is_error(&v), "{profile}: {}", jcs(&v));
        let t = text(&v);
        assert!(t.contains("E_FORMULA_UNKNOWN_FUNCTION"), "{profile}: the code stays: {t}");
        assert!(t.contains(FX), "{profile}: did you mean the full name: {t}");
        assert!(!t.contains(ESTR), "{profile}: not a name of another function: {t}");
    }
}

// =============================================================================== item 22

fn assert_label_answer(what: &str, v: &Value) {
    assert!(is_error(v), "{what}: no value is answered: {}", jcs(v));
    assert!(s(structured(v), "receipt_hash").is_empty(), "{what}: nothing was read: {}", jcs(v));
    let t = text(v);
    assert!(t.to_lowercase().contains("looks like a label"), "{what}: says so: {t}");
    assert!(t.contains(FX), "{what}: carries the top match: {t}");
    assert!(jcs(structured(v)).contains(FX), "{what}: the matches are in the structured part too");
    assert!(structured(v).get("searched").is_none(), "{what}: it is not a find answer: {}", jcs(structured(v)));
}

#[test]
fn i22_a_label_given_to_get_is_not_read_and_carries_the_top_matches() {
    let w = find_world(None);
    for (profile, srv) in [("full", &w.full), ("small", &w.small)] {
        let (v, _) = get(srv, LABEL);
        assert_label_answer(&format!("{profile} get"), &v);
    }
}

#[test]
fn i22_a_label_given_to_describe_carries_the_top_matches() {
    let w = find_world(None);
    let (v, _) = describe(&w.full, LABEL);
    assert_label_answer("describe", &v);
    assert!(!w.stub.queries.lock().unwrap().is_empty(), "the matches came from the find mesh");
}

// =============================================================================== item 23

#[test]
fn i23_a_read_of_a_name_the_mesh_does_not_hold_says_so_and_points_to_find() {
    let w = prod_world();
    let request = r#"datumesh.com/sec/net_income(cik: "1068019", fiscal_year: 2022)"#;
    for (profile, srv) in [("full", &w.full), ("small", &w.small)] {
        for (tool, v) in [("get", get(srv, request).0), ("calc", calc(srv, request).0)] {
            assert!(is_error(&v), "{profile} {tool}: {}", jcs(&v));
            let t = text(&v);
            let lower = t.to_lowercase();
            assert!(t.contains("datumesh.com/sec"), "{profile} {tool}: names the namespace: {t}");
            assert!(lower.contains("no ") && lower.contains("names"), "{profile} {tool}: holds no such names: {t}");
            assert!(lower.contains("find"), "{profile} {tool}: points to find: {t}");
        }
    }
}

// =============================================================================== item 26

/// Every object under `v` (itself included).
fn objects<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            out.push(v);
            m.iter().for_each(|(_, x)| objects(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| objects(x, out)),
        _ => {}
    }
}

#[test]
fn i26_a_refused_long_interval_says_the_most_it_takes_and_how_to_page() {
    let w = prod_world();
    let args = r#"{"function":"treasury/par_yield","keys":{"tenor":"10Y"},"from":"1999-01-01","to":"2026-11-24"}"#;
    for (profile, srv) in [("full", &w.full), ("small", &w.small)] {
        let (v, _) = call(srv, "interval", args);
        assert!(is_error(&v), "{profile}: still refused: {}", jcs(&v));
        let t = text(&v);
        assert!(t.contains("E_RANGE_TOO_LARGE"), "{profile}: the code: {t}");
        // The most it takes: datumesh-http's limit for a daily series, ten years from `from`.
        assert!(t.contains("10 years") || t.contains("ten years"), "{profile}: the most it takes: {t}");
        // How to page: the first page's interval, from the same start to at most ten years on.
        let mut all = Vec::new();
        objects(structured(&v), &mut all);
        let page = all
            .iter()
            .find(|o| s(o, "from") == "1999-01-01" && !s(o, "to").is_empty() && s(o, "to") != "2026-11-24")
            .unwrap_or_else(|| panic!("{profile}: the first page's arguments: {}", jcs(structured(&v))));
        let to = s(page, "to");
        assert!(to.as_str() > "1999-01-01" && to.as_str() <= "2009-01-01", "{profile}: within the most: {}", jcs(page));
        assert!(jcs(page).contains("par_yield") && jcs(page).contains("10Y"), "{profile}: {}", jcs(page));
        // The page is an interval the mesh would take (the fake has no answer for it; its form is what counts).
        assert!(page.get("function").is_some(), "{profile}: interval's arguments: {}", jcs(page));
    }
}

// =============================================================================== item 27, the small profile

#[test]
fn i27_a_small_find_of_many_results_is_a_small_page_naming_the_total() {
    let d = json::parse(br#"{"args":{"date":{"type":"date"}},"order":["date"],"returns":"number"}"#).unwrap();
    let results: Vec<Value> = (0..40)
        .map(|n| {
            let name = format!("mesh-test.datumesh.com/fake/series_{n:02}");
            let description = format!("Synthetic daily series number {n}. ").repeat(10);
            found(&name, &datumesh_core::sha256_hex(name.as_bytes()), &format!("Series {n}"), &description, &d)
        })
        .collect();
    let long = s(&results[0], "description");
    let w = find_world(Some(results));
    let (v, line) = call(&w.small, "find", r#"{"query":"series"}"#);
    assert!(!is_error(&v), "{}", jcs(&v));
    within("small find (40 results)", &line, SMALL_BUDGET);
    let listed = arr(structured(&v), "results");
    assert!(!listed.is_empty() && listed.len() <= 10, "a small page: {} results", listed.len());
    assert!(text(&v).contains("40"), "names the total: {}", text(&v));
    assert!(!line.contains(&long), "a description is cut");
    // The full profile is not cut so (64 KiB).
    let (v, line) = call(&w.full, "find", r#"{"query":"series"}"#);
    within("full find", &line, BUDGET);
    assert_eq!(arr(structured(&v), "results").len(), 40);
}

/// A text of `n` bytes, ASCII with no character JSON escapes, unique to `tag`.
fn plain_text(tag: &str, n: usize) -> String {
    let mut t = format!("{tag} ");
    let words = ["alpha", "beta", "gamma", "delta", "epsilon"];
    let mut i = 0;
    while t.len() < n {
        t.push_str(words[i % words.len()]);
        t.push_str(&format!("{i} "));
        i += 1;
    }
    t.truncate(n);
    t
}

#[test]
fn i27_a_small_get_of_a_short_text_cell_sends_its_text_once() {
    let w = find_world(None);
    let body = plain_text("l055short", 1000);
    w.up.publish_blob_cell("l055short", body.as_bytes(), "text/plain", None);
    let (v, line) = get(&w.small, &text_cell_call("l055short"));
    assert!(!is_error(&v), "{}", jcs(&v));
    within("small get (1,000-byte text)", &line, SMALL_BUDGET);
    assert_eq!(line.matches(&body).count(), 1, "the text, once: {line}");
}

#[test]
fn i27_a_small_get_of_a_long_text_cell_gives_its_address() {
    let w = find_world(None);
    let body = plain_text("l055long", 19 * 1024);
    let sha = w.up.publish_blob_cell("l055long", body.as_bytes(), "text/plain", None);
    let (v, line) = get(&w.small, &text_cell_call("l055long"));
    assert!(!is_error(&v), "{}", jcs(&v));
    within("small get (19 KiB text)", &line, SMALL_BUDGET);
    assert!(line.contains(&format!("blob://{sha}")), "its address: {line}");
    assert!(!line.contains(&body[..2000]), "not its text");
    // A text over about 1.5 KiB is not inlined either.
    let body = plain_text("l055mid", 3000);
    let sha = w.up.publish_blob_cell("l055mid", body.as_bytes(), "text/plain", None);
    let (_, line) = get(&w.small, &text_cell_call("l055mid"));
    within("small get (3,000-byte text)", &line, SMALL_BUDGET);
    assert!(line.contains(&format!("blob://{sha}")) && !line.contains(&body), "{line}");
}

#[test]
fn i27_a_small_get_of_one_cell_is_within_the_budget() {
    let w = find_world(None);
    let (v, line) = get(&w.small, &usd("2026-10-30"));
    assert!(!is_error(&v), "{}", jcs(&v));
    within("small get (one cell)", &line, SMALL_BUDGET);
}

#[test]
fn i27_a_small_slice_answer_is_small_pages_that_cover_the_slice_once() {
    let w = prod_world();
    let mut request = BANK_SLICE.to_string();
    let mut seen: Vec<String> = Vec::new();
    for page in 1..=20 {
        let (v, line) = get(&w.small, &request);
        assert!(!is_error(&v), "page {page}: {}", jcs(&v));
        within(&format!("small slice page {page}"), &line, SMALL_BUDGET);
        let members = arr(structured(&v), "members");
        assert!(!members.is_empty() && members.len() <= 15, "page {page}: a small page of {}", members.len());
        seen.extend(members.iter().map(|m| s(m, "call")));
        match structured(&v).get("next") {
            Some(next) => request = s(next, "request"),
            None => break,
        }
    }
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!((seen.len(), unique.len()), (BANK_MEMBERS, BANK_MEMBERS), "every member once: {seen:?}");
}

#[test]
fn i27_a_small_interval_is_small_pages_that_cover_the_range_once() {
    let w = prod_world();
    let mut args =
        r#"{"function":"datumesh.com/relay/bankofengland.co.uk/bank_rate","from":"2026-09-01","to":"2026-11-20"}"#
            .to_string();
    let mut seen: Vec<String> = Vec::new();
    for page in 1..=20 {
        let (v, line) = call(&w.small, "interval", &args);
        assert!(!is_error(&v), "page {page}: {}", jcs(&v));
        within(&format!("small interval page {page}"), &line, SMALL_BUDGET);
        let members = arr(structured(&v), "members");
        assert!(!members.is_empty() && members.len() <= 30, "page {page}: a small page of {}", members.len());
        seen.extend(members.iter().map(|m| date_of_call(&s(m, "call"))));
        match structured(&v).get("rest") {
            Some(rest) => {
                args = jcs(rest.get("arguments").unwrap_or_else(|| panic!("rest's arguments: {}", jcs(rest))))
            }
            None => break,
        }
    }
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!((seen.len(), unique.len()), (BANK_MEMBERS, BANK_MEMBERS), "every day once: {seen:?}");
}

#[test]
fn i27_the_full_profile_keeps_its_hundred_members() {
    let w = prod_world();
    let args = r#"{"function":"ecb/fx_reference","keys":{"currency":"USD"},"from":"2024-01-01","to":"2024-06-30"}"#;
    let (v, line) = call(&w.full, "interval", args);
    assert!(!is_error(&v), "{}", jcs(&v));
    within("full interval", &line, BUDGET);
    assert_eq!(arr(structured(&v), "members").len(), 100, "the full profile is not cut to 4 KiB");
}

// ------------------------------------------------------------------------------- the level world (calc, explain)

const LEVEL_SIG: &str = r#"{"args": {"day": {"type": "date", "valid": true}}, "order": ["day"], "returns": {"type": "number", "unit": "percent"}}"#;
const CELLS_TX: &str = "2026-11-15T11:00:00.000000Z";
const LEVELS: usize = 60;

fn day(n: usize) -> String {
    let z = 19_723 + n as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn level_call(n: usize) -> String {
    format!("local.invalid/level(day: {})", day(n))
}

struct LW {
    _up: Upstream,
    _dir: TempDir,
    full: McpServer,
    small: McpServer,
}

fn level_world() -> LW {
    let up = upstream();
    let dir = TempDir::new("l055-level");
    let clock = ManualClock::new("2026-11-15T10:00:00.000000Z");
    let r = Arc::new(resolver_at(&dir, &up, &clock));
    r.define_stored("local/level", &json::parse(LEVEL_SIG.as_bytes()).unwrap()).unwrap();
    let tmp = TempDir::new("l055-level-cells");
    let lines: Vec<String> = (0..LEVELS)
        .map(|n| {
            format!(
                r#"{{"cell": {{"fn": "local/level", "args": {{"day": {{"type": "date", "value": "{}"}}}}}}, "value": {{"type": "number", "value": "{}"}}, "tx": "{CELLS_TX}", "asof": "{CELLS_TX}", "licence": "CC0-1.0", "summary": "A test cell."}}"#,
                day(n),
                1 + n % 7
            )
        })
        .collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    r.publish(&jsonl(&tmp, "level.jsonl", &refs)).unwrap();
    clock.set("2026-11-15T12:00:00.000000Z");
    LW { _up: up, _dir: dir, full: McpServer::new(r.clone()), small: McpServer::with_profile(r, Profile::Small) }
}

#[test]
fn i27_a_small_calc_of_40_cells_is_within_the_budget_with_a_small_lineage_page() {
    let w = level_world();
    let formula = (0..40).map(level_call).collect::<Vec<_>>().join(" + ");
    let (v, line) = calc(&w.small, &formula);
    assert!(!is_error(&v), "{}", jcs(&v));
    within("small calc (40 cells)", &line, SMALL_BUDGET);
    let sc = structured(&v);
    let lineage = arr(sc, "lineage");
    assert!(!lineage.is_empty() && lineage.len() <= 10, "a lineage page of about 8: {}", lineage.len());
    assert_eq!(number(sc.get("lineage_total")), 40, "{}", jcs(sc));
    assert!(line.matches(&formula).count() <= 1, "the formula is not echoed twice");
    // The full profile keeps its 20 (A3).
    let (v, line) = calc(&w.full, &formula);
    within("full calc", &line, BUDGET);
    assert_eq!(arr(structured(&v), "lineage").len(), 20);
}

fn calc_name(formula: &str) -> String {
    let canonical = datumesh_formula::canonical_ast(formula, None).unwrap().print();
    format!("local.invalid/calc_{}", &datumesh_core::sha256_hex(canonical.as_bytes())[..16])
}

#[test]
fn also_the_full_profile_s_deepest_explain_entries_carry_the_receipt_hash_only() {
    let w = level_world();
    let inner = ["0..3", "3..6"].map(|r| {
        let (a, b) = r.split_once("..").unwrap();
        (a.parse::<usize>().unwrap()..b.parse::<usize>().unwrap()).map(level_call).collect::<Vec<_>>().join(" + ")
    });
    for f in &inner {
        assert!(!is_error(&calc(&w.full, f).0), "{f}");
    }
    let outer = inner.iter().map(|f| format!("{}()", calc_name(f))).collect::<Vec<_>>().join(" + ");
    assert!(!is_error(&calc(&w.full, &outer).0), "{outer}");
    let (v, line) =
        call(&w.full, "explain", &format!(r#"{{"request":{},"depth":2}}"#, q(&format!("{}()", calc_name(&outer)))));
    assert!(!is_error(&v), "{}", jcs(&v));
    within("full explain", &line, BUDGET);
    let top = arr(structured(&v), "inputs");
    assert_eq!(top.len(), 2, "{}", jcs(structured(&v)));
    for node in top {
        assert!(!s(node, "call").is_empty() && node.get("value").is_some(), "a level-1 entry: {}", jcs(node));
        let deepest = arr(node, "inputs");
        assert_eq!(deepest.len(), 3, "{}", jcs(node));
        for d in deepest {
            let Value::Object(pairs) = d else { panic!("{}", jcs(d)) };
            let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(keys, ["receipt_hash"], "the deepest entry is its receipt hash only: {}", jcs(d));
        }
    }
}
