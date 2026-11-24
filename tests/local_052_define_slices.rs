//! datumesh-local 0.5.2, item 14: `define --tests` takes slice inputs.
//!
//! A case's `inputs` is the bundle's test form (SPEC 6.x `case`, spec-0.7.0 R3): for a slice,
//! `{"<slice call>": {"members": {}}}` (the slice given as empty) or `{"<slice call>": {"members":
//! {"<cell call>": <value>, ...}}}` (its whole member set). At the command line a member's value is
//! a plain literal, typed by the slice's function's return type, as scalar inputs are today; it is
//! kept, and carried, as a typed scalar. Scalar inputs are written as today.
//!
//! The function is Part 2's `bank_rate_change` (PROPOSAL-part2-functions.md), defined locally over
//! the production fake (`tests/support/prod_mesh.rs`). Its empty-slice case is the one the Part 2
//! publish had to add to the bundle by hand (`part2-records/resign_bundle.py`). `define` keeps the
//! tests without running them, so no Bank Rate member is needed here.

#![cfg(unix)]

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::{Config, Resolver};
use prod_mesh::*;
use tools::jcs;

const NS: &str = "mesh-test.datumesh.com/scratch_l14";
const SRC: &str = "last_before(@boe/bank_rate(*), to) - last_before(@boe/bank_rate(*), from)";
const SIG: &str = r#"{"args": {"from": {"type": "date", "valid": true}, "to": {"type": "date", "valid": true}}, "order": ["from", "to"], "returns": {"type": "number", "unit": "percent"}}"#;
const SLICE: &str = "datumesh.com/relay/bankofengland.co.uk/bank_rate(*)";
const DAY4: &str = "datumesh.com/relay/bankofengland.co.uk/bank_rate(date: 2026-11-04)";
const DAY5: &str = "datumesh.com/relay/bankofengland.co.uk/bank_rate(date: 2026-11-05)";

/// The scalar cases, Part 2's empty-slice case, and a members map.
fn tests_json() -> String {
    format!(
        r#"[
  {{"call": "local/bank_rate_change(2026-11-04, 2026-11-05)", "expect": "0.25"}},
  {{"call": "local/bank_rate_change(2026-11-05, 2026-11-20)", "expect": "0"}},
  {{"call": "local/bank_rate_change(2026-11-05, 2026-11-20)", "inputs": {{"{SLICE}": {{"members": {{}}}}}}, "expect": {{"fault": "E_FORMULA_ABSENT"}}}},
  {{"call": "local/bank_rate_change(2026-11-04, 2026-11-05)", "inputs": {{"{SLICE}": {{"members": {{"{DAY4}": "3.75", "{DAY5}": "5"}}}}}}, "expect": "1.25"}}
]"#
    )
}

struct W {
    _prod: ProdMesh,
    _up: Upstream,
    dir: TempDir,
    key: std::path::PathBuf,
    _tmp: TempDir,
}

fn world() -> W {
    let prod = ProdMesh::start(true);
    let up = upstream();
    let dir = fresh_home(&[("datumesh.com", &prod.origin), (HOST, &up.origin)]);
    let tmp = TempDir::new("l14-key");
    let key = tmp.path().join("namespace.key");
    tools::write_key(&key);
    W { _prod: prod, _up: up, dir, key, _tmp: tmp }
}

fn define(dir: &TempDir, tests: &str) -> (i32, String, String) {
    cli(dir, &["define", "local/bank_rate_change", "--formula", SRC, "--signature", SIG, "--tests", tests])
}

fn defined(dir: &TempDir) -> bool {
    let r = Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap();
    r.store().resolve_binding("local.invalid/bank_rate_change", END).unwrap().is_some()
}

fn parse(out: &str) -> Value {
    json::parse(out.trim_end().as_bytes()).unwrap_or_else(|e| panic!("not JSON: {out}: {e:?}"))
}

/// The test objects of a bundle (objects with `call` and `expect`), anywhere in it.
fn bundle_tests(bundle: &Value) -> Vec<Value> {
    let mut found = Vec::new();
    objects_with(bundle, "expect", &mut found);
    found.into_iter().filter(|t| t.get("call").is_some()).cloned().collect()
}

