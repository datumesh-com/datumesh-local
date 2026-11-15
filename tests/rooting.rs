//! CONTRACT-1.8b L5: rooting. A set of this machine's names is rooted under a namespace whose key
//! this machine holds: stored cells are re-signed under the namespace, formulas are rewritten, and
//! the result is a bundle 1 the mesh imports (`datumesh_http::bundle::import_signed`).
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_core::Receipt;
use datumesh_local::{Config, Error, Resolver, RootPlan};

const T: &str = "2026-11-15T12:00:00.000000Z";
const T_IMPORT: &str = "2026-11-15T12:00:01.000000Z";
const CELLS_TX: &str = "2026-11-15T11:00:00.000000Z";
const NS: &str = "mesh-test.datumesh.com/scratch_1_8b";
const LEVEL_SIG: &str = r#"{"args": {"day": {"type": "date", "valid": true}}, "order": ["day"], "returns": {"type": "number", "unit": "1"}}"#;
const RATE_SIG: &str = r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": {"type": "number", "unit": {"of": "USD", "per": "EUR"}}}"#;

fn sig(text: &str) -> Value {
    json::parse(text.as_bytes()).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct Setup {
    up: Upstream,
    dir: TempDir,
    tmp: TempDir,
    clock: Arc<ManualClock>,
    r: Resolver,
    key: PathBuf,
    level_hash: String,
}

fn write_key(path: &Path, seed: &[u8; 32], mode: u32) {
    std::fs::write(path, format!("{}\n", hex(seed))).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn setup() -> Setup {
    let up = upstream();
    let dir = TempDir::new("rooting");
    let tmp = TempDir::new("rooting-tmp");
    let clock = ManualClock::new(T);
    let r = resolver_at(&dir, &up, &clock);
    let level = r.define_stored("local/level", &sig(LEVEL_SIG)).unwrap();
    let line = |day: &str, v: &str| {
        format!(
            r#"{{"cell": {{"fn": "local/level", "args": {{"day": {{"type": "date", "value": "{day}"}}}}}}, "value": {{"type": "number", "value": "{v}"}}, "tx": "{CELLS_TX}", "asof": "{CELLS_TX}", "licence": "CC0-1.0", "summary": "A test cell."}}"#
        )
    };
    let f = jsonl(&tmp, "level.jsonl", &[&line("2026-11-01", "10"), &line("2026-11-02", "20")]);
    r.publish(&f).unwrap();
    r.define_formula("local/double", "local.invalid/level(day: day) * 2", &sig(LEVEL_SIG)).unwrap();
    r.define_formula(
        "local/rate_x",
        r#"mesh-test.datumesh.com/ecb/fx_rate(base: "EUR", quote: "USD", date: date) * 2"#,
        &sig(RATE_SIG),
    )
    .unwrap();
    let d = r.get("local/double(day: 2026-11-01)").unwrap();
    assert_eq!(d.status, 200, "{}", String::from_utf8_lossy(&d.body));
    assert_eq!(s(&receipt(&d.body), "value"), "20");
    let key = tmp.path().join("namespace.key");
    write_key(&key, &[42u8; 32], 0o600);
    Setup { up, dir, tmp, clock, r, key, level_hash: level.fn_hash }
}

fn plan(t: &Setup, names: &[&str], export: Option<PathBuf>) -> RootPlan {
    RootPlan {
        namespace: NS.to_string(),
        key_path: t.key.clone(),
        names: names.iter().map(|n| n.to_string()).collect(),
        export,
    }
}

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        other => panic!("{k}: {other:?}"),
    }
}

