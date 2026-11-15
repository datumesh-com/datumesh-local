//! CONTRACT-1.8b L1: MCP over stdio, in process through `McpServer::handle` (the protocol
//! revision 2026-07-28), with item 10 (a formula with no inputs is refused).

mod common;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Transport};

/// The modern `_meta` (`M`).
const M: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

const CLOSED: &str = r#"{"name":"local/closed","formula":"mesh-test.datumesh.com/ecb/holiday(calendar: \"TARGET\", date: date)","signature":{"args":{"date":{"type":"date","valid":true}},"order":["date"],"returns":"boolean"}}"#;

fn q(s: &str) -> String {
    String::from_utf8(Value::String(s.to_string()).to_jcs()).unwrap()
}

fn server(up: &Upstream, dir: &TempDir) -> McpServer {
    McpServer::new(Arc::new(resolver(dir, up)))
}

/// One request with `M` (and `extra` members, a JSON object body without braces, if not empty);
/// the parsed response.
fn rpc(server: &McpServer, method: &str, extra: &str) -> Value {
    let comma = if extra.is_empty() { "" } else { "," };
    let line = format!(r#"{{"jsonrpc":"2.0","id":1,"method":{},"params":{{{M}{comma}{extra}}}}}"#, q(method));
    let out = server.handle(&line, Transport::Stdio).expect("a response");
    json::parse(out.as_bytes()).unwrap()
}

/// `tools/call` with `arguments` (a JSON object text).
fn tool(server: &McpServer, name: &str, arguments: &str) -> Value {
    rpc(server, "tools/call", &format!(r#""name":{},"arguments":{arguments}"#, q(name)))
}

fn result(v: &Value) -> &Value {
    v.get("result").unwrap_or_else(|| panic!("a result in {}", String::from_utf8_lossy(&v.to_jcs())))
}

fn error_code(v: &Value) -> String {
    v.get("error")
        .and_then(|e| e.get("code"))
        .and_then(Value::as_number_text)
        .unwrap_or_else(|| panic!("an error in {}", String::from_utf8_lossy(&v.to_jcs())))
        .to_string()
}

fn structured(v: &Value) -> &Value {
    result(v).get("structuredContent").expect("structuredContent")
}

fn is_error(v: &Value) -> bool {
    result(v).get("isError") == Some(&Value::Bool(true))
}

fn array<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        other => panic!("{k} is not an array: {other:?}"),
    }
}

fn jcs(v: &Value) -> String {
    String::from_utf8(v.to_jcs()).unwrap()
}

fn get_usd(server: &McpServer) -> Value {
    tool(server, "get", &format!(r#"{{"request":{}}}"#, q(USD)))
}

fn define_closed(server: &McpServer) -> Value {
    tool(server, "define", CLOSED)
}

#[test]
fn server_discover() {
    let up = upstream();
    let dir = TempDir::new("mcp-discover");
    let srv = server(&up, &dir);
    let v = rpc(&srv, "server/discover", "");
    let r = result(&v);
    assert_eq!(r.get("resultType").and_then(Value::as_str), Some("complete"));
    assert_eq!(jcs(r.get("supportedVersions").unwrap()), r#"["2026-07-28"]"#);
    let caps = r.get("capabilities").and_then(Value::as_object).unwrap();
    let mut names: Vec<&str> = caps.iter().map(|(k, _)| k.as_str()).collect();
    names.sort();
    assert_eq!(names, ["resources", "tools"]);
    assert_eq!(r.get("ttlMs").and_then(Value::as_number_text), Some("60000"));
}

#[test]
fn tools_list_has_the_eight_tools_in_order() {
    let up = upstream();
    let dir = TempDir::new("mcp-tools");
    let srv = server(&up, &dir);
    let v = rpc(&srv, "tools/list", "");
    let tools = array(result(&v), "tools");
    let names: Vec<&str> = tools.iter().map(|t| t.get("name").and_then(Value::as_str).unwrap()).collect();
    assert_eq!(names, ["find", "get", "explain", "describe", "verify", "define", "publish", "subscribe"]);
    for t in tools {
        let schema = t.get("inputSchema").unwrap();
        assert_eq!(schema.get("type").and_then(Value::as_str), Some("object"));
        assert_eq!(schema.get("additionalProperties"), Some(&Value::Bool(false)));
    }
}

#[test]
fn meta_is_required_and_its_version_checked() {
    let up = upstream();
    let dir = TempDir::new("mcp-meta");
    let srv = server(&up, &dir);
    let out = srv
        .handle(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#, Transport::Stdio)
        .expect("a response");
    let v = json::parse(out.as_bytes()).unwrap();
    assert_eq!(error_code(&v), "-32602");
    let out = srv
        .handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-11-25","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
            Transport::Stdio,
        )
        .expect("a response");
    let v = json::parse(out.as_bytes()).unwrap();
    assert_eq!(error_code(&v), "-32022");
    let data = v.get("error").and_then(|e| e.get("data")).unwrap();
    assert_eq!(jcs(data.get("supported").unwrap()), r#"["2026-07-28"]"#);
    assert_eq!(data.get("requested").and_then(Value::as_str), Some("2025-11-25"));
}

#[test]
fn get_of_usd_is_the_answer_block() {
    let up = upstream();
    let dir = TempDir::new("mcp-get");
    let srv = server(&up, &dir);
    let v = get_usd(&srv);
    let a = structured(&v);
    assert_eq!(a.get("status").and_then(Value::as_number_text), Some("200"));
    assert_eq!(a.get("outcome").and_then(Value::as_str), Some("ok"));
    assert_eq!(
        a.get("call").and_then(Value::as_str),
        Some(r#"mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", date: 2026-10-30, quote: "USD")"#)
    );
    assert_eq!(
        a.get("uri").and_then(Value::as_str),
        Some("cell://mesh-test.datumesh.com/ecb/fx_rate?base=EUR&date=2026-10-30&quote=USD")
    );
    assert_eq!(a.get("licence").and_then(Value::as_str), Some("CC-BY-4.0"));
    assert_eq!(jcs(a.get("value").unwrap()), r#"{"type":"number","value":"1.1617"}"#);
    assert!(a.get("signer").and_then(Value::as_str).unwrap().starts_with("mesh-test.datumesh.com#"));
    assert_eq!(a.get("signer_label"), Some(&Value::Null));
    assert_eq!(a.get("trust").and_then(Value::as_str), Some("primary"));
}

#[test]
fn get_by_cell_uri_gives_the_same_receipt() {
    let up = upstream();
    let dir = TempDir::new("mcp-get-uri");
    let srv = server(&up, &dir);
    let first = get_usd(&srv);
    let hash = first_hash(&first);
    assert!(hash.is_some());
    let uri = "cell://mesh-test.datumesh.com/ecb/fx_rate?base=EUR&date=2026-10-30&quote=USD";
    let v = tool(&srv, "get", &format!(r#"{{"request":{}}}"#, q(uri)));
    assert_eq!(first_hash(&v), hash);
}

fn first_hash(v: &Value) -> Option<String> {
    structured(v).get("receipt_hash").and_then(Value::as_str).map(str::to_string)
}

#[test]
fn define_then_get_a_local_formula() {
    let up = upstream();
    let dir = TempDir::new("mcp-define");
    let srv = server(&up, &dir);
    let v = define_closed(&srv);
    assert!(!is_error(&v), "{}", jcs(&v));
    let d = structured(&v);
    assert_eq!(d.get("name").and_then(Value::as_str), Some("local.invalid/closed"));
    assert_eq!(d.get("call").and_then(Value::as_str), Some("local.invalid/closed(date: date)"));
    assert_eq!(d.get("uri").and_then(Value::as_str), Some("cell://local.invalid/closed"));
    let v = tool(&srv, "get", r#"{"request":"local/closed(date: 2026-12-25)"}"#);
    let a = structured(&v);
    assert_eq!(jcs(a.get("value").unwrap()), r#"{"type":"boolean","value":true}"#);
    assert_eq!(a.get("signer_label").and_then(Value::as_str), Some("local, verifies only on this machine"));
    assert_eq!(a.get("trust").and_then(Value::as_str), Some("computed"));
}

#[test]
fn define_refuses_a_formula_with_no_inputs() {
    let up = upstream();
    let dir = TempDir::new("mcp-no-inputs");
    let srv = server(&up, &dir);
    let v = tool(
        &srv,
        "define",
        r#"{"name":"local/const","formula":"1 + 1","signature":{"args":{},"order":[],"returns":{"type":"number","unit":"1"}}}"#,
    );
    assert!(is_error(&v), "{}", jcs(&v));
    let s = structured(&v);
    assert_eq!(s.get("error").and_then(Value::as_str), Some("error"));
    assert!(s.get("detail").and_then(Value::as_str).unwrap().contains("formula with no inputs"));
}

#[test]
fn define_of_a_rooted_function_over_an_unrooted_input_is_refused() {
    let up = upstream();
    let dir = TempDir::new("mcp-unrooted");
    let srv = server(&up, &dir);
    assert!(!is_error(&define_closed(&srv)));
    let v = tool(
        &srv,
        "define",
        r#"{"name":"example.org/f","formula":"local.invalid/closed(date: date)","signature":{"args":{"date":{"type":"date","valid":true}},"order":["date"],"returns":"boolean"}}"#,
    );
    assert!(is_error(&v), "{}", jcs(&v));
    assert_eq!(structured(&v).get("error").and_then(Value::as_str), Some("E_UNROOTED_INPUT"));
}

#[test]
fn publish_two_stored_cells_under_local() {
    let up = upstream();
    let dir = TempDir::new("mcp-publish");
    // Rule A: the function binds at the resolver's now, so the cells' tx is that instant (or later).
    let clock = ManualClock::new("2026-11-15T12:00:00.000000Z");
    let srv = McpServer::new(Arc::new(resolver_at(&dir, &up, &clock)));
    let cell = |day: &str, n: &str| {
        format!(
            r#"{{"cell":{{"fn":"local/level","args":{{"day":{{"type":"date","value":"{day}"}}}}}},"value":{{"type":"number","value":"{n}"}},"tx":"2026-11-15T12:00:00.000000Z","asof":"2026-11-02T00:00:00.000000Z","licence":"CC0-1.0","summary":"A level."}}"#
        )
    };
    let lines = format!(
        r#"{{"lines":[{{"function":"local/level","signature":{{"args":{{"day":{{"type":"date","valid":true}}}},"order":["day"],"returns":{{"type":"number","unit":"1"}}}}}},{},{}]}}"#,
        cell("2026-11-01", "1"),
        cell("2026-11-02", "2")
    );
    let v = tool(&srv, "publish", &lines);
    assert!(!is_error(&v), "{}", jcs(&v));
    let published = array(structured(&v), "published");
    assert_eq!(published.len(), 2);
    assert_eq!(published[0].get("call").and_then(Value::as_str), Some("local.invalid/level(day: 2026-11-01)"));
    assert_eq!(published[0].get("uri").and_then(Value::as_str), Some("cell://local.invalid/level?day=2026-11-01"));
}

#[test]
fn subscribe_is_not_implemented() {
    let up = upstream();
    let dir = TempDir::new("mcp-subscribe");
    let srv = server(&up, &dir);
    let v = tool(&srv, "subscribe", &format!(r#"{{"request":{}}}"#, q(USD)));
    assert!(is_error(&v));
    assert_eq!(jcs(structured(&v)), r#"{"error":"not_implemented","tool":"subscribe"}"#);
}

#[test]
fn unknown_tool_and_unsupported_method() {
    let up = upstream();
    let dir = TempDir::new("mcp-unknown");
    let srv = server(&up, &dir);
    let v = tool(&srv, "nope", "{}");
    assert_eq!(error_code(&v), "-32602");
    assert_eq!(v.get("error").and_then(|e| e.get("message")).and_then(Value::as_str), Some("Unknown tool: nope"));
    let v = rpc(&srv, "subscriptions/listen", "");
    assert_eq!(error_code(&v), "-32601");
}

#[test]
fn legacy_initialize_then_requests_without_meta() {
    let up = upstream();
    let dir = TempDir::new("mcp-legacy");
    let srv = server(&up, &dir);
    let out = srv
        .handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
            Transport::Stdio,
        )
        .expect("a response");
    let v = json::parse(out.as_bytes()).unwrap();
    assert_eq!(result(&v).get("protocolVersion").and_then(Value::as_str), Some("2025-06-18"));
    assert!(srv.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, Transport::Stdio).is_none());
    let out = srv
        .handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#, Transport::Stdio)
        .expect("a response");
    let v = json::parse(out.as_bytes()).unwrap();
    assert_eq!(array(result(&v), "tools").len(), 8);
}

#[test]
fn resources() {
    let up = upstream();
    let dir = TempDir::new("mcp-resources");
    let srv = server(&up, &dir);
    assert!(!is_error(&define_closed(&srv)));
    let first = get_usd(&srv);
    let envelope_of_get = jcs(structured(&first).get("envelope").unwrap());

    let v = rpc(&srv, "resources/list", "");
    let listed = array(result(&v), "resources");
    let entry = listed
        .iter()
        .find(|r| r.get("uri").and_then(Value::as_str) == Some("datumesh://name/local.invalid/closed"))
        .unwrap_or_else(|| panic!("closed is listed in {}", jcs(result(&v))));
    assert_eq!(entry.get("name").and_then(Value::as_str), Some("local.invalid/closed"));
    assert_eq!(entry.get("mimeType").and_then(Value::as_str), Some("application/json"));

    let v = rpc(&srv, "resources/templates/list", "");
    let templates = array(result(&v), "resourceTemplates");
    assert!(templates.iter().any(|t| t.get("uriTemplate").and_then(Value::as_str) == Some("cell://{+cell}")));

    let v = rpc(&srv, "resources/read", r#""uri":"datumesh://name/local.invalid/closed""#);
    let text = array(result(&v), "contents")[0].get("text").and_then(Value::as_str).unwrap();
    let doc = json::parse(text.as_bytes()).unwrap();
    for k in ["name", "fn_hash", "descriptor", "bound"] {
        assert!(doc.get(k).is_some(), "{k} in {text}");
    }

    let uri = structured(&first).get("uri").and_then(Value::as_str).unwrap().to_string();
    let v = rpc(&srv, "resources/read", &format!(r#""uri":{}"#, q(&uri)));
    let text = array(result(&v), "contents")[0].get("text").and_then(Value::as_str).unwrap();
    let env = json::parse(text.as_bytes()).unwrap();
    assert_eq!(jcs(&env), envelope_of_get);

    let v = rpc(&srv, "resources/read", r#""uri":"datumesh://name/nothing.invalid/here""#);
    assert_eq!(error_code(&v), "-32602");
}

#[test]
fn classes_expand_to_methods() {
    let up = upstream_classes();
    let dir = TempDir::new("mcp-classes");
    let r = Arc::new(resolver(&dir, &up));
    // The resolver holds the class: it has described it from the forward mesh.
    let (_held, _functions) = held_classes(&up);
    assert_eq!(r.describe(VERSE_CLASS).unwrap().status, 200);
    let srv = McpServer::new(r);

    let v = tool(&srv, "find", r#"{"query":"verse","remote":true}"#);
    let results = array(structured(&v), "results");
    let at = results
        .iter()
        .position(|e| e.get("name").and_then(Value::as_str) == Some(VERSE_CLASS))
        .unwrap_or_else(|| panic!("{VERSE_CLASS} in {}", jcs(structured(&v))));
    assert_eq!(results[at].get("kind").and_then(Value::as_str), Some("class"));
    let methods: Vec<&Value> =
        results[at + 1..].iter().take_while(|e| e.get("kind").and_then(Value::as_str) == Some("method")).collect();
    let names: Vec<&str> = methods.iter().map(|e| e.get("name").and_then(Value::as_str).unwrap()).collect();
    assert!(!names.is_empty());
    for n in &names {
        assert!(n.starts_with(&format!("{VERSE_CLASS}/")), "{n}");
    }

    let v = rpc(&srv, "resources/list", "");
    let listed: Vec<&str> =
        array(result(&v), "resources").iter().filter_map(|r| r.get("uri").and_then(Value::as_str)).collect();
    assert!(listed.contains(&format!("datumesh://class/{VERSE_CLASS}").as_str()), "{listed:?}");
    for n in &names {
        assert!(listed.contains(&format!("datumesh://name/{n}").as_str()), "{n} in {listed:?}");
    }
}
