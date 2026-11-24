//! datumesh-local 0.5.1, L5: a receipt this machine signed under a restricted licence (or the
//! LicenseRef a derivation over several carries), and any cell that depends on one, is never
//! published or exported: `export`, `root`, `publish --to`, bundle export and the MCP tools
//! `publish` and `publish_public` refuse with `E_LICENCE_NOT_PUBLISHABLE`, naming the licence.
//! An open-licence local result still exports.

#![cfg(unix)]

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;
#[path = "support/whatif_stub.rs"]
mod whatif_stub;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self};
use datumesh_local::mcp::McpServer;
use datumesh_local::{Config, Resolver};
use prod_mesh::*;
use tools::{everything, is_error, jcs, q, structured, tool};
use whatif_stub::{DocMode, WhatifStub};

const CODE: &str = "E_LICENCE_NOT_PUBLISHABLE";
const NS: &str = "mesh-test.datumesh.com/scratch_l5";
const SIG: &str = r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": {"type": "number", "unit": "percent"}}"#;
const Y10_SRC: &str = r#"datumesh.com/relay/treasury.gov/par_yield(tenor: "10Y", date: date) * 1"#;
const Y10_TESTS: &str = r#"[{"call":"local/y10(date: 2026-11-20)","expect":"4.92"},{"call":"local/y10(date: 2026-01-01)","expect":{"fault":"E_FORMULA_ABSENT"}}]"#;
const BR_SRC: &str = r#"datumesh.com/relay/bankofengland.co.uk/bank_rate(date: date) * 1"#;
const BR_TESTS: &str = r#"[{"call":"local/br(date: 2026-11-20)","expect":"4"},{"call":"local/br(date: 2026-01-01)","expect":{"fault":"E_FORMULA_ABSENT"}}]"#;

struct W {
    prod: ProdMesh,
    up: Upstream,
    stub: Arc<WhatifStub>,
    dir: TempDir,
    key: std::path::PathBuf,
    _tmp: TempDir,
}

/// A home with the production fake and the test mesh as peers, Treasury's licence accepted, and
/// two functions of the local namespace with their answers held: `local/y10` (Treasury yield, a
/// restricted result) and `local/br` (Bank Rate, an open result).
fn world() -> W {
    let prod = ProdMesh::start(true);
    let up = upstream();
    let stub = WhatifStub::start(&up, DocMode::Direct);
    let dir = fresh_home(&[("datumesh.com", &prod.origin), (HOST, &up.origin)]);
    let tmp = TempDir::new("l5-key");
    let key = tmp.path().join("namespace.key");
    tools::write_key(&key);
    let (code, _, err) = cli(&dir, &["licence", "accept", TREASURY_LICENCE]);
    assert_eq!(code, 0, "{err}");
    for (name, src, tests, call) in [
        ("local/y10", Y10_SRC, Y10_TESTS, "local/y10(date: 2026-11-20)"),
        ("local/br", BR_SRC, BR_TESTS, "local/br(date: 2026-11-20)"),
    ] {
        let (code, _, err) =
            cli(&dir, &["define", name, "--formula", src, "--signature", SIG, "--tests", tests]);
        assert_eq!(code, 0, "{err}\n{:?}", prod.unknown());
        let (code, out, err) = cli(&dir, &["get", call]);
        assert_eq!(code, 0, "{err}\n{:?}", prod.unknown());
        let lic = s(json::parse(out.trim_end().as_bytes()).unwrap().get("receipt").unwrap(), "licence");
        assert_eq!(lic, if name == "local/y10" { TREASURY_LICENCE } else { BANK_RATE_LICENCE }, "{out}");
    }
    W { prod, up, stub, dir, key, _tmp: tmp }
}

fn refused(code: i32, out: &str, err: &str, licence: &str) {
    assert_ne!(code, 0, "{out}");
    assert!(err.contains(CODE), "{err}");
    assert!(err.contains(licence), "names the licence: {err}");
}

#[test]
fn export_refuses_a_restricted_local_result() {
    let w = world();
    let f = w.dir.path().join("bundle.json");
    let (code, out, err) = cli(&w.dir, &["export", "--out", f.to_str().unwrap()]);
    refused(code, &out, &err, TREASURY_LICENCE);
    assert!(!f.exists(), "nothing is written");
    // Bundle export through the library, too.
    let r = Resolver::open(Config::in_dir(w.dir.path()).unwrap()).unwrap();
    let e = r.export().expect_err("the bundle of everything held refuses");
    let text = format!("{e}");
    assert!(text.contains(CODE) && text.contains(TREASURY_LICENCE), "{text}");
}

#[test]
fn export_refuses_the_licenceref_a_derivation_over_several_carries() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    for l in [TREASURY_LICENCE, ECB_LICENCE] {
        assert_eq!(cli(&dir, &["licence", "accept", l]).0, 0);
    }
    let two = "datumesh.com/relay/treasury.gov/par_yield(10Y, 2026-11-20) * datumesh.com/relay/ecb.europa.eu/fx_reference(USD, 2026-11-20)";
    let (code, _, err) = cli(&dir, &["calc", two]);
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = cli(&dir, &["export"]);
    refused(code, &out, &err, "LicenseRef-datumesh-local-derivation");
}

