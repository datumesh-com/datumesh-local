//! Loopback (spec-0.5.0, 9.6, 2.5; `CONTRACT-local-0.5.0.md` item 3). spec-0.5.0 allows an unrooted
//! host's mesh document on a loopback origin (`http://127.0.0.1:<port>`): the document the localhost
//! endpoint serves is a valid mesh document, and core 0.3.8's `resolve_mesh` accepts it for
//! `local.invalid` together with the machine's key document.

mod common;

use std::io::{Read, Write};
use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_core::{resolve_mesh, Retrieval};
use datumesh_local::serve::Server;

fn http_get(addr: std::net::SocketAddr, path: &str) -> (u16, Vec<u8>) {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(s, "GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n").unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    (head.lines().next().unwrap().split(' ').nth(1).unwrap().parse().unwrap(), buf[split + 4..].to_vec())
}

#[test]
fn the_local_mesh_document_is_accepted_by_resolve_mesh() {
    let up = upstream();
    let dir = TempDir::new("loopback");
    let r = Arc::new(resolver(&dir, &up));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let server = rt.block_on(Server::bind(r.clone(), "127.0.0.1:0".parse().unwrap())).unwrap();
    let addr = server.local_addr();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = rt.spawn(server.run(async {
        let _ = rx.await;
    }));
    let (status, md) = http_get(addr, "/.well-known/datumesh-mesh.json");
    assert_eq!(status, 200);
    let (status, kd) = http_get(addr, "/.well-known/datumesh.json");
    assert_eq!(status, 200);
    let resolved = resolve_mesh(
        "local.invalid",
        Retrieval::Response { status: 200, body: Some(&kd) },
        Retrieval::Response { status: 200, body: Some(&md) },
    )
    .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&md)));
    assert_eq!(
        resolved.mesh.endpoints.get("cell").map(String::as_str),
        Some(format!("http://{addr}/cell/{{cell}}").as_str())
    );
    // Every endpoint of the document is on the loopback origin.
    let doc = json::parse(&md).unwrap();
    for (name, url) in doc.get("endpoints").and_then(Value::as_object).unwrap() {
        let url = url.as_str().unwrap();
        assert!(url.starts_with(&format!("http://{addr}/")), "{name}: {url}");
    }
    let _ = tx.send(());
    rt.block_on(handle).unwrap().unwrap();
}
