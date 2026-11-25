//! datumesh-local 0.5.2, item 39: find answers definitely.
//!
//! * Nothing held for a query (local and the find meshes searched, none matching): a quick answer
//!   that says "nothing on this mesh matches; look elsewhere", with the meshes searched listed. It
//!   comes without waiting on a timeout.
//! * A find mesh unavailable: the answer says whether a retry is worth it. Each `searched` entry of
//!   an unavailable mesh (CLI `find` JSON and the MCP tool's structured content) has `"retry": true`
//!   for a network failure or a timeout, `"retry": false` for not allowed, offline mode or a 4xx.
//!
//! The production fake answers `/find?q=...` with production's empty answer
//! (`{"query", "results": [], "vector": false}`), or with a status set by the test.

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::McpServer;
use datumesh_local::{Config, Resolver};
use prod_mesh::*;
use tools::{everything, items, jcs, structured, tool};

const NOTHING: &str = "nothing on this mesh matches; look elsewhere";

fn parse(out: &str) -> Value {
    json::parse(out.trim_end().as_bytes()).unwrap_or_else(|e| panic!("not JSON: {out}: {e:?}"))
}

fn searched(v: &Value) -> Vec<Value> {
    match v.get("searched") {
        Some(Value::Array(a)) => a.clone(),
        other => panic!("searched: {other:?} in {}", jcs(v)),
    }
}

/// The `searched` entry whose `where` names `mesh`.
fn entry(v: &Value, mesh: &str) -> Value {
    searched(v)
        .into_iter()
        .find(|e| s(e, "where").contains(mesh))
        .unwrap_or_else(|| panic!("{mesh} is listed as searched: {}", jcs(v)))
}

fn retry(e: &Value) -> Option<bool> {
    match e.get("retry") {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// A home on the production fake whose find meshes are `datumesh.com` and `extra` (allowed when
/// `allowed`), `extra` reached at `origin`, with a request time limit of `timeout` seconds.
fn home(prod: &ProdMesh, extra: Option<(&str, &str, bool)>, timeout: u64) -> TempDir {
    let mut peers = vec![("datumesh.com", prod.origin.as_str())];
    if let Some((host, origin, _)) = extra {
        peers.push((host, origin));
    }
    let dir = fresh_home(&peers);
    let p = dir.path().join("config.json");
    let mut text = std::fs::read_to_string(&p).unwrap();
    assert!(text.contains("\"timeout_seconds\": 10"), "{text}");
    text = text.replace("\"timeout_seconds\": 10", &format!("\"timeout_seconds\": {timeout}"));
    if let Some((host, _, _)) = extra {
        assert!(text.contains("\"find_meshes\": [\"datumesh.com\"]"), "{text}");
        text = text.replace(
            "\"find_meshes\": [\"datumesh.com\"]",
            &format!("\"find_meshes\": [\"datumesh.com\", \"{host}\"]"),
        );
    }
    std::fs::write(&p, text).unwrap();
    if let Some((host, _, true)) = extra {
        let (code, _, err) = cli(&dir, &["allow", "add", host]);
        assert_eq!(code, 0, "{err}");
    }
    dir
}

/// An origin where nothing listens (connection refused).
fn closed_origin() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", l.local_addr().unwrap())
}

/// An origin that accepts connections and never answers (a timeout).
fn silent_origin() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for c in l.incoming().flatten() {
            held.push(c);
        }
    });
    origin
}

fn find(dir: &TempDir, query: &str) -> (Value, Duration) {
    let t = Instant::now();
    let (code, out, err) = cli(dir, &["find", query]);
    let took = t.elapsed();
    assert_eq!(code, 0, "{err}");
    (parse(&out), took)
}

fn mcp(dir: &TempDir) -> McpServer {
    McpServer::new(Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap()))
}

// ------------------------------------------------------------------------------- nothing matches

