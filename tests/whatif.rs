//! CONTRACT-build2-part3-calculus E6 (datumesh-local 0.5.0 or 0.4.x, as released), the MCP verb
//! `whatif`: the spreadsheet what-if, a scenario that is never a fact. Written before the
//! implementation; the new tests live in their own files (`whatif.rs`, `define_tests.rs`,
//! `publish_public.rs`) so that every other test file keeps compiling.
//!
//! Shapes these tests specify (none of it exists yet):
//!
//! * `tools/list` gains `whatif` and `publish_public`, appended after `subscribe`:
//!   `[find, get, explain, describe, verify, define, publish, subscribe, whatif, publish_public]`.
//!   (tests/mcp_stdio.rs asserts the old 8: lines 102 and 288 are updated by the implementer.)
//! * `whatif` arguments (`additionalProperties: false`, `required: ["request"]`):
//!   - `request`: a call or a plain reference, as `get` takes it;
//!   - `overrides`: an array of `{cell, value}` (a call or reference, and a plain literal: a JSON
//!     string, number or boolean, typed by the cell's return type, which the tool learns from the
//!     cell's function descriptor) or `{slice, add: [{cell, value}], remove: [cell]}`; optional
//!     with `solve`, otherwise at least one;
//!   - `asof`: optional instant;
//!   - `solve`: optional `{vary, target, equals, lo, hi}` (calls or references; numbers as strings
//!     or JSON numbers).
//! * The tool POSTs spec-0.6.0 5.9's JSON body to the `whatif` endpoint of the mesh document of the
//!   answering mesh (the host of the request's callee, reached through `peers` like every other
//!   endpoint): `cell` (a canonical full call, no asof in it), `asof` only when given,
//!   `overrides` (each `{cell: <call>, value: {type, value}}`, or `{slice, add, remove}` with the
//!   members' values typed the same way; `[]` with solve and none), `solve` only when given
//!   (`vary`, `target`, `equals`, `lo`, `hi`: calls and number lexical forms). Content type
//!   `application/json`. The answer is checked with core 0.3.9's `check_answer` against the request
//!   (the cell id, asof, canonical overrides, `solved`); any failure is `E_ENVELOPE`.
//! * The result of a scenario (not an error): the FIRST content item's text starts with
//!   `Scenario (not a fact):`, then the requested call, each override (its cell or slice and its
//!   value) and the value; with `solve`, the word `solved` and the value found. It never says
//!   `verified` and has no receipt block. Structured content: `outcome: "scenario"`, `call`,
//!   `type`, `value`, `asof`, `overrides` (`{cell, value}` or `{slice, add: [{cell, value}],
//!   remove: [cell]}`, calls and lexical values) and, with solve, `solved: {vary, value}`. It has no
//!   `receipt` and no `trust`.
//! * Not a scenario: `bad_request` (`E_WHATIF_UNSUPPORTED`), `not_covered` (`E_WHATIF_NO_SOLUTION`),
//!   a refused envelope (`E_ENVELOPE`), no `whatif` endpoint in the mesh document, and a value that
//!   does not fit the cell's type (`E_CANON_TYPE`, before anything is sent) are error results whose
//!   structured content has `error` (the code, or `not_covered` with `fault`).
//! * For the local namespace the scenario is evaluated here, by the shared engine: nothing is
//!   written, and the normal answers are unchanged.

#![cfg(unix)]

mod common;
#[path = "support/whatif_stub.rs"]
mod whatif_stub;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_core::{check_answer, Answer, Cell, ErrorCode, Fault, Keys, Request};
use datumesh_local::mcp::{McpServer, Transport};
use datumesh_local::{Config, Resolver};
use whatif_stub::{DocMode, WhatifStub};

const M: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

const NOW: &str = "2026-11-19T10:00:00.000000Z";

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

// ---------------------------------------------------------------------------------------------
// Canned scenarios, valid per core 0.3.9's check_answer (checked when they are built).
// ---------------------------------------------------------------------------------------------