#[test]
fn a_home_with_only_an_open_licence_local_result_still_exports() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let (code, _, err) = cli(&dir, &["define", "local/br", "--formula", BR_SRC, "--signature", SIG]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = cli(&dir, &["get", "local/br(date: 2026-11-20)"]);
    assert_eq!(code, 0, "{err}");
    let f = dir.path().join("bundle.json");
    let (code, out, err) = cli(&dir, &["export", "--out", f.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(f.exists());
    // Held restricted cells that the mesh delivered are still exported: delivery is allowed; only
    // what this machine signed under such a licence is not.
    let (code, _, err) = cli(&dir, &["get", "datumesh.com/relay/treasury.gov/par_yield(10Y, 2026-11-20)"]);
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = cli(&dir, &["export", "--out", f.to_str().unwrap()]);
    assert_eq!(code, 0, "a fetched restricted receipt is the mesh's, not this machine's: {out}\n{err}");
}

#[test]
fn root_refuses_a_function_whose_held_result_is_restricted_and_not_an_open_one() {
    let w = world();
    let key = w.key.to_string_lossy().into_owned();
    let (code, out, err) = cli(&w.dir, &["root", "--namespace", NS, "--key", &key, "local/y10"]);
    refused(code, &out, &err, TREASURY_LICENCE);
    // With an export file too; nothing is written.
    let f = w.dir.path().join("rooted.json");
    let (code, out, err) = cli(&w.dir, &["root", "--namespace", NS, "--key", &key, "--export", f.to_str().unwrap(), "local/y10"]);
    refused(code, &out, &err, TREASURY_LICENCE);
    assert!(!f.exists());
    // The open one roots.
    let (code, out, err) = cli(&w.dir, &["root", "--namespace", NS, "--key", &key, "local/br"]);
    assert_eq!(code, 0, "{out}\n{err}");
}

#[test]
fn publish_to_refuses_before_anything_is_sent() {
    let w = world();
    let key = w.key.to_string_lossy().into_owned();
    let (code, out, err) =
        cli(&w.dir, &["publish", "--to", &w.stub.origin, "--namespace", NS, "--key", &key, "local/y10"]);
    refused(code, &out, &err, TREASURY_LICENCE);
    assert!(w.stub.posts().is_empty(), "{:?}", w.stub.posts());
    // Also when it is listed with an open one.
    let (code, out, err) =
        cli(&w.dir, &["publish", "--to", &w.stub.origin, "--namespace", NS, "--key", &key, "local/br", "local/y10"]);
    refused(code, &out, &err, TREASURY_LICENCE);
    assert!(w.stub.posts().is_empty());
    let _ = (&w.up, &w.prod);
}

#[test]
fn the_mcp_publish_public_refuses_with_the_code_and_the_licence() {
    let w = world();
    let srv = McpServer::new(Arc::new(Resolver::open(Config::in_dir(w.dir.path()).unwrap()).unwrap()));
    let args = format!(
        r#"{{"to":{},"namespace":{},"key":{},"names":["local/y10"]}}"#,
        q(&w.stub.origin),
        q(NS),
        q(&w.key.to_string_lossy())
    );
    let v = tool(&srv, "publish_public", &args);
    assert!(is_error(&v), "{}", jcs(&v));
    assert_eq!(s(structured(&v), "error"), CODE, "{}", jcs(structured(&v)));
    assert!(everything(&v).contains(TREASURY_LICENCE), "{}", everything(&v));
    assert!(w.stub.posts().is_empty());
}

#[test]
fn the_mcp_publish_refuses_cells_this_machine_would_sign_under_a_restricted_or_licenceref_licence() {
    let up = upstream();
    let dir = TempDir::new("l5-mcp-publish");
    let clock = ManualClock::new("2026-11-15T12:00:00.000000Z");
    let srv = McpServer::new(Arc::new(resolver_at(&dir, &up, &clock)));
    let sig = r#"{"args":{"day":{"type":"date","valid":true}},"order":["day"],"returns":{"type":"number","unit":"1"}}"#;
    let cell = |day: &str, licence: &str| {
        format!(
            r#"{{"cell":{{"fn":"local/level","args":{{"day":{{"type":"date","value":"{day}"}}}}}},"value":{{"type":"number","value":"1"}},"tx":"2026-11-15T12:00:00.000000Z","asof":"2026-11-15T12:00:00.000000Z","licence":{},"summary":"A level."}}"#,
            q(licence)
        )
    };
    let lines = |c: &str| format!(r#"{{"lines":[{{"function":"local/level","signature":{sig}}},{c}]}}"#);
    for licence in ["https://example.org/restricted-terms", "LicenseRef-datumesh-local-derivation"] {
        let v = tool(&srv, "publish", &lines(&cell("2026-11-01", licence)));
        assert!(is_error(&v), "{licence}: {}", jcs(&v));
        assert_eq!(s(structured(&v), "error"), CODE, "{}", jcs(structured(&v)));
        assert!(everything(&v).contains(licence), "{}", everything(&v));
    }
    // An open licence publishes as before.
    let v = tool(&srv, "publish", &lines(&cell("2026-11-01", "CC0-1.0")));
    assert!(!is_error(&v), "{}", jcs(&v));
}
