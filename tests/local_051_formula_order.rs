//! datumesh-local 0.5.1, L6: a positional call inside a formula takes its argument order from
//! `/describe`'s `order` (datumesh-http 0.5.3, H1), falling back to the full descriptor's
//! `signature.order`. The responses are production's, captured (`tests/support/prod_mesh.rs`);
//! `describe_order: true` serves `/describe` with the `order` H1 adds, false as production answers
//! today (no `order`; the descriptor at `/descriptor/<hash>` has `signature.order`).
//!
//! The parse is checked where 0.5.0 fails it: when the formula is evaluated (`calc`), not at
//! `define`. Bank Rate (open licence) needs no acceptance; the two-argument `par_yield` (Treasury,
//! restricted) is run after `licence accept`.

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;

use common::*;
use datumesh_core::json::{self, Value};
use prod_mesh::*;

fn calc(describe_order: bool, formula: &str) -> (i32, Value, String, ProdMesh) {
    let prod = ProdMesh::start(describe_order);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let (code, out, err) = cli(&dir, &["calc", formula]);
    let v = json::parse(out.trim_end().as_bytes()).unwrap_or(Value::Null);
    (code, v, err, prod)
}

#[test]
fn a_positional_formula_evaluates_with_the_order_of_describe() {
    let (code, v, err, prod) = calc(true, "datumesh.com/relay/bankofengland.co.uk/bank_rate(2026-11-20) * 2");
    assert_eq!(code, 0, "{err}\nrequests: {:?}", prod.targets());
    assert_eq!(s(&v, "value"), "8");
}

#[test]
fn it_falls_back_to_the_descriptors_signature_order_when_describe_has_none() {
    // Production today: /describe carries no `order`.
    let (code, v, err, prod) = calc(false, "datumesh.com/relay/bankofengland.co.uk/bank_rate(2026-11-20) * 2");
    assert_eq!(code, 0, "{err}\nrequests: {:?}", prod.targets());
    assert_eq!(s(&v, "value"), "8");
}

#[test]
fn the_aliased_name_works_too() {
    for order in [true, false] {
        let (code, v, err, _) = calc(order, "@boe/bank_rate(2026-11-20) * 2");
        assert_eq!(code, 0, "describe_order {order}: {err}");
        assert_eq!(s(&v, "value"), "8");
    }
}

#[test]
fn too_many_positional_arguments_are_refused() {
    for order in [true, false] {
        let (code, _, err, _) = calc(order, "datumesh.com/relay/bankofengland.co.uk/bank_rate(2026-11-20, 3) * 2");
        assert_ne!(code, 0, "describe_order {order}");
        assert!(!err.is_empty());
    }
}

#[test]
fn two_arguments_in_the_declared_order_read_the_same_cell_as_the_labelled_call() {
    for order in [true, false] {
        let prod = ProdMesh::start(order);
        let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
        assert_eq!(cli(&dir, &["licence", "accept", TREASURY_LICENCE]).0, 0);
        let (code, pos, err) = cli(&dir, &["calc", "datumesh.com/relay/treasury.gov/par_yield(10Y, 2026-11-20) * 1"]);
        assert_eq!(code, 0, "describe_order {order}: {err}\n{:?}", prod.unknown());
        let (code, lab, err) =
            cli(&dir, &["calc", r#"datumesh.com/relay/treasury.gov/par_yield(tenor: "10Y", date: 2026-11-20) * 1"#]);
        assert_eq!(code, 0, "{err}");
        let (p, l) = (json::parse(pos.trim_end().as_bytes()).unwrap(), json::parse(lab.trim_end().as_bytes()).unwrap());
        assert_eq!(s(&p, "value"), "4.92");
        assert_eq!(s(&p, "canonical"), s(&l, "canonical"), "{pos}\n{lab}");
        assert!(s(&p, "canonical").contains(r#"par_yield(date: 2026-11-20, tenor: "10Y")"#), "{pos}");
    }
}

#[test]
fn the_order_is_used_not_guessed() {
    // par_yield's order is [tenor, date]: the date first is an error, not a silent swap.
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    assert_eq!(cli(&dir, &["licence", "accept", TREASURY_LICENCE]).0, 0);
    let (code, out, _) = cli(&dir, &["calc", "datumesh.com/relay/treasury.gov/par_yield(2026-11-20, 10Y) * 1"]);
    assert_ne!(code, 0, "{out}");
}
