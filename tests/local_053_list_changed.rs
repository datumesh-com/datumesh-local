//! datumesh-local 0.5.3, the guard patch's A4 (NEXT-PATCH-QUEUE item 11): after a catalogue retry
//! that succeeds, the server sends `notifications/tools/list_changed`, and it advertises
//! `capabilities.tools.listChanged: true`. Written before the implementation.
//!
//! * `initialize` (and `server/discover`) give `capabilities.tools.listChanged: true`.
//! * The instructions are sent only at the handshake, so the notification is what tells a client
//!   that what the mesh holds is now known: after the retry of item 37 succeeds (on `tools/list`,
//!   `find` or `get`, at most once a minute on the resolver's clock), the server writes one
//!   notification line, `{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}` (no `id`),
//!   on its output, exactly once. A retry that fails again sends none, nor does a server whose
//!   summary was fetched at start, nor any later request.
//! * The next `tools/list` carries the summary: its result (anywhere in it; `tools` itself stays
//!   the full profile's snapshot, `local_052_mcp_profile.rs`) has the mesh's `held` sentence, and,
//!   in the small profile, each catalogue subject's label.
//!
//! The server is driven as `datumesh mcp` runs it on stdio: `serve_stdio_with` over a reader that
//! yields one JSON-RPC line at a time and, before each line, moves the resolver's clock or the
//! production fake's summary fault. So the seam is the existing one: `serve_stdio_with` writes
//! every line the server has to send (responses and notifications) to its output. Nothing reaches
//! the network: the find mesh is the production fake (`tests/support/prod_mesh.rs`).

#![cfg(unix)]

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::io::{BufRead, Read};
use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{serve_stdio_with, Profile};
use datumesh_local::{Config, Resolver};
use prod_mesh::*;
use tools::{jcs, q};

const T0: &str = "2026-11-25T10:00:00.000000Z";
const T10S: &str = "2026-11-25T10:00:10.000000Z";
const T61S: &str = "2026-11-25T10:01:01.000000Z";
const T2M: &str = "2026-11-25T10:02:02.000000Z";
const T3M: &str = "2026-11-25T10:03:03.000000Z";
const T5M: &str = "2026-11-25T10:05:05.000000Z";
const LIST_CHANGED: &str = "notifications/tools/list_changed";

/// What happens before a line is read: the clock is moved, the summary fault switched.
enum Before {
    Nothing,
    Clock(&'static str),
    Both(&'static str, bool),
}

/// A stdin of JSON-RPC lines, each read only when the server asks for it (so each `Before` runs after
/// the previous line was answered).
struct Script<'a> {
    steps: std::collections::VecDeque<(Before, String)>,
    clock: Arc<ManualClock>,
    prod: &'a ProdMesh,
    buf: Vec<u8>,
    at: usize,
}

impl Read for Script<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = {
            let avail = self.fill_buf()?;
            let n = avail.len().min(out.len());
            out[..n].copy_from_slice(&avail[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for Script<'_> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.at >= self.buf.len() {
            if let Some((before, line)) = self.steps.pop_front() {
                match before {
                    Before::Nothing => {}
                    Before::Clock(t) => self.clock.set(t),
                    Before::Both(t, down) => {
                        self.clock.set(t);
                        self.prod.fail_summary(down);
                    }
                }
                self.buf = format!("{line}\n").into_bytes();
                self.at = 0;
            } else {
                return Ok(&[]);
            }
        }
        Ok(&self.buf[self.at..])
    }

    fn consume(&mut self, n: usize) {
        self.at += n;
    }
}

fn initialize(id: u64) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"t","version":"1"}}}}}}"#
    )
}

fn initialized() -> String {
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_string()
}

fn request(id: u64, method: &str, params: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"method":{},"params":{params}}}"#, q(method))
}

fn tools_list(id: u64) -> String {
    request(id, "tools/list", "{}")
}

/// Runs the script on a fresh server of `profile` (the summary failing at start when `down`): every
/// line the server wrote, parsed.
fn run(profile: Profile, down: bool, steps: Vec<(Before, String)>) -> (ProdMesh, Vec<Value>) {
    let prod = ProdMesh::start(true);
    prod.fail_summary(down);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let clock = ManualClock::new(T0);
    let r = Arc::new(Resolver::open_with_clock(Config::in_dir(dir.path()).unwrap(), clock.clone()).unwrap());
    let script = Script { steps: steps.into(), clock, prod: &prod, buf: Vec::new(), at: 0 };
    let mut out = Vec::new();
    serve_stdio_with(r, profile, script, &mut out).unwrap();
    let lines = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| json::parse(l.as_bytes()).unwrap_or_else(|e| panic!("a JSON line: {l}: {e:?}")))
        .collect();
    drop(dir);
    (prod, lines)
}

