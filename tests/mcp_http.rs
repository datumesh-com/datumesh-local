//! CONTRACT-1.8b L2: MCP over Streamable HTTP on loopback (`mcp::serve_http`), the checks in
//! their order, the token for `define` and `publish`, and the token file. Requests are raw
//! HTTP over a `TcpStream`.

mod common;

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{serve_http, HttpServer};
use datumesh_local::Error;

const M: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

const GET_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}},"name":"get","arguments":{"request":"ecb/fx_rate(base: \"EUR\", quote: \"USD\", date: 2026-10-30)"}}}"#;

const DEFINE_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}},"name":"define","arguments":{"name":"local/closed","formula":"mesh-test.datumesh.com/ecb/holiday(calendar: \"TARGET\", date: date)","signature":{"args":{"date":{"type":"date","valid":true}},"order":["date"],"returns":"boolean"}}}}"#;

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn header(&self, k: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        json::parse(&self.body).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&self.body)))
    }
    fn error_code(&self) -> String {
        self.json()
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(Value::as_number_text)
            .unwrap_or_else(|| panic!("an error in {}", String::from_utf8_lossy(&self.body)))
            .to_string()
    }
}

/// One request over a fresh connection: `headers` as given (no others but Content-Length and
/// Connection: close).
fn send(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, String)], body: &str) -> Response {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()));
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").expect("a response head");
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Response { status, headers, body: buf[split + 4..].to_vec() }
}

struct Started {
    server: HttpServer,
    addr: SocketAddr,
    resolver: Arc<datumesh_local::Resolver>,
    _dir: TempDir,
    _up: Upstream,
}

fn start() -> Started {
    let up = upstream();
    let dir = TempDir::new("mcp-http");
    let resolver = Arc::new(resolver(&dir, &up));
    let server = serve_http(resolver.clone(), 0).unwrap();
    let addr = server.local_addrs().into_iter().find(|a| a.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
    Started { server, addr, resolver, _dir: dir, _up: up }
}

/// The headers of a right `tools/call` of `name`.
fn headers(addr: SocketAddr, name: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Host", addr.to_string()),
        ("Content-Type", "application/json".to_string()),
        ("MCP-Protocol-Version", "2026-07-28".to_string()),
        ("Mcp-Method", "tools/call".to_string()),
        ("Mcp-Name", name.to_string()),
    ]
}

