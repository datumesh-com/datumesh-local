//! CONTRACT-build2-part4-publish P1, the local and CLI side: `datumesh publish --to <mesh url>` and
//! the MCP tool `publish_public`. The public mesh is a stub (`tests/support/whatif_stub.rs`): the
//! test mesh with a canned `POST` answer at the bundles endpoint. Written before the
//! implementation.
//!
//! Shapes these tests specify (none of it exists yet):
//!
//! * MCP `publish_public` (writes: the token over HTTP, like `define` and `publish`;
//!   `additionalProperties: false`): `to` (the public mesh's URL), `namespace` (the rooted
//!   namespace, `<host>/<segments>`), `key` (the namespace host's key file, as `root`'s `--key`),
//!   `names` (this machine's names, the rooting set, as `root`'s) and optional `export` (a path
//!   where the bundle is also kept).
//! * It roots the names under the namespace in memory (1.8b rooting, with the catalogue and the
//!   tests in the bundle), then POSTs the bundle (`Content-Type: application/json`) to the mesh's
//!   bundles endpoint: the `x_bundles` URL of its mesh document (`<to>/.well-known/datumesh-mesh.json`)
//!   or, with no mesh document or no such member, `<to>/bundles`.
//! * The bundle has a `tests` section: one entry per test of the rooted computed functions, in
//!   the format of E5 (`{"test": <rooted function name>, "call": <rooted call>, "asof"?, "inputs"?,
//!   "expect": {"type", "value"} | {"fault"}}`), the names rewritten to the namespace.
//! * Before sending, local checks that every COMPUTED function of the set has tests that cover its
//!   derived cases and exercise every branch, using formula's coverage API
//!   (`datumesh_formula::coverage::{branches, edge_facts, Coverage}`: the tests are evaluated here
//!   with the observer). If not, nothing is sent and the result is an error whose structured content
//!   is `{"error": "E_TESTS_MISSING", "functions": [{"function": <rooted name>, "cases": [<text>],
//!   "branches_total": n, "branches_covered": m}]}`; only functions that lack something are
//!   listed. A case's text names its kind and what it is about: for a divisor, `zero` and the
//!   argument; for a number argument, `negative` and the argument; for an input cell, `absent`,
//!   `E_FORMULA_ABSENT` and the input's (rooted) name. Stored functions need no tests.
//! * Success (the mesh's `201`): not an error; the text lists each function's name and page URL;
//!   structured `{"namespace", "to", "imported": <the mesh's counts>, "names": [...], "functions":
//!   [{"name", "fn_hash", "page"}]}` as the mesh answered.
//! * The mesh's refusal (`400`, `403`, `422`, `{"error", "detail"}`): an error result whose
//!   structured content is `{"error": <the mesh's code>, "detail": <the mesh's detail>}`, text
//!   naming the code and the detail.
//! * The CLI, `datumesh publish --to <url> --namespace <ns> --key <file> [--export <file>] <name>...`
//!   (without `--to`, `publish <file>` is as before): the same, stdout one JSON line (the mesh's
//!   answer on success), exit 0; on a refusal (local or the mesh's) a nonzero exit and the code
//!   and what it lists on stderr; `--tests` of `define` is in `define_tests.rs`.

#![cfg(unix)]

mod common;
#[path = "support/tools.rs"]
mod tools;
#[path = "support/whatif_stub.rs"]
mod whatif_stub;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Transport};
use datumesh_local::{Config, Resolver};
use tools::*;
use whatif_stub::{DocMode, WhatifStub};

struct W {
    up: Upstream,
    stub: Arc<WhatifStub>,
    srv: McpServer,
    r: Arc<Resolver>,
    key: std::path::PathBuf,
    dir: TempDir,
    tmp: TempDir,
}

fn world(mode: DocMode) -> W {
    let up = upstream();
    let stub = WhatifStub::start(&up, mode);
    let dir = TempDir::new("publish-public");
    let tmp = TempDir::new("publish-public-tmp");
    let r = Arc::new(resolver(&dir, &up));
    let srv = McpServer::new(r.clone());
    let key = tmp.path().join("namespace.key");
    write_key(&key);
    W { up, stub, srv, r, key, dir, tmp }
}

fn page(name: &str) -> String {
    format!("https://mesh-test.datumesh.com/fn/{}", name.strip_prefix("mesh-test.datumesh.com/").unwrap())
}

