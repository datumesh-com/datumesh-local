//! CONTRACT-build2-part2-read R4 (datumesh-local 0.4.0): tool descriptions and server
//! instructions are written at start from the answering mesh's catalogue summary
//! (`GET /find?summary=1`), and use no real popular cell as an example.
//!
//! Shape of the summary (http 0.4.0, specified here; R1's `catalog_summary`):
//! `{"summary":true,"mesh":<host>,"classes":[{"name","label","description","cells":int,
//!   "keys":{<key>:{"distinct":int,"values":[{"value","label"}]}}}],"functions":[same members]}`.
//! There is no separate editions list: editions are the values of a key (here `edition`).
//!
//! Specified behaviour: `server/discover` and `initialize` give `instructions`, and `tools/list`
//! gives each tool's `description`, built from the summary: the instructions name each class and
//! function label and its cell count, and the labels of every listed key value (the editions among them, when `distinct` <= 50).
//! None of them carries a famous reference or any `<digits>:<digits>` reference shape.
//!
//! Amended by datumesh-local 0.5.2 (the user's items 36 and 37, CONTRACT-next-patch.md): what the
//! mesh holds is carried once, in the instructions only, no longer in the `find` and `get`
//! descriptions as well (36); and a summary that is fetched and fails is no longer replaced silently
//! by the 0.3.3 static text: the instructions say "What this mesh holds could not be fetched
//! (<reason>); find still works." (37). With no find mesh configured, that is the reason given.

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

/// Invented names only.
const SUMMARY: &str = r#"{"summary":true,"mesh":"mesh-test.datumesh.com","classes":[{"name":"mesh-test.datumesh.com/sample/passage","label":"Sample passages","description":"A passage of the sample corpus","cells":31102,"keys":{"book":{"distinct":2,"values":[{"value":"Bk1","label":"Book One"},{"value":"Bk2","label":"Book Two"}]},"edition":{"distinct":1,"values":[{"value":"plain","label":"Plain text"}]}}}],"functions":[{"name":"mesh-test.datumesh.com/sample/rate","label":"Sample exchange rates","description":"An invented daily rate","cells":4321,"keys":{"quote":{"distinct":2,"values":[{"value":"AAA","label":"Alpha Unit"},{"value":"BBB","label":"Beta Unit"}]}}}]}"#;

const FAMOUS: [&str; 14] = [
    "John 3:16",
    "Genesis 1:1",
    "Gen 1:1",
    "Quran 2:255",
    "Qur'an 2:255",
    "Ayat al-Kursi",
    "Psalm 23",
    "Psalms 23",
    "Matthew 5",
    "Romans 8:28",
    "Exodus 20",
    "Al-Fatiha",
    "Bismillah",
    "Lord's Prayer",
];

fn jcs(v: &Value) -> String {
    String::from_utf8(v.to_jcs()).unwrap()
}

fn rpc(srv: &McpServer, method: &str) -> Value {
    let line = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{{{M}}}}}"#);
    json::parse(srv.handle(&line, Transport::Stdio).expect("a response").as_bytes()).unwrap()
}

fn instructions(srv: &McpServer) -> String {
    let v = rpc(srv, "server/discover");
    s(v.get("result").unwrap_or_else(|| panic!("{}", jcs(&v))), "instructions")
}

fn descriptions(srv: &McpServer) -> Vec<(String, String)> {
    let v = rpc(srv, "tools/list");
    match v.get("result").and_then(|r| r.get("tools")) {
        Some(Value::Array(a)) => a.iter().map(|t| (s(t, "name"), s(t, "description"))).collect(),
        other => panic!("tools: {other:?}"),
    }
}

fn server(summary: Option<&str>, meshes: &[&str]) -> (McpServer, Arc<ReadStub>, Upstream, TempDir) {
    let up = upstream();
    let stub = ReadStub::start(&up);
    stub.set_summary(summary);
    let dir = TempDir::new("read-describe");
    let mut c: Config = config(&dir, &up);
    c.peers.insert(HOST.to_string(), stub.origin.clone());
    c.find_meshes = meshes.iter().map(|m| m.to_string()).collect();
    (McpServer::new(Arc::new(Resolver::open(c).unwrap())), stub, up, dir)
}

/// True when `text` has a digit, a colon and a digit in a row (a scripture reference shape).
fn has_reference_shape(text: &str) -> bool {
    let c: Vec<char> = text.chars().collect();
    c.windows(3).any(|w| w[0].is_ascii_digit() && w[1] == ':' && w[2].is_ascii_digit())
}

