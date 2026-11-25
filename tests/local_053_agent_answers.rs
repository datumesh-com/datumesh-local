//! datumesh-local 0.5.3, the guard patch's part A (CONTRACT-guard-patch.md A1 to A3; the user's
//! items 12 to 14). Written before the implementation.
//!
//! * A1. A slice read is ONE lineage entry in a calc's answer: `{entry, slice, members,
//!   members_hash}`. `entry` is the receipt's `slice:<slice id>:<members hash>` input, byte for
//!   byte; `members_hash` its third part; `members` the member count (a JSON number); `slice` the
//!   slice object as the mesh's explain gives it (`Manifest::slice().to_value()`, datumesh-http's
//!   explain slice node). No per-member entry, no member's receipt hash.
//! * A2. The text part of every tool answer is short and is never the JCS of the structured
//!   content. For `get` and `calc` it is the short answer: the value, its unit, the as-of, the source
//!   (`get`: the answer block's `signer`) and the receipt's id (its hash; for `calc`, the hash of the
//!   held receipt of `local/calc_<h>()`), in under 1 KiB. The structured content keeps the envelope.
//!   For `get`, the items before the last stay as they are (a reference's label and call, a text
//!   blob's checked text, a blob's note); the last item, which was the answer block's JCS, is the
//!   short answer.
//! * A3. A tool answer's lineage is cut to its first `LINEAGE_CUT` (20) entries, with
//!   `lineage_total` (the whole count) and `explain` (an object that is `explain`'s arguments:
//!   `request`, `page`, `limit`, giving the entries after the first 20). `explain` pages its node's
//!   `inputs` with `page` (from 1) and `limit` (default 20); the node carries `inputs_total` (the
//!   same on every page), `page` and `limit`. A page past the end has no inputs and is not an error;
//!   `page` or `limit` of 0 is an argument error. Every tool answer of this file's fixtures, the
//!   whole JSON-RPC response line, is at most `BUDGET` bytes (64 KiB: no budget is documented in the
//!   code; 64 KiB is the text limit `get` already has).
//!
//! The fixture is local: `local/level` (a stored function, unit percent) with `MEMBERS` (500)
//! cells, `level(day)` for the days from 2024-01-01; the upstream is the test mesh of
//! `tests/common` (for `USD`), and the production fake (`tests/support/prod_mesh.rs`) for `find`,
//! `interval` and `describe` against production's recorded data. Nothing reaches the network.

#![cfg(unix)]

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_core::{Manifest, Receipt};
use datumesh_local::mcp::{McpServer, Transport};
use datumesh_local::{Config, Resolver};
use tools::{arr, is_error, items, jcs, q, result, structured, M};

const MEMBERS: usize = 500;
const LINEAGE_CUT: usize = 20;
const BUDGET: usize = 64 * 1024;
const SHORT: usize = 1024;

const CELLS_TX: &str = "2026-11-15T11:00:00.000000Z";
const T: &str = "2026-11-15T12:00:00.000000Z";
const LEVEL_SIG: &str = r#"{"args": {"day": {"type": "date", "valid": true}}, "order": ["day"], "returns": {"type": "number", "unit": "percent"}}"#;
const SLICE_CALL: &str = "local.invalid/level(day: *)";
const SLICE_F: &str = "mean(local.invalid/level(day: *))";
/// A slice read and one cell outside it: two lineage entries.
const MIXED_F: &str = "mean(local.invalid/level(day: *)) + local.invalid/level(day: 2024-01-01)";
/// The number of single cells `wide_formula` reads.
const WIDE: usize = 45;

// ------------------------------------------------------------------------------- the fixture

