//! CONTRACT-build2-part3-calculus E6, `define` with tests (datumesh-local): the tool accepts
//! `tests` with the formula, in one call. Locally tests are optional and never required.
//!
//! Shapes these tests specify (none of it exists yet):
//!
//! * `define` gains `tests`, an array of `{call, expect, asof?, inputs?}` for the function being
//!   defined (its name is implied; a test whose `call` names another function is an argument
//!   error, and then nothing is defined: the call is atomic):
//!   - `call`: a call of the function (the `local/` alias allowed);
//!   - `expect`: a plain literal, typed by the function's return type (a JSON string, number or
//!     boolean), or `{"fault": "E_FORMULA_..."}`;
//!   - `asof`: optional instant; `inputs`: optional `{<call of an input cell>: <plain literal>}`.
//! * The result's structured content gains `tests`: the number of tests kept (0 when none).
//! * The tests are kept with the function (they travel in rooting bundles and publishes: see
//!   `publish_public.rs`). A stored function takes no tests (an argument error).
//! * The CLI: `datumesh define NAME --formula F --signature S [--tests <JSON array>]`.

#![cfg(unix)]

mod common;
#[path = "support/tools.rs"]
mod tools;

use std::sync::Arc;

use common::*;
use datumesh_core::json::Value;
use datumesh_local::mcp::McpServer;
use tools::*;

fn server() -> (Upstream, TempDir, Arc<datumesh_local::Resolver>, McpServer) {
    let up = upstream();
    let dir = TempDir::new("define-tests");
    let r = Arc::new(resolver(&dir, &up));
    let srv = McpServer::new(r.clone());
    (up, dir, r, srv)
}

#[test]
fn a_formula_without_tests_is_defined_locally_as_before() {
    let (_up, _dir, r, srv) = server();
    let v = define_formula(&srv, "local/ratio", RATIO_SRC, RATIO_SIG, None);
    assert!(!is_error(&v), "{}", jcs(&v));
    let sc = structured(&v);
    assert_eq!(s(sc, "name"), "local.invalid/ratio");
    assert_eq!(sc.get("tests").and_then(Value::as_number_text), Some("0"), "{}", jcs(sc));
    // It evaluates: the formula and the signature are fine.
    let a = r.get("local/ratio(date: 2026-10-30, n: 2)").unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
}

#[test]
fn a_formula_and_its_tests_are_defined_in_one_call() {
    let (_up, _dir, r, srv) = server();
    let usd = usd_value(&r);
    let v = define_formula(&srv, "local/ratio", RATIO_SRC, RATIO_SIG, Some(&ratio_tests(&usd)));
    assert!(!is_error(&v), "{}", jcs(&v));
    let sc = structured(&v);
    assert_eq!(s(sc, "name"), "local.invalid/ratio");
    assert_eq!(sc.get("tests").and_then(Value::as_number_text), Some("4"), "{}", jcs(sc));
    assert!(r.store().resolve_binding("local.invalid/ratio", END).unwrap().is_some());
}

#[test]
fn tests_with_inputs_and_asof_and_a_json_number_expectation_are_accepted() {
    let (_up, _dir, _r, srv) = server();
    let tests = format!(
        r#"[{{"call":"local/rate_x(date: 2026-10-30)","asof":"2026-11-02T00:00:00.000000Z","inputs":{{{}:1.5}},"expect":3}}]"#,
        q(r#"mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#)
    );
    let v = define_formula(&srv, "local/rate_x", RATE_X_SRC, RATE_X_SIG, Some(&tests));
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_eq!(structured(&v).get("tests").and_then(Value::as_number_text), Some("1"));
}

#[test]
fn a_test_of_another_function_refuses_the_whole_call() {
    let (_up, _dir, r, srv) = server();
    let tests = r#"[{"call":"local/other(date: 2026-10-30)","expect":"1"}]"#;
    let v = define_formula(&srv, "local/rate_x", RATE_X_SRC, RATE_X_SIG, Some(tests));
    assert!(is_error(&v), "{}", jcs(&v));
    assert!(r.store().resolve_binding("local.invalid/rate_x", END).unwrap().is_none(), "nothing was defined");
}

#[test]
fn a_test_without_an_expectation_is_refused_and_nothing_is_defined() {
    let (_up, _dir, r, srv) = server();
    let tests = r#"[{"call":"local/rate_x(date: 2026-10-30)"}]"#;
    let v = define_formula(&srv, "local/rate_x", RATE_X_SRC, RATE_X_SIG, Some(tests));
    assert!(is_error(&v), "{}", jcs(&v));
    assert!(r.store().resolve_binding("local.invalid/rate_x", END).unwrap().is_none());
}

#[test]
fn a_stored_function_takes_no_tests() {
    let (_up, _dir, r, srv) = server();
    let args = format!(
        r#"{{"name":"local/level","stored":true,"signature":{},"tests":[{{"call":"local/level(day: 2026-11-01)","expect":"1"}}]}}"#,
        r#"{"args": {"day": {"type": "date", "valid": true}}, "order": ["day"], "returns": {"type": "number", "unit": "1"}}"#
    );
    let v = tool(&srv, "define", &args);
    assert!(is_error(&v), "{}", jcs(&v));
    assert!(r.store().resolve_binding("local.invalid/level", END).unwrap().is_none());
}

#[test]
fn the_cli_defines_with_tests() {
    let up = upstream();
    let dir = TempDir::new("define-tests-cli");
    common::in_dir_050(dir.path()).unwrap();
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("\"peers\": {}", &format!("\"peers\": {{\"{HOST}\": \"{}\"}}", up.origin)))
        .unwrap();
    let rdir = TempDir::new("define-tests-cli-r");
    let r = resolver(&rdir, &up);
    let tests = rate_x_tests(&usd_value(&r));
    let args: Vec<String> = [
        "--home",
        &dir.path().to_string_lossy(),
        "define",
        "local/rate_x",
        "--formula",
        RATE_X_SRC,
        "--signature",
        RATE_X_SIG,
        "--tests",
        &tests,
    ]
    .iter()
    .map(|a| a.to_string())
    .collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&args, &mut out, &mut err);
    let (out, err) = (String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap());
    assert_eq!(code, 0, "{err}");
    let v = datumesh_core::json::parse(out.trim_end().as_bytes()).unwrap();
    assert_eq!(s(&v, "name"), "local.invalid/rate_x");
    assert_eq!(v.get("tests").and_then(Value::as_number_text), Some("2"), "{out}");
}
