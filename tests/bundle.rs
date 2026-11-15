//! Brief test 6: a working set exported from one resolver and imported into a fresh one
//! answers the same cells with identical bytes for signed receipts; local receipts import but
//! are labelled unverifiable, unless their machine's host is trusted with --trust-key (1.8b L4:
//! by host, never this machine's own host, never merged into this machine's key document).

mod common;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::{Source, UNVERIFIABLE_LABEL};

const CLOSED_SIG: &str =
    r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": "boolean"}"#;
const CLOSED_SRC: &str = r#"mesh-test.datumesh.com/ecb/holiday(calendar: "TARGET", date: date)"#;
const CLOSED: &str = "local/closed(date: 2026-12-25)";
const NOK: &str = r#"ecb/fx_cross(base: "USD", quote: "NOK", date: 2026-10-30)"#;

struct Exported {
    bundle: Vec<u8>,
    key_id: String,
    answers: Vec<(String, Vec<u8>)>,
    closed: Vec<u8>,
}

fn exported(up: &Upstream) -> Exported {
    exported_from(up, "local.invalid")
}

/// The working set of a machine whose unrooted host is `host` (1.8b L4).
fn exported_from(up: &Upstream, host: &str) -> Exported {
    let dir = TempDir::new("export-a");
    let mut c = config(&dir, up);
    c.host = host.to_string();
    let a = datumesh_local::Resolver::open(c).unwrap();
    a.prefetch(
        r#"scripture/verse_text(corpus: "quran", book: *, chapter: *, verse: *, edition: "uthmani")"#,
        None,
        true,
    )
    .unwrap();
    let mut answers = Vec::new();
    for q in [USD, GBP, CHRISTMAS, VERSE, NOK] {
        let r = a.get(q).unwrap();
        assert_eq!(r.status, 200, "{q}: {}", String::from_utf8_lossy(&r.body));
        answers.push((q.to_string(), r.body));
    }
    a.define_formula("local/closed", CLOSED_SRC, &json::parse(CLOSED_SIG.as_bytes()).unwrap()).unwrap();
    let closed = a.get(CLOSED).unwrap().body;
    Exported { bundle: a.export().unwrap(), key_id: a.key_id().to_string(), answers, closed }
}

#[test]
fn a_fresh_resolver_answers_the_same_bytes_offline_after_import() {
    let up = upstream();
    let e = exported(&up);
    let v = json::parse(&e.bundle).unwrap();
    assert_eq!(v.get("bundle").and_then(Value::as_str), Some("datumesh-local/1"));
    assert_eq!(v.get("machine").and_then(Value::as_str), Some(e.key_id.as_str()));
    // The exporting machine's key document travels.
    assert!(v.get("key_documents").and_then(|k| k.get("local.invalid")).is_some());

    let dir = TempDir::new("import-b");
    let b = resolver(&dir, &up);
    b.set_offline(true);
    let before = up.requests();
    let im = b.import(&e.bundle, &[]).unwrap();
    assert!(im.failed.is_empty(), "{:?}", im.failed);
    assert!(im.receipts >= 5 + 7);
    assert_eq!(im.blobs, 7);
    assert_eq!(im.forwarded, 1);
    // The local receipt is kept aside, labelled.
    assert_eq!(im.quarantined.len(), 1);
    let q = b.quarantined().unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].label, UNVERIFIABLE_LABEL);
    assert_eq!(q[0].key_id, e.key_id);
    for (q, body) in &e.answers {
        let r = b.get(q).unwrap();
        assert_eq!(&r.body, body, "{q}");
    }
    // The prefetched verses answer too, with their content.
    let r =
        b.get(r#"scripture/verse_text(corpus: "quran", book: "1", chapter: 1, verse: 7, edition: "uthmani")"#).unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.source, Source::Local);
    assert_eq!(up.requests(), before, "the import or the gets reached the network");
    // The other machine's receipt does not verify here: its key is unknown.
    let foreign = b.verify(&q[0].bytes);
    assert_eq!(foreign.get("ok"), Some(&Value::Bool(false)));
    assert_eq!(foreign.get("code").and_then(Value::as_str), Some("E_KEY_UNKNOWN"));
    // B evaluates the local cell itself (its inputs were imported), signing with its own key.
    let own = b.get(CLOSED).unwrap();
    assert_eq!(own.status, 200);
    assert_ne!(own.body, e.closed);
    assert_eq!(s(&receipt(&own.body), "computed_by"), format!("local.invalid#{}", b.key_id()));
}

