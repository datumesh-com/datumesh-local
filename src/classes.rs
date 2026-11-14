//! Classes in requests and describe (spec-0.5.0, 5.8, 9.7, 16.3, 16.7, 16.8; CONTRACT-local-0.5.0
//! items 1, 2 and 5).
//!
//! A request in the call form (`N(keys).m(args)`, `N[label]`, a bare name through the alias map, a
//! defaulted `asof`) is parsed by formula 0.3.0's `parse_call` against this resolver: the aliases of
//! the alias map (with its `tx`), the bindings and descriptors of the store, the classes and the
//! lookup cells of the store, and the mesh's now. What the parse needs and the store does not hold
//! is fetched from the forward mesh (the bindings resource names a class by its `class_hash`, the
//! describe endpoint gives its flattened descriptor, which is checked against that hash, and the
//! method functions are imported); the second call of the same class is answered from the store.
//!
//! `describe` answers from the mesh over the local store (local classes, and what the store holds),
//! else from the forward mesh's describe endpoint, the answer checked with core's
//! `check_describe_with` against the classes it names.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use datumesh_core::json::{self, Value};
use datumesh_core::{check_describe_with, Arg, ClassDescriptor, ErrorCode, FunctionDescriptor, Resolved, Scalar};
use datumesh_formula::call::{parse_call, CallEnv};
use datumesh_http::client::{pct, ClientError};

use crate::fetch::{host_of, name_of, network_failure, Shared, END};
use crate::gate::is_unrooted;
use crate::resolver::{client_error, Callees, Reply, Resolver, Source};
use crate::{Error, FOREIGN_UNAVAILABLE_STATUS, LOCAL_HOST};

/// The largest document a class or describe fetch reads.
const MAX: u64 = 16 << 20;

/// What a request in the call form came to.
pub(crate) enum Parse {
    /// A call: the cell URI's rest, what it resolved (`resolved`), and whether it fetched.
    Call { rest: String, resolved: Option<Resolved>, fetched: bool },
    /// Answered without a parse: the class or a function was not held and could not be fetched.
    Answered(Reply),
    /// A call that does not parse or bind: the mesh's own answer to it (no network).
    Malformed,
    /// Not a call (a cell URI, a bare name): the request as the cell path reads it.
    Plain,
}

/// The call environment of this resolver: what the store holds, and what it fetches when it does not.
pub(crate) struct Env<'a> {
    resolver: &'a Resolver,
    fetched: Cell<bool>,
    unavailable: RefCell<Option<Reply>>,
}

impl Env<'_> {
    fn shared(&self) -> &Shared {
        &self.resolver.shared
    }

    /// Records the answer the request must give (`E_FOREIGN_UNAVAILABLE`), and says why to the parse.
    fn unavailable(&self, reply: Reply) -> String {
        let why = reply.detail.clone().unwrap_or_default();
        *self.unavailable.borrow_mut() = Some(reply);
        why
    }

    /// A fetch's failure: unavailable when it reached no answer, else its detail.
    fn failed(&self, e: ClientError) -> String {
        match network_failure(&e) {
            Some(why) => self.unavailable(Reply::unavailable(why)),
            None => e.detail,
        }
    }

    fn may_fetch(&self, host: &str) -> Result<(), String> {
        self.shared().may_fetch(host).map_err(|why| self.unavailable(Reply::unavailable(why)))
    }
}

/// A fetch's failure with its code and detail (the client's own constructor is not public).
fn ce(code: ErrorCode, detail: impl Into<String>) -> ClientError {
    ClientError { code, detail: detail.into() }
}

fn store(e: impl std::fmt::Display) -> String {
    format!("store: {e}")
}

