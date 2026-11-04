//! Explicit fetches and the import into the store (the `Foreign` layer pattern: the HTTP
//! client fetches and checks, then the store's `put_*` checks again and keeps), and the
//! runtime layer through which every evaluation reads: foreign inputs fetched when allowed,
//! and every input receipt shown to the licence gate before its value is used.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use datumesh_core::json::Value;
use datumesh_core::{Args, Cell, ErrorCode, FunctionDescriptor, Receipt, Reference, Scalar, Type};
use datumesh_engine::{Budget, Clock, EvalError, FnBinding, Input, Inputs, Member, Runtime, SlicePattern, SystemClock};
use datumesh_http::client::{Client, ClientError, Fetched};
use datumesh_store::Store;

use crate::config::Config;
use crate::gate::{is_unrooted, LicenceGate};
use crate::index::{HeldCell, Index};
use crate::key::MachineKey;

/// The latest instant.
pub(crate) const END: &str = "9999-12-31T23:59:59.999999Z";
/// The earliest instant.
pub(crate) const START: &str = "0001-01-01T00:00:00.000000Z";

/// What the resolver's parts share.
pub(crate) struct Shared {
    pub(crate) config: Config,
    pub(crate) key: MachineKey,
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
    let kind = if d.contains("dns") || d.contains("resolve") || d.contains("name or service") || d.contains("lookup") {
        "dns"
    } else if d.contains("tls") || d.contains("certificate") || d.contains("rustls") {
        "tls"
    } else if d.contains("timeout") || d.contains("timed out") {
        "timeout"
    } else if d.contains("refused") {
        "refused"
    } else if d.contains("io:") || d.contains("connection") || d.contains("network") || d.contains("unreachable") {
        "network"
    } else {
        return None;
    };
    Some(format!("{kind}: {}", e.detail))
}

/// What happened during one request's evaluation (per thread: the mesh evaluates a request
/// on the thread that asks).
#[derive(Default, Debug)]
pub(crate) struct Ctx {
    /// The gate refused an input: why.
    pub(crate) refused: Option<String>,
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
        SystemClock.now()
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
        self.ensure_namespace(&host)?;
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
    fn gate_check(&self, receipt_hash: &str) -> Result<(), EvalError> {
        if receipt_hash.starts_with("slice:") {
            return Ok(());
        }
        let Some(bytes) = self.store.receipt(receipt_hash) else { return Ok(()) };
        let Ok(r) = Receipt::from_json(&bytes) else { return Ok(()) };
        if self.gate.permits(r.licence(), r.signer_host()) {
            return Ok(());
        }
        let why = format!(
            "the input {}/{} is licensed {}, which is not open here",
            r.cell().ns,
            r.cell().fn_name,
            r.licence()
        );
        note(|c| {
            c.refused.get_or_insert(why.clone());
        });
        Err(EvalError::InvalidInput(datumesh_core::Error::new(ErrorCode::InputUnavailable, "$", why)))
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
        let mut through = Through { inner: inputs, shared: &self.shared };
        self.inner.evaluate(descriptor, cell, &mut through, budget)
    }

    fn asof_dependent(&self, descriptor: &FunctionDescriptor) -> bool {
        self.inner.asof_dependent(descriptor)
    }
}

struct Through<'a> {
    inner: &'a mut dyn Inputs,
    shared: &'a Shared,
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
        self.shared.gate_check(&i.receipt_hash)?;
        Ok(i)
    }

    fn read_asof(&mut self, fn_name: &str, args: &Args, t: &str) -> Result<Input, EvalError> {
        self.cell(fn_name, args)?;
        let i = self.inner.read_asof(fn_name, args, t)?;
        self.shared.gate_check(&i.receipt_hash)?;
        Ok(i)
    }

    fn read_slice(&mut self, fn_name: &str, pattern: &SlicePattern) -> Result<Vec<Member>, EvalError> {
        self.name(fn_name)?;
        let members = self.inner.read_slice(fn_name, pattern)?;
        for m in &members {
            self.shared.gate_check(&m.input.receipt_hash)?;
        }
        Ok(members)
    }

    fn read_versions(&mut self, fn_name: &str, args: &Args, from: &str, to: &str) -> Result<Vec<Input>, EvalError> {
        self.cell(fn_name, args)?;
        let vs = self.inner.read_versions(fn_name, args, from, to)?;
        for i in &vs {
            self.shared.gate_check(&i.receipt_hash)?;
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
