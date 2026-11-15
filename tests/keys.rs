//! CONTRACT-1.8b L4: machine key rotation and trusted machines (clock-controlled resolvers).

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_core::{ReceiptBuilder, SigningKey};
use datumesh_local::{Error, Resolver};

const T0: &str = "2026-11-15T10:00:00.000000Z";
const T1: &str = "2026-11-15T11:00:00.000000Z";
const T_1030: &str = "2026-11-15T10:30:00.000000Z";
const T_1040: &str = "2026-11-15T10:40:00.000000Z";
const T_1130: &str = "2026-11-15T11:30:00.000000Z";

const ONE_SIG: &str = r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": {"type": "number", "unit": {"of": "USD", "per": "EUR"}}}"#;
const DAY_SIG: &str = r#"{"args": {"day": {"type": "date", "valid": true}}, "order": ["day"], "returns": {"type": "number", "unit": "1"}}"#;
const ONE_SRC: &str = r#"mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", quote: "USD", date: date) * 1"#;

fn sig(text: &str) -> Value {
    json::parse(text.as_bytes()).unwrap()
}

/// Machine A at T0 with `local/one` defined and R1 got (signed by K1).
struct A {
    _up: Upstream,
    _dir: TempDir,
    clock: Arc<ManualClock>,
    r: Resolver,
    k1: String,
    r1: Vec<u8>,
}

fn machine_a() -> A {
    let up = upstream();
    let dir = TempDir::new("keys-a");
    let clock = ManualClock::new(T0);
    let r = resolver_at(&dir, &up, &clock);
    let k1 = r.key_id().to_string();
    r.define_formula("local/one", ONE_SRC, &sig(ONE_SIG)).unwrap();
    let a = r.get("local/one(date: 2026-10-30)").unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    let rc = receipt(&a.body);
    assert!(s(&rc, "computed_by").ends_with(&format!("#{k1}")));
    A { _up: up, _dir: dir, clock, r, k1, r1: rc.to_jcs() }
}

fn retired_path(r: &Resolver, k1: &str) -> PathBuf {
    let mut p = r.config().key_file.clone().into_os_string();
    p.push(format!(".retired-{k1}"));
    PathBuf::from(p)
}

#[test]
fn rotation_gives_a_new_key_and_keeps_the_old_one_in_the_document() {
    let a = machine_a();
    a.clock.set(T1);
    let rot = a.r.rotate_key().unwrap();
    assert_eq!(rot.old_key_id, a.k1);
    assert_ne!(rot.new_key_id, a.k1);
    assert_eq!(rot.at, T1);
    let k2 = rot.new_key_id.clone();
    assert_eq!(a.r.key_id(), k2);
    let entries = key_entries_of(&a.r.key_document());
    assert_eq!(entries.len(), 2);
    let of = |id: &str| entries.iter().find(|e| s(e, "id") == id).unwrap_or_else(|| panic!("no key {id}"));
    assert_eq!(s(of(&a.k1), "valid_to"), T1);
    assert_eq!(s(of(&k2), "valid_from"), T1);
    assert!(of(&k2).get("valid_to").is_none());
    let retired = retired_path(&a.r, &a.k1);
    assert!(retired.exists(), "{}", retired.display());
    assert_eq!(mode_of(&retired), 0o600);

    // A new local cell is signed by K2.
    let b = a.r.get("local/one(date: 2026-10-29)").unwrap();
    assert_eq!(b.status, 200, "{}", String::from_utf8_lossy(&b.body));
    assert!(s(&receipt(&b.body), "computed_by").ends_with(&format!("#{k2}")));
    // R1 verifies offline.
    a.r.set_offline(true);
    let v = a.r.verify(&a.r1);
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", String::from_utf8_lossy(&v.to_jcs()));
}

#[test]
fn a_retired_key_after_its_end_is_not_valid() {
    let a = machine_a();
    a.clock.set(T1);
    a.r.rotate_key().unwrap();
    let text = std::fs::read_to_string(retired_path(&a.r, &a.k1)).unwrap();
    let old = SigningKey::from_seed(&seed_of(text.lines().next().unwrap().trim()));
    assert_eq!(old.key_id(), a.k1);
    let r1 = datumesh_core::Receipt::from_json(&a.r1).unwrap();
    let late = ReceiptBuilder::new()
        .cell(r1.cell().clone())
        .value(r1.value().clone())
        .valid(r1.valid().clone())
        .asof(r1.asof())
        .tx(r1.tx())
        .issued("2026-11-15T11:00:00.000001Z")
        .publisher(r1.publisher())
        .signer_host("local.invalid")
        .inputs(r1.inputs().to_vec())
        .licence(r1.licence())
        .summary(r1.summary())
        .sign(&old)
        .unwrap();
    let v = a.r.verify(&receipt_bytes(&late));
    assert_eq!(
        v.get("code").and_then(Value::as_str),
        Some("E_KEY_NOT_VALID"),
        "{}",
        String::from_utf8_lossy(&v.to_jcs())
    );
}

