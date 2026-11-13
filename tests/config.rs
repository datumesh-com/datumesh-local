//! The configuration, the machine key, the alias map and the licence gate's decision.

mod common;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::{AliasMap, Config, LicenceGate, MachineKey, Resolver, LOCAL_HOST};

#[test]
fn first_run_installs_the_default_files() {
    let dir = TempDir::new("first-run");
    let c = Config::in_dir(dir.path()).unwrap();
    let cfg = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    // The comments the user asked for are in the installed file.
    assert!(cfg.contains("mesh-test.datumesh.com"), "{cfg}");
    assert_eq!(c.allowlist, vec!["datumesh.com".to_string(), "mesh-test.datumesh.com".to_string()]);
    assert_eq!(c.forward_mesh.as_deref(), Some("mesh-test.datumesh.com"));
    assert!(!c.offline);
    // Since 0.2.0 the gate is the engine's inheritance rule (spec-0.4.3 8.5): the installed
    // file has no open list, and says so.
    assert!(!cfg.contains("\"open_licences\""), "{cfg}");
    assert!(cfg.contains("8.5"), "{cfg}");
    assert!(c.open_licences.is_empty());
    // The alias map is a file, installed on first run: local/, scripture/ and ecb/ only, and tx.
    let aliases = json::parse(&std::fs::read(dir.path().join("aliases.json")).unwrap()).unwrap();
    let m = aliases.as_object().unwrap();
    let names: Vec<&str> = m.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, vec!["ecb/", "local/", "scripture/", "tx"]);
    assert_eq!(aliases.get("local/").and_then(Value::as_str), Some("local.invalid/"));
    assert_eq!(aliases.get("ecb/").and_then(Value::as_str), Some("mesh-test.datumesh.com/ecb/"));
    assert_eq!(aliases.get("scripture/").and_then(Value::as_str), Some("mesh-test.datumesh.com/scripture/"));
    // A second load reads the files as they are (an edit stands).
    std::fs::write(dir.path().join("config.json"), cfg.replace("\"offline\": false", "\"offline\": true")).unwrap();
    assert!(Config::in_dir(dir.path()).unwrap().offline);
    // A 0.1.0/0.1.1 file's open_licences is still accepted (deprecated, ignored by the gate).
    std::fs::write(
        dir.path().join("config.json"),
        cfg.replace("\"offline\": false", "\"offline\": false, \"open_licences\": [\"CC0-1.0\", \"MIT\"]"),
    )
    .unwrap();
    assert_eq!(Config::in_dir(dir.path()).unwrap().open_licences, vec!["CC0-1.0".to_string(), "MIT".to_string()]);
}

#[test]
fn a_comment_line_is_not_json() {
    let dir = TempDir::new("comments");
    Config::in_dir(dir.path()).unwrap();
    std::fs::write(dir.path().join("config.json"), "// only a comment\n{\"offline\": tru}\n").unwrap();
    assert!(Config::in_dir(dir.path()).is_err());
}

