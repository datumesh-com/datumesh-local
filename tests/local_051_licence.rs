//! datumesh-local 0.5.1, L4: licence acceptance (`datumesh licence accept|list|revoke`,
//! `licences.json`, the gate, the refusals and the MCP server, which has no way to accept).
//! The production mesh is a fake serving captured responses (`tests/support/prod_mesh.rs`).

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::McpServer;
use datumesh_local::{Config, Resolver};
use prod_mesh::*;
use tools::{everything, is_error, result, rpc_on, tool};

const SPREAD: &str = "datumesh.com/relay/treasury.gov/par_yield(10Y, 2026-11-20) - datumesh.com/relay/bankofengland.co.uk/bank_rate(2026-11-20)";
/// Two restricted inputs (Treasury and ECB): the product of a yield and an exchange rate.
const TWO_RESTRICTED: &str = "datumesh.com/relay/treasury.gov/par_yield(10Y, 2026-11-20) * datumesh.com/relay/ecb.europa.eu/fx_reference(USD, 2026-11-20)";

fn parse(out: &str) -> Value {
    json::parse(out.trim_end().as_bytes()).unwrap_or_else(|e| panic!("not JSON: {out}: {e:?}"))
}

fn accept(dir: &TempDir, licence: &str) -> (String, String) {
    let (code, out, err) = cli(dir, &["licence", "accept", licence]);
    assert_eq!(code, 0, "{out}\n{err}");
    (out, err)
}

fn licences_json(dir: &TempDir) -> Value {
    let p = dir.path().join("licences.json");
    json::parse(&std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

/// The records of licences.json with a member `key` (any file shape: the records are found
/// wherever they are).
fn records<'a>(v: &'a Value, key: &str, licence: &str) -> Vec<&'a Value> {
    let mut all = Vec::new();
    objects_with(v, key, &mut all);
    all.into_iter().filter(|r| r.get("licence").and_then(Value::as_str) == Some(licence)).collect()
}

fn world() -> (ProdMesh, TempDir) {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    (prod, dir)
}

// ------------------------------------------------------------------------------- accept / list

#[test]
fn accept_prints_the_licence_the_registers_clauses_page_and_hash_and_records_them() {
    let (prod, dir) = world();
    let (out, err) = accept(&dir, TREASURY_LICENCE);
    let all = format!("{out}\n{err}");
    assert!(all.contains(TREASURY_LICENCE), "{all}");
    assert!(all.contains(TREASURY_CLAUSE), "the register's quoted clauses: {all}");
    assert!(all.contains(TREASURY_REGISTER_HASH), "the register's licence hash: {all}");
    assert!(all.contains("US Treasury daily par yield curve rates") || all.contains("par_yield_curve"), "the dataset: {all}");
    // The licence is printed first, then what the register says of it.
    let stream = if out.contains(TREASURY_CLAUSE) { &out } else { &err };
    assert!(stream.find(TREASURY_LICENCE).unwrap() < stream.find(TREASURY_CLAUSE).unwrap(), "{stream}");
    // The register is read on the forward mesh (datumesh.com), by GET.
    assert!(prod.targets().iter().any(|t| t.contains("register/dataset")), "{:?}", prod.targets());
    assert!(prod.posts().is_empty());
    // Recorded in licences.json: licence, canonical accepted_at, the register's licence hash.
    let v = licences_json(&dir);
    let rec = records(&v, "accepted_at", TREASURY_LICENCE);
    assert_eq!(rec.len(), 1, "{}", String::from_utf8_lossy(&v.to_jcs()));
    assert!(is_canonical_instant(&s(rec[0], "accepted_at")), "{}", s(rec[0], "accepted_at"));
    assert_eq!(s(rec[0], "register_licence_hash"), TREASURY_REGISTER_HASH);
    // list shows it, with its time.
    let (code, listed, err) = cli(&dir, &["licence", "list"]);
    assert_eq!(code, 0, "{err}");
    assert!(listed.contains(TREASURY_LICENCE) && listed.contains(&s(rec[0], "accepted_at")), "{listed}");
}

