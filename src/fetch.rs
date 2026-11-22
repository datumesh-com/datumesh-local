//! Explicit fetches and the import into the store (the `Foreign` layer pattern: the HTTP
//! client fetches and checks, then the store's `put_*` checks again and keeps), and the
//! runtime layer through which every evaluation reads: foreign inputs fetched when allowed,
//! and every input receipt shown to the licence gate before its value is used.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use datumesh_core::json::Value;
use datumesh_core::{
    Args, Cell, ErrorCode, ForeignDetail, FunctionDescriptor, Receipt, Reference, Scalar, Type, Withdrawal,
};
use datumesh_engine::{Budget, Clock, EvalError, FnBinding, Input, Inputs, Member, Runtime, SlicePattern};
use datumesh_http::client::{Client, ClientError, Fetched};
use datumesh_store::Store;

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
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

/// Runs `f` with a fresh context, returning what it noted.
pub(crate) fn with_ctx<T>(f: impl FnOnce() -> T) -> (T, Ctx) {
    let saved = CTX.with(|c| c.borrow_mut().replace(Ctx::default()));
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
    pub(crate) fn fetch_cell(&self, host: &str, reference: &Reference) -> Result<Fetched, ClientError> {
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
        let publisher = host_of(function);
        let declarations = self
            .store
            .namespace(publisher)
            .ok()
            .flatten()
            .and_then(|n| datumesh_core::KeyDocument::from_json(publisher, &n.keydoc).ok())
            .and_then(|d| d.declarations);
        let input = format!("the input {}/{} under {}", r.cell().ns, r.cell().fn_name, r.licence());
        match self.gate.permits(function, r.licence(), r.signer_host(), declarations.as_ref(), &self.now()) {
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
                let why = format!("licence inheritance (SPEC 8.5) refuses {function}: {input}: {}", reason.as_str());
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
        let mut through = Through { inner: inputs, shared: &self.shared, function };
        self.inner.evaluate(descriptor, cell, &mut through, budget)
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
}

fn unavailable(why: String) -> EvalError {
    note(|c| {
        c.unavailable.get_or_insert(why.clone());
    });
    EvalError::InvalidInput(datumesh_core::Error::new(ErrorCode::InputUnavailable, "$", why))
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
        let members = self.inner.read_slice(fn_name, pattern)?;
        for m in &members {
            self.shared.gate_check(&self.function, &m.input.receipt_hash)?;
        }
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
