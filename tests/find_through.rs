//! CONTRACT-1.7a L1-L4 (datumesh-local 0.3.2): `find` falls through to the configured meshes.
//!
//! The mesh is the test mesh with a stub `/find` in datumesh-http 0.3.1's answer shape (H3),
//! reached through `peers`; the stub counts its requests (`common::FindStub`).
//!
//! New API these tests reference (compile-failing until L2 lands):
//! * `Config::find_meshes: Vec<String>` (L1);
//! * `Resolver::find_in(&self, &str, bool) -> Result<FindAnswer, Error>`, `FindAnswer::results:
//!   Vec<Found>` (L2);
//! * `Found::kind` and `Found::description`, both usable with `to_string()` (L2).

mod common;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Transport};
use datumesh_local::Config;

const M: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;
const FIND_URL: &str = "https://mesh-test.datumesh.com/find";
const DAY0: &str = "2026-11-10T00:00:00.000000Z";

fn q(s: &str) -> String {
    String::from_utf8(Value::String(s.to_string()).to_jcs()).unwrap()
}

fn array<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        other => panic!("{k} is not an array: {other:?} in {}", String::from_utf8_lossy(&v.to_jcs())),
    }
}

/// A configuration whose `find_meshes` is `meshes`, reaching the stub mesh for the test mesh.
fn cfg(dir: &TempDir, up: &Upstream, meshes: &[&str]) -> Config {
    let mut c = config(dir, up);
    c.find_meshes = meshes.iter().map(|m| m.to_string()).collect();
    c
}

fn server_of(c: Config) -> McpServer {
    McpServer::new(Arc::new(datumesh_local::Resolver::open(c).unwrap()))
}

/// MCP `find`: the structured result.
fn mcp_find(srv: &McpServer, query: &str, remote: bool) -> Value {
    let args = format!(r#"{{"query":{},"remote":{remote}}}"#, q(query));
    let line = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{M},"name":"find","arguments":{args}}}}}"#
    );
    let out = srv.handle(&line, Transport::Stdio).expect("a response");
    let v = json::parse(out.as_bytes()).unwrap();
    v.get("result")
        .and_then(|r| r.get("structuredContent"))
        .cloned()
        .unwrap_or_else(|| panic!("a find result in {}", String::from_utf8_lossy(&v.to_jcs())))
}

fn searched(v: &Value) -> Vec<(String, Option<String>)> {
    array(v, "searched")
        .iter()
        .map(|e| (s(e, "where"), e.get("status").and_then(Value::as_str).map(str::to_string)))
        .collect()
}

fn local_entry() -> (String, Option<String>) {
    ("local".to_string(), None)
}

fn mesh_entry(status: &str) -> (String, Option<String>) {
    (FIND_URL.to_string(), Some(status.to_string()))
}

// ---- L1 ---------------------------------------------------------------------------------------

#[test]
fn l1_find_meshes_defaults_to_datumesh_com_and_is_in_the_installed_file() {
    let dir = TempDir::new("find-l1");
    let c = Config::in_dir(dir.path()).unwrap();
    assert_eq!(c.find_meshes, vec!["datumesh.com".to_string()]);
    let installed = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(installed.contains("\"find_meshes\": [\"datumesh.com\"]"), "{installed}");
    // The member has a comment above it, in defaults/config.json.
    let at = installed.find("\"find_meshes\"").unwrap();
    let before: Vec<&str> = installed[..at].lines().collect();
    let last = before[before.len() - 2].trim_start().to_string();
    assert!(last.starts_with("//"), "no comment above find_meshes: {last:?}");
    // An edit stands.
    std::fs::write(
        dir.path().join("config.json"),
        installed.replace("\"find_meshes\": [\"datumesh.com\"]", "\"find_meshes\": [\"a.example\", \"b.example\"]"),
    )
    .unwrap();
    assert_eq!(Config::in_dir(dir.path()).unwrap().find_meshes, vec!["a.example".to_string(), "b.example".to_string()]);
}

// ---- L4 ---------------------------------------------------------------------------------------

