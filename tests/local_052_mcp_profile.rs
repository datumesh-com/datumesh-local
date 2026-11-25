//! datumesh-local 0.5.2, item 18: the agent tool surface has a budget.
//!
//! * `datumesh mcp --profile small`: `tools/list` has exactly `find`, `get`, `calc` and `interval`,
//!   each described in one or two sentences; the instructions are one or two sentences plus the held
//!   sentence (the mesh's own `held`, from its catalogue summary). 1,500 tokens or less in all.
//! * `--profile full` is today's surface (`tools/list` and the instructions, byte for byte as 0.5.1
//!   gives them over the production fake: `tests/data/mcp/full-profile-0.5.1.json`), and it is the
//!   default. Any other profile is a usage error (exit 2).
//! * The ceiling. The small profile's `tools/list` tools (their JCS) plus its instructions must stay
//!   within `SMALL_CHAR_CEILING` characters, so the profile cannot creep. Reasoning: the budget is
//!   1,500 tokens; compact JSON with schema punctuation tokenizes densely, at about 3.3 to 4
//!   characters per token for Claude's tokenizer (prose nearer 4, JSON punctuation nearer 3). At the
//!   dense end, 3.33 characters per token, 5,000 characters is 1,500 tokens; so a surface within
//!   5,000 characters is within the budget whatever the mix. (Four one-to-two-sentence tools with
//!   minimal schemas and the ~280-character held sentence measure well under this; once the
//!   implementation exists the ceiling may be lowered to its measure plus a margin, by the test
//!   writer.)
//! * `tools/mcp_tokens.py` counts tokens per tool (name, description, input schema) and for the
//!   instructions. Its interface, as these tests use it, offline:
//!   `python3 tools/mcp_tokens.py --definitions <file> [--json]`, where the file is
//!   `{"instructions": <string>, "tools": <tools/list's tools array>}`. With `--json` it prints one
//!   object: `{"method": <how it counted>, "known_error": <the method's known error>, "tools":
//!   {<name>: <count>, ...}, "instructions": <count>, "total": <count>}`, the total being the sum.
//!   Without `ANTHROPIC_API_KEY` the method is an approximation and says so ("approximat..."); with a
//!   key it may use the token-counting endpoint. Without `--json` it prints a human-readable table
//!   naming each tool, the instructions, the total and the method. `docs/mcp-tokens.md` is the
//!   report it regenerates.
//!
//! The command line is run as `datumesh mcp` runs it, on a real stdin: the test binary runs itself
//! as a child (`child_runs_the_command_line`, which does nothing unless `L052_MCP_CHILD` is set) and
//! the child calls `datumesh_local::cli::run`, with the JSON-RPC lines piped to it.

#![cfg(unix)]

mod common;
#[path = "support/prod_mesh.rs"]
mod prod_mesh;
#[path = "support/tools.rs"]
mod tools;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::*;
use datumesh_core::json::{self, Value};
use prod_mesh::*;
use tools::{jcs, q, M};

const CHILD: &str = "L052_MCP_CHILD";
const SEP: char = '\u{1f}';
const SMALL: [&str; 4] = ["calc", "find", "get", "interval"];
const SMALL_CHAR_CEILING: usize = 5_000;
const BUDGET_TOKENS: u64 = 1_500;

/// The child's side: the command line on this process's own stdin and stdout.
#[test]
fn child_runs_the_command_line() {
    let Ok(args) = std::env::var(CHILD) else { return };
    let args: Vec<String> = args.split(SEP).map(str::to_string).collect();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let code = datumesh_local::cli::run(&args, &mut stdout.lock(), &mut stderr.lock());
    std::process::exit(code);
}

fn request(id: u32, method: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"method":{},"params":{{{M}}}}}"#, q(method))
}

/// `datumesh --home <dir> mcp <flags>` with `tools/list` (id 1) and `server/discover` (id 2) on its
/// stdin: (exit status, the JSON lines it wrote, stderr).
fn mcp_cli(dir: &TempDir, flags: &[&str]) -> (i32, Vec<Value>, String) {
    let mut args = vec!["--home".to_string(), dir.path().to_string_lossy().into_owned(), "mcp".to_string()];
    args.extend(flags.iter().map(|f| f.to_string()));
    let joined = args.join(&SEP.to_string());
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_runs_the_command_line", "--nocapture", "--test-threads=1", "-q"])
        .env(CHILD, joined)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "{}\n{}", request(1, "tools/list"), request(2, "server/discover")).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let lines = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| json::parse(l.as_bytes()).unwrap_or_else(|e| panic!("{l}: {e:?}")))
        .collect();
    (out.status.code().unwrap_or(-1), lines, String::from_utf8_lossy(&out.stderr).into_owned())
}

