//! The tests' upstream: a mesh signing as `mesh-test.datumesh.com` (a key of its own, not the
//! live mesh's), served on 127.0.0.1 by the datumesh-http library over a SQLite store of its
//! own, holding the synthetic ECB-shaped seed (`tests/data/ecb.jsonl`, written by the test
//! mesh's `seed/ecb.py`) and a few synthetic verses (`tests/data/scripture.jsonl`). Resolvers
//! reach it through `peers`, so names and receipts are the live mesh's shapes. Every request
//! it answers is counted.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use datumesh_core::json::{self, Value};
use datumesh_core::SigningKey;
use datumesh_http::{Config as MeshConfig, Mesh};
use datumesh_local::{Config, Resolver};
use datumesh_store::{SqliteStore, Store};

pub const HOST: &str = "mesh-test.datumesh.com";
pub const KEY_FROM: &str = "2026-01-01T00:00:00.000000Z";
pub const END: &str = "9999-12-31T23:59:59.999999Z";

pub const USD: &str = r#"ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#;
pub const USD_FULL: &str = r#"mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#;
pub const GBP: &str = r#"ecb/fx_rate(base: "EUR", quote: "GBP", date: 2026-10-29)"#;
pub const CHRISTMAS: &str = r#"ecb/holiday(calendar: "TARGET", date: 2026-12-25)"#;
pub const VERSE: &str = r#"scripture/verse_text(corpus: "quran", book: "1", chapter: 1, verse: 2, edition: "uthmani")"#;
pub const FX_SLICE: &str = "mesh-test.datumesh.com/ecb/fx_rate(base: *, quote: *, date: *)";

/// A directory removed when dropped.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(what: &str) -> TempDir {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "datumesh-local-test-{what}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

pub fn upstream_key() -> SigningKey {
    SigningKey::from_seed(&[42u8; 32])
}

/// The upstream mesh, its store and the count of requests it answered.
pub struct Upstream {
    pub origin: String,
    pub mesh: Mesh,
    pub store: Arc<dyn Store + Send + Sync>,
    pub requests: Arc<AtomicU64>,
    _rt: tokio::runtime::Runtime,
    _dir: TempDir,
}

impl Upstream {
    /// Requests answered so far.
    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::SeqCst)
    }

    /// Publishes JSON lines (the `publish-stored` format) as the upstream, and tells the mesh.
    pub fn publish_lines(&self, lines: &[Value]) {
        let dir = TempDir::new("publish");
        let path = dir.path().join("more.jsonl");
        let text: String = lines.iter().map(|l| String::from_utf8(l.to_jcs()).unwrap() + "\n").collect();
        std::fs::write(&path, text).unwrap();
        datumesh_http::publish::publish_stored(&*self.store, &upstream_key(), HOST, &path).unwrap();
        self.mesh.clear_memo();
    }
}

/// Starts the upstream; `slice_page` is its default slice page size.
pub fn upstream_with(slice_page: usize) -> Upstream {
    let dir = TempDir::new("upstream");
    let s = SqliteStore::open(dir.path().join("upstream.sqlite")).unwrap();
    datumesh_store::migrate::sqlite::apply(&s).unwrap();
    let doc = datumesh_http::keydoc::with_key(HOST, None, &upstream_key(), KEY_FROM).unwrap();
    s.put_namespace(HOST, &doc, KEY_FROM).unwrap();
    datumesh_http::publish::publish_stored(&s, &upstream_key(), HOST, &data("ecb.jsonl")).unwrap();
    datumesh_http::publish::publish_stored(&s, &upstream_key(), HOST, &data("scripture.jsonl")).unwrap();
    let store: Arc<dyn Store + Send + Sync> = Arc::new(s);
    let mut config = MeshConfig::new(HOST);
    config.rate.enabled = false;
    config.cross_mesh = false;
    config.slice_page = slice_page;
    let mesh = Mesh::builder(config, store.clone()).signing_key(upstream_key()).licence("CC-BY-4.0").build().unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicU64::new(0));
    let counted = requests.clone();
    let router = mesh.router().layer(axum::middleware::from_fn(move |req: axum::extract::Request, next: axum::middleware::Next| {
        counted.fetch_add(1, Ordering::SeqCst);
        next.run(req)
    }));
    rt.spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    Upstream { origin: format!("http://{addr}"), mesh, store, requests, _rt: rt, _dir: dir }
}

pub fn upstream() -> Upstream {
    upstream_with(1000)
}

/// A resolver's configuration in its own directory, reaching `upstream` for the test mesh.
pub fn config(dir: &TempDir, upstream: &Upstream) -> Config {
    let mut c = Config::in_dir(dir.path()).unwrap();
    c.peers.insert(HOST.to_string(), upstream.origin.clone());
    c
}

pub fn resolver(dir: &TempDir, upstream: &Upstream) -> Resolver {
    Resolver::open(config(dir, upstream)).unwrap()
}

pub fn envelope(body: &[u8]) -> Value {
    json::parse(body).unwrap()
}

pub fn receipt(body: &[u8]) -> Value {
    envelope(body).get("receipt").cloned().expect("a receipt")
}

pub fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}
