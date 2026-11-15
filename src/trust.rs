//! Trusted machines (CONTRACT-1.8b L4): other machines' key documents, trusted here.
//!
//! `trust.json` (mode 0600, in the configuration directory) holds one entry per trusted host:
//! its key document as trusted (`key_document`), when it was trusted and when it was revoked.
//! The **effective** document of a host is that document with every key's `valid_to` set to
//! `min(valid_to, revoked_at)` when revoked; it is kept in the store under the host, where
//! verification reads it. It is never merged into this machine's own key document. A receipt
//! from a trusted host verifies with its effective document and is answered with the label
//! `trusted machine <host>`; a receipt issued at or after the revocation fails with
//! `E_KEY_NOT_VALID`.

use std::path::Path;

use datumesh_core::json::{self, Value};

use crate::config::is_machine_host;
use crate::key::{key_entries, write_private};
use crate::resolver::Resolver;
use crate::Error;

/// The trust file in the configuration directory.
const TRUST_FILE: &str = "trust.json";

/// A trusted machine: its host, when it was trusted, and when it was revoked (if it was).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustedMachine {
    /// The machine's host (a name under `.invalid`).
    pub host: String,
    /// When it was first trusted.
    pub trusted_from: String,
    /// When it was revoked; a receipt issued at or after this instant does not verify.
    pub revoked_at: Option<String>,
}

/// A trusted machine as kept in `trust.json`.
#[derive(Clone, Debug)]
pub(crate) struct Machine {
    pub(crate) host: String,
    /// The key document as trusted (its bytes, canonical JSON).
    pub(crate) document: Vec<u8>,
    pub(crate) trusted_from: String,
    pub(crate) revoked_at: Option<String>,
}

impl Machine {
    fn info(&self) -> TrustedMachine {
        TrustedMachine {
            host: self.host.clone(),
            trusted_from: self.trusted_from.clone(),
            revoked_at: self.revoked_at.clone(),
        }
    }

    /// The document verification uses: revocation cuts every key's `valid_to`.
    fn effective(&self) -> Result<Vec<u8>, Error> {
        effective(&self.document, self.revoked_at.as_deref())
    }
}

/// The effective key document of `document` revoked at `revoked_at` (none: the document as it is).
pub(crate) fn effective(document: &[u8], revoked_at: Option<&str>) -> Result<Vec<u8>, Error> {
    let Some(r) = revoked_at else { return Ok(document.to_vec()) };
    let mut v = json::parse(document).map_err(|e| Error::Trust(e.to_string()))?;
    if let Value::Object(members) = &mut v {
        for (k, keys) in members.iter_mut() {
            let Value::Array(keys) = keys else { continue };
            if k != "keys" {
                continue;
            }
            for key in keys.iter_mut() {
                let Value::Object(km) = key else { continue };
                match km.iter_mut().find(|(n, _)| n == "valid_to") {
                    Some((_, vt)) => {
                        if vt.as_str().is_none_or(|x| x > r) {
                            *vt = Value::String(r.to_string());
                        }
                    }
                    None => km.push(("valid_to".into(), Value::String(r.to_string()))),
                }
            }
        }
    }
    Ok(v.to_jcs())
}

