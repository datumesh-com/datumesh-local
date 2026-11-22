//! Build 2 Part 3 addition, `calc` in datumesh-local (CONTRACT-build2-part3-calc C1 to C6; D8, D27,
//! D28, D29). Written before the implementation.
//!
//! Shapes these tests specify (none of it exists yet):
//!
//! * `tools/list` has `calc` (the ten earlier tools keep their order). Arguments, with
//!   `additionalProperties: false`, `required: ["formula"]`: `formula` (a formula of the formula
//!   language with no arguments of its own; calls are full calls, `local.invalid/level(day: ...)`
//!   for a local cell), `asof` (an instant), `remote` (boolean). Its description says what C5 says.
//! * The answer (structured content; a result that is not an error): `kind: "calculation"`,
//!   `value`, `type`, `formula` (as given), `asof`, `lineage` (an array; each entry has `call`,
//!   `receipt` (hash), `publisher`, `trust`) and, for a local calculation, `name`:
//!   `local/calc_<h>`, `<h>` the first 16 hex digits of the SHA-256 of the canonical formula
//!   (the printed canonical AST of 15.6, no final LF: `datumesh_formula::canonical_ast(f,
//!   None).print()`).
//! * An error is an error result whose structured content has `error` (or `fault`) with the
//!   formula library's or engine's code.
//! * The CLI: `datumesh calc <formula> [--asof T] [--remote]` writes the answer as JSON on stdout,
//!   exit 0; an error exits non-zero.
//! * `remote: true` / `--remote`: `POST` of `{"formula": ..., "asof"?}` as JSON to the forward
//!   mesh's `x_calc` (its mesh document); the answer is shown as it is; nothing is memoised.
//! * `get` of a slice call (`*` for the key) with more than 100 members answers (structured
//!   content, and the same JSON from the CLI): `summary` (`count`, `first`, `last`, and for a
//!   number slice `min`, `max`, `mean`), `members` (the first 100) and `calc` (the call that
//!   aggregates the slice: `{"formula": "mean(<slice call>)"}` for numbers, `count(...)` else; it
//!   may also be written as the string `calc {"formula": ...}`). A slice of 100 or fewer has none
//!   of these.

#![cfg(unix)]

mod common;
#[path = "support/whatif_stub.rs"]
mod whatif_stub;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Transport};
use datumesh_local::{Config, Resolver, Source};
use whatif_stub::{DocMode, WhatifStub};

const M: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

const CELLS_TX: &str = "2026-11-15T11:00:00.000000Z";
const T: &str = "2026-11-15T12:00:00.000000Z";
const T2: &str = "2026-11-16T12:00:00.000000Z";
const T3: &str = "2026-11-17T12:00:00.000000Z";
const LEVEL_SIG: &str = r#"{"args": {"day": {"type": "date", "valid": true}}, "order": ["day"], "returns": {"type": "number", "unit": "1"}}"#;

/// `level(2026-11-01)` is 10 and `level(2026-11-02)` 20 until a new version says otherwise.
const F1: &str = "local.invalid/level(day: 2026-11-01) * 2";
const F1_SPACED: &str = "  local.invalid/level(day: 2026-11-01)   *2 ";
const F2: &str = "local.invalid/level(day: 2026-11-01) + local.invalid/level(day: 2026-11-02)";
/// An upstream input: `fx_rate(EUR, USD, 2026-10-30)` times 100.
const FX: &str = r#"mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30) * 100"#;

fn q(s: &str) -> String {
    String::from_utf8(Value::String(s.to_string()).to_jcs()).unwrap()
}

fn jcs(v: &Value) -> String {
    String::from_utf8(v.to_jcs()).unwrap()
}

