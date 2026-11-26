//! MCP over stdio and Streamable HTTP, for agents (protocol revision 2026-07-28; over stdio, the
//! legacy handshake of 2025-11-25 and 2025-06-18 too). JSON-RPC is handled here by hand with
//! [`datumesh_core::json`]. The tools are `find`, `get`, `explain`, `describe`, `verify`,
//! `define`, `publish` and `subscribe` (a stub); the resources are the held names and classes
//! (`datumesh://name/...`, `datumesh://class/...`), the cells (`cell://...`) and the held blobs
//! (`blob://<sha256>`, checked against their SHA-256).
//!
//! `datumesh mcp` runs [`serve_stdio`] (nothing but MCP messages on stdout; logs on stderr) or,
//! with `--http`, [`serve_http`]: the loopback addresses only, the endpoint `/mcp`, and the token
//! file `mcp-token` in the configuration directory for `define` and `publish`.

use std::io::{BufRead, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::Response;
use axum::routing::any;
use axum::Router;
use datumesh_core::json::{self, Value};
use datumesh_core::{ClassDescriptor, Receipt};

use unicode_normalization::UnicodeNormalization;

use crate::bundle::base64;
use crate::calculus::{OverrideArg, SolveArg, WhatifArgs};
use crate::resolver::{call_form, cell_uri, BlobContent, Got, Source, TEXT_LIMIT};
use crate::{Error, PublicPlan, Reply, Resolver, RootPlan};

/// The protocol revision this server speaks.
const PROTOCOL: &str = "2026-07-28";
/// The legacy revisions the stdio handshake answers as they are asked for.
const LEGACY: [&str; 2] = ["2025-11-25", "2025-06-18"];
/// The time a client may cache a list or discovery answer.
const TTL_MS: i64 = 60000;
/// The token file of the HTTP endpoint, in the configuration directory.
const TOKEN_FILE: &str = "mcp-token";
/// The latest instant (the store's "as of now" for a name's binding).
const END: &str = "9999-12-31T23:59:59.999999Z";
/// The text of `server/discover` and of the handshake's `instructions`.
const INSTRUCTIONS: &str = "Datumesh: every answer is a signed receipt. Use find to discover names, describe for classes and objects, get for a cell (call syntax or a cell:// URI; for a text value, its text, checked), explain for its lineage, verify for a receipt. define and publish write under this machine's unrooted namespace.";
/// The tools, in the order `tools/list` gives them, with their input schemas.
/// The inputs an explain answer gives a page of: the default and the most (datumesh-local 0.5.3, A3).
const EXPLAIN_LIMIT: usize = 20;
const EXPLAIN_MAX: usize = 40;
/// The inputs an explain answer gives of each nested node: the first, with an `explain` pointer to the rest.
const EXPLAIN_NESTED: usize = 20;
/// The lineage a tool answer gives in full; the rest is the `explain` pointer's (A3).
const LINEAGE_CUT: usize = 20;
/// The results a find answer's text names (A2).
const FIND_LISTED: usize = 20;
/// The notification that the tools' summary has changed (A4).
const LIST_CHANGED: &str = "notifications/tools/list_changed";

const TOOLS: &str = r#"[
{"name":"find","description":"Names held here whose name contains the query (case-insensitive). When none are held, or with remote, also the configured find_meshes (and the forward mesh with remote); searched lists each place looked and its status. A class is followed by its methods.","inputSchema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"query":{"type":"string"},"remote":{"type":"boolean"}},"required":["query"],"additionalProperties":false}},
{"name":"get","description":"The answer to a call (name(arg: value, ...)) or a cell:// URI, as a signed receipt. A text blob's content (up to 64 KiB) comes first, checked against its SHA-256 (blob_checked); otherwise its blob:// address and size. A slice of more than 100 members answers a summary (count, first and last key, and for numbers min, max and mean), its first 100 members and the calc call that aggregates the slice (mean for numbers, else count): run that calc to get the aggregate.","inputSchema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"request":{"type":"string"}},"required":["request"],"additionalProperties":false}},
{"name":"explain","description":"The lineage of a cell: its answer and the answers of its inputs, to depth. Its inputs are paged: page (from 1) and limit (default 20, at most 40) choose the page, and inputs_total counts them all.","inputSchema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"request":{"type":"string"},"depth":{"type":"integer","minimum":0},"page":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1,"maximum":40}},"required":["request"],"additionalProperties":false}},
{"name":"describe","description":"A class, object, cell or short id (SPEC 16.8), as the mesh describes it.","inputSchema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false}},
{"name":"verify","description":"Verifies a receipt (its JSON) with this machine's keys.","inputSchema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"receipt":{"type":"string"},"call":{"type":"string"},"text":{"type":"string"}},"additionalProperties":false}},
{"name":"define","description":"Defines a function under local/ (a formula with its signature, or a stored function with its signature) or a class (its descriptor). A formula may come with its tests, in the same call: each names the function as its call, with its expect (a plain literal typed by the return type, or {\"fault\": code}) and optionally asof and inputs. Tests are optional here, and a stored function takes none; a publish to a public mesh needs them (publish_public).","inputSchema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"name":{"type":"string"},"formula":{"type":"string"},"signature":{"type":"object"},"stored":{"type":"boolean"},"class":{"type":"object"},"tests":{"type":"array","items":{"type":"object"}}},"required":["name"],"additionalProperties":false}},
{"name":"publish","description":"Publishes publish-stored lines under local/ (functions, classes and cells), each cell signed by this machine.","inputSchema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"lines":{"type":"array","items":{"type":"object"}}},"required":["lines"],"additionalProperties":false}},
{"name":"subscribe","description":"Not implemented in this version.","inputSchema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"request":{"type":"string"}},"required":["request"],"additionalProperties":false}},
{"name":"whatif","description":"The spreadsheet what-if (also called a diddle or a tweak, for those who know SecDB-style systems): the value a cell would have if some cells held other values, or the value of one cell that makes another equal a number (solve). The answer is a labelled scenario, never signed or stored, and never a fact: it has no receipt and is not verified. Nothing else changes.","inputSchema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"request":{"type":"string"},"overrides":{"type":"array","items":{"type":"object"}},"asof":{"type":"string"},"solve":{"type":"object","properties":{"vary":{"type":"string"},"target":{"type":"string"},"equals":{},"lo":{},"hi":{}},"required":["vary","target","equals","lo","hi"],"additionalProperties":false}},"required":["request"],"additionalProperties":false}},
{"name":"calc","description":"Calculate here, not in code: the formula runs beside the data and returns only the result, with its lineage (the cells it read). Try find for an existing function first. The result is memoised on this machine, so the same calculation is not run twice; with remote it is scoped on the forward mesh and nothing is kept.","inputSchema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"formula":{"type":"string"},"asof":{"type":"string"},"remote":{"type":"boolean"}},"required":["formula"],"additionalProperties":false}},
{"name":"interval","description":"The forward mesh's members of a function over a range of keys (x_interval), each with its receipt, resolving aliases. Keys may be omitted for an unkeyed series. Over 100 members it answers a summary (count, first and last keys, min, max and mean for numbers), the first 100 members, and the interval call for the rest.","inputSchema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"function":{"type":"string"},"keys":{"type":"object"},"from":{"type":"string"},"to":{"type":"string"}},"required":["function","from","to"],"additionalProperties":false}},
{"name":"publish_public","description":"Publishes a rooted namespace of this machine's names to a public mesh (the bundle, with its tests). Each formula needs tests that cover its derived cases and every branch (E_TESTS_MISSING lists what is missing); the mesh checks the rest and refuses with its code.","inputSchema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"to":{"type":"string"},"namespace":{"type":"string"},"key":{"type":"string"},"names":{"type":"array","items":{"type":"string"}},"export":{"type":"string"}},"required":["to","namespace","key","names"],"additionalProperties":false}}
]"#;
/// The resource templates of `resources/templates/list`.
const TEMPLATES: &str = r#"[
{"uriTemplate":"cell://{+cell}","name":"cell","description":"A cell: its envelope (SPEC 5.5).","mimeType":"application/json"},
{"uriTemplate":"datumesh://name/{+name}","name":"name","description":"A function name: its binding and descriptor.","mimeType":"application/json"},
{"uriTemplate":"datumesh://class/{+class}","name":"class","description":"A class: its descriptor.","mimeType":"application/json"},
{"uriTemplate":"blob://{sha256}","name":"blob","description":"Blob content by SHA-256, checked against it: text for valid UTF-8 up to 64 KiB, otherwise base64.","mimeType":"application/octet-stream"}
]"#;

/// The transport a message came in on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Standard input and output: the legacy handshake is allowed, and `_meta` is not required
    /// after it.
    Stdio,
    /// Streamable HTTP. `authorized` is whether the request carried the endpoint's token.
    Http {
        /// The request's bearer token is the token file's.
        authorized: bool,
    },
}

/// The tool surface a server gives (datumesh-local 0.5.2, item 18): `full`, the default, is every
/// tool; `small` is `find`, `get`, `calc` and `interval`, with short descriptions and minimal schemas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// Every tool (the default).
    Full,
    /// The four tools an agent needs for lookups and calculations, within a small token budget.
    Small,
}

impl Profile {
    /// The profile named `name` (`full` or `small`); `None` for any other name.
    pub fn named(name: &str) -> Option<Profile> {
        match name {
            "full" => Some(Profile::Full),
            "small" => Some(Profile::Small),
            _ => None,
        }
    }
}

/// The instructions of the small profile: two sentences; the mesh's held sentence follows them.
const SMALL_INSTRUCTIONS: &str = "Datumesh: every answer is a signed receipt. Use find to discover names, then get for a cell or calc to compute one.";

/// The tools of the small profile, in `tools/list` order (`find`, `get`, `calc`, `interval`).
const SMALL_TOOLS: &str = r#"[
{"name":"find","description":"Names held here that contain the query, case-insensitively. With remote, or when none are held, also the find meshes.","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}},
{"name":"get","description":"The signed answer to a call such as name(arg: value), or to a cell:// URI.","inputSchema":{"type":"object","properties":{"request":{"type":"string"}},"required":["request"]}},
{"name":"calc","description":"Calculates a formula beside the data and returns the result with its lineage.","inputSchema":{"type":"object","properties":{"formula":{"type":"string"}},"required":["formula"]}},
{"name":"interval","description":"The members of a function over a range of keys, each with its receipt.","inputSchema":{"type":"object","properties":{"function":{"type":"string"},"from":{"type":"string"},"to":{"type":"string"}},"required":["function","from","to"]}}
]"#;

/// The MCP server over a resolver: [`McpServer::handle`] answers one message.
pub struct McpServer {
    resolver: Arc<Resolver>,
    /// Set by an `initialize` on stdio: the session is legacy from then on.
    legacy: AtomicBool,
    /// The tools, as `tools/list` gives them (static: the held sentence is in the instructions only, item 36).
    tools: Value,
    /// The tool surface this server gives.
    profile: Profile,
    /// What the catalogue summary gave, or why it could not be had (items 36 to 38).
    surface: Mutex<Surface>,
}

/// The catalogue state of a server (datumesh-local 0.5.2, items 37 and 38): the summary and the
/// subjects once fetched; else the reason of the last failure and when it was tried.
#[derive(Default)]
struct Surface {
    catalogue: Option<Catalogue>,
    /// The catalogue's subjects: each label and the publisher of its rank-1 source.
    subjects: Vec<(String, String)>,
    /// Why the summary could not be fetched, the last time it was tried.
    failure: Option<String>,
    /// The resolver's instant of the last attempt.
    tried: Option<String>,
    /// The summary was fetched by a retry, not at start: `find` carries the held sentence (datumesh-local 0.5.3, A4).
    late: bool,
    /// A retry has succeeded and the client is to be told once (A4).
    notice: bool,
}

/// A retry of the catalogue summary is made at most once in this many seconds (item 37).
const RETRY_SECS: i64 = 60;

/// A JSON-RPC error: its code, message and optional data.
struct Rpc {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl Rpc {
    fn new(code: i64, message: impl Into<String>) -> Rpc {
        Rpc { code, message: message.into(), data: None }
    }
}

impl From<Error> for Rpc {
    fn from(e: Error) -> Rpc {
        Rpc::new(-32603, e.to_string())
    }
}

/// A tool failure: its spec code when it has one, the error's Display, and, when the failure has
/// members of its own (`fault`, `functions`, the mesh's `detail`), the structured content instead of
/// `{error, detail}`.
struct ToolError {
    code: Option<String>,
    detail: String,
    body: Option<Value>,
}

impl ToolError {
    fn arg(detail: impl Into<String>) -> ToolError {
        ToolError { code: None, detail: detail.into(), body: None }
    }
}

impl From<Error> for ToolError {
    fn from(e: Error) -> ToolError {
        match &e {
            Error::TestsMissing(list) => ToolError {
                code: Some("E_TESTS_MISSING".into()),
                detail: e.to_string(),
                body: Some(obj(vec![
                    ("error", s("E_TESTS_MISSING")),
                    ("functions", Value::Array(list.iter().map(missing_json).collect())),
                ])),
            },
            Error::Refused { code, detail } => ToolError {
                code: Some(code.clone()),
                detail: e.to_string(),
                body: Some(obj(vec![
                    ("error", s(code)),
                    ("detail", json::parse(detail.as_bytes()).unwrap_or_else(|_| s(detail))),
                ])),
            },
            _ => {
                let code = e.code().map(str::to_string).or_else(|| match &e {
                    Error::Client { code, .. } => Some(code.clone()),
                    _ => None,
                });
                ToolError { code, detail: e.to_string(), body: None }
            }
        }
    }
}

/// A missing-tests report as the tool gives it (`E_TESTS_MISSING`'s `functions`).
fn missing_json(m: &crate::MissingTests) -> Value {
    obj(vec![
        ("function", s(&m.function)),
        ("cases", Value::Array(m.cases.iter().map(s).collect())),
        ("branches_total", num(m.branches_total as u64)),
        ("branches_covered", num(m.branches_covered as u64)),
    ])
}

type RpcResult = Result<Vec<(String, Value)>, Rpc>;
type ToolResult = Result<Value, ToolError>;

impl McpServer {
    /// A server over `resolver`. The find mesh's catalogue summary (`GET /find?summary=1`) is
    /// fetched now, and the instructions and the descriptions of `find` and `get` are written from
    /// it (CONTRACT-build2-part2-read R4); when it cannot be had, the static text of 0.3.3 is used.
    pub fn new(resolver: Arc<Resolver>) -> McpServer {
        McpServer::with_profile(resolver, Profile::Full)
    }

    /// A server over `resolver` giving the tool surface of `profile` (item 18): the full surface is
    /// [`McpServer::new`]'s; the small one is four tools, its instructions the held sentence after two.
    pub fn with_profile(resolver: Arc<Resolver>, profile: Profile) -> McpServer {
        let mut tools = json_const(if profile == Profile::Small { SMALL_TOOLS } else { TOOLS });
        if profile == Profile::Full {
            // The held sentence is carried once, in the instructions (item 36); the plain-reference sentence stays.
            append_description(
                &mut tools,
                "get",
                "A plain reference (a label and a reference, such as \"<label> <reference>\") is read through the mesh's find in the same call: an exact one gives its answer, an ambiguous one its candidates, a range is refused (E_RANGE_REFUSED), and a reference nothing resolves to gives suggestions.",
            );
        }
        let server = McpServer {
            resolver,
            legacy: AtomicBool::new(false),
            tools,
            profile,
            surface: Mutex::new(Surface::default()),
        };
        server.refresh();
        server
    }

    /// The catalogue summary and subjects, fetched when they are not held and the last attempt was at least a
    /// minute ago on the resolver's clock (item 37): at most once a minute, and not at all once fetched.
    fn refresh(&self) {
        let now = self.resolver.clock_now();
        {
            let st = self.surface.lock().unwrap_or_else(|p| p.into_inner());
            if st.catalogue.is_some() {
                return;
            }
            if let (Some(a), Some(b)) =
                (st.tried.as_deref().and_then(crate::resolver::epoch_secs), crate::resolver::epoch_secs(&now))
            {
                if b - a < RETRY_SECS {
                    return;
                }
            }
        }
        let fetched = self.fetch_catalogue();
        let mut st = self.surface.lock().unwrap_or_else(|p| p.into_inner());
        // An attempt before this one failed: this one is a retry (A4).
        let retried = st.tried.is_some();
        st.tried = Some(now);
        match fetched {
            Ok((catalogue, subjects)) => {
                st.catalogue = Some(catalogue);
                st.subjects = subjects;
                st.failure = None;
                if retried {
                    st.late = true;
                    st.notice = true;
                }
            }
            Err(why) => st.failure = Some(why),
        }
    }

    /// The summary and the catalogue's subjects, or why one of them could not be had.
    fn fetch_catalogue(&self) -> Result<(Catalogue, Vec<(String, String)>), String> {
        let summary = self.resolver.find_json("summary=1").map_err(|e| e.to_string())?;
        let catalogue = Catalogue::of(&summary).ok_or_else(|| "the summary has no held text".to_string())?;
        // The subjects are the small profile's extra: without them the summary still stands (item 38).
        let root = self.resolver.catalogue_subjects().unwrap_or(Value::Null);
        let mut subjects = Vec::new();
        if let Some(Value::Array(domains)) = root.get("domains") {
            for d in domains {
                if let Some(Value::Array(top)) = d.get("top") {
                    for t in top {
                        let source = s_of(t, "source");
                        let publisher = source.rsplit_once('/').map_or(source.as_str(), |(p, _)| p).to_string();
                        subjects.push((s_of(t, "label"), publisher));
                    }
                }
            }
        }
        Ok((catalogue, subjects))
    }

    /// The instructions as the surface gives them now (items 36 to 38): the summary's text when fetched, else
    /// the static text and the sentence that says the summary could not be fetched and why (item 37).
    fn instructions(&self) -> String {
        let st = self.surface.lock().unwrap_or_else(|p| p.into_inner());
        let base = if self.profile == Profile::Small { SMALL_INSTRUCTIONS } else { INSTRUCTIONS };
        match (&st.catalogue, self.profile) {
            (Some(c), Profile::Full) => c.instructions.clone(),
            (Some(c), Profile::Small) => {
                let mut text = format!("{SMALL_INSTRUCTIONS} {}", c.held);
                if !st.subjects.is_empty() {
                    text.push_str("\nWhat the mesh holds, by subject and its publisher:");
                    for (label, publisher) in &st.subjects {
                        text.push_str(&format!("\n- {label}: {publisher}"));
                    }
                }
                text
            }
            (None, _) => {
                let why = st.failure.clone().unwrap_or_else(|| "not tried yet".into());
                format!("{base} What this mesh holds could not be fetched ({why}); find still works.")
            }
        }
    }

    /// The tools as `tools/list` gives them now. Once a retry has fetched the summary, `find` says what the mesh
    /// holds (the held sentence, and in the small profile the subjects' labels), carried once (A4).
    fn tools_now(&self) -> Value {
        let st = self.surface.lock().unwrap_or_else(|p| p.into_inner());
        let mut tools = self.tools.clone();
        if let (true, Some(c)) = (st.late, &st.catalogue) {
            let mut extra = c.held.clone();
            if self.profile == Profile::Small && !st.subjects.is_empty() {
                let labels: Vec<&str> = st.subjects.iter().map(|(label, _)| label.as_str()).collect();
                extra.push_str(&format!(" What the mesh holds, by subject: {}.", labels.join("; ")));
            }
            append_description(&mut tools, "find", &extra);
        }
        tools
    }

    /// Whether a retry has succeeded since this was last asked: the stdio loop then sends the
    /// `tools/list_changed` notification once (A4).
    fn take_notice(&self) -> bool {
        let mut st = self.surface.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::take(&mut st.notice)
    }

    /// Answers one message (a JSON-RPC line) received on `transport`: the response as one line
    /// of canonical JSON, or `None` for a notification (no response).
    pub fn handle(&self, line: &str, transport: Transport) -> Option<String> {
        let msg = match json::parse(line.as_bytes()) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => return Some(rpc_error(&Value::Null, &Rpc::new(-32600, "invalid request: not an object"))),
            Err(e) => return Some(rpc_error(&Value::Null, &Rpc::new(-32700, format!("parse error: {e}")))),
        };
        // A message without an id is a notification (or a response from the client): no answer.
        let method = msg.get("method").and_then(Value::as_str)?;
        let id = msg.get("id").cloned()?;
        let params = msg.get("params").cloned().unwrap_or_else(|| Value::Object(Vec::new()));
        let stdio = transport == Transport::Stdio;
        if method == "initialize" {
            if !stdio {
                return Some(rpc_error(&id, &Rpc::new(-32601, "initialize is not offered over HTTP")));
            }
            return Some(self.initialize(&id, &params));
        }
        let legacy = stdio && self.legacy.load(Ordering::Acquire);
        if !legacy {
            if let Err(e) = meta_check(&params) {
                return Some(rpc_error(&id, &e));
            }
        }
        if matches!(method, "initialize" | "server/discover" | "tools/list") {
            self.refresh();
        }
        let result = match method {
            "server/discover" => Ok(self.discover()),
            "tools/list" => Ok(vec![
                ("tools".into(), self.tools_now()),
                ("ttlMs".into(), int(TTL_MS)),
                ("cacheScope".into(), s("private")),
            ]),
            "tools/call" => self.call_tool(&params, transport),
            "resources/list" => self.resources_list(),
            "resources/templates/list" => Ok(vec![
                ("resourceTemplates".into(), json_const(TEMPLATES)),
                ("ttlMs".into(), int(TTL_MS)),
                ("cacheScope".into(), s("private")),
            ]),
            "resources/read" => self.resources_read(&params),
            _ => Err(Rpc::new(-32601, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(members) => respond(&id, "result", result_object(members)),
            Err(e) => rpc_error(&id, &e),
        })
    }

    /// The stdio handshake: the protocol version asked for when it is a legacy one, else the
    /// oldest legacy one; the session is legacy from then on.
    fn initialize(&self, id: &Value, params: &Value) -> String {
        let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
        let version = if LEGACY.contains(&asked) { asked } else { LEGACY[0] };
        self.legacy.store(true, Ordering::Release);
        let members = vec![
            ("protocolVersion".into(), s(version)),
            ("capabilities".into(), capabilities()),
            ("serverInfo".into(), server_info()),
            ("instructions".into(), s(self.instructions())),
        ];
        respond(id, "result", result_object(members))
    }

    /// `tools/call`: the tool's result, or a failure as a result with `isError`.
    fn call_tool(&self, params: &Value, transport: Transport) -> RpcResult {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Rpc::new(-32602, "tools/call needs the tool's name"))?;
        if !TOOL_NAMES.contains(&name) || (self.profile == Profile::Small && !SMALL_NAMES.contains(&name)) {
            return Err(Rpc::new(-32602, format!("Unknown tool: {name}")));
        }
        // Writes need the token over HTTP (`define`, `publish` and `publish_public`).
        if matches!(transport, Transport::Http { authorized: false })
            && matches!(name, "define" | "publish" | "publish_public")
        {
            return Err(Rpc { code: -31001, message: "unauthorized".into(), data: None });
        }
        let args = params.get("arguments").cloned().unwrap_or_else(|| Value::Object(Vec::new()));
        if matches!(name, "find" | "get") {
            self.refresh();
        }
        if name == "whatif" {
            return Ok(match self.whatif(&args) {
                Ok(v) => content_members(vec![scenario_text(&v)], v, false),
                Err(e) => tool_failure(e),
            });
        }
        if name == "get" {
            return Ok(match self.get(&args) {
                Ok(members) => members,
                Err(e) => tool_failure(e),
            });
        }
        if name == "describe" {
            return Ok(match self.describe(&args) {
                Ok((structured, texts)) => content_members(texts, structured, false),
                Err(e) => tool_failure(e),
            });
        }
        let outcome = match name {
            "find" => self.find(&args),
            "explain" => self.explain(&args),
            "verify" => self.verify(&args),
            "define" => self.define(&args),
            "publish" => self.publish(&args),
            "publish_public" => self.publish_public(&args),
            "calc" => self.calc(&args),
            "interval" => self.interval(&args),
            _ => {
                let structured = obj(vec![("error", s("not_implemented")), ("tool", s("subscribe"))]);
                return Ok(tool_members(&json_text(&structured), structured, true));
            }
        };
        Ok(match outcome {
            Ok(v) => content_members(vec![tool_text(name, &v)], v, false),
            Err(e) => tool_failure(e),
        })
    }

    fn find(&self, args: &Value) -> ToolResult {
        let query = str_arg(args, "query")?;
        let remote = args.get("remote") == Some(&Value::Bool(true));
        let answer = self.resolver.find_in(query, remote)?;
        let mut results = Vec::new();
        for f in &answer.results {
            if f.kind == "class" && f.held {
                if let Some((hash, class)) = self.class_of(&f.name)? {
                    results.push(published(
                        described(entry_with_call(&f.name, "class", &hash, &f.name), &f.description),
                        f,
                    ));
                    results.extend(self.method_entries(&f.name, &class));
                    continue;
                }
            }
            if f.kind == "class" {
                results.push(published(
                    described(entry_with_call(&f.name, "class", &f.fn_hash, &f.name), &f.description),
                    f,
                ));
                for m in &f.methods {
                    let call = call_template(&m.name, &m.args);
                    results.push(published(
                        described(entry_with_call(&m.name, "method", &m.fn_hash, &call), &m.description),
                        m,
                    ));
                }
                continue;
            }
            let call =
                if f.held { template(&f.name, &self.descriptor(&f.fn_hash)) } else { call_template(&f.name, &f.args) };
            let mut entry =
                published(described(entry_with_call(&f.name, &f.kind, &f.fn_hash, &call), &f.description), f);
            if let (Some(label), Value::Object(m)) = (self.resolver.catalogue_label(&f.name)?, &mut entry) {
                m.push(("label".into(), s(label)));
            }
            results.push(entry);
        }
        let searched = answer
            .searched
            .iter()
            .map(|x| {
                let mut m = vec![("where", s(&x.place))];
                if let Some(status) = &x.status {
                    m.push(("status", s(status)));
                }
                if let Some(retry) = x.retry {
                    m.push(("retry", Value::Bool(retry)));
                }
                obj(m)
            })
            .collect();
        let mut members = vec![("results", Value::Array(results)), ("searched", Value::Array(searched))];
        if answer.nothing_matches() {
            members.push(("message", s(crate::resolver::NOTHING_MATCHES)));
        }
        Ok(obj(members))
    }

    /// `get` (R4): a call or a cell URI as before; a plain reference is read in this call when the
    /// find mesh resolves it exactly, else its candidates, its range refusal or its suggestions.
    fn get(&self, args: &Value) -> Result<Vec<(String, Value)>, ToolError> {
        let request = str_arg(args, "request")?;
        match self.resolver.get_request(request)? {
            Got::Reply(reply) => match slice_answer_of(&reply) {
                Some(v) => Ok(content_members(vec![slice_text(&v)], v, false)),
                None => self.answer(&reply, None),
            },
            Got::Read { reply, label, call } => self.answer(&reply, Some((label, call))),
            Got::Ambiguous(candidates) => Ok(ambiguous_members(request, &candidates)),
            Got::Range => {
                Err(ToolError { code: Some("E_RANGE_REFUSED".into()), detail: RANGE_DETAIL.into(), body: None })
            }
            Got::NotCovered(results) => Ok(not_covered_members(request, &results)),
        }
    }

    /// The answer to `get` for `reply`: its blob's checked text first when it is text within the
    /// limit (CONTRACT-1.7a-text L1), else its `blob` address; a mismatch is an error. With
    /// `resolved` (a reference's label and call), the answer starts with them (R4).
    fn answer(&self, reply: &Reply, resolved: Option<(String, String)>) -> Result<Vec<(String, Value)>, ToolError> {
        if let Some(detail) = blob_mismatch(reply) {
            return Err(ToolError { code: Some("E_BLOB_MISMATCH".into()), detail, body: None });
        }
        let mut block = answer_block(num(u64::from(reply.status)), &reply.body, reply.label.as_deref());
        // The text part is the short answer, never the block's JCS (A2); the block is the structured content.
        let short = self.short_answer(reply);
        let mut head = Vec::new();
        // A withdrawn cell, or a label through one, is not covered: the words say so, with the reason (CONTRACT-round-0.7 R1).
        if let Some(reason) = not_covered_reason(reply) {
            head.push(format!("not covered: {reason}"));
        }
        if let Some((label, call)) = &resolved {
            block = with_members(block, vec![("resolved", obj(vec![("label", s(label)), ("call", s(call))]))]);
            head.push(format!("{label}\n{call}"));
        }
        // A failed get (status 400 or more) is an error whose text names the code and every detail (datumesh-local
        // 0.5.3): the answer block is the structured content, as it is for any answer.
        if reply.status >= 400 {
            return Err(failed(block, &reply.detail.iter().cloned().collect::<Vec<_>>()));
        }
        let Some(blob) = self.resolver.blob_of(reply)? else {
            head.push(short);
            return Ok(content_members(head, block, false));
        };
        let mut info = vec![
            ("length", num(blob.length)),
            ("media_type", s(&blob.media_type)),
            ("sha256", s(&blob.sha256)),
            ("uri", s(format!("blob://{}", blob.sha256))),
        ];
        if let Some(host) = &blob.host {
            info.push(("url", s(format!("https://{host}/blob/{}", blob.sha256))));
        }
        match blob.content {
            BlobContent::Text(text) => {
                let checked = obj(vec![
                    ("blob_checked", Value::Bool(true)),
                    ("length", num(blob.length)),
                    ("media_type", s(&blob.media_type)),
                    ("sha256", s(&blob.sha256)),
                ]);
                let structured =
                    with_members(block.clone(), vec![("text", s(&text)), ("blob_checked", Value::Bool(true))]);
                head.extend([text, json_text(&checked)]);
                head.push(short);
                Ok(content_members(head, structured, false))
            }
            BlobContent::Mismatch(detail) => {
                Err(ToolError { code: Some("E_BLOB_MISMATCH".into()), detail, body: None })
            }
            BlobContent::NotText | BlobContent::Unavailable(_) => {
                let mut members = vec![("blob", obj(info)), ("blob_checked", Value::Bool(false))];
                if let BlobContent::Unavailable(why) = &blob.content {
                    members.push(("blob_unavailable", s(why)));
                }
                let structured = with_members(block.clone(), members.clone());
                head.extend([json_text(&obj(members)), short]);
                Ok(content_members(head, structured, false))
            }
        }
    }

    /// The short answer of a get (A2): the value and its unit, the call, the as-of, the signer, and the receipt's
    /// hash, in a few lines; the answer block is the structured content.
    fn short_answer(&self, reply: &Reply) -> String {
        let Some(r) = reply.receipt() else {
            return format!("status {}: no receipt to answer with.", reply.status);
        };
        let unit = unit_of(&self.descriptor(&r.cell().fn_hash)).map(|u| format!(" {u}")).unwrap_or_default();
        format!(
            "{}{unit}\n{}, as of {}, signed by {}.\nreceipt {}",
            r.value().lexical(),
            call_form(r.cell()),
            r.asof(),
            r.computed_by(),
            r.hash()
        )
    }

    fn explain(&self, args: &Value) -> ToolResult {
        let request = str_arg(args, "request")?;
        let depth = match args.get("depth") {
            None => 1,
            Some(v) => v
                .as_number_text()
                .and_then(|t| t.parse::<usize>().ok())
                .ok_or_else(|| ToolError::arg("depth: an integer, at least 0"))?,
        };
        let page = page_arg(args, "page", 1, None)?;
        let limit = page_arg(args, "limit", EXPLAIN_LIMIT, Some(EXPLAIN_MAX))?;
        let reply = self.resolver.explain(request, Some(depth))?;
        let node = json::parse(&reply.body).unwrap_or(Value::Null);
        let block = self.explain_answer(num(u64::from(reply.status)), &node, reply.label.as_deref(), page, limit);
        if reply.status >= 400 {
            return Err(failed(block, &reply.detail.iter().cloned().collect::<Vec<_>>()));
        }
        Ok(block)
    }

    /// An explain node as the answer (A3): the node in full (its answer block), its inputs cut to the page
    /// (from 1) of `limit`, with `inputs_total`, `page` and `limit` beside them. Each input is slim (`explain_input`).
    /// The raw node, when it is not a cell (a slice), has no inputs to page.
    fn explain_answer(&self, status: Value, node: &Value, label: Option<&str>, page: usize, limit: usize) -> Value {
        let (envelope, children) = match node.get("envelope") {
            Some(e) => (e, node.get("inputs")),
            None => (node, None),
        };
        let all: &[Value] = match children {
            Some(Value::Array(a)) => a,
            _ => &[],
        };
        let shown: Vec<Value> =
            all.iter().skip((page - 1).saturating_mul(limit)).take(limit).map(|c| self.explain_input(c, 1)).collect();
        let mut block = answer_block(status, &envelope.to_jcs(), label);
        if let Value::Object(m) = &mut block {
            m.push(("inputs".into(), Value::Array(shown)));
            m.push(("inputs_total".into(), num(all.len() as u64)));
            m.push(("page".into(), num(page as u64)));
            m.push(("limit".into(), num(limit as u64)));
        }
        block
    }

    /// An input of an explain answer, slim (datumesh-local 0.5.3, A3). An input with inputs gives its call, value
    /// and receipt hash, its own inputs cut to [`EXPLAIN_NESTED`] (with `inputs_total`), and, when cut, an
    /// `explain` pointer to the rest. A leaf gives its call, value and hash at level 1 (the top's inputs), and its
    /// receipt hash alone deeper, so a 16-deep answer stays within the 64 KiB budget. The full envelope is only at
    /// the top.
    fn explain_input(&self, node: &Value, level: usize) -> Value {
        let Some(r) = node.get("envelope").and_then(|e| e.get("receipt")).and_then(|x| Receipt::from_value(x).ok())
        else {
            return node.clone();
        };
        let (call, value, hash) = (call_form(r.cell()), r.value().to_value(), r.hash());
        let inputs: &[Value] = match node.get("inputs") {
            Some(Value::Array(a)) => a,
            _ => &[],
        };
        if inputs.is_empty() {
            return if level == 1 {
                obj(vec![("call", s(call)), ("value", value), ("receipt_hash", s(hash))])
            } else {
                obj(vec![("receipt_hash", s(hash))])
            };
        }
        let mut out = vec![
            ("call", s(call.clone())),
            ("value", value),
            ("receipt_hash", s(hash)),
            (
                "inputs",
                Value::Array(inputs.iter().take(EXPLAIN_NESTED).map(|c| self.explain_input(c, level + 1)).collect()),
            ),
            ("inputs_total", num(inputs.len() as u64)),
        ];
        if inputs.len() > EXPLAIN_NESTED {
            let pointer = obj(vec![("request", s(call)), ("page", num(2)), ("limit", num(EXPLAIN_NESTED as u64))]);
            out.push(("explain", pointer));
        }
        obj(out)
    }

    /// `describe` (SPEC 16.8): the structured content, and the text items: the answer as JSON,
    /// then, for a function, its tests (`tests`, CONTRACT-0.4.9 L1: null with `tests_note` when
    /// they cannot be read).
    fn describe(&self, args: &Value) -> Result<(Value, Vec<String>), ToolError> {
        let id = str_arg(args, "id")?;
        let (reply, listing) = self.resolver.describe_listing(id)?;
        let answer = json::parse(&reply.body).unwrap_or(Value::Null);
        let mut members = vec![("status", num(u64::from(reply.status))), ("describe", answer.clone())];
        let unbound = self.resolver.describe_unbound(id, reply.status);
        if let Some(sentence) = &unbound {
            members.push(("error", s("E_NAME_UNBOUND")));
            members.push(("detail", s(sentence)));
        }
        if reply.status >= 400 {
            return Err(failed(obj(members), &reply.detail.iter().cloned().collect::<Vec<_>>()));
        }
        if answer.get("kind").and_then(Value::as_str) == Some("class") {
            let class = answer.get("class").and_then(Value::as_str).unwrap_or("");
            let methods = match self.class_of(class)? {
                Some((_, c)) => self.method_entries(class, &c),
                None => Vec::new(),
            };
            members.push(("methods", Value::Array(methods)));
        }
        let mut texts = vec![describe_text(id, &answer)];
        if let Some(sentence) = unbound {
            texts.insert(0, sentence);
        }
        if let Some(listing) = listing {
            members.push(("tests", listing.tests.clone()));
            if let Some(note) = &listing.note {
                members.push(("tests_note", s(note)));
            }
            texts.push(listing.lines(false).join("\n"));
        }
        Ok((obj(members), texts))
    }

    /// `verify` (R4): a receipt, or the receipt of a `call`; with `text`, the quoted text is checked
    /// against the cell's text blob after NFC on both sides (`text_matches`, and `first_difference`
    /// in code points when it does not match).
    fn verify(&self, args: &Value) -> ToolResult {
        let quoted = match args.get("text") {
            None => None,
            Some(Value::String(t)) => Some(t.as_str()),
            Some(_) => return Err(ToolError::arg("text: a string")),
        };
        let (receipt, reply) = match (args.get("receipt"), args.get("call")) {
            (Some(_), _) => (str_arg(args, "receipt")?.to_string(), None),
            (None, Some(_)) => {
                let call = str_arg(args, "call")?;
                let reply = self.resolver.get(call)?;
                if let Some(detail) = blob_mismatch(&reply) {
                    return Err(ToolError { code: Some("E_BLOB_MISMATCH".into()), detail, body: None });
                }
                let receipt = reply
                    .envelope()
                    .and_then(|e| e.get("receipt").cloned())
                    .ok_or_else(|| ToolError::arg(format!("{call}: no receipt to verify")))?;
                (json_text(&receipt), Some(reply))
            }
            (None, None) => return Err(ToolError::arg("a receipt, or a call whose receipt is verified")),
        };
        let v = self.resolver.verify(receipt.as_bytes());
        let pick = |k: &str| v.get(k).cloned().unwrap_or(Value::Null);
        let (signer, licence, call, uri) = match Receipt::from_json(receipt.as_bytes()) {
            Ok(r) => (s(r.computed_by()), s(r.licence()), s(call_form(r.cell())), s(cell_uri(r.cell()))),
            Err(_) => (Value::Null, Value::Null, Value::Null, Value::Null),
        };
        let mut members = vec![
            ("ok", pick("ok")),
            ("level", pick("level")),
            ("code", pick("code")),
            ("trust", pick("trust")),
            ("signer", signer),
            ("licence", licence),
            ("call", call),
            ("uri", uri),
        ];
        if let Some(quoted) = quoted {
            let reply = match reply {
                Some(reply) => reply,
                None => {
                    let envelope = obj(vec![("receipt", json::parse(receipt.as_bytes()).unwrap_or(Value::Null))]);
                    Reply::new(200, envelope.to_jcs(), Source::Local)
                }
            };
            let blob = self
                .resolver
                .blob_of(&reply)?
                .ok_or_else(|| ToolError::arg("text: the receipt's value is not a blob"))?;
            let content = match blob.content {
                BlobContent::Text(t) => t,
                BlobContent::Mismatch(detail) => {
                    return Err(ToolError { code: Some("E_BLOB_MISMATCH".into()), detail, body: None })
                }
                BlobContent::NotText => {
                    return Err(ToolError::arg("text: the value is not a text blob of 64 KiB or less"))
                }
                BlobContent::Unavailable(why) => {
                    return Err(ToolError::arg(format!("text: the blob's content is not here: {why}")))
                }
            };
            let (matches, first) = compare_text(quoted, &content);
            members.push(("text_matches", Value::Bool(matches)));
            if let Some(at) = first {
                members.push(("first_difference", num(at as u64)));
            }
        }
        Ok(obj(members))
    }

    fn define(&self, args: &Value) -> ToolResult {
        let name = str_arg(args, "name")?;
        let formula = args.get("formula").and_then(Value::as_str);
        let stored = args.get("stored") == Some(&Value::Bool(true));
        let class = args.get("class");
        let signature = args.get("signature");
        let tests = args.get("tests");
        let (kind, defined) = match (formula, stored, class) {
            (Some(source), false, None) => {
                let sig = signature.ok_or_else(|| ToolError::arg("a formula needs its signature"))?;
                let defined = match tests {
                    Some(t) => self.resolver.define_formula_with_tests(name, source, sig, t)?,
                    None => self.resolver.define_formula(name, source, sig)?,
                };
                ("function", defined)
            }
            (None, true, None) => {
                if tests.is_some() {
                    return Err(ToolError::arg("a stored function takes no tests (its proof is its source)"));
                }
                let sig = signature.ok_or_else(|| ToolError::arg("a stored function needs its signature"))?;
                ("function", self.resolver.define_stored(name, sig)?)
            }
            (None, false, Some(descriptor)) => ("class", self.resolver.define_class(name, &descriptor.to_jcs())?),
            _ => {
                return Err(ToolError::arg(
                    "define takes exactly one of: formula with signature, stored with signature, class",
                ))
            }
        };
        let call = if kind == "class" {
            defined.name.clone()
        } else {
            template(&defined.name, &self.descriptor(&defined.fn_hash))
        };
        let kept = if kind == "class" { 0 } else { self.tests_kept(&defined.fn_hash)? };
        Ok(obj(vec![
            ("name", s(&defined.name)),
            ("hash", s(&defined.fn_hash)),
            ("kind", s(kind)),
            ("call", s(call)),
            ("uri", s(format!("cell://{}", defined.name))),
            ("tests", num(kept as u64)),
        ]))
    }

    /// The number of tests kept with the function `fn_hash`.
    fn tests_kept(&self, fn_hash: &str) -> Result<usize, ToolError> {
        Ok(self.resolver.tests_of(fn_hash)?.len())
    }

    /// `calc` (Build 2 Part 3 addition, C1 to C3): a formula over cells, here or through the forward
    /// mesh (`remote`). Its arguments are `formula`, `asof` and `remote` only.
    /// `interval` (L7, D8): the forward mesh's members of a function over keys and a range.
    fn interval(&self, args: &Value) -> ToolResult {
        if let Value::Object(members) = args {
            if let Some((k, _)) =
                members.iter().find(|(k, _)| !matches!(k.as_str(), "function" | "keys" | "from" | "to"))
            {
                return Err(ToolError::arg(format!("{k}: not an argument of interval")));
            }
        }
        let function = str_arg(args, "function")?;
        let from = str_arg(args, "from")?;
        let to = str_arg(args, "to")?;
        let keys = match args.get("keys") {
            None => Vec::new(),
            Some(Value::Object(m)) => m
                .iter()
                .map(|(k, v)| match v {
                    Value::String(x) => Ok((k.clone(), x.clone())),
                    _ => Err(ToolError::arg(format!("keys.{k}: a string"))),
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(ToolError::arg("keys: an object of key values")),
        };
        Ok(self.resolver.interval(function, &keys, from, to)?)
    }

    fn calc(&self, args: &Value) -> ToolResult {
        if let Value::Object(members) = args {
            if let Some((k, _)) = members.iter().find(|(k, _)| !matches!(k.as_str(), "formula" | "asof" | "remote")) {
                return Err(ToolError::arg(format!("{k}: not an argument of calc")));
            }
        }
        let formula = str_arg(args, "formula")?;
        let asof = match args.get("asof") {
            None => None,
            Some(Value::String(a)) => Some(a.as_str()),
            Some(_) => return Err(ToolError::arg("asof: an instant")),
        };
        let remote = match args.get("remote") {
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(ToolError::arg("remote: a boolean")),
        };
        Ok(cut_lineage(self.resolver.calc(formula, asof, remote)?))
    }

    /// `whatif` (E6): the scenario of the request, from the mesh that answers for the cell or from
    /// this machine's engine. A `not_covered` answer carries its fault.
    fn whatif(&self, args: &Value) -> ToolResult {
        let request = str_arg(args, "request")?.to_string();
        let overrides = match args.get("overrides") {
            None => Vec::new(),
            Some(Value::Array(items)) => items.iter().map(override_arg).collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(ToolError::arg("overrides: an array of {cell, value} or {slice, add, remove}")),
        };
        let asof = match args.get("asof") {
            None => None,
            Some(Value::String(a)) => Some(a.clone()),
            Some(_) => return Err(ToolError::arg("asof: an instant")),
        };
        let solve = match args.get("solve") {
            None => None,
            Some(sv) => {
                let text = |k: &str| {
                    sv.get(k)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| ToolError::arg(format!("solve.{k}: a call")))
                };
                let literal =
                    |k: &str| sv.get(k).cloned().ok_or_else(|| ToolError::arg(format!("solve.{k}: a number")));
                Some(SolveArg {
                    vary: text("vary")?,
                    target: text("target")?,
                    equals: literal("equals")?,
                    lo: literal("lo")?,
                    hi: literal("hi")?,
                })
            }
        };
        let req = WhatifArgs { request, overrides, asof, solve };
        match self.resolver.whatif(&req) {
            Ok(v) => Ok(v),
            Err(Error::Client { code, detail }) if code == "not_covered" => Err(ToolError {
                code: Some("not_covered".into()),
                detail: format!("not_covered: {detail}"),
                body: Some(obj(vec![("error", s("not_covered")), ("fault", s(detail))])),
            }),
            Err(e) => Err(e.into()),
        }
    }

    /// `publish_public` (P1): roots the names under the namespace and posts the bundle to the public
    /// mesh; the mesh's answer is the result.
    fn publish_public(&self, args: &Value) -> ToolResult {
        let to = str_arg(args, "to")?.to_string();
        let namespace = str_arg(args, "namespace")?.to_string();
        let key = str_arg(args, "key")?.to_string();
        let names = match args.get("names") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|n| n.as_str().map(str::to_string).ok_or_else(|| ToolError::arg("names: strings")))
                .collect::<Result<Vec<_>, _>>()?,
            _ => return Err(ToolError::arg("names: an array of names")),
        };
        let export = args.get("export").and_then(Value::as_str).map(PathBuf::from);
        let plan = PublicPlan { to, root: RootPlan { namespace, key_path: PathBuf::from(key), names, export } };
        Ok(self.resolver.publish_public(&plan)?)
    }

    fn publish(&self, args: &Value) -> ToolResult {
        let Some(Value::Array(lines)) = args.get("lines") else {
            return Err(ToolError::arg("lines: an array of publish-stored lines"));
        };
        static PUBLISHES: AtomicU64 = AtomicU64::new(0);
        let path: PathBuf = self.resolver.config().data_dir.join(format!(
            "mcp-publish-{}-{}.jsonl",
            std::process::id(),
            PUBLISHES.fetch_add(1, Ordering::Relaxed)
        ));
        let text: String = lines.iter().map(|l| format!("{}\n", String::from_utf8_lossy(&l.to_jcs()))).collect();
        std::fs::write(&path, text).map_err(|e| ToolError::arg(format!("publish: {e}")))?;
        let done = self.resolver.publish_cells(&path);
        let _ = std::fs::remove_file(&path);
        let p = done?;
        let published = p
            .cells
            .iter()
            .map(|c| obj(vec![("call", s(&c.call)), ("uri", s(&c.uri)), ("receipt_hash", s(&c.receipt_hash))]))
            .collect();
        Ok(obj(vec![
            ("functions", num(p.counts.functions as u64)),
            ("classes", num(p.counts.classes as u64)),
            ("cells", num(p.counts.cells as u64)),
            ("versions", num(p.counts.versions as u64)),
            ("published", Value::Array(published)),
        ]))
    }

    /// The resources of the held names: a name, or a class with its methods.
    fn resources_list(&self) -> RpcResult {
        let mut out = Vec::new();
        for (name, _) in self.resolver.shared.index.names()? {
            match self.class_of(&name)? {
                Some((_, class)) => {
                    out.push(resource(&format!("datumesh://class/{name}"), &name));
                    for m in class.methods.keys() {
                        let full = format!("{name}/{m}");
                        out.push(resource(&format!("datumesh://name/{full}"), &full));
                    }
                }
                None => out.push(resource(&format!("datumesh://name/{name}"), &name)),
            }
        }
        Ok(vec![
            ("resources".into(), Value::Array(out)),
            ("ttlMs".into(), int(TTL_MS)),
            ("cacheScope".into(), s("private")),
        ])
    }

    fn resources_read(&self, params: &Value) -> RpcResult {
        let uri =
            params.get("uri").and_then(Value::as_str).ok_or_else(|| Rpc::new(-32602, "resources/read needs a uri"))?;
        if let Some(sha) = uri.strip_prefix("blob://") {
            return self.blob_read(uri, sha);
        }
        let store = self.resolver.store();
        let text = if let Some(name) = uri.strip_prefix("datumesh://name/") {
            let b = store
                .resolve_binding(name, END)
                .map_err(|e| Error::Store(e.to_string()))?
                .ok_or_else(|| Rpc::new(-32602, format!("Unknown resource: {uri}")))?;
            let doc = obj(vec![
                ("name", s(name)),
                ("fn_hash", s(&b.fn_hash)),
                ("descriptor", self.descriptor(&b.fn_hash)),
                ("bound", s(&b.tx)),
            ]);
            json_text(&doc)
        } else if let Some(class) = uri.strip_prefix("datumesh://class/") {
            let b = store
                .resolve_class_binding(class, END)
                .map_err(|e| Error::Store(e.to_string()))?
                .ok_or_else(|| Rpc::new(-32602, format!("Unknown resource: {uri}")))?;
            let bytes = store
                .class(&b.class_hash)
                .map_err(|e| Error::Store(e.to_string()))?
                .ok_or_else(|| Rpc::new(-32603, format!("class {} is bound but not stored", b.class_hash)))?;
            let doc = obj(vec![
                ("name", s(class)),
                ("class_hash", s(&b.class_hash)),
                ("descriptor", json::parse(&bytes).unwrap_or(Value::Null)),
                ("bound", s(&b.tx)),
            ]);
            json_text(&doc)
        } else if uri.starts_with("cell://") {
            String::from_utf8_lossy(&self.resolver.get(uri)?.body).into_owned()
        } else {
            return Err(Rpc::new(-32602, format!("Unknown resource: {uri}")));
        };
        Ok(vec![(
            "contents".into(),
            Value::Array(vec![obj(vec![("uri", s(uri)), ("mimeType", s("application/json")), ("text", s(text))])]),
        )])
    }

    /// `blob://<sha256>`: the held blob, checked against its SHA-256; text (UTF-8, up to 64 KiB) or
    /// base64 (CONTRACT-1.7a-text L2).
    fn blob_read(&self, uri: &str, sha: &str) -> RpcResult {
        let well_formed = sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !well_formed {
            return Err(Rpc::new(-32602, format!("Malformed blob resource: {uri}")));
        }
        let held = self.resolver.store().blob(sha).map_err(|e| Error::Store(e.to_string()))?;
        let bytes = held
            .map(|b| b.into_owned())
            .ok_or_else(|| Rpc::new(-32602, format!("Unknown resource: {uri} (not held here; get the cell first)")))?;
        if datumesh_core::sha256_hex(&bytes) != sha {
            return Err(Rpc::new(-32603, format!("E_BLOB_MISMATCH: the stored bytes of {uri} do not hash to it")));
        }
        let content = match std::str::from_utf8(&bytes) {
            Ok(text) if bytes.len() as u64 <= TEXT_LIMIT => {
                obj(vec![("uri", s(uri)), ("mimeType", s("text/plain; charset=utf-8")), ("text", s(text))])
            }
            _ => obj(vec![("uri", s(uri)), ("mimeType", s("application/octet-stream")), ("blob", s(base64(&bytes)))]),
        };
        Ok(vec![("contents".into(), Value::Array(vec![content]))])
    }

    /// A class held under `name`: its hash and descriptor, or `None` when `name` is not a class.
    fn class_of(&self, name: &str) -> Result<Option<(String, ClassDescriptor)>, Error> {
        let store = self.resolver.store();
        let Some(b) = store.resolve_class_binding(name, END).map_err(|e| Error::Store(e.to_string()))? else {
            return Ok(None);
        };
        let bytes = store
            .class(&b.class_hash)
            .map_err(|e| Error::Store(e.to_string()))?
            .ok_or_else(|| Error::Store(format!("class {} is bound but not stored", b.class_hash)))?;
        let class = ClassDescriptor::from_json(&bytes).map_err(|e| Error::Store(e.to_string()))?;
        Ok(Some((b.class_hash, class)))
    }

    fn method_entries(&self, class: &str, c: &ClassDescriptor) -> Vec<Value> {
        c.methods
            .iter()
            .map(|(m, h)| {
                let full = format!("{class}/{m}");
                let d = self.descriptor(h);
                let call = template(&full, &d);
                let description = d.get("description").and_then(Value::as_str).unwrap_or_default();
                described(entry_with_call(&full, "method", h, &call), description)
            })
            .collect()
    }

    /// A stored function's descriptor (JSON), or `null` when it is not stored.
    fn descriptor(&self, fn_hash: &str) -> Value {
        self.resolver.store().descriptor(fn_hash).and_then(|b| json::parse(&b).ok()).unwrap_or(Value::Null)
    }
}

/// The tool names, in `tools/list` order.
/// The tools of the small profile.
const SMALL_NAMES: [&str; 4] = ["find", "get", "calc", "interval"];

const TOOL_NAMES: [&str; 12] = [
    "find",
    "get",
    "explain",
    "describe",
    "verify",
    "define",
    "publish",
    "subscribe",
    "whatif",
    "calc",
    "interval",
    "publish_public",
];

/// `_meta` on a request: the protocol version and the client's capabilities (both required).
fn meta_check(params: &Value) -> Result<(), Rpc> {
    let meta = params.get("_meta");
    let version = meta.and_then(|m| m.get("io.modelcontextprotocol/protocolVersion")).and_then(Value::as_str);
    let caps = meta.and_then(|m| m.get("io.modelcontextprotocol/clientCapabilities"));
    let (Some(version), Some(_)) = (version, caps) else {
        return Err(Rpc::new(
            -32602,
            "missing _meta: io.modelcontextprotocol/protocolVersion and io.modelcontextprotocol/clientCapabilities",
        ));
    };
    if version != PROTOCOL {
        return Err(Rpc {
            code: -32022,
            message: format!("unsupported protocol version {version}"),
            data: Some(obj(vec![("supported", Value::Array(vec![s(PROTOCOL)])), ("requested", s(version))])),
        });
    }
    Ok(())
}

impl McpServer {
    /// `server/discover`'s members.
    fn discover(&self) -> Vec<(String, Value)> {
        vec![
            ("supportedVersions".into(), Value::Array(vec![s(PROTOCOL)])),
            ("capabilities".into(), capabilities()),
            ("instructions".into(), s(self.instructions())),
            ("ttlMs".into(), int(TTL_MS)),
            ("cacheScope".into(), s("private")),
        ]
    }
}

/// The capabilities: the tools' list changes when a retry fetches the summary (A4).
fn capabilities() -> Value {
    obj(vec![("tools", obj(vec![("listChanged", Value::Bool(true))])), ("resources", obj(vec![]))])
}

fn server_info() -> Value {
    obj(vec![("name", s("datumesh")), ("version", s(env!("CARGO_PKG_VERSION")))])
}

/// A result: its members, with `resultType` and the server's info (on every result).
fn result_object(mut members: Vec<(String, Value)>) -> Value {
    members.push(("resultType".into(), s("complete")));
    members.push(("_meta".into(), obj(vec![("io.modelcontextprotocol/serverInfo", server_info())])));
    Value::Object(members)
}

/// A response with `payload` (`result` or `error`) to the request `id`, as one line.
fn respond(id: &Value, key: &str, payload: Value) -> String {
    let v = obj(vec![("jsonrpc", s("2.0")), ("id", id.clone()), (key, payload)]);
    String::from_utf8_lossy(&v.to_jcs()).into_owned()
}

fn rpc_error(id: &Value, e: &Rpc) -> String {
    let mut members = vec![("code", int(e.code)), ("message", s(&e.message))];
    if let Some(d) = &e.data {
        members.push(("data", d.clone()));
    }
    respond(id, "error", obj(members))
}

/// The members of a tool result: a text item and the structured content (and `isError`).
fn tool_members(text: &str, structured: Value, is_error: bool) -> Vec<(String, Value)> {
    content_members(vec![text.to_string()], structured, is_error)
}

/// The members of a tool result with one text item per entry of `texts`, in order.
fn content_members(texts: Vec<String>, structured: Value, is_error: bool) -> Vec<(String, Value)> {
    let items = texts.iter().map(|t| obj(vec![("type", s("text")), ("text", s(t))])).collect();
    let mut members = vec![("content".into(), Value::Array(items)), ("structuredContent".into(), structured)];
    if is_error {
        members.push(("isError".into(), Value::Bool(true)));
    }
    members
}

/// A failed tool call's result: its text (`<code>: <detail>`) and `{error, detail}`, with `isError`.
/// The code the MCP tools give an error: its spec code, else `error` (what `whatif` answers the CLI and
/// `serve` with too).
pub(crate) fn tool_code_of(e: Error) -> String {
    ToolError::from(e).code.unwrap_or_else(|| "error".to_string())
}

/// A failed answer (datumesh-local 0.5.3, A2): its code and every detail its structured content carries, as the
/// other tools give `CODE: detail`; the structured content is the answer block, with the envelope and the status.
fn failed(structured: Value, extra: &[String]) -> ToolError {
    let (code, details) = failure_parts(&structured, extra);
    let detail = if details.is_empty() { code.clone() } else { details.join("; ") };
    ToolError { code: Some(code), detail, body: Some(structured) }
}

/// The code of a failed answer and its details: the first `error` (else `outcome`, else the status) and every string
/// under `error`, `fault`, `detail`, `reason` or `outcome` anywhere in `structured`, then `extra`, each once (the
/// bare words `error` and `ok` are not details).
fn failure_parts(structured: &Value, extra: &[String]) -> (String, Vec<String>) {
    fn walk(v: &Value, out: &mut Vec<(String, String)>) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    if let (true, Value::String(t)) =
                        (matches!(k.as_str(), "error" | "fault" | "detail" | "reason" | "outcome"), x)
                    {
                        out.push((k.clone(), t.clone()));
                    }
                    walk(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(structured, &mut found);
    let text_of = |key: &str| found.iter().find(|(k, t)| k == key && t != "ok" && t != "error").map(|(_, t)| t.clone());
    let code = text_of("error")
        .or_else(|| text_of("outcome"))
        .or_else(|| structured.get("status").and_then(Value::as_number_text).map(|n| format!("status {n}")))
        .unwrap_or_else(|| "error".to_string());
    let mut details: Vec<String> = Vec::new();
    for t in found.iter().map(|(_, t)| t).chain(extra.iter()) {
        if t != &code && t != "error" && t != "ok" && !details.contains(t) {
            details.push(t.clone());
        }
    }
    (code, details)
}

fn tool_failure(e: ToolError) -> Vec<(String, Value)> {
    let text = match &e.code {
        Some(c) if !e.detail.starts_with(c.as_str()) => format!("{c}: {}", e.detail),
        _ => e.detail.clone(),
    };
    let structured = e
        .body
        .unwrap_or_else(|| obj(vec![("error", s(e.code.as_deref().unwrap_or("error"))), ("detail", s(e.detail))]));
    tool_members(&text, structured, true)
}

/// The slice answer of a reply (C4): its envelope, when the reply is a slice's.
fn slice_answer_of(reply: &Reply) -> Option<Value> {
    reply.envelope().filter(|v| v.get("outcome").and_then(Value::as_str) == Some(crate::calc::SLICE))
}

/// The detail of an error envelope that is `E_BLOB_MISMATCH` (the resolver's own fetch of a blob
/// failed its check), or `None` for any other reply.
fn blob_mismatch(reply: &Reply) -> Option<String> {
    let envelope = reply.envelope()?;
    if envelope.get("error").and_then(Value::as_str) != Some("E_BLOB_MISMATCH") {
        return None;
    }
    let detail = reply.detail.as_deref().unwrap_or("the blob's content is not the one its value names");
    Some(detail.strip_prefix("E_BLOB_MISMATCH: ").unwrap_or(detail).to_string())
}

/// `members` of an object with `extra` added (a member of the same name replaced).
fn with_members(block: Value, extra: Vec<(&str, Value)>) -> Value {
    let Value::Object(mut m) = block else { return block };
    for (k, v) in extra {
        m.retain(|(name, _)| name != k);
        m.push((k.to_string(), v));
    }
    Value::Object(m)
}

/// The answer block of an envelope (the shape of the `get` tool): the call and URI of a receipt's
/// cell, its trust, signer, signer label, licence, value, receipt hash and resolved, and the whole
/// envelope.
fn answer_block(status: Value, body: &[u8], label: Option<&str>) -> Value {
    let envelope = json::parse(body).unwrap_or(Value::Null);
    let receipt = envelope.get("receipt").and_then(|r| Receipt::from_value(r).ok());
    let (call, uri, signer, licence, value, receipt_hash) = match &receipt {
        Some(r) => (
            s(call_form(r.cell())),
            s(cell_uri(r.cell())),
            s(r.computed_by()),
            s(r.licence()),
            r.value().to_value(),
            s(r.hash()),
        ),
        None => (Value::Null, Value::Null, Value::Null, Value::Null, Value::Null, Value::Null),
    };
    obj(vec![
        ("status", status),
        ("outcome", envelope.get("outcome").cloned().unwrap_or(Value::Null)),
        ("call", call),
        ("uri", uri),
        ("trust", envelope.get("trust").cloned().unwrap_or(Value::Null)),
        ("signer", signer),
        ("signer_label", label.map_or(Value::Null, s)),
        ("licence", licence),
        ("value", value),
        ("receipt_hash", receipt_hash),
        ("resolved", envelope.get("resolved").cloned().unwrap_or(Value::Null)),
        ("envelope", envelope),
    ])
}

/// A function's call template: `<name>(<arg>: <type>, ...)` in its signature's order.
fn template(name: &str, descriptor: &Value) -> String {
    call_template(name, &crate::resolver::signature_args(descriptor))
}

/// A call template: the name and its arguments, each labelled with its type.
fn call_template(name: &str, args: &[(String, String)]) -> String {
    let parts: Vec<String> = args.iter().map(|(k, ty)| format!("{k}: {ty}")).collect();
    format!("{name}({})", parts.join(", "))
}

/// A result entry with its description.
fn described(mut entry: Value, description: &str) -> Value {
    if let Value::Object(m) = &mut entry {
        m.push(("description".into(), s(description)));
    }
    entry
}

/// A result entry with its `publisher` (the host) and, for a relayed function, its `source` (the
/// domain its values come from), as separate members (spec-0.6.1, 16.8).
fn published(mut entry: Value, found: &crate::Found) -> Value {
    if let Value::Object(m) = &mut entry {
        m.push(("publisher".into(), s(&found.publisher)));
        if let Some(source) = &found.source {
            m.push(("source".into(), s(source)));
        }
    }
    entry
}

fn entry_with_call(name: &str, kind: &str, hash: &str, call: &str) -> Value {
    obj(vec![
        ("name", s(name)),
        ("kind", s(kind)),
        ("hash", s(hash)),
        ("call", s(call)),
        ("uri", s(format!("cell://{name}"))),
    ])
}

fn resource(uri: &str, name: &str) -> Value {
    obj(vec![("uri", s(uri)), ("name", s(name)), ("mimeType", s("application/json"))])
}

/// The refusal of a range (R4): the detail `get` gives.
const RANGE_DETAIL: &str =
    "a range is refused, not shortened: read a range verse by verse, or as a slice (a call with * for the key)";

/// What the catalogue summary (`GET /find?summary=1`) builds (R4): what the mesh holds, as one sentence
/// (`held`, the mesh's own under R8, or the text built from its classes and functions), and the instructions.
struct Catalogue {
    held: String,
    instructions: String,
}

impl Catalogue {
    /// The catalogue of a summary; `None` when it has no `held` text and no class or function, or is
    /// not a summary. A `held` string (the mesh's own summary, a whole sentence under CONTRACT-round-0.7
    /// R8) is used verbatim, with nothing added before or after it; without one, the text is "This mesh
    /// (<host>) holds: " and the classes and functions, ending with a full stop.
    fn of(v: &Value) -> Option<Catalogue> {
        let mut members = Vec::new();
        for key in ["classes", "functions"] {
            if let Some(Value::Array(a)) = v.get(key) {
                members.extend(a.iter().map(member_text));
            }
        }
        let mesh = v.get("mesh").and_then(Value::as_str).filter(|m| !m.is_empty()).unwrap_or("this mesh").to_string();
        let held = match v.get("held").and_then(Value::as_str).filter(|h| !h.is_empty()) {
            Some(h) => h.to_string(),
            None if members.is_empty() => return None,
            None => format!("This mesh ({mesh}) holds: {}.", members.join("; ")),
        };
        let instructions = format!(
            "Datumesh: every answer is a signed receipt. {held} Use find to discover names, by words, a name, a label or a value of the lists above. Use get for a cell: a call (name(arg: value, ...)) or a cell:// URI, or a plain reference (a label and a reference, such as \"<label> <reference>\") that the mesh resolves and reads in the same call. For a text value, get gives its text, checked. Use explain for its lineage and verify for a receipt; verify with text checks a quoted text against the cell's text after NFC. define and publish write under this machine's unrooted namespace."
        );
        Some(Catalogue { held, instructions })
    }
}

/// A class or function of the summary, in words: its label, its cell count, and each small
/// dictionary key with its value labels (more than 50 values: their number only).
fn member_text(m: &Value) -> String {
    let text = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or_default().to_string();
    let count = |v: Option<&Value>| v.and_then(Value::as_number_text).and_then(|t| t.parse::<u64>().ok());
    let name = text(m.get("name"));
    let label = Some(text(m.get("label"))).filter(|l| !l.is_empty()).unwrap_or(name);
    let mut parts = Vec::new();
    if let Some(n) = count(m.get("cells")) {
        parts.push(format!("{} cells", thousands(n)));
    }
    if let Some(Value::Object(keys)) = m.get("keys") {
        for (key, k) in keys {
            let distinct = count(k.get("distinct"));
            match (k.get("values"), distinct) {
                (Some(Value::Array(vals)), Some(d)) if d <= 50 && !vals.is_empty() => {
                    let labels: Vec<String> = vals
                        .iter()
                        .take(50)
                        .map(|x| Some(text(x.get("label"))).filter(|l| !l.is_empty()).unwrap_or(text(x.get("value"))))
                        .collect();
                    parts.push(format!("{key}: {}", labels.join(", ")));
                }
                (_, Some(d)) => parts.push(format!("{key}: {} values", thousands(d))),
                _ => {}
            }
        }
    }
    if parts.is_empty() {
        label
    } else {
        format!("{label} ({})", parts.join("; "))
    }
}

/// A count with thousands separators.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Appends `extra` to the description of the tool `name`.
fn append_description(tools: &mut Value, name: &str, extra: &str) {
    let Value::Array(list) = tools else { return };
    for tool in list.iter_mut() {
        if tool.get("name").and_then(Value::as_str) != Some(name) {
            continue;
        }
        if let Value::Object(members) = tool {
            for (k, v) in members.iter_mut() {
                if let (true, Value::String(d)) = (k == "description", v) {
                    d.push(' ');
                    d.push_str(extra);
                }
            }
        }
    }
}

/// The unit a function's descriptor gives its number (`returns.unit`), when it has one.
fn unit_of(descriptor: &Value) -> Option<String> {
    let sig = descriptor.get("signature").unwrap_or(descriptor);
    sig.get("returns")?.get("unit")?.as_str().map(str::to_string)
}

/// A value of a member as text: a string as it is, nothing for a missing or null one, else its JSON.
fn scalar(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(t)) => t.clone(),
        Some(other) => String::from_utf8_lossy(&other.to_jcs()).into_owned(),
    }
}

/// A tool answer's text (A2): a short statement of the answer for the tools that have one, never the JCS of the
/// structured content; the other tools keep their JSON.
fn tool_text(name: &str, v: &Value) -> String {
    match name {
        "find" => find_text(v),
        "explain" => explain_text(v),
        "verify" => verify_text(v),
        "calc" => calc_text(v),
        "interval" => interval_text(v),
        _ => json_text(v),
    }
}

fn find_text(v: &Value) -> String {
    let results: &[Value] = match v.get("results") {
        Some(Value::Array(a)) => a,
        _ => &[],
    };
    if results.is_empty() {
        let message = scalar(v.get("message"));
        return if message.is_empty() { "No results.".to_string() } else { message };
    }
    let mut lines = vec![format!("{} results:", results.len())];
    for r in results.iter().take(FIND_LISTED) {
        lines.push(format!("- {} ({}): {}", scalar(r.get("name")), scalar(r.get("kind")), scalar(r.get("call"))));
    }
    if results.len() > FIND_LISTED {
        lines.push(format!("... and {} more in the structured content.", results.len() - FIND_LISTED));
    }
    lines.join("\n")
}

/// The explain answer's text: the call (or the request) and the page of its inputs; the answer says the rest.
fn explain_text(v: &Value) -> String {
    let what = match scalar(v.get("call")) {
        c if c.is_empty() => "the request".to_string(),
        c => c,
    };
    let num_of = |k: &str| v.get(k).and_then(Value::as_number_text).and_then(|t| t.parse::<usize>().ok());
    let (Some(total), Some(page), Some(limit)) = (num_of("inputs_total"), num_of("page"), num_of("limit")) else {
        return format!("{what}: no inputs to page.");
    };
    let pages = total.div_ceil(limit).max(1);
    let mut text = format!("{what}: {total} inputs, page {page} of {pages} ({limit} to a page).");
    if page < pages {
        text.push_str(&format!(" The next page is explain's page {}.", page + 1));
    }
    text
}

fn verify_text(v: &Value) -> String {
    let call = scalar(v.get("call"));
    let subject = if call.is_empty() { "The receipt".to_string() } else { call };
    let mut text = if v.get("ok") == Some(&Value::Bool(true)) {
        format!(
            "{subject} is verified at level {}, signed by {}, licence {}.",
            scalar(v.get("level")),
            scalar(v.get("signer")),
            scalar(v.get("licence"))
        )
    } else {
        format!("{subject} is not verified: {}.", scalar(v.get("code")))
    };
    match v.get("text_matches") {
        Some(Value::Bool(true)) => text.push_str(" The quoted text matches."),
        Some(Value::Bool(false)) => text.push_str(" The quoted text does not match."),
        _ => {}
    }
    text
}

/// The calc answer's text: the value and unit, the as-of and the receipt's hash, and the lineage's total when cut.
fn calc_text(v: &Value) -> String {
    let unit = match v.get("unit") {
        Some(Value::String(u)) => format!(" {u}"),
        _ => String::new(),
    };
    let mut text = format!("{}{unit}, as of {}.", scalar(v.get("value")), scalar(v.get("asof")));
    let hash = scalar(v.get("receipt_hash"));
    if !hash.is_empty() {
        text.push_str(&format!(" Receipt {hash}."));
    }
    if let Some(total) = v.get("lineage_total") {
        text.push_str(&format!(
            " It reads {} inputs; explain (the pointer in the answer) lists the rest.",
            scalar(Some(total))
        ));
    }
    text
}

fn interval_text(v: &Value) -> String {
    let mut text = format!(
        "{} members of {}, from {} to {}.",
        scalar(v.get("count")),
        scalar(v.get("function")),
        scalar(v.get("from")),
        scalar(v.get("to"))
    );
    if v.get("rest").is_some() {
        text.push_str(" The first are listed; rest gives the call for the others.");
    }
    text
}

/// A slice answer's text: its call, its total, the members this answer gives, the summary's figures and the next page.
fn slice_text(v: &Value) -> String {
    let total = v.get("summary").and_then(|x| x.get("count")).map_or_else(
        || match v.get("members") {
            Some(Value::Array(a)) => a.len().to_string(),
            _ => "0".to_string(),
        },
        |c| scalar(Some(c)),
    );
    let shown = match v.get("members") {
        Some(Value::Array(a)) => a.len(),
        _ => 0,
    };
    let mut text = format!(
        "{} as of {}: {total} members; this answer gives {shown}, each with its call, value and receipt hash.",
        scalar(v.get("slice")),
        scalar(v.get("asof"))
    );
    if let Some(summary) = v.get("summary") {
        text.push_str(&format!(
            " Mean {}, min {}, max {}.",
            scalar(summary.get("mean")),
            scalar(summary.get("min")),
            scalar(summary.get("max"))
        ));
    }
    if let Some(next) = v.get("next") {
        text.push_str(&format!(" The next page: get {}.", scalar(next.get("request"))));
    }
    text
}

/// A describe answer's text: its summary, else its kind and id.
fn describe_text(id: &str, answer: &Value) -> String {
    let summary = scalar(answer.get("summary"));
    if !summary.is_empty() {
        return summary;
    }
    match scalar(answer.get("kind")) {
        kind if kind.is_empty() => format!("{id}."),
        kind => format!("{id}: a {kind}."),
    }
}

/// An optional positive integer argument (`page`, `limit`), at most `max` when given (A3).
fn page_arg(args: &Value, key: &str, default: usize, max: Option<usize>) -> Result<usize, ToolError> {
    match args.get(key) {
        None => Ok(default),
        Some(v) => v
            .as_number_text()
            .and_then(|t| t.parse::<usize>().ok())
            .filter(|n| *n >= 1 && max.is_none_or(|m| *n <= m))
            .ok_or_else(|| match max {
                Some(m) => ToolError::arg(format!("{key}: an integer from 1 to {m}")),
                None => ToolError::arg(format!("{key}: an integer, at least 1")),
            }),
    }
}

/// A tool answer's lineage cut to its first [`LINEAGE_CUT`] entries (A3): `lineage_total` counts them all, and
/// when the answer has its call, `explain` gives the arguments of the page after the cut.
fn cut_lineage(v: Value) -> Value {
    let Value::Object(mut members) = v else { return v };
    let total = match members.iter().find(|(k, _)| k.as_str() == "lineage") {
        Some((_, Value::Array(a))) => a.len(),
        _ => return Value::Object(members),
    };
    if total <= LINEAGE_CUT {
        return Value::Object(members);
    }
    let call = members.iter().find(|(k, _)| k.as_str() == "call").and_then(|(_, c)| c.as_str().map(str::to_string));
    for (k, v) in members.iter_mut() {
        if let (true, Value::Array(a)) = (k.as_str() == "lineage", v) {
            a.truncate(LINEAGE_CUT);
        }
    }
    members.push(("lineage_total".into(), num(total as u64)));
    if let Some(call) = call {
        let pointer = obj(vec![("request", s(call)), ("page", num(2)), ("limit", num(LINEAGE_CUT as u64))]);
        members.push(("explain".into(), pointer));
    }
    Value::Object(members)
}

/// The answer to an ambiguous reference: its candidates, labels and calls, and no value.
fn ambiguous_members(request: &str, candidates: &[Value]) -> Vec<(String, Value)> {
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let mut lines = vec![format!("{request}: {} candidates; no value is read until one is named.", candidates.len())];
    for c in candidates {
        let mut line = format!("- {}", text(c, "label"));
        let call = text(c, "call");
        if !call.is_empty() {
            line.push_str(&format!(": {call}"));
        }
        if let Some(Value::Array(missing)) = c.get("missing_keys") {
            let keys: Vec<&str> = missing.iter().filter_map(Value::as_str).collect();
            line.push_str(&format!(" (missing: {})", keys.join(", ")));
        }
        lines.push(line);
    }
    let structured = obj(vec![("resolution", s("ambiguous")), ("candidates", Value::Array(candidates.to_vec()))]);
    tool_members(&lines.join("\n"), structured, false)
}

/// The failure of a reference nothing resolves to: the find results as suggestions.
/// The reason of a `not_covered` envelope (a withdrawn cell's `withdrawn` text, or a label's lookup reason).
fn not_covered_reason(reply: &Reply) -> Option<String> {
    let envelope = reply.envelope()?;
    if envelope.get("outcome").and_then(Value::as_str) != Some("not_covered") {
        return None;
    }
    envelope.get("reason").and_then(Value::as_str).map(str::to_string)
}

fn not_covered_members(request: &str, results: &[Value]) -> Vec<(String, Value)> {
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let detail = format!("{request}: nothing the find mesh holds resolves to a value for this reference");
    let mut lines = vec![detail.clone()];
    let mut suggestions = Vec::new();
    if !results.is_empty() {
        lines.push("Try one of:".into());
    }
    for r in results {
        let (name, label, call, kind) = (text(r, "name"), text(r, "label"), text(r, "call"), text(r, "kind"));
        lines.push(format!("- {name} ({label}): {call}"));
        suggestions.push(obj(vec![("name", s(name)), ("kind", s(kind)), ("label", s(label)), ("call", s(call))]));
    }
    let structured =
        obj(vec![("error", s("not_covered")), ("detail", s(detail)), ("suggestions", Value::Array(suggestions))]);
    tool_members(&lines.join("\n"), structured, true)
}

/// The comparison of a quoted text with a blob's text, after NFC on both sides: whether they are
/// equal, and when not, the first code point (after NFC) where they differ (or the shorter's length).
fn compare_text(quoted: &str, blob: &str) -> (bool, Option<usize>) {
    let a: Vec<char> = quoted.nfc().collect();
    let b: Vec<char> = blob.nfc().collect();
    if a == b {
        return (true, None);
    }
    (false, Some(a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()))))
}

/// An override of a `whatif` request, from its JSON (`{cell, value}` or `{slice, add, remove}`).
fn override_arg(v: &Value) -> Result<OverrideArg, ToolError> {
    if let Some(slice) = v.get("slice").and_then(Value::as_str) {
        let add = match v.get("add") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|m| {
                    let cell =
                        m.get("cell").and_then(Value::as_str).ok_or_else(|| ToolError::arg("add: {cell, value}"))?;
                    let value = m.get("value").cloned().ok_or_else(|| ToolError::arg("add: {cell, value}"))?;
                    Ok((cell.to_string(), value))
                })
                .collect::<Result<Vec<_>, ToolError>>()?,
            Some(_) => return Err(ToolError::arg("add: an array")),
        };
        let remove = match v.get("remove") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|c| c.as_str().map(str::to_string).ok_or_else(|| ToolError::arg("remove: cells")))
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(ToolError::arg("remove: an array")),
        };
        return Ok(OverrideArg::Slice { slice: slice.to_string(), add, remove });
    }
    let cell = v
        .get("cell")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::arg("an override: {cell, value} or {slice, add, remove}"))?;
    let value = v.get("value").cloned().ok_or_else(|| ToolError::arg("an override: {cell, value}"))?;
    Ok(OverrideArg::Value { cell: cell.to_string(), value })
}

