//! datumesh-local 0.5.3, before its release: the `next` page pointer a slice answer carries (A3, `?page=<n>`) works for
//! a FOREIGN slice too. Written before the fix: today a get of `<foreign slice>?page=2` answers 400 `E_PARSE_URI`
//! before anything is fetched (seen against production's fake while writing `local_053_slice_get_bound.rs`).
//!
//! The fake remote is the test mesh of `tests/common` (datumesh-http over its own store, signing with its own test
//! key), not production's fake: production's fake can only replay production's recorded receipts, and a slice of more
//! than one 100-member page cannot be made of them without production's key (its recorded Bank Rate window is 59
//! members). The test mesh's `holiday(calendar: "TARGET", date: *)` has 365 members, so four pages of 100 (the
//! answer's page, `PAGE` in `src/calc.rs`): 100, 100, 100 and 65. The mesh serves its own slice pages of 50, so the
//! local pages are not the mesh's.
//!
//! What is asked, through the command (`datumesh get`) and the tool (`get`, both profiles):
//!
//! * the first answer's `next` names the slice and page 2 (`next.request`, `next.page` 2, `next.of` 4);
//! * following `next.request` answers page 2: 100 members, each a cell of the slice, in key order, every one after
//!   page 1's last, none of page 1's; the first member of page 2 is the 101st date of the year, 2026-04-11;
//! * each member's `receipt_hash` is the receipt the cell's own get answers (checked for page 2's first and last);
//! * following `next` to the end gives every member once (365 in all) and the last page (65 members) has no `next`.
//!
//! Nothing reaches the network.

#![cfg(unix)]

mod common;
#[path = "support/tools.rs"]
mod tools;

use std::collections::BTreeSet;
use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::{McpServer, Profile};
use datumesh_local::Resolver;
use tools::{arr, is_error, jcs, q, structured, tool};

const SLICE: &str = r#"mesh-test.datumesh.com/ecb/holiday(calendar: "TARGET", date: *)"#;
const MEMBERS: usize = 365;
const PAGE: usize = 100;
const PAGES: usize = 4;
/// The 101st day of 2026: page 2's first member.
const PAGE2_FIRST: &str = "2026-04-11";
/// The mesh's own slice page.
const MESH_PAGE: usize = 50;

