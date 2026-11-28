//! Explicit fetches and the import into the store (the `Foreign` layer pattern: the HTTP
//! client fetches and checks, then the store's `put_*` checks again and keeps), and the
//! runtime layer through which every evaluation reads: foreign inputs fetched when allowed,
//! and every input receipt shown to the licence gate before its value is used.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use datumesh_core::json::{self, Value};
use datumesh_core::{
    Arg, Args, Cell, ErrorCode, ForeignDetail, FunctionDescriptor, KeyDocument, Keys, LicenceReason, Receipt,
    Reference, Request, Scalar, Type, Withdrawal,
};
use datumesh_engine::{Budget, Clock, EvalError, FnBinding, Input, Inputs, Member, Pattern, Runtime, SlicePattern};
use datumesh_http::client::{pct, Client, ClientError, Fetched};
use datumesh_store::Store;

use crate::acceptance::{accept_hint, is_restricted, Acceptance};
use crate::config::Config;
use crate::gate::{is_unrooted, GateDecision, LicenceGate};
use crate::index::{HeldCell, Index};
use crate::key::MachineKey;

/// The latest instant.
pub(crate) const END: &str = "9999-12-31T23:59:59.999999Z";
/// The earliest instant.
pub(crate) const START: &str = "0001-01-01T00:00:00.000000Z";

/// What the resolver's parts share.
pub(crate) struct Shared {
    pub(crate) config: Config,
    /// Where every "now" of the resolver comes from (the system clock unless opened otherwise).
    pub(crate) clock: Arc<dyn Clock + Send + Sync>,
    /// This machine's key (replaced by a rotation).
    pub(crate) machine: RwLock<Arc<MachineKey>>,
    pub(crate) store: Arc<dyn Store + Send + Sync>,
    pub(crate) client: Client,
    pub(crate) index: Index,
    pub(crate) gate: LicenceGate,
    /// The licences this machine accepted (`licences.json`, L4).
    pub(crate) acceptance: Acceptance,
    pub(crate) offline: AtomicBool,
}

/// The host of a function name or a URI rest.
pub(crate) fn host_of(name: &str) -> &str {
    name.split(['/', '?', '(', '@']).next().unwrap_or(name)
}

/// The function name of a request (a call or a URI rest): before `(`, `@` or `?`.
pub(crate) fn name_of(rest: &str) -> &str {
    let end = rest.find(['(', '@', '?']).unwrap_or(rest.len());
    rest[..end].trim()
}

pub(crate) fn store_err(e: impl std::fmt::Display) -> ClientError {
    ClientError { code: ErrorCode::InputUnavailable, detail: format!("store: {e}") }
}

/// The failure of a fetch that reached no answer (the network's, not another mesh's), named
/// as `E_FOREIGN_UNAVAILABLE`'s detail: `dns`, `tls`, `timeout`, `refused` or `network`.
/// `None` for an answer that came back (a status, a check that failed).
/// A client error of `code` with `detail`.
fn ce(code: ErrorCode, detail: impl Into<String>) -> ClientError {
    ClientError { code, detail: detail.into() }
}

/// The `pending` envelope of a `202` answer, if it is one.
fn pending_of(f: &Fetched) -> Option<datumesh_core::Pending> {
    if f.status != 202 {
        return None;
    }
    f.answer.pending.clone()
}

/// Whether a body is the mesh's `E_JOB_UNKNOWN` error envelope (a job it no longer holds).
fn is_job_unknown_body(body: &[u8]) -> bool {
    json::parse(body).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
        == Some(ErrorCode::JobUnknown.as_str().to_string())
}

/// Whether an answer is the mesh's `E_JOB_UNKNOWN` (a job it no longer holds).
fn is_job_unknown(f: &Fetched) -> bool {
    f.status == 400 && is_job_unknown_body(&f.body)
}

pub(crate) fn network_failure(e: &ClientError) -> Option<String> {
    // The client's own (typed) classification, where it gives one (http 0.2.1: the
    // publisher's key document): its detail starts with the kind. Its catch-all `network` is
    // refined by the text where that names the failure: http 0.2.2 calls the Windows
    // resolver's error (os error 11001-11004) `network`.
    if let Some(kind) = e.foreign_detail() {
        if kind == datumesh_core::ForeignDetail::Network {
            let why = e.detail.split_once(": ").map(|(_, w)| w).unwrap_or("");
            if let Some(better) = transport_kind(why).filter(|k| *k != kind) {
                return Some(format!("{}: {why}", better.as_str()));
            }
        }
        return Some(e.detail.clone());
    }
    if e.code != ErrorCode::InputUnavailable && e.code != ErrorCode::KeyUnavailable && e.code != ErrorCode::MeshDoc {
        return None;
    }
    let d = e.detail.to_ascii_lowercase();
    if d.contains("is not an allowed host") {
        return Some(format!("not allowed: {}", e.detail));
    }
    if d.contains(": status ") || d.contains("store:") || d.starts_with("store") {
        return None;
    }
    let kind = transport_kind(&e.detail)?.as_str();
    Some(format!("{kind}: {}", e.detail))
}

/// The failure of a fetch that reached no answer, from the client's error text
/// (`<url>: <ureq error>`; the URL does not count): `None` when an answer came back.
///
/// datumesh-http's client hands ureq's error over as text, so this reads the text: ureq's
/// own variants (`host not found`, `timeout: ...`, `rustls: ...`, `connection failed`) and
/// the standard library's I/O errors as each platform words them, with the Windows Sockets
/// codes (11001-11004 for the resolver, 10060 a timeout, 10061 refused).
pub(crate) fn transport_kind(detail: &str) -> Option<ForeignDetail> {
    let err = if detail.starts_with("http://") || detail.starts_with("https://") {
        detail.split_once(": ").map(|(_, e)| e).unwrap_or("")
    } else {
        detail
    };
    let d = err.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| d.contains(w));
    if has(&["status ", "store:", "history", "does not extend"]) || d.starts_with("store") {
        return None;
    }
    let kind = if has(&[
        "host not found",
        "failed to lookup address",
        "name or service not known",
        "name resolution",
        "nodename nor servname",
        "no address associated",
        "no such host",
        "hostname resolution",
        "no data of the requested type",
        "database lookup",
        "os error 11001",
        "os error 11002",
        "os error 11003",
        "os error 11004",
        "dns",
    ]) {
        ForeignDetail::Dns
    } else if has(&["rustls", "tls", "certificate"]) {
        ForeignDetail::Tls
    } else if has(&["timeout", "timed out", "did not properly respond", "os error 10060"]) {
        ForeignDetail::Timeout
    } else if has(&["refused", "os error 10061"]) {
        ForeignDetail::Refused
    } else if has(&["io:", "connection", "network", "unreachable", "reset", "forcibly closed"]) {
        ForeignDetail::Network
    } else {
        return None;
    };
    Some(kind)
}

