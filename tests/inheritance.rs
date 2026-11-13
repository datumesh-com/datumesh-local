//! Licence inheritance (spec-0.4.3 8.5, datumesh-engine 0.4.1) as the local resolver meets
//! it: the receipts this machine signs carry the licence the rule gives over their inputs,
//! with the publisher's declarations from the key document held for its host. Brief test 5's
//! decided behaviour stands: a function over the restricted NOK cell is forwarded and the
//! receipt returned is the mesh's.

mod common;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::Source;

const NOK: &str = r#"ecb/fx_cross(base: "USD", quote: "NOK", date: 2026-10-30)"#;
const NOK_UNDECLARED: &str = r#"ecb/fx_cross_undeclared(base: "USD", quote: "NOK", date: 2026-10-30)"#;
const GBP_CROSS: &str = r#"ecb/fx_cross(base: "USD", quote: "GBP", date: 2026-10-30)"#;
const STRIP: &str = r#"scripture/strip_niqqud(book: "Gen", chapter: 1, verse: 1)"#;

#[test]
fn fx_cross_over_the_nok_cell_is_the_mesh_s_with_the_declared_licence() {
    let up = upstream();
    let dir = TempDir::new("inherit-nok");
    let r = resolver(&dir, &up);
    let a = r.get(NOK).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(a.source, Source::Forwarded { mesh: HOST.into() });
    let rc = receipt(&a.body);
    assert!(s(&rc, "computed_by").starts_with("mesh-test.datumesh.com#"));
    // The mesh computed it under its declaration: the result carries the restricted licence.
    assert_eq!(s(&rc, "licence"), RESTRICTED);
    // Nothing signed here.
    let id = a.receipt().unwrap().cell().id();
    assert!(r.store().computed_asof(&id, END).unwrap().is_none());
}

#[test]
fn fx_cross_undeclared_over_the_nok_cell_is_unlicensed() {
    let up = upstream();
    let dir = TempDir::new("inherit-undeclared");
    let r = resolver(&dir, &up);
    let a = r.get(NOK_UNDECLARED).unwrap();
    assert_eq!(a.status, 403, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(s(&envelope(&a.body), "outcome"), "unlicensed");
    let b = r.get(r#"ecb/fx_cross_undeclared(base: "USD", quote: "GBP", date: 2026-10-30)"#).unwrap();
    assert_eq!(b.status, 200);
    assert_eq!(s(&receipt(&b.body), "licence"), "CC-BY-4.0");
}

#[test]
fn strip_niqqud_over_uxlc_carries_the_declared_licence() {
    let up = upstream();
    let dir = TempDir::new("inherit-strip");
    let r = resolver(&dir, &up);
    let a = r.get(STRIP).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    let rc = receipt(&a.body);
    assert_eq!(s(&rc, "computed_by"), format!("local.invalid#{}", r.key_id()));
    assert_eq!(s(&rc, "licence"), UXLC_LICENCE);
    assert_eq!(s(&rc, "value"), "Synthtc tst vrs Gn 1:1 (tst dt).");
    // The publisher's key document, with its declarations, is held here.
    let doc = r.store().namespace(HOST).unwrap().unwrap().keydoc;
    let doc = json::parse(&doc).unwrap();
    assert!(doc.get("declarations").and_then(|d| d.get("mesh-test.datumesh.com/scripture/strip_niqqud")).is_some());
}

#[test]
fn an_open_only_function_inherits_its_inputs_licence() {
    let up = upstream();
    let dir = TempDir::new("inherit-open");
    let r = resolver(&dir, &up);
    let a = r.get(GBP_CROSS).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    let rc = receipt(&a.body);
    assert_eq!(s(&rc, "computed_by"), format!("local.invalid#{}", r.key_id()));
    assert_eq!(s(&rc, "licence"), "CC-BY-4.0");
    // A CC0-1.0 input alone gives CC0-1.0 (the configured licence is not used).
    r.define_formula(
        "local/closed",
        r#"mesh-test.datumesh.com/ecb/holiday(calendar: "TARGET", date: date)"#,
        &json::parse(
            br#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": "boolean"}"#,
        )
        .unwrap(),
    )
    .unwrap();
    let c = r.get("local/closed(date: 2026-12-25)").unwrap();
    assert_eq!(s(&receipt(&c.body), "licence"), "CC0-1.0");
}

#[test]
fn a_local_function_over_local_inputs() {
    let up = upstream();
    let dir = TempDir::new("inherit-local");
    let r = resolver(&dir, &up);
    let file = dir.path().join("local.jsonl");
    std::fs::write(
        &file,
        concat!(
            r#"{"function": "local/note", "bound": "2026-11-01T00:00:00.000000Z", "signature": {"args": {"k": {"type": "string", "valid": false}}, "order": ["k"], "returns": "string"}}"#,
            "\n",
            r#"{"cell": {"fn": "local/note", "args": {"k": {"type": "string", "value": "a"}}}, "value": {"type": "string", "value": "first"}, "tx": "2026-11-02T00:00:00.000000Z", "asof": "2026-11-02T00:00:00.000000Z", "licence": "CC0-1.0", "summary": "A note."}"#,
            "\n",
            r#"{"formula": "local/shout", "bound": "2026-11-03T00:00:00.000000Z", "signature": {"args": {"k": {"type": "string", "valid": false}}, "order": ["k"], "returns": "string"}, "source": "upper(local.invalid/note(k: k))"}"#,
            "\n",
            r#"{"formula": "local/answer", "bound": "2026-11-03T00:00:00.000000Z", "signature": {"args": {"k": {"type": "string", "valid": false}}, "order": ["k"], "returns": "string"}, "source": "\"forty-two\""}"#,
            "\n"
        ),
    )
    .unwrap();
    r.publish(&file).unwrap();
    // Over a local CC0-1.0 cell: CC0-1.0, signed here.
    let a = r.get(r#"local/shout(k: "a")"#).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(s(&receipt(&a.body), "licence"), "CC0-1.0");
    // No inputs: the mesh's own licence, this resolver's configured one.
    let b = r.get(r#"local/answer(k: "a")"#).unwrap();
    assert_eq!(b.status, 200, "{}", String::from_utf8_lossy(&b.body));
    assert_eq!(s(&receipt(&b.body), "licence"), r.config().licence);
    let _ = Value::Null;
}
