//! datumesh-local 0.5.3: a bundle carries the manifest of every slice its receipts read.
//!
//! datumesh-store 0.6.5 keeps a member-set row for a slice read, not the manifest
//! (CONTRACT-guard-patch G5/G10). The export took only held manifests, so on the new store a bundle
//! whose answer reads a slice left out the manifest and the members' receipts, and the slice could
//! not be checked on the importing machine. The export now rebuilds a manifest that is not held, as of
//! the asof of the receipt that cites it, so the bytes are those a stored manifest would hold.

mod common;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_core::{Manifest, Receipt};

const MEMBERS: usize = 150;
const CELLS_TX: &str = "2026-11-15T11:00:00.000000Z";
const T: &str = "2026-11-15T12:00:00.000000Z";
const LEVEL_SIG: &str = r#"{"args": {"day": {"type": "date", "valid": true}}, "order": ["day"], "returns": {"type": "number", "unit": "percent"}}"#;
const AVG_SIG: &str = r#"{"args": {}, "order": [], "returns": {"type": "number", "unit": "percent"}}"#;
/// The exporting machine is the laptop (`LAPTOP`): a machine never trusts a key of its own host.
const AVG_SRC: &str = "mean(laptop.invalid/level(day: *))";
const AVG: &str = "local/avg()";

/// The date `n` days after 2024-01-01.
fn day(n: usize) -> String {
    let z = 19_723 + n as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn level_line(n: usize) -> String {
    format!(
        r#"{{"cell": {{"fn": "local/level", "args": {{"day": {{"type": "date", "value": "{}"}}}}}}, "value": {{"type": "number", "value": "{}"}}, "tx": "{CELLS_TX}", "asof": "{CELLS_TX}", "licence": "CC0-1.0", "summary": "A test cell."}}"#,
        day(n),
        1 + n % 7
    )
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    match v.get(k) {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

/// The laptop: a machine that holds `MEMBERS` stored cells and a formula over their slice, answered once (a current
/// read, so its receipt is held); its bundle, the answer's receipt and the machine's key id.
fn exported(up: &Upstream) -> (Vec<u8>, Receipt, String) {
    let dir = TempDir::new("l053-bundle-a");
    let clock = ManualClock::new("2026-11-15T10:00:00.000000Z");
    let a = Arc::new(laptop_at(&dir, up, &clock));
    a.define_stored("local/level", &json::parse(LEVEL_SIG.as_bytes()).unwrap()).unwrap();
    let tmp = TempDir::new("l053-bundle-cells");
    let lines: Vec<String> = (0..MEMBERS).map(level_line).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    a.publish(&jsonl(&tmp, "level.jsonl", &refs)).unwrap();
    clock.set(T);
    a.define_formula("local/avg", AVG_SRC, &json::parse(AVG_SIG.as_bytes()).unwrap()).unwrap();
    let got = a.get(AVG).unwrap();
    assert_eq!(got.status, 200, "{}", String::from_utf8_lossy(&got.body));
    let rc = Receipt::from_value(&receipt(&got.body)).unwrap();
    (a.export().unwrap(), rc, a.key_id().to_string())
}

#[test]
fn a_bundle_carries_the_manifest_of_a_slice_its_answer_reads_and_the_members() {
    let up = upstream();
    let (bundle, rc, _) = exported(&up);
    let entry = rc.inputs().iter().find(|i| i.starts_with("slice:")).cloned();
    let entry = entry.unwrap_or_else(|| panic!("the answer reads a slice: {:?}", rc.inputs()));
    let v = json::parse(&bundle).unwrap();
    let manifests: Vec<Manifest> =
        strings(&v, "manifests").iter().map(|t| Manifest::from_json(t.as_bytes()).unwrap()).collect();
    let m = manifests.iter().find(|m| m.entry() == entry).unwrap_or_else(|| {
        panic!(
            "the manifest of {entry} is in the bundle: {:?}",
            manifests.iter().map(Manifest::entry).collect::<Vec<_>>()
        )
    });
    assert_eq!(m.members().len(), MEMBERS);
    // The members' receipts travel with it.
    let receipts: std::collections::BTreeSet<String> =
        strings(&v, "receipts").iter().map(|t| Receipt::from_json(t.as_bytes()).unwrap().hash().to_string()).collect();
    for h in m.members() {
        assert!(receipts.contains(h.as_str()), "the member receipt {h} is in the bundle");
    }
}

#[test]
fn the_imported_slice_manifest_is_held_and_its_members_are_held() {
    let up = upstream();
    let (bundle, rc, key_id) = exported(&up);
    let entry = rc.inputs().iter().find(|i| i.starts_with("slice:")).cloned().expect("a slice input");
    let dir = TempDir::new("l053-bundle-b");
    let b = resolver(&dir, &up);
    b.set_offline(true);
    let before = up.requests();
    let im = b.import(&bundle, std::slice::from_ref(&key_id)).unwrap();
    assert!(im.failed.is_empty(), "{:?}", im.failed);
    assert!(im.manifests >= 1, "a manifest imported");
    let held = b.store().manifest(&entry).unwrap_or_else(|| panic!("{entry} is held after the import"));
    let m = Manifest::from_json(&held).unwrap();
    for h in m.members() {
        assert!(b.store().receipt(h).is_some(), "the member receipt {h} is held");
    }
    // The answer's receipt verifies here (its signer is trusted).
    let bytes = b.store().receipt(rc.hash()).expect("the answer's receipt is imported").into_owned();
    let ok = b.verify(&bytes);
    assert_eq!(ok.get("ok"), Some(&Value::Bool(true)), "{}", String::from_utf8_lossy(&ok.to_jcs()));
    assert_eq!(up.requests(), before, "the import reached the network");
}