/// The text of a scenario's answer: its first line is the label (E6).
pub(crate) fn scenario_text(v: &Value) -> String {
    let mut lines = vec![format!(
        "Scenario (not a fact): {} = {} as of {}.",
        member_of(v, "call"),
        member_of(v, "value"),
        member_of(v, "asof")
    )];
    if let Some(Value::Array(items)) = v.get("overrides") {
        for o in items {
            lines.push(match o.get("slice") {
                Some(_) => {
                    let added: Vec<String> = match o.get("add") {
                        Some(Value::Array(a)) => {
                            a.iter().map(|m| format!("{} = {}", member_of(m, "cell"), member_of(m, "value"))).collect()
                        }
                        _ => Vec::new(),
                    };
                    format!("  with the slice {} changed: added {}", member_of(o, "slice"), added.join("; "))
                }
                None => format!("  with {} = {}", member_of(o, "cell"), member_of(o, "value")),
            });
        }
    }
    if let Some(sv) = v.get("solved") {
        lines.push(format!("  solved: {} = {}", member_of(sv, "vary"), member_of(sv, "value")));
    }
    lines.push("A scenario is labelled: it is never signed or stored.".to_string());
    lines.join("\n")
}

/// A member of a JSON object as text (strings as they are, other values as JSON).
fn member_of(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(other) => String::from_utf8_lossy(&other.to_jcs()).into_owned(),
        None => String::new(),
    }
}

