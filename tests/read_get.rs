//! CONTRACT-build2-part2-read R4 (datumesh-local 0.4.0), `get`: a plain reference is read in one
//! tool call through the answering mesh's `/find` resolution (http 0.4.0's answer shape, R3).
//!
//! Shapes these tests specify (not yet implemented):
//! * the answering mesh is the first of `Config::find_meshes`; a `get` argument (still named
//!   `request`) that is not a call or a `cell://` URI is sent to its `/find?q=<the text>`;
//! * `resolution.status == "exact"`: the tool result's FIRST content item is text that begins
//!   with the resolution's `label`, and contains its canonical `call`; the next items are the
//!   value (a text blob's checked text first, as 0.3.3) and the receipt block as today. The
//!   structured content is the 0.3.3 answer block (`status`, `call`, `text`, ...) plus
//!   `resolved: {label, call}`;
//! * `ambiguous`: not an error; structured `{resolution: "ambiguous", candidates: [{label,
//!   call?, missing_keys?}]}`; the text lists each candidate's label (and call); no value, and
//!   no cell is read;
//! * `range`: an error result, structured `error: "E_RANGE_REFUSED"`, whose detail says ranges are
//!   read verse by verse or as a slice;
//! * `none`: an error result, structured `error: "not_covered"`, and the `find` results' names
//!   (the suggestions) appear in the text and in structured `suggestions`;
//! * `name?key=value` is refused as a call, `E_PARSE_URI`, and never sent to `/find`.

mod common;
#[path = "support/read_stub.rs"]
mod read_stub;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Transport};
use datumesh_local::{Config, Resolver};
use read_stub::{pct_decode, ReadStub};

const M: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

/// Synthetic text with a mark, a leading space and a trailing newline.
const TEXT: &str = " alpha beta\u{5B4} gamma\n";
const LABEL: &str = "Alpha 1:1 (plain)";
const REF: &str = "alpha 1:1 plain text";

fn q(s: &str) -> String {
    String::from_utf8(Value::String(s.to_string()).to_jcs()).unwrap()
}

fn jcs(v: &Value) -> String {
    String::from_utf8(v.to_jcs()).unwrap()
}

fn tool(srv: &McpServer, name: &str, args: &str) -> Value {
    let line = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{M},"name":{},"arguments":{args}}}}}"#,
        q(name)
    );
    json::parse(srv.handle(&line, Transport::Stdio).expect("a response").as_bytes()).unwrap()
}

