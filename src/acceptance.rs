//! Licence acceptance (datumesh-local 0.5.1, L4; DECISIONS-round-protocol D4, D4.1 to D4.4).
//!
//! The operator accepts a restricted licence at the command line (`datumesh licence accept`); the
//! acceptance is recorded in `licences.json` in the configuration directory, as a list of records
//! in the order they happened: `{licence, accepted_at}`, and `{licence, revoked_at}` for a
//! revocation, which keeps the history. A licence is in force when its latest record is an
//! acceptance. The licences in force are the engine's local acceptance policy (engine 0.6.1, E1):
//! on this machine a calculation over restricted inputs is computed when every restricted input's
//! licence is in force, and carries the one licence, or [`LOCAL_DERIVATION_LICENCE`] over two or
//! more. No MCP tool accepts a licence: an agent's refusal says the user must accept it.
//!
//! A result this machine signed under a restricted licence, or under [`LOCAL_DERIVATION_LICENCE`],
//! is never published or exported ([`NOT_PUBLISHABLE`], L5).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use datumesh_core::inherit_licence;
use datumesh_core::json::{self, Value};

use crate::fetch::START;
use crate::key::write_private;
use crate::{Error, Resolver};

/// The licences file, in the configuration directory.
pub const LICENCES_FILE: &str = "licences.json";

/// The error code of a result this machine may not publish or export (L5).
pub const NOT_PUBLISHABLE: &str = "E_LICENCE_NOT_PUBLISHABLE";

/// The licence of a local result over two or more restricted licences (engine 0.6.1, E1).
pub use datumesh_engine::LOCAL_DERIVATION_LICENCE;

/// Whether `licence` is restricted under SPEC 8.5: not one of the open tiers' identifiers (an https
/// URL, a LicenseRef, any other licence). Core decides it with no declarations.
pub fn is_restricted(licence: &str) -> bool {
    inherit_licence([licence], None, "local.invalid/licence", START, "").licence.is_none()
}

/// The licences in force of a history of records: each licence whose latest record is an
/// acceptance, with that acceptance's instant.
fn in_force_of(records: &[Value]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for r in records {
        let Some(l) = r.get("licence").and_then(Value::as_str) else { continue };
        if let Some(at) = r.get("accepted_at").and_then(Value::as_str) {
            out.insert(l.to_string(), at.to_string());
        } else if r.get("revoked_at").is_some() {
            out.remove(l);
        }
    }
    out
}

/// One licence in force, with when it was accepted.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedLicence {
    /// The licence, as receipts carry it.
    pub licence: String,
    /// The canonical instant of the acceptance.
    pub accepted_at: String,
}

/// A dataset of the licence register whose licence page is the licence accepted (D4.4).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredDataset {
    /// The dataset's source.
    pub source: String,
    /// The dataset's code.
    pub code: String,
    /// The register's licence hash of the dataset, when it has one.
    pub licence_hash: Option<String>,
    /// The quoted clauses of the dataset, when they could be read.
    pub clauses: Option<String>,
}

/// What `licence accept` showed and recorded.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accepted {
    /// The licence accepted.
    pub licence: String,
    /// The canonical instant recorded.
    pub accepted_at: String,
    /// The register's datasets whose licence page is the licence (none when there are none).
    pub datasets: Vec<RegisteredDataset>,
    /// Why the register could not be read, when it could not.
    pub register_note: Option<String>,
}

/// The licences in force of a file's reading, with the file's modification time.
type Reading = (Option<SystemTime>, BTreeMap<String, String>);

/// The licences file of a configuration directory, with its cached reading.
#[derive(Debug)]
pub(crate) struct Acceptance {
    path: PathBuf,
    cache: Mutex<Option<Reading>>,
}

impl Acceptance {
    /// The acceptance of the configuration directory `config_dir`.
    pub(crate) fn new(config_dir: &Path) -> Acceptance {
        Acceptance { path: config_dir.join(LICENCES_FILE), cache: Mutex::new(None) }
    }