/// What happened during one request's evaluation (per thread: the mesh evaluates a request
/// on the thread that asks).
#[derive(Default, Debug)]
pub(crate) struct Ctx {
    /// The gate forwards the request (a declared restricted input): why.
    pub(crate) refused: Option<String>,
    /// The gate refuses the request (`unlicensed` here): why.
    pub(crate) unlicensed: Option<String>,
    /// A foreign input could not be fetched: why.
    pub(crate) unavailable: Option<String>,
    /// Another mesh answered a fetch with a failure.
    pub(crate) remote: Option<ClientError>,
    /// Cells whose versions were imported.
    pub(crate) added: Vec<String>,
    /// The restricted licences of the inputs the evaluation read (L4: the engine's acceptance rule
    /// needs every one of them accepted).
    pub(crate) restricted: Vec<String>,
    /// Members of foreign slices fetched for this evaluation and not yet kept (datumesh-local 0.5.2,
    /// item 12): read from memory by the run that fetched them, kept before the evaluation runs for good.
    pub(crate) pending: Vec<PendingSlice>,
    /// The members fetched for slices so far in this evaluation, summed (the bound's count).
    pub(crate) count: u64,
    /// The slices kept already by this evaluation (their keys): read from the store.
    pub(crate) satisfied: Vec<String>,
    /// A slice the run reads and has not fetched: the run stops, and this is fetched before it runs again.
    pub(crate) needed: Option<Need>,
    /// The run read a pending slice from memory: its answer is not kept (see [`Gated::evaluate`]).
    pub(crate) served: bool,
}

/// A foreign slice a run reads and has not fetched (item 12).
#[derive(Debug, Clone)]
pub(crate) struct Need {
    /// The function (full name).
    pub(crate) fn_name: String,
    /// The read's pattern.
    pub(crate) pattern: SlicePattern,
    /// The read's key (the range's ends for a range, else the whole slice's rest).
    pub(crate) key: String,
    /// The whole slice's rest.
    pub(crate) whole: String,
    /// Fetched as a range (`/x/interval`) rather than the whole slice's pages.
    pub(crate) interval: bool,
}

/// Why an evaluation with slices stops short of an answer (item 12).
#[derive(Debug)]
pub(crate) enum Stop {
    /// The bound: why (`E_SLICE_FETCH_LIMIT`).
    Limit(String),
    /// No answer from the network: why.
    Unavailable(String),
    /// Another mesh's answer failed, or failed its check, or the store refused a kept member.
    Remote(ClientError),
}

impl Stop {
    /// The reply an endpoint or the tool gives for this stop: the limit's refusal (`E_SLICE_FETCH_LIMIT`, 422),
    /// the network's (`E_FOREIGN_UNAVAILABLE`), or another mesh's failure with its code.
    pub(crate) fn into_reply(self) -> crate::resolver::Reply {
        match self {
            Stop::Limit(why) => {
                let mut r = crate::resolver::Reply::new(
                    422,
                    br#"{"error":"E_SLICE_FETCH_LIMIT","outcome":"error"}"#.to_vec(),
                    crate::resolver::Source::Local,
                );
                r.detail = Some(why);
                r
            }
            Stop::Unavailable(why) => crate::resolver::Reply::unavailable(why),
            Stop::Remote(e) => crate::resolver::Reply::error(e.code, crate::resolver::rejection_detail(&e)),
        }
    }

    /// The error a tool or the CLI reports for this stop.
    pub(crate) fn into_error(self) -> crate::Error {
        match self {
            Stop::Limit(why) => crate::Error::Client { code: "E_SLICE_FETCH_LIMIT".into(), detail: why },
            Stop::Unavailable(why) => crate::Error::ForeignUnavailable(why),
            Stop::Remote(e) => crate::Error::Client { code: e.code.as_str().to_string(), detail: e.detail },
        }
    }
}

/// The members one read of a foreign slice fetched, not yet kept.
#[derive(Debug)]
pub(crate) struct PendingSlice {
    /// The fetch's key: the slice's rest as it was asked (`name(*)` for a whole slice).
    pub(crate) key: String,
    /// The publisher's host.
    pub(crate) host: String,
    /// The function (full name).
    pub(crate) fn_name: String,
    /// Kept as a prefetched slice (`hold_slice`) when the whole slice was fetched.
    pub(crate) whole: Option<String>,
    /// The members, each verified.
    pub(crate) members: Vec<Receipt>,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

/// Runs `f` with a fresh context, returning what it noted.
pub(crate) fn with_ctx<T>(f: impl FnOnce() -> T) -> (T, Ctx) {
    with_ctx_from(Ctx::default(), f)
}

/// Runs `f` with the context `init`, returning what it noted.
fn with_ctx_from<T>(init: Ctx, f: impl FnOnce() -> T) -> (T, Ctx) {
    let saved = CTX.with(|c| c.borrow_mut().replace(init));
    let out = f();
    let ctx = CTX.with(|c| std::mem::replace(&mut *c.borrow_mut(), saved)).unwrap_or_default();
    (out, ctx)
}

fn note(f: impl FnOnce(&mut Ctx)) {
    CTX.with(|c| {
        if let Some(ctx) = c.borrow_mut().as_mut() {
            f(ctx)
        }
    })
}

/// What the evaluation in progress says, when one is in progress.
fn noted<T>(f: impl FnOnce(&Ctx) -> T) -> Option<T> {
    CTX.with(|c| c.borrow().as_ref().map(f))
}

/// The rest (without `cell://`) of a cell's URI, no pin, no asof.
pub(crate) fn cell_rest(cell: &Cell) -> String {
    let r = Reference::new(cell.ns.clone(), cell.fn_name.clone(), cell.args.clone());
    datumesh_core::format_reference(&r)["cell://".len()..].to_string()
}

impl Shared {
    pub(crate) fn now(&self) -> String {
        self.clock.now()
    }

