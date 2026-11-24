//! The 0.7 round (CONTRACT-round-0.7.md R1, SPEC 3.6, 5.5, 8.1, 10.1, 16.7), local: **withdrawal is the null version.** Failing
//! tests: local does not say it yet.
//!
//! * `get` (the MCP tool and the command) of a withdrawn cell says in words that the cell is not covered, and gives the reason
//!   (the envelope's `reason`, the withdrawal's `withdrawn` text); the answer still carries the envelope as the mesh gave it.
//! * A label whose lookup cell is withdrawn is not covered with the lookup's reason, and the answer has no receipt.
//! * Local's own resolver accepts a `not_covered` envelope whose `receipt` is a withdrawal (core 0.3.13's `check_answer`): the
//!   answer is a reply, not a rejection (`E_ENVELOPE`), and the withdrawal verifies at level `signed`.
//!
//! The mesh is the test upstream with a later run (`support/r07.rs`) that withdraws the cell: a line `{"cell": ..., "withdrawn":
//! "<reason>"}` of http 0.5.0, so these tests need that publish line to be there.

mod common;
#[path = "support/r07.rs"]
mod r07;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::McpServer;
use datumesh_local::{Config, Resolver, Source};
use r07::*;

fn server(dir: &TempDir, up: &Upstream) -> McpServer {
    let c: Config = config(dir, up);
    McpServer::new(Arc::new(Resolver::open(c).unwrap()))
}

