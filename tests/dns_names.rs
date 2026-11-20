//! spec-0.6.1 (CONTRACT-build2-spec-0.6.1 S1, S3), datumesh-local 0.4.1: names with a DNS name as
//! a namespace segment after the host (`<host>/relay/ecb.europa.eu/fx_reference`) are read,
//! defined against and described; `find` and `describe` show the publisher (the host) apart from
//! any `source`; refused names are refused with `E_CANON_NAME`.
//!
//! The upstream is the test mesh (datumesh-http 0.4.2) holding two functions under dotted
//! segments: a relay (its cells carry `source`) and a plain stored one.

mod common;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Transport};
use datumesh_local::{Config, Resolver};

const M: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;
const RELAY_NS: &str = "mesh-test.datumesh.com/relay/ecb.europa.eu";
const FX: &str = "mesh-test.datumesh.com/relay/ecb.europa.eu/fx_reference";
const PLAIN: &str = "mesh-test.datumesh.com/relay/bank-x.example/rate";
const CALL: &str = r#"mesh-test.datumesh.com/relay/ecb.europa.eu/fx_reference(currency: "USD", date: 2026-10-30)"#;
const URI: &str = "cell://mesh-test.datumesh.com/relay/ecb.europa.eu/fx_reference?currency=USD&date=2026-10-30";
const PLAIN_CALL: &str = "mesh-test.datumesh.com/relay/bank-x.example/rate(date: 2026-10-30)";
const SIG_NUM: &str = r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": {"type": "number", "unit": "1"}}"#;

fn q(s: &str) -> String {
    String::from_utf8(Value::String(s.to_string()).to_jcs()).unwrap()
}

fn jcs(v: &Value) -> String {
    String::from_utf8(v.to_jcs()).unwrap()
}

fn sig(t: &str) -> Value {
    json::parse(t.as_bytes()).unwrap()
}

/// The upstream with the two dotted-segment functions and one cell each.
fn dotted_upstream() -> Upstream {
    let up = upstream();
    let t = "2026-10-30T15:00:00.000000Z";
    let lines = [
        r#"{"bound": "2026-09-01T00:00:00.000000Z", "function": "mesh-test.datumesh.com/relay/ecb.europa.eu/fx_reference", "signature": {"args": {"currency": {"type": "string", "valid": false}, "date": {"type": "date", "valid": true}}, "order": ["currency", "date"], "returns": {"type": "number", "unit": "1"}}, "description": "Synthetic relay test function, not ECB data."}"#.to_string(),
        format!(
            r#"{{"asof": "{t}", "cell": {{"args": {{"currency": {{"type": "string", "value": "USD"}}, "date": {{"type": "date", "value": "2026-10-30"}}}}, "fn": "{FX}"}}, "licence": "CC-BY-4.0", "source": {{"name": "ecb.europa.eu", "retrieved": "{t}", "takeover": "https://mesh-test.datumesh.com/takeover/ecb", "url": "https://www.ecb.europa.eu/stats/eurofxref/"}}, "summary": "Synthetic test data, not ECB data. Licence CC-BY-4.0.", "tx": "{t}", "value": {{"type": "number", "value": "1.1"}}}}"#
        ),
        r#"{"bound": "2026-09-01T00:00:00.000000Z", "function": "mesh-test.datumesh.com/relay/bank-x.example/rate", "signature": {"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": {"type": "number", "unit": "1"}}, "description": "Synthetic stored test function."}"#.to_string(),
        format!(
            r#"{{"asof": "{t}", "cell": {{"args": {{"date": {{"type": "date", "value": "2026-10-30"}}}}, "fn": "{PLAIN}"}}, "licence": "CC-BY-4.0", "summary": "Synthetic test data. Licence CC-BY-4.0.", "tx": "{t}", "value": {{"type": "number", "value": "3"}}}}"#
        ),
    ];
    let catalog = |name: &str, label: &str| {
        format!(
            r#"{{"catalog": "{name}", "kind": "function", "label": "{label}", "description": "Synthetic test function under a DNS name segment."}}"#
        )
    };
    let mut lines = lines.to_vec();
    lines.push(catalog(FX, "Synthetic reference rate"));
    lines.push(catalog(PLAIN, "Synthetic plain rate"));
    let values: Vec<Value> = lines.iter().map(|l| sig(l)).collect();
    up.publish_lines(&values);
    up
}

