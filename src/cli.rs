//! The `datumesh` command (the binary of crate `datumesh` is [`main`]).
//!
//! ```text
//! datumesh [--home <dir>] [--offline] <command> ...
//!   get <request>...                   the envelope of each (stdout, one per line)
//!   define <name> --formula <source> --signature <json>
//!   define <name> --stored --signature <json>
//!   define <name> --descriptor <file>
//!   define <name> --class <file>       a class (16.1), under local/ only
//!   publish <file.jsonl>               define plus put_version, under local/ only
//!   describe <id>                      a class, object, cell or short id (16.8), as the endpoint serves it
//!   explain <request> [--depth <n>]    the lineage, as nested envelopes
//!   find <text> [--remote]             names held (and, with --remote, the forward mesh's)
//!   prefetch <slice or function>... [--page <n>] [--blobs]
//!   refresh                            re-pull the latest versions of what is held
//!   export [--out <file>]              the working set as a bundle (stdout by default)
//!   import <file> [--trust-key <key id>]...
//!   verify <file or ->                 SPEC 10.1 with this machine's keys
//!   serve [--listen 127.0.0.1:<port>]  the localhost endpoint (127.0.0.1 only; default port 7733)
//!   mcp                                MCP over stdio (stdout carries only MCP messages)
//!   mcp --http [--port <n>] [--listen <addr>]  MCP over Streamable HTTP on loopback (default port 7734)
//!   key                                the machine id and this machine's key document (public)
//!   key rotate                         a new key from now; the old one retired (CONTRACT-1.8b L4)
//!   trust add <key document file>      trust another machine's key document (its host)
//!   trust revoke <host>                revoke trust from now
//!   trust list                         the trusted machines
//! ```
//!
//! A request is a call (`name(arg: value, ...)`, `N(keys).m(args)`, `N[label]`, short names from the
//! alias map) or a cell URI. `get` writes each envelope exactly as the localhost endpoint serves it on
//! stdout, and a line per answer on stderr: the outcome, where it came from, the signer and its label
//! (`local, verifies only on this machine` for this machine's receipts), and for a call the canonical
//! call it resolved to. Exit status: 0 when every answer carries a receipt, 3 when one does not
//! (`not_covered`, `error`, ...), 2 for a usage error, 1 for a failure. `describe` exits 0 for an
//! answer (200), 3 for any other. `--home` keeps configuration and data in one directory
//! (also `DATUMESH_HOME`); `--offline` (also `DATUMESH_OFFLINE=1`) fetches nothing.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use datumesh_core::json::{self, Value};

use crate::{Config, Error, Resolver, Source};

const USAGE: &str = "usage: datumesh [--home <dir>] [--offline] <get|define|publish|describe|explain|find|prefetch|refresh|export|import|verify|serve|key> ...
  get <request>...
  define <name> (--formula <source> | --stored | --descriptor <file> | --class <file>) [--signature <json>]
  publish <file.jsonl>
  describe <id>
  explain <request> [--depth <n>]
  find <text> [--remote]
  prefetch <slice or function>... [--page <n>] [--blobs]
  refresh
  export [--out <file>]
  import <file> [--trust-key <key id>]...
  verify <file or ->
  serve [--listen 127.0.0.1:<port>]
  mcp [--http] [--port <n>] [--listen <addr>]
  key [rotate]
  trust <add <file>|revoke <host>|list>";

/// Runs the command line (without the program name) with these streams; the exit status.
pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match run_inner(args, out, err) {
        Ok(code) => code,
        Err(Failure::Usage(m)) => {
            let _ = writeln!(err, "{m}\n{USAGE}");
            2
        }
        Err(Failure::Error(e)) => {
            let _ = writeln!(err, "datumesh: {e}");
            1
        }
    }
}

/// The binary's entry: [`run`] with the process's arguments and streams.
pub fn main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    run(&args, &mut stdout.lock(), &mut stderr.lock())
}

enum Failure {
    Usage(String),
    Error(Error),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Failure::Error(e)
    }
}

fn io(e: std::io::Error) -> Failure {
    Failure::Error(Error::Request(e.to_string()))
}