fn without(mut h: Vec<(&'static str, String)>, k: &str) -> Vec<(&'static str, String)> {
    h.retain(|(n, _)| !n.eq_ignore_ascii_case(k));
    h
}

fn with(mut h: Vec<(&'static str, String)>, k: &'static str, v: &str) -> Vec<(&'static str, String)> {
    h = without(h, k);
    h.push((k, v.to_string()));
    h
}

#[test]
fn the_server_listens_on_loopback() {
    let st = start();
    let addrs = st.server.local_addrs();
    assert!(!addrs.is_empty());
    assert!(addrs.iter().all(|a| a.ip().is_loopback()), "{addrs:?}");
    assert!(addrs.iter().any(|a| a.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST)), "{addrs:?}");
}

#[test]
fn a_valid_get_is_the_answer_block() {
    let st = start();
    let r = send(st.addr, "POST", "/mcp", &headers(st.addr, "get"), GET_BODY);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert!(r.header("content-type").unwrap().starts_with("application/json"));
    let v = r.json();
    let a = v.get("result").and_then(|r| r.get("structuredContent")).expect("structuredContent");
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
    assert_eq!(String::from_utf8(a.get("value").unwrap().to_jcs()).unwrap(), r#"{"type":"number","value":"1.1617"}"#);
    assert!(a.get("signer").and_then(Value::as_str).unwrap().starts_with("mesh-test.datumesh.com#"));
    assert_eq!(a.get("signer_label"), Some(&Value::Null));
    assert_eq!(a.get("trust").and_then(Value::as_str), Some("primary"));
}

#[test]
fn origin_is_checked() {
    let st = start();
    let h = with(headers(st.addr, "get"), "Origin", "http://evil.example");
    assert_eq!(send(st.addr, "POST", "/mcp", &h, GET_BODY).status, 403);
    let h = with(headers(st.addr, "get"), "Origin", "http://localhost:3000");
    assert_eq!(send(st.addr, "POST", "/mcp", &h, GET_BODY).status, 200);
}

#[test]
fn host_is_checked() {
    let st = start();
    let h = with(headers(st.addr, "get"), "Host", "evil.example");
    assert_eq!(send(st.addr, "POST", "/mcp", &h, GET_BODY).status, 403);
}

#[test]
fn content_type_is_checked() {
    let st = start();
    let h = with(headers(st.addr, "get"), "Content-Type", "text/plain");
    assert_eq!(send(st.addr, "POST", "/mcp", &h, GET_BODY).status, 415);
    let h = with(headers(st.addr, "get"), "Content-Type", "application/json; charset=utf-8");
    assert_eq!(send(st.addr, "POST", "/mcp", &h, GET_BODY).status, 200);
}

#[test]
fn headers_must_match_the_body() {
    let st = start();
    let h = with(headers(st.addr, "get"), "Mcp-Name", "other");
    let r = send(st.addr, "POST", "/mcp", &h, GET_BODY);
    assert_eq!(r.status, 400);
    assert_eq!(r.error_code(), "-32020");
    let h = without(headers(st.addr, "get"), "MCP-Protocol-Version");
    let r = send(st.addr, "POST", "/mcp", &h, GET_BODY);
    assert_eq!(r.status, 400);
    assert_eq!(r.error_code(), "-32020");
}

#[test]
fn get_and_delete_are_405() {
    let st = start();
    let h = vec![("Host", st.addr.to_string())];
    assert_eq!(send(st.addr, "GET", "/mcp", &h, "").status, 405);
    assert_eq!(send(st.addr, "DELETE", "/mcp", &h, "").status, 405);
}

#[test]
fn define_needs_the_token() {
    let st = start();
    let h = headers(st.addr, "define");
    let r = send(st.addr, "POST", "/mcp", &h, DEFINE_BODY);
    assert_eq!(r.status, 401, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.header("www-authenticate"), Some("Bearer"));
    assert_eq!(r.error_code(), "-31001");
    let message = r.json().get("error").and_then(|e| e.get("message")).and_then(Value::as_str).map(str::to_string);
    assert_eq!(message.as_deref(), Some("unauthorized"));
    // A wrong token is the same.
    let wrong = with(h.clone(), "Authorization", &format!("Bearer {}", "0".repeat(64)));
    assert_eq!(send(st.addr, "POST", "/mcp", &wrong, DEFINE_BODY).status, 401);
    // The token from the file.
    let token = std::fs::read_to_string(st.server.token_path()).unwrap();
    let right = with(h, "Authorization", &format!("Bearer {}", token.trim()));
    let r = send(st.addr, "POST", "/mcp", &right, DEFINE_BODY);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
}

#[test]
fn the_token_file() {
    let st = start();
    let path = st.server.token_path();
    let token = std::fs::read_to_string(&path).unwrap();
    let token = token.trim().to_string();
    assert_eq!(token.len(), 64);
    assert!(token.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{token}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // A second start keeps the token.
    let Started { server, resolver, _dir, _up, .. } = st;
    drop(server);
    let again = serve_http(resolver.clone(), 0).unwrap();
    assert_eq!(again.token_path(), path);
    assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), token);
    drop(again);
    // Group or other permission bits: refused.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(serve_http(resolver, 0), Err(Error::Key(_))));
    }
}

#[test]
fn an_unknown_method_is_404_and_a_notification_is_202() {
    let st = start();
    let body = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"nope/x","params":{{{M}}}}}"#);
    let h = without(with(headers(st.addr, "get"), "Mcp-Method", "nope/x"), "Mcp-Name");
    let r = send(st.addr, "POST", "/mcp", &h, &body);
    assert_eq!(r.status, 404, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.error_code(), "-32601");

    let body = format!(r#"{{"jsonrpc":"2.0","method":"notifications/x","params":{{{M}}}}}"#);
    let h = without(with(headers(st.addr, "get"), "Mcp-Method", "notifications/x"), "Mcp-Name");
    let r = send(st.addr, "POST", "/mcp", &h, &body);
    assert_eq!(r.status, 202);
    assert!(r.body.is_empty());
}