    /// This machine's current key.
    pub(crate) fn key(&self) -> Arc<MachineKey> {
        self.machine.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Whether fetching from `host` is allowed now: `Err` names why not.
    pub(crate) fn may_fetch(&self, host: &str) -> Result<(), String> {
        if self.offline.load(Ordering::Acquire) {
            return Err("offline: the resolver is offline".into());
        }
        if !self.client.allowed(host) {
            return Err(format!("not allowed: {host} is not in the allowlist"));
        }
        Ok(())
    }

    /// Runs a fetch, waiting out the other mesh's rate limit (429) a few times.
    pub(crate) fn patient<T>(&self, f: impl Fn() -> Result<T, ClientError>) -> Result<T, ClientError> {
        let mut wait = 1u64;
        for _ in 0..6 {
            match f() {
                Err(e) if e.detail.contains("status 429") => {
                    std::thread::sleep(Duration::from_secs(wait));
                    wait = (wait * 2).min(30);
                }
                r => return r,
            }
        }
        f()
    }

    /// A cell's envelope from `host`, waiting out a `rate_limited` answer a few times.
    fn send_cell(&self, host: &str, reference: &Reference) -> Result<Fetched, ClientError> {
        let mut wait = 1u64;
        for _ in 0..6 {
            let f = self.client.reference(host, reference)?;
            if f.status != 429 {
                return Ok(f);
            }
            let after = f.answer.retry_after_seconds.unwrap_or(wait).clamp(1, 60);
            std::thread::sleep(Duration::from_secs(after));
            wait = (wait * 2).min(30);
        }
        self.client.reference(host, reference)
    }

    /// A cell's envelope from `host`, following its job (CONTRACT-patch-0.6.1 4c).
    pub(crate) fn fetch_cell(&self, host: &str, reference: &Reference) -> Result<Fetched, ClientError> {
        self.follow_job(host, || self.send_cell(host, reference))
    }

    /// The one poll-following helper of `/cell` requests (4c). `send` makes the request. On a `202` with `poll`,
    /// the job's URL is polled, waiting the answer's estimate clamped to 1..10 s before each poll, until the job
    /// answers something else. The original request is not sent again while the job is known; when the mesh no
    /// longer knows the job (`E_JOB_UNKNOWN`, a restart), it is sent once more, and a second `E_JOB_UNKNOWN` ends
    /// the wait with that answer. At the configured `timeout` (counted from the first request) the last `202` is
    /// returned as it came.
    pub(crate) fn follow_job(
        &self,
        host: &str,
        send: impl Fn() -> Result<Fetched, ClientError>,
    ) -> Result<Fetched, ClientError> {
        let deadline = Instant::now() + self.config.timeout;
        let mut resent = false;
        let mut f = send()?;
        loop {
            let Some(mut pending) = pending_of(&f) else { return Ok(f) };
            let request = pending.request.clone();
            let mut last = f;
            let unknown = loop {
                let now = Instant::now();
                if now >= deadline {
                    return Ok(last);
                }
                let wait = Duration::from_secs(pending.estimate_seconds.clamp(1, 10)).min(deadline - now);
                std::thread::sleep(wait);
                if Instant::now() >= deadline {
                    return Ok(last);
                }
                let g = self.poll_job(host, &pending.poll, &request)?;
                if let Some(next) = pending_of(&g) {
                    pending = next;
                    last = g;
                    continue;
                }
                if is_job_unknown(&g) {
                    break g;
                }
                return Ok(g);
            };
            if resent {
                return Ok(unknown);
            }
            resent = true;
            f = send()?;
        }
    }

    /// A poll of a job's URL (through the client's peers, as any URL of `host`), its answer checked against the
    /// request the job answers.
    fn poll_job(&self, host: &str, url: &str, request: &Request) -> Result<Fetched, ClientError> {
        let (status, body) = self.client.get(url, 16 << 20).map_err(|m| ce(ErrorCode::InputUnavailable, m))?;
        let req = request.clone().with_mesh(host).map_err(|e| ce(ErrorCode::Envelope, format!("{e:?}")))?;
        // The mesh's `E_JOB_UNKNOWN` is a bad request with no receipt to check: it is its own answer.
        if status == 400 && is_job_unknown_body(&body) {
            let answer =
                datumesh_core::Answer::bad_request(ErrorCode::JobUnknown).map_err(|e| ce(e.code(), e.to_string()))?;
            return Ok(Fetched { status, body, answer });
        }
        let check = |b: &[u8]| datumesh_core::check_answer(status, b, &self.client.keys_for_body(b), &req);
        let answer = match check(&body) {
            Err(e) if matches!(e.code(), ErrorCode::KeyUnknown | ErrorCode::KeyNotValid) => {
                let _ = self.client.key_document(host, true);
                check(&body)
            }
            r => r,
        }
        .map_err(|e| ce(e.code(), e.to_string()))?;
        Ok(Fetched { status, body, answer })
    }

    /// Puts `host`'s key document (as the client fetched it from `host`) in the store unless
    /// the store's is the same. Never for an unrooted host.
    pub(crate) fn ensure_namespace(&self, host: &str) -> Result<(), ClientError> {
        if is_unrooted(host) {
            return Ok(());
        }
        let stored = self.store.namespace(host).map_err(store_err)?;
        if stored.is_some() && self.client.key_document_bytes(host).is_none() {
            // Held, and not fetched in this process: no request for it.
            return Ok(());
        }
        self.patient(|| self.client.key_document(host, false))?;
        let bytes = self
            .client
            .key_document_bytes(host)
            .ok_or_else(|| ClientError { code: ErrorCode::KeyUnavailable, detail: host.to_string() })?;
        if stored.as_ref().map(|n| n.keydoc.as_slice()) != Some(bytes.as_slice()) {
            self.store.put_namespace(host, &bytes, &self.now()).map_err(store_err)?;
        }
        Ok(())
    }

    /// The key document of a function's publisher (the host of its namespace), fetched
    /// through the client and put in the store when it differs: its declarations (SPEC 9.2,
    /// 8.5) are what the engine's licence inheritance reads (as http's
    /// `Foreign::ensure_publisher`). On explicit fetches only: a name's import (get, prefetch,
    /// refresh) and a forward.
    pub(crate) fn ensure_publisher(&self, host: &str) -> Result<(), ClientError> {
        if is_unrooted(host) {
            return Ok(());
        }
        self.patient(|| self.client.publisher_key_document(host))?;
        let bytes = self
            .client
            .key_document_bytes(host)
            .ok_or_else(|| ClientError { code: ErrorCode::KeyUnavailable, detail: host.to_string() })?;
        let stored = self.store.namespace(host).map_err(store_err)?;
        if stored.as_ref().map(|n| n.keydoc.as_slice()) != Some(bytes.as_slice()) {
            self.store.put_namespace(host, &bytes, &self.now()).map_err(store_err)?;
        }
        Ok(())
    }

    /// A descriptor by hash from `host`, into the store.
    pub(crate) fn import_descriptor(&self, host: &str, fn_hash: &str) -> Result<(), ClientError> {
        if self.store.descriptor(fn_hash).is_some() {
            return Ok(());
        }
        let d = self.patient(|| self.client.descriptor(host, fn_hash))?;
        self.store.put_descriptor(&d).map_err(store_err)?;
        Ok(())
    }

    /// Brings a foreign name's binding up to date from its host: the binding answering now,
    /// with its descriptor; bound in the store from the host's `since` unless already. `Ok(false)`
    /// when the host binds nothing to the name.
    pub(crate) fn import_name(&self, name: &str) -> Result<bool, ClientError> {
        let host = host_of(name).to_string();
        let Some(b) = self.patient(|| self.client.binding(&host, name, None))? else {
            return Ok(false);
        };
        self.ensure_publisher(&host)?;
        self.import_descriptor(&host, &b.fn_hash)?;
        let local = self.store.resolve_binding(name, END).map_err(store_err)?;
        match local {
            Some(l) if l.fn_hash == b.fn_hash => {}
            Some(l) if l.tx.as_str() >= b.since.as_str() => {
                return Err(ClientError {
                    code: ErrorCode::InputUnavailable,
                    detail: format!("{name}: the host's binding (since {}) does not extend the one held", b.since),
                });
            }
            _ => self.store.bind_name(name, &b.fn_hash, &b.since).map_err(store_err)?,
        }
        self.index.hold_name(name, &host, &self.now()).map_err(store_err)?;
        Ok(true)
    }

    /// Imports a fetched receipt: its signer's key document, the receipt (verified again by
    /// the store) and, for a stored cell, the version (unless one at or after its `tx` is
    /// held). `Ok(true)` when a version was added.
    /// Stores a withdrawal (SPEC 8.1, store 0.6.0) as the version of its cell at its `tx`, with its cell held, as a
    /// receipt is: a cell withdrawn as of an instant has no value there, and its history stays (CONTRACT-round-0.7 R1).
    pub(crate) fn import_withdrawal(&self, w: &Withdrawal) -> Result<(), ClientError> {
        let cell = w.cell();
        let host = w.signer_host().to_string();
        self.ensure_namespace(&host)?;
        let id = cell.id();
        self.store_version(&id, w.tx(), w.canonical())?;
        let held = HeldCell { cell_id: id, host: host_of(&cell.ns).to_string(), rest: cell_rest(cell), slice: None };
        self.index.hold_cell(&held, &self.now()).map_err(store_err)?;
        Ok(())
    }

    /// Puts the bytes of a receipt or a withdrawal as the version of cell `id` at `tx`, unless that version is held.
    /// Unlike `import_receipt`, an older version is kept too (a read as of an earlier instant needs it).
    pub(crate) fn store_version(&self, id: &str, tx: &str, bytes: &[u8]) -> Result<(), ClientError> {
        let held = self.store.version_asof(id, tx).map_err(store_err)?;
        if held.is_some_and(|v| v.tx == tx) {
            return Ok(());
        }
        let h = self.store.put_receipt(bytes).map_err(store_err)?;
        self.store.put_version(id, tx, &h).map_err(store_err)
    }

    pub(crate) fn import_receipt(&self, r: &Receipt, stored: bool, slice: Option<&str>) -> Result<bool, ClientError> {
        let bytes = datumesh_http::publish::receipt_json(r);
        self.ensure_namespace(r.signer_host())?;
        let h = self.store.put_receipt(&bytes).map_err(store_err)?;
        if !stored {
            return Ok(false);
        }
        let cell = r.cell();
        let id = cell.id();
        let added = match self.store.latest(&id).map_err(store_err)? {
            Some(l) if l.tx.as_str() >= r.tx() => false,
            _ => {
                self.store.put_version(&id, r.tx(), &h).map_err(store_err)?;
                true
            }
        };
        let held = HeldCell {
            cell_id: id.clone(),
            host: host_of(&cell.ns).to_string(),
            rest: cell_rest(cell),
            slice: slice.map(str::to_string),
        };
        self.index.hold_cell(&held, &self.now()).map_err(store_err)?;
        if added {
            note(|c| c.added.push(id));
        }
        Ok(added)
    }

    /// The blob a receipt's value names, from `host`, unless held.
    pub(crate) fn import_blob(&self, host: &str, r: &Receipt) -> Result<bool, ClientError> {
        let v = r.value();
        if v.ty() != Type::Blob {
            return Ok(false);
        }
        let lex = v.lexical().to_string();
        let sha = lex.split(':').next().unwrap_or("");
        if self.store.blob(sha).map_err(store_err)?.is_some() {
            return Ok(false);
        }
        let bytes = self.patient(|| self.client.blob(host, sha))?;
        self.store.put_blob(&bytes).map_err(store_err)?;
        Ok(true)
    }

    /// The gate's decision on an input receipt held in the store.
    fn gate_check(&self, function: &str, receipt_hash: &str) -> Result<(), EvalError> {
        if receipt_hash.starts_with("slice:") {
            return Ok(());
        }
        let Some(bytes) = self.store.receipt(receipt_hash) else { return Ok(()) };
        let Ok(r) = Receipt::from_json(&bytes) else { return Ok(()) };
        self.gate_receipt(function, &r)
    }

    /// Keeps the members an evaluation fetched (datumesh-local 0.5.2, item 12), as a prefetch
    /// holds them: each member's receipt and version, its blob where it has one, and the slice held
    /// when the whole slice was fetched. Returns the cells whose versions were added.
    pub(crate) fn keep_slices(&self, pending: &[PendingSlice]) -> Result<Vec<String>, ClientError> {
        let (out, ctx) = with_ctx(|| -> Result<(), ClientError> {
            for p in pending {
                for r in &p.members {
                    self.import_receipt(r, true, p.whole.as_deref())?;
                    let _ = self.import_blob(&p.host, r);
                }
                if let Some(k) = &p.whole {
                    self.index.hold_slice(k, &p.host, p.members.len(), &self.now()).map_err(store_err)?;
                }
            }
            Ok(())
        });
        out?;
        Ok(ctx.added)
    }

    /// The gate's decision on an input receipt (the receipt itself, held or not yet).
    fn gate_receipt(&self, function: &str, r: &Receipt) -> Result<(), EvalError> {
        let publisher = host_of(function);
        let declarations = self
            .store
            .namespace(publisher)
            .ok()
            .flatten()
            .and_then(|n| datumesh_core::KeyDocument::from_json(publisher, &n.keydoc).ok())
            .and_then(|d| d.declarations);
        let input = format!("the input {}/{} under {}", r.cell().ns, r.cell().fn_name, r.licence());
        // L4: the licences this machine accepted (`licences.json`) decide what the gate's rule refuses.
        let accepted = self.acceptance.engine_set().map_err(|e| unavailable(e.to_string()))?;
        let licence = r.licence();
        if is_restricted(licence) {
            note(|c| {
                if !c.restricted.iter().any(|l| l == licence) {
                    c.restricted.push(licence.to_string());
                }
            });
        }
        let decision = match self.gate.permits(function, licence, r.signer_host(), declarations.as_ref(), &self.now()) {
            // A receipt this machine signed: a restricted licence is computed over only when accepted.
            GateDecision::Compute
                if is_unrooted(r.signer_host()) && is_restricted(licence) && !accepted.contains(licence) =>
            {
                GateDecision::Refuse(LicenceReason::NotDeclared)
            }
            // A restricted licence the publisher does not declare: computed here when accepted.
            GateDecision::Refuse(_) if accepted.contains(licence) => GateDecision::Compute,
            other => other,
        };
        match decision {
            GateDecision::Compute => Ok(()),
            GateDecision::Forward => {
                let why = format!(
                    "{input}, declared for {function} by its publisher: a restricted licence is not signed here"
                );
                note(|c| {
                    c.refused.get_or_insert(why.clone());
                });
                Err(EvalError::InvalidInput(datumesh_core::Error::new(ErrorCode::InputUnavailable, "$", why)))
            }
            GateDecision::Refuse(reason) => {
                let mut why =
                    format!("licence inheritance (SPEC 8.5) refuses {function}: {input}: {}", reason.as_str());
                if is_restricted(licence) && !accepted.contains(licence) {
                    why.push_str("; ");
                    why.push_str(&accept_hint(licence));
                }
                note(|c| {
                    c.unlicensed.get_or_insert(why.clone());
                });
                Err(EvalError::InvalidInput(datumesh_core::Error::new(ErrorCode::InputUnavailable, "$", why)))
            }
        }
    }
}

/// A runtime with the resolver's reads in front of it: foreign stored inputs fetched (when
/// allowed) and imported before they are read, and every input shown to the gate.
pub(crate) struct Gated {
    inner: Arc<dyn Runtime>,
    shared: Arc<Shared>,
}

impl Gated {
    pub(crate) fn new(inner: Arc<dyn Runtime>, shared: Arc<Shared>) -> Gated {
        Gated { inner, shared }
    }
}

impl Runtime for Gated {
    fn lang(&self) -> &str {
        self.inner.lang()
    }

