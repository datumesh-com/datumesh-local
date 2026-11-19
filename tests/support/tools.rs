//! Helpers shared by the Build 2 Part 3/4 tests (`define_tests.rs`, `publish_public.rs`): the MCP
//! calls, the key file, and two functions of the local namespace over the test mesh's cells.
//!
//! * `local/rate_x(date)`: `fx_rate(EUR, USD, date) * 2`. Derived cases: the input `fx_rate` absent.
//!   No branches.
//! * `local/ratio(date, n)`: `if n > 0 then fx_rate(EUR, USD, date) / n else fx_rate(...) * 0`.
//!   Derived cases: `n` as a divisor (zero), `n` a number (negative), the input absent. One `if`:
//!   two branch arms.

#![allow(dead_code)]

use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Transport};
use datumesh_local::Resolver;

use crate::common::*;

pub const M: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

pub const NS: &str = "mesh-test.datumesh.com/scratch_p1";

pub const RATE_X_SIG: &str = r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": {"type": "number", "unit": {"of": "USD", "per": "EUR"}}}"#;
pub const RATE_X_SRC: &str = r#"mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", quote: "USD", date: date) * 2"#;
pub const RATIO_SIG: &str = r#"{"args": {"date": {"type": "date", "valid": true}, "n": {"type": "number", "valid": false, "unit": "1"}}, "order": ["date", "n"], "returns": {"type": "number", "unit": {"of": "USD", "per": "EUR"}}}"#;
pub const RATIO_SRC: &str = r#"if n > 0 then mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", quote: "USD", date: date) / n else mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", quote: "USD", date: date) * 0"#;

pub fn q(s: &str) -> String {
    String::from_utf8(Value::String(s.to_string()).to_jcs()).unwrap()
}

pub fn jcs(v: &Value) -> String {
    String::from_utf8(v.to_jcs()).unwrap()
}

pub fn rpc_on(srv: &McpServer, transport: Transport, method: &str, extra: &str) -> Value {
    let comma = if extra.is_empty() { "" } else { "," };
    let line = format!(r#"{{"jsonrpc":"2.0","id":1,"method":{},"params":{{{M}{comma}{extra}}}}}"#, q(method));
    json::parse(srv.handle(&line, transport).expect("a response").as_bytes()).unwrap()
}

pub fn tool(srv: &McpServer, name: &str, args: &str) -> Value {
    rpc_on(srv, Transport::Stdio, "tools/call", &format!(r#""name":{},"arguments":{args}"#, q(name)))
}

pub fn result(v: &Value) -> &Value {
    v.get("result").unwrap_or_else(|| panic!("a result in {}", jcs(v)))
}

pub fn is_error(v: &Value) -> bool {
    result(v).get("isError") == Some(&Value::Bool(true))
}

pub fn structured(v: &Value) -> &Value {
    result(v).get("structuredContent").expect("structuredContent")
}

pub fn items(v: &Value) -> Vec<String> {
    match result(v).get("content") {
        Some(Value::Array(a)) => a.iter().map(|i| s(i, "text")).collect(),
        other => panic!("no content: {other:?}"),
    }
}

pub fn everything(v: &Value) -> String {
    format!("{}\n{}", items(v).join("\n"), jcs(structured(v)))
}

pub fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        other => panic!("{k}: {other:?} in {}", jcs(v)),
    }
}

pub fn sig(text: &str) -> Value {
    json::parse(text.as_bytes()).unwrap()
}

/// The value (lexical) of the test mesh's EUR/USD rate of 2026-10-30.
pub fn usd_value(r: &Resolver) -> String {
    let a = r.get(USD).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    s(&receipt(&a.body), "value")
}

/// `d` doubled, in decimal text (no rounding: the test data has a few places).
pub fn doubled(d: &str) -> String {
    let (int, frac) = d.split_once('.').unwrap_or((d, ""));
    let n: i128 = format!("{int}{frac}").parse().unwrap();
    let t = (n * 2).to_string();
    let t = format!("{:0>width$}", t, width = frac.len() + 1);
    let at = t.len() - frac.len();
    if frac.is_empty() {
        t
    } else {
        format!("{}.{}", &t[..at], &t[at..])
    }
}

/// The tests of `local/ratio` that cover everything: the then arm (n = 1), the else arm with the
/// zero divisor, a negative n, and the absent input. `v` is the USD rate's text.
pub fn ratio_tests(v: &str) -> String {
    format!(
        r#"[{{"call":"local/ratio(date: 2026-10-30, n: 1)","expect":{}}},{{"call":"local/ratio(date: 2026-10-30, n: 0)","expect":"0"}},{{"call":"local/ratio(date: 2026-10-30, n: -1)","expect":"0"}},{{"call":"local/ratio(date: 2026-01-01, n: 1)","expect":{{"fault":"E_FORMULA_ABSENT"}}}}]"#,
        q(v)
    )
}

/// The tests of `local/rate_x` that cover everything: a value and the absent input.
pub fn rate_x_tests(v: &str) -> String {
    format!(
        r#"[{{"call":"local/rate_x(date: 2026-10-30)","expect":{}}},{{"call":"local/rate_x(date: 2026-01-01)","expect":{{"fault":"E_FORMULA_ABSENT"}}}}]"#,
        q(&doubled(v))
    )
}

pub fn define_formula(srv: &McpServer, name: &str, src: &str, sig_text: &str, tests: Option<&str>) -> Value {
    let t = tests.map(|t| format!(r#","tests":{t}"#)).unwrap_or_default();
    tool(srv, "define", &format!(r#"{{"name":{},"formula":{},"signature":{sig_text}{t}}}"#, q(name), q(src)))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The namespace host's key file (mesh-test's seed) at `path`, mode 0600.
#[cfg(unix)]
pub fn write_key(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("{}\n", hex(&[42u8; 32]))).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
