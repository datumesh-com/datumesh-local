//! CONTRACT-0.4.9 L1 (datumesh-local 0.4.3): `describe` of a function lists its tests, in the MCP
//! tool and at the command line.
//!
//! Shapes these tests specify (none of it exists yet):
//!
//! * A function on a public mesh: the tests are read from the mesh's function page JSON,
//!   `GET <origin>/fn/<name>?format=json` (the page the describe answer's `Link` names; the origin is
//!   the configured peer of the function's host). Its `tests` member: `count`, `cases` (first 100:
//!   `call`, `expect`, `result`), `cases_total`, `last_run` `{result, at}` (0.4.9 adds
//!   `pass`/`fail`/`error`/`not_run`, accepted and ignored here).
//! * MCP `describe`: the structured content keeps `status` and `describe` as before and gains
//!   `tests`: `{count, cases_total, last_run: {result, at} | null, cases: [{call, expect, result}]}`
//!   (`cases` at most 100; `cases_total` the full number). The text content says "N tests" or
//!   "no tests", the last run's result and time, and each case's call.
//! * A local formula (`local/...`): `tests` is built from its own tests (`count`, `cases` with `call`
//!   and `expect`, `result` a string: stored or "not run"; `last_run` may be null).
//! * A function with no tests: `tests.count` is 0, `cases` is empty, and the text says "no tests".
//! * The CLI `datumesh describe <id>` prints the describe answer line as before, then the same
//!   facts as text: "tests: N" (or "no tests"), "last run <at> <result>", and one line per case
//!   with its call, expectation and result.

#![cfg(unix)]

mod common;
#[path = "support/tools.rs"]
mod tools;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use common::*;
use datumesh_core::json::Value;
use datumesh_local::mcp::McpServer;
use datumesh_local::{Config, Resolver};
use tools::*;

const RATE: &str = "mesh-test.datumesh.com/ecb/fx_rate";
const HOLIDAY: &str = "mesh-test.datumesh.com/ecb/holiday";
const LAST_AT: &str = "2026-11-20T10:30:00.000000Z";

/// The test mesh with `/fn/<name>` answered from canned JSON (what http 0.4.8/0.4.9 serves).
struct FnStub {
    pages: Arc<Mutex<HashMap<String, String>>>,
    requests: Arc<Mutex<Vec<String>>>,
    _rt: tokio::runtime::Runtime,
    origin: String,
}

impl FnStub {
    fn start(up: &Upstream) -> FnStub {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let addr = listener.local_addr().unwrap();
        let pages: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (p, rq) = (pages.clone(), requests.clone());
        let handler = move |req: axum::extract::Request| {
            let (p, rq) = (p.clone(), rq.clone());
            async move {
                let path = req.uri().path().to_string();
                let query = req.uri().query().unwrap_or("").to_string();
                rq.lock().unwrap().push(format!("{path}?{query}"));
                let name = path.strip_prefix("/fn/").unwrap_or("").to_string();
                let body = p.lock().unwrap().get(&name).cloned();
                match body {
                    Some(b) => axum::http::Response::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(b))
                        .unwrap(),
                    None => axum::http::Response::builder().status(404).body(axum::body::Body::from("{}")).unwrap(),
                }
            }
        };
        let router =
            axum::Router::new().route("/fn/{*name}", axum::routing::get(handler)).fallback_service(up.mesh.router());
        rt.spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        FnStub { pages, requests, _rt: rt, origin: format!("http://{addr}") }
    }

    fn page(&self, name: &str, tests: &str) {
        let body = format!(r#"{{"name":"{HOST}/{name}","tests":{tests}}}"#);
        self.pages.lock().unwrap().insert(name.to_string(), body);
    }

    fn asked(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// `n` cases `fx_rate(...)`, `total` in all, a last run at `LAST_AT` with `result`.
fn tests_json(n: usize, total: usize, result: &str) -> String {
    let cases: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"call":"ecb/fx_rate(base: \"EUR\", quote: \"USD\", date: 2026-01-{:02}) /*case {i}*/","expect":"1.{i:04}","result":"pass"}}"#,
                1 + i % 28
            )
        })
        .collect();
    format!(
        r#"{{"count":{total},"branches":2,"covered":2,"counterexamples":0,"open_counterexamples":0,"cases":[{}],"cases_total":{total},"last_run":{{"result":"{result}","at":"{LAST_AT}","pass":{total},"fail":0,"error":0,"not_run":0}}}}"#,
        cases.join(",")
    )
}

