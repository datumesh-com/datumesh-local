//! Brief test 4: a formula under local/ over fetched foreign cells evaluates here, memoizes,
//! and its receipt is signed by this machine's key (local.invalid) and labelled local; a
//! function under a rooted namespace that reads a local cell is refused with a typed error.
//! Publish: define plus put_version, under local/ only.

mod common;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::{Error, Source, LOCAL_LABEL};

fn sig(t: &str) -> Value {
    json::parse(t.as_bytes()).unwrap()
}

const CLOSED_SIG: &str =
    r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": "boolean"}"#;
const CLOSED_SRC: &str = r#"mesh-test.datumesh.com/ecb/holiday(calendar: "TARGET", date: date)"#;

#[test]
fn a_local_formula_over_fetched_cells_evaluates_here_and_memoizes() {
    let up = upstream();
    let dir = TempDir::new("define");
    let r = resolver(&dir, &up);
    // The callee is fetched (binding and descriptor) at define time.
    let d = r.define_formula("local/closed", CLOSED_SRC, &sig(CLOSED_SIG)).unwrap();
    assert_eq!(d.name, "local.invalid/closed");
    assert!(d.bound);
    // Defining it again changes nothing.
    assert!(!r.define_formula("local/closed", CLOSED_SRC, &sig(CLOSED_SIG)).unwrap().bound);
    let a = r.get("local/closed(date: 2026-12-25)").unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    // Its input was fetched for the evaluation.
    assert_eq!(a.source, Source::Fetched);
    let env = envelope(&a.body);
    assert_eq!(s(&env, "trust"), "computed");
    let rc = receipt(&a.body);
    assert_eq!(s(&rc, "computed_by"), format!("local.invalid#{}", r.key_id()));
    assert_eq!(s(rc.get("cell").unwrap(), "ns"), "local.invalid");
    assert_eq!(rc.get("value"), Some(&Value::Bool(true)));
    assert!(matches!(rc.get("inputs"), Some(Value::Array(i)) if i.len() == 1));
    assert_eq!(a.label.as_deref(), Some(LOCAL_LABEL));
    // Memoized: the second answer is the same bytes, from the store, with no request.
    let before = up.requests();
    let b = r.get("local/closed(date: 2026-12-25)").unwrap();
    assert_eq!(b.body, a.body);
    assert_eq!(b.source, Source::Local);
    assert_eq!(up.requests(), before);
    let cell_id = r.get("local/closed(date: 2026-12-25)").unwrap().receipt().unwrap().cell().id();
    assert!(r.store().computed_asof(&cell_id, END).unwrap().is_some(), "the evaluation is kept in the store");
    // It verifies here, at recomputed, with this machine's own key document.
    let v = r.verify(&rc.to_jcs());
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", String::from_utf8_lossy(&v.to_jcs()));
    assert_eq!(s(&v, "level"), "recomputed");
    // Elsewhere there is no key document for local.invalid: E_KEY_UNAVAILABLE.
    let nowhere = datumesh_core::verify(&rc.to_jcs(), &datumesh_core::Keys::new(), None, None);
    assert_eq!(nowhere.code.map(|c| c.as_str()), Some("E_KEY_UNAVAILABLE"));
    // Another date is evaluated too (its input fetched now).
    let c = r.get("local/closed(date: 2026-12-24)").unwrap();
    assert_eq!(receipt(&c.body).get("value"), Some(&Value::Bool(false)));
}

#[test]
fn a_rooted_function_reading_a_local_one_is_refused_at_define() {
    let up = upstream();
    let dir = TempDir::new("rooted");
    let r = resolver(&dir, &up);
    r.define_formula("local/closed", CLOSED_SRC, &sig(CLOSED_SIG)).unwrap();
    let e = r.define_formula("example.org/cal/closed", "local.invalid/closed(date: date)", &sig(CLOSED_SIG));
    match e {
        Err(Error::UnrootedInput { function, input }) => {
            assert_eq!(function, "example.org/cal/closed");
            assert_eq!(input, "local.invalid/closed");
        }
        other => panic!("{other:?}"),
    }
    assert!(r.store().resolve_binding("example.org/cal/closed", END).unwrap().is_none());
    // A descriptor written by hand is refused the same way.
    let d = format!(
        r#"{{"code": {{"body": "local.invalid/closed(date: date)\n", "lang": "formula"}}, "inputs": ["local.invalid/closed"], "signature": {CLOSED_SIG}}}"#
    );
    assert!(matches!(r.define("example.org/cal/closed", d.as_bytes()), Err(Error::UnrootedInput { .. })));
    // A local function may read local and rooted ones.
    r.define_formula("local/closed_too", "local.invalid/closed(date: date)", &sig(CLOSED_SIG)).unwrap();
    let a = r.get("local/closed_too(date: 2026-12-25)").unwrap();
    assert_eq!(receipt(&a.body).get("value"), Some(&Value::Bool(true)));
}