/// The date `n` days after 2024-01-01.
fn day(n: usize) -> String {
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = 19_723 + n as i64 + 719_468; // 2024-01-01 is day 19,723
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

fn level_value(n: usize) -> String {
    // Canonical decimals: no trailing zero after the point.
    match n % 10 {
        0 => format!("{}", 1 + n % 7),
        f => format!("{}.{f}", 1 + n % 7),
    }
}

fn level_line(n: usize) -> String {
    format!(
        r#"{{"cell": {{"fn": "local/level", "args": {{"day": {{"type": "date", "value": "{}"}}}}}}, "value": {{"type": "number", "value": "{}"}}, "tx": "{CELLS_TX}", "asof": "{CELLS_TX}", "licence": "CC0-1.0", "summary": "A test cell."}}"#,
        day(n),
        level_value(n)
    )
}

fn level_call(n: usize) -> String {
    format!("local.invalid/level(day: {})", day(n))
}

/// `level(day 0) + level(day 1) + ...`: `WIDE` cells, so `WIDE` lineage entries.
fn wide_formula() -> String {
    (0..WIDE).map(level_call).collect::<Vec<_>>().join(" + ")
}

struct W {
    _up: Upstream,
    _dir: TempDir,
    r: Arc<Resolver>,
    srv: McpServer,
}

fn world() -> W {
    let up = upstream();
    let dir = TempDir::new("l053-agent");
    let clock = ManualClock::new("2026-11-15T10:00:00.000000Z");
    let r = Arc::new(resolver_at(&dir, &up, &clock));
    r.define_stored("local/level", &json::parse(LEVEL_SIG.as_bytes()).unwrap()).unwrap();
    let tmp = TempDir::new("l053-agent-cells");
    let lines: Vec<String> = (0..MEMBERS).map(level_line).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    r.publish(&jsonl(&tmp, "level.jsonl", &refs)).unwrap();
    clock.set(T);
    let srv = McpServer::new(r.clone());
    W { _up: up, _dir: dir, r, srv }
}

// ------------------------------------------------------------------------------- the calls

/// A `tools/call`: the response as parsed, and its bytes (the whole JSON-RPC line).
fn call(srv: &McpServer, name: &str, args: &str) -> (Value, usize) {
    let line = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{M},"name":{},"arguments":{args}}}}}"#,
        q(name)
    );
    let out = srv.handle(&line, Transport::Stdio).expect("a response");
    (json::parse(out.as_bytes()).unwrap(), out.len())
}