#[test]
fn l4_fresh_home_find_quran_returns_the_remote_class_and_searched_is_local_then_the_mesh() {
    let (up, stub) = upstream_finding();
    let dir = TempDir::new("find-fresh");
    let srv = server_of(cfg(&dir, &up, &[HOST]));
    let v = mcp_find(&srv, "quran", false);
    assert_eq!(searched(&v), vec![local_entry(), mesh_entry("ok")]);
    let results = array(&v, "results");
    assert_eq!(results.len(), 2, "the class and its one method: {}", String::from_utf8_lossy(&v.to_jcs()));
    assert_eq!(s(&results[0], "name"), VERSE_CLASS);
    assert_eq!(s(&results[0], "kind"), "class");
    assert!(s(&results[0], "description").contains("Quran"), "{}", s(&results[0], "description"));
    // One GET with q=quran.
    assert_eq!(stub.finds(), 1);
    assert!(stub.queries.lock().unwrap()[0].split('&').any(|p| p == "q=quran"), "{:?}", stub.queries);
}

#[test]
fn l4_the_resolver_api_gives_results_with_kind_description_and_held() {
    let (up, _stub) = upstream_finding();
    let dir = TempDir::new("find-api");
    let r = datumesh_local::Resolver::open(cfg(&dir, &up, &[HOST])).unwrap();
    let a = r.find_in("quran", false).unwrap();
    assert_eq!(a.results.len(), 1);
    let f = &a.results[0];
    assert_eq!(f.name, VERSE_CLASS);
    assert_eq!(f.kind.to_string(), "class");
    assert!(f.description.to_string().contains("Quran"));
    assert!(!f.held);
}

#[test]
fn l4_a_second_identical_call_makes_no_request() {
    let (up, stub) = upstream_finding();
    let dir = TempDir::new("find-cache");
    let srv = server_of(cfg(&dir, &up, &[HOST]));
    let first = mcp_find(&srv, "quran", false);
    let (finds, all) = (stub.finds(), up.requests());
    let second = mcp_find(&srv, "quran", false);
    assert_eq!(stub.finds(), finds);
    assert_eq!(up.requests(), all, "any request at all");
    assert_eq!(array(&second, "results"), array(&first, "results"));
    assert_eq!(searched(&second), vec![local_entry(), mesh_entry("cached")]);
    // The key is lowercased: the same words in another case are the same entry.
    let third = mcp_find(&srv, "QURAN", false);
    assert_eq!(up.requests(), all);
    assert_eq!(searched(&third), vec![local_entry(), mesh_entry("cached")]);
    // The cache is in the index: a new resolver on the same home asks nothing either.
    drop(srv);
    // (A server asks the catalogue summary when it starts, R4: that is before the snapshot.)
    let again = server_of(cfg(&dir, &up, &[HOST]));
    let all = up.requests();
    let fourth = mcp_find(&again, "quran", false);
    assert_eq!(up.requests(), all);
    assert_eq!(array(&fourth, "results"), array(&first, "results"));
    assert_eq!(searched(&fourth), vec![local_entry(), mesh_entry("cached")]);
    // A different query is a different entry.
    mcp_find(&srv_again(&dir, &up), "torah", false);
    assert_eq!(stub.finds(), finds + 1);
}

fn srv_again(dir: &TempDir, up: &Upstream) -> McpServer {
    server_of(cfg(dir, up, &[HOST]))
}

#[test]
fn l4_a_cached_answer_lasts_24_hours() {
    let (up, stub) = upstream_finding();
    let dir = TempDir::new("find-ttl");
    let clock = ManualClock::new(DAY0);
    let r = Arc::new(datumesh_local::Resolver::open_with_clock(cfg(&dir, &up, &[HOST]), clock.clone()).unwrap());
    let srv = McpServer::new(r);
    mcp_find(&srv, "quran", false);
    assert_eq!(stub.finds(), 1);
    clock.set("2026-11-10T23:00:00.000000Z");
    mcp_find(&srv, "quran", false);
    assert_eq!(stub.finds(), 1, "23 hours: still cached");
    clock.set("2026-11-11T01:00:00.000000Z");
    let v = mcp_find(&srv, "quran", false);
    assert_eq!(stub.finds(), 2, "25 hours: asked again");
    assert_eq!(searched(&v), vec![local_entry(), mesh_entry("ok")]);
}

