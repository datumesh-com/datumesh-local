//! CONTRACT-1.8b L6: the alias map file with a `tx` per entry, resolved at the request's asof, and
//! prefix aliases (`@ecb/...`) in formula sources.

mod common;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::{AliasMap, Resolver};

const TX_A: &str = "2026-11-10T00:00:00.000000Z";
const TX_B: &str = "2026-11-12T00:00:00.000000Z";
const CALL: &str = r#"fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#;
const SIG: &str = r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": {"type": "number", "unit": {"of": "USD", "per": "EUR"}}}"#;

fn entry(short: &str, full: &str, tx: &str) -> String {
    format!(r#"{{"short": "{short}", "full": "{full}", "tx": "{tx}"}}"#)
}

fn map(entries: &[String]) -> String {
    format!(r#"{{"entries": [{}]}}"#, entries.join(", "))
}

fn open_with(dir: &TempDir, up: &Upstream, aliases: &str) -> Resolver {
    let c = config(dir, up);
    std::fs::write(&c.aliases_file, aliases).unwrap();
    Resolver::open(c).unwrap()
}

fn resolved(body: &[u8]) -> Value {
    envelope(body)
        .get("resolved")
        .cloned()
        .unwrap_or_else(|| panic!("no resolved in {}", String::from_utf8_lossy(body)))
}

fn not_ok(r: &datumesh_local::Reply) {
    assert_ne!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
}

#[test]
fn an_entry_that_does_not_exist_yet_at_the_asof_is_not_used() {
    let up = upstream();
    let dir = TempDir::new("aliases-asof");
    let text =
        map(&[entry("ecb/", "mesh-test.datumesh.com/ecb/", TX_A), entry("fx/", "mesh-test.datumesh.com/ecb/", TX_B)]);
    let r = open_with(&dir, &up, &text);
    // No `fx/` entry yet at 2026-11-11: the request is not expanded.
    match r.get(&format!("fx/{CALL}?asof=2026-11-11T00:00:00.000000Z")) {
        Ok(a) => not_ok(&a),
        Err(_) => {}
    }
    let a = r.get(&format!("fx/{CALL}?asof=2026-11-13T00:00:00.000000Z")).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(s(&resolved(&a.body), "aliases_tx"), TX_B);
}

#[test]
fn the_entry_with_the_greatest_tx_at_the_asof_wins() {
    let up = upstream();
    let dir = TempDir::new("aliases-two");
    let text =
        map(&[entry("ecb/", "mesh-test.datumesh.com/ecb/", TX_A), entry("ecb/", "unreachable.example.org/ecb/", TX_B)]);
    let r = open_with(&dir, &up, &text);
    let between = r.get(&format!("ecb/{CALL}?asof=2026-11-11T00:00:00.000000Z")).unwrap();
    assert_eq!(between.status, 200, "{}", String::from_utf8_lossy(&between.body));
    assert!(s(&resolved(&between.body), "call").contains("mesh-test.datumesh.com/ecb/fx_rate"));
    let after = r.get(&format!("ecb/{CALL}?asof=2026-11-13T00:00:00.000000Z")).unwrap();
    assert!(
        s(&resolved(&after.body), "call").contains("unreachable.example.org/ecb/fx_rate"),
        "{}",
        String::from_utf8_lossy(&after.body)
    );
    assert_ne!(after.status, 200);
}

/// Bare reference-class names resolve only to `datumesh.com/std/` (the user's rule, item 8): a bare
/// name whose target is a class elsewhere is refused when a request resolves it. A bare name for a
/// function (`xcross`, tests/classes.rs) is not a class name and keeps working.
#[test]
fn a_bare_class_name_must_be_a_standard_one() {
    let good = map(&[entry("currency", "datumesh.com/std/currency", TX_A)]);
    AliasMap::from_json(good.as_bytes()).unwrap();
    let up = upstream_classes();
    let dir = TempDir::new("aliases-bare");
    let text = map(&[entry("pair", PAIR_CLASS, "2026-01-15T00:00:00.000000Z")]);
    let r = open_with(&dir, &up, &text);
    let full = r.get(&format!("{PAIR_CLASS}(EUR, USD).rate({DAY})?asof={ASOF}")).unwrap();
    assert_eq!(full.status, 200, "{}", String::from_utf8_lossy(&full.body));
    match r.get(&format!("pair(EUR, USD).rate({DAY})?asof={ASOF}")) {
        Ok(a) => {
            not_ok(&a);
            assert!(
                String::from_utf8_lossy(&a.body).contains("datumesh.com/std/"),
                "{}",
                String::from_utf8_lossy(&a.body)
            );
        }
        Err(e) => assert!(e.to_string().contains("datumesh.com/std/"), "{e}"),
    }
}

#[test]
fn the_old_shape_is_still_read_with_its_tx_on_every_entry() {
    let old = format!(r#"{{"local/": "local.invalid/", "tx": "{TX_A}"}}"#);
    AliasMap::from_json(old.as_bytes()).unwrap();
    // As one entry with that tx: seen through a request's asof.
    let up = upstream();
    let dir = TempDir::new("aliases-old");
    let r = open_with(&dir, &up, &format!(r#"{{"ecb/": "mesh-test.datumesh.com/ecb/", "tx": "{TX_A}"}}"#));
    match r.get(&format!("ecb/{CALL}?asof=2026-11-09T00:00:00.000000Z")) {
        Ok(a) => not_ok(&a),
        Err(_) => {}
    }
    let a = r.get(&format!("ecb/{CALL}?asof=2026-11-11T00:00:00.000000Z")).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(s(&resolved(&a.body), "aliases_tx"), TX_A);
}

#[test]
fn a_prefix_alias_in_a_formula_source_is_expanded_before_hashing() {
    let up = upstream();
    let dir = TempDir::new("aliases-source");
    let r = open_with(
        &dir,
        &up,
        &map(&[entry("ecb/", "mesh-test.datumesh.com/ecb/", TX_A), entry("local/", "local.invalid/", TX_A)]),
    );
    let sig = json::parse(SIG.as_bytes()).unwrap();
    let a = r.define_formula("local/a", r#"@ecb/fx_rate(base: "EUR", quote: "USD", date: date) * 2"#, &sig).unwrap();
    let b = r
        .define_formula(
            "local/b",
            r#"mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", quote: "USD", date: date) * 2"#,
            &sig,
        )
        .unwrap();
    assert_eq!(a.fn_hash, b.fn_hash);
    let e = r.define_formula("local/c", r#"@nope/x(base: "EUR", date: date) * 2"#, &sig).unwrap_err();
    assert!(e.to_string().contains("E_FORMULA_ALIAS"), "{e}");
}