/// A string member of a JSON object, or empty.
fn s_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| ToolError::arg(format!("{key}: a string is required")))
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn s(v: impl Into<String>) -> Value {
    Value::String(v.into())
}

fn int(n: i64) -> Value {
    json::parse(n.to_string().as_bytes()).unwrap_or(Value::Null)
}

fn num(n: u64) -> Value {
    int(i64::try_from(n).unwrap_or(i64::MAX))
}

/// A constant JSON text (a list or a schema), parsed.
fn json_const(text: &str) -> Value {
    json::parse(text.as_bytes()).unwrap_or(Value::Null)
}

fn json_text(v: &Value) -> String {
    String::from_utf8_lossy(&v.to_jcs()).into_owned()
}

/// Serves MCP over stdio: each line of `input` is a message, its response one line of
/// `output`; returns at end of input. Only MCP messages go to `output`.
pub fn serve_stdio<R: BufRead, W: Write>(resolver: Arc<Resolver>, input: R, output: W) -> Result<(), Error> {
    serve_stdio_with(resolver, Profile::Full, input, output)
}

/// [`serve_stdio`] with the tool surface of `profile`.
pub fn serve_stdio_with<R: BufRead, W: Write>(
    resolver: Arc<Resolver>,
    profile: Profile,
    input: R,
    mut output: W,
) -> Result<(), Error> {
    let server = McpServer::with_profile(resolver, profile);
    let mut send = |line: &str| -> Result<(), Error> {
        output
            .write_all(line.as_bytes())
            .and_then(|()| output.write_all(b"\n"))
            .and_then(|()| output.flush())
            .map_err(|e| Error::Request(format!("stdout: {e}")))
    };
    for line in input.lines() {
        let line = line.map_err(|e| Error::Request(format!("stdin: {e}")))?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = server.handle(&line, Transport::Stdio) {
            send(&reply)?;
        }
        // A retry of the summary that succeeded: the client is told once, after the answer (A4).
        if server.take_notice() {
            let note = obj(vec![("jsonrpc", s("2.0")), ("method", s(LIST_CHANGED))]);
            send(&json_text(&note))?;
        }
    }
    Ok(())
}

