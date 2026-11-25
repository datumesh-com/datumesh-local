//! datumesh-local 0.5.2, item 36: what the mesh holds (the catalogue summary's `held` sentence) is
//! carried once in the tool surface, in the instructions only, not appended to the `find` and `get`
//! descriptions as well (0.5.1 carries it three times). Both profiles.

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
use tools::{jcs, result, rpc_on};

fn production_held() -> String {
    let text = std::fs::read_to_string(data("production/routes.jsonl")).unwrap();
    for line in text.lines() {
        let v = json::parse(line.as_bytes()).unwrap();
        if v.get("path").and_then(Value::as_str) == Some("/find?summary=1") {
            let body = json::parse(v.get("body").and_then(Value::as_str).unwrap().as_bytes()).unwrap();
            return s(&body, "held");
        }
    }
    panic!("no recorded summary")
}

/// The instructions and every tool description of a server, as one text per part.
fn surface_parts(srv: &McpServer) -> Vec<(String, String)> {
    let discover = rpc_on(srv, Transport::Stdio, "server/discover", "");
    let mut parts = vec![("instructions".to_string(), s(result(&discover), "instructions"))];
    let list = rpc_on(srv, Transport::Stdio, "tools/list", "");
    let Some(Value::Array(tools)) = result(&list).get("tools") else { panic!("tools: {}", jcs(&list)) };
    for t in tools {
        parts.push((s(t, "name"), s(t, "description")));
    }
    parts
}

#[test]
fn the_held_sentence_is_carried_once_in_the_instructions_in_both_profiles() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    let held = production_held();
    assert!(!held.is_empty());
    for profile in [Profile::Full, Profile::Small] {
        let r = Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap());
        let parts = surface_parts(&McpServer::with_profile(r, profile));
        let carriers: Vec<&str> = parts
            .iter()
            .flat_map(|(name, text)| std::iter::repeat_n(name.as_str(), text.matches(&held).count()))
            .collect();
        assert_eq!(carriers, ["instructions"], "{profile:?}: once, in the instructions only");
    }
}
