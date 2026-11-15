//! Export and import of a working set: a bundle of receipts, descriptors, bindings, versions,
//! manifests, blobs, key documents and forwarded answers, so one resolver's working set can
//! seed another's. The bundle is signed where its contents are signed: every receipt carries
//! its signer's signature and is verified again on import; nothing else is trusted
//! (descriptors and blobs are content-addressed, key documents are checked as SPEC 9.2 says).
//!
//! The exporting machine's key document (its host's) travels in the bundle. On import, a receipt
//! signed by another machine's local key is kept aside and labelled
//! [`crate::UNVERIFIABLE_LABEL`], unless its host is trusted here (CONTRACT-1.8b L4). `--trust-key
//! <key id>` trusts the host of the bundle's key document that carries that key, as
//! `trust_machine` does: its receipts then verify and answer, labelled `trusted machine <host>`.
//! It is refused for this machine's own host, whose documents are never merged. Receipts signed
//! under rooted namespaces verify normally.
//!
//! The format is one JSON object (canonical JSON), `"bundle": "datumesh-local/1"`; byte
//! strings (receipts, descriptors, key documents, manifests, envelopes) are kept as JSON
//! strings of their exact text, blobs in base64.

use std::collections::BTreeSet;

use datumesh_core::json::{self, Value};
use datumesh_core::{Receipt, Type};

use crate::fetch::{cell_rest, host_of, END, START};
use crate::gate::is_unrooted;
use crate::index::{Forwarded, HeldCell};
use crate::key::key_entries;
use crate::trust;
use crate::{Error, Resolver};

/// The bundle format.
pub const FORMAT: &str = "datumesh-local/1";