struct Args {
    positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse(args: &[String], valued: &[&str], bare: &[&str]) -> Result<Args, Failure> {
        let mut positional = Vec::new();
        let mut flags = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if valued.contains(&a.as_str()) {
                let v = it.next().ok_or_else(|| Failure::Usage(format!("{a} takes a value")))?;
                flags.push((a.clone(), Some(v.clone())));
            } else if bare.contains(&a.as_str()) {
                flags.push((a.clone(), None));
            } else if a.starts_with("--") {
                return Err(Failure::Usage(format!("unknown option {a}")));
            } else {
                positional.push(a.clone());
            }
        }
        Ok(Args { positional, flags })
    }
    fn value(&self, k: &str) -> Option<&str> {
        self.flags.iter().rev().find(|(f, _)| f == k).and_then(|(_, v)| v.as_deref())
    }
    fn values(&self, k: &str) -> Vec<String> {
        self.flags.iter().filter(|(f, _)| f == k).filter_map(|(_, v)| v.clone()).collect()
    }
    fn has(&self, k: &str) -> bool {
        self.flags.iter().any(|(f, _)| f == k)
    }
}

fn s(v: impl Into<String>) -> Value {
    Value::String(v.into())
}

/// A trusted machine as the `trust` commands print it.
fn trusted_json(m: &crate::TrustedMachine) -> Value {
    Value::Object(vec![
        ("host".into(), s(m.host.clone())),
        ("revoked_at".into(), m.revoked_at.clone().map_or(Value::Null, s)),
        ("trusted_from".into(), s(m.trusted_from.clone())),
    ])
}

fn jline(out: &mut dyn Write, v: Value) -> Result<(), Failure> {
    out.write_all(&v.to_jcs()).map_err(io)?;
    out.write_all(b"\n").map_err(io)
}

