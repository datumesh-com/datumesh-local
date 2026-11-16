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
use std::sync::{Arc, Mutex};

use datumesh_core::json::{self, Value};
use datumesh_core::SigningKey;
use datumesh_engine::Clock;
use datumesh_http::publish::{publish_stored_with, PublishOptions};
use datumesh_http::{Config as MeshConfig, Mesh};
use datumesh_local::{Config, Resolver};
use datumesh_store::{SqliteStore, Store};

pub const HOST: &str = "mesh-test.datumesh.com";
pub const KEY_FROM: &str = "2026-01-01T00:00:00.000000Z";
pub const END: &str = "9999-12-31T23:59:59.999999Z";
pub const RESTRICTED: &str = "https://mesh-test.datumesh.com/licences/restricted-test";
pub const UXLC_LICENCE: &str = "https://tanach.us/License.html";

pub const USD: &str = r#"ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#;
pub const USD_FULL: &str = r#"mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#;
pub const GBP: &str = r#"ecb/fx_rate(base: "EUR", quote: "GBP", date: 2026-10-29)"#;
pub const CHRISTMAS: &str = r#"ecb/holiday(calendar: "TARGET", date: 2026-12-25)"#;
pub const VERSE: &str = r#"scripture/verse_text(corpus: "quran", book: "1", chapter: 1, verse: 2, edition: "uthmani")"#;
pub const FX_SLICE: &str = "mesh-test.datumesh.com/ecb/fx_rate(base: *, quote: *, date: *)";

/// A clock that says what it was last told (CONTRACT-1.8b L4: clock-controlled resolvers).
pub struct ManualClock(Mutex<String>);

impl ManualClock {
    pub fn new(now: &str) -> Arc<ManualClock> {
        Arc::new(ManualClock(Mutex::new(now.to_string())))
    }
    pub fn set(&self, now: &str) {
        *self.0.lock().unwrap() = now.to_string();
    }
}

impl Clock for ManualClock {
    fn now(&self) -> String {
        self.0.lock().unwrap().clone()
    }
}

/// The upstream's publishing options: the clock is `KEY_FROM`, so the seeds' historical
/// starts are not earlier than the mesh clock (rule A, CONTRACT-1.8b shared rules).
pub fn seed_options() -> PublishOptions {
    PublishOptions { clock: Arc::new(ManualClock(Mutex::new(KEY_FROM.to_string()))), backfill: None }
}

/// Publishes the file at `path` into `store` as the test mesh, with a clock of `KEY_FROM`.
pub fn publish_seed(store: &(dyn Store + Send + Sync), path: &Path) {
    publish_stored_with(store, &upstream_key(), HOST, &[HOST.to_string()], path, &seed_options()).unwrap();
}

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
        publish_seed(&*self.store, &path);
        self.mesh.clear_memo();
    }
}

/// Starts the upstream; `slice_page` is its default slice page size.
pub fn upstream_with(slice_page: usize) -> Upstream {
    build_upstream(slice_page, false)
}

/// An upstream whose `/cell` answers carry a receipt `value` changed after signing (L3 test 2).
pub fn upstream_tampering() -> Upstream {
    build_upstream(1000, true)
}

fn tampered(body: &[u8]) -> Vec<u8> {
    match json::parse(body) {
        Ok(Value::Object(mut m)) => {
            for (k, v) in m.iter_mut() {
                if k == "receipt" {
                    if let Value::Object(r) = v {
                        for (k2, v2) in r.iter_mut() {
                            if k2 == "value" {
                                *v2 = Value::String("9.9".into());
                            }
                        }
                    }
                }
            }
            Value::Object(m).to_jcs()
        }
        _ => body.to_vec(),
    }
}

async fn tamper_cell(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let is_cell = req.uri().path().starts_with("/cell/");
    let res = next.run(req).await;
    if !is_cell || res.status() != axum::http::StatusCode::OK {
        return res;
    }
    let (mut parts, body) = res.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    axum::response::Response::from_parts(parts, axum::body::Body::from(tampered(&bytes)))
}