fn cfg(dir: &TempDir, up: &Upstream) -> Config {
    let mut c = config(dir, up);
    c.find_meshes = vec![HOST.to_string()];
    c
}

fn server(dir: &TempDir, up: &Upstream) -> McpServer {
    McpServer::new(Arc::new(Resolver::open(cfg(dir, up)).unwrap()))
}

fn tool(srv: &McpServer, name: &str, args: &str) -> Value {
    let line = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{M},"name":{},"arguments":{args}}}}}"#,
        q(name)
    );
    json::parse(srv.handle(&line, Transport::Stdio).expect("a response").as_bytes()).unwrap()
}

fn structured(v: &Value) -> &Value {
    v.get("result")
        .and_then(|r| r.get("structuredContent"))
        .unwrap_or_else(|| panic!("structuredContent in {}", jcs(v)))
}

fn array<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        other => panic!("{k}: {other:?} in {}", jcs(v)),
    }
}

fn trust_of(body: &[u8]) -> String {
    s(&envelope(body), "trust")
}

// ---- 1. get ------------------------------------------------------------------------------------

#[test]
fn a_relay_cell_under_a_dns_segment_is_answered_verified_and_relayed() {
    let up = dotted_upstream();
    let dir = TempDir::new("dns-get");
    let r = resolver(&dir, &up);
    for request in [CALL, URI] {
        let a = r.get(request).unwrap();
        assert_eq!(a.status, 200, "{request}: {}", String::from_utf8_lossy(&a.body));
        let rc = receipt(&a.body);
        assert_eq!(rc.get("value").and_then(Value::as_str), Some("1.1"), "{request}");
        assert_eq!(s(rc.get("cell").unwrap(), "ns"), RELAY_NS);
        assert_eq!(trust_of(&a.body), "relayed", "a receipt with a source is relayed");
        assert!(rc.get("source").is_some());
        let v = r.verify(&rc.to_jcs());
        assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{request}: {}", jcs(&v));
    }
}

#[test]
fn a_stored_cell_under_a_hyphenated_dns_segment_is_primary() {
    let up = dotted_upstream();
    let dir = TempDir::new("dns-get-plain");
    let r = resolver(&dir, &up);
    let a = r.get(PLAIN_CALL).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(trust_of(&a.body), "primary");
    assert_eq!(r.verify(&receipt(&a.body).to_jcs()).get("ok"), Some(&Value::Bool(true)));
}