/// The surface the command line gave: `{"instructions", "tools"}` (from `server/discover` and
/// `tools/list`).
fn surface(lines: &[Value]) -> Value {
    let by_id = |id: &str| {
        lines
            .iter()
            .find(|l| l.get("id").and_then(Value::as_number_text) == Some(id))
            .and_then(|l| l.get("result"))
            .unwrap_or_else(|| panic!("no result {id} in {lines:?}"))
    };
    let tools = by_id("1").get("tools").cloned().expect("tools");
    let instructions = by_id("2").get("instructions").cloned().expect("instructions");
    Value::Object(vec![("instructions".into(), instructions), ("tools".into(), tools)])
}

fn tools_of(surface: &Value) -> &[Value] {
    match surface.get("tools") {
        Some(Value::Array(a)) => a,
        other => panic!("tools: {other:?}"),
    }
}

fn names(surface: &Value) -> Vec<String> {
    let mut n: Vec<String> = tools_of(surface).iter().map(|t| s(t, "name")).collect();
    n.sort();
    n
}

/// Sentences in `text`: ends of sentence (`.`, `!`, `?`) followed by a space or the end.
fn sentences(text: &str) -> usize {
    let t = text.trim();
    let b = t.as_bytes();
    (0..b.len()).filter(|&i| matches!(b[i], b'.' | b'!' | b'?') && (i + 1 == b.len() || b[i + 1] == b' ')).count()
}

fn world() -> (ProdMesh, TempDir) {
    let prod = ProdMesh::start(true);
    let dir = fresh_home(&[("datumesh.com", &prod.origin)]);
    (prod, dir)
}

fn small_surface() -> Value {
    let (_prod, dir) = world();
    let (code, lines, err) = mcp_cli(&dir, &["--profile", "small"]);
    assert_eq!(code, 0, "{err}");
    surface(&lines)
}

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

/// The full profile as 0.5.2 gives it over the production fake: 0.5.1's surface without the held
/// sentence in the `find` and `get` descriptions (item 36: carried once, in the instructions).
fn snapshot() -> Value {
    json::parse(&std::fs::read(data("mcp/full-profile-0.5.2.json")).unwrap()).unwrap()
}

/// The full profile as 0.5.1 gave it (item 18's baseline).
fn snapshot_051() -> Value {
    json::parse(&std::fs::read(data("mcp/full-profile-0.5.1.json")).unwrap()).unwrap()
}

fn repo(p: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(p)
}

// ------------------------------------------------------------------------------- the profiles

#[test]
fn the_small_profile_lists_exactly_find_get_calc_and_interval() {
    let sf = small_surface();
    assert_eq!(names(&sf), SMALL, "{}", jcs(&sf));
    for t in tools_of(&sf) {
        let d = s(t, "description");
        let n = sentences(&d);
        assert!((1..=2).contains(&n), "{}: one or two sentences, not {n}: {d}", s(t, "name"));
        assert!(t.get("inputSchema").is_some(), "{}", jcs(t));
    }
}

#[test]
fn the_small_profile_s_instructions_are_one_or_two_sentences_and_the_held_sentence() {
    let sf = small_surface();
    let text = s(&sf, "instructions");
    let held = production_held();
    assert!(text.contains(&held), "the held sentence, verbatim: {text}");
    let rest = text.replace(&held, "");
    let n = sentences(&rest);
    assert!((1..=2).contains(&n), "one or two sentences besides the held one, not {n}: {text}");
}

#[test]
fn the_small_profile_stays_within_its_character_ceiling() {
    let sf = small_surface();
    let tools = String::from_utf8(sf.get("tools").unwrap().to_jcs()).unwrap();
    let chars = tools.chars().count() + s(&sf, "instructions").chars().count();
    assert!(
        chars <= SMALL_CHAR_CEILING,
        "{chars} characters, over the ceiling of {SMALL_CHAR_CEILING} (about {BUDGET_TOKENS} tokens): {}",
        jcs(&sf)
    );
}

