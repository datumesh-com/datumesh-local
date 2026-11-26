//! datumesh-local 0.5.3, before its release (REPORT-c-codes-and-small-profile.md, "Two defects", 1): a failed answer
//! says its code in the text. Written before the fix.
//!
//! Since A2 (CONTRACT-guard-patch.md: the MCP text part is the short answer; the structured part is the envelope), a
//! failed `get` answers only "status 404: no receipt to answer with.", a failed `explain` only "the request: 0 inputs,
//! page 1 of 1", and a failed `describe` only the echoed id, which reads like success; none of them sets `isError`. The
//! code and the detail survive only in the structured part. On 0.5.2 (tag v0.5.2, checked by running these requests
//! there) the text of each was the JCS of the whole answer block, so it carried the envelope's `error`, `outcome`,
//! `detail` and `reason` (and describe's `error` and `reason`), though `isError` was not set either.
//!
//! What these tests ask, for every tool of both profiles (full: all twelve; small: find, get, calc, interval):
//!
//! * A failed answer has `isError: true`. A `get`, `explain` or `describe` failed when the mesh's (or the resolver's)
//!   status is 400 or more; the other tools fail through their error path, which already sets it.
//! * Its text (the content items, together) carries the code and every detail the structured part has: each string
//!   value of a member named `error`, `fault`, `detail`, `reason` or `outcome` anywhere in the structured content
//!   (the envelope's included), but the bare words `error` and `ok`; and for `get`, the detail the resolver gave the
//!   reply (`Reply::detail`, the CLI's status line, e.g. "offline: the resolver is offline"). That is at least what
//!   0.5.2's text carried.
//! * The text is still not the structured content's JCS (A2), and the structured content keeps the envelope and the
//!   status as now.
//! * A successful answer stays short, as A2 says (under 1 KiB here, no envelope in the text).
//!
//! `verify` of a receipt that does not verify is an answer (`ok: false`), not a failure: its text names the code
//! (already so; asserted so that it stays).
//!
//! The upstream is the test mesh of `tests/common` with the classes world. Nothing reaches the network.

#![cfg(unix)]

mod common;
#[path = "support/tools.rs"]
mod tools;

use std::sync::Arc;

use common::*;
use datumesh_core::json::Value;
use datumesh_local::mcp::{McpServer, Profile};
use datumesh_local::Resolver;
use tools::{is_error, items, jcs, q, structured, tool};

const SHORT: usize = 1024;