#[test]
fn the_machine_key_is_generated_once_kept_private_and_never_shown() {
    let dir = TempDir::new("key");
    let path = dir.path().join("machine.key");
    let k = MachineKey::load_or_create(&path).unwrap();
    assert_eq!(k.key_id().len(), 16);
    assert!(k.key_id().bytes().all(|b| b.is_ascii_hexdigit()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let again = MachineKey::load_or_create(&path).unwrap();
    assert_eq!(k.key_id(), again.key_id());
    // Debug shows the key id, never the seed.
    let seed = std::fs::read_to_string(&path).unwrap();
    let shown = format!("{k:?}");
    assert!(shown.contains(k.key_id()));
    assert!(!shown.contains(seed.trim()));
    // A key file others can read is refused.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(MachineKey::load_or_create(&path).is_err());
    }
}

#[test]
fn the_resolver_signs_as_local_invalid_with_its_machine_key() {
    let up = upstream();
    let dir = TempDir::new("keydoc");
    let r = resolver(&dir, &up);
    assert_eq!(LOCAL_HOST, "local.invalid");
    let doc = json::parse(&r.key_document()).unwrap();
    assert_eq!(doc.get("host").and_then(Value::as_str), Some("local.invalid"));
    let ids: Vec<String> = match doc.get("keys") {
        Some(Value::Array(ks)) => ks.iter().map(|k| s(k, "id")).collect(),
        _ => vec![],
    };
    assert_eq!(ids, vec![r.key_id().to_string()]);
    // The same key on the next open; a new key file is a new machine.
    let id = r.key_id().to_string();
    drop(r);
    assert_eq!(resolver(&dir, &up).key_id(), id);
    std::fs::remove_file(dir.path().join("machine.key")).unwrap();
    let dir2 = TempDir::new("keydoc-new");
    assert_ne!(resolver(&dir2, &up).key_id(), id);
}

#[test]
fn aliases_expand_prefixes_and_bare_names_only() {
    let m = AliasMap::from_json(
        br#"{"local/": "local.invalid/", "ecb/": "mesh-test.datumesh.com/ecb/", "usd": "mesh-test.datumesh.com/ecb/fx_rate", "tx": "2026-11-04T00:00:00.000000Z"}"#,
    )
    .unwrap();
    assert_eq!(m.tx(), "2026-11-04T00:00:00.000000Z");
    assert_eq!(m.expand("local/notes"), "local.invalid/notes");
    assert_eq!(m.expand("ecb/fx_rate"), "mesh-test.datumesh.com/ecb/fx_rate");
    assert_eq!(m.expand("usd"), "mesh-test.datumesh.com/ecb/fx_rate");
    // A full name (its first segment a DNS name) is never rewritten.
    assert_eq!(m.expand("example.org/ecb/x"), "example.org/ecb/x");
    assert_eq!(m.expand("other/x"), "other/x");
    // Malformed maps: no tx, tx not an instant, a value that is not a string or not a full name.
    assert!(AliasMap::from_json(br#"{"local/": "local.invalid/"}"#).is_err());
    assert!(AliasMap::from_json(br#"{"tx": "yesterday"}"#).is_err());
    assert!(AliasMap::from_json(br#"{"a/": 1, "tx": "2026-11-04T00:00:00.000000Z"}"#).is_err());
    assert!(AliasMap::from_json(br#"{"a/": "nodots/", "tx": "2026-11-04T00:00:00.000000Z"}"#).is_err());
}

#[test]
fn the_gate_is_the_engine_rule_and_never_signs_restricted_here() {
    use datumesh_core::{Declaration, Declarations, LicenceReason};
    use datumesh_local::gate::GateDecision;
    let g = LicenceGate::default();
    let f = "mesh-test.datumesh.com/ecb/fx_cross";
    let now = "2026-11-13T00:00:00.000000Z";
    let mut d = Declarations::new();
    d.insert(f, Declaration::new(RESTRICTED, "2026-09-01T00:00:00.000000Z", None).unwrap()).unwrap();
    // Open inputs (every tier of 8.5): computed here.
    for l in ["CC0-1.0", "PDDL-1.0", "MIT", "Apache-2.0", "CC-BY-4.0", "OGL-UK-3.0", "CC-BY-SA-4.0", "ODbL-1.0"] {
        assert_eq!(g.permits(f, l, HOST, Some(&d), now), GateDecision::Compute, "{l}");
    }
    // A restricted input the publisher declared for this function: forwarded (an unrooted
    // key never signs a restricted licence).
    assert_eq!(g.permits(f, RESTRICTED, HOST, Some(&d), now), GateDecision::Forward);
    // Not declared (another function, no document, another licence, not yet open): refused.
    assert_eq!(
        g.permits("mesh-test.datumesh.com/ecb/fx_cross_undeclared", RESTRICTED, HOST, Some(&d), now),
        GateDecision::Refuse(LicenceReason::NotDeclared)
    );
    assert_eq!(g.permits(f, RESTRICTED, HOST, None, now), GateDecision::Refuse(LicenceReason::NotDeclared));
    assert_eq!(g.permits(f, UXLC_LICENCE, HOST, Some(&d), now), GateDecision::Refuse(LicenceReason::NotDeclared));
    assert_eq!(
        g.permits(f, RESTRICTED, HOST, Some(&d), "2026-08-01T00:00:00.000000Z"),
        GateDecision::Refuse(LicenceReason::DeclarationNotOpen)
    );
    // The open list of 0.1.x is not a computation gate: an https:// licence is restricted.
    assert_eq!(
        g.permits("mesh-test.datumesh.com/scripture/x", UXLC_LICENCE, HOST, None, now),
        GateDecision::Refuse(LicenceReason::NotDeclared)
    );
    // Local inputs: computed here.
    assert_eq!(g.permits("local.invalid/f", "CC-BY-4.0", "local.invalid", None, now), GateDecision::Compute);
}

#[test]
fn a_short_name_resolves_to_its_full_name_in_the_receipt_and_the_map_reloads() {
    let up = upstream();
    let dir = TempDir::new("alias");
    let r = resolver(&dir, &up);
    let short = r.get(USD).unwrap();
    assert_eq!(short.status, 200, "{}", String::from_utf8_lossy(&short.body));
    let rc = receipt(&short.body);
    let cell = rc.get("cell").unwrap();
    assert_eq!(s(cell, "ns"), "mesh-test.datumesh.com/ecb");
    assert_eq!(s(cell, "fn"), "fx_rate");
    assert_eq!(r.get(USD_FULL).unwrap().body, short.body);
    // A new map file is picked up by a reload, not before.
    let fx = r#"fx/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#;
    assert_ne!(r.get(fx).unwrap().status, 200);
    std::fs::write(
        dir.path().join("aliases.json"),
        br#"{"fx/": "mesh-test.datumesh.com/ecb/", "local/": "local.invalid/", "tx": "2026-11-05T00:00:00.000000Z"}"#,
    )
    .unwrap();
    assert_ne!(r.get(fx).unwrap().status, 200);
    r.reload_aliases().unwrap();
    assert_eq!(r.aliases().tx(), "2026-11-05T00:00:00.000000Z");
    assert_eq!(r.get(fx).unwrap().body, short.body);
    drop(r);
    // And the next start reads the new file.
    let r = Resolver::open(config(&dir, &up)).unwrap();
    assert_eq!(r.get(fx).unwrap().body, short.body);
}
