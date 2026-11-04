//! The localhost endpoint: 127.0.0.1 only, the mesh's handlers over the local store, /cell
//! through the resolver (fetch, gate, forward), links on http://127.0.0.1:<port>.

mod common;

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::serve::{encode_path, Server};
use datumesh_local::{Error, LOCAL_LABEL};

/// GET over HTTP/1.0: status, headers (lowercase names) and body.
fn get(addr: SocketAddr, path: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(s, "GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n").unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let body = buf[split + 4..].to_vec();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    (status, headers, body)
}

fn header<'a>(h: &'a [(String, String)], k: &str) -> Option<&'a str> {
    h.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

#[test]
fn serve_refuses_any_address_but_127_0_0_1() {
    let up = upstream();
    let dir = TempDir::new("serve-addr");
    let r = Arc::new(resolver(&dir, &up));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    for a in ["0.0.0.0:0", "192.168.1.1:0", "[::1]:0", "[::]:0"] {
        let addr: SocketAddr = a.parse().unwrap();
        assert!(matches!(rt.block_on(Server::bind(r.clone(), addr)), Err(Error::Address(_))), "{a}");
    }
}

#[test]
fn serve_answers_with_the_mesh_handlers_over_the_local_store() {
    let up = upstream();
    let dir = TempDir::new("serve");
    let r = Arc::new(resolver(&dir, &up));
    r.define_formula(
        "local/closed",
        r#"mesh-test.datumesh.com/ecb/holiday(calendar: "TARGET", date: date)"#,
        &json::parse(
            br#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": "boolean"}"#,
        )
        .unwrap(),
    )
    .unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let server = rt.block_on(Server::bind(r.clone(), "127.0.0.1:0".parse().unwrap())).unwrap();
    let addr = server.local_addr();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = rt.spawn(server.run(async {
        let _ = rx.await;
    }));
    // The mesh document's links are on this endpoint.
    let (status, _, body) = get(addr, "/.well-known/datumesh-mesh.json");
    assert_eq!(status, 200);
    let doc = json::parse(&body).unwrap();
    let cell = doc.get("endpoints").and_then(|e| e.get("cell")).and_then(Value::as_str).unwrap().to_string();
    assert_eq!(cell, format!("http://{addr}/cell/{{cell}}"));
    assert_eq!(doc.get("host").and_then(Value::as_str), Some("local.invalid"));
    // The key document is this machine's.
    let (status, _, body) = get(addr, "/.well-known/datumesh.json");
    assert_eq!(status, 200);
    assert_eq!(body, r.key_document());
    // /cell fetches (through the resolver) and answers as get does.
    let (status, _, body) = get(addr, &format!("/cell/{}", encode_path(USD)));
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(body, r.get(USD).unwrap().body);
    // A local evaluation carries the signer's label.
    let (status, h, body) = get(addr, &format!("/cell/{}", encode_path("local/closed(date: 2026-12-25)")));
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(header(&h, "datumesh-signer"), Some(LOCAL_LABEL));
    // The other handlers: the receipt by hash, the slice endpoint over what is held.
    let rc = datumesh_core::Receipt::from_value(json::parse(&body).unwrap().get("receipt").unwrap()).unwrap();
    let (status, _, _) = get(addr, &format!("/receipt/{}", rc.hash()));
    assert_eq!(status, 200);
    let (status, _, body) = get(addr, &format!("/slice/{}", encode_path(FX_SLICE)));
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let _ = tx.send(());
    rt.block_on(handle).unwrap().unwrap();
}