    fn records(&self) -> Result<Vec<Value>, Error> {
        match std::fs::read(&self.path) {
            Ok(bytes) => match json::parse(&bytes) {
                Ok(Value::Array(a)) => Ok(a),
                _ => Err(Error::Config(format!("{}: not a list of licence records", self.path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(Error::Config(format!("{}: {e}", self.path.display()))),
        }
    }

    /// The licences in force, read again when `licences.json` changed (another process may have
    /// accepted or revoked one: a long-running `mcp` or `serve` picks it up this way).
    pub(crate) fn in_force(&self) -> Result<BTreeMap<String, String>, Error> {
        let mtime = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, map)) = cache.as_ref() {
            if *at == mtime {
                return Ok(map.clone());
            }
        }
        let map = in_force_of(&self.records()?);
        *cache = Some((mtime, map.clone()));
        Ok(map)
    }

    /// The set the engine is given: the licences in force and the local derivation.
    pub(crate) fn engine_set(&self) -> Result<BTreeSet<String>, Error> {
        let mut set: BTreeSet<String> = self.in_force()?.into_keys().collect();
        set.insert(LOCAL_DERIVATION_LICENCE.to_string());
        Ok(set)
    }

    fn append(&self, record: Value) -> Result<(), Error> {
        let mut records = self.records()?;
        records.push(record);
        write_private(&self.path, &Value::Array(records).to_jcs())
            .map_err(|e| Error::Config(format!("{}: {e}", self.path.display())))?;
        *self.cache.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(())
    }

    /// Records an acceptance.
    pub(crate) fn accept(&self, licence: &str, at: &str, register_hash: Option<&str>) -> Result<(), Error> {
        let mut m = vec![
            ("licence".to_string(), Value::String(licence.to_string())),
            ("accepted_at".to_string(), Value::String(at.to_string())),
        ];
        if let Some(h) = register_hash {
            m.push(("register_licence_hash".to_string(), Value::String(h.to_string())));
        }
        self.append(Value::Object(m))
    }

    /// Records a revocation; `false` (and nothing recorded) when the licence is not in force.
    pub(crate) fn revoke(&self, licence: &str, at: &str) -> Result<bool, Error> {
        if !self.in_force()?.contains_key(licence) {
            return Ok(false);
        }
        self.append(Value::Object(vec![
            ("licence".into(), Value::String(licence.to_string())),
            ("revoked_at".into(), Value::String(at.to_string())),
        ]))?;
        Ok(true)
    }

    /// The licences in force, with their acceptance instants, in licence order.
    pub(crate) fn listing(&self) -> Result<Vec<AcceptedLicence>, Error> {
        Ok(self
            .in_force()?
            .into_iter()
            .map(|(licence, accepted_at)| AcceptedLicence { licence, accepted_at })
            .collect())
    }
}

/// The error of a result this machine may not publish or export (L5): its code and the licence.
pub(crate) fn not_publishable(licence: &str) -> Error {
    Error::Client {
        code: NOT_PUBLISHABLE.into(),
        detail: format!(
            "{licence}: a result this machine computed under this licence is never published or exported (datumesh-local 0.5.1, L5)"
        ),
    }
}

/// What an agent is told, and what the CLI names, for a restricted licence the user has not accepted:
/// the user accepts it at the command line (D4.3).
pub(crate) fn accept_hint(licence: &str) -> String {
    format!("the user must accept {licence} at the command line: datumesh licence accept '{licence}'")
}

/// Quotes a call's text value.
fn quoted(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Resolver {
    /// The licences in force, with their acceptance instants (`datumesh licence list`).
    pub fn licences_in_force(&self) -> Result<Vec<AcceptedLicence>, Error> {
        self.shared.acceptance.listing()
    }

    /// `datumesh licence accept <licence>` (D4.4): reads the register's datasets whose licence page
    /// is `licence` (the forward mesh's `register/dataset` class), then records the acceptance with
    /// the register's licence hash when there is one. A missing register entry is said and the
    /// acceptance still records. The caller prints the licence first and what is returned after.
    pub fn accept_licence(&self, licence: &str) -> Result<Accepted, Error> {
        let at = self.shared.now();
        let (datasets, register_note) = self.register_datasets(licence);
        let hash = datasets.iter().find_map(|d| d.licence_hash.clone());
        self.shared.acceptance.accept(licence, &at, hash.as_deref())?;
        Ok(Accepted { licence: licence.to_string(), accepted_at: at, datasets, register_note })
    }

    /// `datumesh licence revoke <licence>`: `false` when the licence was not in force.
    pub fn revoke_licence(&self, licence: &str) -> Result<bool, Error> {
        let at = self.shared.now();
        self.shared.acceptance.revoke(licence, &at)
    }

    /// The register's datasets whose licence page equals `licence`, with their hashes and clauses
    /// (each read as a cell of the forward mesh). The second member says why the register could
    /// not be read, when it could not.
    fn register_datasets(&self, licence: &str) -> (Vec<RegisteredDataset>, Option<String>) {
        let Some(mesh) = self.shared.config.forward_mesh.clone() else {
            return (Vec::new(), Some("no forward mesh is configured, so the register is not read".into()));
        };
        let class = format!("{mesh}/register/dataset");
        let fetched = self.shared.client.slice(&mesh, &format!("{class}/licence_page?code=*&source=*"), None);
        let members = match fetched.map(|f| json::parse(&f.body)) {
            Ok(Ok(v)) => match v.get("members") {
                Some(Value::Array(m)) => m.clone(),
                _ => Vec::new(),
            },
            Ok(Err(e)) => return (Vec::new(), Some(format!("the register's answer is not JSON: {e}"))),
            Err(e) => return (Vec::new(), Some(format!("the register could not be read: {}", e.detail))),
        };
        let mut out = Vec::new();
        for m in members {
            let Some(r) = m.get("receipt") else { continue };
            if r.get("value").and_then(Value::as_str) != Some(licence) {
                continue;
            }
            let arg = |k: &str| {
                r.get("cell")
                    .and_then(|c| c.get("args"))
                    .and_then(|a| a.get(k))
                    .and_then(|x| x.get("value"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            let (source, code) = (arg("source"), arg("code"));
            let call = |method: &str| format!("{class}/{method}(source: {}, code: {})", quoted(&source), quoted(&code));
            let licence_hash = self
                .get(&call("licence_hash"))
                .ok()
                .and_then(|reply| reply.receipt().map(|rc| rc.value().lexical().to_string()));
            let clauses =
                self.get(&call("licence_clauses")).ok().and_then(|reply| self.blob_of(&reply).ok().flatten()).and_then(
                    |b| match b.content {
                        crate::resolver::BlobContent::Text(t) => Some(t),
                        _ => None,
                    },
                );
            out.push(RegisteredDataset { source, code, licence_hash, clauses });
        }
        (out, None)
    }
}
