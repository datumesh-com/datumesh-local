//! describe (spec-0.5.0, 16.8, 9.6; `CONTRACT-local-0.5.0.md` item 2).
//!
//! `Resolver::describe(id)` and `datumesh describe <id>`, `<id>` a class name, a cell id, an object
//! id, a short id (the first 12 hex digits of an object id) or an object's key encoding. It asks the
//! forward mesh's describe endpoint, or answers from local classes if the local store holds the class,
//! and checks the answer with core's `check_describe_with` against the descriptors held. 404 and
//! `E_SHORT_ID_AMBIGUOUS` are exit status 3 at the command line. The localhost endpoint serves
//! `/describe/{id}` too: from local classes, else forwarded.

mod common;

use std::io::{Read, Write};
use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_core::{check_describe_with, DescribeKind};
use datumesh_local::serve::Server;
use datumesh_local::{Config, Source};

fn encoding(class: &str, keys: &str) -> String {
    format!("{class}({keys})")
}

fn field(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no {k} in {}", String::from_utf8_lossy(&v.to_jcs())))
        .into()
}

const RATE_SIG: &str = r#"{"args": {"base": {"type": "string", "valid": false}, "date": {"type": "date", "valid": true}, "quote": {"type": "string", "valid": false}}, "order": ["base", "quote", "date"], "returns": {"type": "number", "unit": "1"}}"#;

/// A resolver with a local class `local/pair` (method `rate`), and what core's check holds for it.
fn with_local_class(
    r: &datumesh_local::Resolver,
) -> (String, datumesh_core::ClassDescriptor, datumesh_core::FunctionDescriptor) {
    let rate = r.define_stored("local/pair/rate", &json::parse(RATE_SIG.as_bytes()).unwrap()).unwrap();
    let descriptor = format!(
        r#"{{"keys": {{"base": {{"key_normalisation": "none", "type": "string"}}, "quote": {{"key_normalisation": "none", "type": "string"}}}}, "methods": {{"rate": "{}"}}, "order": ["base", "quote"], "steward": {{"namespace": "local.invalid", "process": "https://local.invalid/classes/change"}}}}"#,
        rate.fn_hash
    );
    let d = r.define_class("local/pair", descriptor.as_bytes()).unwrap();
    let b = r.store().resolve_class_binding("local.invalid/pair", END).unwrap().unwrap();
    assert_eq!(b.class_hash, d.fn_hash);
    let class = datumesh_core::ClassDescriptor::from_json(&r.store().class(&b.class_hash).unwrap().unwrap()).unwrap();
    let f = datumesh_core::FunctionDescriptor::from_json(&r.store().descriptor(&rate.fn_hash).unwrap()).unwrap();
    (b.class_hash, class, f)
}

/// describe of a class from the forward mesh: 200, forwarded, and core's six-step check passes
/// against the class descriptors and the method descriptors the client holds (with and without the
/// functions).
#[test]
fn describe_a_class() {
    let up = upstream_classes();
    let dir = TempDir::new("describe-class");
    let r = resolver(&dir, &up);
    let (held, functions) = held_classes(&up);
    for name in [VERSE_CLASS, PAIR_CLASS, NAMED_CLASS] {
        let reply = r.describe(name).unwrap();
        assert_eq!(reply.status, 200, "{name}: {}", String::from_utf8_lossy(&reply.body));
        assert_eq!(reply.source, Source::Forwarded { mesh: HOST.to_string() }, "{name}");
        let d = check_describe_with(&reply.body, &held, &functions)
            .unwrap_or_else(|e| panic!("{name}: {e}: {}", String::from_utf8_lossy(&reply.body)));
        assert_eq!((d.kind, d.class.as_str()), (DescribeKind::Class, name));
        check_describe_with(&reply.body, &held, &[]).unwrap();
        let doc = envelope(&reply.body);
        assert_eq!(doc.get("descriptor").map(Value::to_jcs), Some(held[name].canonical()), "{name}");
    }
    // A short name goes through the alias map, as in a request.
    let by_alias = r.describe("scripture/verse").unwrap();
    assert_eq!(by_alias.body, r.describe(VERSE_CLASS).unwrap().body);
}