const UNBOUND: &str = "mesh-test.datumesh.com/ecb/nosuch(date: 2026-10-30)";
/// A failed `get`: the request, and the code its text must carry.
const GET_FAILURES: [(&str, &str); 7] = [
    (UNBOUND, "E_NAME_UNBOUND"),
    (r#"ecb/fx_rate(base: "EUR", quote: "USD", date: "x")"#, "E_CANON_TYPE"),
    ("cell://nonsense", "E_CANON_NAME"),
    (r#"ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30, extra: 1)"#, "E_ARG_UNKNOWN"),
    ("mesh-test.datumesh.com/ecb/fx_pair(base:EUR).rate(2026-03-02)", "E_ARG_MISSING"),
    ("ecb/fx_rate?base=EUR&quote=USD&date=2026-10-30", "E_PARSE_URI"),
    // No cell for the day: not covered, with no error member: the outcome is the code.
    (r#"ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-01-01)"#, "not_covered"),
];
/// A cell not held, read offline: the resolver's own refusal.
const NOT_HELD: &str = r#"ecb/fx_rate(base: "EUR", quote: "GBP", date: 2026-10-30)"#;

struct W {
    _up: Upstream,
    _dir: TempDir,
    r: Arc<Resolver>,
}

fn world() -> W {
    let up = upstream_classes();
    let dir = TempDir::new("l053-failed");
    let r = Arc::new(resolver(&dir, &up));
    W { _up: up, _dir: dir, r }
}

fn server(w: &W, profile: Profile) -> McpServer {
    McpServer::with_profile(w.r.clone(), profile)
}

fn call(srv: &McpServer, name: &str, key: &str, arg: &str) -> Value {
    tool(srv, name, &format!(r#"{{{}:{}}}"#, q(key), q(arg)))
}

/// Every string value of a member named `error`, `fault`, `detail`, `reason` or `outcome`, anywhere in `v`.
fn details(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if let (true, Value::String(t)) =
                    (matches!(k.as_str(), "error" | "fault" | "detail" | "reason" | "outcome"), x)
                {
                    if t != "error" && t != "ok" && !out.contains(t) {
                        out.push(t.clone());
                    }
                }
                details(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| details(x, out)),
        _ => {}
    }
}

/// A failed answer: `isError`, and its text carries `code`, every detail of the structured content, and `extra`.
fn assert_failure_says_it(what: &str, v: &Value, code: &str, extra: &[String]) {
    let text = items(v).join("\n");
    assert!(
        text.contains(code),
        "{what}: the text carries the code {code}: {text:?}\nstructured: {}",
        jcs(structured(v))
    );
    let mut want = Vec::new();
    details(structured(v), &mut want);
    want.extend(extra.iter().cloned());
    for d in want {
        assert!(text.contains(&d), "{what}: the text carries {d:?}: {text:?}\nstructured: {}", jcs(structured(v)));
    }
    assert!(is_error(v), "{what}: a failed answer has isError: true: {}", jcs(v));
}

/// A2 still holds for a failed answer built from the mesh's reply: the text is not the structured content's JCS, and
/// the structured content keeps the envelope's status.
fn assert_not_the_block(what: &str, v: &Value) {
    let st = jcs(structured(v));
    assert!(items(v).iter().all(|t| t.trim() != st), "{what}: the text is not the structured content's JCS: {st}");
    assert!(structured(v).get("status").is_some(), "{what}: the structured content keeps the status: {st}");
}

// ------------------------------------------------------------------------------- get

#[test]
fn a_failed_get_says_its_code_in_both_profiles() {
    let w = world();
    for profile in [Profile::Full, Profile::Small] {
        let srv = server(&w, profile);
        for (request, code) in GET_FAILURES {
            let what = format!("{profile:?} get {request}");
            let v = call(&srv, "get", "request", request);
            assert_failure_says_it(&what, &v, code, &[]);
        }
    }
}

#[test]
fn a_failed_get_keeps_the_envelope_in_the_structured_part() {
    let w = world();
    for profile in [Profile::Full, Profile::Small] {
        let srv = server(&w, profile);
        let v = call(&srv, "get", "request", UNBOUND);
        assert_not_the_block(&format!("{profile:?} get {UNBOUND}"), &v);
        let env = structured(&v).get("envelope").unwrap_or_else(|| panic!("the envelope: {}", jcs(&v)));
        assert_eq!(s(env, "error"), "E_NAME_UNBOUND", "{}", jcs(&v));
        assert_eq!(structured(&v).get("status").and_then(Value::as_number_text), Some("404"), "{}", jcs(&v));
    }
}

#[test]
fn a_get_refused_by_the_resolver_says_its_code_and_why() {
    let w = world();
    w.r.set_offline(true);
    let reply = w.r.get(NOT_HELD).unwrap();
    assert!(reply.status >= 400, "{}", String::from_utf8_lossy(&reply.body));
    let why: Vec<String> = reply.detail.iter().cloned().collect();
    assert!(!why.is_empty(), "the resolver says why: {reply:?}");
    for profile in [Profile::Full, Profile::Small] {
        let v = call(&server(&w, profile), "get", "request", NOT_HELD);
        assert_failure_says_it(&format!("{profile:?} offline get"), &v, "E_FOREIGN_UNAVAILABLE", &why);
        assert_not_the_block(&format!("{profile:?} offline get"), &v);
    }
}

#[test]
fn a_successful_get_stays_short() {
    let w = world();
    for profile in [Profile::Full, Profile::Small] {
        let v = call(&server(&w, profile), "get", "request", USD);
        assert!(!is_error(&v), "{}", jcs(&v));
        let text = items(&v).join("\n");
        assert!(text.len() < SHORT, "{profile:?}: the short answer: {} bytes: {text}", text.len());
        assert!(!text.contains("envelope") && !text.trim_start().starts_with('{'), "{profile:?}: {text}");
        assert!(structured(&v).get("envelope").is_some(), "{}", jcs(&v));
    }
}

// ------------------------------------------------------------------------------- explain and describe (full)

#[test]
fn a_failed_explain_says_its_code() {
    let w = world();
    let srv = server(&w, Profile::Full);
    for (request, code) in [
        (UNBOUND, "E_NAME_UNBOUND"),
        ("cell://nonsense", "E_CANON_NAME"),
        (r#"ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-01-01)"#, "not_covered"),
    ] {
        let what = format!("explain {request}");
        let v = call(&srv, "explain", "request", request);
        assert_failure_says_it(&what, &v, code, &[]);
        assert_not_the_block(&what, &v);
    }
}

#[test]
fn a_failed_describe_says_its_code_and_does_not_read_like_success() {
    let w = world();
    let srv = server(&w, Profile::Full);
    let hex = "f".repeat(64);
    for (id, code) in [
        ("mesh-test.datumesh.com/ecb/nothing", "E_NAME_UNBOUND"),
        ("mesh-test.datumesh.com/ecb/fx_pair(base:EUR)", "E_KEY_ENCODING"),
        // The mesh's 404 with its words (no spec code): the words are the detail.
        ("nonsense", "no such class or object"),
        (hex.as_str(), "no such object"),
    ] {
        let what = format!("describe {id}");
        let v = call(&srv, "describe", "id", id);
        assert_failure_says_it(&what, &v, code, &[]);
        assert_not_the_block(&what, &v);
        assert!(
            items(&v).iter().all(|t| t.trim() != id && t.trim() != format!("{id}.")),
            "{what}: no text item is the echoed id alone: {:?}",
            items(&v)
        );
    }
}

#[test]
fn a_successful_describe_is_not_an_error() {
    let w = world();
    let v = call(&server(&w, Profile::Full), "describe", "id", PAIR_CLASS);
    assert!(!is_error(&v), "{}", jcs(&v));
}

// ------------------------------------------------------------------------------- the other tools

#[test]
fn the_other_tools_failures_say_their_code_in_both_profiles() {
    // These fail through the tools' error path (already `isError` with the code): asserted so that they stay so.
    let w = world();
    for profile in [Profile::Full, Profile::Small] {
        let srv = server(&w, profile);
        for (formula, code) in [
            ("mesh-test.datumesh.com/ecb/nosuch(date: 2026-10-30) * 2", "E_FORMULA_UNRESOLVED"),
            ("1 +", "E_FORMULA_SYNTAX"),
            ("1 / 0", "E_FORMULA_DIV_ZERO"),
        ] {
            let v = call(&srv, "calc", "formula", formula);
            assert_failure_says_it(&format!("{profile:?} calc {formula}"), &v, code, &[]);
        }
        let v = tool(
            &srv,
            "interval",
            r#"{"function":"mesh-test.datumesh.com/ecb/nosuch","from":"2026-01-01","to":"2026-02-01"}"#,
        );
        assert_failure_says_it(&format!("{profile:?} interval"), &v, "not_covered", &[]);
        let v = tool(&srv, "find", r#"{"query":5}"#);
        assert_failure_says_it(&format!("{profile:?} find"), &v, "query", &[]);
    }
    let srv = server(&w, Profile::Full);
    let failures = [
        ("verify", format!(r#"{{"call":{}}}"#, q(UNBOUND)), "no receipt to verify"),
        ("whatif", format!(r#"{{"request":{},"overrides":[]}}"#, q(UNBOUND)), "overrides"),
        (
            "define",
            r#"{"name":"local/x","formula":"1 +","signature":{"args":{},"order":[],"returns":{"type":"number","unit":"1"}}}"#.to_string(),
            "E_FORMULA_SYNTAX",
        ),
        ("publish", r#"{"lines":[{"cell":5}]}"#.to_string(), "a cell without fn"),
        (
            "publish_public",
            r#"{"to":"http://127.0.0.1:1","namespace":"x.example","key":"/nonexistent","names":["local/x"]}"#.to_string(),
            "not a namespace",
        ),
        ("subscribe", r#"{"request":"x"}"#.to_string(), "not_implemented"),
    ];
    for (name, args, code) in failures {
        let v = tool(&srv, name, &args);
        assert_failure_says_it(name, &v, code, &[]);
    }
}

#[test]
fn verify_of_a_receipt_that_does_not_verify_names_the_code() {
    let w = world();
    let v = tool(&server(&w, Profile::Full), "verify", r#"{"receipt":"garbage"}"#);
    let text = items(&v).join("\n");
    assert_eq!(structured(&v).get("ok"), Some(&Value::Bool(false)), "{}", jcs(&v));
    assert!(text.contains("E_PARSE_JSON") && text.contains("not verified"), "{text}");
}
