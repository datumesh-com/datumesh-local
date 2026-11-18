//! CONTRACT-1.7a-text L1-L3 (local 0.3.3): the MCP `get` tool returns a text blob's content,
//! checked against its SHA-256, and the `blob://` resource. The upstream is the test mesh with
//! `/blob/<sha>` counted and switchable to wrong bytes (`tests/common`).

mod common;

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::bundle::base64;
use datumesh_local::mcp::{serve_http, McpServer, Transport};
use datumesh_local::Resolver;

const M: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

/// Hebrew with niqqud and cantillation, Arabic with harakat, a leading space and a trailing
/// newline (nothing may be trimmed or normalised).
const TEXT: &str = " בְּרֵאשִׁ֖ית בָּרָ֣א אֱלֹהִ֑ים\nبِسْمِ ٱللَّهِ ٱلرَّحْمَٰنِ ٱلرَّحِيمِ\n";

fn q(s: &str) -> String {
    String::from_utf8(Value::String(s.to_string()).to_jcs()).unwrap()
}

fn jcs(v: &Value) -> String {
    String::from_utf8(v.to_jcs()).unwrap()
}

fn rpc(server: &McpServer, method: &str, extra: &str) -> Value {
    let comma = if extra.is_empty() { "" } else { "," };
    let line = format!(r#"{{"jsonrpc":"2.0","id":1,"method":{},"params":{{{M}{comma}{extra}}}}}"#, q(method));
    let out = server.handle(&line, Transport::Stdio).expect("a response");
    json::parse(out.as_bytes()).unwrap()
}

fn get(server: &McpServer, request: &str) -> Value {
    rpc(server, "tools/call", &format!(r#""name":"get","arguments":{{"request":{}}}"#, q(request)))
}

fn read(server: &McpServer, uri: &str) -> Value {
    rpc(server, "resources/read", &format!(r#""uri":{}"#, q(uri)))
}

fn result(v: &Value) -> &Value {
    v.get("result").unwrap_or_else(|| panic!("a result in {}", jcs(v)))
}

fn structured(v: &Value) -> &Value {
    result(v).get("structuredContent").expect("structuredContent")
}

fn is_error(v: &Value) -> bool {
    result(v).get("isError") == Some(&Value::Bool(true))
}

fn error_code(v: &Value) -> String {
    v.get("error")
        .and_then(|e| e.get("code"))
        .and_then(Value::as_number_text)
        .unwrap_or_else(|| panic!("an error in {}", jcs(v)))
        .to_string()
}

/// The texts of the tool result's content items (all of type `text`).
fn items(v: &Value) -> Vec<String> {
    match result(v).get("content") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|i| {
                assert_eq!(i.get("type").and_then(Value::as_str), Some("text"), "{}", jcs(i));
                i.get("text").and_then(Value::as_str).expect("a text").to_string()
            })
            .collect(),
        other => panic!("no content: {other:?}"),
    }
}

fn parsed(text: &str) -> Value {
    json::parse(text.as_bytes()).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn sha_of(bytes: &[u8]) -> String {
    datumesh_core::sha256_hex(bytes)
}

fn server_of(up: &Upstream, dir: &TempDir) -> McpServer {
    McpServer::new(Arc::new(resolver(dir, up)))
}

/// Text of `n` bytes (ASCII, valid UTF-8).
fn ascii(n: usize) -> Vec<u8> {
    (0..n).map(|i| b'a' + (i % 26) as u8).collect()
}

/// The answer block's call and receipt: it has the cell's receipt in its envelope.
fn assert_answer_block(block: &Value, call: &str, sha: &str, length: usize, media: &str) {
    assert_eq!(block.get("status").and_then(Value::as_number_text), Some("200"), "{}", jcs(block));
    assert_eq!(block.get("call").and_then(Value::as_str), Some(call));
    assert_eq!(jcs(block.get("value").unwrap()), format!(r#"{{"type":"blob","value":"{sha}:{length}:{media}"}}"#));
    let receipt = block.get("envelope").and_then(|e| e.get("receipt")).expect("a receipt in the envelope");
    assert_eq!(receipt.get("type").and_then(Value::as_str), Some("blob"));
    assert!(block.get("receipt_hash").and_then(Value::as_str).is_some_and(|h| !h.is_empty()));
}

/// An upstream with the text cell `tag` (TEXT, `text/plain`); the cell's call and the sha.
fn with_text(tag: &str) -> (Upstream, String, String) {
    let up = upstream();
    let sha = up.publish_blob_cell(tag, TEXT.as_bytes(), "text/plain", None);
    let call = text_cell_call(tag);
    (up, call, sha)
}

// L-T1
#[test]
fn get_of_a_text_blob_returns_the_checked_text() {
    let (up, call, sha) = with_text("t1");
    let dir = TempDir::new("text-t1");
    let srv = server_of(&up, &dir);
    let v = get(&srv, &call);
    assert!(!is_error(&v), "{}", jcs(&v));
    let c = items(&v);
    assert_eq!(c.len(), 3, "{c:?}");
    assert_eq!(c[0], TEXT, "the text itself, exactly the blob's bytes");
    assert_eq!(
        c[1],
        format!(r#"{{"blob_checked":true,"length":{},"media_type":"text/plain","sha256":"{sha}"}}"#, TEXT.len())
    );
    let block = parsed(&c[2]);
    assert_answer_block(&block, &call, &sha, TEXT.len(), "text/plain");
    let s = structured(&v);
    assert_eq!(s.get("text").and_then(Value::as_str), Some(TEXT));
    assert_eq!(s.get("blob_checked"), Some(&Value::Bool(true)));
    // The structured content is the answer block with `text` and `blob_checked` added.
    for k in ["status", "call", "uri", "value", "receipt_hash", "envelope"] {
        assert_eq!(s.get(k), block.get(k), "{k}");
    }
}

// L-T2
#[test]
fn the_same_cell_again_is_held_and_asks_the_mesh_for_no_blob() {
    let (up, call, sha) = with_text("t2");
    let dir = TempDir::new("text-t2");
    let srv = server_of(&up, &dir);
    let first = get(&srv, &call);
    assert_eq!(items(&first)[0], TEXT);
    let before = up.blob_requests();
    assert_eq!(before, 1, "the first get fetched the blob once");
    let again = get(&srv, &call);
    assert!(!is_error(&again), "{}", jcs(&again));
    let c = items(&again);
    assert_eq!(c.len(), 3);
    assert_eq!(c[0], TEXT);
    assert!(c[1].contains(&sha) && c[1].contains(r#""blob_checked":true"#), "{}", c[1]);
    assert_eq!(structured(&again).get("text").and_then(Value::as_str), Some(TEXT));
    assert_eq!(up.blob_requests(), before, "no new request to /blob");
}

// L-T3
#[test]
fn a_mesh_serving_other_bytes_is_e_blob_mismatch_and_no_text() {
    let (up, call, _) = with_text("t3");
    up.serve_wrong_blobs(true);
    let dir = TempDir::new("text-t3");
    let srv = server_of(&up, &dir);
    let v = get(&srv, &call);
    assert!(is_error(&v), "{}", jcs(&v));
    let c = items(&v);
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(c[0].starts_with("E_BLOB_MISMATCH"), "{}", c[0]);
    let s = structured(&v);
    assert_eq!(s.get("error").and_then(Value::as_str), Some("E_BLOB_MISMATCH"), "{}", jcs(s));
    assert!(s.get("detail").is_some());
    for k in ["text", "receipt", "envelope", "value", "blob_checked"] {
        assert!(s.get(k).is_none(), "{k} in {}", jcs(s));
    }
    let whole = jcs(result(&v));
    assert!(!whole.contains("not the bytes that were asked for"), "{whole}");
    assert!(!whole.contains("בְּרֵאשִׁ"), "{whole}");
}

// L-T4 (bytes that are not UTF-8, under text/plain)
#[test]
fn a_held_blob_that_is_not_utf8_under_text_plain_is_e_blob_mismatch() {
    let up = upstream();
    let bad: &[u8] = &[b'a', 0xff, 0xfe, b'b', 0xc3];
    let sha = up.publish_blob_cell("t4a", bad, "text/plain", None);
    let dir = TempDir::new("text-t4a");
    let r = Arc::new(resolver(&dir, &up));
    // Held first: the resolver fetches the cell and imports the blob.
    let call = text_cell_call("t4a");
    r.get(&call).unwrap();
    assert!(r.store().blob(&sha).unwrap().is_some(), "the blob is held");
    let srv = McpServer::new(r);
    let v = get(&srv, &call);
    assert!(is_error(&v), "{}", jcs(&v));
    let c = items(&v);
    assert_eq!(c.len(), 1);
    assert!(c[0].starts_with("E_BLOB_MISMATCH"), "{}", c[0]);
    let s = structured(&v);
    assert_eq!(s.get("error").and_then(Value::as_str), Some("E_BLOB_MISMATCH"));
    assert!(s.get("text").is_none() && s.get("envelope").is_none());
}

// L-T4 (a length that differs from the value's)
#[test]
fn a_held_blob_of_another_length_than_the_value_states_is_e_blob_mismatch() {
    let up = upstream();
    let sha = up.publish_blob_cell("t4b", TEXT.as_bytes(), "text/plain", Some(TEXT.len() + 1));
    let dir = TempDir::new("text-t4b");
    let r = Arc::new(resolver(&dir, &up));
    let call = text_cell_call("t4b");
    r.get(&call).unwrap();
    assert!(r.store().blob(&sha).unwrap().is_some(), "the blob is held");
    let srv = McpServer::new(r);
    let v = get(&srv, &call);
    assert!(is_error(&v), "{}", jcs(&v));
    let c = items(&v);
    assert_eq!(c.len(), 1);
    assert!(c[0].starts_with("E_BLOB_MISMATCH"), "{}", c[0]);
    let s = structured(&v);
    assert_eq!(s.get("error").and_then(Value::as_str), Some("E_BLOB_MISMATCH"));
    assert!(s.get("text").is_none() && s.get("envelope").is_none());
    assert!(!jcs(result(&v)).contains("בְּרֵאשִׁ"));
}

/// The blob object of a "not text" or "over the limit" reply.
fn assert_blob_only(v: &Value, call: &str, sha: &str, length: usize, media: &str) {
    assert!(!is_error(v), "{}", jcs(v));
    let c = items(v);
    assert_eq!(c.len(), 2, "{c:?}");
    assert_eq!(
        c[0],
        format!(
            r#"{{"blob":{{"length":{length},"media_type":"{media}","sha256":"{sha}","uri":"blob://{sha}","url":"https://{HOST}/blob/{sha}"}},"blob_checked":false}}"#
        )
    );
    assert_answer_block(&parsed(&c[1]), call, sha, length, media);
    let s = structured(v);
    assert!(s.get("text").is_none(), "{}", jcs(s));
    assert_eq!(s.get("blob_checked"), Some(&Value::Bool(false)));
    let blob = s.get("blob").expect("blob");
    assert_eq!(blob.get("uri").and_then(Value::as_str), Some(format!("blob://{sha}").as_str()));
    assert_eq!(blob.get("length").and_then(Value::as_number_text), Some(length.to_string().as_str()));
    assert!(s.get("blob_unavailable").is_none());
}

// L-T5
#[test]
fn a_text_blob_over_64_kib_is_not_returned_as_text() {
    let up = upstream();
    let bytes = ascii(65_537);
    let sha = up.publish_blob_cell("t5a", &bytes, "text/plain", None);
    let dir = TempDir::new("text-t5a");
    let srv = server_of(&up, &dir);
    let v = get(&srv, &text_cell_call("t5a"));
    assert_blob_only(&v, &text_cell_call("t5a"), &sha, 65_537, "text/plain");
}

// L-T5
#[test]
fn an_octet_stream_blob_is_not_returned_as_text() {
    let up = upstream();
    let bytes = b"plain ascii, but declared binary".to_vec();
    let sha = up.publish_blob_cell("t5b", &bytes, "application/octet-stream", None);
    let dir = TempDir::new("text-t5b");
    let srv = server_of(&up, &dir);
    let v = get(&srv, &text_cell_call("t5b"));
    assert_blob_only(&v, &text_cell_call("t5b"), &sha, bytes.len(), "application/octet-stream");
}

// L-T5 (the limit is inclusive: 65,536 bytes is text)
#[test]
fn a_text_blob_of_exactly_64_kib_is_returned_as_text() {
    let up = upstream();
    let bytes = ascii(65_536);
    up.publish_blob_cell("t5c", &bytes, "text/plain", None);
    let dir = TempDir::new("text-t5c");
    let srv = server_of(&up, &dir);
    let v = get(&srv, &text_cell_call("t5c"));
    let c = items(&v);
    assert_eq!(c.len(), 3);
    assert_eq!(c[0].as_bytes(), &bytes[..]);
}

// L-T6
#[test]
fn a_text_blob_not_held_offline_is_unavailable_but_not_an_error() {
    let up = upstream();
    let sha = up.publish_blob_cell("t6", TEXT.as_bytes(), "text/plain", None);
    let dir = TempDir::new("text-t6");
    let r = Arc::new(resolver(&dir, &up));
    // The cell is held (a prefetch without blob content), its blob is not.
    r.prefetch(TEXT_SLICE, None, false).unwrap();
    assert!(r.store().blob(&sha).unwrap().is_none(), "the blob is not held");
    r.set_offline(true);
    let srv = McpServer::new(r);
    let v = get(&srv, &text_cell_call("t6"));
    assert!(!is_error(&v), "{}", jcs(&v));
    let c = items(&v);
    assert_eq!(c.len(), 2, "{c:?}");
    let first = parsed(&c[0]);
    assert_eq!(first.get("blob_checked"), Some(&Value::Bool(false)));
    assert!(first.get("blob_unavailable").and_then(Value::as_str).is_some_and(|r| !r.is_empty()), "{}", c[0]);
    assert_eq!(
        first.get("blob").and_then(|b| b.get("uri")).and_then(Value::as_str),
        Some(format!("blob://{sha}").as_str())
    );
    assert_answer_block(&parsed(&c[1]), &text_cell_call("t6"), &sha, TEXT.len(), "text/plain");
    let s = structured(&v);
    assert!(s.get("text").is_none());
    assert!(s.get("blob_unavailable").and_then(Value::as_str).is_some());
    assert_eq!(s.get("blob_checked"), Some(&Value::Bool(false)));
    assert!(s.get("envelope").and_then(|e| e.get("receipt")).is_some(), "the receipt stands");
}

// L-T7
#[test]
fn a_number_value_keeps_the_0_3_2_shape() {
    let up = upstream();
    let dir = TempDir::new("text-t7");
    let srv = server_of(&up, &dir);
    let v = get(&srv, USD);
    assert!(!is_error(&v));
    let c = items(&v);
    assert_eq!(c.len(), 1, "{c:?}");
    let block = parsed(&c[0]);
    assert_eq!(jcs(block.get("value").unwrap()), r#"{"type":"number","value":"1.1617"}"#);
    let s = structured(&v);
    for k in ["text", "blob", "blob_checked", "blob_unavailable"] {
        assert!(s.get(k).is_none(), "{k}");
        assert!(block.get(k).is_none(), "{k}");
    }
}

// L3
#[test]
fn the_descriptions_mention_the_checked_text() {
    let up = upstream();
    let dir = TempDir::new("text-l3");
    let srv = server_of(&up, &dir);
    let v = rpc(&srv, "tools/list", "");
    let Some(Value::Array(tools)) = result(&v).get("tools") else { panic!("no tools") };
    let get_tool = tools.iter().find(|t| t.get("name").and_then(Value::as_str) == Some("get")).unwrap();
    let d = get_tool.get("description").and_then(Value::as_str).unwrap();
    assert!(
        d.contains("A text blob's content (up to 64 KiB) comes first, checked against its SHA-256 (blob_checked); otherwise its blob:// address and size."),
        "{d}"
    );
    let v = rpc(&srv, "server/discover", "");
    let i = result(&v).get("instructions").and_then(Value::as_str).unwrap();
    assert!(i.contains("checked"), "{i}");
}

fn blob_template() -> String {
    r#"{"description":"Blob content by SHA-256, checked against it: text for valid UTF-8 up to 64 KiB, otherwise base64.","mimeType":"application/octet-stream","name":"blob","uriTemplate":"blob://{sha256}"}"#
        .to_string()
}

// L-T8
#[test]
fn the_templates_list_has_the_blob_template() {
    let up = upstream();
    let dir = TempDir::new("text-t8a");
    let srv = server_of(&up, &dir);
    let v = rpc(&srv, "resources/templates/list", "");
    let Some(Value::Array(ts)) = result(&v).get("resourceTemplates") else { panic!("{}", jcs(&v)) };
    let blob = ts.iter().find(|t| t.get("name").and_then(Value::as_str) == Some("blob")).expect("a blob template");
    assert_eq!(jcs(blob), blob_template());
    assert_eq!(ts.len(), 4, "cell, name, class and blob");
}

// L-T8
#[test]
fn reading_the_blob_uri_gives_the_text_after_a_get() {
    let (up, call, sha) = with_text("t8b");
    let dir = TempDir::new("text-t8b");
    let srv = server_of(&up, &dir);
    assert_eq!(items(&get(&srv, &call))[0], TEXT);
    let uri = format!("blob://{sha}");
    let v = read(&srv, &uri);
    let Some(Value::Array(contents)) = result(&v).get("contents") else { panic!("{}", jcs(&v)) };
    assert_eq!(contents.len(), 1);
    let c = &contents[0];
    assert_eq!(c.get("uri").and_then(Value::as_str), Some(uri.as_str()));
    assert_eq!(c.get("mimeType").and_then(Value::as_str), Some("text/plain; charset=utf-8"));
    assert_eq!(c.get("text").and_then(Value::as_str), Some(TEXT));
    assert!(c.get("blob").is_none());
}

// L-T8
#[test]
fn an_unknown_or_malformed_blob_uri_is_32602() {
    let up = upstream();
    let dir = TempDir::new("text-t8c");
    let srv = server_of(&up, &dir);
    let unknown = format!("blob://{}", "ab".repeat(32));
    let v = read(&srv, &unknown);
    assert_eq!(error_code(&v), "-32602", "{}", jcs(&v));
    let msg = v.get("error").and_then(|e| e.get("message")).and_then(Value::as_str).unwrap();
    assert_eq!(msg, format!("Unknown resource: {unknown} (not held here; get the cell first)"));
    for bad in ["blob://xyz", "blob://", &format!("blob://{}", "AB".repeat(32)), &format!("blob://{}", "ab".repeat(31))]
    {
        let v = read(&srv, bad);
        assert_eq!(error_code(&v), "-32602", "{bad}: {}", jcs(&v));
    }
}

// L-T8
#[test]
fn a_non_utf8_blob_and_a_text_over_64_kib_are_read_as_base64() {
    let up = upstream();
    let binary: Vec<u8> = vec![0x00, 0xff, 0xfe, 0x80, b'a', 0xc3];
    let big = ascii(65_537);
    let sha_bin = up.publish_blob_cell("t8d-bin", &binary, "application/octet-stream", None);
    let sha_big = up.publish_blob_cell("t8d-big", &big, "text/plain", None);
    let dir = TempDir::new("text-t8d");
    let srv = server_of(&up, &dir);
    // After the gets the blobs are held here.
    get(&srv, &text_cell_call("t8d-bin"));
    get(&srv, &text_cell_call("t8d-big"));
    for (sha, bytes) in [(sha_bin, &binary), (sha_big, &big)] {
        let uri = format!("blob://{sha}");
        let v = read(&srv, &uri);
        let Some(Value::Array(contents)) = result(&v).get("contents") else { panic!("{}", jcs(&v)) };
        let c = &contents[0];
        assert_eq!(c.get("uri").and_then(Value::as_str), Some(uri.as_str()));
        assert_eq!(c.get("mimeType").and_then(Value::as_str), Some("application/octet-stream"));
        assert_eq!(c.get("blob").and_then(Value::as_str), Some(base64(bytes).as_str()));
        assert!(c.get("text").is_none());
    }
}

// L-T9 (HTTP)

fn send(addr: SocketAddr, headers: &[(&str, String)], body: &str) -> Value {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    let mut req = String::from("POST /mcp HTTP/1.1\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()));
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").expect("a response head");
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}: {}", String::from_utf8_lossy(&buf[split + 4..]));
    json::parse(&buf[split + 4..]).unwrap()
}

fn headers(addr: SocketAddr, method: &str, name: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Host", addr.to_string()),
        ("Content-Type", "application/json".to_string()),
        ("MCP-Protocol-Version", "2026-07-28".to_string()),
        ("Mcp-Method", method.to_string()),
        ("Mcp-Name", name.to_string()),
    ]
}

// L-T9
#[test]
fn over_http_get_returns_the_text_and_the_blob_resource_reads_it() {
    let (up, call, sha) = with_text("t9");
    let dir = TempDir::new("text-t9");
    let resolver: Arc<Resolver> = Arc::new(self::resolver(&dir, &up));
    let server = serve_http(resolver, 0).unwrap();
    let addr = server.local_addrs().into_iter().find(|a| a.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{M},"name":"get","arguments":{{"request":{}}}}}}}"#,
        q(&call)
    );
    let v = send(addr, &headers(addr, "tools/call", "get"), &body);
    assert!(!is_error(&v), "{}", jcs(&v));
    let c = items(&v);
    assert_eq!(c.len(), 3, "{c:?}");
    assert_eq!(c[0], TEXT);
    assert!(c[1].contains(&sha) && c[1].contains(r#""blob_checked":true"#));
    assert_eq!(structured(&v).get("text").and_then(Value::as_str), Some(TEXT));
    let uri = format!("blob://{sha}");
    let body = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"resources/read","params":{{{M},"uri":{}}}}}"#, q(&uri));
    let name = format!("=?base64?{}?=", base64(uri.as_bytes()));
    let v = send(addr, &headers(addr, "resources/read", &name), &body);
    let Some(Value::Array(contents)) = result(&v).get("contents") else { panic!("{}", jcs(&v)) };
    assert_eq!(contents[0].get("text").and_then(Value::as_str), Some(TEXT));
    assert_eq!(contents[0].get("mimeType").and_then(Value::as_str), Some("text/plain; charset=utf-8"));
    drop(server);
}

#[test]
fn sha_helper_matches_the_receipt() {
    // The sha the stub reports is the SHA-256 of the bytes (guards the helper itself).
    let up = upstream();
    let sha = up.publish_blob_cell("helper", TEXT.as_bytes(), "text/plain", None);
    assert_eq!(sha, sha_of(TEXT.as_bytes()));
}