/// describe of an object by its key encoding, its object id and its short id: one answer, checked
/// against the held class; a cell id is an id the mesh knows too.
#[test]
fn describe_an_object_by_encoding_id_and_short_id() {
    let up = upstream_classes();
    let dir = TempDir::new("describe-object");
    let r = resolver(&dir, &up);
    let (held, _) = held_classes(&up);
    let enc = encoding(PAIR_CLASS, r#"base:"EUR",quote:"USD""#);
    let by_key = r.describe(&enc).unwrap();
    assert_eq!(by_key.status, 200, "{}", String::from_utf8_lossy(&by_key.body));
    let d = check_describe_with(&by_key.body, &held, &[]).unwrap();
    assert_eq!((d.kind, d.class.as_str()), (DescribeKind::Object, PAIR_CLASS));
    let doc = envelope(&by_key.body);
    let (id, short) = (field(&doc, "object_id"), field(&doc, "short_id"));
    assert_eq!((id.len(), short.len()), (64, 12));
    assert_eq!(&id[..12], short);
    assert_eq!(field(&doc, "call"), format!(r#"{PAIR_CLASS}(base: "EUR", quote: "USD")"#));
    // The same object by its id and by its short id, as an id the mesh holds a cell for.
    r.get(&format!("{PAIR_CLASS}(EUR, USD).rate({DAY})")).unwrap();
    for what in [&id, &short] {
        let reply = r.describe(what).unwrap();
        assert_eq!(reply.status, 200, "by {what}: {}", String::from_utf8_lossy(&reply.body));
        assert_eq!(reply.body, by_key.body, "by {what}");
        check_describe_with(&reply.body, &held, &[]).unwrap();
    }
    // Typed keys: a verse, by its encoding.
    let verse = r.describe(&encoding(VERSE_CLASS, r#"book:"1",chapter:1,corpus:"quran",verse:2"#)).unwrap();
    assert_eq!(verse.status, 200, "{}", String::from_utf8_lossy(&verse.body));
    check_describe_with(&verse.body, &held, &[]).unwrap();
    // A cell id (of the cell the call above read).
    let cell = r.get(&call_plain_rate()).unwrap();
    let cell_id = datumesh_core::Receipt::from_value(&receipt(&cell.body)).unwrap().cell().id();
    let reply = r.describe(&cell_id).unwrap();
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.body));
    check_describe_with(&reply.body, &held, &[]).unwrap();
}

fn call_plain_rate() -> String {
    format!(r#"{PAIR_CLASS}/rate(base: "EUR", quote: "USD", date: {DAY})"#)
}

/// describe of an id the mesh does not know is its 404; a malformed encoding is `bad_request`
/// `E_KEY_ENCODING`; both pass through unchanged (the body is the mesh's).
#[test]
fn describe_refusals_pass_through() {
    let up = upstream_classes();
    let dir = TempDir::new("describe-refusals");
    let r = resolver(&dir, &up);
    for id in [format!("{HOST}/ecb/nothing"), "f".repeat(64), "f".repeat(12), "nonsense".to_string()] {
        let reply = r.describe(&id).unwrap();
        assert_eq!(reply.status, 404, "{id}: {}", String::from_utf8_lossy(&reply.body));
    }
    let reply = r.describe(&encoding(PAIR_CLASS, "base:EUR")).unwrap();
    assert_eq!(reply.status, 400, "{}", String::from_utf8_lossy(&reply.body));
    let env = envelope(&reply.body);
    assert_eq!((s(&env, "outcome"), s(&env, "error")), ("bad_request".into(), "E_KEY_ENCODING".into()));
}

/// A class the local store holds is answered from it, with no network (offline works), the answer
/// checked like any other; its objects too. Other ids still go to the forward mesh.
#[test]
fn describe_answers_from_local_classes() {
    let up = upstream_classes();
    let dir = TempDir::new("describe-local");
    let r = resolver(&dir, &up);
    let (hash, class, f) = with_local_class(&r);
    r.set_offline(true);
    let held = std::collections::BTreeMap::from([("local.invalid/pair".to_string(), class)]);
    let reply = r.describe("local/pair").unwrap();
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.body));
    assert_eq!(reply.source, Source::Local);
    let d = check_describe_with(&reply.body, &held, std::slice::from_ref(&f)).unwrap();
    assert_eq!(
        (d.kind, d.class.as_str(), d.class_hash.as_str()),
        (DescribeKind::Class, "local.invalid/pair", hash.as_str())
    );
    let obj = r.describe(r#"local.invalid/pair(base:"EUR",quote:"USD")"#).unwrap();
    assert_eq!(obj.status, 200, "{}", String::from_utf8_lossy(&obj.body));
    assert_eq!(obj.source, Source::Local);
    let d = check_describe_with(&obj.body, &held, &[]).unwrap();
    assert_eq!(d.kind, DescribeKind::Object);
    let short = field(&envelope(&obj.body), "short_id");
    assert_eq!(r.describe(&short).unwrap().body, obj.body);
    // A class the local store does not hold, offline: not answered, and not a 404 of the class.
    let other = r.describe(PAIR_CLASS).unwrap();
    assert_ne!(other.status, 200);
}

// ---------------------------------------------------------------------------------------------
// The command

fn home(up: &Upstream) -> TempDir {
    let dir = TempDir::new("describe-cli");
    Config::in_dir(dir.path()).unwrap();
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("\"peers\": {}", &format!("\"peers\": {{\"{HOST}\": \"{}\"}}", up.origin)))
        .unwrap();
    dir
}

fn run(dir: &TempDir, args: &[&str]) -> (i32, String, String) {
    let mut all = vec!["--home".to_string(), dir.path().to_string_lossy().into_owned()];
    all.extend(args.iter().map(|a| a.to_string()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&all, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

/// `datumesh describe <id>` prints the answer (as the endpoint serves it) on stdout; exit 0 for an
/// answer, 3 for a 404 and for any answer that is not a description; usage errors are 2.
#[test]
fn the_describe_command() {
    let up = upstream_classes();
    let dir = home(&up);
    let (held, functions) = held_classes(&up);
    let (code, out, err) = run(&dir, &["describe", VERSE_CLASS]);
    assert_eq!(code, 0, "{err}");
    let d = check_describe_with(out.trim_end().as_bytes(), &held, &functions).unwrap();
    assert_eq!((d.kind, d.class.as_str()), (DescribeKind::Class, VERSE_CLASS));
    // An object by key encoding, then by short id.
    let (code, obj, err) = run(&dir, &["describe", &encoding(PAIR_CLASS, r#"base:"EUR",quote:"USD""#)]);
    assert_eq!(code, 0, "{err}");
    let short = field(&envelope(obj.trim_end().as_bytes()), "short_id");
    let (code, by_short, err) = run(&dir, &["describe", &short]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(by_short, obj);
    // 404: exit 3, the mesh's body on stdout.
    let (code, out, _) = run(&dir, &["describe", &format!("{HOST}/ecb/nothing")]);
    assert_eq!(code, 3);
    assert!(!out.trim().is_empty());
    // A malformed encoding: bad_request E_KEY_ENCODING, exit 3.
    let (code, out, _) = run(&dir, &["describe", &encoding(PAIR_CLASS, "base:EUR")]);
    assert_eq!(code, 3);
    assert!(out.contains("E_KEY_ENCODING"), "{out}");
    // Usage.
    assert_eq!(run(&dir, &["describe"]).0, 2);
    assert_eq!(run(&dir, &["describe", "a", "b"]).0, 2);
}

/// `define <name> --class <descriptor file>` under `local/`, then `describe` of it, offline.
#[test]
fn define_class_and_describe_it_at_the_command_line() {
    let up = upstream_classes();
    let dir = home(&up);
    let (code, out, err) = run(&dir, &["define", "local/pair/rate", "--stored", "--signature", RATE_SIG]);
    assert_eq!(code, 0, "{err}");
    let hash = field(&envelope(out.trim_end().as_bytes()), "fn_hash");
    let file = dir.path().join("pair.json");
    std::fs::write(
        &file,
        format!(
            r#"{{"keys": {{"base": {{"key_normalisation": "none", "type": "string"}}, "quote": {{"key_normalisation": "none", "type": "string"}}}}, "methods": {{"rate": "{hash}"}}, "order": ["base", "quote"], "steward": {{"namespace": "local.invalid", "process": "https://local.invalid/classes/change"}}}}"#
        ),
    )
    .unwrap();
    let (code, out, err) = run(&dir, &["define", "local/pair", "--class", file.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let d = envelope(out.trim_end().as_bytes());
    assert_eq!((field(&d, "name"), d.get("bound")), ("local.invalid/pair".to_string(), Some(&Value::Bool(true))));
    let (code, out, err) = run(&dir, &["--offline", "describe", "local/pair"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(field(&envelope(out.trim_end().as_bytes()), "kind"), "class");
    // Not under local/: a failure, not a class.
    let (code, _, _) = run(&dir, &["define", "example.org/pair", "--class", file.to_str().unwrap()]);
    assert_ne!(code, 0);
}

// ---------------------------------------------------------------------------------------------
// The localhost endpoint

fn http_get(addr: std::net::SocketAddr, path: &str) -> (u16, Vec<u8>) {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(s, "GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n").unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    (head.lines().next().unwrap().split(' ').nth(1).unwrap().parse().unwrap(), buf[split + 4..].to_vec())
}

/// `serve` serves `/describe/{id}`: from local classes if the local store holds the class, else
/// forwarded to the forward mesh; the mesh document lists the endpoint.
#[test]
fn serve_serves_describe() {
    let up = upstream_classes();
    let dir = TempDir::new("describe-serve");
    let r = Arc::new(resolver(&dir, &up));
    let (_, class, f) = with_local_class(&r);
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let server = rt.block_on(Server::bind(r.clone(), "127.0.0.1:0".parse().unwrap())).unwrap();
    let addr = server.local_addr();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = rt.spawn(server.run(async {
        let _ = rx.await;
    }));
    let enc = |p: &str| p.replace('"', "%22");
    // A local class: from the local store.
    let (status, body) = http_get(addr, "/describe/local.invalid/pair");
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let held = std::collections::BTreeMap::from([("local.invalid/pair".to_string(), class)]);
    check_describe_with(&body, &held, std::slice::from_ref(&f)).unwrap();
    assert_eq!(body, r.describe("local.invalid/pair").unwrap().body);
    let (status, body) = http_get(addr, &format!("/describe/{}", enc(r#"local.invalid/pair(base:"EUR",quote:"USD")"#)));
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    // Another class: forwarded.
    let (held, functions) = held_classes(&up);
    let (status, body) = http_get(addr, &format!("/describe/{VERSE_CLASS}"));
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    check_describe_with(&body, &held, &functions).unwrap();
    let (status, _) = http_get(addr, &format!("/describe/{HOST}/ecb/nothing"));
    assert_eq!(status, 404);
    // The mesh document names it, on this endpoint.
    let (_, md) = http_get(addr, "/.well-known/datumesh-mesh.json");
    let doc = json::parse(&md).unwrap();
    assert_eq!(
        doc.get("endpoints").and_then(|e| e.get("describe")).and_then(Value::as_str),
        Some(format!("http://{addr}/describe/{{id}}").as_str())
    );
    let _ = tx.send(());
    rt.block_on(handle).unwrap().unwrap();
}