#[test]
fn a_dry_run_lists_the_plan_and_writes_nothing() {
    let t = setup();
    let rooted = t.r.root(&plan(&t, &["local/level", "local/double"], None)).unwrap();
    assert_eq!(rooted.namespace, NS);
    assert_eq!(rooted.key_id, upstream_key().key_id());
    assert_eq!(rooted.functions.len(), 2);
    let level = rooted.functions.iter().find(|f| f.to == format!("{NS}/level")).expect("level");
    assert_eq!(level.from_hash, level.to_hash);
    assert_eq!(level.to_hash, t.level_hash);
    assert_eq!(level.kind.to_string(), "stored");
    let double = rooted.functions.iter().find(|f| f.to == format!("{NS}/double")).expect("double");
    assert_ne!(double.to_hash, double.from_hash);
    assert_eq!(double.kind.to_string(), "formula");
    assert_eq!(rooted.cells.len(), 2);
    let mut calls: Vec<String> = rooted.cells.iter().map(|c| c.call.to_string()).collect();
    calls.sort();
    assert_eq!(calls, [format!("{NS}/level(day: 2026-11-01)"), format!("{NS}/level(day: 2026-11-02)")]);
    assert_eq!(rooted.computed, vec![format!("{NS}/double(day: 2026-11-01)")]);
    // Nothing was written.
    for name in ["level", "double"] {
        assert!(t.r.store().resolve_name(&format!("{NS}/{name}"), END).unwrap().is_none(), "{name} is bound");
    }
    assert_eq!(std::fs::read_dir(t.tmp.path()).unwrap().count(), 2, "files: level.jsonl and the key");
}

#[test]
fn a_formula_without_its_input_in_the_set_is_refused() {
    let t = setup();
    let e = t.r.root(&plan(&t, &["local/double"], None)).unwrap_err();
    assert!(matches!(e, Error::UnrootedInput { .. }), "{e:?}");
    assert_eq!(e.code(), Some("E_UNROOTED_INPUT"));
}

