//! datumesh-local 0.5.2, item 38 (amends 18): the small profile carries a short list of what the
//! mesh holds, by subject and publisher: one line per catalogue subject, with its label and the
//! publisher of its rank-1 source. The whole profile, instructions included, stays within the
//! character ceiling of item 18 (5,000 characters, about 1,500 tokens).
//!
//! The subjects are production's: `describe/datumesh.com/catalog` (SPEC 16.8, D122: each domain's
//! `top` subjects, each `{"subject", "label", "source"}`, the source a full name), captured read-only
//! into `tests/data/production/routes.jsonl` on 2026-11-25. Production's `/find?summary=1` (also
//! recorded) has classes, functions and `held`, but no subjects. The publisher of a source is its
//! namespace: the full name without its last segment (`datumesh.com/relay/ecb.europa.eu` for
//! `datumesh.com/relay/ecb.europa.eu/estr`).

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Profile, Transport};
use datumesh_local::{Config, Resolver};
use prod_mesh::*;
use tools::{result, rpc_on};

const SMALL_CHAR_CEILING: usize = 5_000;

/// Production's catalogue subjects: (label, publisher of the rank-1 source).
fn subjects() -> Vec<(String, String)> {
    let text = std::fs::read_to_string(data("production/routes.jsonl")).unwrap();
    for line in text.lines() {
        let v = json::parse(line.as_bytes()).unwrap();
        if v.get("path").and_then(Value::as_str) != Some("/describe/datumesh.com/catalog") {
            continue;
        }
        let body = json::parse(v.get("body").and_then(Value::as_str).unwrap().as_bytes()).unwrap();
        let Some(Value::Array(domains)) = body.get("domains") else { panic!("domains") };
        let mut out = Vec::new();
        for d in domains {
            let Some(Value::Array(top)) = d.get("top") else { panic!("top") };
            for t in top {
                let source = s(t, "source");
                let publisher = source.rsplit_once('/').unwrap().0.to_string();
                out.push((s(t, "label"), publisher));
            }
        }
        return out;
    }
    panic!("the catalogue root is not recorded")
}

fn small(dir: &TempDir) -> McpServer {
    let r = Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap());
    McpServer::with_profile(r, Profile::Small)
}

#[test]
fn the_small_profile_lists_each_subject_with_its_label_and_rank_1_publisher() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let text = s(result(&rpc_on(&small(&dir), Transport::Stdio, "server/discover", "")), "instructions");
    let all = subjects();
    assert_eq!(all.len(), 8, "production's catalogue has eight subjects");
    for (label, publisher) in &all {
        let lines: Vec<&str> = text.lines().filter(|l| l.contains(label.as_str())).collect();
        assert_eq!(lines.len(), 1, "one line for {label}: {text}");
        assert!(lines[0].contains(publisher.as_str()), "{label}: its rank-1 publisher {publisher}: {}", lines[0]);
    }
}

#[test]
fn the_small_profile_with_its_subjects_stays_within_the_ceiling() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let srv = small(&dir);
    let text = s(result(&rpc_on(&srv, Transport::Stdio, "server/discover", "")), "instructions");
    let list = rpc_on(&srv, Transport::Stdio, "tools/list", "");
    let tools = String::from_utf8(result(&list).get("tools").unwrap().to_jcs()).unwrap();
    let chars = tools.chars().count() + text.chars().count();
    assert!(chars <= SMALL_CHAR_CEILING, "{chars} characters, over {SMALL_CHAR_CEILING}:\n{text}\n{tools}");
    assert!(text.contains(&subjects()[0].0), "the subjects are in it: {text}");
}
