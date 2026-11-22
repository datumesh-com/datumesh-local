//! Helpers for the 0.7 round's local tests (CONTRACT-round-0.7.md R1, R2): later publish runs on the test mesh (a withdrawal
//! line, a close line), and the MCP tools in process. The lines are the http 0.5.0 forms: `{"cell": ..., "withdrawn":
//! "<reason>"}` and `{"close": "<full name>"}`, signed by the test mesh's key under a clock of the run's own.

#![allow(dead_code)]

use datumesh_core::json::{self, Value};
use datumesh_http::publish::{publish_stored_with, PublishOptions};
use datumesh_local::mcp::{McpServer, Transport};

use crate::common::*;

/// The clock of the run that withdraws or closes: after every seeded binding and cell, before today.
pub const T_WITHDRAW: &str = "2026-11-10T12:00:00.000000Z";
pub const REASON: &str = "withdrawn by the publisher: the rate is under review";

pub const M: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

/// Publishes `lines` (the `publish-stored` format) to `up` as the test mesh at the mesh clock `now`, and tells the mesh.
/// A line the mesh does not know (a withdrawal or a close before http 0.5.0 has them) panics here, with the mesh's refusal.
pub fn publish_at(up: &Upstream, now: &str, lines: &[String]) {
    let dir = TempDir::new("r07-publish");
    let path = dir.path().join("run.jsonl");
    std::fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    let options = PublishOptions { clock: ManualClock::new(now), backfill: None };
    publish_stored_with(&*up.store, &upstream_key(), HOST, &[HOST.to_string()], &path, &options)
        .unwrap_or_else(|e| panic!("the run at {now} was refused: {e:?}"));
    up.mesh.clear_memo();
}

/// A withdrawal line for the stored cell `function(args)`; `args` is the JSON object of typed values.
pub fn withdrawal_line(function: &str, args: &str, reason: &str) -> String {
    format!(
        r#"{{"cell":{{"fn":{},"args":{args}}},"licence":"CC0-1.0","summary":"Synthetic test data, withdrawn.","withdrawn":{}}}"#,
        jstr(function),
        jstr(reason)
    )
}

pub fn close_line(name: &str) -> String {
    format!(r#"{{"close":{}}}"#, jstr(name))
}

pub fn jstr(s: &str) -> String {
    String::from_utf8(Value::String(s.to_string()).to_jcs()).unwrap()
}

/// The fx cell of `USD` (`ecb/fx_rate`, EUR to USD on 2026-10-30), as a withdrawal line.
pub fn usd_withdrawal(reason: &str) -> String {
    withdrawal_line(
        "mesh-test.datumesh.com/ecb/fx_rate",
        r#"{"base":{"type":"string","value":"EUR"},"date":{"type":"date","value":"2026-10-30"},"quote":{"type":"string","value":"USD"}}"#,
        reason,
    )
}

/// The upstream with the USD cell withdrawn at `T_WITHDRAW`.
pub fn upstream_usd_withdrawn() -> Upstream {
    let up = upstream();
    publish_at(&up, T_WITHDRAW, &[usd_withdrawal(REASON)]);
    up
}

/// The classes world with the lookup cell of the label `cable` of `fx_named` withdrawn at `T_WITHDRAW`.
pub fn upstream_label_withdrawn() -> Upstream {
    let up = upstream_classes();
    publish_at(
        &up,
        T_WITHDRAW,
        &[withdrawal_line(&format!("{NAMED_CLASS}/named"), r#"{"label":{"type":"string","value":"cable"}}"#, REASON)],
    );
    up
}

pub fn jcs(v: &Value) -> String {
    String::from_utf8(v.to_jcs()).unwrap()
}

pub fn tool(srv: &McpServer, name: &str, args: &str) -> Value {
    let line = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{M},"name":{},"arguments":{args}}}}}"#,
        jstr(name)
    );
    json::parse(srv.handle(&line, Transport::Stdio).expect("a response").as_bytes()).unwrap()
}

pub fn result(v: &Value) -> &Value {
    v.get("result").unwrap_or_else(|| panic!("a result in {}", jcs(v)))
}

pub fn structured(v: &Value) -> &Value {
    result(v).get("structuredContent").unwrap_or_else(|| panic!("structuredContent in {}", jcs(v)))
}

/// The text items of a tool result.
pub fn items(v: &Value) -> Vec<String> {
    match result(v).get("content") {
        Some(Value::Array(a)) => a.iter().map(|i| s(i, "text")).collect(),
        other => panic!("no content: {other:?}"),
    }
}

/// Whether `text`, lower-cased, says "not covered" (either spelling: the words or the outcome's name).
pub fn says_not_covered(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("not covered") || t.contains("not_covered")
}