#[test]
fn rotation_at_the_instant_the_key_began_is_refused() {
    let a = machine_a();
    a.clock.set(T1);
    a.r.rotate_key().unwrap();
    assert!(a.r.rotate_key().is_err());
}

/// B (config host `laptop.invalid`) defines `local/b1` and publishes day 2026-11-01 (value 7)
/// at 10:30; A is at the same clock, opened at T0.
struct Two {
    _up: Upstream,
    _dir_a: TempDir,
    _dir_b: TempDir,
    clock: Arc<ManualClock>,
    a: Resolver,
    b: Resolver,
    bundle: Vec<u8>,
    dir_tmp: TempDir,
}

fn cell_line(day: &str, value: &str, tx: &str) -> String {
    format!(
        r#"{{"cell": {{"fn": "local/b1", "args": {{"day": {{"type": "date", "value": "{day}"}}}}}}, "value": {{"type": "number", "value": "{value}"}}, "tx": "{tx}", "asof": "{tx}", "licence": "CC0-1.0", "summary": "A test cell."}}"#
    )
}

fn two_machines() -> Two {
    let up = upstream();
    let (dir_a, dir_b) = (TempDir::new("keys-two-a"), TempDir::new("keys-two-b"));
    let clock = ManualClock::new(T0);
    let a = resolver_at(&dir_a, &up, &clock);
    let b = laptop_at(&dir_b, &up, &clock);
    b.define_stored("local/b1", &sig(DAY_SIG)).unwrap();
    clock.set(T_1030);
    let tmp = TempDir::new("keys-two-tmp");
    let f = jsonl(&tmp, "b1.jsonl", &[&cell_line("2026-11-01", "7", T_1030)]);
    b.publish(&f).unwrap();
    let bundle = b.export().unwrap();
    Two { _up: up, _dir_a: dir_a, _dir_b: dir_b, clock, a, b, bundle, dir_tmp: tmp }
}

const B1: &str = "laptop.invalid/b1(day: 2026-11-01)";

#[test]
fn another_machine_s_receipts_are_quarantined_until_it_is_trusted() {
    let t = two_machines();
    let im = t.a.import(&t.bundle, &[]).unwrap();
    assert!(im.failed.is_empty(), "{:?}", im.failed);
    assert_eq!(t.a.quarantined().unwrap().len(), 1);
    match t.a.get(B1) {
        Ok(r) => assert_ne!(r.status, 200, "{}", String::from_utf8_lossy(&r.body)),
        Err(_) => {}
    }
}

#[test]
fn a_trusted_machine_s_receipts_answer_and_a_formula_may_read_them() {
    let t = two_machines();
    t.a.import(&t.bundle, &[]).unwrap();
    t.clock.set(T_1040);
    t.a.trust_machine(&t.b.key_document()).unwrap();
    let im = t.a.import(&t.bundle, &[]).unwrap();
    assert!(im.failed.is_empty(), "{:?}", im.failed);
    let r = t.a.get(B1).unwrap();
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.label.as_deref(), Some("trusted machine laptop.invalid"));
    // A's own key document is still exactly A's entries.
    let entries = key_entries_of(&t.a.key_document());
    assert_eq!(entries.len(), 1);
    assert_eq!(s(&entries[0], "id"), t.a.key_id());
    let trust = t.a.config().config_dir.join("trust.json");
    assert!(trust.exists());
    assert_eq!(mode_of(&trust), 0o600);

    // A formula over the trusted machine's cell.
    t.a.define_formula("local/uses_b", "laptop.invalid/b1(day: day) + 1", &sig(DAY_SIG)).unwrap();
    let u = t.a.get("local/uses_b(day: 2026-11-01)").unwrap();
    assert_eq!(u.status, 200, "{}", String::from_utf8_lossy(&u.body));
    assert_eq!(s(&receipt(&u.body), "value"), "8");
    let _ = &t.dir_tmp;
}