/// The machines trusted in the configuration directory (none when there is no file).
pub(crate) fn load(config_dir: &Path) -> Result<Vec<Machine>, Error> {
    let path = config_dir.join(TRUST_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read(&path).map_err(|e| Error::Trust(format!("{}: {e}", path.display())))?;
    let bad = || Error::Trust(format!("{}: not a trust file", path.display()));
    let v = json::parse(&text).map_err(|_| bad())?;
    let Some(Value::Array(list)) = v.get("machines") else { return Err(bad()) };
    let mut out = Vec::new();
    for m in list {
        let host = m.get("host").and_then(Value::as_str).ok_or_else(bad)?.to_string();
        let doc = m.get("key_document").ok_or_else(bad)?.to_jcs();
        let trusted_from = m.get("trusted_from").and_then(Value::as_str).ok_or_else(bad)?.to_string();
        let revoked_at = match m.get("revoked_at") {
            Some(Value::String(r)) => Some(r.clone()),
            Some(Value::Null) | None => None,
            Some(_) => return Err(bad()),
        };
        out.push(Machine { host, document: doc, trusted_from, revoked_at });
    }
    Ok(out)
}

/// Writes the machines to `trust.json` (mode 0600).
fn save(config_dir: &Path, machines: &[Machine]) -> Result<(), Error> {
    let list = machines
        .iter()
        .map(|m| {
            let doc = json::parse(&m.document).map_err(|e| Error::Trust(e.to_string()))?;
            Ok(Value::Object(vec![
                ("host".into(), Value::String(m.host.clone())),
                ("key_document".into(), doc),
                ("revoked_at".into(), m.revoked_at.clone().map_or(Value::Null, Value::String)),
                ("trusted_from".into(), Value::String(m.trusted_from.clone())),
            ]))
        })
        .collect::<Result<Vec<Value>, Error>>()?;
    let doc = Value::Object(vec![("machines".into(), Value::Array(list))]).to_jcs();
    let path = config_dir.join(TRUST_FILE);
    write_private(&path, &doc).map_err(|e| Error::Trust(format!("{}: {e}", path.display())))
}

/// Whether `host` is trusted here and not revoked.
pub(crate) fn trusted_live(config_dir: &Path, host: &str) -> Result<bool, Error> {
    Ok(load(config_dir)?.iter().any(|m| m.host == host && m.revoked_at.is_none()))
}

/// Whether `host` is trusted here, revoked or not.
pub(crate) fn trusted(config_dir: &Path, host: &str) -> Result<bool, Error> {
    Ok(load(config_dir)?.iter().any(|m| m.host == host))
}

/// The `(id, public)` pairs of a key document's keys.
fn key_pairs(document: &[u8]) -> Vec<(String, String)> {
    key_entries(document)
        .iter()
        .map(|e| {
            let f = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            (f("id"), f("public"))
        })
        .collect()
}

impl Resolver {
    /// Trusts another machine's key document (`keydoc`, canonical JSON as its `key document`
    /// answers it): the host is the document's. Refused when the host is this machine's, is not
    /// a name under `.invalid`, or is trusted already and the document does not list every key
    /// trusted before (same `id` and `public`, retired or not). A trusted host's document
    /// replaces the old one and keeps `trusted_from` and `revoked_at`. Its effective document is
    /// kept in the store under the host; this machine's own document is not changed.
    pub fn trust_machine(&self, keydoc: &[u8]) -> Result<TrustedMachine, Error> {
        let s = &self.shared;
        let v = json::parse(keydoc).map_err(|e| Error::Trust(format!("key document: {e}")))?;
        let host = v
            .get("host")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Trust("key document: no host".into()))?
            .to_string();
        if host == s.config.host {
            return Err(Error::Trust(format!("{host} is this machine's own host")));
        }
        if !is_machine_host(&host) {
            return Err(Error::Trust(format!("{host}: a trusted machine's host is a name under .invalid")));
        }
        datumesh_core::keys_from_response(&host, 200, Some(keydoc))
            .map_err(|e| Error::Trust(format!("{host}: the key document: {e}")))?;
        let now = s.now();
        let dir = &s.config.config_dir;
        let mut machines = load(dir)?;
        let info = match machines.iter().position(|m| m.host == host) {
            Some(i) => {
                let new = key_pairs(keydoc);
                for (id, public) in key_pairs(&machines[i].document) {
                    if !new.iter().any(|(i2, p2)| *i2 == id && *p2 == public) {
                        return Err(Error::Trust(format!(
                            "{host}: the replacement document does not list the trusted key {id}"
                        )));
                    }
                }
                machines[i].document = keydoc.to_vec();
                machines[i].info()
            }
            None => {
                machines.push(Machine {
                    host: host.clone(),
                    document: keydoc.to_vec(),
                    trusted_from: now.clone(),
                    revoked_at: None,
                });
                machines[machines.len() - 1].info()
            }
        };
        save(dir, &machines)?;
        let eff = effective(keydoc, info.revoked_at.as_deref())?;
        s.store.put_namespace(&host, &eff, &now).map_err(|e| Error::Store(e.to_string()))?;
        Ok(info)
    }

    /// Revokes trust in a trusted machine: from now (`revoked_at`), its keys end then, so a
    /// receipt it signs at or after now fails verification and an earlier one still verifies.
    /// A machine revoked already keeps its first `revoked_at`.
    pub fn revoke_machine(&self, host: &str) -> Result<(), Error> {
        let s = &self.shared;
        let now = s.now();
        let dir = &s.config.config_dir;
        let mut machines = load(dir)?;
        let m = machines
            .iter_mut()
            .find(|m| m.host == host)
            .ok_or_else(|| Error::Trust(format!("{host} is not a trusted machine")))?;
        if m.revoked_at.is_none() {
            m.revoked_at = Some(now.clone());
        }
        let effective = m.effective()?;
        save(dir, &machines)?;
        s.store.put_namespace(host, &effective, &now).map_err(|e| Error::Store(e.to_string()))?;
        Ok(())
    }

    /// The trusted machines, revoked ones included, by host.
    pub fn trusted_machines(&self) -> Result<Vec<TrustedMachine>, Error> {
        Ok(load(&self.shared.config.config_dir)?.iter().map(Machine::info).collect())
    }
}