fn is_list_changed(v: &Value) -> bool {
    v.get("method").and_then(Value::as_str) == Some(LIST_CHANGED)
}

fn notifications(lines: &[Value]) -> Vec<usize> {
    lines.iter().enumerate().filter(|(_, v)| is_list_changed(v)).map(|(i, _)| i).collect()
}

/// The index of the response to `id`.
fn response(lines: &[Value], id: u64) -> usize {
    lines
        .iter()
        .position(|v| v.get("id").and_then(Value::as_number_text) == Some(id.to_string().as_str()))
        .unwrap_or_else(|| panic!("no response {id}: {}", lines.iter().map(jcs).collect::<Vec<_>>().join("\n")))
}

fn result_text(lines: &[Value], id: u64) -> String {
    jcs(lines[response(lines, id)].get("result").unwrap_or_else(|| panic!("a result for {id}")))
}

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

/// Production's catalogue subject labels (`describe/datumesh.com/catalog`, recorded).
fn subject_labels() -> Vec<String> {
    let text = std::fs::read_to_string(data("production/routes.jsonl")).unwrap();
    for line in text.lines() {
        let v = json::parse(line.as_bytes()).unwrap();
        if v.get("path").and_then(Value::as_str) != Some("/describe/datumesh.com/catalog") {
            continue;
        }
        let body = json::parse(v.get("body").and_then(Value::as_str).unwrap().as_bytes()).unwrap();
        let Some(Value::Array(domains)) = body.get("domains") else { panic!("domains") };
        let mut out = Vec::new();
        for d in domains {
            let Some(Value::Array(top)) = d.get("top") else { panic!("top") };
            out.extend(top.iter().map(|t| s(t, "label")));
        }
        return out;
    }
    panic!("the catalogue root is not recorded")
}

/// The JSON text of a JSON string's content, as it appears inside a JCS result.
fn in_jcs(text: &str) -> String {
    let quoted = q(text);
    quoted[1..quoted.len() - 1].to_string()
}

// ------------------------------------------------------------------------------- the capability

#[test]
fn initialize_and_discover_advertise_tools_list_changed() {
    let (_prod, lines) = run(
        Profile::Full,
        false,
        vec![
            (Before::Nothing, initialize(1)),
            (Before::Nothing, initialized()),
            (Before::Nothing, request(2, "server/discover", "{}")),
        ],
    );
    for id in [1, 2] {
        let caps = lines[response(&lines, id)].get("result").and_then(|r| r.get("capabilities")).cloned();
        let list_changed = caps.as_ref().and_then(|c| c.get("tools")).and_then(|t| t.get("listChanged")).cloned();
        assert_eq!(list_changed, Some(Value::Bool(true)), "response {id}: {:?}", caps.as_ref().map(jcs));
    }
}

// ------------------------------------------------------------------------------- the notification

