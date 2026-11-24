//! datumesh-local 0.5.2, item 30: `datumesh allow add|list|remove <domain>` manages the allowlist.
//!
//! * `allow list` prints the allowlist, one domain per line, in the order `config.json` has it.
//! * `allow add <domain>` puts the domain on the allowlist in `config.json`, keeping the file's `//`
//!   comment lines; adding a domain already there changes nothing (idempotent, exit 0).
//! * `allow remove <domain>` takes it off; removing a domain that is not there says so (naming the
//!   domain) and changes nothing.
//! * Only a rooted domain is taken: a URL, a path, an uppercase letter or a trailing dot is refused
//!   and the file is not touched.
//! * No MCP tool manages the allowlist, in either profile.

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Profile, Transport};
use datumesh_local::{Config, Resolver};
use prod_mesh::cli;

fn home() -> TempDir {
    let dir = TempDir::new("l052-allow");
    Config::in_dir(dir.path()).unwrap();
    dir
}

fn config_text(dir: &TempDir) -> String {
    std::fs::read_to_string(dir.path().join("config.json")).unwrap()
}

/// The file's `//` comment lines, in order.
fn comments(text: &str) -> Vec<String> {
    text.lines().filter(|l| l.trim_start().starts_with("//")).map(str::to_string).collect()
}

/// The allowlist as the configuration loads it.
fn allowlist(dir: &TempDir) -> Vec<String> {
    Config::in_dir(dir.path()).unwrap().allowlist
}

fn listed(dir: &TempDir) -> Vec<String> {
    let (code, out, err) = cli(dir, &["allow", "list"]);
    assert_eq!(code, 0, "{err}");
    out.lines().map(str::to_string).collect()
}

#[test]
fn list_prints_the_allowlist_one_domain_per_line() {
    let dir = home();
    let want = allowlist(&dir);
    assert!(!want.is_empty());
    assert_eq!(listed(&dir), want);
}

#[test]
fn add_writes_the_domain_and_keeps_the_comment_lines() {
    let dir = home();
    let (before, was) = (config_text(&dir), allowlist(&dir));
    let (code, out, err) = cli(&dir, &["allow", "add", "publisher.example"]);
    assert_eq!(code, 0, "{out}\n{err}");
    let after = config_text(&dir);
    assert_eq!(comments(&after), comments(&before), "every comment line is kept:\n{after}");
    let now = allowlist(&dir);
    assert!(now.contains(&"publisher.example".to_string()), "{now:?}");
    assert!(was.iter().all(|d| now.contains(d)), "the others stay: {now:?}");
    assert_eq!(now.len(), was.len() + 1);
    assert!(listed(&dir).contains(&"publisher.example".to_string()));
    // The rest of the configuration is as it was.
    let c = Config::in_dir(dir.path()).unwrap();
    assert_eq!(c.peers, Config::in_dir(home().path()).unwrap().peers);
}

#[test]
fn add_is_idempotent() {
    let dir = home();
    assert_eq!(cli(&dir, &["allow", "add", "publisher.example"]).0, 0);
    let (text, list) = (config_text(&dir), allowlist(&dir));
    let (code, out, err) = cli(&dir, &["allow", "add", "publisher.example"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(allowlist(&dir), list, "listed once");
    assert_eq!(allowlist(&dir).iter().filter(|d| *d == "publisher.example").count(), 1);
    assert_eq!(comments(&config_text(&dir)), comments(&text));
}

#[test]
fn remove_takes_the_domain_off_and_an_absent_one_is_said_so() {
    let dir = home();
    assert_eq!(cli(&dir, &["allow", "add", "publisher.example"]).0, 0);
    let before = config_text(&dir);
    let (code, out, err) = cli(&dir, &["allow", "remove", "publisher.example"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(!allowlist(&dir).contains(&"publisher.example".to_string()));
    assert_eq!(comments(&config_text(&dir)), comments(&before));
    // Again: it is not there, and the command says so.
    let text = config_text(&dir);
    let (code, out, err) = cli(&dir, &["allow", "remove", "publisher.example"]);
    assert_ne!(code, 2, "not a usage error: {err}");
    let said = format!("{out}{err}");
    assert!(said.contains("publisher.example") && said.contains("not"), "says it is not on the allowlist: {said}");
    assert_eq!(config_text(&dir), text, "nothing changed");
}

#[test]
fn only_a_rooted_domain_is_taken() {
    let dir = home();
    let text = config_text(&dir);
    for bad in ["https://publisher.example", "publisher.example/widgets", "Publisher.example", "publisher.example."] {
        let (code, out, err) = cli(&dir, &["allow", "add", bad]);
        assert_ne!(code, 0, "{bad}: refused: {out}");
        assert!(err.contains(bad), "{bad}: the refusal names it: {err}");
        assert_eq!(config_text(&dir), text, "{bad}: the file is not touched");
        let (code, _, err) = cli(&dir, &["allow", "remove", bad]);
        assert_ne!(code, 0, "{bad}: remove refuses it too: {err}");
        assert_eq!(config_text(&dir), text);
    }
}

#[test]
fn allow_needs_a_subcommand_and_is_in_the_usage() {
    let dir = home();
    assert_eq!(cli(&dir, &["allow"]).0, 2);
    assert_eq!(cli(&dir, &["allow", "grant", "publisher.example"]).0, 2);
    assert_eq!(cli(&dir, &["allow", "add"]).0, 2);
    let (_, help, _) = cli(&dir, &["--help"]);
    assert!(help.contains("allow <add|list|remove"), "{help}");
}

fn tool_names(srv: &McpServer) -> Vec<String> {
    let line = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{{{}}}}}"#, tools_meta());
    let v = json::parse(srv.handle(&line, Transport::Stdio).unwrap().as_bytes()).unwrap();
    match v.get("result").and_then(|r| r.get("tools")) {
        Some(Value::Array(a)) => a.iter().map(|t| s(t, "name")).collect(),
        other => panic!("tools: {other:?}"),
    }
}

fn tools_meta() -> &'static str {
    r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#
}

#[test]
fn no_mcp_tool_manages_the_allowlist() {
    // Offline, so the server's catalogue summary is not fetched from datumesh.com.
    let dir = home();
    let p = dir.path().join("config.json");
    std::fs::write(&p, config_text(&dir).replace("\"offline\": false", "\"offline\": true")).unwrap();
    assert!(Config::in_dir(dir.path()).unwrap().offline);
    for profile in [Profile::Full, Profile::Small] {
        let r = Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap());
        let names = tool_names(&McpServer::with_profile(r, profile));
        assert!(!names.is_empty());
        assert!(names.iter().all(|n| !n.contains("allow") && !n.contains("config")), "{profile:?}: {names:?}");
    }
}