    fn evaluate(
        &self,
        descriptor: &FunctionDescriptor,
        cell: &Cell,
        inputs: &mut dyn Inputs,
        budget: &mut Budget,
    ) -> Result<Scalar, EvalError> {
        let function = format!("{}/{}", cell.ns, cell.fn_name);
        let mut through = Through { inner: inputs, shared: &self.shared, function, served: false };
        let out = self.inner.evaluate(descriptor, cell, &mut through, budget);
        // An answer that read fetched members from memory is not the engine's to keep: its stored
        // inputs would omit them. The evaluation runs again once they are kept (item 12).
        match out {
            Ok(_) if through.served => Err(EvalError::Runtime("read fetched members not yet kept".into())),
            other => other,
        }
    }

    fn asof_dependent(&self, descriptor: &FunctionDescriptor) -> bool {
        self.inner.asof_dependent(descriptor)
    }
}

struct Through<'a> {
    inner: &'a mut dyn Inputs,
    shared: &'a Shared,
    /// The function evaluated (its full name): its publisher's declarations decide.
    function: String,
    /// A read took members from the fetched, not yet kept (item 12).
    served: bool,
}

fn unavailable(why: String) -> EvalError {
    note(|c| {
        c.unavailable.get_or_insert(why.clone());
    });
    EvalError::InvalidInput(datumesh_core::Error::new(ErrorCode::InputUnavailable, "$", why))
}

/// Whether the arguments of a cell take the pattern (every argument of it, by name).
fn matches_pattern(pattern: &SlicePattern, args: &Args) -> bool {
    pattern.iter().all(|(k, p)| match (p, args.get(k)) {
        (Pattern::Open, Some(Arg::Value(_))) => true,
        (Pattern::Value(v), Some(Arg::Value(x))) => v == x,
        (Pattern::Range { lo, hi }, Some(Arg::Value(x))) => {
            scalar_cmp(lo, x) != std::cmp::Ordering::Greater && scalar_cmp(x, hi) != std::cmp::Ordering::Greater
        }
        _ => false,
    })
}

/// The order of two scalars of one type: numbers by value, the rest by their lexical form (dates
/// and instants sort as they read).
fn scalar_cmp(a: &Scalar, b: &Scalar) -> std::cmp::Ordering {
    match (a, b) {
        (Scalar::Number(x), Scalar::Number(y)) => match (x.parse::<f64>(), y.parse::<f64>()) {
            (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            _ => x.cmp(y),
        },
        _ => a.cmp(b),
    }
}

/// The members' arguments in key order (their values in the order of their names).
fn cmp_args(a: &Args, b: &Args) -> std::cmp::Ordering {
    for ((_, x), (_, y)) in a.iter().zip(b.iter()) {
        let o = match (x, y) {
            (Arg::Value(x), Arg::Value(y)) => scalar_cmp(x, y),
            _ => x.cmp(y),
        };
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

/// A slice's rest as its fetch names it: the canonical URI form without `cell://` (core's
/// `format_uri`, as prefetch holds it), each open argument `*` (a range's too: production has no
/// URI for a ranged slice).
fn slice_rest(fn_name: &str, pattern: &SlicePattern) -> String {
    let (ns, f) = fn_name.rsplit_once('/').unwrap_or(("", fn_name));
    let args: Args = pattern
        .iter()
        .map(|(k, p)| {
            let a = match p {
                Pattern::Value(v) => Arg::Value(v.clone()),
                _ => Arg::Slice,
            };
            (k.clone(), a)
        })
        .collect();
    let uri = datumesh_core::format_uri(&datumesh_core::CellRef {
        ns: ns.to_string(),
        fn_name: f.to_string(),
        args,
        asof: None,
    });
    uri["cell://".len()..].to_string()
}

/// The peer origin a `https://<host>/...` URL is reached at (the configured peer's, else as it is).
fn reached_at(config: &Config, url: &str, host: &str) -> String {
    match (url.strip_prefix(&format!("https://{host}")), config.peers.get(host)) {
        (Some(rest), Some(origin)) => format!("{origin}{rest}"),
        _ => url.to_string(),
    }
}

/// The key of a read of a foreign slice (item 12): the range's key (its ends; no slice URI names a range)
/// for one range, else the whole slice's rest, which every other read of it is held by. Returns
/// (key, whole rest, whether it is a range).
pub(crate) fn slice_key(fn_name: &str, pattern: &SlicePattern) -> (String, String, bool) {
    let whole = slice_rest(fn_name, pattern);
    let ends: Vec<String> = pattern
        .values()
        .filter_map(|p| match p {
            Pattern::Range { lo, hi } => Some(format!("{}..{}", lo.lexical(), hi.lexical())),
            _ => None,
        })
        .collect();
    let interval = ends.len() == 1;
    let key = if interval { format!("{whole} within {}", ends.join(",")) } else { whole.clone() };
    (key, whole, interval)
}

impl Through<'_> {
    /// A read of a stored foreign slice: when the members it takes are not in memory, nor kept, nor the
    /// whole slice held, the run stops here (the slice is fetched, then the evaluation runs again). A read
    /// of members in memory is served from them (the run's answer is then not kept).
    fn reach_slice(&mut self, fn_name: &str, pattern: &SlicePattern) -> Result<(), EvalError> {
        let (key, whole, interval) = slice_key(fn_name, pattern);
        let (in_memory, kept) = noted(|c| {
            let in_memory = c.pending.iter().any(|p| p.key == key || p.key == whole);
            let kept = c.satisfied.contains(&key) || c.satisfied.contains(&whole);
            (in_memory, kept)
        })
        .unwrap_or((false, false));
        if in_memory {
            self.served = true;
            note(|c| c.served = true);
            return Ok(());
        }
        if kept {
            return Ok(());
        }
        if self.shared.index.slice_held(&whole).map_err(|e| unavailable(format!("index: {e}")))? {
            return Ok(());
        }
        note(|c| {
            if c.needed.is_none() {
                c.needed = Some(Need {
                    fn_name: fn_name.to_string(),
                    pattern: pattern.clone(),
                    key: key.clone(),
                    whole: whole.clone(),
                    interval,
                });
            }
        });
        Err(EvalError::InvalidInput(datumesh_core::Error::new(
            ErrorCode::InputUnavailable,
            "$",
            format!("{key}: not held; fetched before it is read"),
        )))
    }
}

/// Why a slice's members were not fetched.
enum FetchFail {
    /// The bound: why, naming the slice, its size so far and the bound.
    Limit(String),
    /// No answer from the network: why.
    Unavailable(String),
    /// Another mesh's answer failed, or failed its check.
    Remote(ClientError),
}

impl From<FetchFail> for Stop {
    fn from(f: FetchFail) -> Stop {
        match f {
            FetchFail::Limit(why) => Stop::Limit(why),
            FetchFail::Unavailable(why) => Stop::Unavailable(why),
            FetchFail::Remote(e) => Stop::Remote(e),
        }
    }
}

impl Shared {
    /// Checks `size` members (the evaluation's count, this fetch included) against the bound.
    fn charge(&self, fn_name: &str, pattern: &SlicePattern, size: u64) -> Result<(), FetchFail> {
        let max = self.config.slice_fetch_max;
        if size > max {
            let rest = slice_rest(fn_name, pattern);
            return Err(FetchFail::Limit(format!(
                "{rest}: {size} members so far, over slice_fetch_max {max} (config.json): prefetch '{rest}' to hold it first, or raise slice_fetch_max"
            )));
        }
        Ok(())
    }

    /// Fetches the members one need asks for into the context `acc` (pending, not kept), counting them
    /// against the bound. Nothing is kept here: a refusal later leaves the store as it was (item 12).
    pub(crate) fn fetch_need(&self, need: &Need, acc: &mut Ctx) -> Result<(), Stop> {
        let host = host_of(&need.fn_name).to_string();
        self.may_fetch(&host).map_err(Stop::Unavailable)?;
        let base = acc.count;
        let members = if need.interval {
            self.fetch_range(&host, &need.fn_name, &need.pattern, base)?
        } else {
            self.fetch_whole(&host, &need.whole, &need.pattern, base)?
        };
        acc.count += members.len() as u64;
        acc.pending.push(PendingSlice {
            key: if need.interval { need.key.clone() } else { need.whole.clone() },
            host,
            fn_name: need.fn_name.clone(),
            whole: (!need.interval).then(|| need.whole.clone()),
            members,
        });
        Ok(())
    }

    /// The whole of a foreign slice a `get` reads and does not hold (datumesh-local 0.5.3): fetched as calc's read
    /// fetches it, counted page by page against `slice_fetch_max` (a refusal keeps nothing), then kept as a prefetch
    /// holds it. `rest` is the slice's rest as the mesh names it (`ns/fn(args, *)`), the key it is held by.
    pub(crate) fn fetch_slice_read(&self, fn_name: &str, args: &Args, rest: &str) -> Result<(), Stop> {
        let pattern: SlicePattern = args
            .iter()
            .map(|(k, a)| (k.clone(), if let Arg::Value(v) = a { Pattern::Value(v.clone()) } else { Pattern::Open }))
            .collect();
        let need = Need {
            fn_name: fn_name.to_string(),
            pattern,
            key: rest.to_string(),
            whole: rest.to_string(),
            interval: false,
        };
        let mut acc = Ctx::default();
        self.fetch_need(&need, &mut acc)?;
        self.keep_slices(&acc.pending).map_err(Stop::Remote)?;
        Ok(())
    }

    /// The evaluation `run` as an answer (item 12): a run that needs a slice fetches it and runs again; a run
    /// that read fetched members is run again once they are kept, so the engine records the inputs it read from
    /// the store; the last run's context is returned, with the cells kept on the way.
    pub(crate) fn evaluating<T>(&self, run: impl Fn() -> T) -> Result<(T, Ctx), Stop> {
        let mut pending: Vec<PendingSlice> = Vec::new();
        let mut count = 0u64;
        let mut satisfied: Vec<String> = Vec::new();
        let mut added: Vec<String> = Vec::new();
        loop {
            let init =
                Ctx { pending: std::mem::take(&mut pending), count, satisfied: satisfied.clone(), ..Ctx::default() };
            let (out, mut ctx) = with_ctx_from(init, &run);
            pending = std::mem::take(&mut ctx.pending);
            count = ctx.count;
            if let Some(need) = ctx.needed.take() {
                let mut acc = Ctx { pending, count, ..Ctx::default() };
                self.fetch_need(&need, &mut acc)?;
                pending = acc.pending;
                count = acc.count;
                continue;
            }
            if ctx.served && !pending.is_empty() {
                added.extend(self.keep_slices(&pending).map_err(Stop::Remote)?);
                for p in pending.drain(..) {
                    satisfied.push(p.key.clone());
                    if let Some(w) = p.whole {
                        satisfied.push(w);
                    }
                }
                continue;
            }
            ctx.added.extend(added);
            return Ok((out, ctx));
        }
    }

    /// Verifies a member's receipt against its signer's key document, as held (the store's key).
    fn verify_member(&self, r: &Receipt) -> Result<(), FetchFail> {
        let signer = r.signer_host().to_string();
        self.ensure_namespace(&signer).map_err(FetchFail::Remote)?;
        let doc = self
            .store
            .namespace(&signer)
            .map_err(|e| FetchFail::Remote(store_err(e)))?
            .and_then(|n| KeyDocument::from_json(&signer, &n.keydoc).ok())
            .ok_or_else(|| {
                FetchFail::Remote(ClientError {
                    code: ErrorCode::KeyUnavailable,
                    detail: format!("{signer}: no key document to check {} by", r.hash()),
                })
            })?;
        let mut keys = Keys::new();
        keys.insert_document(doc);
        let v = datumesh_core::verify(&datumesh_http::publish::receipt_json(r), &keys, None, None);
        if v.ok {
            Ok(())
        } else {
            Err(FetchFail::Remote(ClientError {
                code: ErrorCode::InputUnavailable,
                detail: format!("member {} of {} does not verify: {}", r.hash(), r.cell().ns, v.detail),
            }))
        }
    }

    /// The members of a range, from production's `/x/interval` (`from`, `to` and the value arguments as
    /// keys), each verified. `base` is the evaluation's count before this fetch.
    fn fetch_range(&self, host: &str, fn_name: &str, pattern: &SlicePattern, base: u64) -> Result<Vec<Receipt>, Stop> {
        let mut query = Vec::new();
        let (mut from, mut to) = (String::new(), String::new());
        for (k, p) in pattern {
            match p {
                Pattern::Range { lo, hi } => {
                    from = lo.lexical().to_string();
                    to = hi.lexical().to_string();
                }
                Pattern::Value(v) => query.push(format!("{}={}", pct(k), pct(v.lexical()))),
                _ => {}
            }
        }
        query.push(format!("from={}", pct(&from)));
        query.push(format!("to={}", pct(&to)));
        let url = reached_at(&self.config, &format!("https://{host}/x/interval/{fn_name}?{}", query.join("&")), host);
        let (status, bytes) = crate::net::get(&url, self.config.timeout).map_err(Stop::Unavailable)?;
        let v = json::parse(&bytes)
            .map_err(|e| Stop::Remote(ClientError { code: ErrorCode::Envelope, detail: format!("{url}: {e}") }))?;
        if status != 200 {
            return Err(Stop::Remote(ClientError {
                code: ErrorCode::InputUnavailable,
                detail: format!("{url}: status {status}"),
            }));
        }
        let Some(Value::Array(members)) = v.get("members") else {
            return Err(Stop::Remote(ClientError { code: ErrorCode::Envelope, detail: format!("{url}: no members") }));
        };
        self.charge(fn_name, pattern, base + members.len() as u64)?;
        let mut out = Vec::new();
        for m in members {
            let r = m.get("receipt").and_then(|x| Receipt::from_value(x).ok()).ok_or_else(|| {
                Stop::Remote(ClientError {
                    code: ErrorCode::Envelope,
                    detail: format!("{url}: a member has no receipt"),
                })
            })?;
            self.verify_member(&r)?;
            out.push(r);
        }
        Ok(out)
    }

    /// The members of a whole slice, from the slice's pages (`next` followed, every page checked by the
    /// client), counted page by page against the bound. `base` is the evaluation's count before this fetch.
    fn fetch_whole(&self, host: &str, rest: &str, pattern: &SlicePattern, base: u64) -> Result<Vec<Receipt>, Stop> {
        let fn_name = rest.split('?').next().unwrap_or(rest).to_string();
        let open: Vec<String> = pattern
            .iter()
            .filter(|(_, p)| matches!(p, Pattern::Open | Pattern::Range { .. }))
            .map(|(k, _)| k.clone())
            .collect();
        let doc = self.patient(|| self.client.resolve(host)).map_err(|e| Stop::from(remote_or_unavailable(&e)))?;
        let first = doc.url("slice", &[("cell", rest)]).ok_or_else(|| {
            Stop::Remote(ClientError { code: ErrorCode::MeshDoc, detail: format!("{host} has no slice endpoint") })
        })?;
        let mut url = Some(first);
        let mut after: Option<BTreeMap<String, Scalar>> = None;
        let mut out: Vec<Receipt> = Vec::new();
        while let Some(u) = url.take() {
            let f = self
                .patient(|| {
                    let f = self.client.slice_page(host, rest, Some(&u), after.clone())?;
                    if f.status == 429 {
                        return Err(ClientError { code: ErrorCode::InputUnavailable, detail: "status 429".into() });
                    }
                    Ok(f)
                })
                .map_err(|e| Stop::from(remote_or_unavailable(&e)))?;
            let Some(p) = f.answer.page.as_ref() else {
                return Err(Stop::Remote(ClientError {
                    code: f.answer.error.unwrap_or(ErrorCode::InputUnavailable),
                    detail: format!("{rest}: {}", f.answer.outcome.as_str()),
                }));
            };
            for m in &p.members {
                if let Some(r) = m.receipt.as_ref() {
                    out.push(r.clone());
                }
            }
            self.charge(&fn_name, pattern, base + out.len() as u64)?;
            match (&p.next, p.members.last().and_then(|m| m.receipt.as_ref())) {
                (Some(next), Some(last)) => {
                    after = Some(
                        open.iter()
                            .filter_map(|k| match last.cell().args.get(k) {
                                Some(Arg::Value(v)) => Some((k.clone(), v.clone())),
                                _ => None,
                            })
                            .collect(),
                    );
                    url = Some(next.clone());
                }
                _ => break,
            }
        }
        Ok(out)
    }
}

/// A client failure as a fetch's: the network's (unavailable), else another mesh's (remote).
fn remote_or_unavailable(e: &ClientError) -> FetchFail {
    match network_failure(e) {
        Some(w) => FetchFail::Unavailable(w),
        None => FetchFail::Remote(e.clone()),
    }
}

impl Through<'_> {
    /// A foreign name not held: fetched when allowed.
    fn name(&mut self, fn_name: &str) -> Result<(), EvalError> {
        let host = host_of(fn_name);
        if is_unrooted(host) {
            return Ok(());
        }
        let held = self.shared.store.resolve_binding(fn_name, END).map_err(|e| unavailable(format!("store: {e}")))?;
        if held.is_some() {
            return Ok(());
        }
        self.shared.may_fetch(host).map_err(unavailable)?;
        match self.shared.import_name(fn_name) {
            Ok(_) => Ok(()),
            Err(e) => match network_failure(&e) {
                Some(why) => Err(unavailable(why)),
                None => {
                    note(|c| {
                        c.remote.get_or_insert(e.clone());
                    });
                    Err(EvalError::InvalidInput(datumesh_core::Error::new(e.code, "$", e.detail)))
                }
            },
        }
    }

    /// A foreign stored cell not held: fetched when allowed.
    fn cell(&mut self, fn_name: &str, args: &Args) -> Result<(), EvalError> {
        let host = host_of(fn_name).to_string();
        if is_unrooted(&host) {
            return Ok(());
        }
        self.name(fn_name)?;
        let Some(b) = self.inner.binding_peek(fn_name, None)? else { return Ok(()) };
        if b.descriptor.lang != "stored" {
            return Ok(());
        }
        let (ns, f) = fn_name.rsplit_once('/').unwrap_or((fn_name, ""));
        let cell = Cell { ns: ns.to_string(), fn_name: f.to_string(), fn_hash: b.fn_hash.clone(), args: args.clone() };
        let held = self.shared.store.latest(&cell.id()).map_err(|e| unavailable(format!("store: {e}")))?;
        if held.is_some() {
            return Ok(());
        }
        self.shared.may_fetch(&host).map_err(unavailable)?;
        let reference = Reference::new(cell.ns.clone(), cell.fn_name.clone(), cell.args.clone());
        let fetched = self.shared.fetch_cell(&host, &reference).and_then(|f| {
            if let Some(r) = f.answer.receipt.as_ref() {
                self.shared.import_receipt(r, true, None)?;
                // Content for text_of; a failure here is the blob's, at evaluation.
                let _ = self.shared.import_blob(&host, r);
            }
            Ok(())
        });
        match fetched {
            Ok(()) => Ok(()),
            Err(e) => match network_failure(&e) {
                Some(why) => Err(unavailable(why)),
                None => {
                    note(|c| {
                        c.remote.get_or_insert(e.clone());
                    });
                    Err(EvalError::InvalidInput(datumesh_core::Error::new(e.code, "$", e.detail)))
                }
            },
        }
    }
}

impl Inputs for Through<'_> {
    fn read(&mut self, fn_name: &str, args: &Args) -> Result<Scalar, EvalError> {
        self.read_cell(fn_name, args).map(|i| i.value)
    }

    fn read_cell(&mut self, fn_name: &str, args: &Args) -> Result<Input, EvalError> {
        self.cell(fn_name, args)?;
        let i = self.inner.read_cell(fn_name, args)?;
        self.shared.gate_check(&self.function, &i.receipt_hash)?;
        Ok(i)
    }

    fn read_asof(&mut self, fn_name: &str, args: &Args, t: &str) -> Result<Input, EvalError> {
        self.cell(fn_name, args)?;
        let i = self.inner.read_asof(fn_name, args, t)?;
        self.shared.gate_check(&self.function, &i.receipt_hash)?;
        Ok(i)
    }

    fn read_slice(&mut self, fn_name: &str, pattern: &SlicePattern) -> Result<Vec<Member>, EvalError> {
        self.name(fn_name)?;
        // A stored foreign slice is read only once it is held (datumesh-local 0.5.2, item 12): fetched first.
        if !is_unrooted(host_of(fn_name)) {
            if let Some(b) = self.inner.binding_peek(fn_name, None)? {
                if b.descriptor.lang == "stored" {
                    self.reach_slice(fn_name, pattern)?;
                }
            }
        }
        let mut members = self.inner.read_slice(fn_name, pattern)?;
        for m in &members {
            self.shared.gate_check(&self.function, &m.input.receipt_hash)?;
        }
        // The fetched members the pattern takes (not held: the store's are read above).
        let fetched: Vec<Receipt> = noted(|c| {
            c.pending
                .iter()
                .filter(|p| p.fn_name == fn_name)
                .flat_map(|p| p.members.iter())
                .filter(|r| matches_pattern(pattern, &r.cell().args))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
        if !fetched.is_empty() {
            self.served = true;
        }
        for r in fetched {
            if members.iter().any(|m| m.args == r.cell().args) {
                continue;
            }
            self.shared.gate_receipt(&self.function, &r)?;
            let input = Input::new(r.value().clone(), r.hash(), r.tx(), r.asof());
            members.push(Member::new(r.cell().args.clone(), input));
        }
        members.sort_by(|a, b| cmp_args(&a.args, &b.args));
        Ok(members)
    }

    fn read_versions(&mut self, fn_name: &str, args: &Args, from: &str, to: &str) -> Result<Vec<Input>, EvalError> {
        self.cell(fn_name, args)?;
        let vs = self.inner.read_versions(fn_name, args, from, to)?;
        for i in &vs {
            self.shared.gate_check(&self.function, &i.receipt_hash)?;
        }
        Ok(vs)
    }

    fn binding(&mut self, fn_name: &str, at: Option<&str>) -> Result<Option<FnBinding>, EvalError> {
        self.name(fn_name)?;
        self.inner.binding(fn_name, at)
    }

    fn asof(&mut self) -> String {
        self.inner.asof()
    }

    fn binding_peek(&mut self, fn_name: &str, at: Option<&str>) -> Result<Option<FnBinding>, EvalError> {
        self.name(fn_name)?;
        self.inner.binding_peek(fn_name, at)
    }

    fn asof_compare(&mut self, t: &str) -> Result<std::cmp::Ordering, EvalError> {
        self.inner.asof_compare(t)
    }

    fn read_blob(&mut self, sha256: &str, length: u64) -> Result<Vec<u8>, EvalError> {
        self.inner.read_blob(sha256, length)
    }
}

/// The descriptor value of a stored function with `signature`.
pub(crate) fn stored_descriptor(signature: &Value) -> Value {
    Value::Object(vec![
        (
            "code".into(),
            Value::Object(vec![
                ("body".into(), Value::String(String::new())),
                ("lang".into(), Value::String("stored".into())),
            ]),
        ),
        ("inputs".into(), Value::Array(vec![])),
        ("signature".into(), signature.clone()),
    ])
}

#[cfg(test)]
mod transport_tests {
    //! The failure of a fetch that got no answer, named as `E_FOREIGN_UNAVAILABLE`'s
    //! `detail`, from the client's error text as each platform words it. datumesh-http's
    //! client hands the transport error over as text (`<url>: <ureq error>`), so the
    //! classification is of that text; these are the shapes ureq 3.4 and the standard
    //! library produce on Linux, macOS and Windows.

    use super::transport_kind;
    use datumesh_core::ForeignDetail::{self, Dns, Network, Refused, Timeout, Tls};

    const URL: &str = "https://mesh-test.datumesh.com/.well-known/datumesh.json";

    fn kind(err: &str) -> Option<ForeignDetail> {
        transport_kind(&format!("{URL}: {err}"))
    }

    #[test]
    fn dns_on_every_platform() {
        for e in [
            // Linux (glibc getaddrinfo, through std's lookup error).
            "io: failed to lookup address information: Temporary failure in name resolution",
            "io: failed to lookup address information: Name or service not known",
            "io: failed to lookup address information: No address associated with hostname",
            // macOS.
            "io: failed to lookup address information: nodename nor servname provided, or not known",
            // Windows (WSAHOST_NOT_FOUND, WSATRY_AGAIN, WSANO_DATA, WSANO_RECOVERY).
            "io: No such host is known. (os error 11001)",
            "io: This is usually a temporary error during hostname resolution and means that the local server did not receive a response from an authoritative server. (os error 11002)",
            "io: The requested name is valid, but no data of the requested type was found. (os error 11004)",
            "io: A non-recoverable error occurred during a database lookup. (os error 11003)",
            // ureq's own: a name that resolved to no address.
            "host not found",
        ] {
            assert_eq!(kind(e), Some(Dns), "{e}");
        }
    }

    #[test]
    fn refused_timeout_tls_network() {
        for (e, k) in [
            ("io: Connection refused (os error 111)", Refused),
            ("io: Connection refused (os error 61)", Refused),
            (
                "io: No connection could be made because the target machine actively refused it. (os error 10061)",
                Refused,
            ),
            ("timeout: global", Timeout),
            ("io: Connection timed out (os error 110)", Timeout),
            ("io: Operation timed out (os error 60)", Timeout),
            (
                "io: A connection attempt failed because the connected party did not properly respond after a period of time, or established connection failed because connected host has failed to respond. (os error 10060)",
                Timeout,
            ),
            ("rustls: invalid peer certificate: UnknownIssuer", Tls),
            ("io: Network is unreachable (os error 101)", Network),
            ("io: A socket operation was attempted to an unreachable network. (os error 10051)", Network),
            ("io: Connection reset by peer (os error 104)", Network),
            ("io: An existing connection was forcibly closed by the remote host. (os error 10054)", Network),
            ("connection failed", Network),
        ] {
            assert_eq!(kind(e), Some(k), "{e}");
        }
    }

    #[test]
    fn the_url_does_not_count_and_answers_are_not_failures() {
        // A host whose name has the words in it: the error decides.
        assert_eq!(
            transport_kind("https://dns.tls.timeout.example/x: io: Connection refused (os error 111)"),
            Some(Refused)
        );
        assert_eq!(transport_kind("https://refused.example/x: io: No such host is known. (os error 11001)"), Some(Dns));
        // An answer came back: not a transport failure.
        assert_eq!(transport_kind(&format!("{URL}: status 404")), None);
        assert_eq!(transport_kind("store: database is locked"), None);
        assert_eq!(transport_kind("mesh-test.datumesh.com/ecb/fx_rate: the other mesh's history does not end"), None);
    }

    #[test]
    fn the_clients_catch_all_network_is_refined_by_the_text() {
        // datumesh-http 0.2.2's typed classification names only the Linux resolver's wording
        // `dns`; the Windows resolver's failure reaches it as an I/O error it calls `network`
        // (the v0.2.1 release run on windows-latest). The text decides where it can.
        use datumesh_core::ErrorCode;
        use datumesh_http::client::ClientError;
        let e = |detail: &str| ClientError { code: ErrorCode::ForeignUnavailable, detail: detail.to_string() };
        let win = format!("network: {URL}: io: No such host is known. (os error 11001)");
        assert_eq!(
            super::network_failure(&e(&win)),
            Some(format!("dns: {URL}: io: No such host is known. (os error 11001)"))
        );
        // A specific kind from the client stays, and a network failure the text cannot name stays network.
        let tls = format!("tls: {URL}: rustls: invalid peer certificate");
        assert_eq!(super::network_failure(&e(&tls)), Some(tls.clone()));
        let reset = format!("network: {URL}: io: Connection reset by peer (os error 104)");
        assert_eq!(super::network_failure(&e(&reset)), Some(reset.clone()));
        let odd = format!("network: {URL}: something else");
        assert_eq!(super::network_failure(&e(&odd)), Some(odd.clone()));
    }
}
