//! The 0.7 round (CONTRACT-round-0.7.md R2, SPEC 9.7, 16.3), local: **a name can be closed.** Failing tests: local does not say
//! it yet, and the mesh's close line is http 0.5.0's.
//!
//! * `describe` (the MCP tool and the command) of a closed name says it is unbound (`E_NAME_UNBOUND`), a function's and a
//!   class's; before the close the same call describes it (the control).
//! * `get` of a cell of a closed function is `not_covered` with `E_NAME_UNBOUND` from a fresh resolver; the receipts it held
//!   before still verify.
//!
//! The mesh is the test upstream with a later run (`support/r07.rs`) whose line is `{"close": "<full name>"}`.

mod common;
#[path = "support/r07.rs"]
mod r07;

use std::sync::Arc;

use common::*;
use datumesh_core::json::Value;
use datumesh_local::mcp::McpServer;
use datumesh_local::{Config, Resolver};
use r07::*;

const FX_FN: &str = "mesh-test.datumesh.com/ecb/fx_rate";

fn server(dir: &TempDir, up: &Upstream) -> McpServer {
    let c: Config = config(dir, up);
    McpServer::new(Arc::new(Resolver::open(c).unwrap()))
}

fn describe(srv: &McpServer, id: &str) -> Value {
    tool(srv, "describe", &format!(r#"{{"id":{}}}"#, jstr(id)))
}

fn upstream_fn_closed() -> Upstream {
    let up = upstream();
    publish_at(&up, T_WITHDRAW, &[close_line(FX_FN)]);
    up
}

fn upstream_class_closed() -> Upstream {
    let up = upstream_classes();
    publish_at(&up, T_WITHDRAW, &[close_line(NAMED_CLASS)]);
    up
}

/// A text item or the structured answer says the name is unbound.
fn assert_unbound(v: &Value, name: &str) {
    let all = format!("{}\n{}", items(v).join("\n"), jcs(structured(v)));
    assert!(all.to_lowercase().contains("unbound"), "says the name is unbound: {all}");
    assert!(all.contains(name), "and names it: {all}");
    assert_eq!(structured(v).get("status").and_then(Value::as_number_text), Some("404"), "{all}");
    assert!(all.contains("E_NAME_UNBOUND"), "{all}");
}

#[test]
fn mcp_describe_of_a_closed_function_says_it_is_unbound() {
    let up = upstream_fn_closed();
    let dir = TempDir::new("r07-close-fn");
    let v = describe(&server(&dir, &up), FX_FN);
    assert_unbound(&v, FX_FN);
}

#[test]
fn mcp_describe_of_a_closed_class_says_it_is_unbound() {
    let up = upstream_class_closed();
    let dir = TempDir::new("r07-close-class");
    let srv = server(&dir, &up);
    assert_unbound(&describe(&srv, NAMED_CLASS), NAMED_CLASS);
    // the class's methods were closed with it
    let method = format!("{NAMED_CLASS}/rate");
    assert_unbound(&describe(&srv, &method), &method);
    // the other classes are untouched
    let other = describe(&srv, PAIR_CLASS);
    assert_eq!(structured(&other).get("status").and_then(Value::as_number_text), Some("200"), "{}", jcs(&other));
}

/// The control: the same describe on a mesh that closed nothing.
#[test]
fn mcp_describe_of_an_open_name_is_unchanged() {
    let up = upstream();
    let dir = TempDir::new("r07-close-control");
    let v = describe(&server(&dir, &up), FX_FN);
    assert_eq!(structured(&v).get("status").and_then(Value::as_number_text), Some("200"), "{}", jcs(&v));
}

#[test]
fn cli_describe_of_a_closed_name_says_it_is_unbound() {
    let up = upstream_fn_closed();
    let dir = TempDir::new("r07-close-cli");
    Config::in_dir(dir.path()).unwrap();
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("\"peers\": {}", &format!("\"peers\": {{\"{HOST}\": \"{}\"}}", up.origin)))
        .unwrap();
    let args = vec!["--home".to_string(), dir.path().to_string_lossy().into_owned(), "describe".into(), FX_FN.into()];
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&args, &mut out, &mut err);
    let (out, err) = (String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap());
    assert_eq!(code, 3, "{err}");
    assert!(format!("{out} {err}").to_lowercase().contains("unbound"), "says the name is unbound: {out} {err}");
}

/// A read of a cell of a closed function is `not_covered` with `E_NAME_UNBOUND`.
#[test]
fn get_of_a_cell_of_a_closed_name_is_not_covered_and_unbound() {
    let up = upstream_fn_closed();
    let dir = TempDir::new("r07-close-get");
    let r = resolver(&dir, &up);
    let reply = r.get(USD_FULL).unwrap();
    let env = reply.envelope().unwrap();
    assert_eq!(
        (reply.status, s(&env, "outcome"), s(&env, "error")),
        (404, "not_covered".to_string(), "E_NAME_UNBOUND".to_string()),
        "{}",
        jcs(&env)
    );
}

/// Receipts held from before the close still verify: read first, then close, then verify the held receipt.
#[test]
fn a_receipt_held_from_before_the_close_still_verifies() {
    let up = upstream();
    let dir = TempDir::new("r07-close-verify");
    let r = resolver(&dir, &up);
    let held = r.get(USD_FULL).unwrap();
    assert_eq!(held.status, 200, "{}", String::from_utf8_lossy(&held.body));
    let receipt = held.envelope().unwrap().get("receipt").cloned().expect("a receipt");
    publish_at(&up, T_WITHDRAW, &[close_line(FX_FN)]);
    let v = r.verify(&receipt.to_jcs());
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", jcs(&v));
}
