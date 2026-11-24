//! Against the live test mesh, read-only (a few dozen requests in all, under its rate
//! limits): brief tests 3 and 5 and the user's scripture prefetch. Run with DATUMESH_LIVE=1
//! and `--ignored`; the default configuration's allowlist and forward mesh are used as
//! installed.

mod common;

use common::*;
use datumesh_core::json::Value;
use datumesh_local::{Resolver, Source, FOREIGN_UNAVAILABLE};

fn live() -> Option<(TempDir, Resolver)> {
    if std::env::var("DATUMESH_LIVE").is_err() {
        eprintln!("DATUMESH_LIVE is not set: skipped");
        return None;
    }
    let dir = TempDir::new("live");
    let r = Resolver::open(common::in_dir_050(dir.path()).unwrap()).unwrap();
    Some((dir, r))
}

#[test]
#[ignore = "the live test mesh (DATUMESH_LIVE=1)"]
fn live_prefetch_of_the_ecb_slice_answers_offline() {
    let Some((_d, r)) = live() else { return };
    let p = r.prefetch("mesh-test.datumesh.com/ecb/fx_rate(base: *, quote: *, date: *)", Some(10_000), false).unwrap();
    eprintln!("{p:?}");
    assert_eq!(p.members, 121);
    assert_eq!(p.pages, 1);
    r.set_offline(true);
    for q in ["USD", "GBP", "JPY", "CHF"] {
        for d in ["2026-09-21", "2026-10-15", "2026-10-30"] {
            let reply = r.get(&format!(r#"ecb/fx_rate(base: "EUR", quote: "{q}", date: {d})"#)).unwrap();
            assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.body));
            assert_eq!(reply.source, Source::Local);
            assert_eq!(s(&envelope(&reply.body), "trust"), "primary");
        }
    }
    let nok = r.get(r#"ecb/fx_rate(base: "EUR", quote: "NOK", date: 2026-10-30)"#).unwrap();
    assert_eq!(nok.status, 200);
    let h = r.get(r#"ecb/holiday(calendar: "TARGET", date: 2026-12-25)"#).unwrap();
    assert_eq!(s(&envelope(&h.body), "error"), FOREIGN_UNAVAILABLE);
}

#[test]
#[ignore = "the live test mesh (DATUMESH_LIVE=1)"]
fn live_prefetch_of_the_quran_answers_offline() {
    let Some((_d, r)) = live() else { return };
    let t = std::time::Instant::now();
    let p = r
        .prefetch(
            r#"scripture/verse_text(corpus: "quran", book: *, chapter: *, verse: *, edition: "uthmani")"#,
            Some(10_000),
            false,
        )
        .unwrap();
    eprintln!("{p:?} in {:?}", t.elapsed());
    assert_eq!(p.members, 6236);
    r.set_offline(true);
    for (b, v) in [("1", 1), ("2", 255), ("114", 6), ("36", 83)] {
        let q = format!(
            r#"scripture/verse_text(corpus: "quran", book: "{b}", chapter: 1, verse: {v}, edition: "uthmani")"#
        );
        let reply = r.get(&q).unwrap();
        assert_eq!(reply.status, 200, "{q}: {}", String::from_utf8_lossy(&reply.body));
        assert_eq!(reply.source, Source::Local);
        let rc = receipt(&reply.body);
        assert_eq!(s(&rc, "licence"), "https://tanzil.net/docs/text_license");
        assert_eq!(rc.get("type"), Some(&Value::String("blob".into())));
    }
    // Not prefetched: the simple edition.
    let other =
        r.get(r#"scripture/verse_text(corpus: "quran", book: "1", chapter: 1, verse: 1, edition: "simple")"#).unwrap();
    assert_eq!(s(&envelope(&other.body), "error"), FOREIGN_UNAVAILABLE);
}

#[test]
#[ignore = "the live test mesh (DATUMESH_LIVE=1)"]
fn live_gate_forwards_the_restricted_cross_rate() {
    let Some((_d, r)) = live() else { return };
    let a = r.get(r#"ecb/fx_cross(base: "USD", quote: "NOK", date: 2026-10-30)"#).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    assert_eq!(a.source, Source::Forwarded { mesh: "mesh-test.datumesh.com".into() });
    let rc = receipt(&a.body);
    assert!(s(&rc, "computed_by").starts_with("mesh-test.datumesh.com#"));
    assert_eq!(s(&envelope(&a.body), "trust"), "computed");
    eprintln!("forwarded: {}", s(&rc, "computed_by"));
    let b = r.get(r#"ecb/fx_cross(base: "USD", quote: "GBP", date: 2026-10-30)"#).unwrap();
    assert_eq!(b.status, 200, "{}", String::from_utf8_lossy(&b.body));
    assert_eq!(s(&receipt(&b.body), "computed_by"), format!("local.invalid#{}", r.key_id()));
}