fn get(srv: &McpServer, request: &str) -> Value {
    tool(srv, "get", &format!(r#"{{"request":{}}}"#, q(request)))
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

struct World {
    up: Upstream,
    stub: Arc<ReadStub>,
    srv: McpServer,
    call: String,
    _dir: TempDir,
}

fn find_body(resolution: &str, results: &str) -> String {
    format!(r#"{{"query":"x","vector":false,"results":[{results}],"resolution":{resolution}}}"#)
}

/// A mesh with the text cell `tag` and the stub's `/find` answering `REF` with `resolution`.
fn world(tag: &str, resolution: impl Fn(&str) -> String, results: &str) -> World {
    let up = upstream();
    up.publish_blob_cell(tag, TEXT.as_bytes(), "text/plain", None);
    let call = text_cell_call(tag);
    let stub = ReadStub::start(&up);
    stub.set_summary(None);
    stub.answer(REF, &find_body(&resolution(&call), results));
    let dir = TempDir::new("read-get");
    let mut c: Config = config(&dir, &up);
    c.peers.insert(HOST.to_string(), stub.origin.clone());
    c.find_meshes = vec![HOST.to_string()];
    let srv = McpServer::new(Arc::new(Resolver::open(c).unwrap()));
    World { up, stub, srv, call, _dir: dir }
}

#[test]
fn a_plain_reference_is_read_in_one_call_and_the_answer_starts_with_the_label_and_call() {
    let w = world("r4a", |call| format!(r#"{{"status":"exact","call":{},"label":{}}}"#, q(call), q(LABEL)), "");
    let v = get(&w.srv, REF);
    assert!(!is_error(&v), "{}", jcs(&v));
    let c = items(&v);
    assert!(c[0].starts_with(LABEL), "the first item begins with the label: {:?}", c[0]);
    assert!(c[0].contains(&w.call), "and has the canonical call: {:?}", c[0]);
    assert!(!c[0].contains(TEXT.trim()), "the value comes after: {:?}", c[0]);
    let at = c.iter().position(|i| i == TEXT).expect("the checked text is an item");
    assert!(at >= 1);
    let st = structured(&v);
    assert_eq!(st.get("status").and_then(Value::as_number_text), Some("200"));
    assert_eq!(s(st, "call"), w.call);
    assert_eq!(s(st, "text"), TEXT);
    let resolved = st.get("resolved").expect("resolved");
    assert_eq!((s(resolved, "label"), s(resolved, "call")), (LABEL.to_string(), w.call.clone()));
    assert!(st.get("envelope").and_then(|e| e.get("receipt")).is_some(), "the receipt as today");
    // One request to /find, with the reference as typed.
    let asked = w.stub.plain_queries();
    assert_eq!(asked.len(), 1, "{asked:?}");
    let qv = asked[0].split('&').find_map(|p| p.strip_prefix("q=")).map(pct_decode).unwrap();
    assert_eq!(qv, REF);
    let _ = &w.up;
}

#[test]
fn a_real_call_is_not_sent_to_find() {
    let w = world("r4b", |c| format!(r#"{{"status":"exact","call":{},"label":"x"}}"#, q(c)), "");
    let v = get(&w.srv, &w.call);
    assert!(!is_error(&v), "{}", jcs(&v));
    assert!(w.stub.plain_queries().is_empty(), "{:?}", w.stub.plain_queries());
    assert!(!items(&v)[0].starts_with("x"), "no resolution header for a call");
}

#[test]
fn an_ambiguous_reference_answers_with_the_candidates_and_no_value() {
    let other = r#"mesh-test.datumesh.com/scripture/verse_text(book: "other", chapter: 1, corpus: "textcases", edition: "plain", verse: 1)"#;
    let w = world(
        "r4c",
        |call| {
            format!(
                r#"{{"status":"ambiguous","candidates":[{{"call":{},"label":"Alpha 1:1 (plain)"}},{{"call":{},"label":"Alpha 1:1 (other)"}},{{"label":"Alpha 1:1 (third)","missing_keys":["edition"]}}]}}"#,
                q(call),
                q(other)
            )
        },
        "",
    );
    let v = get(&w.srv, REF);
    assert!(!is_error(&v), "{}", jcs(&v));
    let all = everything(&v);
    for needle in ["Alpha 1:1 (plain)", "Alpha 1:1 (other)", "Alpha 1:1 (third)", &w.call, other] {
        assert!(all.contains(needle), "{needle} in {all}");
    }
    assert!(!all.contains("beta"), "no value: {all}");
    let st = structured(&v);
    assert_eq!(s(st, "resolution"), "ambiguous");
    let Some(Value::Array(cands)) = st.get("candidates") else { panic!("candidates in {}", jcs(st)) };
    assert_eq!(cands.len(), 3);
    assert_eq!(s(&cands[0], "label"), "Alpha 1:1 (plain)");
    assert_eq!(s(&cands[0], "call"), w.call);
    assert!(st.get("text").is_none() && st.get("value").is_none() && st.get("envelope").is_none());
    assert_eq!(w.up.blob_requests(), 0, "no cell was read");
}

#[test]
fn a_range_is_refused_with_e_range_refused() {
    let w = world("r4d", |_| r#"{"status":"range","error":"E_RANGE_REFUSED"}"#.to_string(), "");
    let v = get(&w.srv, REF);
    assert!(is_error(&v), "{}", jcs(&v));
    let st = structured(&v);
    assert_eq!(s(st, "error"), "E_RANGE_REFUSED", "{}", jcs(st));
    let detail = s(st, "detail");
    assert!(detail.contains("verse by verse") && detail.contains("slice"), "{detail}");
    assert!(items(&v)[0].starts_with("E_RANGE_REFUSED"), "{:?}", items(&v));
    assert_eq!(w.up.blob_requests(), 0);
}

#[test]
fn nothing_found_is_not_covered_with_the_suggestions_of_find() {
    let w = world(
        "r4e",
        |_| r#"{"status":"none"}"#.to_string(),
        r#"{"kind":"class","name":"mesh-test.datumesh.com/sample/passage","label":"Sample passages","description":"d","call":"mesh-test.datumesh.com/sample/passage(book: <string>, chapter: <number>)","match":"text","score":0.5}"#,
    );
    let v = get(&w.srv, REF);
    assert!(is_error(&v), "{}", jcs(&v));
    let st = structured(&v);
    assert_eq!(s(st, "error"), "not_covered", "{}", jcs(st));
    assert!(everything(&v).contains("mesh-test.datumesh.com/sample/passage"), "{}", everything(&v));
    assert!(matches!(st.get("suggestions"), Some(Value::Array(a)) if !a.is_empty()), "{}", jcs(st));
}

#[test]
fn the_query_string_form_is_refused_and_not_resolved() {
    let w = world("r4f", |c| format!(r#"{{"status":"exact","call":{},"label":"x"}}"#, q(c)), "");
    // Even when /find would happily resolve this text.
    for form in [
        "ecb/fx_rate?base=EUR&quote=USD&date=2026-10-30",
        "mesh-test.datumesh.com/scripture/verse_text?book=r4f&chapter=1&corpus=textcases&edition=plain&verse=1",
    ] {
        w.stub.answer(form, &find_body(&format!(r#"{{"status":"exact","call":{},"label":"x"}}"#, q(&w.call)), ""));
        let v = get(&w.srv, form);
        let all = everything(&v);
        assert!(all.contains("E_PARSE_URI"), "{form}: {all}");
        assert!(is_error(&v) || all.contains("\"status\":400"), "{form}: {all}");
        assert!(!all.contains("beta"), "no value: {all}");
    }
    assert!(w.stub.plain_queries().is_empty(), "never sent to find: {:?}", w.stub.plain_queries());
}
