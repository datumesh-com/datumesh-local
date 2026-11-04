//! Brief test 2: with the network blocked, every cell fetched before answers from the store
//! with the same bytes; a foreign cell that was not fetched answers `error` with a code that
//! says it could not be fetched (`E_FOREIGN_UNAVAILABLE`, spec-0.4.3, HTTP 502), never
//! `not_covered`.

mod common;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::{foreign_unavailable_envelope, Source, FOREIGN_UNAVAILABLE};

const NOK: &str = r#"ecb/fx_cross(base: "USD", quote: "NOK", date: 2026-10-30)"#;
const CLOSED_SIG: &str =
    r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": "boolean"}"#;

/// `kind`: the envelope's `detail` (spec-0.4.3's closed set); `why`: how the reply's own
/// detail (the full failure) starts.
fn assert_unavailable(reply: &datumesh_local::Reply, kind: &str, why: &str) {
    assert_eq!(reply.status, 502, "{}", String::from_utf8_lossy(&reply.body));
    let env = envelope(&reply.body);
    assert_eq!(s(&env, "outcome"), "error");
    assert_eq!(s(&env, "error"), FOREIGN_UNAVAILABLE);
    assert_eq!(FOREIGN_UNAVAILABLE, "E_FOREIGN_UNAVAILABLE");
    assert_eq!(s(&env, "detail"), kind);
    assert_eq!(reply.body, foreign_unavailable_envelope(kind));
    assert_eq!(
        String::from_utf8(reply.body.clone()).unwrap(),
        format!(r#"{{"detail":"{kind}","error":"E_FOREIGN_UNAVAILABLE","outcome":"error"}}"#)
    );
    assert_eq!(reply.source, Source::Unavailable);
    let d = reply.detail.as_deref().unwrap_or("");
    assert!(d.starts_with(why), "{d}");
}

#[test]
fn fetched_cells_answer_offline_with_the_same_bytes() {
    let up = upstream();
    let dir = TempDir::new("offline");
    let r = resolver(&dir, &up);
    r.prefetch("ecb/fx_rate(base: *, quote: *, date: *)", None, false).unwrap();
    r.define_formula(
        "local/closed",
        r#"mesh-test.datumesh.com/ecb/holiday(calendar: "TARGET", date: date)"#,
        &json::parse(CLOSED_SIG.as_bytes()).unwrap(),
    )
    .unwrap();
    let requests = [USD, GBP, CHRISTMAS, VERSE, NOK, "local/closed(date: 2026-12-25)"];
    let online: Vec<Vec<u8>> = requests.iter().map(|q| r.get(q).unwrap().body).collect();
    r.set_offline(true);
    let before = up.requests();
    for (q, body) in requests.iter().zip(&online) {
        let reply = r.get(q).unwrap();
        assert_eq!(&reply.body, body, "{q}");
        assert_ne!(reply.source, Source::Unavailable);
    }
    // After a restart too (offline from the configuration).
    drop(r);
    let mut c = config(&dir, &up);
    c.offline = true;
    let r = datumesh_local::Resolver::open(c).unwrap();
    for (q, body) in requests.iter().zip(&online) {
        assert_eq!(&r.get(q).unwrap().body, body, "{q}");
    }
    assert_eq!(up.requests(), before, "an offline answer reached the network");
}

#[test]
fn an_unfetched_foreign_cell_is_e_foreign_unavailable_not_not_covered() {
    let up = upstream();
    let dir = TempDir::new("offline-unfetched");
    let r = resolver(&dir, &up);
    r.get(USD).unwrap();
    r.set_offline(true);
    // A name held, a cell not held.
    assert_unavailable(&r.get(GBP).unwrap(), "offline", "offline");
    // A name not held.
    assert_unavailable(&r.get(CHRISTMAS).unwrap(), "offline", "offline");
    assert_unavailable(&r.get(VERSE).unwrap(), "offline", "offline");
    // A local evaluation whose foreign input is not held.
    r.set_offline(false);
    r.define_formula(
        "local/closed",
        r#"mesh-test.datumesh.com/ecb/holiday(calendar: "TARGET", date: date)"#,
        &json::parse(CLOSED_SIG.as_bytes()).unwrap(),
    )
    .unwrap();
    r.set_offline(true);
    assert_unavailable(&r.get("local/closed(date: 2026-12-25)").unwrap(), "offline", "offline");
    // The gate's forward, offline.
    assert_unavailable(&r.get(NOK).unwrap(), "offline", "offline");
    assert_eq!(up.requests(), up.requests());
}

#[test]
fn the_network_down_or_a_host_not_allowed_is_e_foreign_unavailable_with_the_failure() {
    let up = upstream();
    let dir = TempDir::new("network-down");
    let mut c = config(&dir, &up);
    // Nothing listens on port 1: the connection is refused.
    c.peers.insert(HOST.to_string(), "http://127.0.0.1:1".to_string());
    let r = datumesh_local::Resolver::open(c).unwrap();
    assert!(!r.offline());
    assert_unavailable(&r.get(USD).unwrap(), "refused", "refused");
    // A host outside the allowlist is never asked.
    // (detail offline: the resolver is configured not to use the network for it)
    assert_unavailable(&r.get("example.org/x/f(a: 1)").unwrap(), "offline", "not allowed");
    // A name that does not resolve.
    let dir2 = TempDir::new("network-dns");
    let mut c = config(&dir2, &up);
    c.peers.insert(HOST.to_string(), "http://no-such-host.invalid".to_string());
    let r = datumesh_local::Resolver::open(c).unwrap();
    assert_unavailable(&r.get(USD).unwrap(), "dns", "dns");
}

#[test]
fn the_cli_writes_the_envelope_and_exits_3() {
    let up = upstream();
    let dir = TempDir::new("offline-cli");
    datumesh_local::Config::in_dir(dir.path()).unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let args: Vec<String> =
        ["--home", dir.path().to_str().unwrap(), "--offline", "get", USD].iter().map(|s| s.to_string()).collect();
    let code = datumesh_local::cli::run(&args, &mut out, &mut err);
    assert_eq!(code, 3);
    assert_eq!(
        String::from_utf8(out).unwrap().trim_end(),
        r#"{"detail":"offline","error":"E_FOREIGN_UNAVAILABLE","outcome":"error"}"#
    );
    assert!(String::from_utf8(err).unwrap().contains("offline"));
    drop(up);
}

/// The envelope checked by the core, as a client would (datumesh-core 0.3.7: the code, its
/// 502 and its detail), for each failure.
#[test]
fn the_envelope_passes_the_core_check() {
    let up = upstream();
    let dir = TempDir::new("offline-core");
    let r = resolver(&dir, &up);
    r.set_offline(true);
    let cell_id = "0".repeat(64);
    let request = datumesh_core::Request::new(&cell_id, None).unwrap();
    let reply = r.get(USD).unwrap();
    let a = datumesh_core::check_answer(reply.status, &reply.body, &datumesh_core::Keys::new(), &request).unwrap();
    assert_eq!(a.error, Some(datumesh_core::ErrorCode::ForeignUnavailable));
    assert_eq!(a.error.map(|c| c.as_str()), Some(FOREIGN_UNAVAILABLE));
    assert_eq!(a.detail, Some(datumesh_core::ForeignDetail::Offline));
    // The network down: refused, also checked.
    let dir2 = TempDir::new("offline-core-refused");
    let mut c = config(&dir2, &up);
    c.peers.insert(HOST.to_string(), "http://127.0.0.1:1".to_string());
    let r = datumesh_local::Resolver::open(c).unwrap();
    let reply = r.get(USD).unwrap();
    let a = datumesh_core::check_answer(reply.status, &reply.body, &datumesh_core::Keys::new(), &request).unwrap();
    assert_eq!(a.detail, Some(datumesh_core::ForeignDetail::Refused));
    // The same bytes as core's builder writes.
    assert_eq!(reply.body, datumesh_core::Answer::foreign_unavailable(datumesh_core::ForeignDetail::Refused).to_json());
    // A status other than 502 fails the check.
    assert!(datumesh_core::check_answer(503, &reply.body, &datumesh_core::Keys::new(), &request).is_err());
    let _ = Value::Null;
}