#[test]
fn accept_shows_every_register_dataset_under_the_licence() {
    let (_prod, dir) = world();
    let (out, err) = accept(&dir, ECB_LICENCE);
    let all = format!("{out}\n{err}");
    // The ECB's disclaimer page is the licence page of two datasets.
    assert!(all.contains("policy_rates") && all.contains("reference_rates"), "{all}");
    assert!(all.contains(ECB_REGISTER_HASH), "{all}");
    let v = licences_json(&dir);
    let rec = records(&v, "accepted_at", ECB_LICENCE);
    assert_eq!(rec.len(), 1);
    assert_eq!(s(rec[0], "register_licence_hash"), ECB_REGISTER_HASH);
}

#[test]
fn a_missing_register_entry_is_said_and_the_acceptance_still_records() {
    let (_prod, dir) = world();
    // Bank Rate's receipts carry the SPDX id OGL-UK-3.0; no register dataset has it as its page.
    let (out, err) = accept(&dir, BANK_RATE_LICENCE);
    let all = format!("{out}\n{err}");
    assert!(all.contains(BANK_RATE_LICENCE), "{all}");
    let lower = all.to_lowercase();
    assert!(lower.contains("register"), "the register is named: {all}");
    assert!(
        ["no register", "not in the register", "no entry", "not found", "not listed", "no dataset", "has no", "holds no", "does not"]
            .iter()
            .any(|p| lower.contains(p)),
        "the missing entry is said plainly: {all}"
    );
    let v = licences_json(&dir);
    let rec = records(&v, "accepted_at", BANK_RATE_LICENCE);
    assert_eq!(rec.len(), 1);
    assert!(rec[0].get("register_licence_hash").is_none_or(|h| matches!(h, Value::Null)), "no hash to record");
    // An https licence the register does not hold, too.
    let (_, err) = accept(&dir, "https://example.org/some/licence");
    assert!(!err.is_empty() || records(&licences_json(&dir), "accepted_at", "https://example.org/some/licence").len() == 1);
    assert_eq!(records(&licences_json(&dir), "accepted_at", "https://example.org/some/licence").len(), 1);
}

#[test]
fn revoke_removes_from_force_and_keeps_the_history() {
    let (_prod, dir) = world();
    accept(&dir, TREASURY_LICENCE);
    accept(&dir, ECB_LICENCE);
    let (code, out, err) = cli(&dir, &["licence", "revoke", TREASURY_LICENCE]);
    assert_eq!(code, 0, "{out}\n{err}");
    let (code, listed, err) = cli(&dir, &["licence", "list"]);
    assert_eq!(code, 0, "{err}");
    assert!(!listed.contains(TREASURY_LICENCE), "revoked licences are not in force: {listed}");
    assert!(listed.contains(ECB_LICENCE), "{listed}");
    let v = licences_json(&dir);
    // The acceptance is still in the file, and the revocation is appended with its time.
    assert_eq!(records(&v, "accepted_at", TREASURY_LICENCE).len(), 1, "history is kept");
    let rev = records(&v, "revoked_at", TREASURY_LICENCE);
    assert_eq!(rev.len(), 1, "{}", String::from_utf8_lossy(&v.to_jcs()));
    assert!(is_canonical_instant(&s(rev[0], "revoked_at")));
    // Accepting again puts it back in force, and adds to the history.
    accept(&dir, TREASURY_LICENCE);
    let (_, listed, _) = cli(&dir, &["licence", "list"]);
    assert!(listed.contains(TREASURY_LICENCE), "{listed}");
    assert_eq!(records(&licences_json(&dir), "accepted_at", TREASURY_LICENCE).len(), 2);
}

#[test]
fn an_empty_list_is_an_empty_list() {
    let (_prod, dir) = world();
    let (code, out, err) = cli(&dir, &["licence", "list"]);
    assert_eq!(code, 0, "{err}");
    assert!(!out.contains("http") && !out.contains("OGL"), "{out}");
}

// ------------------------------------------------------------------------ the gate and refusals

