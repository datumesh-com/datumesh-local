//! Requests with classes (spec-0.5.0, 5.8, 16.3, 16.7; `CONTRACT-local-0.5.0.md` items 1 and 5).
//!
//! `get` and `explain` take the call form through formula 0.3.0's `parse_call`: method calls with
//! unquoted values by position type, `N[label]`, the `asof` default (the call's `asof`, or now),
//! bare names through the alias map. The classes, the clock and the lookups come from the local
//! store or the forward mesh's bindings. The envelope carries `resolved` when something was
//! resolved. **A method call and its plain read give the same cell id, the same receipt and the
//! same envelope apart from `resolved`.**
//!
//! The upstream is `common::upstream_classes()` (the live check's `verse` and `fx_pair`, and
//! `fx_named`, which has a `lookup`); the resolver reaches it as its forward mesh.

mod common;

use common::*;
use datumesh_core::json::Value;
use datumesh_local::{Error, Source};

fn call(c: &str, asof: Option<&str>) -> String {
    match asof {
        Some(a) => format!("{c}?asof={a}"),
        None => c.to_string(),
    }
}

/// A receipt without its signing (`issued`, `sig`): what an answer states. A read that names an asof
/// stores nothing (CONTRACT-guard-patch B1, datumesh-engine 0.6.2), so each such read is evaluated and
/// signed again: two of them state the same thing and differ in these two members only.
fn unsigned(receipt: &Value) -> Value {
    match receipt {
        Value::Object(m) => Value::Object(m.iter().filter(|(k, _)| k != "issued" && k != "sig").cloned().collect()),
        other => other.clone(),
    }
}

/// The envelope without `resolved`, its receipt [`unsigned`].
fn stated(body: &[u8]) -> Value {
    match without_resolved(body) {
        Value::Object(m) => {
            Value::Object(m.into_iter().map(|(k, v)| if k == "receipt" { (k, unsigned(&v)) } else { (k, v) }).collect())
        }
        other => other,
    }
}

/// The same statement as `want`'s, and `got`'s own receipt verifies here.
fn same_answer(r: &datumesh_local::Resolver, got: &[u8], want: &[u8], what: &str) {
    assert_eq!(unsigned(&receipt(got)).to_jcs(), unsigned(&receipt(want)).to_jcs(), "{what}: the receipt");
    assert_eq!(stated(got), stated(want), "{what}: the envelope");
    let v = r.verify(&receipt(got).to_jcs());
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{what}: {}", String::from_utf8_lossy(&v.to_jcs()));
}

fn cell_id(body: &[u8]) -> String {
    datumesh_core::Receipt::from_value(&receipt(body)).unwrap().cell().id()
}

/// `resolved`: the full call, and each resolution as `(from, kind, to, asof)`.
type Resolutions = Vec<(String, String, String, Option<String>)>;

fn resolved_of(body: &[u8]) -> (String, Resolutions, Option<String>) {
    let env = envelope(body);
    let r = env.get("resolved").unwrap_or_else(|| panic!("no resolved in {}", String::from_utf8_lossy(body)));
    let call = s(r, "call");
    let Some(Value::Array(rs)) = r.get("resolutions") else { panic!("no resolutions") };
    let list = rs
        .iter()
        .map(|x| {
            let f = |k: &str| x.get(k).and_then(Value::as_str).map(str::to_string);
            (f("from").unwrap(), f("kind").unwrap(), f("to").unwrap(), f("asof"))
        })
        .collect();
    (call, list, r.get("aliases_tx").and_then(Value::as_str).map(str::to_string))
}

fn res(from: &str, kind: &str, to: &str) -> (String, String, String, Option<String>) {
    (from.into(), kind.into(), to.into(), None)
}

// ---------------------------------------------------------------------------------------------
// Method-call equivalence