impl CallEnv for Env<'_> {
    fn alias(&self, alias: &str) -> Result<Option<String>, String> {
        Ok(self.resolver.aliases().bare().get(alias).cloned())
    }

    fn bound(&self, name: &str, asof: Option<&str>) -> Result<Option<FunctionDescriptor>, String> {
        let s = self.shared();
        let at = asof.unwrap_or(END);
        let host = host_of(name);
        if !is_unrooted(host) && s.store.resolve_binding(name, at).map_err(store)?.is_none() {
            // A method of a class (16.3) is bound by its class, in the class's transaction: the class
            // is held first, so its methods are bound by it, never imported as names of their own.
            if let Some((class, method)) = name.rsplit_once('/') {
                if let Some(cd) = self.bound_class(class, asof)? {
                    if let Some(h) = cd.methods.get(method) {
                        let bytes = s.store.descriptor(h).ok_or_else(|| format!("descriptor {h} is not stored"))?;
                        return FunctionDescriptor::from_json(&bytes).map(Some).map_err(|e| e.to_string());
                    }
                }
            }
            self.may_fetch(host)?;
            match s.import_name(name) {
                Ok(_) => self.fetched.set(true),
                Err(e) if e.code == ErrorCode::NameUnbound => {}
                Err(e) => return Err(self.failed(e)),
            }
        }
        match s.store.resolve_binding(name, at).map_err(store)? {
            Some(b) => {
                let bytes =
                    s.store.descriptor(&b.fn_hash).ok_or_else(|| format!("descriptor {} is not stored", b.fn_hash))?;
                FunctionDescriptor::from_json(&bytes).map(Some).map_err(|e| e.to_string())
            }
            None => Ok(None),
        }
    }

    fn bound_class(&self, name: &str, asof: Option<&str>) -> Result<Option<ClassDescriptor>, String> {
        let s = self.shared();
        let at = asof.unwrap_or(END);
        if let Some(d) = s.held_class(name, at).map_err(|e| e.detail)? {
            return Ok(Some(d));
        }
        let host = host_of(name);
        // A name held as a function is no class. Any other name under another host is asked for its
        // class once (the bindings resource names a class by its class hash, 9.7); a function not
        // held costs that one request before its own import.
        if is_unrooted(host)
            || !datumesh_formula::names::is_function_name(name)
            || s.store.resolve_binding(name, END).map_err(store)?.is_some()
        {
            return Ok(None);
        }
        self.may_fetch(host)?;
        match s.hold_class(host, name) {
            Ok(Some(d)) => {
                self.fetched.set(true);
                Ok(Some(d))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(self.failed(e)),
        }
    }

    fn pinned(&self, fn_hash: &str) -> Result<Option<FunctionDescriptor>, String> {
        match self.shared().store.descriptor(fn_hash) {
            Some(b) => FunctionDescriptor::from_json(&b).map(Some).map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    fn aliases_tx(&self) -> Result<Option<String>, String> {
        Ok(Some(self.resolver.aliases().tx().to_string()))
    }

    fn now(&self) -> Result<Option<String>, String> {
        Ok(Some(self.shared().now()))
    }

    /// The lookup method's cell at the label, read as the answer of the cell path (so a stored cell
    /// is fetched and a formula is evaluated as any request is).
    fn lookup(&self, class: &str, label: &str, asof: &str) -> Result<Option<String>, String> {
        let Some(d) = self.bound_class(class, Some(asof))? else { return Ok(None) };
        let Some(method) = d.lookup.clone() else { return Ok(None) };
        let rest = format!("{class}/{method}?label={}&asof={asof}", pct(label));
        let reply = self.resolver.answer(&rest, &self.fetched).map_err(|e| e.to_string())?;
        if reply.status == FOREIGN_UNAVAILABLE_STATUS {
            return Err(self.unavailable(reply));
        }
        if reply.status != 200 {
            return Ok(None);
        }
        Ok(reply.receipt().and_then(|r| match r.value() {
            Scalar::String(k) => Some(k.clone()),
            _ => None,
        }))
    }
}

/// `resolved` as an envelope member (5.5, 16.7), when the answer's outcome may carry one.
pub(crate) fn with_resolved(mut reply: Reply, resolved: &Resolved) -> Reply {
    let Some(member) = crate::resolved_member(resolved) else { return reply };
    let Ok(Value::Object(mut members)) = json::parse(&reply.body) else { return reply };
    let outcome = members.iter().find(|(k, _)| k == "outcome").and_then(|(_, v)| v.as_str());
    if !matches!(outcome, Some("ok" | "stale" | "not_covered" | "unlicensed" | "payment_required" | "error")) {
        return reply;
    }
    members.retain(|(k, _)| k != "resolved");
    members.push(("resolved".into(), member));
    reply.body = Value::Object(members).to_jcs();
    reply
}

/// A transport failure of a fetch, as the envelope's detail names it.
fn transport(url: &str, e: String) -> Reply {
    let c = ce(ErrorCode::InputUnavailable, format!("{url}: {e}"));
    Reply::unavailable(network_failure(&c).unwrap_or_else(|| format!("network: {url}: {e}")))
}

impl Shared {
    /// The class `name` is bound to in the store as of `at`, flattened, or `None`.
    pub(crate) fn held_class(&self, name: &str, at: &str) -> Result<Option<ClassDescriptor>, ClientError> {
        let Some(b) = self.store.resolve_class_binding(name, at).map_err(crate::fetch::store_err)? else {
            return Ok(None);
        };
        let bytes = self
            .store
            .class(&b.class_hash)
            .map_err(crate::fetch::store_err)?
            .ok_or_else(|| crate::fetch::store_err(format!("class {} is bound but not stored", b.class_hash)))?;
        ClassDescriptor::from_json(&bytes).map(Some).map_err(|e| ce(e.code(), e.to_string()))
    }

    /// Holds the class `name` of `host` (SPEC 9.7, 16.8): the class hash and its binding from the
    /// host's bindings resource, the flattened descriptor from its describe endpoint (checked against
    /// that hash), the method functions, then the class bound in the store from the host's `since`.
    /// `Ok(None)` when the host binds no class to the name (a function, or nothing).
    pub(crate) fn hold_class(&self, host: &str, name: &str) -> Result<Option<ClassDescriptor>, ClientError> {
        if let Some(d) = self.held_class(name, END)? {
            return Ok(Some(d));
        }
        let doc = self.client.resolve(host)?;
        let url = format!("{}?name={}", doc.bindings, pct(name));
        let (status, body) = self.patient(|| self.get_text(&url))?;
        if status != 200 {
            return Err(ce(ErrorCode::InputUnavailable, format!("{url}: status {status}")));
        }
        let v = json::parse(&body).map_err(|e| ce(ErrorCode::InputUnavailable, format!("{url}: {e}")))?;
        let Some(b) = v.get("bindings").and_then(|b| b.get(name)) else { return Ok(None) };
        let Some(class_hash) = b.get("class_hash").and_then(Value::as_str) else { return Ok(None) };
        let since = b.get("since").and_then(Value::as_str).unwrap_or("").to_string();
        let enc = pct(name);
        let describe = doc
            .url("describe", &[("id", enc.as_str())])
            .ok_or_else(|| ce(ErrorCode::MeshDoc, format!("{host} has no describe endpoint")))?;
        let (status, answer) = self.patient(|| self.get_text(&describe))?;
        if status != 200 {
            return Err(ce(ErrorCode::InputUnavailable, format!("{describe}: status {status}")));
        }
        check_describe_with(&answer, &BTreeMap::new(), &[]).map_err(|e| ce(e.code(), e.to_string()))?;
        let doc_answer = json::parse(&answer).map_err(|e| ce(ErrorCode::Describe, e.to_string()))?;
        let descriptor = match (doc_answer.get("kind").and_then(Value::as_str), doc_answer.get("descriptor")) {
            (Some("class"), Some(d)) => ClassDescriptor::from_value(d).map_err(|e| ce(e.code(), e.to_string()))?,
            _ => return Ok(None),
        };
        if descriptor.hash() != class_hash {
            return Err(ce(ErrorCode::Describe, format!("{describe}: not the class {host} binds")));
        }
        self.ensure_publisher(host)?;
        self.store.put_class(&descriptor.canonical()).map_err(crate::fetch::store_err)?;
        for h in descriptor.methods.values() {
            self.import_descriptor(host, h)?;
        }
        self.store.bind_class(name, class_hash, &since).map_err(crate::fetch::store_err)?;
        Ok(Some(descriptor))
    }

    fn get_text(&self, url: &str) -> Result<(u16, Vec<u8>), ClientError> {
        self.client.get(url, MAX).map_err(|e| ce(ErrorCode::InputUnavailable, format!("{url}: {e}")))
    }
}

impl Resolver {
    /// Parses a request in the call form (see the module documentation).
    pub(crate) fn parse_request(&self, text: &str) -> Parse {
        let env = Env { resolver: self, fetched: Cell::new(false), unavailable: RefCell::new(None) };
        match parse_call(text, &env) {
            Ok(call) => {
                if call.cell.args.values().any(|a| matches!(a, Arg::Slice)) {
                    return Parse::Plain;
                }
                Parse::Call {
                    rest: call.uri()["cell://".len()..].to_string(),
                    resolved: call.resolved,
                    fetched: env.fetched.get(),
                }
            }
            Err(_) => {
                if let Some(reply) = env.unavailable.into_inner() {
                    return Parse::Answered(reply);
                }
                if text.contains('(') {
                    Parse::Malformed
                } else {
                    Parse::Plain
                }
            }
        }
    }

    /// Checks a describe answer from the forward mesh: the class it names is held (fetched, and
    /// bound to its hash), then core's check against what is held (16.8). A failed check is an error.
    fn hold_described(&self, host: &str, answer: &[u8]) -> Result<(), Error> {
        let s = &self.shared;
        let bad = |d: String| Error::Client { code: ErrorCode::Describe.as_str().into(), detail: d };
        let v = json::parse(answer).map_err(|e| bad(e.to_string()))?;
        let class = match v.get("kind").and_then(Value::as_str) {
            Some("class") => v.get("class"),
            Some("object") => v.get("object").and_then(|o| o.get("class")),
            _ => None,
        }
        .and_then(Value::as_str)
        .ok_or_else(|| bad("a describe answer names its class".into()))?
        .to_string();
        let d = s
            .hold_class(host, &class)
            .map_err(|e| client_error(&e))?
            .ok_or_else(|| bad(format!("{class} is not bound at {host}")))?;
        let functions: Vec<FunctionDescriptor> = d
            .methods
            .values()
            .filter_map(|h| s.store.descriptor(h))
            .filter_map(|b| FunctionDescriptor::from_json(&b).ok())
            .collect();
        let held = BTreeMap::from([(class, d)]);
        check_describe_with(answer, &held, &functions)
            .map_err(|e| Error::Client { code: e.code_str().into(), detail: e.to_string() })?;
        Ok(())
    }

    /// Defines a class from its descriptor (JSON, as an author writes it, 16.1) as `name`, under
    /// `local/` only (owner `local.invalid`): `define_class`, `put_class`, `bind_class` from now.
    /// [`crate::Defined::fn_hash`] is the class hash. Since spec-0.5.0.
    pub fn define_class(&self, name: &str, descriptor: &[u8]) -> Result<crate::Defined, Error> {
        let s = &self.shared;
        let full = self.aliases().expand(name);
        if host_of(&full) != LOCAL_HOST || !datumesh_formula::names::is_function_name(&full) {
            return Err(Error::Request(format!("{name}: a class is defined under local/ only")));
        }
        let now = s.now();
        let def =
            datumesh_formula::definition::define_class(&full, descriptor, LOCAL_HOST, &Callees { shared: s }, &now)
                .map_err(|e| Error::Define(e.to_string()))?;
        let h = s.store.put_class(&def.bytes).map_err(|e| Error::Store(e.to_string()))?;
        let bound = match s.store.resolve_class(&full, END).map_err(|e| Error::Store(e.to_string()))? {
            Some(x) if x == h => false,
            _ => {
                s.store.bind_class(&full, &h, &now).map_err(|e| Error::Store(e.to_string()))?;
                true
            }
        };
        self.mesh().clear_memo();
        Ok(crate::Defined { name: full, fn_hash: h, bound })
    }

    /// describe (SPEC 16.8): `id` is a class name, a cell id, an object id, a short id or an
    /// object's key encoding (short names through the alias map). Answered by the mesh over the
    /// local store when it can (a held class, object or cell); else by the forward mesh's describe
    /// endpoint (the host of a name or an encoding, else the configured forward mesh), whose answer
    /// is checked before it is returned. Since spec-0.5.0.
    pub fn describe(&self, id: &str) -> Result<Reply, Error> {
        let s = &self.shared;
        let full = self.aliases().expand(id);
        let local = self.mesh().describe(&full);
        if local.status != 404 {
            if local.status == 200 && full.contains('(') {
                self.remember_object(&full, &local.body)?;
            }
            return Ok(self.mesh_reply(local, Source::Local));
        }
        // A short id or an object id of an object this machine described by its encoding: the mesh
        // knows objects by their cells only, so the encoding is answered again.
        if is_hex(&full, 64) || is_hex(&full, 12) {
            let encodings = s.index.objects_with(&full)?;
            if let [encoding] = encodings.as_slice() {
                let again = self.mesh().describe(encoding);
                if again.status == 200 {
                    return Ok(self.mesh_reply(again, Source::Local));
                }
            }
        }
        let name = name_of(&full);
        let target = if datumesh_formula::names::is_function_name(name) {
            host_of(name).to_string()
        } else {
            match &s.config.forward_mesh {
                Some(m) => m.clone(),
                None => return Ok(self.mesh_reply(local, Source::Local)),
            }
        };
        if target == LOCAL_HOST {
            return Ok(self.mesh_reply(local, Source::Local));
        }
        if let Err(why) = s.may_fetch(&target) {
            return Ok(Reply::unavailable(why));
        }
        let doc = match s.client.resolve(&target) {
            Ok(d) => d,
            Err(e) => {
                return Ok(match network_failure(&e) {
                    Some(w) => Reply::unavailable(w),
                    None => return Err(client_error(&e)),
                })
            }
        };
        let enc = pct(&full);
        let Some(url) = doc.url("describe", &[("id", enc.as_str())]) else {
            return Err(Error::Client {
                code: ErrorCode::MeshDoc.as_str().into(),
                detail: format!("{target} has no describe endpoint"),
            });
        };
        let (status, body) = match s.client.get(&url, MAX) {
            Ok(x) => x,
            Err(e) => return Ok(transport(&url, e)),
        };
        if status == 200 {
            self.hold_described(&target, &body)?;
            if full.contains('(') {
                self.remember_object(&full, &body)?;
            }
        }
        Ok(Reply::new(status, body, Source::Forwarded { mesh: target }))
    }

    /// Remembers the object an answer describes, by its key encoding (for its short id later).
    fn remember_object(&self, encoding: &str, body: &[u8]) -> Result<(), Error> {
        let Ok(v) = json::parse(body) else { return Ok(()) };
        if v.get("kind").and_then(Value::as_str) != Some("object") {
            return Ok(());
        }
        let Some(id) = v.get("object_id").and_then(Value::as_str) else { return Ok(()) };
        self.shared.index.hold_object(id, encoding, &self.shared.now())
    }
}

/// Whether `text` is `n` hex digits.
fn is_hex(text: &str, n: usize) -> bool {
    text.len() == n && text.bytes().all(|b| b.is_ascii_hexdigit())
}