fn world() -> (Upstream, FnStub, TempDir, Arc<Resolver>, McpServer) {
    let up = upstream();
    let stub = FnStub::start(&up);
    let dir = TempDir::new("describe-tests");
    let mut c: Config = config(&dir, &up);
    c.peers.insert(HOST.to_string(), stub.origin.clone());
    let r = Arc::new(Resolver::open(c).unwrap());
    let srv = McpServer::new(r.clone());
    (up, stub, dir, r, srv)
}

fn describe(srv: &McpServer, id: &str) -> Value {
    tool(srv, "describe", &format!(r#"{{"id":{}}}"#, q(id)))
}

fn tests_of(v: &Value) -> &Value {
    assert!(!is_error(v), "{}", jcs(v));
    structured(v).get("tests").unwrap_or_else(|| panic!("no `tests` in {}", jcs(structured(v))))
}

fn num_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_number_text).unwrap_or_else(|| panic!("no number {k} in {}", jcs(v))).to_string()
}

#[test]
fn describe_of_a_public_function_lists_its_tests_from_the_function_page() {
    let (_up, stub, _dir, _r, srv) = world();
    stub.page("ecb/fx_rate", &tests_json(3, 3, "pass"));
    let v = describe(&srv, RATE);
    let t = tests_of(&v);
    assert_eq!(num_of(t, "count"), "3", "{}", jcs(t));
    assert_eq!(num_of(t, "cases_total"), "3");
    let lr = t.get("last_run").unwrap_or_else(|| panic!("last_run in {}", jcs(t)));
    assert_eq!(s(lr, "result"), "pass");
    assert_eq!(s(lr, "at"), LAST_AT);
    let cases = arr(t, "cases");
    assert_eq!(cases.len(), 3);
    assert!(s(&cases[0], "call").contains("ecb/fx_rate(base: \"EUR\""), "{}", jcs(&cases[0]));
    assert_eq!(s(&cases[1], "expect"), "1.0001");
    assert_eq!(s(&cases[2], "result"), "pass");
    // The tests were read from the mesh's function page JSON, once the describe answer named it.
    assert!(
        stub.asked().iter().any(|r| r.starts_with("/fn/ecb/fx_rate") && r.contains("format=json")),
        "{:?}",
        stub.asked()
    );
    // The describe answer itself is as before.
    assert_eq!(structured(&v).get("status").and_then(Value::as_number_text), Some("200"));
    assert_eq!(s(structured(&v).get("describe").unwrap(), "kind"), "function");
}

#[test]
fn the_text_of_describe_gives_count_last_run_and_cases() {
    let (_up, stub, _dir, _r, srv) = world();
    stub.page("ecb/fx_rate", &tests_json(2, 2, "fail"));
    let v = describe(&srv, RATE);
    assert!(!is_error(&v), "{}", jcs(&v));
    let text = items(&v).join("\n");
    assert!(text.contains("2 tests"), "{text}");
    assert!(text.contains("fail"), "{text}");
    assert!(text.contains(LAST_AT) || text.contains("2026-11-20"), "{text}");
    assert!(text.contains("ecb/fx_rate(base: \"EUR\""), "{text}");
    assert!(!text.contains("no tests"), "{text}");
}