fn rpc(srv: &McpServer, method: &str, extra: &str) -> Value {
    let comma = if extra.is_empty() { "" } else { "," };
    let line = format!(r#"{{"jsonrpc":"2.0","id":1,"method":{},"params":{{{M}{comma}{extra}}}}}"#, q(method));
    json::parse(srv.handle(&line, Transport::Stdio).expect("a response").as_bytes()).unwrap()
}

fn tool(srv: &McpServer, name: &str, args: &str) -> Value {
    rpc(srv, "tools/call", &format!(r#""name":{},"arguments":{args}"#, q(name)))
}

fn result(v: &Value) -> &Value {
    v.get("result").unwrap_or_else(|| panic!("a result in {}", jcs(v)))
}

fn is_error(v: &Value) -> bool {
    result(v).get("isError") == Some(&Value::Bool(true))
}

fn structured(v: &Value) -> &Value {
    result(v).get("structuredContent").expect("structuredContent")
}

fn items(v: &Value) -> Vec<String> {
    match result(v).get("content") {
        Some(Value::Array(a)) => a.iter().map(|i| s(i, "text")).collect(),
        other => panic!("no content: {other:?}"),
    }
}

fn everything(v: &Value) -> String {
    format!("{}\n{}", items(v).join("\n"), jcs(structured(v)))
}

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        other => panic!("{k}: {other:?} in {}", jcs(v)),
    }
}

fn calc_args(formula: &str) -> String {
    format!(r#"{{"formula":{}}}"#, q(formula))
}

fn calc(srv: &McpServer, formula: &str) -> Value {
    tool(srv, "calc", &calc_args(formula))
}

/// The name `local/calc_<h>` of a formula (D28), and its full form.
fn calc_hash(formula: &str) -> String {
    let canonical = datumesh_formula::canonical_ast(formula, None).unwrap().print();
    datumesh_core::sha256_hex(canonical.as_bytes())[..16].to_string()
}

fn calc_name(formula: &str) -> String {
    format!("local/calc_{}", calc_hash(formula))
}

fn calc_full(formula: &str) -> String {
    format!("local.invalid/calc_{}", calc_hash(formula))
}

/// A number given as a JSON number or as its lexical string.
fn number(v: &Value) -> f64 {
    match v {
        Value::String(t) => t.parse().unwrap_or_else(|_| panic!("a number: {t}")),
        other => {
            other.as_number_text().and_then(|t| t.parse().ok()).unwrap_or_else(|| panic!("a number: {}", jcs(other)))
        }
    }
}

fn code_of(v: &Value) -> String {
    let sc = structured(v);
    match (sc.get("error"), sc.get("fault")) {
        (_, Some(Value::String(f))) => f.clone(),
        (Some(Value::String(e)), _) => e.clone(),
        _ => panic!("no code: {}", jcs(v)),
    }
}

// ---------------------------------------------------------------------------------------------
// The local mesh: a stored function `local/level` with two cells, as the whatif tests have.
// ---------------------------------------------------------------------------------------------

fn sig(t: &str) -> Value {
    json::parse(t.as_bytes()).unwrap()
}

fn level_line(day: &str, v: &str, tx: &str) -> String {
    format!(
        r#"{{"cell": {{"fn": "local/level", "args": {{"day": {{"type": "date", "value": "{day}"}}}}}}, "value": {{"type": "number", "value": "{v}"}}, "tx": "{tx}", "asof": "{tx}", "licence": "CC0-1.0", "summary": "A test cell."}}"#
    )
}

struct W {
    up: Upstream,
    dir: TempDir,
    clock: Arc<ManualClock>,
    r: Arc<Resolver>,
    srv: McpServer,
}

fn world() -> W {
    let up = upstream();
    let dir = TempDir::new("calc");
    let clock = ManualClock::new("2026-11-15T10:00:00.000000Z");
    let r = Arc::new(resolver_at(&dir, &up, &clock));
    r.define_stored("local/level", &sig(LEVEL_SIG)).unwrap();
    let tmp = TempDir::new("calc-tmp");
    let f = jsonl(
        &tmp,
        "level.jsonl",
        &[&level_line("2026-11-01", "10", CELLS_TX), &level_line("2026-11-02", "20", CELLS_TX)],
    );
    r.publish(&f).unwrap();
    clock.set(T);
    let srv = McpServer::new(r.clone());
    W { up, dir, clock, r, srv }
}

impl W {
    /// A new version of `level(2026-11-01)`, at `at` (the clock is moved there).
    fn new_level(&self, v: &str, at: &str) {
        self.clock.set(at);
        let tmp = TempDir::new("calc-new");
        let f = jsonl(&tmp, "level2.jsonl", &[&level_line("2026-11-01", v, at)]);
        self.r.publish(&f).unwrap();
    }

    /// The held cell `local/calc_<h>()`: a get of it (never an evaluation of its own).
    fn held(&self, formula: &str) -> datumesh_local::Reply {
        self.r.get(&format!("{}()", calc_name(formula))).unwrap()
    }
}

// ---------------------------------------------------------------------------------------------
// The tool and its description (C5)
// ---------------------------------------------------------------------------------------------

#[test]
fn tools_list_has_calc_with_the_c5_wording() {
    let up = upstream();
    let dir = TempDir::new("calc-list");
    let srv = McpServer::new(Arc::new(resolver(&dir, &up)));
    let v = rpc(&srv, "tools/list", "");
    let tools = arr(result(&v), "tools");
    let names: Vec<&str> = tools.iter().map(|t| t.get("name").and_then(Value::as_str).unwrap()).collect();
    assert_eq!(names.iter().filter(|n| **n == "calc").count(), 1, "{names:?}");
    let earlier: Vec<&str> = names.iter().copied().filter(|n| *n != "calc").collect();
    assert_eq!(
        earlier,
        ["find", "get", "explain", "describe", "verify", "define", "publish", "subscribe", "whatif", "publish_public"]
    );
    let t = tools.iter().find(|t| s(t, "name") == "calc").unwrap();
    let d = s(t, "description").to_lowercase();
    // calculate here and not in code, because it runs beside the data and returns only the result,
    // with its lineage
    assert!(d.contains("calculate here"), "{d}");
    assert!(d.contains("not in code"), "{d}");
    assert!(d.contains("beside the data"), "{d}");
    assert!(d.contains("returns only the result"), "{d}");
    assert!(d.contains("lineage"), "{d}");
    // try find for an existing function first
    assert!(d.contains("find") && d.contains("existing function") && d.contains("first"), "{d}");
    // memoised on this machine
    assert!(d.contains("memoised") && d.contains("this machine"), "{d}");
    let schema = t.get("inputSchema").unwrap();
    assert_eq!(schema.get("additionalProperties"), Some(&Value::Bool(false)));
    assert_eq!(jcs(schema.get("required").unwrap()), r#"["formula"]"#);
    let props = schema.get("properties").unwrap();
    for k in ["formula", "asof", "remote"] {
        assert!(props.get(k).is_some(), "property {k}");
    }
    // get says that a large slice gives a calc call to aggregate it
    let g = tools.iter().find(|t| s(t, "name") == "get").unwrap();
    let gd = s(g, "description").to_lowercase();
    assert!(gd.contains("calc") && gd.contains("slice"), "{gd}");
}

// ---------------------------------------------------------------------------------------------
// A local calc: the answer, the memoised cell, the catalogue entry (C1, C2, D28)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_local_calc_answers_with_its_value_name_and_lineage() {
    let w = world();
    let v = calc(&w.srv, F1);
    assert!(!is_error(&v), "{}", jcs(&v));
    let sc = structured(&v);
    assert_eq!(s(sc, "kind"), "calculation", "{}", jcs(sc));
    assert_eq!(s(sc, "value"), "20");
    assert_eq!(s(sc, "type"), "number");
    assert_eq!(s(sc, "formula"), F1);
    assert_eq!(s(sc, "name"), calc_name(F1));
    assert!(sc.get("asof").is_some(), "{}", jcs(sc));
    // the lineage: the cell read, with its call, receipt hash, publisher and trust
    let l = arr(sc, "lineage");
    assert_eq!(l.len(), 1, "{}", jcs(sc));
    assert!(s(&l[0], "call").contains("level(day: 2026-11-01)"), "{}", jcs(&l[0]));
    for k in ["receipt", "publisher", "trust"] {
        assert!(!s(&l[0], k).is_empty(), "{k}: {}", jcs(&l[0]));
    }
    // the receipt is one this machine holds, for that cell
    let hash = s(&l[0], "receipt");
    let held = w.r.store().receipt(&hash);
    assert!(held.is_some(), "the lineage's receipt {hash} is held");
    // never presented as verified in a way that a public mesh's answer would be
    assert!(sc.get("trust").is_none(), "the calculation itself has no trust member: {}", jcs(sc));
}

#[test]
fn the_result_is_kept_as_a_local_cell_signed_by_the_machine() {
    let w = world();
    let v = calc(&w.srv, F1);
    assert!(!is_error(&v), "{}", jcs(&v));
    // the function is bound under the name, with no arguments
    let full = calc_full(F1);
    assert!(w.r.store().resolve_binding(&full, END).unwrap().is_some(), "{full} is bound");
    // its cell is held: a get of `local/calc_<h>()` is answered from the store
    let a = w.held(F1);
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(a.source, Source::Local);
    let rc = receipt(&a.body);
    assert_eq!(s(&rc, "value"), "20");
    assert_eq!(s(&rc, "computed_by"), format!("local.invalid#{}", w.r.key_id()));
    assert_eq!(s(rc.get("cell").unwrap(), "fn"), format!("calc_{}", calc_hash(F1)));
    assert!(w.r.store().computed_asof(&a.receipt().unwrap().cell().id(), END).unwrap().is_some());
    // the answer's lineage is the held cell's inputs
    let sc = structured(&v);
    let want: Vec<String> = match rc.get("inputs") {
        Some(Value::Array(i)) => i.iter().map(|h| h.as_str().unwrap().to_string()).collect(),
        _ => panic!("inputs"),
    };
    let got: Vec<String> = arr(sc, "lineage").iter().map(|e| s(e, "receipt")).collect();
    assert_eq!(got, want);
    // it verifies here
    let ver = w.r.verify(&rc.to_jcs());
    assert_eq!(ver.get("ok"), Some(&Value::Bool(true)), "{}", jcs(&ver));
}

#[test]
fn a_local_calc_is_found_later_by_find_and_a_new_resolver_reads_the_cell() {
    let w = world();
    let first = calc(&w.srv, F1);
    assert!(!is_error(&first), "{}", jcs(&first));
    let body_before = w.held(F1).body;
    let h = calc_hash(F1);
    // a new resolver on the same home (a later session)
    let W { up, dir, clock, r, srv, .. } = w;
    drop(srv);
    drop(r);
    let r2 = Arc::new(resolver_at(&dir, &up, &clock));
    let srv2 = McpServer::new(r2.clone());
    let v = tool(&srv2, "find", r#"{"query":"calc"}"#);
    assert!(!is_error(&v), "{}", jcs(&v));
    let found: Vec<&Value> = arr(structured(&v), "results").iter().filter(|x| s(x, "name").contains(&h)).collect();
    assert_eq!(found.len(), 1, "{}", jcs(structured(&v)));
    let f = found[0];
    assert_eq!(s(f, "name"), format!("local.invalid/calc_{h}"));
    assert_eq!(s(f, "kind"), "function");
    assert_eq!(s(f, "label"), "calc", "{}", jcs(f));
    let canonical = datumesh_formula::canonical_ast(F1, None).unwrap().print();
    let desc = s(f, "description");
    assert!(desc == F1 || desc == canonical, "the formula is the description: {desc}");
    // `get local/calc_<h>()` answers from the store, the same bytes, with no request
    let before = up.requests();
    let a = r2.get(&format!("local/calc_{h}()")).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(a.source, Source::Local);
    assert_eq!(a.body, body_before);
    assert_eq!(s(&receipt(&a.body), "value"), "20");
    assert_eq!(up.requests(), before);
    // and the same calc again is not run again either
    let again = calc(&srv2, F1);
    assert_eq!(s(structured(&again), "value"), "20");
    assert_eq!(r2.get(&format!("local/calc_{h}()")).unwrap().body, body_before);
}

// ---------------------------------------------------------------------------------------------
// Memoisation (D28): the same calculation is not run twice
// ---------------------------------------------------------------------------------------------

#[test]
fn a_second_identical_calc_does_not_evaluate_again() {
    let w = world();
    let v1 = calc(&w.srv, FX);
    assert!(!is_error(&v1), "{}", jcs(&v1));
    assert_eq!(s(structured(&v1), "type"), "number");
    let first_value = s(structured(&v1), "value");
    assert!(!first_value.is_empty());
    let after_first = w.up.requests();
    assert!(after_first > 0, "the input was fetched for the first evaluation");
    let held = w.held(FX).body;
    // same clock: no request, the same cell
    let v2 = calc(&w.srv, FX);
    assert_eq!(s(structured(&v2), "value"), first_value);
    assert_eq!(w.up.requests(), after_first, "no request for the second calc");
    assert_eq!(w.held(FX).body, held, "the same held cell, not a new evaluation");
    // later in time, with no new input version: still the held cell
    w.clock.set(T2);
    let v3 = calc(&w.srv, FX);
    assert_eq!(s(structured(&v3), "value"), first_value);
    assert_eq!(w.up.requests(), after_first);
    assert_eq!(w.held(FX).body, held);
    assert_eq!(s(structured(&v3), "name"), s(structured(&v1), "name"));
    // the lineage is the same receipts
    assert_eq!(jcs(structured(&v2).get("lineage").unwrap()), jcs(structured(&v1).get("lineage").unwrap()));
}

#[test]
fn a_new_version_of_an_input_makes_it_recompute() {
    let w = world();
    let v1 = calc(&w.srv, F1);
    assert_eq!(s(structured(&v1), "value"), "20");
    let held1 = w.held(F1).body;
    w.new_level("50", T2);
    let v2 = calc(&w.srv, F1);
    assert!(!is_error(&v2), "{}", jcs(&v2));
    assert_eq!(s(structured(&v2), "value"), "100", "{}", jcs(structured(&v2)));
    assert_eq!(s(structured(&v2), "name"), s(structured(&v1), "name"), "the same name");
    let held2 = w.held(F1);
    assert_ne!(held2.body, held1, "a new version of the held cell");
    assert_eq!(s(&receipt(&held2.body), "value"), "100");
    // the lineage now names the new input receipt
    let r1 = s(&arr(structured(&v1), "lineage")[0], "receipt");
    let r2 = s(&arr(structured(&v2), "lineage")[0], "receipt");
    assert_ne!(r1, r2);
    // and again with no further change: no new version
    w.clock.set(T3);
    let v3 = calc(&w.srv, F1);
    assert_eq!(s(structured(&v3), "value"), "100");
    assert_eq!(w.held(F1).body, held2.body);
}

#[test]
fn equivalent_spellings_share_one_name_and_one_evaluation() {
    let w = world();
    let a = calc(&w.srv, F1);
    let held = w.held(F1).body;
    let b = calc(&w.srv, F1_SPACED);
    assert!(!is_error(&b), "{}", jcs(&b));
    assert_eq!(s(structured(&a), "name"), calc_name(F1));
    assert_eq!(s(structured(&b), "name"), calc_name(F1), "canonical-equivalent formulas share a name");
    assert_eq!(calc_name(F1), calc_name(F1_SPACED));
    assert_eq!(s(structured(&b), "value"), "20");
    assert_eq!(s(structured(&b), "formula"), F1_SPACED, "the formula is as given");
    assert_eq!(w.held(F1).body, held, "one held cell");
    // a different formula is another name
    let c = calc(&w.srv, F2);
    assert_eq!(s(structured(&c), "value"), "30");
    assert_ne!(s(structured(&c), "name"), s(structured(&a), "name"));
    assert_eq!(s(structured(&c), "name"), calc_name(F2));
    assert_eq!(arr(structured(&c), "lineage").len(), 2);
}

#[test]
fn asof_reads_the_data_as_of_that_instant() {
    let w = world();
    w.new_level("50", T2);
    w.clock.set(T3);
    let old = calc(&w.srv, F1);
    let now_value = s(structured(&old), "value");
    assert_eq!(now_value, "100");
    let args = format!(r#"{{"formula":{},"asof":{}}}"#, q(F1), q(T));
    let v = tool(&w.srv, "calc", &args);
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_eq!(s(structured(&v), "value"), "20", "{}", jcs(structured(&v)));
    assert_eq!(s(structured(&v), "asof"), T);
    // the answer without asof is still the latest
    assert_eq!(s(structured(&calc(&w.srv, F1)), "value"), "100");
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

#[test]
fn formula_errors_carry_their_codes_and_bind_nothing() {
    let w = world();
    let typed = format!("{F1} + \"a\"");
    let absent = "local.invalid/level(day: 2030-01-01) * 2";
    for (what, f, code) in [
        ("a syntax error", "local.invalid/level(day: 2026-11-01) *".to_string(), "E_FORMULA_SYNTAX"),
        ("a type error", typed.clone(), "E_FORMULA_TYPE"),
        ("an absent cell", absent.to_string(), "E_FORMULA_ABSENT"),
    ] {
        let v = calc(&w.srv, &f);
        assert!(is_error(&v), "{what}: {}", jcs(&v));
        assert_eq!(code_of(&v), code, "{what}: {}", jcs(&v));
        assert_ne!(structured(&v).get("kind"), Some(&Value::String("calculation".into())), "{what}");
    }
    // a refused formula is never bound
    let h = calc_hash(&typed);
    assert!(w.r.store().resolve_binding(&format!("local.invalid/calc_{h}"), END).unwrap().is_none());
    // no arguments of its own
    let v = calc(&w.srv, "x + 1");
    assert!(is_error(&v), "{}", jcs(&v));
    assert!(code_of(&v).starts_with("E_FORMULA_"), "{}", jcs(&v));
    // arguments are checked
    let v = tool(&w.srv, "calc", r#"{}"#);
    assert!(is_error(&v) || v.get("error").is_some(), "{}", jcs(&v));
    let v = tool(&w.srv, "calc", &format!(r#"{{"formula":{},"extra":1}}"#, q(F1)));
    assert!(is_error(&v) || v.get("error").is_some(), "{}", jcs(&v));
}

// ---------------------------------------------------------------------------------------------
// --remote: the forward mesh's x_calc, nothing memoised (C3)
// ---------------------------------------------------------------------------------------------

struct R {
    up: Upstream,
    stub: Arc<WhatifStub>,
    r: Arc<Resolver>,
    srv: McpServer,
    dir: TempDir,
}

fn remote_world() -> R {
    let up = upstream();
    let stub = WhatifStub::start(&up, DocMode::Peer);
    let dir = TempDir::new("calc-remote");
    let mut c: Config = config(&dir, &up);
    c.peers.insert(HOST.to_string(), stub.origin.clone());
    assert_eq!(c.forward_mesh.as_deref(), Some(HOST), "the default forward mesh is the test mesh");
    let r = Arc::new(Resolver::open(c).unwrap());
    let srv = McpServer::new(r.clone());
    R { up, stub, r, srv, dir }
}

/// What `POST /x/calc` of a public mesh answers (C1, C3): a calculation with no receipt and no
/// trust, and the lineage of signed input receipts.
fn remote_answer(formula: &str, value: &str) -> String {
    let canonical = datumesh_formula::canonical_ast(formula, None).unwrap().print();
    format!(
        r#"{{"kind":"calculation","value":{},"type":"number","formula":{},"canonical":{},"asof":"2026-11-16T00:00:00.000000Z","lineage":[{{"call":{},"receipt":{},"publisher":"mesh-test.datumesh.com","trust":"primary"}}]}}"#,
        q(value),
        q(formula),
        q(&canonical),
        q(r#"mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#),
        q(&"ab".repeat(32)),
    )
}

#[test]
fn remote_asks_the_forward_meshs_x_calc_and_memoises_nothing() {
    let w = remote_world();
    w.stub.answer_calc(200, &remote_answer(FX, "116.37"));
    let args = format!(r#"{{"formula":{},"remote":true}}"#, q(FX));
    let v = tool(&w.srv, "calc", &args);
    assert!(!is_error(&v), "{}", jcs(&v));
    let posts = w.stub.posts_to("/x/calc");
    assert_eq!(posts.len(), 1, "one POST to the x_calc endpoint: {:?}", w.stub.posts());
    assert_eq!(posts[0].content_type.split(';').next().unwrap().trim(), "application/json");
    let body = posts[0].json();
    let sent = s(&body, "formula");
    assert_eq!(
        datumesh_formula::canonical_ast(&sent, None).unwrap().print(),
        datumesh_formula::canonical_ast(FX, None).unwrap().print(),
        "the formula sent is the formula asked"
    );
    assert!(body.get("asof").is_none(), "{}", jcs(&body));
    let sc = structured(&v);
    assert_eq!(s(sc, "kind"), "calculation");
    assert_eq!(s(sc, "value"), "116.37");
    assert_eq!(s(sc, "type"), "number");
    assert_eq!(arr(sc, "lineage").len(), 1);
    assert!(sc.get("receipt").is_none() && sc.get("trust").is_none(), "{}", jcs(sc));
    assert!(sc.get("name").is_none(), "nothing is named: {}", jcs(sc));
    // nothing is memoised here: no function bound, nothing evaluated, nothing fetched
    let full = calc_full(FX);
    assert!(w.r.store().resolve_binding(&full, END).unwrap().is_none(), "{full} is bound");
    let id_probe = w.r.get(&format!("{}()", calc_name(FX)));
    assert!(id_probe.map(|a| a.status != 200).unwrap_or(true), "a held cell for a remote calc");
    assert_eq!(w.stub.posts_to("/x/calc").len(), 1);
    // a second remote calc asks again (nothing was kept)
    let _ = tool(&w.srv, "calc", &args);
    assert_eq!(w.stub.posts_to("/x/calc").len(), 2);
    let _ = &w.up;
}

#[test]
fn remote_sends_asof_and_shows_the_meshs_errors() {
    let w = remote_world();
    w.stub.answer_calc(200, &remote_answer(FX, "116.37"));
    let args = format!(r#"{{"formula":{},"asof":{},"remote":true}}"#, q(FX), q(T));
    let v = tool(&w.srv, "calc", &args);
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_eq!(s(&w.stub.posts_to("/x/calc")[0].json(), "asof"), T);
    // the mesh's refusal is an error result with its code
    w.stub.answer_calc(500, r#"{"outcome":"error","error":"E_EVAL_BUDGET"}"#);
    let v = tool(&w.srv, "calc", &format!(r#"{{"formula":{},"remote":true}}"#, q(FX)));
    assert!(is_error(&v), "{}", jcs(&v));
    assert!(everything(&v).contains("E_EVAL_BUDGET"), "{}", jcs(&v));
    w.stub.answer_calc(429, r#"{"outcome":"rate_limited"}"#);
    let v = tool(&w.srv, "calc", &format!(r#"{{"formula":{},"remote":true}}"#, q(FX)));
    assert!(is_error(&v), "{}", jcs(&v));
    assert!(everything(&v).contains("rate_limited"), "{}", jcs(&v));
}

#[test]
fn remote_with_no_x_calc_endpoint_is_an_error_naming_it() {
    let w = remote_world();
    w.stub.no_calc_endpoint.store(true, std::sync::atomic::Ordering::SeqCst);
    let v = tool(&w.srv, "calc", &format!(r#"{{"formula":{},"remote":true}}"#, q(FX)));
    assert!(is_error(&v), "{}", jcs(&v));
    assert!(everything(&v).contains("x_calc"), "{}", jcs(&v));
    assert!(w.stub.posts().is_empty(), "nothing was posted anywhere");
}

#[test]
fn a_calc_that_is_not_remote_never_posts() {
    let w = remote_world();
    w.stub.answer_calc(200, &remote_answer(FX, "0"));
    let v = tool(&w.srv, "calc", &calc_args(FX));
    assert!(!is_error(&v), "{}", jcs(&v));
    assert!(w.stub.posts().is_empty(), "a calc evaluates locally, fetching its inputs as get does");
    assert_ne!(s(structured(&v), "value"), "0");
    assert_eq!(s(structured(&v), "name"), calc_name(FX));
    let _ = &w.dir;
}

// ---------------------------------------------------------------------------------------------
// The CLI
// ---------------------------------------------------------------------------------------------

fn home(up: &Upstream, dir: &TempDir) {
    Config::in_dir(dir.path()).unwrap();
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("\"peers\": {}", &format!("\"peers\": {{\"{HOST}\": \"{}\"}}", up.origin)))
        .unwrap();
}

fn run(dir: &TempDir, args: &[&str]) -> (i32, String, String) {
    let mut all = vec!["--home".to_string(), dir.path().to_string_lossy().into_owned()];
    all.extend(args.iter().map(|a| a.to_string()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&all, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

#[test]
fn the_cli_calc_writes_the_answer_and_memoises() {
    let up = upstream();
    let dir = TempDir::new("calc-cli");
    home(&up, &dir);
    // the data: `local/level`, defined and published through the library on this home
    {
        let clock = ManualClock::new("2026-11-15T10:00:00.000000Z");
        let r = Resolver::open_with_clock(
            {
                let mut c = Config::in_dir(dir.path()).unwrap();
                c.peers.insert(HOST.to_string(), up.origin.clone());
                c
            },
            clock.clone(),
        )
        .unwrap();
        r.define_stored("local/level", &sig(LEVEL_SIG)).unwrap();
        let tmp = TempDir::new("calc-cli-tmp");
        let f = jsonl(
            &tmp,
            "l.jsonl",
            &[&level_line("2026-11-01", "10", CELLS_TX), &level_line("2026-11-02", "20", CELLS_TX)],
        );
        r.publish(&f).unwrap();
    }
    let (code, out, err) = run(&dir, &["calc", F1]);
    assert_eq!(code, 0, "{err}");
    let a = json::parse(out.trim_end().as_bytes()).unwrap_or_else(|e| panic!("{e}: {out}"));
    assert_eq!(s(&a, "kind"), "calculation");
    assert_eq!(s(&a, "value"), "20");
    assert_eq!(s(&a, "name"), calc_name(F1));
    assert_eq!(arr(&a, "lineage").len(), 1);
    // the cell is kept: `get` of its name answers it, from the store
    let (code, out2, err) = run(&dir, &["--offline", "get", &format!("{}()", calc_name(F1))]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(s(&receipt(out2.trim_end().as_bytes()), "value"), "20");
    // the same calc again is the same answer, offline too
    let (code, again, err) = run(&dir, &["--offline", "calc", F1]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(s(&json::parse(again.trim_end().as_bytes()).unwrap(), "value"), "20");
    // an error exits non-zero and writes no calculation
    let (code, out, _) = run(&dir, &["calc", "local.invalid/level(day: 2026-11-01) *"]);
    assert_ne!(code, 0);
    assert!(!out.contains("\"calculation\""), "{out}");
    // no formula is a usage error
    let (code, _, _) = run(&dir, &["calc"]);
    assert_ne!(code, 0);
}

#[test]
fn the_cli_calc_remote_asks_x_calc_and_keeps_nothing() {
    let up = upstream();
    let stub = WhatifStub::start(&up, DocMode::Peer);
    let dir = TempDir::new("calc-cli-remote");
    Config::in_dir(dir.path()).unwrap();
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("\"peers\": {}", &format!("\"peers\": {{\"{HOST}\": \"{}\"}}", stub.origin)))
        .unwrap();
    stub.answer_calc(200, &remote_answer(FX, "116.37"));
    let (code, out, err) = run(&dir, &["calc", FX, "--remote"]);
    assert_eq!(code, 0, "{err}");
    let a = json::parse(out.trim_end().as_bytes()).unwrap_or_else(|e| panic!("{e}: {out}"));
    assert_eq!(s(&a, "value"), "116.37");
    assert_eq!(s(&a, "kind"), "calculation");
    assert!(a.get("name").is_none(), "{out}");
    assert_eq!(stub.posts_to("/x/calc").len(), 1);
    // nothing kept: a get of the would-be name finds nothing
    let (code, _, _) = run(&dir, &["--offline", "get", &format!("{}()", calc_name(FX))]);
    assert_ne!(code, 0);
    // --asof goes with it
    let (code, _, err) = run(&dir, &["calc", FX, "--remote", "--asof", T]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(s(&stub.posts_to("/x/calc")[1].json(), "asof"), T);
}

// ---------------------------------------------------------------------------------------------
// A large slice get (C4, D29)
// ---------------------------------------------------------------------------------------------

const MONTHS: [(&str, u32); 5] = [("01", 31), ("02", 28), ("03", 31), ("04", 30), ("05", 31)];

/// The `n` first days of 2026 as dates, in key order.
fn days(n: usize) -> Vec<String> {
    MONTHS.iter().flat_map(|(m, len)| (1..=*len).map(move |d| format!("2026-{m}-{d:02}"))).take(n).collect()
}

fn series_line(function: &str, day: &str, v: usize) -> String {
    format!(
        r#"{{"cell": {{"fn": "{function}", "args": {{"day": {{"type": "date", "value": "{day}"}}}}}}, "value": {{"type": "number", "value": "{v}"}}, "tx": "{CELLS_TX}", "asof": "{CELLS_TX}", "licence": "CC0-1.0", "summary": "A test cell."}}"#
    )
}

/// `local/rates` has 150 members (1 to 150 for 2026-01-01 on) and `local/hundred` has exactly 100.
fn slice_world() -> W {
    let w = world();
    w.clock.set("2026-11-15T10:30:00.000000Z");
    let tmp = TempDir::new("calc-series");
    for (name, n) in [("local/rates", 150), ("local/hundred", 100)] {
        w.r.define_stored(name, &sig(LEVEL_SIG)).unwrap();
        let lines: Vec<String> = days(n).iter().enumerate().map(|(i, d)| series_line(name, d, i + 1)).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let f = jsonl(&tmp, &format!("{}.jsonl", name.trim_start_matches("local/")), &refs);
        w.r.publish(&f).unwrap();
    }
    w.clock.set(T);
    w
}

/// The formula of the `calc` member: an object `{"formula": ...}` or the string `calc {...}`.
fn calc_formula(sc: &Value) -> String {
    match sc.get("calc").unwrap_or_else(|| panic!("a calc member in {}", jcs(sc))) {
        Value::String(t) => {
            let body = t.strip_prefix("calc ").unwrap_or_else(|| panic!("calc {{...}}: {t}"));
            s(&json::parse(body.as_bytes()).unwrap(), "formula")
        }
        o => s(o, "formula"),
    }
}

#[test]
fn a_large_number_slice_shows_a_summary_the_first_page_and_the_calc_call() {
    let w = slice_world();
    let v = tool(&w.srv, "get", r#"{"request":"local/rates(day: *)"}"#);
    assert!(!is_error(&v), "{}", jcs(&v));
    let sc = structured(&v);
    // the summary
    let sum = sc.get("summary").unwrap_or_else(|| panic!("a summary in {}", jcs(sc)));
    assert_eq!(number(sum.get("count").unwrap()), 150.0, "{}", jcs(sum));
    assert!(jcs(sum.get("first").unwrap()).contains("2026-01-01"), "{}", jcs(sum));
    assert!(jcs(sum.get("last").unwrap()).contains("2026-05-30"), "{}", jcs(sum));
    assert_eq!(number(sum.get("min").unwrap()), 1.0);
    assert_eq!(number(sum.get("max").unwrap()), 150.0);
    assert_eq!(number(sum.get("mean").unwrap()), 75.5);
    // the first page: 100 members, in key order
    let members = arr(sc, "members");
    assert_eq!(members.len(), 100, "{}", members.len());
    assert!(jcs(&members[0]).contains("2026-01-01"), "{}", jcs(&members[0]));
    assert!(jcs(&members[99]).contains("2026-04-10"), "{}", jcs(&members[99]));
    assert!(!jcs(&Value::Array(members.to_vec())).contains("2026-04-11"));
    // the exact calc call
    let f = calc_formula(sc);
    assert!(f == "mean(local.invalid/rates(day: *))" || f == "mean(local/rates(day: *))", "{f}");
    let text = everything(&v);
    assert!(text.contains("calc") && text.contains("mean("), "the call is shown: {text}");
    // run, it gives the aggregate
    let r = calc(&w.srv, &f);
    assert!(!is_error(&r), "{f}: {}", jcs(&r));
    assert_eq!(number(structured(&r).get("value").unwrap()), 75.5);
    assert_eq!(number(structured(&r).get("value").unwrap()), number(sum.get("mean").unwrap()));
    assert_eq!(s(structured(&r), "kind"), "calculation");
}

#[test]
fn a_slice_of_100_or_fewer_has_no_summary_and_no_calc_call() {
    let w = slice_world();
    // exactly 100 members is not large (D29: more than 100): the members, no summary, no calc call
    let v = tool(&w.srv, "get", r#"{"request":"local/hundred(day: *)"}"#);
    assert!(!is_error(&v), "{}", jcs(&v));
    let sc = structured(&v);
    assert_eq!(arr(sc, "members").len(), 100, "{}", jcs(sc));
    assert!(sc.get("summary").is_none() && sc.get("calc").is_none(), "{}", jcs(sc));
    assert!(!everything(&v).contains("calc {"), "{}", everything(&v));
    // a slice of a mesh, with fewer than 100 members: the same
    let v = tool(
        &w.srv,
        "get",
        r#"{"request":"mesh-test.datumesh.com/ecb/fx_rate(base: \"EUR\", quote: \"USD\", date: *)"}"#,
    );
    assert!(!is_error(&v), "{}", jcs(&v));
    let sc = structured(&v);
    assert_eq!(arr(sc, "members").len(), 30, "{}", jcs(sc));
    assert!(sc.get("summary").is_none() && sc.get("calc").is_none(), "{}", jcs(sc));
}

/// Not a number slice: `count(...)`, and a summary without min, max or mean.
#[test]
fn a_large_slice_of_another_type_gets_count() {
    let w = slice_world();
    let slice = r#"mesh-test.datumesh.com/ecb/holiday(calendar: "TARGET", date: *)"#;
    let v = tool(&w.srv, "get", &format!(r#"{{"request":{}}}"#, q(slice)));
    assert!(!is_error(&v), "{}", jcs(&v));
    let sc = structured(&v);
    let sum = sc.get("summary").unwrap_or_else(|| panic!("a summary in {}", jcs(sc)));
    assert_eq!(number(sum.get("count").unwrap()), 365.0, "{}", jcs(sum));
    for k in ["min", "max", "mean"] {
        assert!(sum.get(k).is_none(), "{k} is for number slices: {}", jcs(sum));
    }
    assert_eq!(arr(sc, "members").len(), 100);
    let f = calc_formula(sc);
    assert_eq!(f, format!("count({slice})"));
    let r = calc(&w.srv, &f);
    assert!(!is_error(&r), "{f}: {}", jcs(&r));
    assert_eq!(number(structured(&r).get("value").unwrap()), 365.0);
}

#[test]
fn the_cli_get_of_a_large_slice_gives_the_same() {
    let up = upstream();
    let dir = TempDir::new("calc-cli-slice");
    home(&up, &dir);
    {
        let clock = ManualClock::new("2026-11-15T10:00:00.000000Z");
        let mut c = Config::in_dir(dir.path()).unwrap();
        c.peers.insert(HOST.to_string(), up.origin.clone());
        let r = Resolver::open_with_clock(c, clock).unwrap();
        r.define_stored("local/rates", &sig(LEVEL_SIG)).unwrap();
        let lines: Vec<String> =
            days(150).iter().enumerate().map(|(i, d)| series_line("local/rates", d, i + 1)).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let tmp = TempDir::new("calc-cli-slice-tmp");
        r.publish(&jsonl(&tmp, "s.jsonl", &refs)).unwrap();
    }
    let (code, out, err) = run(&dir, &["get", "local/rates(day: *)"]);
    assert_eq!(code, 0, "{err}");
    let a = json::parse(out.trim_end().as_bytes()).unwrap_or_else(|e| panic!("{e}: {out}"));
    assert_eq!(number(a.get("summary").unwrap().get("count").unwrap()), 150.0);
    assert_eq!(arr(&a, "members").len(), 100);
    let f = calc_formula(&a);
    assert!(f == "mean(local.invalid/rates(day: *))" || f == "mean(local/rates(day: *))", "{f}");
    let (code, out, err) = run(&dir, &["calc", &f]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(number(json::parse(out.trim_end().as_bytes()).unwrap().get("value").unwrap()), 75.5);
}
