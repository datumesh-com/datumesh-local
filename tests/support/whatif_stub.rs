//! A stub mesh for Build 2 Parts 3 and 4 (CONTRACT-build2-part3-calculus E6, CONTRACT-build2-part4-publish
//! P1): the real test mesh (`common::Upstream`: cells, receipts and descriptors signed by the test
//! key) with three things added that datumesh-http 0.4.0 does not have yet:
//!
//! * a mesh document (`/.well-known/datumesh-mesh.json`, the upstream's own plus endpoints) that
//!   lists `whatif` (spec-0.6.0, 9.6) and, as the unknown member `x_bundles`, the bundles endpoint;
//! * `POST /whatif`, answering a canned body and status (a scenario envelope, or an error);
//! * `POST /bundles` (and `POST /x/bundles-in`, where the mesh document's `x_bundles` points when
//!   it is asked to), answering a canned body and status.
//!
//! Every POST is kept: its path, its content type and its body.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use datumesh_core::json::{self, Value};
use tower::ServiceExt;

use crate::common::{Upstream, HOST};

/// How the stub's mesh document names its endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocMode {
    /// The URLs of a rooted mesh: `https://mesh-test.datumesh.com/whatif` (reached through the
    /// resolver's `peers`, as every other endpoint of the test mesh is). `x_bundles` is
    /// `https://mesh-test.datumesh.com/x/bundles-in`.
    Peer,
    /// The URLs on the stub's own loopback origin (`http://127.0.0.1:<port>/...`): what a mesh
    /// document looks like to a command given `--to http://127.0.0.1:<port>`. `x_bundles` is
    /// `http://127.0.0.1:<port>/x/bundles-in`.
    Direct,
    /// No mesh document at all (404): the bundles endpoint is then `<url>/bundles`.
    None,
}

/// One POST the stub received.
#[derive(Clone, Debug)]
pub struct Post {
    pub path: String,
    pub content_type: String,
    pub body: String,
}

impl Post {
    pub fn json(&self) -> Value {
        json::parse(self.body.as_bytes()).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }
}

pub struct WhatifStub {
    posts: Mutex<Vec<Post>>,
    whatif: Mutex<(u16, String)>,
    bundles: Mutex<(u16, String)>,
    /// When set, the mesh document leaves `whatif` out.
    pub no_whatif_endpoint: AtomicBool,
    _rt: tokio::runtime::Runtime,
    pub origin: String,
}

const NOT_SET: &str = r#"{"error":"stub has no canned answer"}"#;

impl WhatifStub {
    pub fn start(up: &Upstream, mode: DocMode) -> Arc<WhatifStub> {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let addr = listener.local_addr().unwrap();
        let origin = format!("http://{addr}");
        let stub = Arc::new(WhatifStub {
            posts: Mutex::new(Vec::new()),
            whatif: Mutex::new((500, NOT_SET.to_string())),
            bundles: Mutex::new((500, NOT_SET.to_string())),
            no_whatif_endpoint: AtomicBool::new(false),
            _rt: rt,
            origin: origin.clone(),
        });
        // The upstream's own mesh document, read once through its router.
        let router = up.mesh.router();
        let doc_bytes = stub._rt.block_on(async {
            let req = axum::http::Request::builder()
                .uri("/.well-known/datumesh-mesh.json")
                .body(axum::body::Body::empty())
                .unwrap();
            let res = router.clone().oneshot(req).await.unwrap();
            assert_eq!(res.status(), 200, "the test mesh serves its mesh document");
            axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap().to_vec()
        });
        let base = match mode {
            DocMode::Direct => origin.clone(),
            _ => format!("https://{HOST}"),
        };
        let s = stub.clone();
        let doc_handler = move || {
            let s = s.clone();
            let doc_bytes = doc_bytes.clone();
            let base = base.clone();
            async move {
                if mode == DocMode::None {
                    return axum::http::Response::builder().status(404).body(axum::body::Body::empty()).unwrap();
                }
                let Value::Object(mut m) = json::parse(&doc_bytes).unwrap() else { panic!("a mesh document") };
                for (k, v) in m.iter_mut() {
                    if k == "endpoints" {
                        if let Value::Object(e) = v {
                            if !s.no_whatif_endpoint.load(Ordering::SeqCst) {
                                e.push(("whatif".into(), Value::String(format!("{base}/whatif"))));
                            }
                            e.push(("x_bundles".into(), Value::String(format!("{base}/x/bundles-in"))));
                        }
                    }
                }
                axum::http::Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(Value::Object(m).to_jcs()))
                    .unwrap()
            }
        };
        let post_handler = |which: bool, s: Arc<WhatifStub>| {
            move |req: axum::extract::Request| {
                let s = s.clone();
                async move {
                    let path = req.uri().path().to_string();
                    let content_type =
                        req.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
                    let body = axum::body::to_bytes(req.into_body(), 64 << 20).await.unwrap();
                    s.posts.lock().unwrap().push(Post {
                        path,
                        content_type,
                        body: String::from_utf8_lossy(&body).into_owned(),
                    });
                    let (status, answer) =
                        if which { s.whatif.lock().unwrap().clone() } else { s.bundles.lock().unwrap().clone() };
                    axum::http::Response::builder()
                        .status(status)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(answer))
                        .unwrap()
                }
            }
        };
        let app = axum::Router::new()
            .route("/.well-known/datumesh-mesh.json", axum::routing::get(doc_handler))
            .route("/whatif", axum::routing::post(post_handler(true, stub.clone())))
            .route("/bundles", axum::routing::post(post_handler(false, stub.clone())))
            .route("/x/bundles-in", axum::routing::post(post_handler(false, stub.clone())))
            .fallback_service(router);
        stub._rt.spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        stub
    }

    /// The answer of `POST /whatif`.
    pub fn answer_whatif(&self, status: u16, body: &str) {
        *self.whatif.lock().unwrap() = (status, body.to_string());
    }

    /// The answer of the bundles endpoint.
    pub fn answer_bundles(&self, status: u16, body: &str) {
        *self.bundles.lock().unwrap() = (status, body.to_string());
    }

    /// Every POST received, in order.
    pub fn posts(&self) -> Vec<Post> {
        self.posts.lock().unwrap().clone()
    }

    /// The POSTs to `path`.
    pub fn posts_to(&self, path: &str) -> Vec<Post> {
        self.posts().into_iter().filter(|p| p.path == path).collect()
    }
}