#[test]
fn describe_lists_the_first_100_cases_with_the_total() {
    let (_up, stub, _dir, _r, srv) = world();
    // The page already holds the first 100 of 250; describe shows them and says 250.
    stub.page("ecb/fx_rate", &tests_json(100, 250, "pass"));
    let v = describe(&srv, RATE);
    let t = tests_of(&v);
    assert_eq!(num_of(t, "count"), "250");
    assert_eq!(num_of(t, "cases_total"), "250");
    assert_eq!(arr(t, "cases").len(), 100);
    let text = items(&v).join("\n");
    assert!(text.contains("250"), "{text}");
    // Even if a page gave more than 100, describe shows at most 100.
    stub.page("ecb/fx_rate", &tests_json(130, 250, "pass"));
    let v = describe(&srv, RATE);
    assert_eq!(arr(tests_of(&v), "cases").len(), 100);
    assert_eq!(num_of(tests_of(&v), "cases_total"), "250");
}

#[test]
fn the_0_4_9_last_run_members_are_accepted_and_a_bare_0_4_8_last_run_too() {
    let (_up, stub, _dir, _r, srv) = world();
    stub.page(
        "ecb/fx_rate",
        &format!(
            r#"{{"count":1,"cases":[{{"call":"ecb/fx_rate(base: \"EUR\", quote: \"USD\", date: 2026-01-02)","expect":"1","result":"error"}}],"cases_total":1,"last_run":{{"result":"error","at":"{LAST_AT}","pass":0,"fail":0,"error":1,"not_run":0}}}}"#
        ),
    );
    let v = describe(&srv, RATE);
    assert_eq!(s(tests_of(&v).get("last_run").unwrap(), "result"), "error");
    stub.page(
        "ecb/fx_rate",
        &format!(
            r#"{{"count":1,"cases":[{{"call":"ecb/fx_rate(base: \"EUR\", quote: \"USD\", date: 2026-01-02)","expect":"1","result":"pass"}}],"cases_total":1,"last_run":{{"result":"pass","at":"{LAST_AT}"}}}}"#
        ),
    );
    let v = describe(&srv, RATE);
    assert_eq!(s(tests_of(&v).get("last_run").unwrap(), "result"), "pass");
}

#[test]
fn a_public_function_with_no_tests_says_no_tests() {
    let (_up, stub, _dir, _r, srv) = world();
    stub.page(
        "ecb/holiday",
        r#"{"count":0,"branches":0,"covered":0,"counterexamples":0,"open_counterexamples":0,"cases":[],"cases_total":0,"last_run":null}"#,
    );
    let v = describe(&srv, HOLIDAY);
    let t = tests_of(&v);
    assert_eq!(num_of(t, "count"), "0");
    assert!(arr(t, "cases").is_empty());
    assert!(items(&v).join("\n").contains("no tests"), "{}", items(&v).join("\n"));
}

#[test]
fn a_local_formula_defined_with_tests_lists_its_own_tests() {
    let (_up, _stub, _dir, r, srv) = world();
    let usd = usd_value(&r);
    let d = define_formula(&srv, "local/ratio", RATIO_SRC, RATIO_SIG, Some(&ratio_tests(&usd)));
    assert!(!is_error(&d), "{}", jcs(&d));
    let v = describe(&srv, "local/ratio");
    let t = tests_of(&v);
    assert_eq!(num_of(t, "count"), "4", "{}", jcs(t));
    assert_eq!(num_of(t, "cases_total"), "4");
    let cases = arr(t, "cases");
    assert_eq!(cases.len(), 4);
    let calls: Vec<String> = cases.iter().map(|c| s(c, "call")).collect();
    for needle in ["n: 1)", "n: 0)", "n: -1)", "date: 2026-01-01"] {
        assert!(calls.iter().any(|c| c.contains("ratio(") && c.contains(needle)), "{needle} in {calls:?}");
    }
    for c in cases {
        assert!(c.get("expect").is_some() && c.get("result").is_some(), "{}", jcs(c));
    }
    let text = items(&v).join("\n");
    assert!(text.contains("4 tests"), "{text}");
    assert!(!text.contains("no tests"), "{text}");
}