#[test]
fn mcp_get_of_a_dotted_name_by_call_and_by_uri() {
    let up = dotted_upstream();
    let dir = TempDir::new("dns-mcp-get");
    let srv = server(&dir, &up);
    for request in [CALL, URI] {
        let v = tool(&srv, "get", &format!(r#"{{"request":{}}}"#, q(request)));
        assert_ne!(v.get("result").and_then(|r| r.get("isError")), Some(&Value::Bool(true)), "{}", jcs(&v));
        let st = structured(&v);

        assert_eq!(s(st, "trust"), "relayed", "{}", jcs(st));
    }
}

// ---- 2. define ---------------------------------------------------------------------------------

#[test]
fn a_local_formula_reading_a_dotted_name_is_defined_and_evaluated() {
    let up = dotted_upstream();
    let dir = TempDir::new("dns-define");
    let r = resolver(&dir, &up);
    let src = format!(r#"{FX}(currency: "USD", date: date) * 2"#);
    let d = r.define_formula("local/x", &src, &sig(SIG_NUM)).unwrap();
    assert_eq!(d.name, "local.invalid/x");
    let a = r.get("local/x(date: 2026-10-30)").unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    let rc = receipt(&a.body);
    assert_eq!(rc.get("value").and_then(Value::as_str), Some("2.2"), "{}", jcs(&rc));
    assert_eq!(trust_of(&a.body), "computed");
    assert_eq!(s(&rc, "computed_by"), format!("local.invalid#{}", r.key_id()));
    // Its input is the dotted cell, whose own receipt is the relay's.
    assert!(matches!(rc.get("inputs"), Some(Value::Array(i)) if i.len() == 1), "{}", jcs(&rc));
    assert_eq!(r.verify(&rc.to_jcs()).get("ok"), Some(&Value::Bool(true)));
}

#[test]
fn a_dotted_segment_beside_minus_and_a_hyphenated_one_beside_divide() {
    let up = dotted_upstream();
    let dir = TempDir::new("dns-define-ops");
    let r = resolver(&dir, &up);
    r.define_formula("local/minus", &format!(r#"{FX}(currency: "USD", date: date)-1"#), &sig(SIG_NUM)).unwrap();
    let a = r.get("local/minus(date: 2026-10-30)").unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert!(receipt(&a.body).get("value").and_then(Value::as_str).unwrap().starts_with("0.1"));
    r.define_formula(
        "local/ratio",
        &format!(r#"{FX}(currency: "USD", date: date)/{PLAIN}(date: date)"#),
        &sig(SIG_NUM),
    )
    .unwrap();
    let b = r.get("local/ratio(date: 2026-10-30)").unwrap();
    assert_eq!(b.status, 200, "{}", String::from_utf8_lossy(&b.body));
}

// ---- 3. find and describe ----------------------------------------------------------------------

fn result_named<'a>(v: &'a Value, name: &str) -> &'a Value {
    array(v, "results").iter().find(|r| s(r, "name") == name).unwrap_or_else(|| panic!("{name} in {}", jcs(v)))
}

#[test]
fn mcp_find_shows_the_publisher_apart_from_the_source() {
    let up = dotted_upstream();
    let dir = TempDir::new("dns-mcp-find");
    let srv = server(&dir, &up);
    let v = tool(&srv, "find", r#"{"query":"fx_reference"}"#);
    let st = structured(&v);
    let f = result_named(st, FX);
    assert_eq!(s(f, "publisher"), HOST, "{}", jcs(f));
    assert_eq!(s(f, "source"), "ecb.europa.eu", "{}", jcs(f));
    // A stored function has a publisher and no source.
    let v = tool(&srv, "find", r#"{"query":"bank-x.example"}"#);
    let g = result_named(structured(&v), PLAIN);
    assert_eq!(s(g, "publisher"), HOST, "{}", jcs(g));
    assert!(g.get("source").is_none(), "{}", jcs(g));
}

#[test]
fn a_held_name_found_locally_has_the_host_as_publisher_too() {
    let up = dotted_upstream();
    let dir = TempDir::new("dns-find-held");
    let srv = server(&dir, &up);
    // Held after a get.
    tool(&srv, "get", &format!(r#"{{"request":{}}}"#, q(CALL)));
    let v = tool(&srv, "find", r#"{"query":"fx_reference"}"#);
    let f = result_named(structured(&v), FX);
    assert_eq!(s(f, "publisher"), HOST, "{}", jcs(f));
    assert_ne!(s(f, "publisher"), "ecb.europa.eu");
}

#[test]
fn mcp_describe_of_a_dotted_function_shows_the_publisher_and_the_source_apart() {
    let up = dotted_upstream();
    let dir = TempDir::new("dns-mcp-describe");
    let srv = server(&dir, &up);
    let v = tool(&srv, "describe", &format!(r#"{{"id":{}}}"#, q(FX)));
    let st = structured(&v);
    let d = st.get("describe").unwrap_or_else(|| panic!("describe in {}", jcs(st)));
    assert_eq!(s(d, "publisher"), HOST, "{}", jcs(d));
    let text = jcs(d);
    assert!(!text.contains(r#""publisher":"ecb.europa.eu""#), "{text}");
    let source = d.get("source").unwrap_or_else(|| panic!("a relay's source in {text}"));
    assert_eq!(s(source, "name"), "ecb.europa.eu", "{text}");
}

fn home(up: &Upstream) -> TempDir {
    let dir = TempDir::new("dns-cli");
    Config::in_dir(dir.path()).unwrap();
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let text = text
        .replace("\"peers\": {}", &format!("\"peers\": {{\"{HOST}\": \"{}\"}}", up.origin))
        .replace("\"find_meshes\": [\"datumesh.com\"]", &format!("\"find_meshes\": [\"{HOST}\"]"));
    std::fs::write(&path, text).unwrap();
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
fn cli_find_shows_the_publisher_apart_from_the_source() {
    let up = dotted_upstream();
    let dir = home(&up);
    let (code, out, err) = run(&dir, &["find", "fx_reference"]);
    assert_eq!(code, 0, "{err}");
    let v = json::parse(out.trim_end().as_bytes()).unwrap();
    let f = result_named(&v, FX);
    assert_eq!(s(f, "publisher"), HOST, "{out}");
    assert_eq!(s(f, "source"), "ecb.europa.eu", "{out}");
}

#[test]
fn cli_describe_shows_the_publisher_apart_from_the_source() {
    let up = dotted_upstream();
    let dir = home(&up);
    let (code, out, err) = run(&dir, &["describe", FX]);
    assert_eq!(code, 0, "{err}{out}");
    let d = json::parse(out.trim_end().as_bytes()).unwrap();
    assert_eq!(s(&d, "publisher"), HOST, "{out}");
    assert_eq!(s(d.get("source").unwrap_or_else(|| panic!("source in {out}")), "name"), "ecb.europa.eu");
}

// ---- 4. refusals -------------------------------------------------------------------------------

fn text_of(r: Result<datumesh_local::Reply, datumesh_local::Error>) -> String {
    match r {
        Ok(reply) => {
            assert_ne!(reply.status, 200, "refused, not answered");
            String::from_utf8_lossy(&reply.body).into_owned()
        }
        Err(e) => format!("{e:?} {e}"),
    }
}

#[test]
fn an_uppercase_dns_segment_and_a_dns_name_as_the_last_segment_are_refused() {
    let up = dotted_upstream();
    let dir = TempDir::new("dns-refused");
    let r = resolver(&dir, &up);
    let upper = r#"mesh-test.datumesh.com/relay/ECB.europa.eu/fx_reference(currency: "USD", date: 2026-10-30)"#;
    let last = "mesh-test.datumesh.com/relay/ecb.europa.eu(date: 2026-10-30)";
    for bad in [upper, last] {
        let t = text_of(r.get(bad));
        assert!(t.contains("E_CANON_NAME"), "{bad}: {t}");
    }
    // Nothing was asked of the mesh for a refused name.
    let before = up.requests();
    let _ = r.get(upper);
    assert_eq!(up.requests(), before);
}

#[test]
fn a_formula_body_with_a_refused_name_is_refused_at_define() {
    let up = dotted_upstream();
    let dir = TempDir::new("dns-refused-define");
    let r = resolver(&dir, &up);
    for body in [
        r#"mesh-test.datumesh.com/relay/ECB.europa.eu/fx_reference(currency: "USD", date: date)"#,
        "mesh-test.datumesh.com/relay/ecb.europa.eu(date: date)",
    ] {
        let e = r.define_formula("local/bad", body, &sig(SIG_NUM)).expect_err(body);
        let t = format!("{e:?} {e}");
        assert!(t.contains("E_CANON_NAME") || t.contains("E_FORMULA"), "{body}: {t}");
    }
    assert!(r.store().resolve_binding("local.invalid/bad", END).unwrap().is_none());
}

#[test]
fn a_name_defined_under_a_refused_segment_is_refused() {
    let up = dotted_upstream();
    let dir = TempDir::new("dns-refused-name");
    let r = resolver(&dir, &up);
    let body = format!(r#"{FX}(currency: "USD", date: date)"#);
    let e = r.define_formula("local/ECB.europa.eu/x", &body, &sig(SIG_NUM)).expect_err("uppercase DNS segment");
    let t = format!("{e:?} {e}");
    assert!(t.contains("E_CANON_NAME"), "{t}");
}
