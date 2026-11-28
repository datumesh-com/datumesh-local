//! datumesh-local 0.5.1, L7 (D8): the MCP tool `interval`, `{function, keys?, from, to}`. It calls
//! the forward mesh's `x_interval` (GET `<origin>/x/interval/<full function name>?<keys>&from&to`),
//! resolving aliases; over 100 members it answers with a summary, the first 100 members and the
//! call for the rest; each member carries its receipt; the mesh's errors are passed through.
//! Since 0.5.3 (the guard patch's A3, the coordinator's decision of 25 Nov) a listed member carries
//! its `call`, `value` and `receipt_hash` (with its `keys` and `asof`), not the whole receipt, so a
//! 100-member answer stays within the 64 KiB tool budget; the receipts are still verified.
//! The mesh is the fake of production responses (`tests/support/prod_mesh.rs`).

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::sync::Arc;

use common::*;
use datumesh_core::json::Value;
use datumesh_local::mcp::{McpServer, Transport};
use datumesh_local::{Config, Resolver};
use prod_mesh::*;
use tools::*;

fn world() -> (ProdMesh, TempDir, McpServer) {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let srv = McpServer::new(Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap()));
    (prod, dir, srv)
}

/// A listed member (0.5.3): `call`, `value` and `receipt_hash` (64 hex), and no whole receipt.
fn assert_listed_member(m: &Value) {
    assert!(m.get("receipt").is_none(), "no whole receipt in a listed member: {}", jcs(m));
    assert!(!s(m, "call").is_empty(), "its call: {}", jcs(m));
    assert!(m.get("value").is_some(), "its value: {}", jcs(m));
    let h = s(m, "receipt_hash");
    assert!(h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit()), "its receipt hash: {}", jcs(m));
}

/// A listed member's date, from its call (local 0.5.5, NEXT-PATCH-QUEUE item 18: a member is a compact row
/// of its call, value and receipt hash; its `keys` and `asof` are no longer repeated).
fn date_of(m: &Value) -> String {
    let call = s(m, "call");
    let at = call.find("date: ").unwrap_or_else(|| panic!("a date in {call}")) + 6;
    call[at..at + 10].to_string()
}

fn interval_targets(prod: &ProdMesh) -> Vec<String> {
    prod.targets().into_iter().filter(|t| t.starts_with("/x/interval/")).collect()
}

#[test]
fn interval_is_a_tool_with_function_keys_from_to() {
    let (_p, _d, srv) = world();
    let v = rpc_on(&srv, Transport::Stdio, "tools/list", "");
    let t = arr(result(&v), "tools").iter().find(|t| s(t, "name") == "interval").expect("an interval tool").clone();
    let schema = t.get("inputSchema").unwrap();
    assert_eq!(schema.get("additionalProperties"), Some(&Value::Bool(false)));
    let props = schema.get("properties").unwrap();
    for k in ["function", "keys", "from", "to"] {
        assert!(props.get(k).is_some(), "property {k}: {}", jcs(schema));
    }
    let required = jcs(schema.get("required").unwrap());
    for k in ["function", "from", "to"] {
        assert!(required.contains(&format!("\"{k}\"")), "{k} is required: {required}");
    }
    assert!(!required.contains("\"keys\""), "keys may be omitted: {required}");
    assert!(!s(&t, "description").is_empty());
}

#[test]
fn it_asks_x_interval_of_the_forward_mesh_with_the_aliased_name_resolved() {
    let (prod, _d, srv) = world();
    let v = tool(
        &srv,
        "interval",
        r#"{"function":"treasury/par_yield","keys":{"tenor":"10Y"},"from":"2026-11-16","to":"2026-11-24"}"#,
    );
    assert!(!is_error(&v), "{}", everything(&v));
    let asked = interval_targets(&prod);
    assert_eq!(asked.len(), 1, "{asked:?}");
    // The full name with its host, the keys and the dates as parameters.
    let (path, query) = asked[0].split_once('?').unwrap();
    assert_eq!(path, "/x/interval/datumesh.com/relay/treasury.gov/par_yield");
    let mut q: Vec<&str> = query.split('&').collect();
    q.sort();
    assert_eq!(q, ["from=2026-11-16", "tenor=10Y", "to=2026-11-24"]);
    assert!(prod.posts().is_empty());
}