/// The token of the HTTP endpoint: 64 lowercase hex digits. `Debug` does not show it.
pub struct Token(String);

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(..)")
    }
}

impl Token {
    /// The token in `path`, made there at first use: 32 bytes from the system's random source,
    /// as 64 hex digits, in a new file of mode 0600. A file with group or other permission bits
    /// is refused ([`Error::Key`]), and so is one that is not a token.
    pub fn load_or_create(path: &Path) -> Result<Token, Error> {
        let err = |e: String| Error::Key(format!("{}: {e}", path.display()));
        if path.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(path).map_err(|e| err(e.to_string()))?.permissions().mode();
                if mode & 0o077 != 0 {
                    return Err(err(format!("mode {:o}: the MCP token must have mode 0600", mode & 0o777)));
                }
            }
            let text = std::fs::read_to_string(path).map_err(|e| err(e.to_string()))?;
            let token = text.trim();
            if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                return Err(err("not a token (64 lowercase hex digits)".into()));
            }
            return Ok(Token(token.to_string()));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| err(e.to_string()))?;
        }
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| err(e.to_string()))?;
        let hex: String = seed.iter().map(|b| format!("{b:02x}")).collect();
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(path).map_err(|e| err(e.to_string()))?;
        f.write_all(format!("{hex}\n").as_bytes()).map_err(|e| err(e.to_string()))?;
        f.sync_all().map_err(|e| err(e.to_string()))?;
        Ok(Token(hex))
    }

    /// Whether `presented` is this token (compared in full, without an early exit).
    fn matches(&self, presented: &str) -> bool {
        let (a, b) = (self.0.as_bytes(), presented.as_bytes());
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

#[derive(Clone)]
struct AppState {
    server: Arc<McpServer>,
    token: Arc<Token>,
}

/// The router of the endpoint `/mcp` (any other path is 404): the checks of the Streamable
/// HTTP transport, in their order (see [`serve_http`]).
pub fn http_router(server: Arc<McpServer>, token: Token) -> Router {
    Router::new().route("/mcp", any(endpoint)).with_state(AppState { server, token: Arc::new(token) })
}

/// A running HTTP endpoint on loopback. Dropping it stops the server.
pub struct HttpServer {
    addrs: Vec<SocketAddr>,
    token_path: PathBuf,
    stop: Option<tokio::sync::watch::Sender<bool>>,
}

impl HttpServer {
    /// The addresses it is bound to (`127.0.0.1`, and `[::1]` when it could be bound).
    pub fn local_addrs(&self) -> Vec<SocketAddr> {
        self.addrs.clone()
    }

    /// The token file's path (its content is never printed).
    pub fn token_path(&self) -> PathBuf {
        self.token_path.clone()
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
    }
}

/// Serves MCP over Streamable HTTP on `127.0.0.1:port` and `[::1]:port` (port 0: a free port).
/// If `[::1]` cannot be bound, the server goes on with 127.0.0.1 alone and writes one line to
/// stderr. The token is read from (or made in) the token file of the configuration directory.
pub fn serve_http(resolver: Arc<Resolver>, port: u16) -> Result<HttpServer, Error> {
    serve_http_with(resolver, Profile::Full, port)
}

/// [`serve_http`] with the tool surface of `profile`.
pub fn serve_http_with(resolver: Arc<Resolver>, profile: Profile, port: u16) -> Result<HttpServer, Error> {
    let token_path = resolver.config().config_dir.join(TOKEN_FILE);
    let token = Token::load_or_create(&token_path)?;
    let v4 = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
        .map_err(|e| Error::Address(format!("127.0.0.1:{port}: {e}")))?;
    let port = v4.local_addr().map(|a| a.port()).map_err(|e| Error::Address(e.to_string()))?;
    let mut listeners = vec![v4];
    match TcpListener::bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port)) {
        Ok(l) => listeners.push(l),
        Err(e) => eprintln!("datumesh mcp: [::1]:{port} is not bound ({e}); serving 127.0.0.1 only"),
    }
    let addrs: Vec<SocketAddr> = listeners.iter().filter_map(|l| l.local_addr().ok()).collect();
    let router = http_router(Arc::new(McpServer::with_profile(resolver, profile)), token);
    let (stop, stopped) = tokio::sync::watch::channel(false);
    std::thread::Builder::new()
        .name("datumesh-mcp-http".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("datumesh mcp: no runtime for the HTTP endpoint: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                let mut tasks = Vec::new();
                for l in listeners {
                    if l.set_nonblocking(true).is_err() {
                        continue;
                    }
                    let Ok(l) = tokio::net::TcpListener::from_std(l) else { continue };
                    let app = router.clone();
                    let mut stopped = stopped.clone();
                    tasks.push(tokio::spawn(async move {
                        let shutdown = async move {
                            let _ = stopped.wait_for(|v| *v).await;
                        };
                        let _ = axum::serve(l, app).with_graceful_shutdown(shutdown).await;
                    }));
                }
                for t in tasks {
                    let _ = t.await;
                }
            });
        })
        .map_err(|e| Error::Address(format!("the HTTP endpoint's thread: {e}")))?;
    Ok(HttpServer { addrs, token_path, stop: Some(stop) })
}

