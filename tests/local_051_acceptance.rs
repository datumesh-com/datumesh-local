//! The acceptance test of the protocol round (CONTRACT-protocol-part1, datumesh-local 0.5.1),
//! offline with canned production receipts (`tests/data/production/routes.jsonl`, captured by
//! GET from datumesh.com on 2026-11-24; `tests/support/prod_mesh.rs`):
//!
//! 1. a fresh home;
//! 2. `licence accept` for the Treasury licence and for Bank Rate's licence;
//! 3. `calc` of the 10-year Treasury yield minus Bank Rate on 2026-11-20 answers 0.92, both input
//!    receipts in its lineage;
//! 4. `define local/spread` with tests, then `get` it: 0.92;
//! 5. whatif with Bank Rate set to 4.5: 0.42;
//! 6. `export` refuses.
//!
//! What the licence of the result is: production's Bank Rate receipt carries `OGL-UK-3.0`, an OPEN
//! licence (SPEC 8.5's tier list), so exactly one input licence is restricted (Treasury's) and the
//! engine's rule E1 gives the result THAT licence; `LicenseRef-datumesh-local-derivation` is the
//! result over two or more restricted licences (`local_051_licence.rs`, Treasury times ECB). The
//! contract's step 3 says the LicenseRef; the facts of production make that unreachable.

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::McpServer;
use datumesh_local::{Config, Resolver};
use prod_mesh::*;
use tools::*;

const FORMULA: &str = "datumesh.com/relay/treasury.gov/par_yield(10Y, 2026-11-20) - datumesh.com/relay/bankofengland.co.uk/bank_rate(2026-11-20)";
const SPREAD_SRC: &str = r#"datumesh.com/relay/treasury.gov/par_yield(tenor: "10Y", date: date) - datumesh.com/relay/bankofengland.co.uk/bank_rate(date: date)"#;
const SPREAD_SIG: &str = r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": {"type": "number", "unit": "percent"}}"#;
const SPREAD_TESTS: &str = r#"[{"call":"local/spread(date: 2026-11-20)","expect":"0.92"}]"#;
const BANK_RATE_CELL: &str = "datumesh.com/relay/bankofengland.co.uk/bank_rate(date: 2026-11-20)";

fn parse(out: &str) -> Value {
    json::parse(out.trim_end().as_bytes()).unwrap_or_else(|e| panic!("not JSON: {out}: {e:?}"))
}

fn inputs_of(rc: &Value) -> Vec<String> {
    match rc.get("inputs") {
        Some(Value::Array(a)) => a.iter().map(|h| h.as_str().unwrap().to_string()).collect(),
        other => panic!("inputs: {other:?}"),
    }
}

