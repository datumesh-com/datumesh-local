//! datumesh-local 0.5.3, before its release (REPORT-c-codes-and-small-profile.md, "Two defects", 2): a `get` of a
//! foreign slice is bound by `slice_fetch_max` as an evaluation's slice reads are (0.5.2, item 12). Written before the
//! fix: today `get <fn>(date: *, ...)` prefetches every member with no bound (seen fetching 100,188 members of
//! production's `par_yield` against the bound of 50,000).
//!
//! The semantics are calc's (`local_052_slices.rs`, `Shared::charge` and `fetch_whole` in `src/fetch.rs`): the members
//! are counted page by page as they come; at the first page that takes the count over the bound the fetch stops, and
//! the read is **refused** with `E_SLICE_FETCH_LIMIT`, naming the slice, its size so far and the bound, with prefetch
//! as the way to hold it first. Nothing it fetched is kept (no member, and the slice is not held). A refusal, not a
//! partial answer: a partial page would read as the slice, and the bound exists so that a read never pulls more than
//! the machine was told to take. `prefetch` stays the unbounded way, so a slice held by prefetch is answered whatever
//! the bound, and a slice within the bound is answered as before.
//!
//! The fakes:
//!
//! * production's (`tests/support/prod_mesh.rs`): Bank Rate's recorded window, 59 members, served by `/slice` in pages
//!   of 20 (the fake's default page) and logged, so the pages a read asks for are counted;
//! * the test mesh of `tests/common` in pages of 10: `fx_rate(base: *, quote: *, date: *)`, 121 members over three
//!   open arguments (the shape of the report's `par_yield(date: *, tenor: *)`), its requests counted.
//!
//! Nothing reaches the network.

#![cfg(unix)]

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::sync::Arc;

use common::*;
use datumesh_core::json::Value;
use datumesh_local::mcp::{McpServer, Profile};
use datumesh_local::{Config, Resolver};
use prod_mesh::*;
use tools::{arr, is_error, items, jcs, q, structured, tool};

const BR_FULL: &str = "datumesh.com/relay/bankofengland.co.uk/bank_rate";
/// The Bank Rate slice the fake serves: 59 members.
const MEMBERS: usize = 59;
/// The production fake's `/slice` page (its default, as no `limit` is asked).
const PROD_PAGE: usize = 20;
/// The bound of most tests here: over it after the second page of 20.
const BOUND: usize = 30;

/// The test mesh's `fx_rate` slice: three open arguments, 121 members, pages of `FX_PAGE`.
const FX_MEMBERS: usize = 121;
const FX_PAGE: usize = 10;
const FX_BOUND: usize = 25;

fn bank_slice() -> String {
    format!("{BR_FULL}(date: *)")
}

/// Standalone integers in `text` (not part of a date, a hash or a larger word).
fn integers(text: &str) -> Vec<u64> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let before = if start > 0 { b[start - 1] } else { b' ' };
            let after = if i < b.len() { b[i] } else { b' ' };
            let part_of_word = |c: u8| c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.' || c == b':';
            if !part_of_word(before) && !part_of_word(after) {
                out.push(text[start..i].parse().unwrap());
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The most `/slice` pages a read of a slice in pages of `page` may ask for under `bound`: the pages up to the bound,
/// and the one page that takes the count over it.
fn pages_needed(bound: usize, page: usize) -> usize {
    bound / page + 1
}

/// The `/slice` page requests the production fake answered for Bank Rate.
fn slice_pages(prod: &ProdMesh) -> Vec<String> {
    prod.bank_requests().into_iter().filter(|t| t.starts_with("/slice/")).collect()
}

/// The held member `day`: offline, so only the local store can answer.
fn held(dir: &TempDir, day: &str) -> i32 {
    cli(dir, &["--offline", "get", &format!("{BR_FULL}(date: {day})")]).0
}

fn mcp(dir: &TempDir, profile: Profile) -> McpServer {
    McpServer::with_profile(Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap()), profile)
}

