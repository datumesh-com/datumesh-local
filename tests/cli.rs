//! The `datumesh` command, in process ([`datumesh_local::cli::run`]).

mod common;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::Config;

fn home(up: &Upstream) -> TempDir {
    let dir = TempDir::new("cli");
    Config::in_dir(dir.path()).unwrap();
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("\"peers\": {}", &format!("\"peers\": {{\"{HOST}\": \"{}\"}}", up.origin)))
        .unwrap();
    dir
}

fn run(dir: &TempDir, args: &[&str]) -> (i32, String, String) {
    let mut all = vec!["--home".to_string(), dir.path().to_string_lossy().into_owned()];
    all.extend(args.iter().map(|a| a.to_string()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&all, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

#[test]
fn get_writes_the_envelope_and_labels_the_signer() {
    let up = upstream();
    let dir = home(&up);
    let (code, out, err) = run(&dir, &["get", USD]);
    assert_eq!(code, 0, "{err}");
    let env = json::parse(out.trim_end().as_bytes()).unwrap();
    assert_eq!(s(&env, "outcome"), "ok");
    assert!(err.contains("fetched"), "{err}");
    assert!(err.contains("mesh-test.datumesh.com#"), "{err}");
    // Offline, the same bytes from the local store.
    let (code, out2, err) = run(&dir, &["--offline", "get", USD]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out2, out);
    assert!(err.contains("local store"), "{err}");
    // A local evaluation is labelled.
    let sig = r#"{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": "boolean"}"#;
    let (code, out, err) = run(
        &dir,
        &[
            "define",
            "local/closed",
            "--formula",
            r#"mesh-test.datumesh.com/ecb/holiday(calendar: "TARGET", date: date)"#,
            "--signature",
            sig,
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("local.invalid/closed"));
    let (code, _, err) = run(&dir, &["get", "local/closed(date: 2026-12-25)"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("(local, verifies only on this machine)"), "{err}");
    // A cell with no value: exit 3.
    let (code, out, _) = run(&dir, &["get", r#"ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-01-01)"#]);
    assert_eq!(code, 3);
    assert_eq!(s(&json::parse(out.trim_end().as_bytes()).unwrap(), "outcome"), "not_covered");
}

#[test]
fn the_other_commands() {
    let up = upstream();
    let dir = home(&up);
    let (code, out, err) = run(&dir, &["prefetch", "ecb/fx_rate(base: *, quote: *, date: *)"]);
    assert_eq!(code, 0, "{err}");
    let p = json::parse(out.trim_end().as_bytes()).unwrap();
    assert_eq!(s(&p, "members"), "121");
    let (code, out, err) = run(&dir, &["refresh"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(s(&json::parse(out.trim_end().as_bytes()).unwrap(), "slices"), "1");
    let (code, out, _) = run(&dir, &["find", "fx_"]);
    assert_eq!(code, 0);
    assert!(out.contains("mesh-test.datumesh.com/ecb/fx_rate"), "{out}");
    let (code, out, _) = run(&dir, &["explain", USD]);
    assert_eq!(code, 0, "{out}");
    // The key: the machine id and the public key document, never the seed.
    let (code, out, _) = run(&dir, &["key"]);
    assert_eq!(code, 0);
    let seed = std::fs::read_to_string(dir.path().join("machine.key")).unwrap();
    let seed = seed.lines().next().unwrap();
    assert!(!out.contains(seed));
    let machine = s(&json::parse(out.trim_end().as_bytes()).unwrap(), "machine");
    assert_eq!(machine.len(), 16);
    // Verify an envelope's receipt.
    let (_, env, _) = run(&dir, &["get", USD]);
    let f = dir.path().join("usd.json");
    std::fs::write(&f, env).unwrap();
    let (code, out, _) = run(&dir, &["verify", f.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(json::parse(out.trim_end().as_bytes()).unwrap().get("ok"), Some(&Value::Bool(true)));
    // Export to a file, import into another home with --trust-key.
    let bundle = dir.path().join("bundle.json");
    let (code, _, err) = run(&dir, &["export", "--out", bundle.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let other = home(&up);
    let (code, out, err) = run(&other, &["--offline", "import", bundle.to_str().unwrap(), "--trust-key", &machine]);
    assert_eq!(code, 0, "{err}");
    let im = json::parse(out.trim_end().as_bytes()).unwrap();
    assert_eq!(im.get("trusted"), Some(&Value::Array(vec![Value::String(machine.clone())])));
    let (code, out2, _) = run(&other, &["--offline", "get", USD]);
    assert_eq!(code, 0);
    let (_, out1, _) = run(&dir, &["get", USD]);
    assert_eq!(out2, out1);
    // Publish under local/ only.
    let lines = dir.path().join("p.jsonl");
    std::fs::write(
        &lines,
        r#"{"function": "local/n", "bound": "2026-11-01T00:00:00.000000Z", "signature": {"args": {"k": {"type": "string", "valid": false}}, "order": ["k"], "returns": "string"}}"#,
    )
    .unwrap();
    let (code, out, err) = run(&dir, &["publish", lines.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(s(&json::parse(out.trim_end().as_bytes()).unwrap(), "functions"), "1");
}

#[test]
fn usage_errors_and_serve_elsewhere_than_localhost() {
    let up = upstream();
    let dir = home(&up);
    assert_eq!(run(&dir, &["frobnicate"]).0, 2);
    assert_eq!(run(&dir, &["get"]).0, 2);
    assert_eq!(run(&dir, &["get", "--nope", USD]).0, 2);
    let (code, _, err) = run(&dir, &["serve", "--listen", "0.0.0.0:7733"]);
    assert_eq!(code, 1);
    assert!(err.contains("127.0.0.1 only"), "{err}");
    let (code, out, _) = run(&dir, &["--version"]);
    assert_eq!(code, 0);
    assert!(out.starts_with(&format!("datumesh {}", env!("CARGO_PKG_VERSION"))));
}