#[test]
fn a_local_function_without_tests_says_no_tests() {
    let (_up, _stub, _dir, _r, srv) = world();
    let d = define_formula(&srv, "local/rate_x", RATE_X_SRC, RATE_X_SIG, None);
    assert!(!is_error(&d), "{}", jcs(&d));
    let v = describe(&srv, "local/rate_x");
    let t = tests_of(&v);
    assert_eq!(num_of(t, "count"), "0");
    assert!(arr(t, "cases").is_empty());
    assert!(items(&v).join("\n").contains("no tests"), "{}", items(&v).join("\n"));
}

// ---- the command line ----

fn home(up: &Upstream, stub: &FnStub) -> TempDir {
    let dir = TempDir::new("describe-tests-cli");
    Config::in_dir(dir.path()).unwrap();
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let _ = up;
    std::fs::write(&path, text.replace("\"peers\": {}", &format!("\"peers\": {{\"{HOST}\": \"{}\"}}", stub.origin)))
        .unwrap();
    dir
}

fn run(dir: &TempDir, args: &[&str]) -> (i32, String, String) {
    let mut all = vec!["--home".to_string(), dir.path().to_string_lossy().into_owned()];
    all.extend(args.iter().map(|a| a.to_string()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&all, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

#[test]
fn the_cli_describe_prints_the_tests_of_a_public_function() {
    let up = upstream();
    let stub = FnStub::start(&up);
    stub.page("ecb/fx_rate", &tests_json(3, 250, "pass"));
    let dir = home(&up, &stub);
    let (code, out, err) = run(&dir, &["describe", RATE]);
    assert_eq!(code, 0, "{err}");
    // The first line is the describe answer, as before.
    let first = out.lines().next().unwrap();
    let doc = datumesh_core::json::parse(first.as_bytes()).unwrap_or_else(|e| panic!("{e:?}: {first}"));
    assert_eq!(s(&doc, "kind"), "function");
    assert!(out.contains("tests: 250"), "{out}");
    assert!(out.contains("pass"), "{out}");
    assert!(out.contains(LAST_AT) || out.contains("2026-11-20"), "{out}");
    assert!(out.contains("ecb/fx_rate(base: \"EUR\""), "{out}");
    assert!(out.contains("1.0002"), "{out}");
}

#[test]
fn the_cli_describe_says_no_tests_for_a_function_without_tests() {
    let up = upstream();
    let stub = FnStub::start(&up);
    stub.page("ecb/holiday", r#"{"count":0,"cases":[],"cases_total":0,"last_run":null}"#);
    let dir = home(&up, &stub);
    let (code, out, err) = run(&dir, &["describe", HOLIDAY]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("no tests"), "{out}");
    // A local formula with no tests: the same.
    let sig = r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": "boolean"}"#;
    let (code, _, err) = run(
        &dir,
        &[
            "define",
            "local/closed",
            "--formula",
            r#"mesh-test.datumesh.com/ecb/holiday(calendar: "TARGET", date: date)"#,
            "--signature",
            sig,
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = run(&dir, &["describe", "local/closed"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("no tests"), "{out}");
}

#[test]
fn the_cli_describe_lists_the_tests_of_a_local_formula() {
    let up = upstream();
    let stub = FnStub::start(&up);
    let dir = home(&up, &stub);
    let tests = r#"[{"call":"local/rate_x(date: 2026-01-01)","expect":{"fault":"E_FORMULA_ABSENT"}},{"call":"local/rate_x(date: 2026-01-02)","expect":{"fault":"E_FORMULA_ABSENT"}}]"#;
    let (code, _, err) =
        run(&dir, &["define", "local/rate_x", "--formula", RATE_X_SRC, "--signature", RATE_X_SIG, "--tests", tests]);
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = run(&dir, &["describe", "local/rate_x"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("tests: 2"), "{out}");
    assert!(out.contains("date: 2026-01-01") && out.contains("date: 2026-01-02"), "{out}");
    assert!(!out.contains("no tests"), "{out}");
}