fn build_upstream(slice_page: usize, tamper: bool) -> Upstream {
    build_upstream_with(slice_page, tamper, None)
}

/// A stub `/find` for local 0.3.2 (CONTRACT-1.7a L4): datumesh-http 0.3.1's answer shape (H3)
/// for the query "quran" (the class `VERSE_CLASS` with a `text(edition)` method), an empty
/// `results` for any other query, or a 500 when `fail` is set. It counts its requests and
/// keeps their raw query strings. Every other path is the real test mesh's.
pub struct FindStub {
    pub finds: AtomicU64,
    pub queries: Mutex<Vec<String>>,
    pub fail: std::sync::atomic::AtomicBool,
}

impl FindStub {
    pub fn finds(&self) -> u64 {
        self.finds.load(Ordering::SeqCst)
    }
}

const STUB_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn stub_answer(query: &str) -> String {
    if !query.to_lowercase().contains("quran") {
        return format!(r#"{{"query":{},"results":[]}}"#, String::from_utf8(Value::String(query.into()).to_jcs()).unwrap());
    }
    format!(
        r#"{{"query":"quran","results":[{{"kind":"class","name":"{VERSE_CLASS}","description":"A verse of the Quran, the Tanakh or the New Testament, by corpus, book, chapter and verse","class_hash":"{STUB_HASH}","descriptor":{{"keys":{{"book":{{"key_normalisation":"none","type":"string"}},"chapter":{{"type":"number","unit":"1"}},"corpus":{{"key_normalisation":"none","type":"string"}},"verse":{{"type":"number","unit":"1"}}}},"methods":{{"text":"{STUB_HASH}"}},"order":["corpus","book","chapter","verse"],"steward":{{"namespace":"{HOST}","process":"https://{HOST}/classes/change"}}}},"methods":{{"text":{{"fn":"{VERSE_CLASS}/text","fn_hash":"{STUB_HASH}","args":{{"edition":{{"type":"string","valid":false}}}},"returns":"blob","description":"The text of the verse in an edition"}}}},"since":"2026-02-02T00:00:00.000000Z"}}]}}"#
    )
}

/// The upstream with the stub `/find`.
pub fn upstream_finding() -> (Upstream, Arc<FindStub>) {
    let stub = Arc::new(FindStub {
        finds: AtomicU64::new(0),
        queries: Mutex::new(Vec::new()),
        fail: std::sync::atomic::AtomicBool::new(false),
    });
    (build_upstream_with(1000, false, Some(stub.clone())), stub)
}

fn build_upstream_with(slice_page: usize, tamper: bool, find: Option<Arc<FindStub>>) -> Upstream {
    let dir = TempDir::new("upstream");
    // UPSTREAM_DATABASE_URL: the upstream on PostgreSQL, as the test mesh runs (an empty
    // database of its own, made by the caller, for one test: tools/refresh_pg.sh); else on
    // SQLite.
    let store: Arc<dyn Store + Send + Sync> = match std::env::var("UPSTREAM_DATABASE_URL") {
        Ok(url) => {
            let s = datumesh_store::PgStore::connect(&url).unwrap();
            datumesh_store::migrate::apply(&s).unwrap();
            Arc::new(s)
        }
        Err(_) => {
            let s = SqliteStore::open(dir.path().join("upstream.sqlite")).unwrap();
            datumesh_store::migrate::sqlite::apply(&s).unwrap();
            Arc::new(s)
        }
    };
    let doc = datumesh_http::keydoc::with_key(HOST, None, &upstream_key(), KEY_FROM).unwrap();
    // The test mesh's declarations (spec-0.4.3 9.2), as http 0.2.1's deploy/licences.sh makes
    // them: fx_cross over the restricted test licence, strip_niqqud over the UXLC's.
    let (doc, _) = datumesh_http::keydoc::with_declaration(
        HOST,
        &doc,
        "mesh-test.datumesh.com/ecb/fx_cross",
        RESTRICTED,
        "2026-09-01T00:00:00.000000Z",
        None,
    )
    .unwrap();
    let (doc, _) = datumesh_http::keydoc::with_declaration(
        HOST,
        &doc,
        "mesh-test.datumesh.com/scripture/strip_niqqud",
        UXLC_LICENCE,
        "2026-11-02T00:00:00.000000Z",
        None,
    )
    .unwrap();
    store.put_namespace(HOST, &doc, KEY_FROM).unwrap();
    publish_seed(&*store, &data("ecb.jsonl"));
    publish_seed(&*store, &data("scripture.jsonl"));
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
    let mut router = mesh.router();
    if let Some(stub) = find {
        let handler = move |req: axum::extract::Request| {
            let stub = stub.clone();
            async move {
                stub.finds.fetch_add(1, Ordering::SeqCst);
                let raw = req.uri().query().unwrap_or("").to_string();
                stub.queries.lock().unwrap().push(raw.clone());
                let q = raw.split('&').find_map(|p| p.strip_prefix("q=")).unwrap_or("").replace('+', " ");
                let (status, body) = if stub.fail.load(Ordering::SeqCst) {
                    (500, r#"{"error":"stub failure"}"#.to_string())
                } else {
                    (200, stub_answer(&q))
                };
                axum::http::Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap()
            }
        };
        router = axum::Router::new().route("/find", axum::routing::get(handler)).fallback_service(router);
    }
    if tamper {
        router = router.layer(axum::middleware::from_fn(tamper_cell));
    }
    let router =
        router.layer(axum::middleware::from_fn(move |req: axum::extract::Request, next: axum::middleware::Next| {
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

/// A resolver on `clock` (every "now" it uses), reaching `upstream` for the test mesh.
pub fn resolver_at(dir: &TempDir, upstream: &Upstream, clock: &Arc<ManualClock>) -> Resolver {
    Resolver::open_with_clock(config(dir, upstream), clock.clone()).unwrap()
}

/// A second machine: its own directory, config host `laptop.invalid`, on `clock`.
pub const LAPTOP: &str = "laptop.invalid";

pub fn laptop_at(dir: &TempDir, upstream: &Upstream, clock: &Arc<ManualClock>) -> Resolver {
    let mut c = config(dir, upstream);
    c.host = LAPTOP.to_string();
    Resolver::open_with_clock(c, clock.clone()).unwrap()
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

// ---------------------------------------------------------------------------------------------
// The classes world (spec-0.5.0, section 16), modelled on the live check of the test mesh:
//
// * `scripture/verse`: keys `corpus`, `book`, `chapter`, `verse`, all `none`; methods `text`
//   (stored, a blob, with an `edition` argument) and `consonantal` (a formula over `text`);
// * `ecb/fx_pair`: keys `base`, `quote`, `none`; methods `rate` (stored) and `cross` (a formula
//   whose `date` has `"default": "asof"`);
// * `ecb/fx_named`: like `fx_pair`, with a `lookup` method `named` (label to key, 16.7).
//
// `tests/data/classes-functions.jsonl` holds the method functions (bound at `CLASSES_BIND`), the
// classes are bound at `CLASSES_AT` (their descriptors name the functions' hashes, so the class
// lines are written here), `tests/data/classes-cells.jsonl` the cells (at `TV`).

pub const CLASSES_BIND: &str = "2026-02-01T00:00:00.000000Z";
pub const CLASSES_AT: &str = "2026-02-02T00:00:00.000000Z";
/// The tx of every stored cell of the classes world.
pub const TV: &str = "2026-03-02T16:00:00.000000Z";
/// An asof after the cells.
pub const ASOF: &str = "2026-03-02T18:00:00.000000Z";
/// The date of the world's FX cells.
pub const DAY: &str = "2026-03-02";
pub const VERSE_CLASS: &str = "mesh-test.datumesh.com/scripture/verse";
pub const PAIR_CLASS: &str = "mesh-test.datumesh.com/ecb/fx_pair";
pub const NAMED_CLASS: &str = "mesh-test.datumesh.com/ecb/fx_named";

fn class_line(name: &str, keys: &str, order: &[&str], methods: &[(&str, String)], lookup: Option<&str>) -> Value {
    let ms: Vec<String> = methods.iter().map(|(m, h)| format!("\"{m}\":\"{h}\"")).collect();
    let os: Vec<String> = order.iter().map(|o| format!("\"{o}\"")).collect();
    let lk = lookup.map(|l| format!("\"lookup\":\"{l}\",")).unwrap_or_default();
    let descriptor = format!(
        r#"{{"keys":{keys},"methods":{{{}}},{lk}"order":[{}],"steward":{{"namespace":"{HOST}","process":"https://{HOST}/classes/change"}}}}"#,
        ms.join(","),
        os.join(",")
    );
    Value::Object(vec![
        ("bound".into(), Value::String(CLASSES_AT.into())),
        ("class".into(), Value::String(name.into())),
        ("descriptor".into(), json::parse(descriptor.as_bytes()).unwrap()),
    ])
}

/// The upstream with the classes world published: the method functions, the classes, the cells.
pub fn upstream_classes() -> Upstream {
    let up = upstream();
    let key = upstream_key();
    publish_seed(&*up.store, &data("classes-functions.jsonl"));
    let hash = |name: &str| -> String {
        up.store.resolve_name(name, END).unwrap().unwrap_or_else(|| panic!("{name} unbound"))
    };
    let none_str = r#"{"key_normalisation":"none","type":"string"}"#;
    let pair_keys = format!(r#"{{"base":{none_str},"quote":{none_str}}}"#);
    let verse_keys = format!(
        r#"{{"book":{none_str},"chapter":{{"type":"number","unit":"1"}},"corpus":{none_str},"verse":{{"type":"number","unit":"1"}}}}"#
    );
    let lines = [
        class_line(
            VERSE_CLASS,
            &verse_keys,
            &["corpus", "book", "chapter", "verse"],
            &[
                ("consonantal", hash(&format!("{VERSE_CLASS}/consonantal"))),
                ("text", hash(&format!("{VERSE_CLASS}/text"))),
            ],
            None,
        ),
        class_line(
            PAIR_CLASS,
            &pair_keys,
            &["base", "quote"],
            &[("cross", hash(&format!("{PAIR_CLASS}/cross"))), ("rate", hash(&format!("{PAIR_CLASS}/rate")))],
            None,
        ),
        class_line(
            NAMED_CLASS,
            &pair_keys,
            &["base", "quote"],
            &[("named", hash(&format!("{NAMED_CLASS}/named"))), ("rate", hash(&format!("{NAMED_CLASS}/rate")))],
            Some("named"),
        ),
    ];
    let dir = TempDir::new("classes");
    let path = dir.path().join("classes.jsonl");
    let text: String = lines.iter().map(|l| String::from_utf8(l.to_jcs()).unwrap() + "\n").collect();
    std::fs::write(&path, text).unwrap();
    let p = publish_stored_with(&*up.store, &key, HOST, &[HOST.to_string()], &path, &seed_options()).unwrap();
    assert_eq!(p.classes, 3);
    publish_seed(&*up.store, &data("classes-cells.jsonl"));
    up.mesh.clear_memo();
    up
}

/// The class descriptors the upstream's store holds, by name, and the descriptors of their
/// methods' functions: what a client of the mesh holds for core's describe check.
pub fn held_classes(
    up: &Upstream,
) -> (std::collections::BTreeMap<String, datumesh_core::ClassDescriptor>, Vec<datumesh_core::FunctionDescriptor>) {
    let mut held = std::collections::BTreeMap::new();
    let mut functions = Vec::new();
    for name in [VERSE_CLASS, PAIR_CLASS, NAMED_CLASS] {
        let b = up.store.resolve_class_binding(name, END).unwrap().expect("a bound class");
        let bytes = up.store.class(&b.class_hash).unwrap().expect("a stored class");
        let c = datumesh_core::ClassDescriptor::from_json(&bytes).unwrap();
        for h in c.methods.values() {
            functions.push(datumesh_core::FunctionDescriptor::from_json(&up.store.descriptor(h).unwrap()).unwrap());
        }
        held.insert(name.to_string(), c);
    }
    (held, functions)
}

/// Before the classes' binding and `ASOF`: an alias entry does not exist before its tx (L6).
pub const ALIASES_TX: &str = "2026-01-15T00:00:00.000000Z";

/// A resolver as `resolver`, with the alias map's file replaced by one that also has the bare
/// name `xcross` (the pair's `cross` method function) and a `tx` of `ALIASES_TX`.
pub fn resolver_with_aliases(dir: &TempDir, upstream: &Upstream) -> Resolver {
    let c = config(dir, upstream);
    std::fs::write(
        &c.aliases_file,
        format!(
            r#"{{"ecb/": "{HOST}/ecb/", "local/": "local.invalid/", "scripture/": "{HOST}/scripture/", "tx": "{ALIASES_TX}", "xcross": "{PAIR_CLASS}/cross"}}"#
        ),
    )
    .unwrap();
    Resolver::open(c).unwrap()
}

/// The envelope of `body` without its `resolved` member (the one thing a method call's answer has
/// that the plain read's has not).
pub fn without_resolved(body: &[u8]) -> Value {
    match envelope(body) {
        Value::Object(m) => Value::Object(m.into_iter().filter(|(k, _)| k != "resolved").collect()),
        other => other,
    }
}

/// Today's date in UTC, from the system clock.
pub fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let z = secs.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The id of the cell `<ns>/<fn_name>` (hash `fn_hash`) with `args` (the JSON object text of its
/// arguments, in canonical form).
pub fn cell_id_of(ns: &str, fn_name: &str, fn_hash: &str, args: &str) -> String {
    let text = format!(r#"{{"args":{args},"fn":"{fn_name}","fn_hash":"{fn_hash}","ns":"{ns}"}}"#);
    datumesh_core::Cell::from_json(text.as_bytes()).unwrap().id()
}

/// The key document the upstream holds for itself, as a resolver fetches it.
pub fn upstream_keys(up: &Upstream) -> datumesh_core::Keys {
    let doc = up.store.namespace(HOST).unwrap().unwrap().keydoc;
    let mut keys = datumesh_core::Keys::new();
    keys.insert_response(HOST, 200, Some(&doc));
    keys
}

/// The full JSON of a receipt (its payload and its `sig`), as the mesh serves it.
pub fn receipt_bytes(r: &datumesh_core::Receipt) -> Vec<u8> {
    let Value::Object(mut m) = json::parse(r.payload()).unwrap() else { panic!("a receipt payload is an object") };
    m.push(("sig".to_string(), Value::String(r.sig().to_string())));
    m.sort_by(|a, b| a.0.cmp(&b.0));
    Value::Object(m).to_jcs()
}

/// The file's permission bits (unix; 0o600 elsewhere, where there are none to check).
pub fn mode_of(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0o600
    }
}

/// The 32 bytes of a lowercase hex seed.
pub fn seed_of(hex: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
    }
    out
}

/// The `keys` of a key document.
pub fn key_entries_of(doc: &[u8]) -> Vec<Value> {
    match json::parse(doc).unwrap().get("keys") {
        Some(Value::Array(k)) => k.clone(),
        other => panic!("no keys in the key document: {other:?}"),
    }
}

/// A file with `lines` as JSON lines, for `Resolver::publish`.
pub fn jsonl(dir: &TempDir, name: &str, lines: &[&str]) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}
