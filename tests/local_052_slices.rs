//! datumesh-local 0.5.2, item 12: a local evaluation over a public slice fetches what it needs on
//! demand, within a stated bound (`slice_fetch_max`, default 50,000 members per evaluation); aliases
//! work in `prefetch`. The production mesh is the fake of `tests/support/prod_mesh.rs`, whose Bank
//! Rate slice is the recorded window 2026-09-01..2026-11-20 (59 members, production's receipts),
//! served in JSON pages by `next` as production writes it, and, narrowed, by `/x/interval` with
//! `from` and `to`.
//!
//! A ranged slice has no URI on production (SPEC 4.4; `/slice/...bank_rate(date:[lo..hi])` answers
//! 400 `E_PARSE_URI`, recorded, and the fake answers the same), so the narrowed range is fetched by
//! production's `/x/interval`. The tests read the requests the fake logged for the range's two ends
//! and count the members it served.

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::sync::Arc;

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_local::mcp::McpServer;
use datumesh_local::{Config, Resolver};
use prod_mesh::*;
use tools::{q, tool};

const BR_FULL: &str = "datumesh.com/relay/bankofengland.co.uk/bank_rate";
const WHOLE: &str = "last_before(@boe/bank_rate(*), 2026-11-05)";
const BOUNDED: &str = "last_before(@boe/bank_rate(*), 2026-11-05, 30 d)";
const MEMBERS: usize = 59;

fn parse(out: &str) -> Value {
    json::parse(out.trim_end().as_bytes()).unwrap_or_else(|e| panic!("not JSON: {out}: {e:?}"))
}

fn world() -> (ProdMesh, TempDir) {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    (prod, dir)
}