fn get(srv: &McpServer, request: &str) -> Value {
    tool(srv, "get", &format!(r#"{{"request":{}}}"#, q(request)))
}

/// A slice page's member dates, in order.
fn dates(page: &Value) -> Vec<String> {
    arr(page, "members")
        .iter()
        .map(|m| {
            let call = m.get("call").and_then(Value::as_str).unwrap_or_else(|| panic!("a call: {}", jcs(m)));
            assert!(
                call.starts_with("mesh-test.datumesh.com/ecb/holiday(") && call.contains("\"TARGET\""),
                "a cell of the slice: {call}"
            );
            let at = call.find("date: ").unwrap_or_else(|| panic!("a date: {call}")) + 6;
            call[at..at + 10].to_string()
        })
        .collect()
}

fn next_of(page: &Value) -> Option<&Value> {
    page.get("next")
}

fn number(v: Option<&Value>) -> Option<usize> {
    v.and_then(Value::as_number_text).and_then(|t| t.parse().ok())
}

/// Checks a whole walk of `next` from the first page: `pages` as they were answered, in order.
fn assert_walk(what: &str, pages: &[Value]) {
    assert_eq!(pages.len(), PAGES, "{what}: {PAGES} pages");
    let first = &pages[0];
    let next = next_of(first).unwrap_or_else(|| panic!("{what}: the first page has next: {}", jcs(first)));
    let request = next.get("request").and_then(Value::as_str).unwrap_or_default();
    assert!(
        request.contains("holiday") && request.contains("page=2"),
        "{what}: next names the slice and page 2: {request}"
    );
    assert_eq!(number(next.get("page")), Some(2), "{what}: {}", jcs(next));
    assert_eq!(number(next.get("of")), Some(PAGES), "{what}: {}", jcs(next));
    let p1 = dates(first);
    let p2 = dates(&pages[1]);
    assert_eq!(p1.len(), PAGE, "{what}");
    assert_eq!(p2.len(), PAGE, "{what}: page 2 has {PAGE} members");
    assert_eq!(p2[0], PAGE2_FIRST, "{what}: page 2 starts at the 101st date: {p2:?}");
    assert!(p2.windows(2).all(|w| w[0] < w[1]), "{what}: page 2 is in key order: {p2:?}");
    assert!(p2[0] > p1[PAGE - 1], "{what}: page 2 comes after page 1's last ({}): {p2:?}", p1[PAGE - 1]);
    let mut all = BTreeSet::new();
    for (i, page) in pages.iter().enumerate() {
        for d in dates(page) {
            assert!(all.insert(d.clone()), "{what}: {d} on page {} was on an earlier page", i + 1);
        }
        if i + 1 < PAGES {
            assert_eq!(number(next_of(page).and_then(|n| n.get("page"))), Some(i + 2), "{what}: {}", jcs(page));
        }
    }
    assert_eq!(all.len(), MEMBERS, "{what}: every member once");
    let last = &pages[PAGES - 1];
    assert_eq!(dates(last).len(), MEMBERS - (PAGES - 1) * PAGE, "{what}: the last page");
    assert!(next_of(last).is_none(), "{what}: the last page has no next: {}", jcs(last));
}

/// Page 2's first and last members carry the receipt their cells' own gets answer.
fn assert_hashes(what: &str, r: &Resolver, page2: &Value) {
    let members = arr(page2, "members");
    for m in [&members[0], &members[members.len() - 1]] {
        let call = m.get("call").and_then(Value::as_str).unwrap();
        let reply = r.get(call).unwrap();
        assert_eq!(reply.status, 200, "{what}: {call}: {}", String::from_utf8_lossy(&reply.body));
        let want = reply.receipt().unwrap().hash().to_string();
        assert_eq!(m.get("receipt_hash").and_then(Value::as_str), Some(want.as_str()), "{what}: {call}");
    }
}

#[test]
fn mcp_get_follows_next_through_a_foreign_slice_in_both_profiles() {
    for profile in [Profile::Full, Profile::Small] {
        let what = format!("{profile:?}");
        let up = upstream_with(MESH_PAGE);
        let dir = TempDir::new("l053-pages-foreign");
        let r = Arc::new(resolver(&dir, &up));
        let srv = McpServer::with_profile(r.clone(), profile);
        let mut pages = Vec::new();
        let mut v = get(&srv, SLICE);
        loop {
            assert!(!is_error(&v), "{what}: page {}: {}", pages.len() + 1, jcs(&v));
            let page = structured(&v).clone();
            assert!(page.get("members").is_some(), "{what}: page {} is a slice page: {}", pages.len() + 1, jcs(&v));
            let next = next_of(&page).and_then(|n| n.get("request")).and_then(Value::as_str).map(str::to_string);
            pages.push(page);
            match next {
                Some(request) if pages.len() < MEMBERS => v = get(&srv, &request),
                _ => break,
            }
        }
        if profile == Profile::Full {
            assert_walk(&what, &pages);
            assert_hashes(&what, &r, &pages[1]);
        } else {
            assert_small_walk(&pages);
        }
    }
}

/// The small profile pages by at most 15 members (local 0.5.5, item 27): `next` covers every member once, in key
/// order, and the last page has no `next`.
fn assert_small_walk(pages: &[Value]) {
    const SMALL: usize = 15;
    assert_eq!(pages.len(), MEMBERS.div_ceil(SMALL), "Small: pages of {SMALL}");
    let mut all = BTreeSet::new();
    let mut last = String::new();
    for (i, page) in pages.iter().enumerate() {
        let d = dates(page);
        assert!(!d.is_empty() && d.len() <= SMALL, "Small: page {} has {} members", i + 1, d.len());
        for x in d {
            assert!(x > last, "Small: key order across pages at {x}");
            last = x.clone();
            assert!(all.insert(x), "Small: a member on two pages");
        }
    }
    assert_eq!(all.len(), MEMBERS, "Small: every member once");
    assert!(next_of(&pages[pages.len() - 1]).is_none(), "Small: the last page has no next");
}

/// A home of 0.5.0's shape whose config reaches the test mesh (the command reads config.json).
fn home(up: &Upstream) -> TempDir {
    let dir = TempDir::new("l053-pages-foreign-cli");
    in_dir_050(dir.path()).unwrap();
    let path = dir.path().join("config.json");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"peers\": {}"), "{text}");
    std::fs::write(&path, text.replace("\"peers\": {}", &format!("\"peers\": {{\"{HOST}\": \"{}\"}}", up.origin)))
        .unwrap();
    dir
}

fn cli(dir: &TempDir, args: &[&str]) -> (i32, String, String) {
    let mut all = vec!["--home".to_string(), dir.path().to_string_lossy().into_owned()];
    all.extend(args.iter().map(|a| a.to_string()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = datumesh_local::cli::run(&all, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

#[test]
fn cli_get_follows_next_through_a_foreign_slice() {
    let up = upstream_with(MESH_PAGE);
    let dir = home(&up);
    let mut pages = Vec::new();
    let mut request = SLICE.to_string();
    loop {
        let (code, out, err) = cli(&dir, &["get", &request]);
        assert_eq!(code, 0, "page {}: {request}: {err}\n{out}", pages.len() + 1);
        let page = json::parse(out.trim_end().as_bytes()).unwrap_or_else(|e| panic!("{out}: {e:?}"));
        let next = next_of(&page).and_then(|n| n.get("request")).and_then(Value::as_str).map(str::to_string);
        pages.push(page);
        match next {
            Some(n) if pages.len() < PAGES + 2 => request = n,
            _ => break,
        }
    }
    assert_walk("cli", &pages);
    let r = Resolver::open(datumesh_local::Config::in_dir(dir.path()).unwrap()).unwrap();
    assert_hashes("cli", &r, &pages[1]);
}