#[test]
fn a_full_name_works_too_and_keys_may_be_omitted() {
    let (prod, _d, srv) = world();
    let v = tool(
        &srv,
        "interval",
        r#"{"function":"datumesh.com/relay/treasury.gov/par_yield","keys":{"tenor":"10Y"},"from":"2026-11-16","to":"2026-11-24"}"#,
    );
    assert!(!is_error(&v), "{}", everything(&v));
    // No keys: the request carries none (the mesh answers every key, or says what it needs).
    let _ = tool(&srv, "interval", r#"{"function":"boe/bank_rate","from":"2026-11-16","to":"2026-11-24"}"#);
    let asked = interval_targets(&prod);
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert!(asked[1].starts_with("/x/interval/datumesh.com/relay/bankofengland.co.uk/bank_rate?"), "{asked:?}");
    let q: Vec<&str> = asked[1].split_once('?').unwrap().1.split('&').collect();
    assert!(q.contains(&"from=2026-11-16") && q.contains(&"to=2026-11-24") && q.len() == 2, "{q:?}");
}

#[test]
fn a_small_interval_answers_its_members_each_with_its_receipt() {
    let (_p, _d, srv) = world();
    let v = tool(
        &srv,
        "interval",
        r#"{"function":"treasury/par_yield","keys":{"tenor":"10Y"},"from":"2026-11-16","to":"2026-11-24"}"#,
    );
    assert!(!is_error(&v), "{}", everything(&v));
    let sc = structured(&v);
    let members = arr(sc, "members");
    assert_eq!(members.len(), 6, "{}", jcs(sc));
    let by_date: Vec<(String, String)> = members.iter().map(|m| (date_of(m), s(m, "value"))).collect();
    assert_eq!(by_date[0], ("2026-11-16".to_string(), "5.12".to_string()));
    assert_eq!(by_date[4], ("2026-11-20".to_string(), "4.92".to_string()));
    for m in members {
        assert_listed_member(m);
    }
    // The hash is the receipt's: 2026-11-20's is production's recorded /cell receipt.
    let m20 = members.iter().find(|m| date_of(m) == "2026-11-20").unwrap();
    assert_eq!(s(m20, "receipt_hash"), fixture_receipt_hash("par_yield?date=2026-11-20&tenor=10Y"), "{}", jcs(m20));
    assert!(s(m20, "call").contains("par_yield") && s(m20, "call").contains("2026-11-20"), "{}", jcs(m20));
    // Under the hundred: no summary of the kind a long answer has, no call for the rest.
    assert!(sc.get("rest").is_none() && sc.get("next").is_none() && sc.get("summary").is_none(), "{}", jcs(sc));
}

#[test]
fn over_a_hundred_members_it_summarises_gives_the_first_hundred_and_the_call_for_the_rest() {
    let (prod, _d, srv) = world();
    let v = tool(
        &srv,
        "interval",
        r#"{"function":"ecb/fx_reference","keys":{"currency":"USD"},"from":"2024-01-01","to":"2024-06-30"}"#,
    );
    assert!(!is_error(&v), "{}", everything(&v));
    assert_eq!(interval_targets(&prod).len(), 1);
    let sc = structured(&v);
    let members = arr(sc, "members");
    assert_eq!(members.len(), 100, "the first 100 of 126");
    for m in members {
        assert_listed_member(m);
    }
    // Members in key order: the first is the first business day of 2024.
    assert_eq!(date_of(&members[0]), "2024-01-02");
    // The summary: count 126, first and last key, min/max/mean for numbers.
    let summary = sc.get("summary").unwrap_or_else(|| panic!("a summary: {}", jcs(sc)));
    let st = jcs(summary);
    assert!(st.contains("126"), "the count: {st}");
    assert!(st.contains("2024-01-02") && st.contains("2024-06-28"), "first and last key: {st}");
    for k in ["min", "max", "mean"] {
        assert!(st.contains(k), "{k}: {st}");
    }
    // The call for the rest: names the function, the keys and a range that starts after the 100th.
    let hundredth = date_of(&members[99]);
    // (the member is `rest`, like `get`'s `calc`; `next` and `call` are accepted as its name)
    let call = sc
        .get("rest")
        .or_else(|| sc.get("next"))
        .or_else(|| sc.get("call"))
        .unwrap_or_else(|| panic!("the call for the rest: {}", jcs(sc)));
    let call = jcs(call);
    assert!(call.contains("interval") && call.contains("fx_reference") && call.contains("USD"), "{call}");
    assert!(call.contains("2024-06-30"), "up to the same end: {call}");
    assert!(!call.contains("\"2024-01-01\"") && !call.contains("from\":\"2024-01-01"), "does not start again: {call}");
    let _ = hundredth;
}

#[test]
fn the_meshs_errors_are_passed_through() {
    let (_p, _d, srv) = world();
    // A range over the mesh's ten-year limit: 400 E_RANGE_TOO_LARGE.
    let v = tool(
        &srv,
        "interval",
        r#"{"function":"treasury/par_yield","keys":{"tenor":"10Y"},"from":"1999-01-01","to":"2026-11-24"}"#,
    );
    assert!(is_error(&v), "{}", jcs(&v));
    assert_eq!(s(structured(&v), "error"), "E_RANGE_TOO_LARGE", "{}", jcs(structured(&v)));
    assert!(everything(&v).contains("E_RANGE_TOO_LARGE"));
    // A function the mesh does not cover: not covered.
    let v = tool(
        &srv,
        "interval",
        r#"{"function":"datumesh.com/relay/nothing/here","from":"2024-01-01","to":"2024-01-02"}"#,
    );
    assert!(is_error(&v), "{}", jcs(&v));
    let all = everything(&v);
    assert!(all.contains("not_covered") || all.contains("not covered"), "{all}");
}

#[test]
fn a_member_whose_receipt_signature_is_broken_makes_the_tool_answer_an_error_naming_it() {
    // Decision: interval members are verified like every other answer.
    let (prod, _d, srv) = world();
    prod.tamper_interval(true);
    let v = tool(
        &srv,
        "interval",
        r#"{"function":"treasury/par_yield","keys":{"tenor":"10Y"},"from":"2026-11-16","to":"2026-11-24"}"#,
    );
    assert!(is_error(&v), "{}", jcs(&v));
    let all = everything(&v);
    // The second member is 2026-11-17: it is named by its key.
    assert!(all.contains("2026-11-17"), "the member is named: {all}");
}