/// The laptop's cell, as the importer names it.
const LAPTOP_CLOSED: &str = "laptop.invalid/closed(date: 2026-12-25)";

#[test]
fn a_trusted_machine_key_makes_its_receipts_answer_and_verify() {
    let up = upstream();
    let e = exported_from(&up, LAPTOP);
    let dir = TempDir::new("import-c");
    let c = resolver(&dir, &up);
    c.set_offline(true);
    // A key id the bundle does not carry is refused.
    assert!(c.import(&e.bundle, &["0123456789abcdef".to_string()]).is_err());
    let im = c.import(&e.bundle, std::slice::from_ref(&e.key_id)).unwrap();
    assert!(im.failed.is_empty(), "{:?}", im.failed);
    // Trust is by host (1.8b L4).
    assert_eq!(im.trusted, vec![LAPTOP.to_string()]);
    assert!(im.quarantined.is_empty());
    assert_eq!(im.computed, 1);
    let r = c.get(LAPTOP_CLOSED).unwrap();
    // The same receipt; the envelope differs (this machine is not the signer, so its trust is not
    // `computed`, and the exporter's request was written with a prefix).
    assert_eq!(receipt(&r.body).to_jcs(), receipt(&e.closed).to_jcs());
    assert_eq!(r.label.as_deref(), Some("trusted machine laptop.invalid"));
    let v = c.verify(&receipt(&r.body).to_jcs());
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", String::from_utf8_lossy(&v.to_jcs()));
    // Never merged: this machine's key document still has its own key only.
    let doc = json::parse(&c.key_document()).unwrap();
    assert_eq!(doc.get("host").and_then(Value::as_str), Some("local.invalid"));
    let n = match doc.get("keys") {
        Some(Value::Array(k)) => k.len(),
        _ => 0,
    };
    assert_eq!(n, 1);
    // Trust survives a restart.
    drop(c);
    let c = resolver(&dir, &up);
    c.set_offline(true);
    assert_eq!(receipt(&c.get(LAPTOP_CLOSED).unwrap().body).to_jcs(), receipt(&e.closed).to_jcs());
}

#[test]
fn a_later_import_with_trust_releases_the_quarantine() {
    let up = upstream();
    let e = exported_from(&up, LAPTOP);
    let dir = TempDir::new("import-d");
    let d = resolver(&dir, &up);
    d.set_offline(true);
    assert_eq!(d.import(&e.bundle, &[]).unwrap().quarantined.len(), 1);
    let im = d.import(&e.bundle, std::slice::from_ref(&e.key_id)).unwrap();
    assert!(im.failed.is_empty(), "{:?}", im.failed);
    assert!(d.quarantined().unwrap().is_empty());
    assert_eq!(receipt(&d.get(LAPTOP_CLOSED).unwrap().body).to_jcs(), receipt(&e.closed).to_jcs());
}

#[test]
fn a_machine_with_this_machines_host_is_never_trusted() {
    let up = upstream();
    let e = exported(&up);
    let dir = TempDir::new("import-e");
    let d = resolver(&dir, &up);
    d.set_offline(true);
    assert!(d.import(&e.bundle, std::slice::from_ref(&e.key_id)).is_err());
    // Its receipt stays aside, and this machine's document is unchanged.
    assert_eq!(d.import(&e.bundle, &[]).unwrap().quarantined.len(), 1);
}

#[test]
fn a_bundle_that_is_not_one_is_refused() {
    let up = upstream();
    let dir = TempDir::new("import-bad");
    let r = resolver(&dir, &up);
    assert!(r.import(b"{}", &[]).is_err());
    assert!(r.import(b"not json", &[]).is_err());
}