/// What an import did.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// Receipts stored (new or already there).
    pub receipts: usize,
    /// Stored versions added.
    pub versions: usize,
    /// Computed versions added.
    pub computed: usize,
    /// Descriptors stored.
    pub descriptors: usize,
    /// Names bound.
    pub bindings: usize,
    /// Manifests stored.
    pub manifests: usize,
    /// Blobs stored.
    pub blobs: usize,
    /// Forwarded answers kept.
    pub forwarded: usize,
    /// Receipts kept aside: signed by another machine's local key, not trusted (hashes).
    pub quarantined: Vec<String>,
    /// Machine keys trusted by this import.
    pub trusted: Vec<String>,
    /// What failed, and why (the rest was imported).
    pub failed: Vec<String>,
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 (RFC 4648, padded).
pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n =
            (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Decodes [`base64`].
pub fn unbase64(s: &str) -> Option<Vec<u8>> {
    let s = s.as_bytes();
    if !s.len().is_multiple_of(4) {
        return None;
    }
    let val = |b: u8| B64.iter().position(|&x| x == b).map(|p| p as u32);
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for (i, c) in s.chunks(4).enumerate() {
        let last = i == s.len() / 4 - 1;
        let pad = c.iter().rev().take_while(|&&b| b == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &b in &c[..4 - pad] {
            n = (n << 6) | val(b)?;
        }
        n <<= 6 * pad as u32;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

fn text(b: &[u8]) -> Value {
    Value::String(String::from_utf8_lossy(b).into_owned())
}

fn obj(m: Vec<(&str, Value)>) -> Value {
    Value::Object(m.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn st(e: impl std::fmt::Display) -> Error {
    Error::Store(e.to_string())
}

impl Resolver {
    /// The bundle of everything held: names (bindings, descriptors), stored cells (every
    /// version), the receipts this machine answered with and their lineage (input receipts,
    /// manifests), blobs held for those values, forwarded answers, and the key documents of
    /// every signer (this machine's own included).
    pub fn export(&self) -> Result<Vec<u8>, Error> {
        let s = &self.shared;
        let store = &s.store;
        let mut descriptors = BTreeSet::new();
        let mut bindings = Vec::new();
        for (name, _) in s.index.names()? {
            if let Some(b) = store.resolve_binding(&name, END).map_err(st)? {
                descriptors.insert(b.fn_hash.clone());
                bindings.push(obj(vec![
                    ("fn_hash", Value::String(b.fn_hash)),
                    ("name", Value::String(name)),
                    ("since", Value::String(b.tx)),
                ]));
            }
        }
        let mut receipts: Vec<String> = Vec::new();
        let mut seen = BTreeSet::new();
        let mut versions = Vec::new();
        for c in s.index.cells()? {
            for v in store.versions(&c.cell_id, START, END).map_err(st)? {
                versions.push(obj(vec![
                    ("cell_id", Value::String(c.cell_id.clone())),
                    ("receipt", Value::String(v.receipt_hash.clone())),
                    ("tx", Value::String(v.tx.clone())),
                ]));
                if seen.insert(v.receipt_hash.clone()) {
                    receipts.push(v.receipt_hash);
                }
            }
        }
        let mut computed = Vec::new();
        let mut manifests = BTreeSet::new();
        let mut todo: Vec<String> = Vec::new();
        for (h, cell_id) in s.index.all_answered()? {
            if let Some(bytes) = store.receipt(&h) {
                if let Ok(r) = Receipt::from_json(&bytes) {
                    computed.push(obj(vec![
                        ("cell_id", Value::String(cell_id)),
                        ("receipt", Value::String(h.clone())),
                        ("tx", Value::String(r.tx().to_string())),
                    ]));
                }
            }
            todo.push(h);
        }
        for f in s.index.all_forwarded()? {
            if let Some(r) = json::parse(&f.body).ok().and_then(|v| v.get("receipt").cloned()) {
                if let Ok(r) = Receipt::from_value(&r) {
                    todo.push(r.hash().to_string());
                }
            }
        }
        // The lineage of computed receipts: inputs and slice entries, recursively.
        while let Some(h) = todo.pop() {
            if h.starts_with("slice:") {
                if manifests.insert(h.clone()) {
                    if let Some(m) = store.manifest(&h) {
                        if let Ok(m) = datumesh_core::Manifest::from_json(&m) {
                            todo.extend(m.members().iter().cloned());
                        }
                    }
                }
                continue;
            }
            if seen.insert(h.clone()) {
                receipts.push(h.clone());
            }
            if let Some(bytes) = store.receipt(&h) {
                if let Ok(r) = Receipt::from_json(&bytes) {
                    for i in r.inputs() {
                        if !seen.contains(i) && !manifests.contains(i) {
                            todo.push(i.clone());
                        }
                    }
                }
            }
        }
        let mut hosts = BTreeSet::from([s.config.host.clone()]);
        let mut receipt_values = Vec::new();
        let mut blobs = Vec::new();
        for h in &receipts {
            let Some(bytes) = store.receipt(h) else { continue };
            if let Ok(r) = Receipt::from_json(&bytes) {
                hosts.insert(r.signer_host().to_string());
                descriptors.insert(r.cell().fn_hash.clone());
                if r.value().ty() == Type::Blob {
                    let sha = r.value().lexical().split(':').next().unwrap_or("").to_string();
                    if let Some(b) = store.blob(&sha).map_err(st)? {
                        blobs.push((sha, base64(&b)));
                    }
                }
            }
            receipt_values.push(text(&bytes));
        }
        blobs.sort();
        blobs.dedup();
        let mut keydocs = Vec::new();
        for h in hosts {
            if let Some(n) = store.namespace(&h).map_err(st)? {
                keydocs.push((h, text(&n.keydoc)));
            }
        }
        let descriptor_values: Vec<Value> =
            descriptors.iter().filter_map(|h| store.descriptor(h)).map(|b| text(&b)).collect();
        let manifest_values: Vec<Value> =
            manifests.iter().filter_map(|e| store.manifest(e)).map(|b| text(&b)).collect();
        let forwarded: Vec<Value> = s
            .index
            .all_forwarded()?
            .into_iter()
            .map(|f| {
                obj(vec![
                    ("body", text(&f.body)),
                    ("cell_id", Value::String(f.cell_id)),
                    ("mesh", Value::String(f.mesh)),
                    ("rest", Value::String(f.rest)),
                    ("status", Value::String(f.status.to_string())),
                ])
            })
            .collect();
        Ok(obj(vec![
            ("bindings", Value::Array(bindings)),
            ("blobs", Value::Object(blobs.into_iter().map(|(k, v)| (k, Value::String(v))).collect())),
            ("bundle", Value::String(FORMAT.into())),
            ("computed", Value::Array(computed)),
            ("descriptors", Value::Array(descriptor_values)),
            ("exported_at", Value::String(s.now())),
            ("forwarded", Value::Array(forwarded)),
            ("key_documents", Value::Object(keydocs)),
            ("machine", Value::String(self.key_id().to_string())),
            ("manifests", Value::Array(manifest_values)),
            ("receipts", Value::Array(receipt_values)),
            ("versions", Value::Array(versions)),
        ])
        .to_jcs())
    }

    /// Imports a bundle (see the module documentation). `trust` names key ids of other machines
    /// (in the bundle's key documents) whose hosts are trusted here.
    pub fn import(&self, bundle: &[u8], trust: &[String]) -> Result<Imported, Error> {
        let s = &self.shared;
        let store = &s.store;
        let now = s.now();
        let v = json::parse(bundle).map_err(|e| Error::Bundle(e.to_string()))?;
        if v.get("bundle").and_then(Value::as_str) != Some(FORMAT) {
            return Err(Error::Bundle(format!("not a {FORMAT} bundle")));
        }
        let arr = |k: &str| match v.get(k) {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let strs = |k: &str| -> Vec<String> { arr(k).iter().filter_map(|x| x.as_str().map(str::to_string)).collect() };
        let mut out = Imported::default();
        let own = s.config.host.clone();
        let docs: Vec<(String, String)> = v
            .get("key_documents")
            .and_then(Value::as_object)
            .map(|d| d.iter().filter_map(|(h, doc)| doc.as_str().map(|t| (h.clone(), t.to_string()))).collect())
            .unwrap_or_default();
        // Rooted hosts' key documents are stored when absent here. An unrooted host's are taken
        // only by `--trust-key` (below): never merged into this machine's own document.
        for (host, doc) in &docs {
            if is_unrooted(host) {
                continue;
            }
            if store.namespace(host).map_err(st)?.is_none() {
                if let Err(e) = store.put_namespace(host, doc.as_bytes(), &now) {
                    out.failed.push(format!("key document {host}: {e}"));
                }
            }
        }
        for t in trust {
            let carries =
                |d: &str| key_entries(d.as_bytes()).iter().any(|e| e.get("id").and_then(Value::as_str) == Some(t));
            let Some((host, doc)) = docs.iter().find(|(h, d)| is_unrooted(h) && carries(d)) else {
                return Err(Error::Bundle(format!("--trust-key {t}: no such key in the bundle's key documents")));
            };
            if *host == own {
                return Err(Error::Bundle(format!(
                    "--trust-key {t}: the key is of this machine's own host {own}, which is never trusted"
                )));
            }
            self.trust_machine(doc.as_bytes())?;
            if !out.trusted.contains(host) {
                out.trusted.push(host.clone());
            }
        }
        let trusted: BTreeSet<String> = trust::load(&s.config.config_dir)?.into_iter().map(|m| m.host).collect();
        let local_ids: BTreeSet<String> = key_entries(&self.key_document())
            .iter()
            .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_string))
            .collect();
        for d in strs("descriptors") {
            match store.put_descriptor(d.as_bytes()) {
                Ok(_) => out.descriptors += 1,
                Err(e) => out.failed.push(format!("descriptor: {e}")),
            }
        }
        let mut bindings = arr("bindings");
        bindings.sort_by(|a, b| a.get("since").and_then(Value::as_str).cmp(&b.get("since").and_then(Value::as_str)));
        for b in bindings {
            let g = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let (name, h, since) = (g("name"), g("fn_hash"), g("since"));
            match store.resolve_binding(&name, END).map_err(st)? {
                Some(x) if x.fn_hash == h => {}
                Some(_) => out
                    .failed
                    .push(format!("{name}: bound here to another function; the bundle's binding is not taken")),
                None => match store.bind_name(&name, &h, &since) {
                    Ok(()) => out.bindings += 1,
                    Err(e) => {
                        out.failed.push(format!("bind {name}: {e}"));
                        continue;
                    }
                },
            }
            s.index.hold_name(&name, host_of(&name), &now)?;
        }
        // Receipts: those signed by an untrusted local key, or by an unrooted host not trusted here,
        // are kept aside.
        let mut stored = BTreeSet::new();
        for text in strs("receipts") {
            let r = match Receipt::from_json(text.as_bytes()) {
                Ok(r) => r,
                Err(e) => {
                    out.failed.push(format!("receipt: {e}"));
                    continue;
                }
            };
            let sh = r.signer_host();
            let aside =
                if sh == own { !local_ids.contains(r.key_id()) } else { is_unrooted(sh) && !trusted.contains(sh) };
            if aside {
                s.index.quarantine(r.hash(), r.key_id(), text.as_bytes(), &now)?;
                out.quarantined.push(r.hash().to_string());
                continue;
            }
            match store.put_receipt(text.as_bytes()) {
                Ok(h) => {
                    s.index.release(&h)?;
                    stored.insert(h);
                    out.receipts += 1;
                }
                Err(e) => out.failed.push(format!("receipt {}: {e}", r.hash())),
            }
        }
        for m in strs("manifests") {
            match store.put_manifest(m.as_bytes()) {
                Ok(_) => out.manifests += 1,
                Err(e) => out.failed.push(format!("manifest: {e}")),
            }
        }
        if let Some(bs) = v.get("blobs").and_then(Value::as_object) {
            for (sha, b) in bs {
                match b.as_str().and_then(unbase64) {
                    Some(bytes) if datumesh_core::sha256_hex(&bytes) == *sha => match store.put_blob(&bytes) {
                        Ok(_) => out.blobs += 1,
                        Err(e) => out.failed.push(format!("blob {sha}: {e}")),
                    },
                    _ => out.failed.push(format!("blob {sha}: content of another hash")),
                }
            }
        }
        let mesh = self.mesh();
        let mut entries: Vec<(String, String, String, bool)> = Vec::new();
        for (k, computed) in [("versions", false), ("computed", true)] {
            for x in arr(k) {
                let g = |n: &str| x.get(n).and_then(Value::as_str).unwrap_or("").to_string();
                entries.push((g("cell_id"), g("tx"), g("receipt"), computed));
            }
        }
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        for (cell_id, tx, h, computed) in entries {
            if !stored.contains(&h) {
                continue;
            }
            let r = match store.receipt(&h).map(|b| Receipt::from_json(&b)) {
                Some(Ok(r)) => r,
                _ => continue,
            };
            if computed {
                match store.computed_asof(&cell_id, END).map_err(st)? {
                    Some(c) if c.tx.as_str() >= tx.as_str() => {}
                    _ => match store.put_computed(&cell_id, &tx, &h) {
                        Ok(()) => {
                            out.computed += 1;
                            s.index.answered(&h, &cell_id, &now)?;
                        }
                        Err(e) => out.failed.push(format!("computed {cell_id}: {e}")),
                    },
                }
                continue;
            }
            match store.latest(&cell_id).map_err(st)? {
                Some(l) if l.tx.as_str() >= tx.as_str() => {}
                _ => match store.put_version(&cell_id, &tx, &h) {
                    Ok(()) => out.versions += 1,
                    Err(e) => {
                        out.failed.push(format!("version {cell_id}: {e}"));
                        continue;
                    }
                },
            }
            let held = HeldCell {
                cell_id: cell_id.clone(),
                host: host_of(&r.cell().ns).to_string(),
                rest: cell_rest(r.cell()),
                slice: None,
            };
            s.index.hold_cell(&held, &now)?;
            mesh.stored_version_added(&cell_id);
        }
        for f in arr("forwarded") {
            let g = |n: &str| f.get(n).and_then(Value::as_str).unwrap_or("").to_string();
            let fw = Forwarded {
                rest: g("rest"),
                mesh: g("mesh"),
                status: g("status").parse().unwrap_or(200),
                body: g("body").into_bytes(),
                cell_id: g("cell_id"),
            };
            s.index.put_forwarded(&fw, &now)?;
            out.forwarded += 1;
        }
        mesh.clear_memo();
        Ok(out)
    }
}