/// `N(keys).m(args)` and the plain read `N/m(...)` give the same cell id, the same receipt (the
/// same bytes) and the same envelope apart from `resolved`: for a stored cell (a blob), and for
/// computed ones (a formula over a stored method; a formula over two). The plain read has no
/// `resolved`; the method call has one. Every spelling (quoted, unquoted, labelled, short names
/// through the alias map's prefixes) is the same request.
#[test]
fn a_method_call_and_the_plain_read_are_one_cell() {
    let up = upstream_classes();
    let (v, p) = (VERSE_CLASS, PAIR_CLASS);
    // (method calls, the plain read, the function's name)
    let cases: Vec<(Vec<String>, String, String)> = vec![
        (
            vec![
                call(&format!("{v}(quran, 1, 1, 2).text(uthmani)"), None),
                call(&format!("{v}(\"quran\", \"1\", 1, 2).text(\"uthmani\")"), None),
                call(&format!("{v}(quran,1,1,2).text(uthmani)"), None),
                call(&format!("{v}(corpus: quran, book: 1, chapter: 1, verse: 2).text(edition: uthmani)"), None),
                call("scripture/verse(quran, 1, 1, 2).text(uthmani)", None),
            ],
            call(&format!(r#"{v}/text(corpus: "quran", book: "1", chapter: 1, verse: 2, edition: "uthmani")"#), None),
            format!("{v}/text"),
        ),
        (
            vec![
                call(&format!("{v}(quran, 1, 1, 2).consonantal()"), Some(ASOF)),
                call(&format!("{v}(\"quran\", \"1\", 1, 2).consonantal()"), Some(ASOF)),
            ],
            call(&format!(r#"{v}/consonantal(corpus: "quran", book: "1", chapter: 1, verse: 2)"#), Some(ASOF)),
            format!("{v}/consonantal"),
        ),
        (
            vec![
                call(&format!("{p}(EUR, USD).rate({DAY})"), None),
                call(&format!("{p}(\"EUR\", \"USD\").rate({DAY})"), None),
                call(&format!("ecb/fx_pair(EUR, USD).rate({DAY})"), None),
            ],
            call(&format!(r#"{p}/rate(base: "EUR", quote: "USD", date: {DAY})"#), None),
            format!("{p}/rate"),
        ),
        (
            vec![
                call(&format!("{p}(GBP, USD).cross({DAY})"), Some(ASOF)),
                call(&format!("{p}(base: GBP, quote: USD).cross(date: {DAY})"), Some(ASOF)),
            ],
            call(&format!(r#"{p}/cross(base: "GBP", quote: "USD", date: {DAY})"#), Some(ASOF)),
            format!("{p}/cross"),
        ),
    ];
    for (methods, plain, name) in cases {
        // A resolver of its own for each: the plain read first, then the method calls.
        let dir = TempDir::new("classes-eq");
        let r = resolver(&dir, &up);
        let want = r.get(&plain).unwrap();
        assert_eq!(want.status, 200, "{name}: {}", String::from_utf8_lossy(&want.body));
        assert!(envelope(&want.body).get("resolved").is_none(), "{name}: a plain read of a full name resolves nothing");
        let id = cell_id(&want.body);
        for m in &methods {
            let got = r.get(m).unwrap();
            assert_eq!(got.status, 200, "{m}: {}", String::from_utf8_lossy(&got.body));
            assert_eq!(cell_id(&got.body), id, "{m}: the cell id");
            same_answer(&r, &got.body, &want.body, m);
            let (full, rs, _) = resolved_of(&got.body);
            assert!(full.starts_with(&format!("{name}(")), "{m}: resolved.call is the full call: {full}");
            assert!(rs.iter().any(|x| x.1 == "method" && x.2 == name), "{m}: a method resolution: {rs:?}");
        }
        // Every one of them verifies, here.
        let v = r.verify(&receipt(&want.body).to_jcs());
        assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{name}: {}", String::from_utf8_lossy(&v.to_jcs()));
    }
}

/// A method call on a resolver that holds nothing: the class and its method functions come from the
/// forward mesh's bindings (a fetch), the answer is the plain read's, and later answers, online or
/// offline, have the same bytes (from the store).
#[test]
fn a_method_call_fetches_the_class_once_and_is_then_answered_offline() {
    let up = upstream_classes();
    let dir = TempDir::new("classes-fetch");
    let r = resolver(&dir, &up);
    let m = format!("{VERSE_CLASS}(quran, 1, 1, 2).text(uthmani)");
    let first = r.get(&m).unwrap();
    assert_eq!(first.status, 200, "{}", String::from_utf8_lossy(&first.body));
    assert_eq!(first.source, Source::Fetched);
    assert!(up.requests() > 0);
    r.set_offline(true);
    let again = r.get(&m).unwrap();
    assert_eq!(again.body, first.body);
    assert_eq!(again.source, Source::Local);
    // And the plain read, offline, is the same cell.
    let plain = r
        .get(&format!(r#"{VERSE_CLASS}/text(corpus: "quran", book: "1", chapter: 1, verse: 2, edition: "uthmani")"#))
        .unwrap();
    assert_eq!(plain.status, 200);
    assert_eq!(without_resolved(&plain.body), without_resolved(&first.body));
    // A method call of a class never fetched, offline: nothing to read the class from.
    let other = r.get(&format!("{PAIR_CLASS}(EUR, USD).rate({DAY})")).unwrap();
    assert_eq!(other.status, 502, "{}", String::from_utf8_lossy(&other.body));
    assert_eq!(s(&envelope(&other.body), "error"), datumesh_local::FOREIGN_UNAVAILABLE);
}

// ---------------------------------------------------------------------------------------------
// resolved, defaults, aliases, labels

/// A short call whose `date` is left out: the default takes the call's `asof` (the cell URI carries
/// the value, so the call is the plain read with that date), `resolved` records the method and the
/// default (in the order alias, lookup, method, default); a bare name of the alias map is an `alias`
/// resolution and the answer says which snapshot (`aliases_tx`, the map's `tx`). With no `asof` the
/// default takes now.
#[test]
fn resolved_on_a_short_call_with_a_defaulted_date() {
    let up = upstream_classes();
    let dir = TempDir::new("classes-resolved");
    let r = resolver_with_aliases(&dir, &up);
    let p = PAIR_CLASS;
    let plain = r.get(&format!(r#"{p}/cross(base: "GBP", quote: "USD", date: {DAY})?asof={ASOF}"#)).unwrap();
    assert_eq!(plain.status, 200, "{}", String::from_utf8_lossy(&plain.body));
    assert_eq!(receipt(&plain.body).get("value").and_then(Value::as_str), Some("1.5"));
    let full = format!(r#"{p}/cross(base: "GBP", date: {DAY}, quote: "USD")?asof={ASOF}"#);

    let got = r.get(&call(&format!("{p}(GBP, USD).cross()"), Some(ASOF))).unwrap();
    assert_eq!(got.status, 200, "{}", String::from_utf8_lossy(&got.body));
    assert_eq!(cell_id(&got.body), cell_id(&plain.body), "the default is bound in the cell");
    same_answer(&r, &got.body, &plain.body, "the defaulted date");
    let (c, rs, tx) = resolved_of(&got.body);
    assert_eq!(c, full);
    assert_eq!(rs, vec![res(&format!("{p}(...).cross"), "method", &format!("{p}/cross")), res("date", "default", DAY)]);
    assert_eq!(tx, None, "no alias was used");

    // A bare name of the alias map: the alias resolution comes first, the snapshot is named.
    for text in ["xcross(GBP, USD)", "@xcross(GBP, USD)", "xcross(base: GBP, quote: USD)"] {
        let got = r.get(&call(text, Some(ASOF))).unwrap();
        assert_eq!(got.status, 200, "{text}: {}", String::from_utf8_lossy(&got.body));
        assert_eq!(unsigned(&receipt(&got.body)).to_jcs(), unsigned(&receipt(&plain.body)).to_jcs(), "{text}");
        let (c, rs, tx) = resolved_of(&got.body);
        assert_eq!(c, full, "{text}");
        let from = if text.starts_with('@') { "@xcross" } else { "xcross" };
        assert_eq!(rs, vec![res(from, "alias", &format!("{p}/cross")), res("date", "default", DAY)], "{text}");
        assert_eq!(tx.as_deref(), Some(ALIASES_TX), "{text}");
    }

    // A date given is not defaulted.
    let got = r.get(&call(&format!("{p}(GBP, USD).cross({DAY})"), Some(ASOF))).unwrap();
    assert_eq!(resolved_of(&got.body).1, vec![res(&format!("{p}(...).cross"), "method", &format!("{p}/cross"))]);

    // No asof: the default takes now. No cell has a value for today's date: not_covered, with
    // `resolved`, whose date is today's (UTC).
    let before = today();
    let got = r.get(&format!("{p}(GBP, USD).cross()")).unwrap();
    let after = today();
    assert_eq!((got.status, s(&envelope(&got.body), "outcome")), (404, "not_covered".to_string()));
    let (c, rs, _) = resolved_of(&got.body);
    let days = [before, after];
    assert!(days.iter().any(|d| c == format!(r#"{p}/cross(base: "GBP", date: {d}, quote: "USD")"#)), "{c}");
    assert!(rs.iter().any(|x| x.0 == "date" && x.1 == "default" && days.contains(&x.2)), "{rs:?}");
}

/// `N[label]` names the object the class's `lookup` maps the label to, as of the call's asof; the
/// lookup cell comes from the forward mesh; `resolved` has a `lookup` resolution with its `asof`.
/// A label with no value is `E_NAME_UNBOUND` (`not_covered`).
#[test]
fn a_label_resolves_through_the_classes_lookup() {
    let up = upstream_classes();
    let dir = TempDir::new("classes-label");
    let r = resolver(&dir, &up);
    let nm = NAMED_CLASS;
    let plain = r.get(&format!(r#"{nm}/rate(base: "GBP", quote: "USD", date: {DAY})"#)).unwrap();
    assert_eq!(plain.status, 200, "{}", String::from_utf8_lossy(&plain.body));
    for text in [format!("{nm}[cable].rate({DAY})"), format!("{nm}[\"cable\"].rate({DAY})")] {
        let got = r.get(&call(&text, Some(ASOF))).unwrap();
        assert_eq!(got.status, 200, "{text}: {}", String::from_utf8_lossy(&got.body));
        assert_eq!(receipt(&got.body).to_jcs(), receipt(&plain.body).to_jcs(), "{text}");
        assert_eq!(without_resolved(&got.body), without_resolved(&plain.body), "{text}");
        let (c, rs, _) = resolved_of(&got.body);
        assert_eq!(c, format!(r#"{nm}/rate(base: "GBP", date: {DAY}, quote: "USD")?asof={ASOF}"#), "{text}");
        assert_eq!(
            rs,
            vec![
                (
                    "cable".to_string(),
                    "lookup".to_string(),
                    format!(r#"{nm}(base:"GBP",quote:"USD")"#),
                    Some(ASOF.to_string())
                ),
                res(&format!("{nm}(...).rate"), "method", &format!("{nm}/rate")),
            ],
            "{text}"
        );
    }
    for text in [format!("{nm}[nope].rate({DAY})"), format!("{PAIR_CLASS}[cable].rate({DAY})")] {
        let got = r.get(&call(&text, Some(ASOF))).unwrap();
        let env = envelope(&got.body);
        assert_eq!(
            (got.status, s(&env, "outcome"), s(&env, "error")),
            (404, "not_covered".into(), "E_NAME_UNBOUND".into()),
            "{text}"
        );
    }
}

/// `explain` takes the call form as `get` does: the lineage of a method call is the lineage of its
/// plain read.
#[test]
fn explain_takes_a_method_call() {
    let up = upstream_classes();
    let dir = TempDir::new("classes-explain");
    let r = resolver(&dir, &up);
    // explain answers from the store: the reads come first (the second fetches the class).
    let plain = format!(r#"{PAIR_CLASS}/cross(base: "GBP", quote: "USD", date: {DAY})?asof={ASOF}"#);
    let method = call(&format!("{PAIR_CLASS}(GBP, USD).cross({DAY})"), Some(ASOF));
    assert_eq!(r.get(&plain).unwrap().status, 200);
    assert_eq!(r.get(&method).unwrap().status, 200);
    let want = r.explain(&plain, Some(2)).unwrap();
    assert_eq!(want.status, 200, "{}", String::from_utf8_lossy(&want.body));
    let m = r.explain(&method, Some(2)).unwrap();
    assert_eq!(m.status, 200, "{}", String::from_utf8_lossy(&m.body));
    // A receipt cell carries the namespace and the function name as separate members (SPEC 4.1):
    // the method `rate` of the class, read as an input of `cross`.
    let body = String::from_utf8_lossy(&m.body);
    assert!(body.contains(r#""fn":"rate""#) && body.contains(&format!(r#""ns":"{PAIR_CLASS}""#)), "{body}");
    // The same lineage: the same receipts (the hashes of the cell and of its inputs).
    let hashes = |b: &[u8]| {
        let t = String::from_utf8_lossy(b).into_owned();
        let mut v: Vec<String> =
            t.split("\"hash\":\"").skip(1).filter_map(|x| x.get(..64).map(str::to_string)).collect();
        v.sort();
        v
    };
    assert_eq!(hashes(&m.body), hashes(&want.body));
}

/// A malformed call is refused as a bad request, before anything is fetched (no request reaches
/// the upstream for a call that does not parse).
#[test]
fn a_malformed_call_is_refused_without_a_request() {
    let up = upstream_classes();
    let dir = TempDir::new("classes-bad");
    let r = resolver(&dir, &up);
    r.set_offline(true);
    for text in [
        format!("{VERSE_CLASS}(quran, 1, 1, 2).text(\"uth mani\" extra)"),
        format!("{PAIR_CLASS}(EUR, USD).rate(2026-03-02)?asof=yesterday"),
    ] {
        match r.get(&text) {
            Ok(reply) => {
                assert_eq!(reply.status, 400, "{text}: {}", String::from_utf8_lossy(&reply.body));
                assert_eq!(s(&envelope(&reply.body), "outcome"), "bad_request", "{text}");
            }
            Err(Error::Request(_)) => {}
            Err(e) => panic!("{text}: {e}"),
        }
    }
    assert_eq!(up.requests(), 0, "no request to the upstream");
}

// ---------------------------------------------------------------------------------------------
// Local classes (item 5)

const RATE_SIG: &str = r#"{"args": {"base": {"type": "string", "valid": false}, "date": {"type": "date", "valid": true}, "quote": {"type": "string", "valid": false}}, "order": ["base", "quote", "date"], "returns": {"type": "number", "unit": "1"}}"#;

fn local_class(rate_hash: &str) -> String {
    format!(
        r#"{{"keys": {{"base": {{"key_normalisation": "none", "type": "string"}}, "quote": {{"key_normalisation": "none", "type": "string"}}}}, "methods": {{"rate": "{rate_hash}"}}, "order": ["base", "quote"], "steward": {{"namespace": "local.invalid", "process": "https://local.invalid/classes/change"}}}}"#
    )
}

/// `define_class` under `local/` only: owner `local.invalid`, `put_class`, `bind_class`; the class
/// is then called as any other, over a local stored cell (`publish`); a class under another
/// namespace is refused.
#[test]
fn a_local_class_is_defined_and_called() {
    let up = upstream_classes();
    let dir = TempDir::new("classes-local");
    let r = resolver(&dir, &up);
    let rate = r.define_stored("local/pair/rate", &datumesh_core::json::parse(RATE_SIG.as_bytes()).unwrap()).unwrap();
    let d = r.define_class("local/pair", local_class(&rate.fn_hash).as_bytes()).unwrap();
    assert_eq!(d.name, "local.invalid/pair");
    assert!(d.bound);
    // Again: nothing changes.
    assert!(!r.define_class("local/pair", local_class(&rate.fn_hash).as_bytes()).unwrap().bound);
    let b = r.store().resolve_class_binding("local.invalid/pair", END).unwrap().expect("bound");
    assert_eq!(b.class_hash, d.fn_hash, "Defined.fn_hash is the class hash for a class");
    // A cell, by publish.
    let file = dir.path().join("pair.jsonl");
    std::fs::write(
        &file,
        concat!(
            r#"{"cell": {"fn": "local/pair/rate", "args": {"base": {"type": "string", "value": "EUR"}, "date": {"type": "date", "value": "2099-03-02"}, "quote": {"type": "string", "value": "USD"}}}, "value": {"type": "number", "value": "1.2"}, "tx": "2099-03-02T16:00:00.000000Z", "asof": "2099-03-02T16:00:00.000000Z", "licence": "CC0-1.0", "summary": "A rate."}"#,
            "\n"
        ),
    )
    .unwrap();
    r.publish(&file).unwrap();
    let m = r.get("local/pair(EUR, USD).rate(2099-03-02)?asof=2099-03-03T00:00:00.000000Z").unwrap();
    assert_eq!(m.status, 200, "{}", String::from_utf8_lossy(&m.body));
    let plain = r
        .get(r#"local/pair/rate(base: "EUR", quote: "USD", date: 2099-03-02)?asof=2099-03-03T00:00:00.000000Z"#)
        .unwrap();
    assert_eq!(receipt(&m.body).to_jcs(), receipt(&plain.body).to_jcs());
    assert_eq!(without_resolved(&m.body), without_resolved(&plain.body));
    let (c, rs, _) = resolved_of(&m.body);
    assert_eq!(
        c,
        r#"local.invalid/pair/rate(base: "EUR", date: 2099-03-02, quote: "USD")?asof=2099-03-03T00:00:00.000000Z"#
    );
    assert!(rs.iter().any(|x| x.1 == "method" && x.2 == "local.invalid/pair/rate"), "{rs:?}");
    // Not under local/: refused, and nothing is stored.
    let e = r.define_class("example.org/pair", local_class(&rate.fn_hash).as_bytes());
    assert!(matches!(e, Err(Error::Request(_)) | Err(Error::Define(_))), "{e:?}");
    assert!(r.store().resolve_class_binding("example.org/pair", END).unwrap().is_none());
}

/// `publish` accepts class lines as the mesh's publish-stored does (`{"class": ..., "descriptor":
/// ..., "bound": ...}`, the method functions first in the file or already defined), under `local/`
/// only.
#[test]
fn publish_accepts_class_lines() {
    let up = upstream_classes();
    let dir = TempDir::new("classes-publish");
    let r = resolver(&dir, &up);
    let rate = r.define_stored("local/pair/rate", &datumesh_core::json::parse(RATE_SIG.as_bytes()).unwrap()).unwrap();
    let descriptor = local_class(&rate.fn_hash);
    let file = dir.path().join("class.jsonl");
    std::fs::write(
        &file,
        format!(
            r#"{{"class": "local/pair", "bound": "2099-03-01T00:00:00.000000Z", "descriptor": {descriptor}}}{}"#,
            "\n"
        ),
    )
    .unwrap();
    let p = r.publish(&file).unwrap();
    assert_eq!(p.classes, 1);
    assert!(r.store().resolve_class_binding("local.invalid/pair", END).unwrap().is_some());
    // Again: nothing is bound again.
    assert_eq!(r.publish(&file).unwrap().classes, 0);
    // Not under local/.
    let other = dir.path().join("other.jsonl");
    std::fs::write(
        &other,
        format!(
            r#"{{"class": "example.org/pair", "bound": "2026-03-01T00:00:00.000000Z", "descriptor": {descriptor}}}{}"#,
            "\n"
        ),
    )
    .unwrap();
    assert!(matches!(r.publish(&other), Err(Error::Request(_))));
    assert!(r.store().resolve_class_binding("example.org/pair", END).unwrap().is_none());
}
