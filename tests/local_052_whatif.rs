//! datumesh-local 0.5.2, item 13: what-if in the CLI, and the local mesh's `POST /whatif`.
//!
//! * `datumesh whatif <request> --set '<cell>=<value>' [--set ...] [--asof <instant>] [--json]`
//!   prints the scenario as the MCP tool `whatif` does: the text's first line is
//!   `Scenario (not a fact): <call> = <value> as of <asof>.`, then one `  with <cell> = <value>` line
//!   per override; with `--json`, the structured answer (the tool's `structuredContent`). Exit 0 for
//!   a scenario, 3 for anything else (the code on stderr). The value is typed by the overridden
//!   cell's type.
//! * `datumesh serve`'s `POST /whatif` (SPEC 5.9's body) answers the same request with the same
//!   value as the MCP tool, for a local function over production inputs. 0.5.1 answers
//!   `E_EVAL_UNAVAILABLE`.
//!
//! The world is `local_051_acceptance.rs`'s: a fresh home on the production fake
//! (`tests/support/prod_mesh.rs`), the Treasury licence accepted, `local/spread` (10-year Treasury
//! yield minus Bank Rate, 0.92 on 2026-11-20). With Bank Rate at 4.5 it is 0.42.
//!
//! The scenario's `asof` is the evaluation's, so the CLI and the tool are compared on everything
//! else (the call, the type, the value and the overrides).

#![cfg(unix)]

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::McpServer;
use datumesh_local::serve::Server;
use datumesh_local::{Config, Resolver};
use prod_mesh::*;
use tools::{everything, is_error, items, jcs, q, structured, tool};

const SPREAD_SRC: &str = r#"datumesh.com/relay/treasury.gov/par_yield(tenor: "10Y", date: date) - datumesh.com/relay/bankofengland.co.uk/bank_rate(date: date)"#;
const SPREAD_SIG: &str = r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": {"type": "number", "unit": "percent"}}"#;
const SPREAD: &str = "local/spread(date: 2026-11-20)";
const SPREAD_FULL: &str = "local.invalid/spread(date: 2026-11-20)";
const BR_CELL: &str = "datumesh.com/relay/bankofengland.co.uk/bank_rate(date: 2026-11-20)";
/// A Bank Rate cell `local/spread(date: 2026-11-20)` does not read.
const NOT_AN_INPUT: &str = "datumesh.com/relay/bankofengland.co.uk/bank_rate(date: 2026-11-19)";

fn parse(out: &str) -> Value {
    json::parse(out.trim_end().as_bytes()).unwrap_or_else(|e| panic!("not JSON: {out}: {e:?}"))
}

/// A fresh home on the production fake, Treasury's licence accepted, `local/spread` defined.
fn world() -> (ProdMesh, TempDir) {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let (code, _, err) = cli(&dir, &["licence", "accept", TREASURY_LICENCE]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = cli(&dir, &["define", "local/spread", "--formula", SPREAD_SRC, "--signature", SPREAD_SIG]);
    assert_eq!(code, 0, "{err}\n{:?}", prod.unknown());
    (prod, dir)
}

fn mcp(dir: &TempDir) -> McpServer {
    McpServer::new(Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap()))
}