#[test]
fn publish_is_define_plus_put_version_under_local_only() {
    let up = upstream();
    let dir = TempDir::new("publish");
    let r = resolver(&dir, &up);
    let file = dir.path().join("notes.jsonl");
    std::fs::write(
        &file,
        concat!(
            r#"{"function": "local/note", "bound": "2026-11-01T00:00:00.000000Z", "signature": {"args": {"key": {"type": "string", "valid": false}}, "order": ["key"], "returns": "string"}}"#,
            "\n",
            r#"{"cell": {"fn": "local/note", "args": {"key": {"type": "string", "value": "a"}}}, "value": {"type": "string", "value": "first"}, "tx": "2026-11-02T00:00:00.000000Z", "asof": "2026-11-02T00:00:00.000000Z", "licence": "CC0-1.0", "summary": "A note."}"#,
            "\n",
            r#"{"formula": "local/shout", "bound": "2026-11-03T00:00:00.000000Z", "signature": {"args": {"key": {"type": "string", "valid": false}}, "order": ["key"], "returns": "string"}, "source": "upper(local.invalid/note(key: key))"}"#,
            "\n"
        ),
    )
    .unwrap();
    let p = r.publish(&file).unwrap();
    assert_eq!((p.functions, p.cells, p.versions), (2, 1, 1));
    let a = r.get(r#"local/note(key: "a")"#).unwrap();
    assert_eq!(a.status, 200);
    let rc = receipt(&a.body);
    assert_eq!(s(&rc, "value"), "first");
    assert_eq!(s(&rc, "computed_by"), format!("local.invalid#{}", r.key_id()));
    // A stored local cell is primary: its signer is its namespace's host.
    assert_eq!(s(&envelope(&a.body), "trust"), "primary");
    assert_eq!(a.label.as_deref(), Some(LOCAL_LABEL));
    let b = r.get(r#"local/shout(key: "a")"#).unwrap();
    assert_eq!(s(&receipt(&b.body), "value"), "FIRST");
    // Not under local/: refused before anything is stored.
    let other = dir.path().join("other.jsonl");
    std::fs::write(
        &other,
        r#"{"function": "example.org/x/note", "bound": "2026-11-01T00:00:00.000000Z", "signature": {"args": {"key": {"type": "string", "valid": false}}, "order": ["key"], "returns": "string"}}"#,
    )
    .unwrap();
    assert!(matches!(r.publish(&other), Err(Error::Request(_))));
    assert!(r.store().resolve_binding("example.org/x/note", END).unwrap().is_none());
    // A stored function defined directly, and find.
    r.define_stored(
        "local/scores",
        &sig(r#"{"args": {"k": {"type": "string", "valid": false}}, "order": ["k"], "returns": {"type": "number", "unit": "1"}}"#),
    )
    .unwrap();
    let found = r.find("note", false).unwrap();
    assert_eq!(found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["local.invalid/note"]);
}

#[test]
fn explain_gives_the_lineage_from_the_store() {
    let up = upstream();
    let dir = TempDir::new("explain");
    let r = resolver(&dir, &up);
    r.define_formula("local/closed", CLOSED_SRC, &sig(CLOSED_SIG)).unwrap();
    r.get("local/closed(date: 2026-12-25)").unwrap();
    let e = r.explain("local/closed(date: 2026-12-25)", Some(2)).unwrap();
    assert_eq!(e.status, 200, "{}", String::from_utf8_lossy(&e.body));
    let text = String::from_utf8_lossy(&e.body);
    assert!(text.contains("local.invalid#"), "{text}");
    assert!(text.contains("mesh-test.datumesh.com/ecb"), "{text}");
}
