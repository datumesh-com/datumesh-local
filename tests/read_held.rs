//! CONTRACT-classes-round C8 (local): the MCP instructions sentence "This mesh (<host>) holds: ..." uses the `held` member of
//! the mesh's `find?summary=1` unchanged when it is there (http: a string generated from the catalogue, at most 60 words, as
//! `{"summary":true,"mesh":..,"classes":[..],"functions":[..],"held":"finance: fundamentals sec/company; ..."}`), and
//! today's text (built from the classes and functions) when it is absent (an older mesh, or a mesh with no facets).
//!
//! The summary is canned (`tests/support/read_stub.rs`); the names are invented.

mod common;
#[path = "support/read_stub.rs"]
mod read_stub;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Transport};
use datumesh_local::{Config, Resolver};
use read_stub::ReadStub;

const M: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

/// The summary without `held` (what an older mesh answers), closed by `tail` (e.g. `,"held":"..."`) before its last brace.
fn summary(tail: &str) -> String {
    format!(
        r#"{{"summary":true,"mesh":"mesh-test.datumesh.com","classes":[{{"name":"mesh-test.datumesh.com/sample/passage","label":"Sample passages","description":"A passage of the sample corpus","cells":31102,"keys":{{"book":{{"distinct":2,"values":[{{"value":"Bk1","label":"Book One"}},{{"value":"Bk2","label":"Book Two"}}]}}}}}}],"functions":[{{"name":"mesh-test.datumesh.com/sample/rate","label":"Sample exchange rates","description":"An invented daily rate","cells":4321,"keys":{{}}}}]{tail}}}"#
    )
}

fn with_held(held: &str) -> String {
    let h = String::from_utf8(Value::String(held.to_string()).to_jcs()).unwrap();
    summary(&format!(r#","held":{h}"#))
}

/// A held text in the shape the mesh generates: domains, measures, rank-1 sources, under 60 words.
const HELD: &str = "finance: fundamentals sample/company; interest rate sample/policy_rate; exchange rate sample/fx_reference; scripture: text sample/passage; tools: ratio sample/ratio;";

fn jcs(v: &Value) -> String {
    String::from_utf8(v.to_jcs()).unwrap()
}

fn instructions(srv: &McpServer) -> String {
    let line = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{{{M}}}}}"#);
    let v = json::parse(srv.handle(&line, Transport::Stdio).expect("a response").as_bytes()).unwrap();
    s(v.get("result").unwrap_or_else(|| panic!("{}", jcs(&v))), "instructions")
}

fn server(summary: &str) -> (McpServer, Upstream, TempDir) {
    let up = upstream();
    let stub = ReadStub::start(&up);
    stub.set_summary(Some(summary));
    let dir = TempDir::new("read-held");
    let mut c: Config = config(&dir, &up);
    c.peers.insert(HOST.to_string(), stub.origin.clone());
    c.find_meshes = vec![HOST.to_string()];
    // keep the stub alive as long as the server: leak it (a test process is short)
    std::mem::forget(stub);
    (McpServer::new(Arc::new(Resolver::open(c).unwrap())), up, dir)
}

/// The sentence of the instructions that names the mesh: from "This mesh (" to the sentence "Use find".
fn sentence(ins: &str) -> &str {
    let from = ins.find("This mesh (").unwrap_or_else(|| panic!("no 'This mesh (' in {ins}"));
    let to = ins[from..].find(" Use find").map(|i| from + i).unwrap_or(ins.len());
    &ins[from..to]
}

#[test]
fn the_instructions_use_held_verbatim_when_the_summary_has_it() {
    let (srv, _up, _dir) = server(&with_held(HELD));
    let ins = instructions(&srv);
    let want = format!("This mesh (mesh-test.datumesh.com) holds: {HELD}");
    assert!(ins.contains(&want), "held is the sentence's text: {ins}");
    // today's member text is not used beside it
    for today in ["Sample passages", "Sample exchange rates", "31,102", "Book One"] {
        assert!(!sentence(&ins).contains(today), "{today} is not in the sentence when held is: {ins}");
    }
    // the rest of the instructions is as it was
    assert!(ins.starts_with("Datumesh: every answer is a signed receipt."), "{ins}");
    assert!(ins.contains("Use find to discover names"), "{ins}");
}

#[test]
fn the_sentence_still_names_the_host() {
    let (srv, _up, _dir) = server(&with_held(HELD));
    let ins = instructions(&srv);
    assert!(sentence(&ins).starts_with("This mesh (mesh-test.datumesh.com) holds: "), "{ins}");
    // and a different mesh name in the summary is the one named
    let other = with_held(HELD).replace(r#""mesh":"mesh-test.datumesh.com""#, r#""mesh":"other.example""#);
    let (srv, _up, _dir) = server(&other);
    assert!(instructions(&srv).contains("This mesh (other.example) holds: finance:"), "{}", instructions(&srv));
}

#[test]
fn without_held_the_instructions_are_todays_text() {
    let (srv, _up, _dir) = server(&summary(""));
    let ins = instructions(&srv);
    assert!(
        ins.contains("This mesh (mesh-test.datumesh.com) holds: Sample passages (31,102 cells; book: Book One, Book Two); Sample exchange rates (4,321 cells)."),
        "{ins}"
    );
}

#[test]
fn the_text_with_held_differs_from_the_text_without_only_in_the_sentence() {
    let (without, _u0, _d0) = server(&summary(""));
    let (with, _u1, _d1) = server(&with_held(HELD));
    let (a, b) = (instructions(&without), instructions(&with));
    let (sa, sb) = (sentence(&a), sentence(&b));
    assert_ne!(sa, sb);
    assert_eq!(a.replacen(sa, "", 1), b.replacen(sb, "", 1), "everything but the sentence is the same");
}

/// Text with no characters that need escaping (letters, digits, slashes, underscores, punctuation, words of many lengths and a
/// sixty-word text) is not truncated, re-worded or re-ordered.
#[test]
fn a_plain_held_is_not_truncated_or_reworded() {
    let words: Vec<String> = (1..=60).map(|i| format!("w{i}/n_{i}")).collect();
    let long = format!("{};", words.join(" "));
    assert_eq!(long.split_whitespace().count(), 60);
    for held in [long.as_str(), "a b c", "one-word", HELD] {
        let (srv, _up, _dir) = server(&with_held(held));
        let ins = instructions(&srv);
        assert!(ins.contains(&format!("holds: {held}")), "{held:?} is verbatim in {ins}");
    }
}

#[test]
fn held_with_non_ascii_text_and_quotes_is_read_back_unchanged() {
    let held = "scripture: texte \"cité\" sample/passage; é à ü 日本 sample/jp;";
    let (srv, _up, _dir) = server(&with_held(held));
    let ins = instructions(&srv);
    assert!(ins.contains(&format!("holds: {held}")), "{ins}");
}