fn get(srv: &McpServer, request: &str) -> Value {
    tool(srv, "get", &format!(r#"{{"request":{}}}"#, q(request)))
}

/// The refusal says the code, the slice, the bound, a size over the bound (at most `most`), and prefetch.
fn assert_refusal_names_all(text: &str, slice_word: &str, bound: usize, most: usize) {
    assert!(text.contains("E_SLICE_FETCH_LIMIT"), "the bound's code: {text}");
    assert!(text.contains(slice_word), "the slice: {text}");
    let ns = integers(text);
    assert!(ns.contains(&(bound as u64)), "the bound {bound}: {text}");
    assert!(
        ns.iter().any(|n| (bound as u64 + 1..=most as u64).contains(n)),
        "its size so far, over the bound {bound} and at most {most}: {text}"
    );
    assert!(text.contains("prefetch"), "prefetch as the way to hold it first: {text}");
}

// ------------------------------------------------------------------------------- the command

#[test]
fn cli_get_of_a_slice_over_the_bound_is_refused_and_stops_at_the_bound() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home_bounded(&[("datumesh.com", &prod.origin)], BOUND as u64);
    let (code, out, err) = cli(&dir, &["get", &bank_slice()]);
    assert_ne!(code, 0, "refused, not answered: {out}");
    // The members it read before stopping: the pages up to the bound and the one over it, no more.
    let most = pages_needed(BOUND, PROD_PAGE) * PROD_PAGE;
    assert_refusal_names_all(&err, "bank_rate", BOUND, most);
    assert!(!out.contains("\"members\""), "no members are answered: {out}");
    let pages = slice_pages(&prod);
    assert!(
        !pages.is_empty() && pages.len() <= pages_needed(BOUND, PROD_PAGE),
        "at most {} pages of {PROD_PAGE} for a bound of {BOUND}, not the whole slice: {pages:?}",
        pages_needed(BOUND, PROD_PAGE)
    );
    assert!(
        prod.bank_served() <= most && prod.bank_served() < MEMBERS,
        "members served: {} (the slice has {MEMBERS})",
        prod.bank_served()
    );
}

#[test]
fn a_refused_slice_get_keeps_nothing() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home_bounded(&[("datumesh.com", &prod.origin)], BOUND as u64);
    let (code, _, _) = cli(&dir, &["get", &bank_slice()]);
    assert_ne!(code, 0);
    // Not one member was kept, the first page's included, and the slice is not held: offline it is not answered.
    assert_ne!(held(&dir, "2026-09-01"), 0, "the first member was not kept");
    assert_ne!(held(&dir, "2026-11-04"), 0, "nor a later one");
    let (code, out, _) = cli(&dir, &["--offline", "get", &bank_slice()]);
    assert_ne!(code, 0, "the slice is not held: {out}");
}

#[test]
fn a_bound_below_the_first_page_asks_for_one_page() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home_bounded(&[("datumesh.com", &prod.origin)], 10);
    let (code, out, err) = cli(&dir, &["get", &bank_slice()]);
    assert_ne!(code, 0, "{out}");
    assert_refusal_names_all(&err, "bank_rate", 10, PROD_PAGE);
    assert_eq!(slice_pages(&prod).len(), 1, "{:?}", slice_pages(&prod));
    assert!(prod.bank_served() <= PROD_PAGE, "{}", prod.bank_served());
}

#[test]
fn a_slice_of_exactly_the_bound_is_answered_and_one_more_member_is_not() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home_bounded(&[("datumesh.com", &prod.origin)], MEMBERS as u64);
    let (code, out, err) = cli(&dir, &["get", &bank_slice()]);
    assert_eq!(code, 0, "a slice of exactly the bound is answered: {err}\nunknown to the fake: {:?}", prod.unknown());
    assert!(out.contains("\"members\""), "{out}");
    let prod2 = ProdMesh::start(true);
    let dir2 = fresh_home_bounded(&[("datumesh.com", &prod2.origin)], MEMBERS as u64 - 1);
    let (code, out, err) = cli(&dir2, &["get", &bank_slice()]);
    assert_ne!(code, 0, "{out}");
    assert!(err.contains("E_SLICE_FETCH_LIMIT"), "{err}");
}

#[test]
fn a_slice_held_by_prefetch_is_answered_whatever_the_bound() {
    // prefetch is the unbounded way the refusal points to: it holds the slice, and the get then reads it locally.
    let prod = ProdMesh::start(true);
    let dir = fresh_home_bounded(&[("datumesh.com", &prod.origin)], BOUND as u64);
    let (code, out, err) = cli(&dir, &["prefetch", &bank_slice()]);
    assert_eq!(code, 0, "prefetch is not bound: {err}");
    assert!(integers(&out).contains(&(MEMBERS as u64)), "{out}");
    let pages = slice_pages(&prod).len();
    let (code, out, err) = cli(&dir, &["get", &bank_slice()]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("\"members\""), "{out}");
    assert_eq!(slice_pages(&prod).len(), pages, "read from the store: {:?}", slice_pages(&prod));
}