#[test]
fn nothing_held_anywhere_is_a_quick_definite_answer_listing_the_meshes_searched() {
    let prod = ProdMesh::start(true);
    let dir = home(&prod, None, 10);
    let (v, took) = find(&dir, "zebra crossing statistics");
    let text = jcs(&v);
    assert!(text.contains(NOTHING), "{text}");
    assert!(took < Duration::from_secs(3), "no timeout waited on: {took:?}");
    let ds = entry(&v, "datumesh.com");
    assert_eq!(s(&ds, "status"), "ok", "{text}");
    assert!(searched(&v).iter().any(|e| s(e, "where") == "local"), "local is listed: {text}");
    // The agent is told the same.
    let t = Instant::now();
    let m = tool(&mcp(&dir), "find", r#"{"query":"zebra crossing statistics 2"}"#);
    assert!(t.elapsed() < Duration::from_secs(3));
    assert!(items(&m).join("\n").contains(NOTHING), "{}", everything(&m));
    entry(structured(&m), "datumesh.com");
}

#[test]
fn a_match_is_not_told_to_look_elsewhere() {
    let prod = ProdMesh::start(true);
    let dir = home(&prod, None, 10);
    let (code, _, err) = cli(&dir, &["get", BANK_RATE_DAY]);
    assert_eq!(code, 0, "{err}");
    let (v, _) = find(&dir, "bank_rate");
    assert!(!jcs(&v).contains(NOTHING), "{}", jcs(&v));
}

// ------------------------------------------------------------------------------- unavailable

#[test]
fn a_find_mesh_refusing_connections_is_worth_retrying() {
    let prod = ProdMesh::start(true);
    let closed = closed_origin();
    let dir = home(&prod, Some(("down.example", &closed, true)), 10);
    let (v, _) = find(&dir, "zebra one");
    assert_eq!(retry(&entry(&v, "down.example")), Some(true), "{}", jcs(&v));
    let m = tool(&mcp(&dir), "find", r#"{"query":"zebra one m"}"#);
    assert_eq!(retry(&entry(structured(&m), "down.example")), Some(true), "{}", everything(&m));
}

#[test]
fn a_find_mesh_that_times_out_is_worth_retrying() {
    let prod = ProdMesh::start(true);
    let silent = silent_origin();
    let dir = home(&prod, Some(("slow.example", &silent, true)), 1);
    let (v, took) = find(&dir, "zebra two");
    assert_eq!(retry(&entry(&v, "slow.example")), Some(true), "{}", jcs(&v));
    assert!(took < Duration::from_secs(30), "{took:?}");
}

#[test]
fn a_4xx_from_a_find_mesh_is_not_worth_retrying() {
    for status in [403u16, 404] {
        let prod = ProdMesh::start(true);
        prod.set_find_status(status);
        let dir = home(&prod, None, 10);
        let (v, _) = find(&dir, &format!("zebra {status}"));
        let e = entry(&v, "datumesh.com");
        assert_eq!(retry(&e), Some(false), "{status}: {}", jcs(&v));
        assert!(!jcs(&v).contains(NOTHING), "an unavailable mesh is not 'nothing matches': {}", jcs(&v));
        let m = tool(&mcp(&dir), "find", &format!(r#"{{"query":"zebra {status} m"}}"#));
        assert_eq!(retry(&entry(structured(&m), "datumesh.com")), Some(false), "{}", everything(&m));
    }
}

#[test]
fn a_5xx_from_a_find_mesh_is_worth_retrying() {
    let prod = ProdMesh::start(true);
    prod.set_find_status(503);
    let dir = home(&prod, None, 10);
    let (v, _) = find(&dir, "zebra 503");
    assert_eq!(retry(&entry(&v, "datumesh.com")), Some(true), "{}", jcs(&v));
}

#[test]
fn a_find_mesh_not_on_the_allowlist_is_not_worth_retrying() {
    let prod = ProdMesh::start(true);
    let closed = closed_origin();
    let dir = home(&prod, Some(("elsewhere.example", &closed, false)), 10);
    let (v, _) = find(&dir, "zebra three");
    let e = entry(&v, "elsewhere.example");
    assert_eq!(s(&e, "status"), "not allowed", "{}", jcs(&v));
    assert_eq!(retry(&e), Some(false), "{}", jcs(&v));
}

#[test]
fn offline_mode_is_not_worth_retrying() {
    let prod = ProdMesh::start(true);
    let dir = home(&prod, None, 10);
    let (code, out, err) = cli(&dir, &["--offline", "find", "zebra four"]);
    assert_eq!(code, 0, "{err}");
    let v = parse(&out);
    let e = entry(&v, "datumesh.com");
    assert_eq!(s(&e, "status"), "offline", "{out}");
    assert_eq!(retry(&e), Some(false), "{out}");
    assert!(prod.requests().iter().all(|r| !r.target.starts_with("/find?q")), "nothing was asked offline");
}