fn typed(ty: &str, v: &str) -> Value {
    json::parse(format!(r#"{{"type":"{ty}","value":"{v}"}}"#).as_bytes()).unwrap()
}

#[test]
fn define_takes_the_empty_slice_case_and_a_members_map() {
    let w = world();
    let (code, out, err) = define(&w.dir, &tests_json());
    assert_eq!(code, 0, "{out}\n{err}");
    let v = parse(&out);
    assert_eq!(v.get("tests").and_then(Value::as_number_text), Some("4"), "all four kept: {out}");
    assert!(defined(&w.dir));
}

#[test]
fn root_export_carries_the_slice_inputs_unchanged() {
    let w = world();
    let (code, out, err) = define(&w.dir, &tests_json());
    assert_eq!(code, 0, "{out}\n{err}");
    let f = w.dir.path().join("rooted.json");
    let key = w.key.to_string_lossy().into_owned();
    let (code, out, err) = cli(
        &w.dir,
        &["root", "--namespace", NS, "--key", &key, "--export", f.to_str().unwrap(), "local/bank_rate_change"],
    );
    assert_eq!(code, 0, "{out}\n{err}");
    let bundle = json::parse(&std::fs::read(&f).unwrap()).unwrap();
    let tests = bundle_tests(&bundle);
    assert_eq!(tests.len(), 4, "{}", jcs(&bundle));
    let rooted = format!("{NS}/bank_rate_change");
    for t in &tests {
        assert_eq!(s(t, "test"), rooted, "{}", jcs(t));
        assert!(s(t, "call").starts_with(&format!("{rooted}(")), "{}", jcs(t));
    }
    // The empty-slice case: the slice given as empty, exactly as Part 2's hand-edited bundle had it.
    let empty: Vec<&Value> = tests.iter().filter(|t| t.get("inputs").is_some_and(|i| jcs(i).contains("{}"))).collect();
    assert_eq!(empty.len(), 1, "{tests:?}");
    let want_empty = json::parse(format!(r#"{{"{SLICE}":{{"members":{{}}}}}}"#).as_bytes()).unwrap();
    assert_eq!(jcs(empty[0].get("inputs").unwrap()), jcs(&want_empty), "{}", jcs(empty[0]));
    assert_eq!(jcs(empty[0].get("expect").unwrap()), r#"{"fault":"E_FORMULA_ABSENT"}"#);
    // The members map: the same cells, each value typed by Bank Rate's return type.
    let map: Vec<&Value> = tests.iter().filter(|t| t.get("inputs").is_some_and(|i| jcs(i).contains(DAY4))).collect();
    assert_eq!(map.len(), 1, "{tests:?}");
    let members = map[0].get("inputs").unwrap().get(SLICE).and_then(|x| x.get("members")).unwrap_or_else(|| {
        panic!("the slice's members: {}", jcs(map[0]));
    });
    let want = Value::Object(vec![(DAY4.into(), typed("number", "3.75")), (DAY5.into(), typed("number", "5"))]);
    assert_eq!(jcs(members), jcs(&want), "{}", jcs(map[0]));
    assert_eq!(jcs(map[0].get("expect").unwrap()), jcs(&typed("number", "1.25")));
    // The scalar cases carry no inputs, as before.
    assert_eq!(tests.iter().filter(|t| t.get("inputs").is_none()).count(), 2);
}

#[test]
fn a_malformed_slice_input_is_refused_at_define_and_nothing_is_defined() {
    let cases = [
        ("members is not an object", format!(r#"{{"{SLICE}": {{"members": []}}}}"#)),
        ("a plain literal for a slice", format!(r#"{{"{SLICE}": "4"}}"#)),
        ("a member's value is not a number", format!(r#"{{"{SLICE}": {{"members": {{"{DAY4}": "high"}}}}}}"#)),
        (
            "a member that is not a cell of the slice",
            format!(
                r#"{{"{SLICE}": {{"members": {{"datumesh.com/relay/treasury.gov/par_yield(date: 2026-11-20, tenor: \"10Y\")": "4"}}}}}}"#
            ),
        ),
        ("a member that is itself a slice", format!(r#"{{"{SLICE}": {{"members": {{"{SLICE}": "4"}}}}}}"#)),
        ("members with another member beside it", format!(r#"{{"{SLICE}": {{"members": {{}}, "add": {{}}}}}}"#)),
        ("an unknown member", format!(r#"{{"{SLICE}": {{"cells": {{}}}}}}"#)),
        ("members given for a single cell", format!(r#"{{"{DAY4}": {{"members": {{}}}}}}"#)),
    ];
    for (what, inputs) in cases {
        let w = world();
        let tests = format!(
            r#"[{{"call": "local/bank_rate_change(2026-11-05, 2026-11-20)", "inputs": {inputs}, "expect": {{"fault": "E_FORMULA_ABSENT"}}}}]"#
        );
        let (code, out, err) = define(&w.dir, &tests);
        assert_ne!(code, 0, "{what}: refused: {out}");
        assert!(err.contains("inputs"), "{what}: the refusal names the inputs: {err}");
        assert!(!defined(&w.dir), "{what}: nothing is defined");
    }
}