#[test]
fn a_key_not_in_the_namespace_s_key_document_is_refused() {
    let t = setup();
    let fresh = t.tmp.path().join("fresh.key");
    write_key(&fresh, &[7u8; 32], 0o600);
    let export = t.tmp.path().join("never.json");
    let mut p = plan(&t, &["local/level"], Some(export.clone()));
    p.key_path = fresh;
    match t.r.root(&p).unwrap_err() {
        Error::Rooting(m) => assert!(m.contains("is not in mesh-test.datumesh.com's key document"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(!export.exists());
}

#[test]
fn a_key_file_others_can_read_is_refused() {
    let t = setup();
    let open = t.tmp.path().join("open.key");
    write_key(&open, &[42u8; 32], 0o644);
    let export = t.tmp.path().join("never.json");
    let mut p = plan(&t, &["local/level"], Some(export.clone()));
    p.key_path = open;
    let e = t.r.root(&p).unwrap_err();
    assert!(matches!(e, Error::Key(_)), "{e:?}");
    assert!(!export.exists());
}

fn export_all(t: &Setup, name: &str) -> (PathBuf, Value) {
    let export = t.tmp.path().join(name);
    t.r.root(&plan(t, &["local/level", "local/double", "local/rate_x"], Some(export.clone()))).unwrap();
    let v = json::parse(&std::fs::read(&export).unwrap()).unwrap();
    (export, v)
}

#[test]
fn the_export_is_a_bundle_of_the_stored_cells_re_signed_by_the_namespace_key() {
    let t = setup();
    let (export, b) = export_all(&t, "export.json");
    assert_eq!(std::fs::metadata(&export).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(b.get("bundle").and_then(Value::as_number_text), Some("1"));
    assert_eq!(s(&b, "namespace"), NS);
    assert_eq!(arr(&b, "names").len(), 3);
    let receipts = arr(&b, "receipts");
    assert_eq!(receipts.len(), 2);
    let u = upstream_key();
    for rc in receipts {
        assert_eq!(s(rc, "tx"), T);
        assert_eq!(s(rc, "issued"), T);
        assert_eq!(s(rc, "computed_by"), format!("{HOST}#{}", u.key_id()));
        assert_eq!(rc.get("cell").map(|c| s(c, "fn_hash")), Some(t.level_hash.clone()), "only level's cells");
        let day = rc.get("cell").and_then(|c| c.get("args")).and_then(|a| a.get("day")).map(|d| s(d, "value")).unwrap();
        let local = t.r.get(&format!("local/level(day: {day})")).unwrap();
        let lr = receipt(&local.body);
        // Q7 a (pending the user): tx = asof = issued = T, since SPEC 8 needs tx <= asof <= issued.
        assert_eq!(s(rc, "asof"), T);
        assert_eq!(rc.get("value"), lr.get("value"));
        assert_eq!(rc.get("licence"), lr.get("licence"));
        assert_eq!(rc.get("valid"), lr.get("valid"));
    }
}

#[test]
fn the_bundle_imports_into_the_mesh_and_the_rooted_formulas_recompute() {
    let t = setup();
    let (_, b) = export_all(&t, "export.json");
    let bytes = std::fs::read(t.tmp.path().join("export.json")).unwrap();
    let c = ManualClock::new(T_IMPORT);
    let owned: Vec<String> = Vec::new();
    let im = datumesh_http::bundle::import_signed(&*t.up.store, HOST, &owned, &*c, &bytes).unwrap();
    assert_eq!(im.functions_bound, 3);
    assert_eq!(im.receipts, 2);
    t.up.mesh.clear_memo();
    t.clock.set("2026-11-15T12:00:02.000000Z");
    let rooted_level_1 = arr(&b, "receipts")
        .iter()
        .map(|rc| Receipt::from_value(rc).unwrap())
        .find(|rc| rc.value().lexical() == "10")
        .expect("the receipt of day 2026-11-01 (value 10)");
    let d = t.r.get(&format!("{NS}/double(day: 2026-11-01)")).unwrap();
    assert_eq!(d.status, 200, "{}", String::from_utf8_lossy(&d.body));
    let rc = receipt(&d.body);
    assert_eq!(s(&rc, "value"), "20");
    let inputs: Vec<&str> = arr(&rc, "inputs").iter().filter_map(Value::as_str).collect();
    assert!(inputs.contains(&rooted_level_1.hash()), "{inputs:?}");
    let local = t.r.get("local/double(day: 2026-11-01)").unwrap();
    assert_eq!(s(&rc, "value"), s(&receipt(&local.body), "value"));
    let x = t.r.get(&format!("{NS}/rate_x(date: 2026-10-30)")).unwrap();
    assert_eq!(x.status, 200, "{}", String::from_utf8_lossy(&x.body));
    let lx = t.r.get("local/rate_x(date: 2026-10-30)").unwrap();
    assert_eq!(s(&receipt(&x.body), "value"), "2.3234");
    assert_eq!(s(&receipt(&x.body), "value"), s(&receipt(&lx.body), "value"));
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            files_under(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn the_namespace_key_is_nowhere_but_its_file() {
    let t = setup();
    let (export, _) = export_all(&t, "export.json");
    // The CLI run: through `cli::run`, with captured writers, to a new export path.
    let cfg = t.dir.path().join("config.json");
    let text = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(&cfg, text.replace("\"peers\": {}", &format!("\"peers\": {{\"{HOST}\": \"{}\"}}", t.up.origin)))
        .unwrap();
    let cli_export = t.tmp.path().join("cli-export.json");
    let args: Vec<String> = [
        "--home",
        t.dir.path().to_str().unwrap(),
        "root",
        "--namespace",
        NS,
        "--key",
        t.key.to_str().unwrap(),
        "--export",
        cli_export.to_str().unwrap(),
        "local/level",
        "local/double",
        "local/rate_x",
    ]
    .iter()
    .map(|a| a.to_string())
    .collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&args, &mut out, &mut err);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
    assert!(cli_export.exists());

    let seed = [42u8; 32];
    let lower = hex(&seed);
    let needles: Vec<Vec<u8>> = vec![lower.clone().into_bytes(), lower.to_uppercase().into_bytes(), seed.to_vec()];
    let mut haystacks: Vec<(String, Vec<u8>)> = Vec::new();
    let mut files = Vec::new();
    files_under(&Config::in_dir(t.dir.path()).unwrap().data_dir, &mut files);
    for f in &files {
        haystacks.push((f.display().to_string(), std::fs::read(f).unwrap()));
    }
    haystacks.push(("export".into(), std::fs::read(&export).unwrap()));
    haystacks.push(("cli export".into(), std::fs::read(&cli_export).unwrap()));
    haystacks.push(("cli stdout".into(), out));
    haystacks.push(("cli stderr".into(), err));
    haystacks.push(("config.json".into(), std::fs::read(&cfg).unwrap()));
    for (name, bytes) in &haystacks {
        for n in &needles {
            assert!(!contains(bytes, n), "the namespace key's seed is in {name}");
        }
    }
    // The key's path is neither in config.json nor in the store files.
    let path = t.key.to_str().unwrap().as_bytes();
    for (name, bytes) in &haystacks {
        if name.as_str() == "config.json" || files.iter().any(|f| &f.display().to_string() == name) {
            assert!(!contains(bytes, path), "the key's path is in {name}");
        }
    }
}