/// The endpoint's checks, in the order of the transport: Host, Origin, the method, the
/// Content-Type, the headers against the body, then the body (answered by [`McpServer::handle`]),
/// the token for `define` and `publish`, and the response.
async fn endpoint(State(app): State<AppState>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if !host.is_some_and(loopback_authority) {
        return text(StatusCode::FORBIDDEN, "the Host header must be a loopback name");
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let ok = origin.to_str().ok().and_then(|o| o.strip_prefix("http://")).is_some_and(loopback_authority);
        if !ok {
            return text(StatusCode::FORBIDDEN, "the Origin is not a loopback http origin");
        }
    }
    if method != Method::POST {
        let mut r = text(StatusCode::METHOD_NOT_ALLOWED, "use POST");
        r.headers_mut().insert(header::ALLOW, "POST".parse().expect("a header value"));
        return r;
    }
    let content_type = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok());
    if !content_type.is_some_and(json_content_type) {
        return text(StatusCode::UNSUPPORTED_MEDIA_TYPE, "the Content-Type must be application/json");
    }
    let Ok(msg) = json::parse(&body) else {
        return json_reply(StatusCode::BAD_REQUEST, rpc_error(&Value::Null, &Rpc::new(-32700, "parse error")));
    };
    if !matches!(msg, Value::Object(_)) {
        return json_reply(StatusCode::BAD_REQUEST, rpc_error(&Value::Null, &Rpc::new(-32600, "invalid request")));
    }
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let method_name = msg.get("method").and_then(Value::as_str).unwrap_or("").to_string();
    let params = msg.get("params").cloned().unwrap_or_else(|| Value::Object(Vec::new()));
    if let Err(message) = check_headers(&headers, &method_name, &params) {
        return json_reply(StatusCode::BAD_REQUEST, rpc_error(&id, &Rpc { code: -32020, message, data: None }));
    }
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| app.token.matches(t.trim()));
    let server = app.server.clone();
    let line = String::from_utf8_lossy(&body).into_owned();
    let reply = match tokio::task::spawn_blocking(move || server.handle(&line, Transport::Http { authorized })).await {
        Ok(reply) => reply,
        Err(_) => return text(StatusCode::INTERNAL_SERVER_ERROR, "the request failed"),
    };
    match reply {
        None => {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::ACCEPTED;
            r
        }
        Some(line) => {
            let code = json::parse(line.as_bytes())
                .ok()
                .and_then(|v| {
                    v.get("error").and_then(|e| e.get("code")).and_then(Value::as_number_text).map(str::to_string)
                })
                .and_then(|c| c.parse::<i64>().ok());
            let status = match code {
                Some(-32601) => StatusCode::NOT_FOUND,
                Some(-32602 | -32022 | -32700 | -32600) => StatusCode::BAD_REQUEST,
                Some(-31001) => StatusCode::UNAUTHORIZED,
                _ => StatusCode::OK,
            };
            let mut r = json_reply(status, line);
            if status == StatusCode::UNAUTHORIZED {
                r.headers_mut().insert(header::WWW_AUTHENTICATE, "Bearer".parse().expect("a header value"));
            }
            r
        }
    }
}