#[test]
fn a_revoked_machine_s_later_receipts_do_not_verify() {
    let t = two_machines();
    t.clock.set(T_1040);
    t.a.trust_machine(&t.b.key_document()).unwrap();
    t.a.import(&t.bundle, &[]).unwrap();
    let early = t.a.get(B1).unwrap();
    assert_eq!(early.status, 200, "{}", String::from_utf8_lossy(&early.body));
    t.clock.set(T1);
    t.a.revoke_machine("laptop.invalid").unwrap();
    // The receipt issued at 10:30 still verifies.
    let v = t.a.verify(&receipt(&early.body).to_jcs());
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", String::from_utf8_lossy(&v.to_jcs()));
    // B signs another at 11:30.
    t.clock.set(T_1130);
    let f = jsonl(&t.dir_tmp, "b2.jsonl", &[&cell_line("2026-11-02", "9", T_1130)]);
    t.b.publish(&f).unwrap();
    let later = t.b.get("local/b1(day: 2026-11-02)").unwrap();
    assert_eq!(later.status, 200, "{}", String::from_utf8_lossy(&later.body));
    let _ = t.a.import(&t.b.export().unwrap(), &[]);
    let v = t.a.verify(&receipt(&later.body).to_jcs());
    assert_eq!(
        v.get("code").and_then(Value::as_str),
        Some("E_KEY_NOT_VALID"),
        "{}",
        String::from_utf8_lossy(&v.to_jcs())
    );
    match t.a.get("laptop.invalid/b1(day: 2026-11-02)") {
        Ok(r) => assert_ne!(r.status, 200, "{}", String::from_utf8_lossy(&r.body)),
        Err(_) => {}
    }
}

#[test]
fn a_formula_over_an_untrusted_machine_is_refused_at_define() {
    let up = upstream();
    let dir = TempDir::new("keys-untrusted");
    let clock = ManualClock::new(T0);
    let a = resolver_at(&dir, &up, &clock);
    let e = a.define_formula("local/uses_c", "other.invalid/c(day: day) + 1", &sig(DAY_SIG)).unwrap_err();
    assert!(matches!(e, Error::UntrustedInput { .. }), "{e:?}");
}

#[test]
fn a_machine_cannot_trust_its_own_host() {
    let up = upstream();
    let dir = TempDir::new("keys-own");
    let clock = ManualClock::new(T0);
    let a = resolver_at(&dir, &up, &clock);
    assert!(a.trust_machine(&a.key_document()).is_err());
}

#[test]
fn a_replacement_key_document_must_keep_every_trusted_key() {
    let t = two_machines();
    t.clock.set(T_1040);
    t.a.trust_machine(&t.b.key_document()).unwrap();
    let trust = t.a.config().config_dir.join("trust.json");

    // B rotates (K_B1 -> K_B2) and publishes a cell signed by K_B2.
    t.clock.set(T1);
    let rot = t.b.rotate_key().unwrap();
    let (k_b1, k_b2) = (rot.old_key_id, rot.new_key_id);
    assert_ne!(k_b1, k_b2);
    t.clock.set(T_1130);
    let f = jsonl(&t.dir_tmp, "b2.jsonl", &[&cell_line("2026-11-02", "9", T_1130)]);
    t.b.publish(&f).unwrap();
    let later = t.b.get("local/b1(day: 2026-11-02)").unwrap();
    assert_eq!(later.status, 200, "{}", String::from_utf8_lossy(&later.body));
    assert!(s(&receipt(&later.body), "computed_by").ends_with(&format!("#{k_b2}")));

    // The new document lists K_B1 (retired, with valid_to) and K_B2: accepted.
    let new_doc = t.b.key_document();
    let entries = key_entries_of(&new_doc);
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|e| s(e, "id") == k_b1 && e.get("valid_to").is_some()));
    t.a.trust_machine(&new_doc).unwrap();
    let _ = t.a.import(&t.b.export().unwrap(), &[]);
    let v = t.a.verify(&receipt(&later.body).to_jcs());
    assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", String::from_utf8_lossy(&v.to_jcs()));

    // A document for the same host listing only K_B2 drops K_B1: refused, nothing changes.
    let before = std::fs::read(&trust).unwrap();
    let Value::Object(members) = json::parse(&new_doc).unwrap() else { panic!("key document is not an object") };
    let only_b2: Vec<Value> = entries.into_iter().filter(|e| s(e, "id") == k_b2).collect();
    assert_eq!(only_b2.len(), 1);
    let members: Vec<(String, Value)> = members
        .into_iter()
        .map(|(k, v)| if k == "keys" { (k, Value::Array(only_b2.clone())) } else { (k, v) })
        .collect();
    let e = t.a.trust_machine(&Value::Object(members).to_jcs()).unwrap_err();
    assert!(matches!(e, Error::Trust(..)), "{e:?}");
    assert_eq!(std::fs::read(&trust).unwrap(), before);
}
