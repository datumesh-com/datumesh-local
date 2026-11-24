//! datumesh-local 0.5.2, item 33: local `calc` takes its result type from the compiled body. A text,
//! date, instant or boolean result is answered with its type and value, not refused with
//! `E_FORMULA_RETURN_TYPE`; a number is answered as before (with its unit).
//!
//! The inputs are stored cells of the local namespace (`datumesh publish <file.jsonl>`, local only),
//! so no mesh is asked: every calculation also runs `--offline`.

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;

use common::*;
use datumesh_core::json::{self, Value};
use prod_mesh::cli;

/// Now, as a canonical instant (a cell's `tx` must be within its function's binding, which starts when
/// the function line is published).
fn now() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let t = d.as_secs() % 86400;
    format!("{}T{:02}:{:02}:{:02}.{:06}Z", today(), t / 3600, t / 60 % 60, t % 60, d.subsec_micros())
}

/// The function line and the cell line (`k: "a"`, at `tx`) of a local stored function.
fn lines(function: &str, ty: &str, value: &str, tx: &str) -> (String, String) {
    // A number's return is an object with its unit; any other type is written as its name (SPEC 7.1).
    let returns =
        if ty == "number" { r#"{"type":"number","unit":"percent"}"#.to_string() } else { format!(r#""{ty}""#) };
    // A boolean's value is JSON true or false; every other value is its lexical string.
    let value = if ty == "boolean" { value.to_string() } else { format!(r#""{value}""#) };
    (
        format!(
            r#"{{"function":"{function}","signature":{{"args":{{"k":{{"type":"string","valid":false}}}},"order":["k"],"returns":{returns}}}}}"#
        ),
        format!(
            r#"{{"cell":{{"fn":"{function}","args":{{"k":{{"type":"string","value":"a"}}}}}},"value":{{"type":"{ty}","value":{value}}},"tx":"{tx}","asof":"{tx}","licence":"CC0-1.0","summary":"A {ty}."}}"#
        ),
    )
}

/// A fresh home with five local stored functions, one cell each (`k: "a"`).
fn home() -> TempDir {
    let dir = TempDir::new("l052-calc-types");
    datumesh_local::Config::in_dir(dir.path()).unwrap();
    let specs = [
        ("local/label", "string", "Bank Rate"),
        ("local/day", "date", "2026-11-05"),
        ("local/moment", "instant", "2026-11-05T12:00:00.000000Z"),
        ("local/flag", "boolean", "true"),
        ("local/level", "number", "3.75"),
    ];
    // The functions first; then their cells, at an instant inside the bindings.
    let functions: Vec<String> = specs.iter().map(|(f, t, v)| lines(f, t, v, "").0).collect();
    let f = jsonl(&dir, "functions.jsonl", &functions.iter().map(String::as_str).collect::<Vec<_>>());
    let (code, out, err) = cli(&dir, &["publish", f.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}\n{err}");
    std::thread::sleep(std::time::Duration::from_millis(5));
    let tx = now();
    let cells: Vec<String> = specs.iter().map(|(f, t, v)| lines(f, t, v, &tx).1).collect();
    let f = jsonl(&dir, "cells.jsonl", &cells.iter().map(String::as_str).collect::<Vec<_>>());
    let (code, out, err) = cli(&dir, &["publish", f.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}\n{err}");
    dir
}

/// `calc <formula>` here and offline: (type, value); the two answers agree.
fn calc(dir: &TempDir, formula: &str) -> (String, String) {
    let mut seen = Vec::new();
    for offline in [false, true] {
        let mut args = vec!["calc", formula];
        if offline {
            args.insert(0, "--offline");
        }
        let (code, out, err) = cli(dir, &args);
        assert_eq!(code, 0, "{formula} (offline {offline}): {err}");
        assert!(!err.contains("E_FORMULA_RETURN_TYPE"), "{err}");
        let v = json::parse(out.trim_end().as_bytes()).unwrap_or_else(|e| panic!("{out}: {e:?}"));
        assert_eq!(s(&v, "kind"), "calculation", "{out}");
        seen.push((s(&v, "type"), s(&v, "value"), v.get("unit").cloned()));
    }
    assert_eq!(seen[0], seen[1]);
    let (ty, value, unit) = seen.remove(0);
    if ty != "number" {
        assert_eq!(unit, None, "only a number has a unit");
    }
    (ty, value)
}

#[test]
fn a_text_result_is_answered_with_its_type_and_value() {
    let dir = home();
    assert_eq!(calc(&dir, r#"local/label(k: "a")"#), ("string".into(), "Bank Rate".into()));
    let derived = r#"if local/flag(k: "a") then local/label(k: "a") else "none""#;
    assert_eq!(calc(&dir, derived), ("string".into(), "Bank Rate".into()));
}

#[test]
fn a_date_result_is_answered_with_its_type_and_value() {
    let dir = home();
    assert_eq!(calc(&dir, r#"local/day(k: "a")"#), ("date".into(), "2026-11-05".into()));
}

#[test]
fn an_instant_result_is_answered_with_its_type_and_value() {
    let dir = home();
    assert_eq!(calc(&dir, r#"local/moment(k: "a")"#), ("instant".into(), "2026-11-05T12:00:00.000000Z".into()));
}

#[test]
fn a_boolean_result_is_answered_with_its_type_and_value() {
    let dir = home();
    assert_eq!(calc(&dir, r#"local/flag(k: "a")"#), ("boolean".into(), "true".into()));
    assert_eq!(calc(&dir, r#"local/level(k: "a") > 3 percent"#), ("boolean".into(), "true".into()));
}

#[test]
fn a_number_is_answered_as_before_with_its_unit() {
    let dir = home();
    let (code, out, err) = cli(&dir, &["calc", r#"local/level(k: "a") * 2"#]);
    assert_eq!(code, 0, "{err}");
    let v: Value = json::parse(out.trim_end().as_bytes()).unwrap();
    assert_eq!(s(&v, "type"), "number");
    assert_eq!(s(&v, "value"), "7.5");
    assert_eq!(s(&v, "unit"), "percent", "{out}");
}
