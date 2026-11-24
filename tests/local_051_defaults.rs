//! datumesh-local 0.5.1, L1 (production defaults for a new home; an old home untouched, with a
//! notice), L2 (`describe` asks `/fn/<full name>?format=json`) and L3 (`verify` says `signed`
//! unless it recomputed). The production mesh is a fake serving responses captured from
//! datumesh.com (`tests/support/prod_mesh.rs`); nothing leaves the machine.

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::Config;
use prod_mesh::*;

fn parse(out: &str) -> Value {
    json::parse(out.trim_end().as_bytes()).unwrap_or_else(|e| panic!("not JSON: {out}: {e:?}"))
}

/// The alias map file as (short, full) pairs, in either of its shapes.
fn alias_pairs(dir: &TempDir) -> Vec<(String, String)> {
    let v = json::parse(&std::fs::read(dir.path().join("aliases.json")).unwrap()).unwrap();
    let mut out = Vec::new();
    if let Some(Value::Array(entries)) = v.get("entries") {
        for e in entries {
            out.push((s(e, "short"), s(e, "full")));
        }
    } else {
        for (k, x) in v.as_object().unwrap() {
            if k != "tx" {
                out.push((k.clone(), x.as_str().unwrap().to_string()));
            }
        }
    }
    out.sort();
    out
}

const NOTICE: &str = "the default forward mesh is now datumesh.com; your config names mesh-test.datumesh.com (edit ";

// ------------------------------------------------------------------------------------------ L1