fn get(srv: &McpServer, request: &str) -> Value {
    tool(srv, "get", &format!(r#"{{"request":{}}}"#, jstr(request)))
}

/// A text item that is words, not the JSON block: says "not covered" and gives `reason`.
fn assert_says_not_covered_with(v: &Value, reason: &str) {
    let texts = items(v);
    let words: Vec<&String> = texts.iter().filter(|t| !t.trim_start().starts_with('{')).collect();
    assert!(
        words.iter().any(|t| says_not_covered(t) && t.contains(reason)),
        "a text item in words says the answer is not covered and gives the reason {reason:?}: {texts:?}"
    );
}

fn envelope_of(v: &Value) -> &Value {
    structured(v).get("envelope").unwrap_or_else(|| panic!("an envelope in {}", jcs(v)))
}

/// `get` of a withdrawn cell, a call: not covered with the reason, in words; the envelope is the mesh's (404, the withdrawal as
/// `receipt`, `reason`).
#[test]
fn mcp_get_of_a_withdrawn_cell_says_it_is_not_covered_and_gives_the_reason() {
    let up = upstream_usd_withdrawn();
    let dir = TempDir::new("r07-wd-mcp");
    let srv = server(&dir, &up);
    let v = get(&srv, USD_FULL);
    assert_says_not_covered_with(&v, REASON);
    let st = structured(&v);
    assert_eq!(st.get("status").and_then(Value::as_number_text), Some("404"), "{}", jcs(&v));
    let env = envelope_of(&v);
    assert_eq!((s(env, "outcome"), s(env, "reason")), ("not_covered".to_string(), REASON.to_string()), "{}", jcs(&v));
    assert_eq!(s(env.get("receipt").expect("the withdrawal as receipt"), "withdrawn"), REASON, "{}", jcs(&v));
    assert!(env.get("fault").is_none() && env.get("error").is_none(), "{}", jcs(&v));
}

/// The same as of an instant before the withdrawal: the old value, unchanged (history stays).
#[test]
fn mcp_get_of_the_cell_as_of_before_the_withdrawal_still_has_its_value() {
    let up = upstream_usd_withdrawn();
    let dir = TempDir::new("r07-wd-mcp-before");
    let srv = server(&dir, &up);
    let v = get(&srv, &format!("{USD_FULL}?asof=2026-11-05T00:00:00.000000Z"));
    let env = envelope_of(&v);
    assert_eq!(s(env, "outcome"), "ok", "{}", jcs(&v));
    assert!(!says_not_covered(&items(&v).join("\n").replace("not_covered", "")), "{}", jcs(&v));
}

/// A label whose lookup cell is withdrawn: not covered with the lookup's reason, in words, and no receipt.
#[test]
fn mcp_get_through_a_withdrawn_label_says_it_is_not_covered_and_gives_the_reason() {
    let up = upstream_label_withdrawn();
    let dir = TempDir::new("r07-wd-label");
    let srv = server(&dir, &up);
    let v = get(&srv, &format!("{NAMED_CLASS}[cable].rate({DAY})"));
    assert_says_not_covered_with(&v, REASON);
    let env = envelope_of(&v);
    assert_eq!((s(env, "outcome"), s(env, "reason")), ("not_covered".to_string(), REASON.to_string()), "{}", jcs(&v));
    assert!(env.get("receipt").is_none(), "no cell of the request was resolved, so no receipt: {}", jcs(&v));
    // as of before the withdrawal the label still resolves
    let before = get(&srv, &format!("{NAMED_CLASS}[cable].rate({DAY})?asof={ASOF}"));
    assert_eq!(s(envelope_of(&before), "outcome"), "ok", "{}", jcs(&before));
}

fn home(up: &Upstream) -> TempDir {
    let dir = TempDir::new("r07-wd-cli");
    common::in_dir_050(dir.path()).unwrap();
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("\"peers\": {}", &format!("\"peers\": {{\"{HOST}\": \"{}\"}}", up.origin)))
        .unwrap();
    dir
}

fn run(dir: &TempDir, args: &[&str]) -> (i32, String, String) {
    let mut all = vec!["--home".to_string(), dir.path().to_string_lossy().into_owned()];
    all.extend(args.iter().map(|a| a.to_string()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&all, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

/// The command: exit 3, the envelope on stdout, and a status line on stderr that says not covered and gives the reason.
#[test]
fn cli_get_of_a_withdrawn_cell_says_it_is_not_covered_and_gives_the_reason() {
    let up = upstream_usd_withdrawn();
    let dir = home(&up);
    let (code, out, err) = run(&dir, &["get", USD_FULL]);
    assert_eq!(code, 3, "{err}");
    let env = json::parse(out.trim_end().as_bytes()).unwrap();
    assert_eq!((s(&env, "outcome"), s(&env, "reason")), ("not_covered".to_string(), REASON.to_string()), "{out}");
    assert!(says_not_covered(&err), "the status line says not covered: {err}");
    assert!(err.contains(REASON), "and gives the reason: {err}");
}

#[test]
fn cli_get_through_a_withdrawn_label_says_it_is_not_covered_and_gives_the_reason() {
    let up = upstream_label_withdrawn();
    let dir = home(&up);
    let (code, out, err) = run(&dir, &["get", &format!("{NAMED_CLASS}[cable].rate({DAY})")]);
    assert_eq!(code, 3, "{err}");
    let env = json::parse(out.trim_end().as_bytes()).unwrap();
    assert_eq!(s(&env, "reason"), REASON, "{out}");
    assert!(env.get("receipt").is_none(), "{out}");
    assert!(says_not_covered(&err) && err.contains(REASON), "{err}");
}

/// Local's own resolver fetches a withdrawn foreign cell: the `not_covered` envelope with a withdrawal as `receipt` is accepted
/// (core 0.3.13's `check_answer`), not refused as `E_ENVELOPE`, and says where it came from.
#[test]
fn the_resolver_accepts_a_not_covered_envelope_with_a_withdrawal_receipt() {
    let up = upstream_usd_withdrawn();
    let dir = TempDir::new("r07-wd-resolver");
    let r = resolver(&dir, &up);
    let reply = r.get(USD_FULL).unwrap_or_else(|e| panic!("a reply, not an error: {e}"));
    assert_eq!(reply.status, 404, "{}", String::from_utf8_lossy(&reply.body));
    let env = reply.envelope().expect("an envelope");
    assert_eq!(s(&env, "outcome"), "not_covered", "{}", jcs(&env));
    assert_eq!(s(&env, "reason"), REASON, "{}", jcs(&env));
    assert_eq!(s(env.get("receipt").expect("the withdrawal"), "withdrawn"), REASON, "{}", jcs(&env));
    assert!(env.get("error").is_none(), "no E_ENVELOPE or other rejection: {}", jcs(&env));
    assert!(matches!(reply.source, Source::Fetched), "{:?}", reply.source);
}

/// The withdrawal receipt verifies through local's own verification, at level `signed` (10.1; no value to compare).
#[test]
fn the_resolver_verifies_a_withdrawal_receipt() {
    let up = upstream_usd_withdrawn();
    let dir = TempDir::new("r07-wd-verify");
    let r = resolver(&dir, &up);
    let reply = r.get(USD_FULL).unwrap();
    let env = reply.envelope().unwrap();
    let receipt = env.get("receipt").unwrap_or_else(|| panic!("a withdrawal receipt: {}", jcs(&env)));
    let v = r.verify(&receipt.to_jcs());
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", jcs(&v));
    assert_eq!(s(&v, "level"), "signed", "{}", jcs(&v));
    // the MCP verify tool says the same
    let srv = server(&dir, &up);
    let t = tool(&srv, "verify", &format!(r#"{{"receipt":{}}}"#, jstr(&jcs(receipt))));
    assert_eq!(structured(&t).get("ok"), Some(&Value::Bool(true)), "{}", jcs(&t));
}