/// The held member `day`: offline, so only the local store can answer.
fn held(dir: &TempDir, day: &str) -> i32 {
    cli(dir, &["--offline", "get", &format!("{BR_FULL}(date: {day})")]).0
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

// ------------------------------------------------------------------------------- prefetch

#[test]
fn prefetch_takes_short_names() {
    let (prod, dir) = world();
    let (code, out, err) = cli(&dir, &["prefetch", "boe/bank_rate(*)"]);
    assert_eq!(code, 0, "{err}\nnot known to the fake: {:?}", prod.unknown());
    assert_eq!(s(&parse(&out), "members"), MEMBERS.to_string(), "{out}");
    // It fetched the full name's slice, and nothing under the short name.
    let asked = prod.bank_requests();
    assert!(!asked.is_empty() && asked.iter().all(|t| t.contains(BR_FULL)), "{asked:?}");
    assert_eq!(prod.bank_served(), MEMBERS);
    assert_eq!(held(&dir, "2026-11-04"), 0, "the members are held");
}

#[test]
fn prefetch_follows_next_page_by_page() {
    let (prod, dir) = world();
    let (code, out, err) = cli(&dir, &["prefetch", "boe/bank_rate(*)", "--page", "10"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(s(&parse(&out), "members"), MEMBERS.to_string(), "{out}");
    assert!(prod.bank_requests().len() >= 6, "six pages of ten: {:?}", prod.bank_requests());
}

// ------------------------------------------------------------------------------- on demand

#[test]
fn calc_over_last_before_fetches_the_slice_on_demand_with_no_prefetch() {
    let (prod, dir) = world();
    let (code, out, err) = cli(&dir, &["calc", WHOLE]);
    assert_eq!(code, 0, "{err}\nunknown to the fake: {:?}", prod.unknown());
    let v = parse(&out);
    assert_eq!(s(&v, "value"), bank_value_on_or_before("2026-11-05"), "{out}");
    // The whole slice was read, and every member was verified and held, as prefetch holds them.
    assert_eq!(prod.bank_served(), MEMBERS);
    assert_eq!(held(&dir, "2026-11-04"), 0);
    assert_eq!(held(&dir, "2026-09-01"), 0);
    // The slice is one lineage entry (SPEC 15.13): the answer says what it read.
    assert!(out.contains("slice:") || out.contains("bank_rate"), "{out}");
}

#[test]
fn a_second_evaluation_over_the_held_slice_fetches_nothing() {
    let (prod, dir) = world();
    assert_eq!(cli(&dir, &["calc", WHOLE]).0, 0);
    let (asked, served) = (prod.bank_requests().len(), prod.bank_served());
    // Another formula over the same slice (not the memoised calc): read locally.
    let other = "last_before(@boe/bank_rate(*), 2026-11-04) * 2";
    let (code, out, err) = cli(&dir, &["calc", other]);
    assert_eq!(code, 0, "{err}");
    let want = bank_value_on_or_before("2026-11-04").parse::<f64>().unwrap() * 2.0;
    assert_eq!(s(&parse(&out), "value").parse::<f64>().unwrap(), want, "{out}");
    assert_eq!(prod.bank_requests().len(), asked, "{:?}", prod.bank_requests());
    assert_eq!(prod.bank_served(), served);
    // And with the network off.
    let (code, _, err) = cli(&dir, &["--offline", "calc", "last_before(@boe/bank_rate(*), 2026-11-03) * 3"]);
    assert_eq!(code, 0, "{err}");
}

#[test]
fn a_bounded_lookup_fetches_only_the_narrowed_range() {
    let (prod, dir) = world();
    let (code, out, err) = cli(&dir, &["calc", BOUNDED]);
    assert_eq!(code, 0, "{err}\n{:?}", prod.unknown());
    assert_eq!(s(&parse(&out), "value"), bank_value_on_or_before("2026-11-05"), "{out}");
    // last_before(f(*), x, b) reads f([x - b..x]) (SPEC 15.13): 2026-10-06..2026-11-05.
    let asked = prod.bank_requests().join("\n");
    assert!(asked.contains("2026-10-06") && asked.contains("2026-11-05"), "the range's two ends: {asked}");
    let served = prod.bank_served();
    assert!((1..=31).contains(&served), "only the days of the range, not the {MEMBERS} of the slice: {served}");
    assert_eq!(held(&dir, "2026-11-04"), 0);
    assert_ne!(held(&dir, "2026-09-01"), 0, "a day outside the range was not fetched");
    let refused: Vec<Req> = prod.requests().into_iter().filter(|r| r.answered >= 400).collect();
    assert!(refused.is_empty(), "no request the production mesh refuses: {refused:?}");
}

// ------------------------------------------------------------------------------- the bound

#[test]
fn over_the_bound_the_evaluation_is_refused_naming_the_slice_its_size_and_the_bound() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home_bounded(&[("datumesh.com", &prod.origin)], 30);
    let (code, out, err) = cli(&dir, &["calc", WHOLE]);
    assert_ne!(code, 0, "{out}");
    assert!(err.contains("E_SLICE_FETCH_LIMIT"), "{err}");
    assert!(err.contains("bank_rate"), "the slice: {err}");
    assert!(integers(&err).contains(&30), "the bound: {err}");
    assert!(integers(&err).iter().any(|n| (31..=MEMBERS as u64).contains(n)), "its size so far: {err}");
    assert!(err.contains("prefetch"), "prefetch as the way to hold it first: {err}");
    // Nothing partial was computed or kept.
    assert!(!out.contains("\"value\""), "{out}");
    assert_ne!(held(&dir, "2026-11-04"), 0, "no member was stored");
    assert_ne!(held(&dir, "2026-09-01"), 0);
    let (code, _, _) = cli(&dir, &["--offline", "calc", WHOLE]);
    assert_ne!(code, 0, "no answer was kept");
}

#[test]
fn a_bounded_lookup_inside_the_bound_is_not_refused() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home_bounded(&[("datumesh.com", &prod.origin)], 30);
    let (code, out, err) = cli(&dir, &["calc", BOUNDED]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(s(&parse(&out), "value"), bank_value_on_or_before("2026-11-05"));
}

#[test]
fn a_slice_of_exactly_the_bound_is_allowed_and_one_more_member_is_not() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home_bounded(&[("datumesh.com", &prod.origin)], MEMBERS as u64);
    let (code, _, err) = cli(&dir, &["calc", WHOLE]);
    assert_eq!(code, 0, "a slice of exactly the bound is allowed: {err}");
    let prod2 = ProdMesh::start(true);
    let dir2 = fresh_home_bounded(&[("datumesh.com", &prod2.origin)], MEMBERS as u64 - 1);
    let (code, _, err) = cli(&dir2, &["calc", WHOLE]);
    assert_ne!(code, 0);
    assert!(err.contains("E_SLICE_FETCH_LIMIT"), "{err}");
}

/// Two narrowed reads of different ranges: `[2026-10-06..2026-11-05]` (23 members) and
/// `first_after(.., 2026-09-01, 20 d)`, `[2026-09-01..2026-09-21]` (15 members), 38 in all.
const TWO_RANGES: &str =
    "last_before(@boe/bank_rate(*), 2026-11-05, 30 d) + first_after(@boe/bank_rate(*), 2026-09-01, 20 d)";

#[test]
fn the_bound_is_summed_over_the_slices_an_evaluation_fetches() {
    // Each range is under a bound of 30; their sum (38) is not.
    let prod = ProdMesh::start(true);
    let dir = fresh_home_bounded(&[("datumesh.com", &prod.origin)], 30);
    let (code, out, err) = cli(&dir, &["calc", TWO_RANGES]);
    assert_ne!(code, 0, "{out}");
    assert!(err.contains("E_SLICE_FETCH_LIMIT") && integers(&err).contains(&30), "{err}");
    assert_ne!(held(&dir, "2026-09-01"), 0, "nothing stored");
    assert_ne!(held(&dir, "2026-11-04"), 0, "nothing stored");
    // A bound of 40 holds both.
    let prod2 = ProdMesh::start(true);
    let dir2 = fresh_home_bounded(&[("datumesh.com", &prod2.origin)], 40);
    let (code, out, err) = cli(&dir2, &["calc", TWO_RANGES]);
    assert_eq!(code, 0, "{err}");
    let want = bank_value_on_or_before("2026-11-05").parse::<f64>().unwrap()
        + bank_value_on_or_before("2026-09-01").parse::<f64>().unwrap();
    assert_eq!(s(&parse(&out), "value").parse::<f64>().unwrap(), want, "{out}");
    assert!((1..=38).contains(&prod2.bank_served()), "{}", prod2.bank_served());
}

#[test]
fn the_agent_is_told_the_same_through_mcp() {
    let prod = ProdMesh::start(true);
    let dir = fresh_home_bounded(&[("datumesh.com", &prod.origin)], 30);
    let srv = McpServer::new(Arc::new(Resolver::open(Config::in_dir(dir.path()).unwrap()).unwrap()));
    let v = tool(&srv, "calc", &format!(r#"{{"formula":{}}}"#, q(WHOLE)));
    let text = agent_refusal(&v);
    assert!(text.contains("E_SLICE_FETCH_LIMIT") && text.contains("prefetch") && text.contains("bank_rate"), "{text}");
}

// ------------------------------------------------------------------------------- stated

#[test]
fn the_bound_is_stated_in_the_config_the_help_and_the_readme() {
    let dir = TempDir::new("l052-docs");
    Config::in_dir(dir.path()).unwrap();
    let cfg = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(cfg.contains("slice_fetch_max"), "{cfg}");
    assert!(cfg.contains("50000"), "the default is 50,000: {cfg}");
    let (code, help, _) = cli(&dir, &["--help"]);
    assert_eq!(code, 0);
    assert!(help.contains("slice_fetch_max"), "datumesh --help states the bound: {help}");
    let readme = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md")).unwrap();
    assert!(readme.contains("slice_fetch_max") && (readme.contains("50,000") || readme.contains("50000")), "README");
}