#[test]
fn l1_a_fresh_home_has_the_production_defaults() {
    let dir = TempDir::new("l1-fresh");
    let c = Config::in_dir(dir.path()).unwrap();
    assert_eq!(c.forward_mesh.as_deref(), Some("datumesh.com"));
    // The test mesh stays on the allowlist, so full mesh-test names still work.
    assert!(c.allowlist.contains(&"datumesh.com".to_string()), "{:?}", c.allowlist);
    assert!(c.allowlist.contains(&"mesh-test.datumesh.com".to_string()), "{:?}", c.allowlist);
    assert_eq!(c.find_meshes, vec!["datumesh.com".to_string()]);
    let mut want = vec![
        ("boe/", "datumesh.com/relay/bankofengland.co.uk/"),
        ("earth/", "datumesh.com/earth/"),
        ("ecb/", "datumesh.com/relay/ecb.europa.eu/"),
        ("local/", "local.invalid/"),
        ("scripture/", "datumesh.com/scripture/"),
        ("treasury/", "datumesh.com/relay/treasury.gov/"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect::<Vec<_>>();
    want.sort();
    assert_eq!(alias_pairs(&dir), want);
    // The installed config names the forward mesh in its file, too.
    let cfg = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(cfg.contains("\"forward_mesh\": \"datumesh.com\""), "{cfg}");
}

#[test]
fn l1_an_alias_resolves_to_the_production_name_from_a_canned_fetch() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let (code, out, err) = cli(&dir, &["get", "ecb/fx_reference(USD, 2026-11-20)"]);
    assert_eq!(code, 0, "{err}\nunknown to the fake: {:?}", prod.unknown());
    let env = parse(&out);
    assert_eq!(s(&env, "outcome"), "ok");
    assert_eq!(s(env.get("receipt").unwrap(), "value"), "1.099");
    assert_eq!(s(env.get("receipt").unwrap().get("cell").unwrap(), "ns"), "datumesh.com/relay/ecb.europa.eu");
    assert!(
        err.contains(r#"as datumesh.com/relay/ecb.europa.eu/fx_reference(currency: "USD", date: 2026-11-20)"#),
        "{err}"
    );
    assert!(err.contains("signer datumesh.com#eddd0c12fedb022e"), "{err}");
    // Every request went to the fake (the production host), none to the test mesh; only GETs.
    assert!(prod.posts().is_empty());
    assert!(prod.targets().iter().any(|t| t.starts_with("/cell/datumesh.com/relay/ecb.europa.eu/fx_reference")));
    // Now held: the same bytes offline.
    let (code, out2, err) = cli(&dir, &["--offline", "get", "ecb/fx_reference(USD, 2026-11-20)"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out2, out);
}

#[test]
fn l1_the_treasury_and_boe_short_names_resolve_too() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let (code, out, err) = cli(&dir, &["get", "treasury/par_yield(10Y, 2026-11-20)"]);
    assert_eq!(code, 0, "{err}\n{:?}", prod.unknown());
    assert_eq!(s(parse(&out).get("receipt").unwrap(), "value"), "4.92");
    let (code, out, err) = cli(&dir, &["get", "boe/bank_rate(2026-11-20)"]);
    assert_eq!(code, 0, "{err}\n{:?}", prod.unknown());
    assert_eq!(s(parse(&out).get("receipt").unwrap(), "value"), "4");
}

#[test]
fn l1_an_old_home_is_not_rewritten_and_every_command_prints_the_notice() {
    let dir = old_home(&[]);
    let cfg_path = dir.path().join("config.json");
    let (cfg0, al0) = (std::fs::read(&cfg_path).unwrap(), std::fs::read(dir.path().join("aliases.json")).unwrap());
    let cfg_text = String::from_utf8(cfg0.clone()).unwrap();
    assert!(cfg_text.contains("\"forward_mesh\": \"mesh-test.datumesh.com\""));
    for args in [
        &["--offline", "key"][..],
        &["--offline", "trust", "list"],
        &["--offline", "get", "local/nothing(date: 2026-01-01)"],
    ] {
        let (_, _, err) = cli(&dir, args);
        let notices: Vec<&str> =
            err.lines().filter(|l| l.contains("the default forward mesh is now datumesh.com")).collect();
        assert_eq!(notices.len(), 1, "one line for {args:?}: {err}");
        assert!(notices[0].contains(NOTICE), "{}", notices[0]);
        assert!(notices[0].contains(&*cfg_path.to_string_lossy()), "the path of config.json: {}", notices[0]);
        assert!(notices[0].trim_end().ends_with("to change)"), "{}", notices[0]);
    }
    assert_eq!(std::fs::read(&cfg_path).unwrap(), cfg0, "config.json is the user's");
    assert_eq!(std::fs::read(dir.path().join("aliases.json")).unwrap(), al0, "aliases.json is the user's");
    // The old aliases still work on the old home (it is not migrated).
    assert!(String::from_utf8(al0).unwrap().contains("mesh-test.datumesh.com/ecb/"));
}

#[test]
fn l1_no_notice_for_a_fresh_home_or_a_home_that_already_names_production() {
    let dir = TempDir::new("l1-nonotice");
    let (_, _, err) = cli(&dir, &["--offline", "key"]);
    assert!(!err.contains("default forward mesh"), "{err}");
    // The second run on the same (now existing) fresh home: still none.
    let (_, _, err) = cli(&dir, &["--offline", "key"]);
    assert!(!err.contains("default forward mesh"), "{err}");
    // An old-shaped home whose user has moved to production.
    let old = old_home(&[]);
    let p = old.path().join("config.json");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(
        &p,
        text.replace("\"forward_mesh\": \"mesh-test.datumesh.com\"", "\"forward_mesh\": \"datumesh.com\""),
    )
    .unwrap();
    let (_, _, err) = cli(&old, &["--offline", "key"]);
    assert!(!err.contains("default forward mesh"), "{err}");
}

// ------------------------------------------------------------------------------------------ L2

#[test]
fn l2_describe_asks_the_fn_page_by_its_full_name_with_the_host() {
    // Production today: /describe has no `order`.
    let prod = ProdMesh::start(false);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let (code, out, err) = cli(&dir, &["describe", "datumesh.com/relay/treasury.gov/par_yield"]);
    assert_eq!(code, 0, "{err}");
    let targets = prod.targets();
    assert!(
        targets.iter().any(|t| t == "/fn/datumesh.com/relay/treasury.gov/par_yield?format=json"),
        "the tests are asked at <host>/fn/<full name>?format=json: {targets:?}"
    );
    assert!(!targets.iter().any(|t| t.starts_with("/fn/relay/")), "never the name without its host: {targets:?}");
    let all = format!("{out}\n{err}");
    assert!(!all.contains("tests: unavailable"), "the page is there: {all}");
}

#[test]
fn l2_an_alias_is_expanded_before_the_fn_page_is_asked() {
    let prod = ProdMesh::start(false);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let (code, _, err) = cli(&dir, &["describe", "boe/bank_rate"]);
    assert_eq!(code, 0, "{err}");
    let targets = prod.targets();
    assert!(
        targets.iter().any(|t| t == "/fn/datumesh.com/relay/bankofengland.co.uk/bank_rate?format=json"),
        "{targets:?}"
    );
}

#[test]
fn l2_describe_still_works_when_the_function_describe_carries_order() {
    // datumesh-http 0.5.3 (H1): /describe of a function has `order`, outside the descriptor.
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let (code, out, err) = cli(&dir, &["describe", "datumesh.com/relay/treasury.gov/par_yield"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        prod.targets().iter().any(|t| t == "/fn/datumesh.com/relay/treasury.gov/par_yield?format=json"),
        "{:?}",
        prod.targets()
    );
}

// ------------------------------------------------------------------------------------------ L3

fn verify_level(dir: &TempDir, envelope: &str) -> Value {
    let f = dir.path().join("env.json");
    std::fs::write(&f, envelope).unwrap();
    let (code, out, err) = cli(dir, &["verify", f.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}\n{err}");
    parse(&out)
}

#[test]
fn l3_a_relayed_receipt_verifies_at_signed() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let (code, env, err) = cli(&dir, &["get", "ecb/fx_reference(USD, 2026-11-20)"]);
    assert_eq!(code, 0, "{err}");
    let v = verify_level(&dir, &env);
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", String::from_utf8_lossy(&v.to_jcs()));
    assert_eq!(s(&v, "level"), "signed", "a relayed receipt is checked by its signature only");
    // From the store (offline) it is the same.
    let (_, env2, _) = cli(&dir, &["--offline", "get", "ecb/fx_reference(USD, 2026-11-20)"]);
    assert_eq!(s(&verify_level(&dir, &env2), "level"), "signed");
    // Through the pipe form, `verify -`, as the README shows it: the same file contents.
    assert_eq!(env, env2);
}

#[test]
fn l3_a_computed_receipt_verifies_at_recomputed() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    // Bank Rate is under an open licence (OGL-UK-3.0): computed here with no acceptance.
    let sig = r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": {"type": "number", "unit": "percent"}}"#;
    let (code, _, err) = cli(
        &dir,
        &[
            "define",
            "local/br2",
            "--formula",
            "datumesh.com/relay/bankofengland.co.uk/bank_rate(date: date) * 2",
            "--signature",
            sig,
        ],
    );
    assert_eq!(code, 0, "{err}\n{:?}", prod.unknown());
    let (code, env, err) = cli(&dir, &["get", "local/br2(date: 2026-11-20)"]);
    assert_eq!(code, 0, "{err}\n{:?}", prod.unknown());
    assert_eq!(s(parse(&env).get("receipt").unwrap(), "value"), "8");
    let v = verify_level(&dir, &env);
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", String::from_utf8_lossy(&v.to_jcs()));
    assert_eq!(s(&v, "level"), "recomputed", "step 7 recomputed the value here");
}