#[test]
fn the_full_profile_is_today_s_surface_and_the_default() {
    let (_prod, dir) = world();
    let want = snapshot();
    let (code, lines, err) = mcp_cli(&dir, &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(jcs(&surface(&lines)), jcs(&want), "the default is 0.5.2's full surface");
    let (code, lines, err) = mcp_cli(&dir, &["--profile", "full"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(jcs(&surface(&lines)), jcs(&want), "--profile full is the default's surface");
    assert_eq!(tools_of(&want).len(), 12);
}

#[test]
fn the_full_profile_is_0_5_1_s_but_for_the_held_sentence_in_find_and_get() {
    // Item 18: the full profile is unchanged; item 36 then takes the held sentence out of the find
    // and get descriptions. Everything else (names, order, schemas, other descriptions, the
    // instructions) is 0.5.1's.
    let (old, new) = (snapshot_051(), snapshot());
    assert_eq!(s(&new, "instructions"), s(&old, "instructions"));
    let held = production_held();
    let (a, b) = (tools_of(&old), tools_of(&new));
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b) {
        let name = s(x, "name");
        assert_eq!(name, s(y, "name"), "the same tools in the same order");
        assert_eq!(jcs(x.get("inputSchema").unwrap()), jcs(y.get("inputSchema").unwrap()), "{name}: its schema");
        let d = s(x, "description");
        let want = if name == "find" || name == "get" {
            assert!(d.contains(&held), "0.5.1 carried it in {name}");
            d.replace(&format!(" {held}"), "")
        } else {
            d
        };
        assert_eq!(s(y, "description"), want, "{name}: its description, the held sentence removed");
        assert!(!s(y, "description").contains(&held), "{name}");
    }
}

#[test]
fn an_unknown_profile_is_a_usage_error() {
    let (_prod, dir) = world();
    let (code, lines, err) = mcp_cli(&dir, &["--profile", "tiny"]);
    assert_eq!(code, 2, "{err}");
    assert!(lines.is_empty(), "{lines:?}");
    assert!(err.contains("--profile"), "{err}");
    let (_, help, _) = cli(&dir, &["--help"]);
    assert!(help.contains("--profile"), "the usage names --profile: {help}");
}

// ------------------------------------------------------------------------------- the count

/// `python3 tools/mcp_tokens.py --definitions <file> <extra>`, without an API key.
fn tokens_script(definitions: &Value, extra: &[&str]) -> (i32, String, String) {
    let tmp = TempDir::new("l052-tokens");
    let f = tmp.path().join("definitions.json");
    std::fs::write(&f, definitions.to_jcs()).unwrap();
    let out = Command::new("python3")
        .arg("-I")
        .arg(repo("tools/mcp_tokens.py"))
        .arg("--definitions")
        .arg(&f)
        .args(extra)
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .unwrap_or_else(|e| panic!("python3: {e}"));
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn count(v: Option<&Value>) -> u64 {
    v.and_then(Value::as_number_text).and_then(|t| t.parse().ok()).unwrap_or_else(|| panic!("a count: {v:?}"))
}

#[test]
fn the_token_script_exists() {
    assert!(repo("tools/mcp_tokens.py").is_file(), "tools/mcp_tokens.py");
}

#[test]
fn the_token_script_counts_each_tool_and_the_instructions_and_names_its_method() {
    let full = snapshot();
    let (code, out, err) = tokens_script(&full, &["--json"]);
    assert_eq!(code, 0, "{out}\n{err}");
    let v = json::parse(out.trim().as_bytes()).unwrap_or_else(|e| panic!("one JSON object: {out}: {e:?}"));
    let method = s(&v, "method");
    assert!(method.to_lowercase().contains("approximat"), "no key: the approximation, named: {method}");
    assert!(!s(&v, "known_error").is_empty(), "the approximation's known error: {out}");
    let per_tool = v.get("tools").and_then(Value::as_object).unwrap_or_else(|| panic!("tools: {out}"));
    let mut listed: Vec<String> = per_tool.iter().map(|(k, _)| k.clone()).collect();
    listed.sort();
    assert_eq!(listed, names(&full), "{out}");
    let sum: u64 = per_tool.iter().map(|(_, n)| count(Some(n))).sum::<u64>() + count(v.get("instructions"));
    assert!(per_tool.iter().all(|(_, n)| count(Some(n)) > 0), "{out}");
    assert_eq!(count(v.get("total")), sum, "the total is the sum: {out}");
    // The human-readable form names every tool, the instructions, the total and the method.
    let (code, text, err) = tokens_script(&full, &[]);
    assert_eq!(code, 0, "{err}");
    for n in names(&full) {
        assert!(text.contains(&n), "{n}: {text}");
    }
    let lower = text.to_lowercase();
    assert!(lower.contains("instructions") && lower.contains("total") && lower.contains("method"), "{text}");
}

#[test]
fn the_script_counts_the_small_profile_at_1500_tokens_or_less() {
    let sf = small_surface();
    let (code, out, err) = tokens_script(&sf, &["--json"]);
    assert_eq!(code, 0, "{out}\n{err}");
    let v = json::parse(out.trim().as_bytes()).unwrap_or_else(|e| panic!("{out}: {e:?}"));
    let total = count(v.get("total"));
    assert!(total <= BUDGET_TOKENS, "{total} tokens on its method ({}): {out}", s(&v, "method"));
}

#[test]
fn the_report_lists_each_tool_of_the_default_list_and_the_method() {
    let path = repo("docs/mcp-tokens.md");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    for n in names(&snapshot()) {
        assert!(text.contains(&format!("`{n}`")) || text.contains(&n), "{n} in docs/mcp-tokens.md");
    }
    let lower = text.to_lowercase();
    assert!(lower.contains("instructions") && lower.contains("small") && lower.contains("method"), "{text}");
    assert!(lower.contains("mcp_tokens.py"), "names the script that regenerates it");
}