#[test]
fn without_an_acceptance_the_refusal_stays_unlicensed_and_names_the_command() {
    let (prod, dir) = world();
    let (code, out, err) = cli(&dir, &["calc", SPREAD]);
    assert_eq!(code, 1, "{out}");
    assert!(err.contains("unlicensed"), "{err}");
    assert!(err.contains(TREASURY_LICENCE), "the licence: {err}");
    assert!(err.contains(&format!("datumesh licence accept '{TREASURY_LICENCE}'")), "the command: {err}");
    assert!(!dir.path().join("licences.json").exists() || !licences_json(&dir).to_jcs().windows(5).any(|w| w == b"https"));
    assert!(prod.posts().is_empty());
}

#[test]
fn one_of_two_restricted_licences_accepted_refuses_naming_the_missing_one() {
    let (_prod, dir) = world();
    accept(&dir, TREASURY_LICENCE);
    let (code, out, err) = cli(&dir, &["calc", TWO_RESTRICTED]);
    assert_eq!(code, 1, "{out}");
    assert!(err.contains("unlicensed"), "{err}");
    assert!(err.contains(&format!("datumesh licence accept '{ECB_LICENCE}'")), "the missing one: {err}");
    assert!(!err.contains(&format!("datumesh licence accept '{TREASURY_LICENCE}'")), "the accepted one is not asked for again: {err}");
    // Accepting the second computes it; two restricted licences make the derivation LicenseRef.
    accept(&dir, ECB_LICENCE);
    let (code, out, err) = cli(&dir, &["calc", TWO_RESTRICTED]);
    assert_eq!(code, 0, "{err}");
    let v = parse(&out);
    assert_eq!(s(&v, "value"), "5.40708");
    let (code, got, err) = cli(&dir, &["get", &format!("{}()", s(&v, "name"))]);
    assert_eq!(code, 0, "{err}");
    let rc = parse(&got);
    let rc = rc.get("receipt").unwrap();
    assert_eq!(s(rc, "licence"), "LicenseRef-datumesh-local-derivation");
    assert!(s(rc, "computed_by").starts_with("local.invalid#"), "signed by this machine's key");
}

#[test]
fn revoking_puts_the_refusal_back() {
    let (_prod, dir) = world();
    accept(&dir, TREASURY_LICENCE);
    accept(&dir, BANK_RATE_LICENCE);
    let (code, _, err) = cli(&dir, &["calc", SPREAD]);
    assert_eq!(code, 0, "{err}");
    let (code, _, _) = cli(&dir, &["licence", "revoke", TREASURY_LICENCE]);
    assert_eq!(code, 0);
    let (code, _, err) = cli(&dir, &["calc", "datumesh.com/relay/treasury.gov/par_yield(10Y, 2026-11-20) - datumesh.com/relay/bankofengland.co.uk/bank_rate(2026-11-19)"]);
    assert_ne!(code, 0);
    assert!(err.contains("unlicensed") && err.contains(&format!("datumesh licence accept '{TREASURY_LICENCE}'")), "{err}");
}