#[test]
fn l2_a_local_hit_without_remote_asks_no_mesh() {
    let (up, stub) = upstream_finding();
    let dir = TempDir::new("find-local-hit");
    let r = Arc::new(datumesh_local::Resolver::open(cfg(&dir, &up, &[HOST])).unwrap());
    r.define_stored(
        "local/quran_notes",
        &json::parse(br#"{"args": {"k": {"type": "string", "valid": false}}, "order": ["k"], "returns": "string"}"#)
            .unwrap(),
    )
    .unwrap();
    // A server asks the mesh's catalogue summary when it starts (R4): the snapshot is after that.
    let srv = McpServer::new(r);
    let before = up.requests();
    let v = mcp_find(&srv, "quran", false);
    assert_eq!(up.requests(), before);
    assert_eq!(stub.finds(), 0);
    assert_eq!(searched(&v), vec![local_entry()]);
    let results = array(&v, "results");
    assert_eq!(results.len(), 1);
    assert_eq!(s(&results[0], "name"), "local.invalid/quran_notes");
    assert_eq!(s(&results[0], "kind"), "function");
    assert!(results[0].get("description").is_some(), "{}", String::from_utf8_lossy(&results[0].to_jcs()));
}

#[test]
fn l2_remote_true_asks_the_meshes_even_after_a_local_hit_and_the_forward_mesh_once() {
    let (up, stub) = upstream_finding();
    // find_meshes lists the mesh, which is also the forward mesh: asked once, listed once.
    let dir = TempDir::new("find-remote-listed");
    let r = Arc::new(datumesh_local::Resolver::open(cfg(&dir, &up, &[HOST])).unwrap());
    r.define_stored(
        "local/quran_notes",
        &json::parse(br#"{"args": {"k": {"type": "string", "valid": false}}, "order": ["k"], "returns": "string"}"#)
            .unwrap(),
    )
    .unwrap();
    let srv = McpServer::new(r);
    let v = mcp_find(&srv, "quran", true);
    assert_eq!(stub.finds(), 1);
    assert_eq!(searched(&v), vec![local_entry(), mesh_entry("ok")]);
    let names: Vec<&str> = array(&v, "results").iter().filter_map(|e| e.get("name").and_then(Value::as_str)).collect();
    assert!(names.contains(&"local.invalid/quran_notes") && names.contains(&VERSE_CLASS), "{names:?}");
    // find_meshes empty: remote adds the forward mesh.
    let dir2 = TempDir::new("find-remote-forward");
    let srv2 = server_of(cfg(&dir2, &up, &[]));
    let v2 = mcp_find(&srv2, "quran", true);
    assert_eq!(stub.finds(), 2);
    assert_eq!(searched(&v2), vec![local_entry(), mesh_entry("ok")]);
}

#[test]
fn l4_a_mesh_not_on_the_allowlist_is_not_asked_and_is_listed_as_not_allowed() {
    let (up, stub) = upstream_finding();
    let dir = TempDir::new("find-not-allowed");
    let mut c = cfg(&dir, &up, &[HOST]);
    c.allowlist = vec!["datumesh.com".to_string()];
    let srv = server_of(c);
    let v = mcp_find(&srv, "quran", false);
    assert_eq!(up.requests(), 0, "any request at all");
    assert_eq!(stub.finds(), 0);
    assert!(array(&v, "results").is_empty());
    let s = searched(&v);
    assert_eq!(s.len(), 2, "{s:?}");
    assert_eq!(s[0], local_entry());
    assert_eq!(s[1].1.as_deref(), Some("not allowed"));
    assert!(s[1].0.contains(HOST), "{s:?}");
}

#[test]
fn l4_offline_asks_nothing_and_says_so() {
    let (up, stub) = upstream_finding();
    let dir = TempDir::new("find-offline");
    let mut c = cfg(&dir, &up, &[HOST]);
    c.offline = true;
    let srv = server_of(c);
    let v = mcp_find(&srv, "quran", false);
    assert_eq!(up.requests(), 0);
    assert_eq!(stub.finds(), 0);
    assert!(array(&v, "results").is_empty());
    let s = searched(&v);
    assert_eq!(s.len(), 2, "{s:?}");
    assert_eq!(s[0], local_entry());
    assert_eq!(s[1].1.as_deref(), Some("offline"));
    // Also with remote asked for.
    let v = mcp_find(&srv, "quran", true);
    assert_eq!(up.requests(), 0);
    assert_eq!(searched(&v)[1].1.as_deref(), Some("offline"));
}

#[test]
fn l4_a_failing_mesh_gives_empty_results_and_the_error_in_searched() {
    let (up, stub) = upstream_finding();
    stub.fail.store(true, Ordering::SeqCst);
    let dir = TempDir::new("find-fail");
    let srv = server_of(cfg(&dir, &up, &[HOST]));
    let v = mcp_find(&srv, "quran", false);
    assert!(array(&v, "results").is_empty());
    let s = searched(&v);
    assert_eq!(s.len(), 2, "{s:?}");
    assert_eq!(s[0], local_entry());
    assert_eq!(s[1].0, FIND_URL);
    let status = s[1].1.clone().unwrap();
    assert!(status.starts_with("E_"), "{status}");
    assert!(status.contains("500"), "{status}");
    // The failure is not an answer to keep: once the mesh is well, the next call asks again.
    stub.fail.store(false, Ordering::SeqCst);
    let v = mcp_find(&srv, "quran", false);
    assert_eq!(array(&v, "results").len(), 2);
    assert_eq!(searched(&v), vec![local_entry(), mesh_entry("ok")]);
}

// ---- L3 ---------------------------------------------------------------------------------------

#[test]
fn l3_a_remote_class_lists_its_methods_with_call_templates_from_the_meshes_args() {
    let (up, _stub) = upstream_finding();
    let dir = TempDir::new("find-mcp-methods");
    let srv = server_of(cfg(&dir, &up, &[HOST]));
    let v = mcp_find(&srv, "quran", false);
    let results = array(&v, "results");
    assert_eq!(s(&results[0], "kind"), "class");
    assert_eq!(s(&results[0], "name"), VERSE_CLASS);
    // As for a local class: the methods follow it, one entry each, with a call template.
    let m = &results[1];
    assert_eq!(s(m, "kind"), "method");
    assert_eq!(s(m, "name"), format!("{VERSE_CLASS}/text"));
    let call = s(m, "call");
    assert!(call.starts_with(&format!("{VERSE_CLASS}/text(")), "{call}");
    assert!(call.contains("edition: string"), "{call}");
}

// ---- L3: the CLI ------------------------------------------------------------------------------

fn home(up: &Upstream) -> TempDir {
    let dir = TempDir::new("find-cli");
    Config::in_dir(dir.path()).unwrap();
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let with_peer = text.replace(
        "\"peers\": {}",
        &format!("\"find_meshes\": [\"{HOST}\"], \"peers\": {{\"{HOST}\": \"{}\"}}", up.origin),
    );
    assert_ne!(with_peer, text);
    // The default `find_meshes` member is replaced by the one written here.
    let with_peer = with_peer.replacen("\"find_meshes\": [\"datumesh.com\"],", "", 1);
    std::fs::write(&path, with_peer).unwrap();
    dir
}

fn run(dir: &TempDir, args: &[&str]) -> (i32, String, String) {
    let mut all = vec!["--home".to_string(), dir.path().to_string_lossy().into_owned()];
    all.extend(args.iter().map(|a| a.to_string()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&all, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

#[test]
fn l3_the_cli_prints_searched_when_there_are_no_results() {
    let (up, stub) = upstream_finding();
    let dir = home(&up);
    let (code, out, err) = run(&dir, &["find", "nothing_matches_this"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(stub.finds(), 1);
    let all = format!("{out}{err}");
    assert!(all.contains("searched:"), "{all}");
    assert!(all.contains(FIND_URL), "{all}");
}

#[test]
fn l3_the_cli_finds_the_remote_class_from_a_fresh_home() {
    let (up, stub) = upstream_finding();
    let dir = home(&up);
    let (code, out, err) = run(&dir, &["find", "quran"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(stub.finds(), 1);
    assert!(out.contains(VERSE_CLASS), "{out}");
    // The second command makes no request (the index is the home's).
    let all = up.requests();
    let (code, out2, _) = run(&dir, &["find", "quran"]);
    assert_eq!(code, 0);
    assert_eq!(up.requests(), all);
    assert!(out2.contains(VERSE_CLASS), "{out2}");
}