/// The MCP tool's answer to `request` with one value override, `asof` when given.
fn mcp_whatif(dir: &TempDir, request: &str, cell: &str, value: &str, asof: Option<&str>) -> Value {
    let asof = asof.map(|a| format!(r#","asof":{}"#, q(a))).unwrap_or_default();
    tool(
        &mcp(dir),
        "whatif",
        &format!(r#"{{"request":{},"overrides":[{{"cell":{},"value":{}}}]{asof}}}"#, q(request), q(cell), q(value)),
    )
}

/// The members a scenario is compared on (not its `asof`).
fn comparable(v: &Value) -> Vec<(String, Value)> {
    match v {
        Value::Object(m) => m.iter().filter(|(k, _)| k != "asof").cloned().collect(),
        other => panic!("an object: {other:?}"),
    }
}

/// The error code of a tool's error result.
fn tool_code(v: &Value) -> String {
    assert!(is_error(v), "{}", jcs(v));
    s(structured(v), "error")
}

// ------------------------------------------------------------------------------- the command

#[test]
fn whatif_with_bank_rate_at_4_5_gives_0_42_as_a_labelled_scenario() {
    let (prod, dir) = world();
    let set = format!("{BR_CELL}=4.5");
    let (code, out, err) = cli(&dir, &["whatif", SPREAD, "--set", &set]);
    assert_eq!(code, 0, "exit 0 for a scenario: {out}\n{err}\n{:?}", prod.unknown());
    let first = out.lines().next().unwrap_or_default();
    assert!(
        first.starts_with(&format!("Scenario (not a fact): {SPREAD_FULL} = 0.42 as of ")),
        "the tool's label line: {out}"
    );
    assert!(out.lines().any(|l| l == format!("  with {BR_CELL} = 4.5")), "the override is listed: {out}");
    // The same lines as the tool's text, but for the asof.
    let v = mcp_whatif(&dir, SPREAD, BR_CELL, "4.5", None);
    assert!(!is_error(&v), "{}", everything(&v));
    let tool_text = items(&v)[0].clone();
    let strip = |t: &str| t.lines().map(|l| l.split(" as of ").next().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(strip(out.trim_end()), strip(tool_text.trim_end()), "CLI:\n{out}\ntool:\n{tool_text}");
    // Nothing was sent anywhere, and the held answer is not changed by a scenario.
    assert!(prod.posts().is_empty(), "{:?}", prod.posts());
    let (code, got, err) = cli(&dir, &["get", SPREAD]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(s(parse(&got).get("receipt").unwrap(), "value"), "0.92");
}

#[test]
fn whatif_json_prints_the_structured_answer_the_tool_gives() {
    let (_prod, dir) = world();
    let set = format!("{BR_CELL}=4.5");
    let (code, out, err) = cli(&dir, &["whatif", SPREAD, "--set", &set, "--json"]);
    assert_eq!(code, 0, "{err}");
    let v = parse(&out);
    assert_eq!(s(&v, "outcome"), "scenario", "{out}");
    assert_eq!(s(&v, "value"), "0.42", "{out}");
    assert_eq!(s(&v, "type"), "number", "the value is typed by the cell's type: {out}");
    assert!(v.get("receipt").is_none() && v.get("trust").is_none(), "{out}");
    let t = mcp_whatif(&dir, SPREAD, BR_CELL, "4.5", None);
    assert_eq!(comparable(&v), comparable(structured(&t)), "CLI {out}\ntool {}", jcs(structured(&t)));
}

#[test]
fn several_sets_are_several_overrides() {
    let (_prod, dir) = world();
    let par = r#"datumesh.com/relay/treasury.gov/par_yield(date: 2026-11-20, tenor: "10Y")"#;
    let (code, out, err) =
        cli(&dir, &["whatif", SPREAD, "--set", &format!("{BR_CELL}=4.5"), "--set", &format!("{par}=5"), "--json"]);
    assert_eq!(code, 0, "{err}");
    let v = parse(&out);
    assert_eq!(s(&v, "value"), "0.5", "5 - 4.5: {out}");
    match v.get("overrides") {
        Some(Value::Array(o)) => assert_eq!(o.len(), 2, "{out}"),
        other => panic!("overrides: {other:?}"),
    }
}

#[test]
fn short_names_work_in_the_request_and_the_set() {
    let (_prod, dir) = world();
    let (code, out, err) = cli(&dir, &["whatif", SPREAD, "--set", "boe/bank_rate(2026-11-20)=4.5", "--json"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(s(&parse(&out), "value"), "0.42", "{out}");
}

#[test]
fn asof_is_passed_as_the_tool_takes_it() {
    let (_prod, dir) = world();
    let asof = "2026-11-24T12:00:00.000000Z";
    let set = format!("{BR_CELL}=4.5");
    let (code, out, err) = cli(&dir, &["whatif", SPREAD, "--set", &set, "--asof", asof, "--json"]);
    let t = mcp_whatif(&dir, SPREAD, BR_CELL, "4.5", Some(asof));
    if is_error(&t) {
        assert_eq!(code, 3, "{out}\n{err}");
        assert!(err.contains(&tool_code(&t)), "{err}\ntool: {}", everything(&t));
    } else {
        assert_eq!(code, 0, "{err}");
        let v = parse(&out);
        assert_eq!(comparable(&v), comparable(structured(&t)), "{out}");
        assert_eq!(s(&v, "asof"), s(structured(&t), "asof"), "{out}");
    }
}

#[test]
fn an_override_of_a_cell_that_is_not_an_input_is_answered_as_the_tool_answers_it() {
    let (_prod, dir) = world();
    let set = format!("{NOT_AN_INPUT}=4.5");
    let (code, out, err) = cli(&dir, &["whatif", SPREAD, "--set", &set, "--json"]);
    let t = mcp_whatif(&dir, SPREAD, NOT_AN_INPUT, "4.5", None);
    if is_error(&t) {
        assert_eq!(code, 3, "not a scenario: exit 3: {out}\n{err}");
        assert!(err.contains(&tool_code(&t)), "the tool's code: {err}\ntool: {}", everything(&t));
    } else {
        assert_eq!(code, 0, "{err}");
        assert_eq!(comparable(&parse(&out)), comparable(structured(&t)), "{out}\ntool: {}", jcs(structured(&t)));
    }
}

#[test]
fn a_value_that_does_not_fit_the_cell_s_type_exits_3_with_the_tool_s_code() {
    let (_prod, dir) = world();
    let set = format!("{BR_CELL}=four and a half");
    let (code, out, err) = cli(&dir, &["whatif", SPREAD, "--set", &set]);
    assert_eq!(code, 3, "{out}\n{err}");
    let t = mcp_whatif(&dir, SPREAD, BR_CELL, "four and a half", None);
    let c = tool_code(&t);
    assert!(err.contains(&c), "{c}: {err}");
    assert!(out.is_empty(), "no scenario printed: {out}");
}

#[test]
fn whatif_without_a_set_or_with_a_set_without_a_value_is_a_usage_error() {
    let (_prod, dir) = world();
    let (code, _, err) = cli(&dir, &["whatif", SPREAD]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = cli(&dir, &["whatif", SPREAD, "--set", BR_CELL]);
    assert_eq!(code, 2, "`<cell>=<value>`: {err}");
    let (_, help, _) = cli(&dir, &["--help"]);
    assert!(help.contains("whatif <request> --set"), "the usage lists whatif: {help}");
}

#[test]
fn without_the_acceptance_whatif_is_refused_naming_the_licence() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let (code, _, err) = cli(&dir, &["define", "local/spread", "--formula", SPREAD_SRC, "--signature", SPREAD_SIG]);
    assert_eq!(code, 0, "{err}");
    let set = format!("{BR_CELL}=4.5");
    let (code, out, err) = cli(&dir, &["whatif", SPREAD, "--set", &set]);
    assert_eq!(code, 3, "{out}\n{err}");
    assert!(err.contains(TREASURY_LICENCE) && err.contains("licence accept"), "{err}");
}

// ------------------------------------------------------------------------------- serve

/// POST over HTTP/1.0: status and body.
fn post(addr: SocketAddr, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(
        s,
        "POST {path} HTTP/1.0\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .unwrap();
    s.write_all(body).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let status = head.lines().next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    (status, buf[split + 4..].to_vec())
}

/// Runs `f` against `datumesh serve` over the home, then stops it.
fn serving(dir: &TempDir, f: impl FnOnce(SocketAddr)) {
    let r = Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap());
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let server = rt.block_on(Server::bind(r, "127.0.0.1:0".parse().unwrap())).unwrap();
    let addr = server.local_addr();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = rt.spawn(server.run(async {
        let _ = rx.await;
    }));
    f(addr);
    let _ = tx.send(());
    rt.block_on(handle).unwrap().unwrap();
}

/// A typed scalar's lexical form, or a plain string as it is.
fn lexical(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(t)) => t.clone(),
        Some(o @ Value::Object(_)) => s(o, "value"),
        other => panic!("a value: {other:?}"),
    }
}

/// SPEC 5.9's body: the cell, and one value override, typed.
fn body(cell: &str, over: &str, value: &str) -> Vec<u8> {
    format!(
        r#"{{"cell":{},"overrides":[{{"cell":{},"value":{{"type":"number","value":{}}}}}]}}"#,
        q(cell),
        q(over),
        q(value)
    )
    .into_bytes()
}

#[test]
fn serve_post_whatif_gives_the_tool_s_value_for_a_local_function_over_production_inputs() {
    let (prod, dir) = world();
    let t = mcp_whatif(&dir, SPREAD, BR_CELL, "4.5", None);
    assert!(!is_error(&t), "{}", everything(&t));
    assert_eq!(s(structured(&t), "value"), "0.42");
    serving(&dir, |addr| {
        let (status, bytes) = post(addr, "/whatif", &body(SPREAD_FULL, BR_CELL, "4.5"));
        let text = String::from_utf8_lossy(&bytes).into_owned();
        assert_eq!(status, 200, "{text}\n{:?}", prod.unknown());
        let v = parse(&text);
        assert_eq!(s(&v, "outcome"), "scenario", "{text}");
        let sc = v.get("scenario").unwrap_or_else(|| panic!("a scenario: {text}"));
        assert_eq!(lexical(sc.get("value")), s(structured(&t), "value"), "{text}");
        assert!(v.get("receipt").is_none() && sc.get("receipt").is_none(), "{text}");
        assert!(String::from_utf8_lossy(&sc.to_jcs()).contains("bank_rate"), "its override: {text}");
    });
    assert!(prod.posts().is_empty(), "computed here: {:?}", prod.posts());
}

#[test]
fn serve_post_whatif_of_a_cell_that_is_not_an_input_answers_as_the_tool() {
    let (_prod, dir) = world();
    let t = mcp_whatif(&dir, SPREAD, NOT_AN_INPUT, "4.5", None);
    serving(&dir, |addr| {
        let (status, bytes) = post(addr, "/whatif", &body(SPREAD_FULL, NOT_AN_INPUT, "4.5"));
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let v = parse(&text);
        assert_ne!(s(&v, "error"), "E_EVAL_UNAVAILABLE", "{text}");
        if is_error(&t) {
            assert_ne!(status, 200, "{text}");
            assert_eq!(s(&v, "error"), tool_code(&t), "{text}\ntool: {}", everything(&t));
        } else {
            assert_eq!(status, 200, "{text}");
            let sc = v.get("scenario").unwrap_or_else(|| panic!("a scenario: {text}"));
            assert_eq!(lexical(sc.get("value")), s(structured(&t), "value"), "{text}");
        }
    });
}
