//! CONTRACT-1.8b L3: forwarding and holding. A foreign cell is fetched once, verified before use,
//! and held (the second answer is the same bytes with no request); a declared restricted
//! computation is forwarded and held; without the declaration there is no local derivation.

mod common;

use common::*;
use datumesh_core::json;
use datumesh_core::Level;
use datumesh_local::Source;

const NOK_FULL: &str = r#"mesh-test.datumesh.com/ecb/fx_cross(base: "USD", quote: "NOK", date: 2026-10-30)"#;

#[test]
fn a_foreign_stored_cell_is_fetched_verified_and_then_held() {
    let up = upstream();
    let dir = TempDir::new("fwd-stored");
    let r = resolver(&dir, &up);
    let before = up.requests();
    let a = r.get(USD).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert!(up.requests() > before);
    // The receipt verifies against the upstream's key document, at level `signed`.
    let rc = receipt(&a.body);
    let v = datumesh_core::verify(&rc.to_jcs(), &upstream_keys(&up), None, None);
    assert!(v.ok, "{:?}: {}", v.code, v.detail);
    assert_eq!(v.level, Some(Level::Signed));
    // The second answer: the same bytes, no request.
    let held = up.requests();
    let b = r.get(USD).unwrap();
    assert_eq!(b.status, 200);
    assert_eq!(b.body, a.body);
    assert_eq!(up.requests() - held, 0);
}

#[test]
fn a_receipt_that_fails_verification_is_not_used_or_kept() {
    let bad = upstream_tampering();
    let dir = TempDir::new("fwd-tamper");
    let r = resolver(&dir, &bad);
    match r.get(USD) {
        Ok(a) => {
            assert_ne!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
            assert_ne!(a.outcome().as_deref(), Some("ok"));
        }
        Err(e) => assert!(matches!(e, datumesh_local::Error::Client { .. }), "{e}"),
    }
    // The cell id, from an untampered upstream's answer.
    let good = upstream();
    let gdir = TempDir::new("fwd-tamper-good");
    let id = resolver(&gdir, &good).get(USD).unwrap().receipt().unwrap().cell().id();
    assert!(r.store().latest(&id).unwrap().is_none(), "a version was stored for the tampered cell");
}

#[test]
fn a_declared_restricted_computation_is_forwarded_and_held() {
    let up = upstream();
    let dir = TempDir::new("fwd-cross");
    let r = resolver(&dir, &up);
    let a = r.get(NOK_FULL).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    let rc = receipt(&a.body);
    assert!(s(&rc, "computed_by").starts_with("mesh-test.datumesh.com#"));
    assert_eq!(s(&rc, "licence"), RESTRICTED);
    let held = up.requests();
    let b = r.get(NOK_FULL).unwrap();
    assert_eq!(b.status, 200);
    assert_eq!(b.body, a.body);
    assert_eq!(up.requests() - held, 0);
}

#[test]
fn there_is_no_local_derivation_without_a_declaration() {
    let up = upstream();
    let dir = TempDir::new("fwd-nok-twice");
    let r = resolver(&dir, &up);
    // The forwarded, mesh-signed answer is held first: the derivation is from a held restricted receipt.
    assert_eq!(r.get(NOK_FULL).unwrap().status, 200);
    let sig = json::parse(
        br#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": {"type": "number", "unit": {"of": "NOK", "per": "USD"}}}"#,
    )
    .unwrap();
    let d = r
        .define_formula(
            "local/nok_twice",
            r#"mesh-test.datumesh.com/ecb/fx_cross(base: "USD", quote: "NOK", date: date) * 2"#,
            &sig,
        )
        .unwrap();
    let before = up.requests();
    let a = r.get("local/nok_twice(date: 2026-10-30)").unwrap();
    assert_eq!(a.status, 403, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(a.outcome().as_deref(), Some("unlicensed"));
    assert_eq!(a.source, Source::Local);
    assert_eq!(up.requests() - before, 0);
    let id = cell_id_of("local.invalid", "nok_twice", &d.fn_hash, r#"{"date":{"type":"date","value":"2026-10-30"}}"#);
    assert!(r.store().computed_asof(&id, END).unwrap().is_none(), "this machine signed a receipt for the cell");
}
