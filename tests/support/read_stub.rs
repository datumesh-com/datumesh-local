//! A stub mesh for the Build 2 Part 2 read path (CONTRACT-build2-part2-read, R3/R4): the real
//! test mesh (`common::Upstream`: cells, receipts and blobs signed by the test key) with `/find`
//! replaced by canned answers in datumesh-http 0.4.0's shape, and `/find?summary=1` by a canned
//! catalogue summary. Every raw `/find` query string is kept.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::common::Upstream;

pub struct ReadStub {
    /// Every raw query string `/find` was asked, in order.
    pub queries: Mutex<Vec<String>>,
    /// Decoded `q` (lower-cased) to the answer body.
    answers: Mutex<HashMap<String, String>>,
    /// The body of `/find?summary=1`; `None` is a 500.
    summary: Mutex<Option<String>>,
    pub fail_find: AtomicBool,
    _rt: tokio::runtime::Runtime,
    pub origin: String,
}

pub fn pct_decode(s: &str) -> String {
    let b = s.replace('+', " ").into_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl ReadStub {
    /// The stub in front of `up`'s mesh, on 127.0.0.1.
    pub fn start(up: &Upstream) -> Arc<ReadStub> {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let addr = listener.local_addr().unwrap();
        let stub = Arc::new(ReadStub {
            queries: Mutex::new(Vec::new()),
            answers: Mutex::new(HashMap::new()),
            summary: Mutex::new(None),
            fail_find: AtomicBool::new(false),
            _rt: rt,
            origin: format!("http://{addr}"),
        });
        let s = stub.clone();
        let handler = move |req: axum::extract::Request| {
            let s = s.clone();
            async move {
                let raw = req.uri().query().unwrap_or("").to_string();
                s.queries.lock().unwrap().push(raw.clone());
                let param = |k: &str| raw.split('&').find_map(|p| p.strip_prefix(&format!("{k}=")).map(pct_decode));
                let (status, body) = if s.fail_find.load(Ordering::SeqCst) {
                    (500, r#"{"error":"stub failure"}"#.to_string())
                } else if param("summary").is_some() {
                    match s.summary.lock().unwrap().clone() {
                        Some(b) => (200, b),
                        None => (500, r#"{"error":"no summary"}"#.to_string()),
                    }
                } else {
                    let q = param("q").unwrap_or_default().to_lowercase();
                    let body = s.answers.lock().unwrap().get(&q).cloned().unwrap_or_else(|| {
                        format!(
                            r#"{{"query":{},"vector":false,"results":[],"resolution":{{"status":"none"}}}}"#,
                            datumesh_core::json::Value::String(q.clone())
                                .to_jcs()
                                .iter()
                                .map(|&c| c as char)
                                .collect::<String>()
                        )
                    });
                    (200, body)
                };
                axum::http::Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap()
            }
        };
        let router = axum::Router::new().route("/find", axum::routing::get(handler)).fallback_service(up.mesh.router());
        stub._rt.spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        stub
    }

    /// Answers `/find?q=<query>` (matched lower-cased) with `body`.
    pub fn answer(&self, query: &str, body: &str) {
        self.answers.lock().unwrap().insert(query.to_lowercase(), body.to_string());
    }

    /// Sets the body of `/find?summary=1` (`None`: a 500).
    pub fn set_summary(&self, body: Option<&str>) {
        *self.summary.lock().unwrap() = body.map(str::to_string);
    }

    /// The queries of the `/find` requests that were not for the summary.
    pub fn plain_queries(&self) -> Vec<String> {
        self.queries.lock().unwrap().iter().filter(|q| !q.contains("summary=")).cloned().collect()
    }

    pub fn summary_requests(&self) -> usize {
        self.queries.lock().unwrap().iter().filter(|q| q.contains("summary=1")).count()
    }
}