#[test]
fn a_retry_that_succeeds_sends_list_changed_once_and_the_next_tools_list_has_the_summary() {
    let held = production_held();
    let (prod, lines) = run(
        Profile::Full,
        true,
        vec![
            (Before::Nothing, initialize(1)),
            (Before::Nothing, initialized()),
            // Within the minute: no retry.
            (Before::Clock(T10S), tools_list(2)),
            // The mesh recovers; after the minute the next tools/list retries and succeeds.
            (Before::Both(T61S, false), tools_list(3)),
            (Before::Nothing, tools_list(4)),
            // Later requests: no other notification.
            (Before::Clock(T2M), tools_list(5)),
            (Before::Clock(T3M), request(6, "tools/call", r#"{"name":"find","arguments":{"query":"bank rate"}}"#)),
        ],
    );
    let all = lines.iter().map(jcs).collect::<Vec<_>>().join("\n");
    let n = notifications(&lines);
    assert_eq!(n.len(), 1, "exactly one {LIST_CHANGED}:\n{all}");
    let note = &lines[n[0]];
    assert!(note.get("id").is_none(), "a notification has no id: {}", jcs(note));
    assert_eq!(note.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
    // It follows the retry (made for request 3), not before.
    assert!(n[0] > response(&lines, 2), "not before the retry:\n{all}");
    assert!(n[0] < response(&lines, 4), "sent with the request that retried:\n{all}");
    assert!(prod.summary_requests() >= 2, "the summary was asked again");
    // Before the retry, tools/list did not have the held sentence; the next one has it.
    assert!(!result_text(&lines, 2).contains(&in_jcs(&held)), "not yet: {}", result_text(&lines, 2));
    assert!(
        result_text(&lines, 4).contains(&in_jcs(&held)),
        "the next tools/list carries the summary: {}",
        result_text(&lines, 4)
    );
    // The handshake's instructions said it could not be fetched.
    assert!(result_text(&lines, 1).contains("could not be fetched"), "{}", result_text(&lines, 1));
}

#[test]
fn in_the_small_profile_the_next_tools_list_carries_the_subjects() {
    let (_prod, lines) = run(
        Profile::Small,
        true,
        vec![
            (Before::Nothing, initialize(1)),
            (Before::Nothing, initialized()),
            (Before::Both(T61S, false), tools_list(2)),
            (Before::Nothing, tools_list(3)),
        ],
    );
    let all = lines.iter().map(jcs).collect::<Vec<_>>().join("\n");
    assert_eq!(notifications(&lines).len(), 1, "{all}");
    let listed = result_text(&lines, 3);
    assert!(listed.contains(&in_jcs(&production_held())), "{listed}");
    let labels = subject_labels();
    assert!(!labels.is_empty());
    for label in labels {
        assert!(listed.contains(&in_jcs(&label)), "the subject {label}: {listed}");
    }
}

#[test]
fn find_and_get_retries_that_succeed_notify_too() {
    for (which, call) in [
        ("find", r#"{"name":"find","arguments":{"query":"bank rate"}}"#.to_string()),
        ("get", format!(r#"{{"name":"get","arguments":{{"request":{}}}}}"#, q(BANK_RATE_DAY))),
    ] {
        let (_prod, lines) = run(
            Profile::Full,
            true,
            vec![
                (Before::Nothing, initialize(1)),
                (Before::Nothing, initialized()),
                (Before::Both(T61S, false), request(2, "tools/call", &call)),
                (Before::Nothing, tools_list(3)),
            ],
        );
        let all = lines.iter().map(jcs).collect::<Vec<_>>().join("\n");
        assert_eq!(notifications(&lines).len(), 1, "{which}:\n{all}");
        assert!(result_text(&lines, 3).contains(&in_jcs(&production_held())), "{which}: {}", result_text(&lines, 3));
    }
}

#[test]
fn a_retry_that_fails_sends_nothing() {
    let (prod, lines) = run(
        Profile::Full,
        true,
        vec![
            (Before::Nothing, initialize(1)),
            (Before::Nothing, initialized()),
            (Before::Clock(T61S), tools_list(2)),
            (Before::Clock(T2M), tools_list(3)),
            (Before::Clock(T3M), request(4, "tools/call", r#"{"name":"find","arguments":{"query":"bank rate"}}"#)),
        ],
    );
    let all = lines.iter().map(jcs).collect::<Vec<_>>().join("\n");
    assert!(prod.summary_requests() >= 3, "retried: {}", prod.summary_requests());
    assert!(notifications(&lines).is_empty(), "no notification after a failed retry:\n{all}");
}

#[test]
fn a_failed_retry_then_a_good_one_notifies_once() {
    let (_prod, lines) = run(
        Profile::Full,
        true,
        vec![
            (Before::Nothing, initialize(1)),
            (Before::Nothing, initialized()),
            (Before::Clock(T61S), tools_list(2)),
            (Before::Both(T3M, false), tools_list(3)),
            (Before::Clock(T5M), tools_list(4)),
        ],
    );
    let all = lines.iter().map(jcs).collect::<Vec<_>>().join("\n");
    let n = notifications(&lines);
    assert_eq!(n.len(), 1, "{all}");
    assert!(n[0] > response(&lines, 2) && n[0] < response(&lines, 4), "{all}");
}

#[test]
fn a_summary_fetched_at_start_never_notifies() {
    let (_prod, lines) = run(
        Profile::Full,
        false,
        vec![
            (Before::Nothing, initialize(1)),
            (Before::Nothing, initialized()),
            (Before::Clock(T61S), tools_list(2)),
            (Before::Clock(T3M), tools_list(3)),
        ],
    );
    let all = lines.iter().map(jcs).collect::<Vec<_>>().join("\n");
    assert!(notifications(&lines).is_empty(), "{all}");
    assert!(result_text(&lines, 2).contains(&in_jcs(&production_held())), "{}", result_text(&lines, 2));
}