fn obj(mut m: Vec<(&str, Value)>) -> Value {
    m.sort_by(|a, b| a.0.cmp(b.0));
    Value::Object(m.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn st(t: &str) -> Value {
    Value::String(t.to_string())
}

fn typed(ty: &str, v: &str) -> Value {
    obj(vec![("type", st(ty)), ("value", st(v))])
}

/// The cell (4.4) of `name` (a mesh-test function) with `args` (name, type, lexical value).
fn cell_json(up: &Upstream, name: &str, args: &[(&str, &str, &str)]) -> Value {
    let hash = up.store.resolve_binding(name, END).unwrap().expect("bound").fn_hash;
    let (ns, f) = name.rsplit_once('/').unwrap();
    obj(vec![
        ("ns", st(ns)),
        ("fn", st(f)),
        ("fn_hash", st(&hash)),
        ("args", obj(args.iter().map(|(k, t, v)| (*k, typed(t, v))).collect())),
    ])
}

fn cell_id(c: &Value) -> String {
    Cell::from_json(&c.to_jcs()).unwrap().id()
}

/// `fx_rate(EUR, USD, 2026-10-30)`: the cell overridden.
const RATE_CALL: &str = r#"mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#;
/// The requested cell: a computed function of the test mesh.
const TARGET_CALL: &str =
    r#"mesh-test.datumesh.com/ecb/fx_cross_undeclared(base: "EUR", quote: "USD", date: 2026-10-30)"#;
const ARGS: [(&str, &str, &str); 3] =
    [("base", "string", "EUR"), ("date", "date", "2026-10-30"), ("quote", "string", "USD")];

fn rate_cell(up: &Upstream) -> Value {
    cell_json(up, "mesh-test.datumesh.com/ecb/fx_rate", &ARGS)
}

fn target_cell(up: &Upstream) -> Value {
    cell_json(up, "mesh-test.datumesh.com/ecb/fx_cross_undeclared", &ARGS)
}

/// The scenario envelope for a request of `target` as of `asof_sent` with `overrides`, answering
/// `value` as of `asof`; `solved` is `(vary cell, found value)`. Checked with core's `check_answer`.
fn scenario(
    target: &Value,
    asof_sent: Option<&str>,
    overrides: &[Value],
    value: &str,
    asof: &str,
    solved: Option<(&Value, &str)>,
) -> String {
    let mut members = vec![
        ("asof", st(asof)),
        ("cell", target.clone()),
        ("overrides", Value::Array(overrides.to_vec())),
        ("type", st("number")),
        ("value", st(value)),
    ];
    if let Some((vary, v)) = solved {
        members.push(("solved", obj(vec![("vary", vary.clone()), ("value", typed("number", v))])));
    }
    let body = jcs(&obj(vec![("outcome", st("scenario")), ("scenario", obj(members))]));
    let request = Request::new(&cell_id(target), asof_sent)
        .unwrap()
        .with_whatif(&Value::Array(overrides.to_vec()), solved.map(|(c, _)| cell_id(c)).as_deref())
        .unwrap();
    check_answer(200, body.as_bytes(), &Keys::new(), &request).unwrap_or_else(|e| panic!("{e}: {body}"));
    body
}

fn value_override(cell: &Value, v: &str) -> Value {
    obj(vec![("cell", cell.clone()), ("value", typed("number", v))])
}

struct W {
    up: Upstream,
    stub: Arc<WhatifStub>,
    srv: McpServer,
    r: Arc<Resolver>,
    _dir: TempDir,
}

fn world_with(mode: DocMode) -> W {
    let up = upstream();
    let stub = WhatifStub::start(&up, mode);
    let dir = TempDir::new("whatif");
    let mut c: Config = config(&dir, &up);
    c.peers.insert(HOST.to_string(), stub.origin.clone());
    let r = Arc::new(Resolver::open(c).unwrap());
    let srv = McpServer::new(r.clone());
    W { up, stub, srv, r, _dir: dir }
}

fn world() -> W {
    world_with(DocMode::Peer)
}

fn whatif(w: &W, args: &str) -> Value {
    tool(&w.srv, "whatif", args)
}

fn one_post(w: &W) -> Value {
    let posts = w.stub.posts_to("/whatif");
    assert_eq!(posts.len(), 1, "one POST to /whatif: {:?}", w.stub.posts());
    assert_eq!(posts[0].content_type.split(';').next().unwrap().trim(), "application/json");
    posts[0].json()
}

// ---------------------------------------------------------------------------------------------
// The tool and its description
// ---------------------------------------------------------------------------------------------

#[test]
fn tools_list_has_whatif_and_it_is_described_as_the_spreadsheet_what_if() {
    let up = upstream();
    let dir = TempDir::new("whatif-list");
    let srv = McpServer::new(Arc::new(resolver(&dir, &up)));
    let v = rpc(&srv, "tools/list", "");
    let tools = arr(result(&v), "tools");
    let names: Vec<&str> = tools.iter().map(|t| t.get("name").and_then(Value::as_str).unwrap()).collect();
    assert_eq!(
        names,
        [
            "find",
            "get",
            "explain",
            "describe",
            "verify",
            "define",
            "publish",
            "subscribe",
            "whatif",
            "calc",
            "publish_public",
        ]
    );
    let t = tools.iter().find(|t| s(t, "name") == "whatif").unwrap();
    let d = s(t, "description").to_lowercase();
    assert!(d.contains("what-if"), "the spreadsheet what-if: {d}");
    assert!(d.contains("spreadsheet"), "{d}");
    assert!(d.contains("diddle"), "for those who know SecDB-style systems: {d}");
    assert!(d.contains("tweak"), "{d}");
    assert!(d.contains("scenario"), "{d}");
    assert!(d.contains("never signed") && d.contains("stored"), "never signed or stored: {d}");
    let schema = t.get("inputSchema").unwrap();
    assert_eq!(schema.get("additionalProperties"), Some(&Value::Bool(false)));
    assert_eq!(jcs(schema.get("required").unwrap()), r#"["request"]"#);
    let props = schema.get("properties").unwrap();
    for k in ["request", "overrides", "asof", "solve"] {
        assert!(props.get(k).is_some(), "property {k}");
    }
}

// ---------------------------------------------------------------------------------------------
// A scenario from the answering mesh
// ---------------------------------------------------------------------------------------------

#[test]
fn a_value_override_is_posted_and_the_answer_is_a_labelled_scenario_with_no_receipt() {
    let w = world();
    let (rate, target) = (rate_cell(&w.up), target_cell(&w.up));
    w.stub.answer_whatif(200, &scenario(&target, None, &[value_override(&rate, "1.2")], "1.1337", NOW, None));
    let args = format!(
        r#"{{"request":{},"overrides":[{{"cell":{},"value":"1.2"}}]}}"#,
        q("ecb/fx_cross_undeclared(base: \"EUR\", quote: \"USD\", date: 2026-10-30)"),
        q(r#"ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#)
    );
    let v = whatif(&w, &args);
    assert!(!is_error(&v), "{}", jcs(&v));
    // What was sent: 5.9's body, calls canonical and full, the value typed by the cell's function.
    let body = one_post(&w);
    let cell = s(&body, "cell");
    assert!(cell.starts_with("mesh-test.datumesh.com/ecb/fx_cross_undeclared("), "{cell}");
    assert!(
        cell.contains(r#"base: "EUR""#) && cell.contains(r#"quote: "USD""#) && cell.contains("date: 2026-10-30"),
        "{cell}"
    );
    assert!(body.get("asof").is_none(), "no asof was given: {}", jcs(&body));
    assert!(body.get("solve").is_none());
    let o = arr(&body, "overrides");
    assert_eq!(o.len(), 1);
    let oc = s(&o[0], "cell");
    assert!(oc.starts_with("mesh-test.datumesh.com/ecb/fx_rate("), "{oc}");
    assert_eq!(jcs(o[0].get("value").unwrap()), r#"{"type":"number","value":"1.2"}"#);
    // What the agent reads.
    let c = items(&v);
    assert!(c[0].starts_with("Scenario (not a fact):"), "{:?}", c[0]);
    let all = everything(&v);
    assert!(all.contains("fx_cross_undeclared"), "the requested call: {all}");
    assert!(all.contains("fx_rate"), "the override is listed: {all}");
    assert!(all.contains("1.2"), "with its value: {all}");
    assert!(all.contains("1.1337"), "and the scenario's value: {all}");
    assert!(!all.to_lowercase().contains("verified"), "never says verified: {all}");
    let sc = structured(&v);
    assert_eq!(s(sc, "outcome"), "scenario");
    assert_eq!(s(sc, "value"), "1.1337");
    assert_eq!(s(sc, "type"), "number");
    assert_eq!(s(sc, "asof"), NOW);
    assert!(sc.get("receipt").is_none() && sc.get("trust").is_none(), "no receipt, no trust: {}", jcs(sc));
    let so = arr(sc, "overrides");
    assert_eq!(so.len(), 1);
    assert!(s(&so[0], "cell").contains("fx_rate("));
    assert_eq!(s(&so[0], "value"), "1.2");
}

#[test]
fn a_json_number_is_a_plain_literal_too_and_asof_is_sent_and_checked() {
    let w = world();
    let (rate, target) = (rate_cell(&w.up), target_cell(&w.up));
    let asof = "2026-10-31T00:00:00.000000Z";
    w.stub.answer_whatif(200, &scenario(&target, Some(asof), &[value_override(&rate, "1.2")], "1.1337", asof, None));
    let args = format!(
        r#"{{"request":{},"asof":{},"overrides":[{{"cell":{},"value":1.2}}]}}"#,
        q(TARGET_CALL),
        q(asof),
        q(RATE_CALL)
    );
    let v = whatif(&w, &args);
    assert!(!is_error(&v), "{}", jcs(&v));
    let body = one_post(&w);
    assert_eq!(s(&body, "asof"), asof);
    assert_eq!(jcs(arr(&body, "overrides")[0].get("value").unwrap()), r#"{"type":"number","value":"1.2"}"#);
    assert_eq!(s(structured(&v), "asof"), asof);
}

#[test]
fn solve_is_posted_and_the_solved_value_is_listed() {
    let w = world();
    let (rate, target) = (rate_cell(&w.up), target_cell(&w.up));
    w.stub.answer_whatif(200, &scenario(&target, None, &[], "1.5", NOW, Some((&rate, "1.4999999999"))));
    let args = format!(
        r#"{{"request":{},"solve":{{"vary":{},"target":{},"equals":"1.5","lo":1,"hi":"2"}}}}"#,
        q(TARGET_CALL),
        q(RATE_CALL),
        q(TARGET_CALL)
    );
    let v = whatif(&w, &args);
    assert!(!is_error(&v), "{}", jcs(&v));
    let body = one_post(&w);
    assert_eq!(arr(&body, "overrides").len(), 0, "overrides are [] with solve and none");
    let sv = body.get("solve").expect("solve is posted");
    assert!(s(sv, "vary").contains("fx_rate(") && s(sv, "target").contains("fx_cross_undeclared("), "{}", jcs(sv));
    assert_eq!((s(sv, "equals"), s(sv, "lo"), s(sv, "hi")), ("1.5".into(), "1".into(), "2".into()));
    let all = everything(&v);
    assert!(items(&v)[0].starts_with("Scenario (not a fact):"));
    assert!(all.contains("solved"), "{all}");
    assert!(all.contains("1.4999999999"), "{all}");
    let sc = structured(&v);
    let solved = sc.get("solved").expect("solved");
    assert!(s(solved, "vary").contains("fx_rate("));
    assert_eq!(s(solved, "value"), "1.4999999999");
    assert_eq!(s(sc, "value"), "1.5");
    assert!(!all.to_lowercase().contains("verified"));
}

#[test]
fn a_slice_override_is_posted_with_its_added_and_removed_members() {
    let w = world();
    let args = format!(
        r#"{{"request":{},"overrides":[{{"slice":{},"add":[{{"cell":{},"value":"1.5"}}],"remove":[{}]}}]}}"#,
        q(TARGET_CALL),
        q("ecb/fx_rate(base: *, quote: *, date: *)"),
        q(r#"ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#),
        q(r#"ecb/fx_rate(base: "EUR", quote: "GBP", date: 2026-10-29)"#)
    );
    // The stub has no canned answer (a 500): what the tool answers is not this test's business.
    let _ = whatif(&w, &args);
    let body = one_post(&w);
    let o = arr(&body, "overrides");
    assert_eq!(o.len(), 1);
    assert!(s(&o[0], "slice").starts_with("mesh-test.datumesh.com/ecb/fx_rate("), "{}", jcs(&o[0]));
    assert!(s(&o[0], "slice").contains("base: *"), "{}", jcs(&o[0]));
    let add = arr(&o[0], "add");
    assert_eq!(add.len(), 1);
    assert!(s(&add[0], "cell").contains(r#"quote: "USD""#));
    assert_eq!(jcs(add[0].get("value").unwrap()), r#"{"type":"number","value":"1.5"}"#);
    let rm = arr(&o[0], "remove");
    assert_eq!(rm.len(), 1);
    assert!(matches!(&rm[0], Value::String(c) if c.contains(r#"quote: "GBP""#)), "{}", jcs(&o[0]));
}

#[test]
fn a_scenario_is_never_stored_here() {
    let w = world();
    let (rate, target) = (rate_cell(&w.up), target_cell(&w.up));
    w.stub.answer_whatif(200, &scenario(&target, None, &[value_override(&rate, "1.2")], "1.1337", NOW, None));
    let args = format!(r#"{{"request":{},"overrides":[{{"cell":{},"value":"1.2"}}]}}"#, q(TARGET_CALL), q(RATE_CALL));
    assert!(!is_error(&whatif(&w, &args)));
    let args_hash = cell_id(&target_cell(&w.up));
    assert!(w.r.store().computed_asof(&args_hash, END).unwrap().is_none(), "no computed version of the target");
    // Nor of the overridden cell: nothing was fetched or written for it.
    assert!(w.r.store().computed_asof(&cell_id(&rate), END).unwrap().is_none());
}

// ---------------------------------------------------------------------------------------------
// What is not a scenario
// ---------------------------------------------------------------------------------------------

fn plain_args() -> String {
    format!(r#"{{"request":{},"overrides":[{{"cell":{},"value":"1.2"}}]}}"#, q(TARGET_CALL), q(RATE_CALL))
}

/// A request with a solve: the solver codes are valid only for one.
fn solve_args() -> String {
    format!(
        r#"{{"request":{},"solve":{{"vary":{},"target":{},"equals":"1.5","lo":1,"hi":"2"}}}}"#,
        q(TARGET_CALL),
        q(RATE_CALL),
        q(TARGET_CALL)
    )
}

#[test]
fn a_mesh_that_does_not_offer_solve_is_an_error_with_its_code() {
    let w = world();
    let bad = Answer::bad_request(ErrorCode::WhatifUnsupported).unwrap();
    w.stub.answer_whatif(bad.status(), &String::from_utf8(bad.to_json()).unwrap());
    let v = whatif(&w, &solve_args());
    assert!(is_error(&v), "{}", jcs(&v));
    assert_eq!(s(structured(&v), "error"), "E_WHATIF_UNSUPPORTED");
    assert!(!items(&v)[0].starts_with("Scenario (not a fact):"), "an error is not a scenario");
}

#[test]
fn a_solver_code_on_a_request_without_solve_is_an_envelope_error() {
    let w = world();
    let bad = Answer::bad_request(ErrorCode::WhatifUnsupported).unwrap();
    w.stub.answer_whatif(bad.status(), &String::from_utf8(bad.to_json()).unwrap());
    let v = whatif(&w, &plain_args());
    assert!(is_error(&v), "{}", jcs(&v));
    assert_eq!(s(structured(&v), "error"), "E_ENVELOPE");
    let nf = Answer::not_covered().with_fault(Fault::WhatifNoSolution);
    w.stub.answer_whatif(nf.status(), &String::from_utf8(nf.to_json()).unwrap());
    let v = whatif(&w, &plain_args());
    assert!(is_error(&v), "{}", jcs(&v));
    assert_eq!(s(structured(&v), "error"), "E_ENVELOPE");
}

#[test]
fn no_solution_is_not_covered_with_its_fault() {
    let w = world();
    let nf = Answer::not_covered().with_fault(Fault::WhatifNoSolution);
    w.stub.answer_whatif(nf.status(), &String::from_utf8(nf.to_json()).unwrap());
    let v = whatif(&w, &solve_args());
    assert!(is_error(&v), "{}", jcs(&v));
    assert_eq!(s(structured(&v), "error"), "not_covered");
    assert_eq!(s(structured(&v), "fault"), "E_WHATIF_NO_SOLUTION");
    assert!(everything(&v).contains("E_WHATIF_NO_SOLUTION"));
}

#[test]
fn a_scenario_that_is_not_the_one_asked_for_is_refused_as_an_envelope_error() {
    let w = world();
    let (rate, target) = (rate_cell(&w.up), target_cell(&w.up));
    // The mesh answers for another override value than the one sent.
    w.stub.answer_whatif(200, &scenario(&target, None, &[value_override(&rate, "9.99")], "7777.5", NOW, None));
    let v = whatif(&w, &plain_args());
    assert!(is_error(&v), "{}", jcs(&v));
    assert_eq!(s(structured(&v), "error"), "E_ENVELOPE");
    let all = everything(&v);
    assert!(!all.contains("7777.5"), "the refused value is never presented: {all}");
    assert!(!items(&v)[0].starts_with("Scenario (not a fact):"));
}

#[test]
fn a_mesh_without_a_whatif_endpoint_is_an_error_naming_it() {
    let w = world();
    w.stub.no_whatif_endpoint.store(true, std::sync::atomic::Ordering::SeqCst);
    let v = whatif(&w, &plain_args());
    assert!(is_error(&v), "{}", jcs(&v));
    assert!(everything(&v).contains("whatif"), "{}", jcs(&v));
    assert!(w.stub.posts().is_empty(), "nothing was posted anywhere");
}

#[test]
fn a_value_that_does_not_fit_the_cells_type_is_refused_before_anything_is_sent() {
    let w = world();
    let args =
        format!(r#"{{"request":{},"overrides":[{{"cell":{},"value":"not a number"}}]}}"#, q(TARGET_CALL), q(RATE_CALL));
    let v = whatif(&w, &args);
    assert!(is_error(&v), "{}", jcs(&v));
    assert_eq!(s(structured(&v), "error"), "E_CANON_TYPE");
    assert!(w.stub.posts().is_empty());
}

#[test]
fn overrides_are_required_without_solve() {
    let w = world();
    let v = whatif(&w, &format!(r#"{{"request":{}}}"#, q(TARGET_CALL)));
    assert!(is_error(&v) || v.get("error").is_some(), "{}", jcs(&v));
    assert!(w.stub.posts().is_empty());
}

// ---------------------------------------------------------------------------------------------
// The local namespace: evaluated here by the shared engine
// ---------------------------------------------------------------------------------------------

const T: &str = "2026-11-15T12:00:00.000000Z";
const CELLS_TX: &str = "2026-11-15T11:00:00.000000Z";
const LEVEL_SIG: &str = r#"{"args": {"day": {"type": "date", "valid": true}}, "order": ["day"], "returns": {"type": "number", "unit": "1"}}"#;

fn local_world() -> (Upstream, TempDir, Arc<Resolver>, McpServer) {
    let up = upstream();
    let dir = TempDir::new("whatif-local");
    let tmp = TempDir::new("whatif-local-tmp");
    let clock = ManualClock::new("2026-11-15T10:00:00.000000Z");
    let r = Arc::new(resolver_at(&dir, &up, &clock));
    let sig = |t: &str| json::parse(t.as_bytes()).unwrap();
    r.define_stored("local/level", &sig(LEVEL_SIG)).unwrap();
    let line = |day: &str, v: &str| {
        format!(
            r#"{{"cell": {{"fn": "local/level", "args": {{"day": {{"type": "date", "value": "{day}"}}}}}}, "value": {{"type": "number", "value": "{v}"}}, "tx": "{CELLS_TX}", "asof": "{CELLS_TX}", "licence": "CC0-1.0", "summary": "A test cell."}}"#
        )
    };
    let f = jsonl(&tmp, "level.jsonl", &[&line("2026-11-01", "10"), &line("2026-11-02", "20")]);
    r.publish(&f).unwrap();
    clock.set(T);
    r.define_formula("local/double", "local.invalid/level(day: day) * 2", &sig(LEVEL_SIG)).unwrap();
    let d = r.get("local/double(day: 2026-11-01)").unwrap();
    assert_eq!(s(&receipt(&d.body), "value"), "20");
    let srv = McpServer::new(r.clone());
    (up, dir, r, srv)
}

#[test]
fn a_local_scenario_is_evaluated_here_and_changes_nothing() {
    let (_up, _dir, r, srv) = local_world();
    let before = r.get("local/double(day: 2026-11-01)").unwrap().body;
    let v = tool(
        &srv,
        "whatif",
        r#"{"request":"local/double(day: 2026-11-01)","overrides":[{"cell":"local/level(day: 2026-11-01)","value":100}]}"#,
    );
    assert!(!is_error(&v), "{}", jcs(&v));
    assert!(items(&v)[0].starts_with("Scenario (not a fact):"), "{:?}", items(&v)[0]);
    let sc = structured(&v);
    assert_eq!(s(sc, "outcome"), "scenario");
    assert_eq!(s(sc, "value"), "200", "{}", jcs(sc));
    assert!(sc.get("receipt").is_none() && sc.get("trust").is_none());
    assert!(!everything(&v).to_lowercase().contains("verified"));
    // The normal answer is unchanged, from the store (the same bytes), and no new version exists.
    let after = r.get("local/double(day: 2026-11-01)").unwrap();
    assert_eq!(after.body, before);
    assert_eq!(s(&receipt(&after.body), "value"), "20");
    // A cell that does not depend on the override reads as it always did.
    let other = r.get("local/double(day: 2026-11-02)").unwrap();
    assert_eq!(s(&receipt(&other.body), "value"), "40");
}

#[test]
fn a_local_solve_finds_the_value_of_the_varied_cell() {
    let (_up, _dir, _r, srv) = local_world();
    let v = tool(
        &srv,
        "whatif",
        r#"{"request":"local/double(day: 2026-11-01)","solve":{"vary":"local/level(day: 2026-11-01)","target":"local/double(day: 2026-11-01)","equals":50,"lo":0,"hi":100}}"#,
    );
    assert!(!is_error(&v), "{}", jcs(&v));
    let sc = structured(&v);
    let near = |t: &str, want: f64| {
        let x: f64 = t.parse().unwrap_or_else(|_| panic!("a number: {t}"));
        assert!((x - want).abs() < 1e-6, "{t} is not near {want}");
    };
    near(&s(sc, "value"), 50.0);
    near(&s(sc.get("solved").expect("solved"), "value"), 25.0);
    assert!(everything(&v).contains("solved"));
}

#[test]
fn a_local_solve_without_a_sign_change_has_no_solution() {
    let (_up, _dir, _r, srv) = local_world();
    let v = tool(
        &srv,
        "whatif",
        r#"{"request":"local/double(day: 2026-11-01)","solve":{"vary":"local/level(day: 2026-11-01)","target":"local/double(day: 2026-11-01)","equals":1000,"lo":0,"hi":100}}"#,
    );
    assert!(is_error(&v), "{}", jcs(&v));
    assert_eq!(s(structured(&v), "fault"), "E_WHATIF_NO_SOLUTION", "{}", jcs(&v));
}
