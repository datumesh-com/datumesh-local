//! datumesh-local 0.5.2, item 37: no silent fallback when the catalogue summary
//! (`GET /find?summary=1`) cannot be fetched.
//!
//! * The instructions say so in one sentence: "What this mesh holds could not be fetched
//!   (<reason>); find still works." (0.5.1 falls back silently to the static text of 0.3.3.)
//! * The server tries again on the next request that needs it (`find`, `get` or `tools/list`), at
//!   most once a minute. The minute is measured on the resolver's clock (`Resolver::open_with_clock`),
//!   so these tests advance a `ManualClock` instead of waiting.
//! * After a success, the surface carries the real `held` sentence (in the instructions, item 36).
//!
//! `notifications/tools/list_changed` (sent when the client declared support) is not tested here:
//! `McpServer::handle` answers one message with one response.

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Profile, Transport};
use datumesh_local::{Config, Resolver};
use prod_mesh::*;
use tools::{q, result, rpc_on, tool};

const T0: &str = "2026-11-25T10:00:00.000000Z";
const T10S: &str = "2026-11-25T10:00:10.000000Z";
const T59S: &str = "2026-11-25T10:00:59.000000Z";
const T61S: &str = "2026-11-25T10:01:01.000000Z";
const T2M: &str = "2026-11-25T10:02:02.000000Z";
const NOT_FETCHED: &str = "What this mesh holds could not be fetched (";
const STILL_WORKS: &str = "); find still works.";

fn production_held() -> String {
    let text = std::fs::read_to_string(data("production/routes.jsonl")).unwrap();
    for line in text.lines() {
        let v = json::parse(line.as_bytes()).unwrap();
        if v.get("path").and_then(Value::as_str) == Some("/find?summary=1") {
            let body = json::parse(v.get("body").and_then(Value::as_str).unwrap().as_bytes()).unwrap();
            return s(&body, "held");
        }
    }
    panic!("no recorded summary")
}

fn server(dir: &TempDir, clock: &Arc<ManualClock>, profile: Profile) -> McpServer {
    let r = Resolver::open_with_clock(Config::in_dir(dir.path()).unwrap(), clock.clone()).unwrap();
    McpServer::with_profile(Arc::new(r), profile)
}

fn instructions(srv: &McpServer) -> String {
    s(result(&rpc_on(srv, Transport::Stdio, "server/discover", "")), "instructions")
}

/// The reason between the sentence's parentheses.
fn reason(text: &str) -> String {
    let start =
        text.find(NOT_FETCHED).unwrap_or_else(|| panic!("says it could not be fetched: {text}")) + NOT_FETCHED.len();
    let end = text[start..].find(STILL_WORKS).unwrap_or_else(|| panic!("says find still works: {text}")) + start;
    text[start..end].to_string()
}

fn tools_list(srv: &McpServer) {
    let _ = rpc_on(srv, Transport::Stdio, "tools/list", "");
}

#[test]
fn a_failed_summary_is_said_with_its_reason_in_both_profiles() {
    let prod = ProdMesh::start(true);
    prod.fail_summary(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let clock = ManualClock::new(T0);
    for profile in [Profile::Full, Profile::Small] {
        let text = instructions(&server(&dir, &clock, profile));
        let why = reason(&text);
        assert!(why.contains("503"), "{profile:?}: the reason names the status: {why}");
        assert!(!text.contains(&production_held()), "{text}");
        assert_eq!(text.matches(NOT_FETCHED).count(), 1, "one sentence: {text}");
    }
}

#[test]
fn an_unreachable_find_mesh_is_said_too() {
    // Nothing listens on the peer's port: connection refused.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let dir = fresh_home(&[("datumesh.com", &closed)]);
    let clock = ManualClock::new(T0);
    let text = instructions(&server(&dir, &clock, Profile::Full));
    assert!(!reason(&text).trim().is_empty(), "a reason is given: {text}");
}

#[test]
fn tools_list_retries_after_a_minute_and_then_carries_the_held_sentence() {
    let prod = ProdMesh::start(true);
    prod.fail_summary(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let clock = ManualClock::new(T0);
    let srv = server(&dir, &clock, Profile::Full);
    let first = prod.summary_requests();
    assert!(first >= 1, "asked at start");
    assert!(instructions(&srv).contains(NOT_FETCHED));
    // The mesh recovers; within the minute nothing is asked again.
    prod.fail_summary(false);
    for t in [T10S, T59S] {
        clock.set(t);
        tools_list(&srv);
        assert_eq!(prod.summary_requests(), first, "no retry before a minute has passed ({t})");
    }
    assert!(instructions(&srv).contains(NOT_FETCHED), "still the failure, not a silent fallback");
    // After the minute, the next tools/list asks again, and the surface carries the real text.
    clock.set(T61S);
    tools_list(&srv);
    assert_eq!(prod.summary_requests(), first + 1, "one retry");
    let text = instructions(&srv);
    assert!(text.contains(&production_held()), "{text}");
    assert!(!text.contains(NOT_FETCHED), "{text}");
    // Once it has the summary, it does not ask again.
    clock.set(T2M);
    tools_list(&srv);
    assert_eq!(prod.summary_requests(), first + 1);
}

#[test]
fn a_retry_that_fails_again_waits_another_minute() {
    let prod = ProdMesh::start(true);
    prod.fail_summary(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let clock = ManualClock::new(T0);
    let srv = server(&dir, &clock, Profile::Small);
    let first = prod.summary_requests();
    clock.set(T61S);
    tools_list(&srv);
    assert_eq!(prod.summary_requests(), first + 1, "retried after a minute");
    assert!(instructions(&srv).contains(NOT_FETCHED));
    // Ten seconds after the failed retry: no request.
    clock.set("2026-11-25T10:01:11.000000Z");
    tools_list(&srv);
    assert_eq!(prod.summary_requests(), first + 1, "at most once a minute");
}

#[test]
fn find_and_get_retry_too() {
    for which in ["find", "get"] {
        let prod = ProdMesh::start(true);
        prod.fail_summary(true);
        let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
        let clock = ManualClock::new(T0);
        let srv = server(&dir, &clock, Profile::Full);
        let first = prod.summary_requests();
        prod.fail_summary(false);
        clock.set(T61S);
        let _ = match which {
            "find" => tool(&srv, "find", r#"{"query":"bank rate"}"#),
            _ => tool(&srv, "get", &format!(r#"{{"request":{}}}"#, q(BANK_RATE_DAY))),
        };
        assert_eq!(prod.summary_requests(), first + 1, "{which} retries");
        assert!(instructions(&srv).contains(&production_held()), "{which}: the held sentence after the retry");
    }
}