fn assert_clean(what: &str, text: &str) {
    let lower = text.to_lowercase();
    for f in FAMOUS {
        assert!(!lower.contains(&f.to_lowercase()), "{what} has {f:?}: {text}");
    }
    assert!(!has_reference_shape(text), "{what} has a reference shape: {text}");
}

#[test]
fn instructions_and_descriptions_are_built_from_the_summary() {
    let (srv, stub, _up, _dir) = server(Some(SUMMARY), &[HOST]);
    assert!(stub.summary_requests() >= 1, "the summary is fetched at start: {:?}", stub.queries.lock().unwrap());
    assert!(stub.queries.lock().unwrap().iter().any(|q| q.split('&').any(|p| p == "summary=1")));
    let ins = instructions(&srv);
    for needle in ["Sample passages", "Sample exchange rates", "Plain text", "Book One", "Alpha Unit", "Beta Unit"] {
        assert!(ins.contains(needle), "{needle} in {ins}");
    }
    for count in ["31102", "4321"] {
        assert!(
            ins.contains(count)
                || ins.contains(&format!("{},{}", &count[..count.len() - 3], &count[count.len() - 3..])),
            "count {count} in {ins}"
        );
    }
    // The same text over the stdio handshake.
    let init = json::parse(
        srv.handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
            Transport::Stdio,
        )
        .unwrap()
        .as_bytes(),
    )
    .unwrap();
    assert_eq!(s(init.get("result").unwrap(), "instructions"), ins);
    // Item 36 (0.5.2): what the mesh holds is in the instructions only, not in find and get too.
    let d = descriptions(&srv);
    let find = d.iter().find(|(n, _)| n == "find").unwrap();
    let get = d.iter().find(|(n, _)| n == "get").unwrap();
    assert!(!find.1.contains("Sample passages"), "{}", find.1);
    assert!(!get.1.contains("Sample passages"), "{}", get.1);
    let (plain, _s0, _u0, _d0) = server(None, &[]);
    assert_eq!(d, descriptions(&plain), "the descriptions are the static ones");
    assert_eq!(d.len(), 12, "the same tools, calc (0.4.2) and interval (0.5.1)");
}

#[test]
fn no_real_popular_cell_or_reference_shape_appears_in_what_the_summary_builds() {
    let (srv, _stub, _up, _dir) = server(Some(SUMMARY), &[HOST]);
    assert_clean("instructions", &instructions(&srv));
    for (name, text) in descriptions(&srv) {
        assert_clean(&format!("description of {name}"), &text);
    }
}

#[test]
fn a_summary_with_a_famous_label_still_gives_no_example_of_it() {
    // A mesh whose dictionary holds real book names: they may be named as what the mesh holds
    // (labels), but never offered as an example cell (no reference shape, no famous reference).
    let summary = SUMMARY.replace("Book One", "Genesis").replace("Book Two", "John");
    let (srv, _stub, _up, _dir) = server(Some(&summary), &[HOST]);
    assert_clean("instructions", &instructions(&srv));
    for (name, text) in descriptions(&srv) {
        assert_clean(&format!("description of {name}"), &text);
    }
}

/// Item 37's sentence, with the reason between its parentheses.
const NOT_FETCHED: &str = "What this mesh holds could not be fetched (";
const STILL_WORKS: &str = "); find still works.";

#[test]
fn a_failing_summary_is_said_and_not_replaced_silently() {
    let (plain, _s0, _u0, _d0) = server(None, &[]);
    let static_desc = descriptions(&plain);
    assert!(!instructions(&plain).contains("Sample passages"));
    let (srv, stub, _up, _dir) = server(None, &[HOST]);
    assert!(stub.summary_requests() >= 1, "it tried");
    // Item 37 (0.5.2): the failure is said, with its reason, in one sentence; no silent fallback.
    let ins = instructions(&srv);
    let start =
        ins.find(NOT_FETCHED).unwrap_or_else(|| panic!("says it could not be fetched: {ins}")) + NOT_FETCHED.len();
    let end = ins[start..].find(STILL_WORKS).unwrap_or_else(|| panic!("says find still works: {ins}")) + start;
    assert!(!ins[start..end].trim().is_empty(), "a reason: {ins}");
    assert_eq!(descriptions(&srv), static_desc);
    // And an unparseable summary is a failure too.
    let (bad, _s, _u, _d) = server(Some("not json"), &[HOST]);
    let ins = instructions(&bad);
    assert!(ins.contains(NOT_FETCHED) && ins.contains(STILL_WORKS), "{ins}");
}

#[test]
fn the_static_text_keeps_its_0_3_3_words() {
    let (plain, _s, _u, _d) = server(None, &[]);
    assert!(instructions(&plain).starts_with("Datumesh: every answer is a signed receipt."));
}