fn calc(srv: &McpServer, formula: &str) -> (Value, usize) {
    call(srv, "calc", &format!(r#"{{"formula":{}}}"#, q(formula)))
}

fn get(srv: &McpServer, request: &str) -> (Value, usize) {
    call(srv, "get", &format!(r#"{{"request":{}}}"#, q(request)))
}

fn explain(srv: &McpServer, args: &str) -> (Value, usize) {
    call(srv, "explain", args)
}

fn calc_name(formula: &str) -> String {
    let canonical = datumesh_formula::canonical_ast(formula, None).unwrap().print();
    format!("local/calc_{}", &datumesh_core::sha256_hex(canonical.as_bytes())[..16])
}

/// The held receipt of a calc's cell `local/calc_<h>()` (a get, not an evaluation).
fn calc_receipt(r: &Resolver, formula: &str) -> Receipt {
    let reply = r.get(&format!("{}()", calc_name(formula))).unwrap();
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.body));
    Receipt::from_value(&receipt(&reply.body)).unwrap()
}

fn number_of(v: Option<&Value>) -> u64 {
    v.and_then(Value::as_number_text).and_then(|t| t.parse().ok()).unwrap_or_else(|| panic!("a JSON number: {v:?}"))
}

fn text_of(v: &Value) -> String {
    items(v).join("\n")
}

/// A2: no text item is the JCS of the structured content (nor of the whole result).
fn assert_text_is_not_the_structured(what: &str, v: &Value) {
    let sc = jcs(structured(v));
    for (i, t) in items(v).iter().enumerate() {
        assert_ne!(t.trim(), sc, "{what}: text item {i} is the JCS of the structured content");
        if let Ok(parsed) = json::parse(t.trim().as_bytes()) {
            assert_ne!(jcs(&parsed), sc, "{what}: text item {i} is the structured content as JSON");
        }
    }
}

fn assert_within_budget(what: &str, bytes: usize) {
    assert!(bytes <= BUDGET, "{what}: a tool answer of {bytes} bytes, over the budget of {BUDGET}");
}

// =============================================================================== A1

#[test]
fn a1_a_calc_over_a_500_member_slice_has_one_lineage_entry_for_the_slice() {
    let w = world();
    let (v, _) = calc(&w.srv, SLICE_F);
    assert!(!is_error(&v), "{}", jcs(&v));
    let lineage = arr(structured(&v), "lineage");
    assert_eq!(lineage.len(), 1, "one entry for the slice read, not one per member: {} entries", lineage.len());
    let e = &lineage[0];
    // The entry is the receipt's slice input, byte for byte.
    let rc = calc_receipt(&w.r, SLICE_F);
    let inputs: Vec<&String> = rc.inputs().iter().collect();
    assert_eq!(inputs.len(), 1, "{inputs:?}");
    let entry = inputs[0].clone();
    assert!(entry.starts_with("slice:"), "{entry}");
    assert_eq!(s(e, "entry"), entry, "{}", jcs(e));
    let parts: Vec<&str> = entry.split(':').collect();
    assert_eq!(parts.len(), 3, "{entry}");
    assert_eq!(s(e, "members_hash"), parts[2], "the members hash is the entry's third part: {}", jcs(e));
    assert_eq!(number_of(e.get("members")), MEMBERS as u64, "{}", jcs(e));
    // The slice is the manifest's slice object, as the mesh's explain gives it.
    let bytes =
        w.r.store().manifest(&entry).expect("the manifest is held locally (ManifestPolicy::Store)").into_owned();
    let m = Manifest::from_json(&bytes).unwrap();
    assert_eq!(m.entry(), entry);
    assert_eq!(m.members().len(), MEMBERS);
    let slice = e.get("slice").unwrap_or_else(|| panic!("slice: {}", jcs(e)));
    assert_eq!(jcs(slice), jcs(&m.slice().to_value()), "the slice object");
    // Exactly these four members: no per-member call, receipt, publisher or trust.
    let Value::Object(members) = e else { panic!("an object: {}", jcs(e)) };
    let mut keys: Vec<&str> = members.iter().map(|(k, _)| k.as_str()).collect();
    keys.sort();
    assert_eq!(keys, ["entry", "members", "members_hash", "slice"], "{}", jcs(e));
}

#[test]
fn a1_the_lineage_names_no_member() {
    let w = world();
    let (v, _) = calc(&w.srv, SLICE_F);
    assert!(!is_error(&v), "{}", jcs(&v));
    let whole = jcs(result(&v));
    let entry = calc_receipt(&w.r, SLICE_F).inputs()[0].clone();
    let m = Manifest::from_json(&w.r.store().manifest(&entry).unwrap()).unwrap();
    for hash in m.members().iter().take(50).chain(m.members().iter().rev().take(50)) {
        assert!(!whole.contains(hash.as_str()), "a member's receipt hash {hash} is in the answer");
    }
    let lineage = jcs(structured(&v).get("lineage").unwrap());
    assert!(lineage.len() < 2048, "the lineage of one slice read is small: {} bytes", lineage.len());
}

#[test]
fn a1_a_slice_and_a_cell_are_two_entries_each_of_its_own_shape() {
    let w = world();
    let (v, _) = calc(&w.srv, MIXED_F);
    assert!(!is_error(&v), "{}", jcs(&v));
    let lineage = arr(structured(&v), "lineage");
    assert_eq!(lineage.len(), 2, "{}", jcs(structured(&v)));
    let rc = calc_receipt(&w.r, MIXED_F);
    let slice_entry = rc.inputs().iter().find(|i| i.starts_with("slice:")).expect("a slice input").clone();
    let cell_hash = rc.inputs().iter().find(|i| !i.starts_with("slice:")).expect("a cell input").clone();
    let slice = lineage.iter().find(|e| e.get("entry").is_some()).expect("the slice's entry");
    assert_eq!(s(slice, "entry"), slice_entry);
    assert_eq!(number_of(slice.get("members")), MEMBERS as u64);
    let cell = lineage.iter().find(|e| e.get("entry").is_none()).expect("the cell's entry");
    // A cell keeps C1's shape: call, receipt, publisher, trust.
    assert_eq!(s(cell, "receipt"), cell_hash, "{}", jcs(cell));
    assert_eq!(s(cell, "call"), level_call(0));
    assert!(!s(cell, "publisher").is_empty() && !s(cell, "trust").is_empty(), "{}", jcs(cell));
}

// =============================================================================== A2

#[test]
fn a2_calc_text_is_the_short_answer_and_the_structured_part_the_answer() {
    let w = world();
    let (v, bytes) = calc(&w.srv, SLICE_F);
    assert!(!is_error(&v), "{}", jcs(&v));
    let sc = structured(&v);
    let text = text_of(&v);
    assert!(text.len() < SHORT, "the text part is short: {} bytes: {text}", text.len());
    assert_text_is_not_the_structured("calc", &v);
    // value, unit, as-of and the receipt's id
    assert!(text.contains(&s(sc, "value")), "the value {}: {text}", s(sc, "value"));
    assert_eq!(s(sc, "unit"), "percent", "{}", jcs(sc));
    assert!(text.contains("percent"), "the unit: {text}");
    assert!(text.contains(&s(sc, "asof")), "the as-of {}: {text}", s(sc, "asof"));
    let hash = calc_receipt(&w.r, SLICE_F).hash().to_string();
    assert!(text.contains(&hash), "the receipt's id {hash}: {text}");
    // The answer is sent once in full: the message is the structured content plus a short text.
    let sc_len = jcs(sc).len();
    assert!(bytes < sc_len + SHORT + 512, "{bytes} bytes for a structured content of {sc_len}");
    assert!(bytes < 2 * sc_len || sc_len < SHORT, "far below twice the envelope: {bytes} vs {sc_len}");
}

#[test]
fn a2_get_text_is_the_short_answer_and_the_structured_part_keeps_the_envelope() {
    let w = world();
    let (v, bytes) = get(&w.srv, &level_call(3));
    assert!(!is_error(&v), "{}", jcs(&v));
    let sc = structured(&v);
    let rc = sc.get("envelope").and_then(|e| e.get("receipt")).expect("the envelope with its receipt");
    let r = Receipt::from_value(rc).unwrap();
    let text = text_of(&v);
    assert!(text.len() < SHORT, "{} bytes: {text}", text.len());
    assert_text_is_not_the_structured("get", &v);
    assert!(text.contains(&level_value(3)), "the value: {text}");
    assert!(text.contains("percent"), "the unit (the function's returns): {text}");
    assert!(text.contains(&s(rc, "asof")), "the as-of {}: {text}", s(rc, "asof"));
    assert!(text.contains(&s(sc, "signer")), "the source {}: {text}", s(sc, "signer"));
    assert!(text.contains(&r.hash().to_string()), "the receipt's id: {text}");
    assert_eq!(s(sc, "receipt_hash"), r.hash());
    let sc_len = jcs(sc).len();
    assert!(bytes < sc_len + SHORT + 512, "{bytes} bytes for a structured content of {sc_len}");
}

#[test]
fn a2_get_of_an_upstream_cell_names_its_source() {
    let w = world();
    let (v, _) = get(&w.srv, USD);
    assert!(!is_error(&v), "{}", jcs(&v));
    let sc = structured(&v);
    let text = text_of(&v);
    assert!(text.len() < SHORT, "{text}");
    assert_text_is_not_the_structured("get USD", &v);
    assert!(text.contains("1.1617"), "{text}");
    assert!(text.contains(HOST), "the source: {text}");
    assert!(text.contains(&s(sc, "receipt_hash")), "{text}");
}

#[test]
fn a2_explain_whatif_and_verify_texts_are_not_the_structured_content() {
    let w = world();
    let wide = wide_formula();
    assert!(!is_error(&calc(&w.srv, &wide).0));
    let request = format!("{}()", calc_name(&wide));
    let (v, _) = explain(&w.srv, &format!(r#"{{"request":{}}}"#, q(&request)));
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_text_is_not_the_structured("explain", &v);
    assert!(text_of(&v).len() < 2 * SHORT, "explain's text is short: {}", text_of(&v));

    let (v, _) = call(
        &w.srv,
        "whatif",
        &format!(r#"{{"request":{},"overrides":[{{"cell":{},"value":"99"}}]}}"#, q(&request), q(&level_call(0))),
    );
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_text_is_not_the_structured("whatif", &v);

    let (g, _) = get(&w.srv, &level_call(5));
    let rc = structured(&g).get("envelope").and_then(|e| e.get("receipt")).unwrap().clone();
    let (v, _) = call(&w.srv, "verify", &format!(r#"{{"receipt":{}}}"#, q(&jcs(&rc))));
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_text_is_not_the_structured("verify (a receipt)", &v);
    let (v, _) = call(&w.srv, "verify", &format!(r#"{{"call":{}}}"#, q(&level_call(5))));
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_text_is_not_the_structured("verify (a call)", &v);
}

#[test]
fn a2_get_of_a_large_slice_text_is_not_the_structured_content() {
    let w = world();
    let (v, _) = get(&w.srv, SLICE_CALL);
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_text_is_not_the_structured("get (a slice)", &v);
    assert!(text_of(&v).len() < 2 * SHORT, "a slice's text is its summary: {}", text_of(&v));
    // The summary is still in the structured content (C4).
    assert!(structured(&v).get("summary").is_some(), "{}", jcs(structured(&v)));
}

/// The production fake's world: `find`, `interval` and `describe` against production's data.
fn prod_world() -> (prod_mesh::ProdMesh, TempDir, McpServer) {
    let prod = prod_mesh::ProdMesh::start(true);
    let dir = prod_mesh::fresh_home(&[("datumesh.com", &prod.origin)]);
    let srv = McpServer::new(Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap()));
    (prod, dir, srv)
}

#[test]
fn a2_find_interval_and_describe_texts_are_not_the_structured_content() {
    let (_prod, _dir, srv) = prod_world();
    let (v, bytes) = call(&srv, "find", r#"{"query":"bank rate"}"#);
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_text_is_not_the_structured("find", &v);
    assert_within_budget("find", bytes);

    let (v, bytes) = call(
        &srv,
        "interval",
        r#"{"function":"treasury/par_yield","keys":{"tenor":"10Y"},"from":"2026-11-16","to":"2026-11-24"}"#,
    );
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_text_is_not_the_structured("interval", &v);
    assert!(text_of(&v).len() < 2 * SHORT, "interval's text is short: {}", text_of(&v));
    assert_within_budget("interval", bytes);

    let (v, bytes) = call(
        &srv,
        "interval",
        r#"{"function":"ecb/fx_reference","keys":{"currency":"USD"},"from":"2024-01-01","to":"2024-06-30"}"#,
    );
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_text_is_not_the_structured("interval (126 members)", &v);
    assert_within_budget("interval (126 members)", bytes);

    let (v, bytes) = call(&srv, "describe", r#"{"id":"datumesh.com/catalog"}"#);
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_text_is_not_the_structured("describe", &v);
    assert_within_budget("describe", bytes);
}

// =============================================================================== A3

/// The receipt hashes (or slice entries) of an explain node's inputs, in order.
fn input_ids(node: &Value) -> Vec<String> {
    arr(node, "inputs")
        .iter()
        .map(|c| {
            let h = s(c, "receipt_hash");
            if h.is_empty() {
                s(c, "entry")
            } else {
                h
            }
        })
        .collect()
}

#[test]
fn a3_a_calc_lineage_is_cut_to_20_with_the_total_and_a_pointer_to_explain() {
    let w = world();
    let wide = wide_formula();
    let (v, bytes) = calc(&w.srv, &wide);
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_within_budget("calc (45 inputs)", bytes);
    let sc = structured(&v);
    let lineage = arr(sc, "lineage");
    assert_eq!(lineage.len(), LINEAGE_CUT, "the first 20 entries");
    assert_eq!(number_of(sc.get("lineage_total")), WIDE as u64, "{}", jcs(sc));
    // The first 20 are the receipt's first 20 inputs, in order.
    let rc = calc_receipt(&w.r, &wide);
    assert_eq!(rc.inputs().len(), WIDE);
    let got: Vec<String> = lineage.iter().map(|e| s(e, "receipt")).collect();
    assert_eq!(got, rc.inputs()[..LINEAGE_CUT].to_vec());
    // The pointer is explain's arguments, and gives the next entries.
    let pointer = sc.get("explain").unwrap_or_else(|| panic!("a pointer to explain: {}", jcs(sc)));
    assert!(pointer.get("request").and_then(Value::as_str).is_some(), "{}", jcs(pointer));
    let (e, _) = explain(&w.srv, &jcs(pointer));
    assert!(!is_error(&e), "the pointer is explain's arguments: {}", jcs(&e));
    let next = input_ids(structured(&e));
    assert_eq!(next, rc.inputs()[LINEAGE_CUT..2 * LINEAGE_CUT].to_vec(), "the entries after the first 20");
    // And the text says so.
    let text = text_of(&v);
    assert!(text.contains(&WIDE.to_string()) && text.contains("explain"), "the total and explain: {text}");
}

#[test]
fn a3_a_lineage_of_20_or_fewer_is_not_cut() {
    let w = world();
    let (v, _) = calc(&w.srv, MIXED_F);
    let sc = structured(&v);
    assert_eq!(arr(sc, "lineage").len(), 2);
    assert!(sc.get("explain").is_none(), "no pointer when nothing is cut: {}", jcs(sc));
    if let Some(t) = sc.get("lineage_total") {
        assert_eq!(number_of(Some(t)), 2);
    }
}

#[test]
fn a3_explain_pages_its_inputs_with_page_and_limit() {
    let w = world();
    let wide = wide_formula();
    assert!(!is_error(&calc(&w.srv, &wide).0));
    let request = format!("{}()", calc_name(&wide));
    let all = calc_receipt(&w.r, &wide).inputs().to_vec();
    // The default page: the first 20, with the total.
    let (v, bytes) = explain(&w.srv, &format!(r#"{{"request":{}}}"#, q(&request)));
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_within_budget("explain", bytes);
    let node = structured(&v);
    assert_eq!(input_ids(node), all[..LINEAGE_CUT].to_vec(), "page 1 of 20 by default");
    assert_eq!(number_of(node.get("inputs_total")), WIDE as u64, "{}", jcs(node));
    assert_eq!(number_of(node.get("page")), 1);
    assert_eq!(number_of(node.get("limit")), LINEAGE_CUT as u64);
    // Pages of 10: each continues the last, the total stays, and together they are the inputs.
    let mut seen = Vec::new();
    for page in 1..=5u64 {
        let (v, _) = explain(&w.srv, &format!(r#"{{"request":{},"page":{page},"limit":10}}"#, q(&request)));
        assert!(!is_error(&v), "page {page}: {}", jcs(&v));
        let node = structured(&v);
        assert_eq!(number_of(node.get("inputs_total")), WIDE as u64, "page {page}: the same total");
        assert_eq!(number_of(node.get("page")), page);
        assert_eq!(number_of(node.get("limit")), 10);
        let ids = input_ids(node);
        let want = if page < 5 { 10 } else { WIDE - 40 };
        assert_eq!(ids.len(), want, "page {page}");
        seen.extend(ids);
    }
    assert_eq!(seen, all, "page 2 continues page 1, and so on: no gap, no repeat");
    // Past the end: no inputs, not an error.
    let (v, _) = explain(&w.srv, &format!(r#"{{"request":{},"page":6,"limit":10}}"#, q(&request)));
    assert!(!is_error(&v), "{}", jcs(&v));
    assert!(input_ids(structured(&v)).is_empty());
    assert_eq!(number_of(structured(&v).get("inputs_total")), WIDE as u64);
}

#[test]
fn a3_explain_refuses_a_page_or_limit_of_zero() {
    let w = world();
    let wide = wide_formula();
    assert!(!is_error(&calc(&w.srv, &wide).0));
    let request = format!("{}()", calc_name(&wide));
    for args in [r#""page":0"#, r#""limit":0"#, r#""page":"two""#] {
        let (v, _) = explain(&w.srv, &format!(r#"{{"request":{},{args}}}"#, q(&request)));
        assert!(is_error(&v), "{args}: {}", jcs(&v));
    }
}

#[test]
fn a3_explain_s_schema_takes_page_and_limit() {
    let w = world();
    let line = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{{{M}}}}}"#);
    let v = json::parse(w.srv.handle(&line, Transport::Stdio).unwrap().as_bytes()).unwrap();
    let t = arr(result(&v), "tools").iter().find(|t| s(t, "name") == "explain").unwrap().clone();
    let props = t.get("inputSchema").and_then(|x| x.get("properties")).unwrap();
    for k in ["request", "depth", "page", "limit"] {
        assert!(props.get(k).is_some(), "explain takes {k}: {}", jcs(&t));
    }
    for k in ["page", "limit"] {
        assert_eq!(s(props.get(k).unwrap(), "type"), "integer", "{}", jcs(&t));
    }
    let d = s(&t, "description").to_lowercase();
    assert!(d.contains("page") && d.contains("limit"), "the description says it pages: {d}");
}

#[test]
fn a3_explain_of_the_slice_calc_is_one_input() {
    let w = world();
    assert!(!is_error(&calc(&w.srv, SLICE_F).0));
    let request = format!("{}()", calc_name(SLICE_F));
    let (v, bytes) = explain(&w.srv, &format!(r#"{{"request":{}}}"#, q(&request)));
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_within_budget("explain (the slice calc)", bytes);
    let node = structured(&v);
    assert_eq!(number_of(node.get("inputs_total")), 1);
    let ids = input_ids(node);
    assert_eq!(ids.len(), 1);
    assert!(ids[0].starts_with("slice:"), "{ids:?}");
}

#[test]
fn a3_no_tool_answer_of_the_local_fixture_is_over_the_budget() {
    let w = world();
    let wide = wide_formula();
    let mut sizes = Vec::new();
    let mut record = |what: &str, (v, bytes): (Value, usize)| {
        assert!(!is_error(&v), "{what}: {}", jcs(&v));
        sizes.push((what.to_string(), bytes));
    };
    record("calc (the slice)", calc(&w.srv, SLICE_F));
    record("calc (45 cells)", calc(&w.srv, &wide));
    record("get (a cell)", get(&w.srv, &level_call(0)));
    record("get (the 500-member slice)", get(&w.srv, SLICE_CALL));
    record("get (the 45-input calc cell)", get(&w.srv, &format!("{}()", calc_name(&wide))));
    record(
        "explain (45 inputs)",
        explain(&w.srv, &format!(r#"{{"request":{}}}"#, q(&format!("{}()", calc_name(&wide))))),
    );
    record(
        "explain (45 inputs, depth 2)",
        explain(&w.srv, &format!(r#"{{"request":{},"depth":2}}"#, q(&format!("{}()", calc_name(&wide))))),
    );
    record("find", call(&w.srv, "find", r#"{"query":"level"}"#));
    record(
        "whatif",
        call(
            &w.srv,
            "whatif",
            &format!(
                r#"{{"request":{},"overrides":[{{"cell":{},"value":"99"}}]}}"#,
                q(&format!("{}()", calc_name(&wide))),
                q(&level_call(0))
            ),
        ),
    );
    record("verify", call(&w.srv, "verify", &format!(r#"{{"call":{}}}"#, q(&level_call(0)))));
    let over: Vec<&(String, usize)> = sizes.iter().filter(|(_, b)| *b > BUDGET).collect();
    assert!(over.is_empty(), "over the budget of {BUDGET} bytes: {over:?} (all: {sizes:?})");
}