/// The header checks: `MCP-Protocol-Version` (and `Mcp-Method`, `Mcp-Name` for a `tools/call` or a
/// `resources/read`) present and equal to the body's `_meta` version, method, name or uri.
fn check_headers(headers: &HeaderMap, method: &str, params: &Value) -> Result<(), String> {
    let get = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(decoded_header);
    let version = get("mcp-protocol-version").ok_or("MCP-Protocol-Version is required")?;
    let body_version =
        params.get("_meta").and_then(|m| m.get("io.modelcontextprotocol/protocolVersion")).and_then(Value::as_str);
    if body_version.is_some_and(|b| b != version) {
        return Err("MCP-Protocol-Version does not match the body's _meta protocol version".into());
    }
    let mcp_method = get("mcp-method").ok_or("Mcp-Method is required")?;
    if mcp_method != method {
        return Err("Mcp-Method does not match the body's method".into());
    }
    let body_name = match method {
        "tools/call" => params.get("name").and_then(Value::as_str),
        "resources/read" => params.get("uri").and_then(Value::as_str),
        _ => return Ok(()),
    };
    let header_name = get("mcp-name").ok_or("Mcp-Name is required for this method")?;
    if body_name != Some(header_name.as_str()) {
        return Err("Mcp-Name does not match the body's name or uri".into());
    }
    Ok(())
}

