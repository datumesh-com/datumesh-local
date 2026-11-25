//! datumesh-local 0.5.3, the guard patch's A5 (NEXT-PATCH-QUEUE item 12): the first line of
//! `datumesh --help`, `usage: datumesh [--home <dir>] [--offline] <a|b|...> ...`, lists every
//! command the dispatcher takes. 0.5.2's line lacks whatif, mcp, licence, trust, root and allow,
//! which the lines under it describe. Written before the implementation.
//!
//! The dispatcher's commands are read from `src/cli.rs` too (each `"<name>" => {` arm of `run`'s
//! command match, and the `allow` command taken before it), so a command added later without the
//! usage line fails here.

mod common;

use common::*;

const REQUIRED: [&str; 20] = [
    "get", "calc", "define", "publish", "describe", "explain", "find", "prefetch", "whatif", "refresh", "export",
    "import", "verify", "serve", "mcp", "key", "licence", "trust", "root", "allow",
];

fn cli(dir: &TempDir, args: &[&str]) -> (i32, String, String) {
    let mut all = vec!["--home".to_string(), dir.path().to_string_lossy().into_owned()];
    all.extend(args.iter().map(|a| a.to_string()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&all, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

/// The commands in the first usage line's `<...>` list.
fn listed() -> Vec<String> {
    let dir = TempDir::new("l053-usage");
    let (code, help, err) = cli(&dir, &["--help"]);
    assert_eq!(code, 0, "{err}");
    let first = help.lines().next().unwrap_or_default().to_string();
    assert!(first.starts_with("usage: datumesh "), "the first line is the usage line: {first}");
    // The command list is the `<...>` that holds a `|` (not `<dir>`).
    let open = first
        .match_indices('<')
        .map(|(i, _)| i)
        .find(|&i| first[i..].split('>').next().is_some_and(|x| x.contains('|')));
    let start = open.unwrap_or_else(|| panic!("a <a|b|...> list of commands: {first}")) + 1;
    let end = first[start..].find('>').unwrap() + start;
    first[start..end].split('|').map(str::to_string).collect()
}

/// The commands `run` dispatches: its match arms `        "<name>" => {` and `cmd == "<name>"`.
fn dispatched() -> Vec<String> {
    let src = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli.rs")).unwrap();
    let mut out = Vec::new();
    for line in src.lines() {
        if let Some(rest) = line.strip_prefix("        \"") {
            if let Some((name, tail)) = rest.split_once('"') {
                if tail == " => {" && !name.is_empty() && name.bytes().all(|c| c.is_ascii_lowercase() || c == b'_') {
                    out.push(name.to_string());
                }
            }
        }
        if let Some(rest) = line.trim().strip_prefix("if cmd == \"") {
            if let Some((name, _)) = rest.split_once('"') {
                out.push(name.to_string());
            }
        }
    }
    out
}

#[test]
fn the_usage_line_lists_whatif_mcp_licence_trust_root_and_allow() {
    let got = listed();
    for c in ["whatif", "mcp", "licence", "trust", "root", "allow"] {
        assert!(got.iter().any(|g| g == c), "{c} in the usage line's list: {got:?}");
    }
}

#[test]
fn the_usage_line_lists_every_command() {
    let got = listed();
    for c in REQUIRED {
        assert!(got.iter().any(|g| g == c), "{c} in the usage line's list: {got:?}");
    }
    let mut sorted = got.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), got.len(), "no command twice: {got:?}");
}

#[test]
fn the_usage_line_lists_every_command_the_dispatcher_takes() {
    let d = dispatched();
    for c in REQUIRED {
        assert!(d.iter().any(|x| x == c), "the dispatcher was read: {c} not found in {d:?}");
    }
    let got = listed();
    for c in &d {
        assert!(got.iter().any(|g| g == c), "{c} is dispatched but not in the usage line: {got:?}");
    }
    for g in &got {
        assert!(d.iter().any(|c| c == g), "{g} is in the usage line but not dispatched: {d:?}");
    }
}