#[test]
fn the_round_acceptance_test() {
    let prod = ProdMesh::start(true);
    // 1. A fresh home (production defaults).
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let (par_hash, br_hash) = (
        fixture_receipt_hash("par_yield?date=2026-11-20&tenor=10Y"),
        fixture_receipt_hash("bank_rate?date=2026-11-20"),
    );

    // 2. The two acceptances.
    for l in [TREASURY_LICENCE, BANK_RATE_LICENCE] {
        let (code, out, err) = cli(&dir, &["licence", "accept", l]);
        assert_eq!(code, 0, "{out}\n{err}");
    }

    // 3. The spread, by calc.
    let (code, out, err) = cli(&dir, &["calc", FORMULA]);
    assert_eq!(code, 0, "{err}\nnot known to the fake: {:?}", prod.unknown());
    let c = parse(&out);
    assert_eq!(s(&c, "value"), "0.92", "{out}");
    let lineage: Vec<String> = match c.get("lineage") {
        Some(Value::Array(l)) => l.iter().map(|e| s(e, "receipt")).collect(),
        other => panic!("lineage: {other:?}"),
    };
    assert_eq!(lineage.len(), 2, "{out}");
    assert!(lineage.contains(&par_hash) && lineage.contains(&br_hash), "both input receipts: {lineage:?}");
    // The kept result: signed by this machine; its licence is the one restricted input's (above);
    // both input receipts are in its inputs.
    let (code, got, err) = cli(&dir, &["get", &format!("{}()", s(&c, "name"))]);
    assert_eq!(code, 0, "{err}");
    let rc = parse(&got);
    let rc = rc.get("receipt").unwrap();
    assert_eq!(s(rc, "value"), "0.92");
    assert_eq!(s(rc, "licence"), TREASURY_LICENCE);
    assert!(s(rc, "computed_by").starts_with("local.invalid#"), "{}", s(rc, "computed_by"));
    let ins = inputs_of(rc);
    assert!(ins.contains(&par_hash) && ins.contains(&br_hash), "{ins:?}");
    // Both inputs were fetched from the (fake) production mesh, by GET only.
    assert!(prod.posts().is_empty());

    // 4. A local function over the same inputs, with tests, then get.
    let (code, out, err) = cli(
        &dir,
        &["define", "local/spread", "--formula", SPREAD_SRC, "--signature", SPREAD_SIG, "--tests", SPREAD_TESTS],
    );
    assert_eq!(code, 0, "{out}\n{err}");
    let (code, out, err) = cli(&dir, &["get", "local/spread(date: 2026-11-20)"]);
    assert_eq!(code, 0, "{err}");
    let rc = parse(&out);
    let rc = rc.get("receipt").unwrap();
    assert_eq!(s(rc, "value"), "0.92");
    assert_eq!(s(rc, "licence"), TREASURY_LICENCE);
    let ins = inputs_of(rc);
    assert!(ins.contains(&par_hash) && ins.contains(&br_hash), "{ins:?}");

    // 5. What if Bank Rate were 4.5 (an MCP tool; nothing is sent anywhere).
    let srv = McpServer::new(Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap()));
    let v = tool(
        &srv,
        "whatif",
        &format!(
            r#"{{"request":"local/spread(date: 2026-11-20)","overrides":[{{"cell":{},"value":"4.5"}}]}}"#,
            q(BANK_RATE_CELL)
        ),
    );
    assert!(!is_error(&v), "{}", everything(&v));
    let sc = structured(&v);
    assert_eq!(s(sc, "outcome"), "scenario", "{}", jcs(sc));
    assert_eq!(s(sc, "value"), "0.42", "{}", jcs(sc));
    assert!(prod.posts().is_empty(), "computed here, on the terms of calc: {:?}", prod.posts());
    // The scenario is not a fact: the held answer is still 0.92.
    let (_, out, _) = cli(&dir, &["--offline", "get", "local/spread(date: 2026-11-20)"]);
    assert_eq!(s(parse(&out).get("receipt").unwrap(), "value"), "0.92");

    // 6. Export refuses: this machine signed a result under a restricted licence.
    let f = dir.path().join("bundle.json");
    let (code, out, err) = cli(&dir, &["export", "--out", f.to_str().unwrap()]);
    assert_ne!(code, 0, "{out}");
    assert!(err.contains("E_LICENCE_NOT_PUBLISHABLE") && err.contains(TREASURY_LICENCE), "{err}");
    assert!(!f.exists());
}

#[test]
fn whatif_over_accepted_inputs_needs_the_acceptance() {
    // D4.2: on the same terms as calc. Without the acceptance the scenario is refused, naming the
    // licence, and the agent is told the user must accept it.
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let (code, _, err) = cli(&dir, &["define", "local/spread", "--formula", SPREAD_SRC, "--signature", SPREAD_SIG]);
    assert_eq!(code, 0, "{err}");
    let srv = McpServer::new(Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap()));
    let v = tool(
        &srv,
        "whatif",
        &format!(
            r#"{{"request":"local/spread(date: 2026-11-20)","overrides":[{{"cell":{},"value":"4.5"}}]}}"#,
            q(BANK_RATE_CELL)
        ),
    );
    let all = agent_refusal(&v);
    assert!(
        all.contains(&format!(
            "the user must accept {TREASURY_LICENCE} at the command line: datumesh licence accept '{TREASURY_LICENCE}'"
        )),
        "{all}"
    );
}