/// The mesh's `201` for `names` (rooted function names).
fn created(names: &[&str]) -> String {
    let fs: Vec<String> = names
        .iter()
        .map(|n| format!(r#"{{"name":{},"fn_hash":"{}","page":{}}}"#, q(n), "ab".repeat(32), q(&page(n))))
        .collect();
    let ns: Vec<String> = names.iter().map(|n| q(n)).collect();
    format!(r#"{{"imported":{{"functions":{},"receipts":0}},"names":[{}],"functions":[{}]}}"#, names.len(), ns.join(","), fs.join(","))
}

fn publish_args(w: &W, names: &[&str]) -> String {
    let ns: Vec<String> = names.iter().map(|n| q(n)).collect();
    format!(
        r#"{{"to":{},"namespace":{},"key":{},"names":[{}]}}"#,
        q(&w.stub.origin),
        q(NS),
        q(&w.key.to_string_lossy()),
        ns.join(",")
    )
}

fn define_both(w: &W) {
    let usd = usd_value(&w.r);
    assert!(!is_error(&define_formula(&w.srv, "local/rate_x", RATE_X_SRC, RATE_X_SIG, Some(&rate_x_tests(&usd)))));
    assert!(!is_error(&define_formula(&w.srv, "local/ratio", RATIO_SRC, RATIO_SIG, Some(&ratio_tests(&usd)))));
}

fn publish_public(w: &W, names: &[&str]) -> Value {
    tool(&w.srv, "publish_public", &publish_args(w, names))
}

// ---------------------------------------------------------------------------------------------
// The tool in tools/list
// ---------------------------------------------------------------------------------------------

#[test]
fn publish_public_is_a_tool_with_its_arguments() {
    let w = world(DocMode::Direct);
    let v = rpc_on(&w.srv, Transport::Stdio, "tools/list", "");
    let t = arr(result(&v), "tools").iter().find(|t| s(t, "name") == "publish_public").expect("publish_public").clone();
    let schema = t.get("inputSchema").unwrap();
    assert_eq!(schema.get("additionalProperties"), Some(&Value::Bool(false)));
    let props = schema.get("properties").unwrap();
    for k in ["to", "namespace", "key", "names", "export"] {
        assert!(props.get(k).is_some(), "property {k}");
    }
    let required = jcs(schema.get("required").unwrap());
    for k in ["to", "namespace", "key", "names"] {
        assert!(required.contains(&format!("\"{k}\"")), "{k} is required: {required}");
    }
    assert!(s(&t, "description").to_lowercase().contains("tests"), "the description says tests are needed");
}

#[test]
fn publish_public_needs_the_token_over_http() {
    let w = world(DocMode::Direct);
    let v = rpc_on(
        &w.srv,
        Transport::Http { authorized: false },
        "tools/call",
        &format!(r#""name":"publish_public","arguments":{}"#, publish_args(&w, &["local/rate_x"])),
    );
    let message = v.get("error").and_then(|e| e.get("message")).and_then(Value::as_str);
    assert_eq!(message, Some("unauthorized"), "{}", jcs(&v));
    assert!(w.stub.posts().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Refused here, before anything is sent
// ---------------------------------------------------------------------------------------------

#[test]
fn a_computed_function_without_tests_is_refused_locally_with_the_derived_cases() {
    let w = world(DocMode::Direct);
    assert!(!is_error(&define_formula(&w.srv, "local/ratio", RATIO_SRC, RATIO_SIG, None)));
    let v = publish_public(&w, &["local/ratio"]);
    assert!(is_error(&v), "{}", jcs(&v));
    assert!(w.stub.posts().is_empty(), "nothing is sent: {:?}", w.stub.posts());
    let sc = structured(&v);
    assert_eq!(s(sc, "error"), "E_TESTS_MISSING");
    let fs = arr(sc, "functions");
    assert_eq!(fs.len(), 1, "{}", jcs(sc));
    assert_eq!(s(&fs[0], "function"), format!("{NS}/ratio"), "the name the public mesh will ask for");
    let cases: Vec<String> = arr(&fs[0], "cases").iter().map(|c| c.as_str().unwrap().to_string()).collect();
    let has = |words: &[&str]| cases.iter().any(|c| words.iter().all(|w| c.contains(w)));
    assert!(has(&["zero", "n"]), "n is a divisor: {cases:?}");
    assert!(has(&["negative", "n"]), "n is a number: {cases:?}");
    assert!(has(&["absent", "E_FORMULA_ABSENT", "ecb/fx_rate"]), "the input: {cases:?}");
    assert_eq!(fs[0].get("branches_total").and_then(Value::as_number_text), Some("2"));
    assert_eq!(fs[0].get("branches_covered").and_then(Value::as_number_text), Some("0"));
    let all = everything(&v);
    assert!(all.contains("E_TESTS_MISSING") && all.contains(&format!("{NS}/ratio")), "{all}");
}

#[test]
fn partial_tests_list_only_what_is_still_missing() {
    let w = world(DocMode::Direct);
    let usd = usd_value(&w.r);
    // Only the then arm: the else arm, the zero divisor, the negative and the absent input remain.
    let tests = format!(r#"[{{"call":"local/ratio(date: 2026-10-30, n: 1)","expect":{}}}]"#, q(&usd));
    assert!(!is_error(&define_formula(&w.srv, "local/ratio", RATIO_SRC, RATIO_SIG, Some(&tests))));
    let v = publish_public(&w, &["local/ratio"]);
    assert!(is_error(&v), "{}", jcs(&v));
    assert!(w.stub.posts().is_empty());
    let fs = arr(structured(&v), "functions");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].get("branches_total").and_then(Value::as_number_text), Some("2"));
    assert_eq!(fs[0].get("branches_covered").and_then(Value::as_number_text), Some("1"), "{}", jcs(&fs[0]));
    assert!(!arr(&fs[0], "cases").is_empty());
}

#[test]
fn only_the_functions_that_lack_something_are_listed() {
    let w = world(DocMode::Direct);
    let usd = usd_value(&w.r);
    assert!(!is_error(&define_formula(&w.srv, "local/rate_x", RATE_X_SRC, RATE_X_SIG, Some(&rate_x_tests(&usd)))));
    assert!(!is_error(&define_formula(&w.srv, "local/ratio", RATIO_SRC, RATIO_SIG, None)));
    let v = publish_public(&w, &["local/rate_x", "local/ratio"]);
    assert!(is_error(&v), "{}", jcs(&v));
    let fs = arr(structured(&v), "functions");
    assert_eq!(fs.len(), 1, "{}", jcs(structured(&v)));
    assert_eq!(s(&fs[0], "function"), format!("{NS}/ratio"));
    assert!(w.stub.posts().is_empty());
}

#[test]
fn a_namespace_that_is_not_rooted_is_refused_before_sending() {
    let w = world(DocMode::Direct);
    define_both(&w);
    let args = publish_args(&w, &["local/rate_x"]).replace(NS, "local.invalid/scratch");
    let v = tool(&w.srv, "publish_public", &args);
    assert!(is_error(&v), "{}", jcs(&v));
    assert!(w.stub.posts().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Sent, with the tests in the bundle
// ---------------------------------------------------------------------------------------------

#[test]
fn a_function_with_tests_is_rooted_and_posted_to_the_mesh_documents_bundles_endpoint() {
    let w = world(DocMode::Direct);
    define_both(&w);
    let names = [format!("{NS}/rate_x"), format!("{NS}/ratio")];
    w.stub.answer_bundles(201, &created(&[&names[0], &names[1]]));
    let v = publish_public(&w, &["local/rate_x", "local/ratio"]);
    assert!(!is_error(&v), "{}", jcs(&v));
    // Posted once, to where `x_bundles` points, as JSON.
    assert_eq!(w.stub.posts().len(), 1, "{:?}", w.stub.posts().iter().map(|p| &p.path).collect::<Vec<_>>());
    let post = &w.stub.posts()[0];
    assert_eq!(post.path, "/x/bundles-in");
    assert_eq!(post.content_type.split(';').next().unwrap().trim(), "application/json");
    let b = post.json();
    assert_eq!(b.get("bundle").and_then(Value::as_number_text), Some("1"));
    assert_eq!(s(&b, "namespace"), NS);
    let bn: Vec<String> = arr(&b, "names").iter().map(|n| n.as_str().unwrap().to_string()).collect();
    for n in &names {
        assert!(bn.contains(n), "{n} in {bn:?}");
    }
    // The tests: 2 of rate_x and 4 of ratio, named under the namespace.
    let tests = arr(&b, "tests");
    assert_eq!(tests.len(), 6, "{}", jcs(&b));
    for t in tests {
        let f = s(t, "test");
        assert!(f == names[0] || f == names[1], "{f}");
        assert!(s(t, "call").starts_with(&format!("{f}(")), "the call is rooted too: {}", jcs(t));
        match t.get("expect") {
            Some(e) if e.get("fault").is_some() => assert_eq!(s(e, "fault"), "E_FORMULA_ABSENT"),
            Some(e) => assert_eq!(s(e, "type"), "number", "{}", jcs(t)),
            None => panic!("no expect: {}", jcs(t)),
        }
    }
    let usd = usd_value(&w.r);
    assert!(
        tests.iter().any(|t| s(t, "test") == names[0] && t.get("expect").map(|e| s(e, "value")) == Some(doubled(&usd))),
        "rate_x's value test: {}",
        jcs(&b)
    );
    // What the agent reads: each function and its page.
    let all = everything(&v);
    for n in &names {
        assert!(all.contains(n.as_str()) && all.contains(&page(n)), "{n}: {all}");
    }
    let sc = structured(&v);
    assert_eq!(s(sc, "namespace"), NS);
    let fs = arr(sc, "functions");
    assert_eq!(fs.len(), 2);
    assert_eq!(s(&fs[0], "page"), page(&names[0]));
}

#[test]
fn without_a_mesh_document_the_bundles_endpoint_is_slash_bundles() {
    let w = world(DocMode::None);
    define_both(&w);
    let n = format!("{NS}/rate_x");
    w.stub.answer_bundles(201, &created(&[&n]));
    let v = publish_public(&w, &["local/rate_x"]);
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_eq!(w.stub.posts_to("/bundles").len(), 1, "{:?}", w.stub.posts().iter().map(|p| &p.path).collect::<Vec<_>>());
    assert!(w.stub.posts_to("/x/bundles-in").is_empty());
}

#[test]
fn a_stored_function_needs_no_tests() {
    let w = world(DocMode::Direct);
    let sg = r#"{"args": {"day": {"type": "date", "valid": true}}, "order": ["day"], "returns": {"type": "number", "unit": "1"}}"#;
    let d = tool(&w.srv, "define", &format!(r#"{{"name":"local/level","stored":true,"signature":{sg}}}"#));
    assert!(!is_error(&d), "{}", jcs(&d));
    let n = format!("{NS}/level");
    w.stub.answer_bundles(201, &created(&[&n]));
    let v = publish_public(&w, &["local/level"]);
    assert!(!is_error(&v), "{}", jcs(&v));
    let b = w.stub.posts()[0].json();
    assert!(b.get("tests").is_none() || arr(&b, "tests").is_empty(), "{}", jcs(&b));
}

#[test]
fn the_tests_defined_earlier_travel_in_a_later_publish() {
    // The tests are kept with the function: a new server over the same store still carries them.
    let w = world(DocMode::Direct);
    let usd = usd_value(&w.r);
    assert!(!is_error(&define_formula(&w.srv, "local/rate_x", RATE_X_SRC, RATE_X_SIG, Some(&rate_x_tests(&usd)))));
    let again = McpServer::new(w.r.clone());
    let n = format!("{NS}/rate_x");
    w.stub.answer_bundles(201, &created(&[&n]));
    let v = tool(&again, "publish_public", &publish_args(&w, &["local/rate_x"]));
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_eq!(arr(&w.stub.posts()[0].json(), "tests").len(), 2);
}

#[test]
fn the_export_is_kept_when_asked_for() {
    let w = world(DocMode::Direct);
    define_both(&w);
    let n = format!("{NS}/rate_x");
    w.stub.answer_bundles(201, &created(&[&n]));
    let export = w.tmp.path().join("kept.json");
    let args = publish_args(&w, &["local/rate_x"]).replacen('{', &format!(r#"{{"export":{},"#, q(&export.to_string_lossy())), 1);
    let v = tool(&w.srv, "publish_public", &args);
    assert!(!is_error(&v), "{}", jcs(&v));
    let kept = json::parse(&std::fs::read(&export).unwrap()).unwrap();
    assert_eq!(jcs(&kept), jcs(&w.stub.posts()[0].json()), "the bundle kept is the bundle sent");
}

// ---------------------------------------------------------------------------------------------
// The mesh refuses
// ---------------------------------------------------------------------------------------------

#[test]
fn the_meshs_refusals_are_passed_on_with_their_codes_and_details() {
    let w = world(DocMode::Direct);
    define_both(&w);
    for (status, code, detail) in [
        (403, "E_DOMAIN_NOT_ALLOWED", r#""mesh-test.datumesh.com may not publish on this mesh""#),
        (422, "E_TESTS_FAILING", r#"{"failing":[{"function":"x","case":"0123456789abcdef","detail":"expected 2, got 1"}]}"#),
        (400, "E_BUNDLE_SIGNATURE", r#""the manifest signature does not verify""#),
    ] {
        w.stub.answer_bundles(status, &format!(r#"{{"error":"{code}","detail":{detail}}}"#));
        let v = publish_public(&w, &["local/rate_x"]);
        assert!(is_error(&v), "{status}: {}", jcs(&v));
        assert_eq!(s(structured(&v), "error"), code);
        assert!(structured(&v).get("detail").is_some(), "{}", jcs(structured(&v)));
        let all = everything(&v);
        assert!(all.contains(code), "{all}");
        let needle = if code == "E_TESTS_FAILING" { "expected 2, got 1" } else { detail.trim_matches('"') };
        assert!(all.contains(needle), "{all}");
    }
}

// ---------------------------------------------------------------------------------------------
// The CLI
// ---------------------------------------------------------------------------------------------

fn home(w: &W) -> TempDir {
    let dir = TempDir::new("publish-public-cli");
    Config::in_dir(dir.path()).unwrap();
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("\"peers\": {}", &format!("\"peers\": {{\"{HOST}\": \"{}\"}}", w.up.origin))).unwrap();
    dir
}

fn run(dir: &TempDir, args: &[&str]) -> (i32, String, String) {
    let mut all = vec!["--home".to_string(), dir.path().to_string_lossy().into_owned()];
    all.extend(args.iter().map(|a| a.to_string()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&all, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

fn cli_define(w: &W, dir: &TempDir, name: &str, src: &str, sg: &str, tests: Option<&str>) {
    let mut a = vec!["define", name, "--formula", src, "--signature", sg];
    if let Some(t) = tests {
        a.push("--tests");
        a.push(t);
    }
    let (code, _, err) = run(dir, &a);
    assert_eq!(code, 0, "{err}");
    let _ = w;
}

#[test]
fn the_cli_roots_posts_and_prints_the_functions_and_their_pages() {
    let w = world(DocMode::Direct);
    let dir = home(&w);
    let usd = usd_value(&w.r);
    cli_define(&w, &dir, "local/rate_x", RATE_X_SRC, RATE_X_SIG, Some(&rate_x_tests(&usd)));
    let n = format!("{NS}/rate_x");
    w.stub.answer_bundles(201, &created(&[&n]));
    let key = w.key.to_string_lossy().into_owned();
    let (code, out, err) =
        run(&dir, &["publish", "--to", &w.stub.origin, "--namespace", NS, "--key", &key, "local/rate_x"]);
    assert_eq!(code, 0, "{err}");
    let posts = w.stub.posts();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].path, "/x/bundles-in");
    let b = posts[0].json();
    assert_eq!(s(&b, "namespace"), NS);
    assert_eq!(arr(&b, "tests").len(), 2);
    let v = json::parse(out.trim_end().as_bytes()).unwrap();
    let fs = arr(&v, "functions");
    assert_eq!(fs.len(), 1);
    assert_eq!(s(&fs[0], "name"), n);
    assert_eq!(s(&fs[0], "page"), page(&n));
}

#[test]
fn the_cli_refuses_without_tests_and_sends_nothing() {
    let w = world(DocMode::Direct);
    let dir = home(&w);
    cli_define(&w, &dir, "local/rate_x", RATE_X_SRC, RATE_X_SIG, None);
    let key = w.key.to_string_lossy().into_owned();
    let (code, out, err) =
        run(&dir, &["publish", "--to", &w.stub.origin, "--namespace", NS, "--key", &key, "local/rate_x"]);
    assert_ne!(code, 0, "{out}");
    assert!(err.contains("E_TESTS_MISSING"), "{err}");
    assert!(err.contains(&format!("{NS}/rate_x")), "the function: {err}");
    assert!(err.contains("E_FORMULA_ABSENT"), "the derived case: {err}");
    assert!(w.stub.posts().is_empty());
}

#[test]
fn the_cli_reports_the_meshs_refusal() {
    let w = world(DocMode::Direct);
    let dir = home(&w);
    let usd = usd_value(&w.r);
    cli_define(&w, &dir, "local/rate_x", RATE_X_SRC, RATE_X_SIG, Some(&rate_x_tests(&usd)));
    w.stub.answer_bundles(403, r#"{"error":"E_DOMAIN_NOT_ALLOWED","detail":"not allowed here"}"#);
    let key = w.key.to_string_lossy().into_owned();
    let (code, _, err) =
        run(&dir, &["publish", "--to", &w.stub.origin, "--namespace", NS, "--key", &key, "local/rate_x"]);
    assert_ne!(code, 0);
    assert!(err.contains("E_DOMAIN_NOT_ALLOWED") && err.contains("not allowed here"), "{err}");
    assert_eq!(w.stub.posts().len(), 1);
}

#[test]
fn publish_of_a_file_is_as_before() {
    let w = world(DocMode::Direct);
    let dir = home(&w);
    let f = jsonl(&w.tmp, "empty.jsonl", &[]);
    let (code, out, err) = run(&dir, &["publish", &f.to_string_lossy()]);
    assert_eq!(code, 0, "{err}");
    assert!(json::parse(out.trim_end().as_bytes()).unwrap().get("cells").is_some());
    let _ = &w.dir;
}