fn run_inner(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> Result<i32, Failure> {
    let mut i = 0;
    let mut home: Option<PathBuf> = None;
    let mut offline = false;
    while i < args.len() {
        match args[i].as_str() {
            "--home" => {
                home =
                    Some(PathBuf::from(args.get(i + 1).ok_or_else(|| Failure::Usage("--home takes a value".into()))?));
                i += 2;
            }
            "--offline" => {
                offline = true;
                i += 1;
            }
            "-h" | "--help" | "help" => {
                writeln!(out, "{USAGE}").map_err(io)?;
                return Ok(0);
            }
            "--version" | "version" => {
                writeln!(out, "datumesh {}", env!("CARGO_PKG_VERSION")).map_err(io)?;
                return Ok(0);
            }
            _ => break,
        }
    }
    let Some(cmd) = args.get(i) else { return Err(Failure::Usage("no command".into())) };
    let rest = &args[i + 1..];
    let mut config = match home {
        Some(h) => Config::in_dir(h)?,
        None => Config::load()?,
    };
    if offline {
        config.offline = true;
    }
    let r = Resolver::open(config)?;
    match cmd.as_str() {
        "get" => {
            let a = Args::parse(rest, &[], &[])?;
            if a.positional.is_empty() {
                return Err(Failure::Usage("get: no request".into()));
            }
            let mut code = 0;
            for q in &a.positional {
                let reply = r.get(q)?;
                out.write_all(&reply.body).map_err(io)?;
                out.write_all(b"\n").map_err(io)?;
                let receipt = reply.receipt();
                if receipt.is_none() {
                    code = 3;
                }
                let source = match &reply.source {
                    Source::Local => "local store".to_string(),
                    Source::Fetched => "fetched".to_string(),
                    Source::Forwarded { mesh } => format!("forwarded to {mesh}"),
                    Source::Unavailable => "not held, not fetched".to_string(),
                };
                let mut line = format!("{} ({source})", reply.outcome().unwrap_or_default());
                if let Some(call) = reply.envelope().and_then(|v| {
                    v.get("resolved").and_then(|r| r.get("call")).and_then(Value::as_str).map(str::to_string)
                }) {
                    line.push_str(&format!("; as {call}"));
                }
                if let Some(rc) = receipt {
                    line.push_str(&format!("; signer {}", rc.computed_by()));
                }
                if let Some(l) = &reply.label {
                    line.push_str(&format!(" ({l})"));
                }
                if let Some(d) = &reply.detail {
                    line.push_str(&format!("; {d}"));
                }
                writeln!(err, "{line}").map_err(io)?;
            }
            Ok(code)
        }
        "define" => {
            let a = Args::parse(rest, &["--formula", "--signature", "--descriptor", "--class"], &["--stored"])?;
            let [name] = a.positional.as_slice() else {
                return Err(Failure::Usage("define: one name".into()));
            };
            let sig = match a.value("--signature") {
                Some(t) => Some(json::parse(t.as_bytes()).map_err(|e| Failure::Usage(format!("--signature: {e}")))?),
                None => None,
            };
            let d = if let Some(src) = a.value("--formula") {
                r.define_formula(name, src, &sig.ok_or_else(|| Failure::Usage("--formula needs --signature".into()))?)?
            } else if a.has("--stored") {
                r.define_stored(name, &sig.ok_or_else(|| Failure::Usage("--stored needs --signature".into()))?)?
            } else if let Some(f) = a.value("--descriptor") {
                r.define(name, &std::fs::read(f).map_err(io)?)?
            } else if let Some(f) = a.value("--class") {
                r.define_class(name, &std::fs::read(f).map_err(io)?)?
            } else {
                return Err(Failure::Usage("define: --formula, --stored, --descriptor or --class".into()));
            };
            jline(
                out,
                Value::Object(vec![
                    ("bound".into(), Value::Bool(d.bound)),
                    ("fn_hash".into(), s(d.fn_hash)),
                    ("name".into(), s(d.name)),
                ]),
            )?;
            Ok(0)
        }
        "publish" => {
            let a = Args::parse(rest, &[], &[])?;
            let [file] = a.positional.as_slice() else {
                return Err(Failure::Usage("publish: one file".into()));
            };
            let p = r.publish(std::path::Path::new(file))?;
            jline(
                out,
                Value::Object(vec![
                    ("cells".into(), s(p.cells.to_string())),
                    ("functions".into(), s(p.functions.to_string())),
                    ("versions".into(), s(p.versions.to_string())),
                ]),
            )?;
            Ok(0)
        }
        "describe" => {
            let a = Args::parse(rest, &[], &[])?;
            let [id] = a.positional.as_slice() else {
                return Err(Failure::Usage("describe: one id".into()));
            };
            let reply = r.describe(id)?;
            out.write_all(&reply.body).map_err(io)?;
            out.write_all(b"\n").map_err(io)?;
            Ok(if reply.status == 200 { 0 } else { 3 })
        }
        "explain" => {
            let a = Args::parse(rest, &["--depth"], &[])?;
            let [q] = a.positional.as_slice() else {
                return Err(Failure::Usage("explain: one request".into()));
            };
            let depth = match a.value("--depth") {
                Some(d) => Some(d.parse().map_err(|_| Failure::Usage("--depth: a number".into()))?),
                None => None,
            };
            let reply = r.explain(q, depth)?;
            out.write_all(&reply.body).map_err(io)?;
            out.write_all(b"\n").map_err(io)?;
            Ok(if reply.status == 200 { 0 } else { 3 })
        }
        "find" => {
            let a = Args::parse(rest, &[], &["--remote"])?;
            let [q] = a.positional.as_slice() else {
                return Err(Failure::Usage("find: one query".into()));
            };
            let found = r.find(q, a.has("--remote"))?;
            let results = found
                .into_iter()
                .map(|f| {
                    Value::Object(vec![
                        ("fn_hash".into(), s(f.fn_hash)),
                        ("held".into(), Value::Bool(f.held)),
                        ("name".into(), s(f.name)),
                    ])
                })
                .collect();
            jline(out, Value::Object(vec![("query".into(), s(q.clone())), ("results".into(), Value::Array(results))]))?;
            Ok(0)
        }
        "prefetch" => {
            let a = Args::parse(rest, &["--page"], &["--blobs"])?;
            if a.positional.is_empty() {
                return Err(Failure::Usage("prefetch: no slice or function".into()));
            }
            let page = match a.value("--page") {
                Some(n) => Some(n.parse().map_err(|_| Failure::Usage("--page: a number".into()))?),
                None => None,
            };
            for t in &a.positional {
                let p = r.prefetch(t, page, a.has("--blobs"))?;
                jline(
                    out,
                    Value::Object(vec![
                        ("blobs".into(), s(p.blobs.to_string())),
                        ("members".into(), s(p.members.to_string())),
                        ("names".into(), s(p.names.to_string())),
                        ("pages".into(), s(p.pages.to_string())),
                        ("target".into(), s(p.target)),
                        ("versions".into(), s(p.versions.to_string())),
                    ]),
                )?;
            }
            Ok(0)
        }
        "refresh" => {
            Args::parse(rest, &[], &[])?;
            let f = r.refresh()?;
            let failed = f
                .failed
                .iter()
                .map(|(w, d)| Value::Object(vec![("detail".into(), s(d.clone())), ("what".into(), s(w.clone()))]))
                .collect();
            jline(
                out,
                Value::Object(vec![
                    ("cells".into(), s(f.cells.to_string())),
                    ("failed".into(), Value::Array(failed)),
                    ("forwarded".into(), s(f.forwarded.to_string())),
                    ("names".into(), s(f.names.to_string())),
                    ("slices".into(), s(f.slices.to_string())),
                    ("versions".into(), s(f.versions.to_string())),
                ]),
            )?;
            Ok(if f.failed.is_empty() { 0 } else { 3 })
        }
        "export" => {
            let a = Args::parse(rest, &["--out"], &[])?;
            let b = r.export()?;
            match a.value("--out") {
                Some(p) => std::fs::write(p, &b).map_err(io)?,
                None => {
                    out.write_all(&b).map_err(io)?;
                    out.write_all(b"\n").map_err(io)?;
                }
            }
            Ok(0)
        }
        "import" => {
            let a = Args::parse(rest, &["--trust-key"], &[])?;
            let [file] = a.positional.as_slice() else {
                return Err(Failure::Usage("import: one file".into()));
            };
            let bytes = std::fs::read(file).map_err(io)?;
            let im = r.import(&bytes, &a.values("--trust-key"))?;
            let list = |v: &[String]| Value::Array(v.iter().map(|x| s(x.clone())).collect());
            jline(
                out,
                Value::Object(vec![
                    ("bindings".into(), s(im.bindings.to_string())),
                    ("blobs".into(), s(im.blobs.to_string())),
                    ("computed".into(), s(im.computed.to_string())),
                    ("descriptors".into(), s(im.descriptors.to_string())),
                    ("failed".into(), list(&im.failed)),
                    ("forwarded".into(), s(im.forwarded.to_string())),
                    ("manifests".into(), s(im.manifests.to_string())),
                    ("quarantined".into(), list(&im.quarantined)),
                    ("receipts".into(), s(im.receipts.to_string())),
                    ("trusted".into(), list(&im.trusted)),
                    ("versions".into(), s(im.versions.to_string())),
                ]),
            )?;
            for q in &im.quarantined {
                writeln!(err, "{q}: {}", crate::UNVERIFIABLE_LABEL).map_err(io)?;
            }
            Ok(0)
        }
        "verify" => {
            let a = Args::parse(rest, &[], &[])?;
            let [file] = a.positional.as_slice() else {
                return Err(Failure::Usage("verify: one file (or -)".into()));
            };
            let bytes = if file == "-" {
                let mut b = Vec::new();
                std::io::Read::read_to_end(&mut std::io::stdin(), &mut b).map_err(io)?;
                b
            } else {
                std::fs::read(file).map_err(io)?
            };
            // An envelope's receipt, or a receipt.
            let receipt = json::parse(&bytes).ok().and_then(|v| v.get("receipt").map(Value::to_jcs)).unwrap_or(bytes);
            let v = r.verify(&receipt);
            let ok = matches!(v.get("ok"), Some(Value::Bool(true)));
            jline(out, v)?;
            Ok(if ok { 0 } else { 3 })
        }
        "serve" => {
            let a = Args::parse(rest, &["--listen"], &[])?;
            let addr: std::net::SocketAddr = a
                .value("--listen")
                .unwrap_or("127.0.0.1:7733")
                .parse()
                .map_err(|_| Failure::Usage("--listen: an address, 127.0.0.1:<port>".into()))?;
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| Failure::Error(Error::Request(e.to_string())))?;
            let r = Arc::new(r);
            rt.block_on(async move {
                let server = crate::serve::Server::bind(r, addr).await?;
                writeln!(err, "datumesh: serving http://{}", server.local_addr()).map_err(io)?;
                err.flush().map_err(io)?;
                server
                    .run(async {
                        let _ = tokio::signal::ctrl_c().await;
                    })
                    .await
                    .map_err(io)
            })?;
            Ok(0)
        }
        "mcp" => {
            let a = Args::parse(rest, &["--port", "--listen"], &["--http"])?;
            if !a.has("--http") {
                if a.has("--port") || a.has("--listen") {
                    return Err(Failure::Usage("--port and --listen go with --http".into()));
                }
                // Stdout carries the MCP messages and nothing else.
                let stdin = std::io::stdin();
                crate::mcp::serve_stdio(Arc::new(r), stdin.lock(), &mut *out)?;
                return Ok(0);
            }
            let mut port: u16 = match a.value("--port") {
                Some(p) => p.parse().map_err(|_| Failure::Usage("--port: a port number".into()))?,
                None => 7734,
            };
            if let Some(l) = a.value("--listen") {
                let (ip, at) = match l.parse::<std::net::SocketAddr>() {
                    Ok(sa) => (sa.ip(), Some(sa.port())),
                    Err(_) => (
                        l.parse::<std::net::IpAddr>().map_err(|_| Failure::Usage("--listen: an address".into()))?,
                        None,
                    ),
                };
                if !ip.is_loopback() {
                    return Err(Error::Address(format!("{l}: the MCP endpoint listens on loopback only")).into());
                }
                port = at.unwrap_or(port);
            }
            let server = crate::mcp::serve_http(Arc::new(r), port)?;
            for addr in server.local_addrs() {
                writeln!(err, "datumesh mcp: http://{addr}/mcp").map_err(io)?;
            }
            writeln!(err, "datumesh mcp: token file {}", server.token_path().display()).map_err(io)?;
            err.flush().map_err(io)?;
            // Serves until the process is stopped.
            loop {
                std::thread::park();
            }
        }
        "key" => {
            let a = Args::parse(rest, &[], &[])?;
            match a.positional.as_slice() {
                [] => jline(
                    out,
                    Value::Object(vec![
                        ("key_document".into(), json::parse(&r.key_document()).unwrap_or(Value::Null)),
                        ("machine".into(), s(r.key_id())),
                    ]),
                )?,
                [c] if c == "rotate" => {
                    let rot = r.rotate_key()?;
                    jline(
                        out,
                        Value::Object(vec![
                            ("at".into(), s(rot.at)),
                            ("new_key_id".into(), s(rot.new_key_id)),
                            ("old_key_id".into(), s(rot.old_key_id)),
                        ]),
                    )?
                }
                _ => return Err(Failure::Usage("key: no argument, or rotate".into())),
            }
            Ok(0)
        }
        "trust" => {
            let a = Args::parse(rest, &[], &[])?;
            match a.positional.as_slice() {
                [c, file] if c == "add" => {
                    let m = r.trust_machine(&std::fs::read(file).map_err(io)?)?;
                    jline(out, trusted_json(&m))?;
                }
                [c, host] if c == "revoke" => {
                    r.revoke_machine(host)?;
                    let m = r.trusted_machines()?.into_iter().find(|m| &m.host == host);
                    jline(out, m.as_ref().map_or(Value::Null, trusted_json))?;
                }
                [c] if c == "list" => {
                    let list = r.trusted_machines()?.iter().map(trusted_json).collect();
                    jline(out, Value::Object(vec![("machines".into(), Value::Array(list))]))?;
                }
                _ => return Err(Failure::Usage("trust: add <file>, revoke <host> or list".into())),
            }
            Ok(0)
        }
        other => Err(Failure::Usage(format!("unknown command {other}"))),
    }
}