#[test]
fn an_acceptance_covers_the_licence_from_every_publisher() {
    // Scope (D4): the licence, not the publisher. ECB's licence accepted covers the ECB's other
    // datasets under the same page: here the same function again under a second call.
    let (_prod, dir) = world();
    accept(&dir, ECB_LICENCE);
    let (code, out, err) = cli(&dir, &["calc", "datumesh.com/relay/ecb.europa.eu/fx_reference(USD, 2026-11-20) * 2"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(s(&parse(&out), "value"), "2.198");
}

// ---------------------------------------------------------------------------- the MCP server

fn server(dir: &TempDir) -> McpServer {
    let cfg = Config::in_dir(dir.path()).unwrap();
    McpServer::new(Arc::new(Resolver::open(cfg).unwrap()))
}

#[test]
fn no_mcp_tool_accepts_a_licence() {
    let (_prod, dir) = world();
    let srv = server(&dir);
    let v = rpc_on(&srv, datumesh_local::mcp::Transport::Stdio, "tools/list", "");
    let names: Vec<String> = match result(&v).get("tools") {
        Some(Value::Array(t)) => t.iter().map(|t| s(t, "name")).collect(),
        other => panic!("{other:?}"),
    };
    for n in &names {
        let l = n.to_lowercase();
        assert!(!l.contains("licen") && !l.contains("accept") && !l.contains("consent"), "{n}");
    }
    // No tool takes a licence to accept: none has a property that names one.
    let text = String::from_utf8(result(&v).to_jcs()).unwrap();
    assert!(!text.contains("\"accept"), "{text}");
}

#[test]
fn the_agents_refusal_names_the_licence_and_says_the_user_must_accept_it() {
    let (_prod, dir) = world();
    let srv = server(&dir);
    let v = tool(&srv, "calc", &format!(r#"{{"formula":{}}}"#, tools::q(SPREAD)));
    let all = agent_refusal(&v);
    assert!(
        all.contains(&format!(
            "the user must accept {TREASURY_LICENCE} at the command line: datumesh licence accept '{TREASURY_LICENCE}'"
        )),
        "{all}"
    );
    // It is not a way round: calling `get` over a local function does not accept either.
    assert!(!dir.path().join("licences.json").exists() || records(&licences_json(&dir), "accepted_at", TREASURY_LICENCE).is_empty());
}

#[test]
fn a_running_server_picks_up_an_acceptance_made_at_the_command_line() {
    let (_prod, dir) = world();
    let srv = server(&dir);
    let args = format!(r#"{{"formula":{}}}"#, tools::q(SPREAD));
    let _ = agent_refusal(&tool(&srv, "calc", &args));
    // The user accepts in another process (here, another command); the file's mtime moves.
    std::thread::sleep(Duration::from_millis(1100));
    accept(&dir, TREASURY_LICENCE);
    let v = tool(&srv, "calc", &args);
    assert!(!is_error(&v), "{}", everything(&v));
    assert_eq!(s(tools::structured(&v), "value"), "0.92", "{}", everything(&v));
    // And a revocation is picked up the same way.
    std::thread::sleep(Duration::from_millis(1100));
    let (code, _, err) = cli(&dir, &["licence", "revoke", TREASURY_LICENCE]);
    assert_eq!(code, 0, "{err}");
    let other = format!(r#"{{"formula":{}}}"#, tools::q("datumesh.com/relay/treasury.gov/par_yield(10Y, 2026-11-20) - datumesh.com/relay/bankofengland.co.uk/bank_rate(2026-11-19)"));
    let _ = agent_refusal(&tool(&srv, "calc", &other));
}

// ------------------------------------------------------------- forwarding keeps its precedence

#[test]
fn a_declared_restricted_input_is_still_forwarded_when_its_licence_is_accepted() {
    let up = upstream();
    let dir = old_home(&[(HOST, &up.origin)]);
    let nok = r#"mesh-test.datumesh.com/ecb/fx_cross(base: "USD", quote: "NOK", date: 2026-10-30)"#;
    let (code, out, err) = cli(&dir, &["get", nok]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(s(parse(&out).get("receipt").unwrap(), "licence"), RESTRICTED);
    accept(&dir, RESTRICTED);
    // Forwarded as today: the answer is the mesh's, signed by the mesh.
    let (code, out2, err) = cli(&dir, &["get", nok]);
    assert_eq!(code, 0, "{err}");
    assert!(s(parse(&out2).get("receipt").unwrap(), "computed_by").starts_with("mesh-test.datumesh.com#"));
    // A local function over it is still refused (the declared input decides Forward, which a
    // local function cannot use): the acceptance does not change the order of the gate.
    let sig = r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": {"type": "number", "unit": {"of": "NOK", "per": "USD"}}}"#;
    let (code, _, err) = cli(
        &dir,
        &["define", "local/nok_twice", "--formula", &format!(r#"mesh-test.datumesh.com/ecb/fx_cross(base: "USD", quote: "NOK", date: date) * 2"#), "--signature", sig],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = cli(&dir, &["get", "local/nok_twice(date: 2026-10-30)"]);
    assert_ne!(code, 0, "{out}");
    assert!(format!("{out}{err}").contains("unlicensed"), "{out}{err}");
}
