//! CONTRACT-build2-part2-read R4 (datumesh-local 0.4.0), `verify` with `text`.
//!
//! Specified shape: `verify` takes `text` (a string) with `call` (a call or `cell://` URI) or
//! `receipt` (JSON). The blob of the cell's text value is fetched and its SHA-256 checked as
//! `get` does. The structured result gains `text_matches: bool`: the quoted text equals the
//! blob's text, both after NFC. When false it also has `first_difference`: the 0-based position
//! in code points, after NFC, of the first code point where the two differ (when one is a prefix
//! of the other, the length of the shorter). When true, `first_difference` is absent. The
//! receipt checks (`ok`, `level`, ...) stay as before. A blob that fails its hash is
//! `E_BLOB_MISMATCH` and no `text_matches`.

mod common;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Transport};

const M: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

fn q(s: &str) -> String {
    String::from_utf8(Value::String(s.to_string()).to_jcs()).unwrap()
}

fn jcs(v: &Value) -> String {
    String::from_utf8(v.to_jcs()).unwrap()
}

fn verify(srv: &McpServer, args: &str) -> Value {
    let line = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{M},"name":"verify","arguments":{args}}}}}"#
    );
    json::parse(srv.handle(&line, Transport::Stdio).expect("a response").as_bytes()).unwrap()
}

fn structured(v: &Value) -> Value {
    v.get("result")
        .and_then(|r| r.get("structuredContent"))
        .cloned()
        .unwrap_or_else(|| panic!("structuredContent in {}", jcs(v)))
}

/// A server over an upstream whose cell `tag` holds `blob` (as published, not normalised).
fn with_blob(tag: &str, blob: &str) -> (McpServer, Upstream, String, TempDir) {
    let up = upstream();
    up.publish_blob_cell(tag, blob.as_bytes(), "text/plain", None);
    let dir = TempDir::new("read-verify");
    let srv = McpServer::new(Arc::new(resolver(&dir, &up)));
    (srv, up, text_cell_call(tag), dir)
}

fn check(srv: &McpServer, call: &str, text: &str) -> Value {
    structured(&verify(srv, &format!(r#"{{"call":{},"text":{}}}"#, q(call), q(text))))
}

fn first(v: &Value) -> Option<String> {
    v.get("first_difference").and_then(Value::as_number_text).map(str::to_string)
}

// Arabic with shadda (U+0651, class 33) and fatha (U+064E, class 30): NFC puts the fatha first.
const NON_CANONICAL: &str = "\u{644}\u{651}\u{64E}\u{647}";
const CANONICAL: &str = "\u{644}\u{64E}\u{651}\u{647}";

#[test]
fn text_equal_is_a_match() {
    let (srv, _up, call, _d) = with_blob("vt1", "plain text\n");
    let v = check(&srv, &call, "plain text\n");
    assert_eq!(v.get("text_matches"), Some(&Value::Bool(true)), "{}", jcs(&v));
    assert!(first(&v).is_none());
}

#[test]
fn text_equal_after_nfc_is_a_match_on_both_sides() {
    // Quoted decomposed, blob precomposed.
    let (srv, _up, call, _d) = with_blob("vt2", "caf\u{E9}");
    assert_eq!(check(&srv, &call, "cafe\u{301}").get("text_matches"), Some(&Value::Bool(true)));
    // Blob decomposed, quoted precomposed.
    let (srv, _up, call, _d) = with_blob("vt3", "cafe\u{301}");
    assert_eq!(check(&srv, &call, "caf\u{E9}").get("text_matches"), Some(&Value::Bool(true)));
    // Marks in a non-canonical order in the blob, canonical in the quote.
    let (srv, _up, call, _d) = with_blob("vt4", NON_CANONICAL);
    let v = check(&srv, &call, CANONICAL);
    assert_eq!(v.get("text_matches"), Some(&Value::Bool(true)), "{}", jcs(&v));
}

#[test]
fn one_mark_different_is_false_with_the_position() {
    let (srv, _up, call, _d) = with_blob("vt5", "ab\u{5B4}cd");
    let v = check(&srv, &call, "ab\u{5B5}cd");
    assert_eq!(v.get("text_matches"), Some(&Value::Bool(false)), "{}", jcs(&v));
    assert_eq!(first(&v).as_deref(), Some("2"), "{}", jcs(&v));
}

#[test]
fn the_position_counts_code_points_after_nfc() {
    // After NFC: "caf\u{E9}x" against "caf\u{E9}y": the difference is at 4, not 5 (the
    // decomposed spelling's extra code point is not counted).
    let (srv, _up, call, _d) = with_blob("vt6", "cafe\u{301}x");
    let v = check(&srv, &call, "cafe\u{301}y");
    assert_eq!(v.get("text_matches"), Some(&Value::Bool(false)));
    assert_eq!(first(&v).as_deref(), Some("4"), "{}", jcs(&v));
}

#[test]
fn a_prefix_differs_at_the_length_of_the_shorter() {
    let (srv, _up, call, _d) = with_blob("vt7", "abc");
    assert_eq!(first(&check(&srv, &call, "ab")).as_deref(), Some("2"));
    assert_eq!(first(&check(&srv, &call, "abcd")).as_deref(), Some("3"));
}

#[test]
fn whitespace_is_not_normalised() {
    let (srv, _up, call, _d) = with_blob("vt8", "abc\n");
    let v = check(&srv, &call, "abc");
    assert_eq!(v.get("text_matches"), Some(&Value::Bool(false)));
    assert_eq!(first(&v).as_deref(), Some("3"));
}

#[test]
fn text_with_a_receipt_is_checked_the_same_way() {
    let (srv, _up, call, _d) = with_blob("vt9", "abc");
    let got = structured(&{
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{M},"name":"get","arguments":{{"request":{}}}}}}}"#,
            q(&call)
        );
        json::parse(srv.handle(&line, Transport::Stdio).unwrap().as_bytes()).unwrap()
    });
    let receipt = jcs(got.get("envelope").and_then(|e| e.get("receipt")).expect("a receipt"));
    let v = structured(&verify(&srv, &format!(r#"{{"receipt":{},"text":"abd"}}"#, q(&receipt))));
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", jcs(&v));
    assert_eq!(v.get("text_matches"), Some(&Value::Bool(false)));
    assert_eq!(first(&v).as_deref(), Some("2"));
}

#[test]
fn a_blob_that_fails_its_hash_is_e_blob_mismatch() {
    let (srv, up, call, _d) = with_blob("vt10", "abc");
    up.serve_wrong_blobs(true);
    let v = verify(&srv, &format!(r#"{{"call":{},"text":"abc"}}"#, q(&call)));
    let st = structured(&v);
    assert_eq!(s(&st, "error"), "E_BLOB_MISMATCH", "{}", jcs(&st));
    assert!(st.get("text_matches").is_none());
}

#[test]
fn without_text_verify_is_as_before() {
    let (srv, _up, call, _d) = with_blob("vt11", "abc");
    let line = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{M},"name":"get","arguments":{{"request":{}}}}}}}"#,
        q(&call)
    );
    let got = structured(&json::parse(srv.handle(&line, Transport::Stdio).unwrap().as_bytes()).unwrap());
    let receipt = jcs(got.get("envelope").and_then(|e| e.get("receipt")).unwrap());
    let v = structured(&verify(&srv, &format!(r#"{{"receipt":{}}}"#, q(&receipt))));
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)));
    assert!(v.get("text_matches").is_none());
}
