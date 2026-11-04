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
    assert!(cfg.contains("open for delivery"), "{cfg}");
    assert!(cfg.contains("mesh-test.datumesh.com"), "{cfg}");
    assert!(cfg.contains("engine"), "{cfg}");
    assert_eq!(c.allowlist, vec!["datumesh.com".to_string(), "mesh-test.datumesh.com".to_string()]);
    assert_eq!(c.forward_mesh.as_deref(), Some("mesh-test.datumesh.com"));
    assert!(!c.offline);
    for l in [
        "CC0-1.0",
        "CC-BY-4.0",
        "CC-BY-SA-4.0",
        "ODbL-1.0",
        "OGL-UK-3.0",
        "PDDL-1.0",
        "Apache-2.0",
        "MIT",
        "https://tanach.us/License.html",
    ] {
        assert!(c.open_licences.iter().any(|x| x == l), "{l} is open by default");
    }
    assert_eq!(c.open_licences.len(), 9);
    assert!(!c.open_licences.iter().any(|x| x.contains("tanzil")));
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
fn the_gate_is_the_open_list_or_local() {
    let dir = TempDir::new("gate");
    let c = Config::in_dir(dir.path()).unwrap();
    let g = LicenceGate::new(&c.open_licences);
    assert!(g.permits("CC-BY-4.0", "mesh-test.datumesh.com"));
    assert!(g.permits("CC-BY-SA-4.0", "mesh-test.datumesh.com"));
    assert!(g.permits("https://tanach.us/License.html", "mesh-test.datumesh.com"));
    assert!(!g.permits("https://tanzil.net/docs/text_license", "mesh-test.datumesh.com"));
    assert!(!g.permits("https://mesh-test.datumesh.com/licences/restricted-test", "mesh-test.datumesh.com"));
    assert!(!g.permits("CC-BY-NC-4.0", "example.org"));
    // Local inputs pass whatever their licence.
    assert!(g.permits("https://mesh-test.datumesh.com/licences/restricted-test", "local.invalid"));
    assert!(g.permits("CC-BY-NC-4.0", "agent-7.local.invalid"));
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