/// A header value, decoded from the `=?base64?...?=` form when it is in it.
fn decoded_header(v: &str) -> String {
    v.strip_prefix("=?base64?")
        .and_then(|r| r.strip_suffix("?="))
        .and_then(base64_decode)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| v.to_string())
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for c in text.bytes() {
        if c == b'=' {
            break;
        }
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Whether an authority (`host` or `host:port`, the host a loopback name) is loopback.
fn loopback_authority(a: &str) -> bool {
    let (name, port) = if a.starts_with('[') {
        match a.find(']') {
            Some(close) => (&a[..=close], &a[close + 1..]),
            None => return false,
        }
    } else {
        match a.find(':') {
            Some(i) => (&a[..i], &a[i..]),
            None => (a, ""),
        }
    };
    let name_ok = matches!(name, "127.0.0.1" | "localhost" | "[::1]");
    let port_ok =
        port.is_empty() || (port.len() > 1 && port.starts_with(':') && port[1..].bytes().all(|b| b.is_ascii_digit()));
    name_ok && port_ok
}

/// `application/json`, with at most `; charset=utf-8` (case-insensitive).
fn json_content_type(v: &str) -> bool {
    let mut parts = v.split(';').map(str::trim);
    let media = parts.next().unwrap_or("");
    media.eq_ignore_ascii_case("application/json") && parts.all(|p| p.eq_ignore_ascii_case("charset=utf-8"))
}

fn text(status: StatusCode, message: &str) -> Response {
    let mut r = Response::new(Body::from(message.to_string()));
    *r.status_mut() = status;
    r
}

fn json_reply(status: StatusCode, body: String) -> Response {
    let mut r = Response::new(Body::from(body));
    *r.status_mut() = status;
    r.headers_mut().insert(header::CONTENT_TYPE, "application/json".parse().expect("a header value"));
    r
}