// ------------------------------------------------------------------------------- the agent

#[test]
fn mcp_get_of_a_slice_over_the_bound_is_refused_in_both_profiles() {
    for profile in [Profile::Full, Profile::Small] {
        let prod = ProdMesh::start(true);
        let dir = fresh_home_bounded(&[("datumesh.com", &prod.origin)], BOUND as u64);
        let srv = mcp(&dir, profile);
        let v = get(&srv, &bank_slice());
        assert!(is_error(&v), "{profile:?}: a refusal, not an answer: {}", jcs(&v));
        let most = pages_needed(BOUND, PROD_PAGE) * PROD_PAGE;
        // The text an agent reads says it all; the structured content carries the code.
        assert_refusal_names_all(&items(&v).join("\n"), "bank_rate", BOUND, most);
        assert_eq!(structured(&v).get("error").and_then(Value::as_str), Some("E_SLICE_FETCH_LIMIT"), "{}", jcs(&v));
        assert!(structured(&v).get("members").is_none(), "no partial members: {}", jcs(&v));
        assert!(slice_pages(&prod).len() <= pages_needed(BOUND, PROD_PAGE), "{profile:?}: {:?}", slice_pages(&prod));
        assert!(prod.bank_served() < MEMBERS, "{profile:?}: {}", prod.bank_served());
    }
}

#[test]
fn mcp_get_of_a_slice_within_the_bound_is_answered() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home_bounded(&[("datumesh.com", &prod.origin)], MEMBERS as u64);
    // The full profile answers the whole slice in one page; the small one pages it by 15 (local 0.5.5, item 27),
    // and its first page is an answer, not a refusal.
    let v = get(&mcp(&dir, Profile::Full), &bank_slice());
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_eq!(arr(structured(&v), "members").len(), MEMBERS, "{}", jcs(&v));
    let v = get(&mcp(&dir, Profile::Small), &bank_slice());
    assert!(!is_error(&v), "{}", jcs(&v));
    assert_eq!(arr(structured(&v), "members").len(), 15, "{}", jcs(&v));
}

// ------------------------------------------------------------------------------- several open arguments

/// A resolver on the test mesh in pages of `FX_PAGE`, with the bound `max`.
fn fx_resolver(dir: &TempDir, up: &Upstream, max: u64) -> Arc<Resolver> {
    let mut c = config(dir, up);
    c.slice_fetch_max = max;
    Arc::new(Resolver::open(c).unwrap())
}

#[test]
fn a_slice_with_several_open_arguments_stops_at_the_bound() {
    // The control: with the default bound the get reads every member, in ceil(121 / 10) pages, plus what any read asks
    // for (the mesh document, the name, the key document).
    let up = upstream_with(FX_PAGE);
    let dir = TempDir::new("l053-bound-fx-control");
    let r = fx_resolver(&dir, &up, 50_000);
    let v = get(&McpServer::new(r), FX_SLICE);
    assert!(!is_error(&v), "{}", jcs(&v));
    let full = up.requests();
    let all_pages = FX_MEMBERS.div_ceil(FX_PAGE);

    let up = upstream_with(FX_PAGE);
    let dir = TempDir::new("l053-bound-fx");
    let r = fx_resolver(&dir, &up, FX_BOUND as u64);
    let v = get(&McpServer::new(r.clone()), FX_SLICE);
    assert!(is_error(&v), "refused, not answered: {}", jcs(&v));
    let text = items(&v).join("\n");
    let most = pages_needed(FX_BOUND, FX_PAGE) * FX_PAGE;
    assert_refusal_names_all(&text, "fx_rate", FX_BOUND, most);
    // No more pages than the bound needs: the requests saved are at least the pages past it.
    let bounded = up.requests();
    let saved = all_pages - pages_needed(FX_BOUND, FX_PAGE);
    assert!(
        full >= bounded + saved as u64,
        "the bounded get asked {bounded} requests, the whole slice's {full}: at least {saved} pages fewer"
    );
    // Nothing kept: offline, no member answers.
    r.set_offline(true);
    let reply = r.get(USD).unwrap();
    assert_ne!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.body));
}
