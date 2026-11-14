//! WASM `parse_call` with classes (`CONTRACT-local-0.5.0.md` item 6): the environment JSON accepts
//! `classes`, `now`, `lookups` and `aliases_tx` (as the spec's `call` vectors' environment does), and
//! the result carries `resolved`. Run with `--features wasm` (the functions are plain functions off
//! wasm32); the wasm-smoke run is separate and stays green.
#![cfg(feature = "wasm")]

use datumesh_core::json::{self, Value};
use datumesh_core::FunctionDescriptor;
use datumesh_local::wasm::parse_call;

const RATE: &str = r#"{"code":{"body":"","lang":"stored"},"inputs":[],"signature":{"args":{"base":{"type":"string","valid":false},"date":{"default":"asof","type":"date","valid":true},"quote":{"type":"string","valid":false}},"order":["base","quote","date"],"returns":{"type":"number","unit":"1"}}}"#;
const NAMED: &str = r#"{"code":{"body":"","lang":"stored"},"inputs":[],"signature":{"args":{"label":{"type":"string","valid":false}},"order":["label"],"returns":{"classes":["rates.example.org/fx_pair"],"type":"string"}}}"#;

fn hash(d: &str) -> String {
    FunctionDescriptor::from_json(d.as_bytes()).unwrap().hash()
}

/// The environment: `rates.example.org/fx_pair` (keys base, quote; methods `rate` with a defaulted
/// date, and the lookup `named`), the alias `fx` for the method function, `aliases_tx`, `now`, and
/// the lookup table of the class.
fn env() -> String {
    format!(
        r#"{{"aliases":{{"fx":"rates.example.org/fx_pair/rate"}},"aliases_tx":"2026-03-01T00:00:00.000000Z","classes":{{"rates.example.org/fx_pair":{{"keys":{{"base":{{"key_normalisation":"none","type":"string"}},"quote":{{"key_normalisation":"none","type":"string"}}}},"lookup":"named","methods":{{"named":"{}","rate":"{}"}},"order":["base","quote"],"steward":{{"namespace":"rates.example.org","process":"https://rates.example.org/classes/change"}}}}}},"functions":{{"rates.example.org/fx_pair/named":{NAMED},"rates.example.org/fx_pair/rate":{RATE}}},"lookups":{{"rates.example.org/fx_pair":{{"cable":"rates.example.org/fx_pair(base:\"GBP\",quote:\"USD\")"}}}},"now":"2026-03-16T18:00:00.000000Z"}}"#,
        hash(NAMED),
        hash(RATE)
    )
}

fn out(text: &str, env: &str) -> Value {
    json::parse(parse_call(text, env).as_bytes()).unwrap()
}

fn uri(v: &Value) -> &str {
    v.get("uri").and_then(Value::as_str).unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&v.to_jcs())))
}

#[test]
fn a_method_call_is_the_plain_reads_cell_and_resolved_says_so() {
    let env = env();
    let plain = out(r#"rates.example.org/fx_pair/rate("GBP", "USD", 2026-03-16)"#, &env);
    assert_eq!(plain.get("ok"), Some(&Value::Bool(true)), "{}", String::from_utf8_lossy(&plain.to_jcs()));
    assert!(plain.get("resolved").is_none(), "nothing resolved");
    let m = out("rates.example.org/fx_pair(GBP, USD).rate(2026-03-16)", &env);
    assert_eq!(uri(&m), uri(&plain));
    assert_eq!(m.get("reference"), plain.get("reference"));
    let resolved = m.get("resolved").expect("resolved");
    assert_eq!(
        resolved.get("call").and_then(Value::as_str),
        Some(r#"rates.example.org/fx_pair/rate(base: "GBP", date: 2026-03-16, quote: "USD")"#)
    );
    let Some(Value::Array(rs)) = resolved.get("resolutions") else { panic!("resolutions") };
    assert_eq!(rs.len(), 1);
    assert_eq!(rs[0].get("kind").and_then(Value::as_str), Some("method"));
    assert_eq!(rs[0].get("to").and_then(Value::as_str), Some("rates.example.org/fx_pair/rate"));
}

#[test]
fn a_short_call_takes_now_and_the_alias_snapshot() {
    let env = env();
    // The date defaults to the asof's date; the call has no asof, so now's.
    let m = out("rates.example.org/fx_pair(GBP, USD).rate()", &env);
    assert!(uri(&m).contains("date=2026-03-16"), "{}", uri(&m));
    let resolved = m.get("resolved").unwrap();
    let Some(Value::Array(rs)) = resolved.get("resolutions") else { panic!("resolutions") };
    assert!(rs.iter().any(|r| r.get("kind").and_then(Value::as_str) == Some("default")
        && r.get("from").and_then(Value::as_str) == Some("date")
        && r.get("to").and_then(Value::as_str) == Some("2026-03-16")));
    assert!(resolved.get("aliases_tx").is_none(), "no alias was used");
    // With the call's asof.
    let m = out("rates.example.org/fx_pair(GBP, USD).rate()?asof=2026-03-10T09:00:00.000000Z", &env);
    assert!(uri(&m).contains("date=2026-03-10"), "{}", uri(&m));
    // A bare name through the alias map: the snapshot is named.
    let m = out("fx(GBP, USD, 2026-03-16)", &env);
    assert_eq!(m.get("ok"), Some(&Value::Bool(true)), "{}", String::from_utf8_lossy(&m.to_jcs()));
    let resolved = m.get("resolved").unwrap();
    assert_eq!(resolved.get("aliases_tx").and_then(Value::as_str), Some("2026-03-01T00:00:00.000000Z"));
    let Some(Value::Array(rs)) = resolved.get("resolutions") else { panic!("resolutions") };
    assert_eq!(rs[0].get("kind").and_then(Value::as_str), Some("alias"));
}

#[test]
fn a_label_goes_through_the_lookups_of_the_environment() {
    let env = env();
    let m = out("rates.example.org/fx_pair[cable].rate(2026-03-16)", &env);
    let plain = out(r#"rates.example.org/fx_pair/rate("GBP", "USD", 2026-03-16)"#, &env);
    assert_eq!(uri(&m), uri(&plain));
    let resolved = m.get("resolved").unwrap();
    let Some(Value::Array(rs)) = resolved.get("resolutions") else { panic!("resolutions") };
    assert_eq!(rs[0].get("kind").and_then(Value::as_str), Some("lookup"));
    assert_eq!(rs[0].get("from").and_then(Value::as_str), Some("cable"));
    // No value for the label: E_NAME_UNBOUND.
    let m = out("rates.example.org/fx_pair[nope].rate(2026-03-16)", &env);
    assert_eq!(m.get("ok"), Some(&Value::Bool(false)));
    assert_eq!(m.get("code").and_then(Value::as_str), Some("E_NAME_UNBOUND"));
}

/// The environment of 0.2.0 (no classes, no clock) still parses a plain call, and a method call in
/// it fails as a call of an unknown name, not as a malformed environment.
#[test]
fn the_old_environment_still_works() {
    let env = format!(r#"{{"functions":{{"rates.example.org/fx_pair/rate":{RATE}}}}}"#);
    let v = out(r#"rates.example.org/fx_pair/rate("GBP", "USD", 2026-03-16)"#, &env);
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", String::from_utf8_lossy(&v.to_jcs()));
    let m = out("rates.example.org/fx_pair(GBP, USD).rate(2026-03-16)", &env);
    assert_eq!(m.get("ok"), Some(&Value::Bool(false)));
    assert_ne!(m.get("code").and_then(Value::as_str), Some("E_ARGUMENT"));
}
