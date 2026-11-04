//! Brief test 5: a function over a cell whose licence is not open is forwarded to the
//! configured mesh, and the receipt returned is signed by that mesh, not locally; over open
//! cells the same function evaluates here.

mod common;

use common::*;
use datumesh_local::Source;

const NOK: &str = r#"ecb/fx_cross(base: "USD", quote: "NOK", date: 2026-10-30)"#;
const GBP_CROSS: &str = r#"ecb/fx_cross(base: "USD", quote: "GBP", date: 2026-10-30)"#;

#[test]
fn a_restricted_input_forwards_to_the_configured_mesh() {
    let up = upstream();
    let dir = TempDir::new("gate");
    let r = resolver(&dir, &up);
    let a = r.get(NOK).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(a.source, Source::Forwarded { mesh: HOST.to_string() });
    let rc = receipt(&a.body);
    let signer = s(&rc, "computed_by");
    assert!(signer.starts_with("mesh-test.datumesh.com#"), "{signer}");
    assert_eq!(s(&envelope(&a.body), "trust"), "computed");
    assert!(a.label.is_none());
    assert!(a.detail.as_deref().unwrap_or("").contains("restricted-test"), "{:?}", a.detail);
    // The answer is kept: again, and offline, the same bytes.
    r.set_offline(true);
    let b = r.get(NOK).unwrap();
    assert_eq!(b.body, a.body);
    assert_eq!(b.source, Source::Forwarded { mesh: HOST.to_string() });
    // Nothing was signed here for the cell.
    let id = a.receipt().unwrap().cell().id();
    let local = r.store().computed_asof(&id, END).unwrap();
    assert!(local.is_none(), "a local evaluation was kept: {local:?}");
}

#[test]
fn open_inputs_evaluate_here() {
    let up = upstream();
    let dir = TempDir::new("gate-open");
    let r = resolver(&dir, &up);
    let a = r.get(GBP_CROSS).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(s(&receipt(&a.body), "computed_by"), format!("local.invalid#{}", r.key_id()));
    assert_eq!(s(&envelope(&a.body), "trust"), "computed");
    // The full name, in the receipt.
    assert_eq!(s(receipt(&a.body).get("cell").unwrap(), "ns"), "mesh-test.datumesh.com/ecb");
}

#[test]
fn without_a_forward_mesh_the_answer_is_unlicensed() {
    let up = upstream();
    let dir = TempDir::new("gate-none");
    let mut c = config(&dir, &up);
    c.forward_mesh = None;
    let r = datumesh_local::Resolver::open(c).unwrap();
    let a = r.get(NOK).unwrap();
    assert_eq!(a.status, 403);
    assert_eq!(s(&envelope(&a.body), "outcome"), "unlicensed");
}
