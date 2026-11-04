//! Brief test 3: prefetch pulls a slice (pages), its descriptors and keys; a later offline get
//! of any member is a local hit; refresh picks up a new version published on the mesh. Here
//! against the upstream mesh started by the tests (the live mesh's version is in
//! `tests/live.rs`).

mod common;

use common::*;
use datumesh_core::json::Value;
use datumesh_local::Source;

const DATES: [&str; 3] = ["2026-09-21", "2026-10-14", "2026-10-30"];

#[test]
fn prefetch_pulls_every_page_and_the_members_answer_offline() {
    // Pages of 50: the 121 fx_rate cells come in three pages.
    let up = upstream_with(50);
    let dir = TempDir::new("prefetch");
    let r = resolver(&dir, &up);
    let p = r.prefetch("ecb/fx_rate(base: *, quote: *, date: *)", None, false).unwrap();
    assert_eq!(p.members, 121);
    assert_eq!(p.pages, 3);
    assert_eq!(p.versions, 121);
    assert_eq!(p.names, 1);
    assert!(p.target.starts_with("mesh-test.datumesh.com/ecb/fx_rate?"), "{}", p.target);
    // The key document and the descriptor are held: the store verifies with them.
    assert!(r.store().namespace(HOST).unwrap().is_some());
    let b = r.store().resolve_binding("mesh-test.datumesh.com/ecb/fx_rate", END).unwrap().unwrap();
    assert!(r.store().descriptor(&b.fn_hash).is_some());
    // Offline: every member is a local hit, and nothing reaches the upstream.
    r.set_offline(true);
    let before = up.requests();
    for q in ["USD", "GBP", "JPY", "CHF"] {
        for d in DATES {
            let reply = r.get(&format!(r#"ecb/fx_rate(base: "EUR", quote: "{q}", date: {d})"#)).unwrap();
            assert_eq!(reply.status, 200, "{q} {d}: {}", String::from_utf8_lossy(&reply.body));
            assert_eq!(reply.source, Source::Local);
            assert_eq!(s(&receipt(&reply.body), "computed_by").split('#').next(), Some(HOST));
        }
    }
    assert_eq!(up.requests(), before, "an offline get reached the network");
}

#[test]
fn prefetch_with_a_page_size_and_of_a_function_name() {
    let up = upstream();
    let dir = TempDir::new("prefetch-page");
    let r = resolver(&dir, &up);
    let p = r.prefetch(FX_SLICE, Some(40), false).unwrap();
    assert_eq!((p.members, p.pages), (121, 4));
    // A function name: its binding, descriptor and key document only.
    let f = r.prefetch("ecb/holiday", None, false).unwrap();
    assert_eq!((f.names, f.members, f.pages), (1, 0, 0));
    assert!(r.store().resolve_binding("mesh-test.datumesh.com/ecb/holiday", END).unwrap().is_some());
    // A slice with a fixed argument, and blob content.
    let v = r
        .prefetch(
            r#"scripture/verse_text(corpus: "quran", book: *, chapter: *, verse: *, edition: "uthmani")"#,
            None,
            true,
        )
        .unwrap();
    assert_eq!((v.members, v.blobs), (7, 7));
    r.set_offline(true);
    let reply = r.get(VERSE).unwrap();
    assert_eq!(reply.status, 200);
    let lex = s(&receipt(&reply.body), "value");
    let sha = lex.split(':').next().unwrap();
    assert!(r.store().blob(sha).unwrap().is_some(), "the blob is held");
}

#[test]
fn refresh_picks_up_a_new_version_published_on_the_mesh() {
    let up = upstream();
    let dir = TempDir::new("refresh");
    let r = resolver(&dir, &up);
    r.prefetch(FX_SLICE, None, false).unwrap();
    // A single cell held outside the slice too (a holiday), refreshed through /versions.
    let h1 = r.get(CHRISTMAS).unwrap();
    assert_eq!(h1.status, 200);
    let old = r.get(USD).unwrap();
    assert_eq!(s(&receipt(&old.body), "value"), "1.1617");
    // The upstream publishes a correction of USD on 2026-10-30, and a new holiday version.
    let t = "2026-11-03T09:00:00.000000Z";
    let line = |fn_: &str, args: Value, value: Value| {
        datumesh_core::json::parse(
            format!(
                r#"{{"cell": {{"fn": "{fn_}", "args": {}}}, "value": {}, "tx": "{t}", "asof": "{t}", "licence": "CC-BY-4.0", "summary": "Synthetic test data: a correction."}}"#,
                String::from_utf8(args.to_jcs()).unwrap(),
                String::from_utf8(value.to_jcs()).unwrap()
            )
            .as_bytes(),
        )
        .unwrap()
    };
    let p = |t: &str| datumesh_core::json::parse(t.as_bytes()).unwrap();
    up.publish_lines(&[
        line(
            "mesh-test.datumesh.com/ecb/fx_rate",
            p(r#"{"base": {"type": "string", "value": "EUR"}, "date": {"type": "date", "value": "2026-10-30"}, "quote": {"type": "string", "value": "USD"}}"#),
            p(r#"{"type": "number", "value": "1.1777"}"#),
        ),
        line(
            "mesh-test.datumesh.com/ecb/holiday",
            p(r#"{"calendar": {"type": "string", "value": "TARGET"}, "date": {"type": "date", "value": "2026-12-25"}}"#),
            p(r#"{"type": "boolean", "value": false}"#),
        ),
    ]);
    // Held cells are answered from the store: no network, the old version.
    let before = up.requests();
    assert_eq!(r.get(USD).unwrap().body, old.body);
    assert_eq!(up.requests(), before);
    let f = r.refresh().unwrap();
    assert!(f.failed.is_empty(), "{:?}", f.failed);
    assert_eq!(f.slices, 1);
    assert_eq!(f.cells, 1);
    assert_eq!(f.versions, 2);
    let new = r.get(USD).unwrap();
    assert_eq!(s(&receipt(&new.body), "value"), "1.1777");
    assert_eq!(s(&receipt(&new.body), "tx"), t);
    let h2 = r.get(CHRISTMAS).unwrap();
    assert_eq!(receipt(&h2.body).get("value"), Some(&Value::Bool(false)));
    // As of before the correction, the old version still answers.
    let at = r.get(&format!("{USD_FULL}?asof=2026-11-01T00:00:00.000000Z")).unwrap();
    assert_eq!(s(&receipt(&at.body), "value"), "1.1617");
    // A second refresh adds nothing.
    assert_eq!(r.refresh().unwrap().versions, 0);
}

#[test]
fn prefetch_offline_or_of_a_host_not_allowed_fetches_nothing() {
    let up = upstream();
    let dir = TempDir::new("prefetch-offline");
    let r = resolver(&dir, &up);
    r.set_offline(true);
    assert!(matches!(r.prefetch(FX_SLICE, None, false), Err(datumesh_local::Error::ForeignUnavailable(_))));
    r.set_offline(false);
    assert!(matches!(
        r.prefetch("example.org/x/f(a: *)", None, false),
        Err(datumesh_local::Error::ForeignUnavailable(d)) if d.starts_with("not allowed")
    ));
    assert_eq!(up.requests(), 0);
}
