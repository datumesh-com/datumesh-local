//! CONTRACT-build2-part2-read R5, local side: the rooting export bundle carries a `catalog`
//! section (the publisher's descriptions travel with the cells).
//!
//! Specified shape: the bundle's top-level `catalog` is an object keyed by the ROOTED full name
//! (`<namespace>/<name>`) of each rooted function, whose entry is
//! `{"label": <non-empty text, at most 120 bytes>, "description": <non-empty, at most 1024 bytes>,
//!   "args": {<arg>: <one line of text>}, "examples": [{"call", "expected"}],
//!   "dictionary": {<key>: [{"value", "label", "aliases"?}]}}`.
//! `args` has a line for each argument of the signature; `examples` has one entry for each
//! rooted stored cell (its rooted call and its typed value, e.g. `{"type":"number","value":"10"}`);
//! `dictionary` is present (it may be empty). Local descriptors carry no description (a
//! `description` member is E_SCHEMA), so the export makes one from the name and kind. The catalogue is part of the exported document, so the mesh's manifest
//! signature covers it (http 0.4.0 checks that); here it is only that it is in the bundle.
//! Every rooted function has an entry, with a label made from its name.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::RootPlan;

const T: &str = "2026-11-15T12:00:00.000000Z";
const CELLS_TX: &str = "2026-11-15T11:00:00.000000Z";
const NS: &str = "mesh-test.datumesh.com/scratch_catalog";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_rooting_export_carries_the_catalog_section() {
    let up = upstream();
    let dir = TempDir::new("catalog");
    let tmp = TempDir::new("catalog-tmp");
    let clock = ManualClock::new("2026-11-15T10:00:00.000000Z");
    let r = resolver_at(&dir, &up, &clock);
    // (A descriptor has no `description` member in 0.3.x: E_SCHEMA. The export makes the
    // entry's description itself, e.g. from the name and kind, when the machine has none.)
    r.define_stored(
        "local/level",
        &json::parse(
            br#"{"args":{"day":{"type":"date","valid":true}},"order":["day"],"returns":{"type":"number","unit":"1"}}"#,
        )
        .unwrap(),
    )
    .unwrap();
    r.define_stored(
        "local/bare",
        &json::parse(
            br#"{"args":{"day":{"type":"date","valid":true}},"order":["day"],"returns":{"type":"number","unit":"1"}}"#,
        )
        .unwrap(),
    )
    .unwrap();
    let cell = format!(
        r#"{{"cell": {{"fn": "local/level", "args": {{"day": {{"type": "date", "value": "2026-11-01"}}}}}}, "value": {{"type": "number", "value": "10"}}, "tx": "{CELLS_TX}", "asof": "{CELLS_TX}", "licence": "CC0-1.0", "summary": "A test cell."}}"#
    );
    r.publish(&jsonl(&tmp, "level.jsonl", &[&cell])).unwrap();
    clock.set(T);
    let key = tmp.path().join("namespace.key");
    std::fs::write(&key, format!("{}\n", hex(&[42u8; 32]))).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let export = tmp.path().join("export.json");
    r.root(&RootPlan {
        namespace: NS.to_string(),
        key_path: key,
        names: vec!["local/level".into(), "local/bare".into()],
        export: Some(export.clone()),
    })
    .unwrap();
    let _ = Arc::new(());
    let b = json::parse(&std::fs::read(&export).unwrap()).unwrap();
    let catalog = b.get("catalog").unwrap_or_else(|| panic!("a catalog section in the bundle"));
    // http 0.4.0's bundle member (signed with the manifest): an array of entries, each with its
    // rooted `name` (bundle::build_with_catalog sorts them by name).
    let Value::Array(entries) = catalog else { panic!("catalog is an array") };
    let mut names: Vec<String> = entries.iter().map(|e| s(e, "name")).collect();
    names.sort();
    assert_eq!(names, [format!("{NS}/bare"), format!("{NS}/level")], "one entry per rooted name");
    let entry = |n: &str| entries.iter().find(|e| s(e, "name") == n).unwrap_or_else(|| panic!("an entry of {n}"));
    let level = entry(&format!("{NS}/level"));
    let description = s(level, "description");
    assert!(!description.is_empty() && description.len() <= 1024, "{description:?}");
    let label = s(level, "label");
    assert!(!label.is_empty() && label.len() <= 120, "{label:?}");
    let Some(Value::Object(args)) = level.get("args") else {
        panic!("args: {}", String::from_utf8_lossy(&level.to_jcs()))
    };
    assert!(args.iter().any(|(k, v)| k == "day" && v.as_str().is_some_and(|t| !t.is_empty() && !t.contains('\n'))));
    let Some(Value::Array(examples)) = level.get("examples") else { panic!("examples") };
    let call = format!("{NS}/level(day: 2026-11-01)");
    let ex = examples.iter().find(|e| s(e, "call") == call).unwrap_or_else(|| panic!("an example of {call}"));
    assert_eq!(
        ex.get("expected").map(|v| (s(v, "type"), s(v, "value"))),
        Some(("number".to_string(), "10".to_string()))
    );
    assert!(matches!(level.get("dictionary"), Some(Value::Object(_))));
    let bare = entry(&format!("{NS}/bare"));
    assert!(!s(bare, "label").is_empty(), "a label made from the name");
}
