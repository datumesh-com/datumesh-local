//! The resolver: answers, definitions, prefetch and refresh, verification.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use datumesh_core::json::{self, Value};
use datumesh_core::{Arg, ErrorCode, FunctionDescriptor, Receipt, Request, Scalar};
use datumesh_http::client::{Client, ClientConfig, ClientError};
use datumesh_http::{Config as MeshConfig, Mesh};
use datumesh_store::{SqliteStore, Store};

use crate::alias::{request_asof, request_head, AliasMap};
use crate::classes::{with_resolved, Parse};
use crate::config::Config;
use crate::fetch::{self, host_of, name_of, network_failure, Gated, Shared, END};
use crate::gate::{is_unrooted, LicenceGate};
use crate::index::{Forwarded, Index};
use crate::key::{key_entries, local_key_document, retired_entries, write_private, MachineKey, RETIRED_KEYS};
use crate::trust;
use crate::{foreign_unavailable_envelope, Error, FOREIGN_UNAVAILABLE_STATUS, LOCAL_LABEL};

/// Where an answer came from.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The local store, without a fetch (a cell held, or evaluated here).
    Local,
    /// Fetched from its host now, imported, and answered from the store.
    Fetched,
    /// Forwarded by the licence gate to this mesh (now, or cached from before): its answer.
    Forwarded {
        /// The mesh.
        mesh: String,
    },
    /// Not held and could not be fetched: `E_FOREIGN_UNAVAILABLE`.
    Unavailable,
}

/// An answer: the HTTP status and the envelope (SPEC 5.5) exactly as the localhost endpoint
/// serves them, with where it came from and how to label its signer.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    /// The HTTP status.
    pub status: u16,
    /// The envelope (canonical JSON).
    pub body: Vec<u8>,
    /// Where it came from.
    pub source: Source,
    /// The signer's label when this machine's key (or a trusted machine key) signed the
    /// receipt: [`LOCAL_LABEL`].
    pub label: Option<String>,
    /// Why, for an answer that is not the cell's (`E_FOREIGN_UNAVAILABLE`'s failure, the
    /// gate's reason to forward).
    pub detail: Option<String>,
    /// The `Link: rel="canonical"` target of an answer about now (the as-of form of the cell,
    /// on `https://<host>/cell/...`, name-based): as the mesh gives it. `None` otherwise.
    pub canonical: Option<String>,
}

impl Reply {
    pub(crate) fn new(status: u16, body: Vec<u8>, source: Source) -> Reply {
        Reply { status, body, source, label: None, detail: None, canonical: None }
    }

    /// [`crate::FOREIGN_UNAVAILABLE`], core's envelope.
    pub(crate) fn unavailable(detail: String) -> Reply {
        Reply {
            status: FOREIGN_UNAVAILABLE_STATUS,
            body: foreign_unavailable_envelope(kind_of(&detail)),
            source: Source::Unavailable,
            label: None,
            detail: Some(detail),
            canonical: None,
        }
    }

    fn error(code: ErrorCode, detail: String) -> Reply {
        let r = if code == ErrorCode::NameUnbound {
            datumesh_http::envelope::name_unbound_reply(0)
        } else {
            datumesh_http::envelope::error_reply(code)
        };
        Reply {
            status: r.status,
            body: r.body.to_vec(),
            source: Source::Local,
            label: None,
            detail: Some(detail),
            canonical: None,
        }
    }

    /// The envelope, parsed.
    pub fn envelope(&self) -> Option<Value> {
        json::parse(&self.body).ok()
    }

    /// The envelope's `outcome`.
    pub fn outcome(&self) -> Option<String> {
        self.envelope().and_then(|v| v.get("outcome").and_then(Value::as_str).map(str::to_string))
    }

    /// The receipt, parsed, when the envelope has one.
    pub fn receipt(&self) -> Option<Receipt> {
        let v = self.envelope()?;
        Receipt::from_value(v.get("receipt")?).ok()
    }
}

/// A definition made: the name (full) and the function hash it is bound to.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Defined {
    /// The full name.
    pub name: String,
    /// The function hash.
    pub fn_hash: String,
    /// Whether the name was newly bound (false: already bound to this function).
    pub bound: bool,
}

/// What a prefetch pulled.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prefetched {
    /// The slice (its URI rest) or function prefetched.
    pub target: String,
    /// Pages fetched.
    pub pages: usize,
    /// Members (cells) seen.
    pub members: usize,
    /// Versions newly added to the store.
    pub versions: usize,
    /// Blobs fetched.
    pub blobs: usize,
    /// Function names held (bindings and descriptors).
    pub names: usize,
}

/// What a refresh pulled.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refreshed {
    /// Names whose binding was checked.
    pub names: usize,
    /// Slices fetched again.
    pub slices: usize,
    /// Single cells checked.
    pub cells: usize,
    /// Versions newly added.
    pub versions: usize,
    /// Forwarded answers fetched again.
    pub forwarded: usize,
    /// Failures (what, why); the rest was refreshed.
    pub failed: Vec<(String, String)>,
}

/// A name `find` found.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The full name.
    pub name: String,
    /// The function hash it is bound to now (here, or on the mesh asked).
    pub fn_hash: String,
    /// Held here (false: found on the mesh only).
    pub held: bool,
}

/// An imported receipt kept aside: signed by another machine's local key, not trusted.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quarantined {
    /// The receipt hash.
    pub hash: String,
    /// The signing key's id (the other machine).
    pub key_id: String,
    /// [`crate::UNVERIFIABLE_LABEL`].
    pub label: String,
    /// The receipt.
    pub bytes: Vec<u8>,
}

/// A key rotation made: the key it retired, the key it began, and the instant (CONTRACT-1.8b L4).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rotated {
    /// The key retired (its file is kept as `<key file>.retired-<id>`).
    pub old_key_id: String,
    /// The key now signing.
    pub new_key_id: String,
    /// When the new key began (its `valid_from`, and the old key's `valid_to`).
    pub at: String,
}

/// A cell line published: its call form, its URI and the hash of its receipt.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedCell {
    /// The call form (`<ns>/<fn>(k: v, ...)`, every argument labelled).
    pub call: String,
    /// The cell's URI (`cell://...`, no asof).
    pub uri: String,
    /// The hash of the receipt published for it.
    pub receipt_hash: String,
}

/// What a publish did: its counts (as [`datumesh_http::publish::Published`]) and each cell
/// line in file order.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedCells {
    /// The counts of the publish.
    pub counts: datumesh_http::publish::Published,
    /// The cell lines, in file order.
    pub cells: Vec<PublishedCell>,
}

/// The local resolver (see the crate documentation). Blocking; cheap to share behind an
/// `Arc`.
pub struct Resolver {
    pub(crate) shared: Arc<Shared>,
    aliases: RwLock<Arc<AliasMap>>,
    mesh: RwLock<Mesh>,
    base_url: RwLock<Option<String>>,
}

impl std::fmt::Debug for Resolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolver").field("key_id", &self.key_id()).finish_non_exhaustive()
    }
}

fn open_store(config: &Config) -> Result<Arc<dyn Store + Send + Sync>, Error> {
    let target =
        config.store.clone().unwrap_or_else(|| config.data_dir.join("store.sqlite").to_string_lossy().into_owned());
    if target.starts_with("postgres://") || target.starts_with("postgresql://") {
        let s = datumesh_store::PgStore::connect(&target).map_err(|e| Error::Store(e.to_string()))?;
        datumesh_store::migrate::apply(&s).map_err(|e| Error::Store(e.to_string()))?;
        return Ok(Arc::new(s));
    }
    let path = target.strip_prefix("sqlite://").unwrap_or(&target);
    let s = SqliteStore::open(path).map_err(|e| Error::Store(format!("{path}: {e}")))?;
    datumesh_store::migrate::sqlite::apply(&s).map_err(|e| Error::Store(e.to_string()))?;
    Ok(Arc::new(s))
}

pub(crate) fn rejection_detail(e: &ClientError) -> String {
    format!("{}: {}", e.code.as_str(), e.detail)
}

impl Resolver {
    /// Opens the resolver: the machine key (made at first run), the store (migrated), the
    /// index, the alias map, and this machine's key document in the store for `local.invalid`
    /// (this key and the trusted ones).
    pub fn open(config: Config) -> Result<Resolver, Error> {
        Self::open_with_clock(config, Arc::new(datumesh_engine::SystemClock))
    }

    /// As [`Resolver::open`], with `clock` for every "now" the resolver uses: signing, binding
    /// starts (a definition's, a publish line's left out), key rotation, trust and the MCP
    /// endpoint.
    pub fn open_with_clock(
        config: Config,
        clock: Arc<dyn datumesh_engine::Clock + Send + Sync>,
    ) -> Result<Resolver, Error> {
        crate::config::check_host(&config.host)?;
        let key = MachineKey::load_or_create_at(&config.key_file, clock.as_ref())?;
        let store = open_store(&config)?;
        let index = Index::open(&config.data_dir.join("index.sqlite"))?;
        let aliases = AliasMap::load(&config.aliases_file)?.for_host(&config.host);
        let mut cc = ClientConfig::default();
        cc.peers = config.peers.clone();
        cc.allowed = Some(config.allowlist.clone());
        cc.timeout = config.timeout;
        let client = Client::new(cc);
        let gate = LicenceGate::new();
        let offline = AtomicBool::new(config.offline);
        let shared = Arc::new(Shared {
            config,
            clock,
            machine: RwLock::new(Arc::new(key)),
            store,
            client,
            index,
            gate,
            offline,
        });
        publish_key_document(&shared)?;
        let r = Resolver {
            mesh: RwLock::new(build_mesh(&shared, &aliases.at(&shared.now()), None)?),
            shared,
            aliases: RwLock::new(Arc::new(aliases)),
            base_url: RwLock::new(None),
        };
        Ok(r)
    }

    /// The machine id: the key id of this machine's current key.
    pub fn key_id(&self) -> String {
        self.shared.key().key_id().to_string()
    }

    /// This machine's key document (its host's: this machine's keys, retired ones included) as
    /// verification uses it. Trusted machines' documents are not in it.
    pub fn key_document(&self) -> Vec<u8> {
        self.shared.store.namespace(&self.shared.config.host).ok().flatten().map(|n| n.keydoc).unwrap_or_default()
    }

    /// Rotates this machine's key (`datumesh key rotate`, CONTRACT-1.8b L4): a new key from now;
    /// the old key file is renamed `<key file>.retired-<old id>` (mode kept, never deleted) and
    /// its public entry, with `valid_to` now, is appended to `retired-keys.json`. Refused unless
    /// now is later than the current key's `valid_from`. Receipts from now on are signed by the
    /// new key; the old one verifies up to its `valid_to`.
    pub fn rotate_key(&self) -> Result<Rotated, Error> {
        let s = &self.shared;
        let now = s.now();
        let old = s.key();
        if now.as_str() <= old.valid_from() {
            return Err(Error::Key(format!(
                "rotation at {now} refused: the current key began at {} (a rotation must be later)",
                old.valid_from()
            )));
        }
        let dir = s.config.config_dir.clone();
        // The old public entry, retired at now: recorded before anything moves.
        let mut retired = retired_entries(&dir)?;
        let Value::Object(mut entry) = old.entry() else { unreachable!("a key entry is an object") };
        entry.push(("valid_to".into(), Value::String(now.clone())));
        retired.push(Value::Object(entry));
        write_private(&dir.join(RETIRED_KEYS), &Value::Array(retired).to_jcs())
            .map_err(|e| Error::Key(format!("{RETIRED_KEYS}: {e}")))?;
        // The old key file kept beside the new one.
        let mut kept = s.config.key_file.clone().into_os_string();
        kept.push(format!(".retired-{}", old.key_id()));
        std::fs::rename(&s.config.key_file, &kept)
            .map_err(|e| Error::Key(format!("{}: {e}", kept.to_string_lossy())))?;
        let new = MachineKey::load_or_create_at(&s.config.key_file, s.clock.as_ref())?;
        let new_id = new.key_id().to_string();
        *s.machine.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(new);
        publish_key_document(s)?;
        self.rebuild_mesh()?;
        Ok(Rotated { old_key_id: old.key_id().to_string(), new_key_id: new_id, at: now })
    }

    /// Builds the mesh again (its signing key, its aliases, its links): after a key change.
    fn rebuild_mesh(&self) -> Result<(), Error> {
        let base = self.base_url.read().unwrap_or_else(|p| p.into_inner()).clone();
        let mesh = build_mesh(&self.shared, &self.aliases_at(None), base)?;
        *self.mesh.write().unwrap_or_else(|p| p.into_inner()) = mesh;
        Ok(())
    }

    /// The configuration.
    pub fn config(&self) -> &Config {
        &self.shared.config
    }

    /// The store.
    pub fn store(&self) -> &Arc<dyn Store + Send + Sync> {
        &self.shared.store
    }

    /// The mesh (the handlers over the local store).
    pub fn mesh(&self) -> Mesh {
        self.mesh.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The alias map in use.
    pub fn aliases(&self) -> Arc<AliasMap> {
        self.aliases.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The alias map as it stands at `asof` (a request's `?asof=`; `None`: now).
    pub(crate) fn aliases_at(&self, asof: Option<&str>) -> AliasMap {
        let now = self.shared.now();
        self.aliases().at(asof.unwrap_or(&now))
    }

    /// Reads the alias map's file again (a refreshed snapshot).
    pub fn reload_aliases(&self) -> Result<(), Error> {
        let m = AliasMap::load(&self.shared.config.aliases_file)?.for_host(&self.shared.config.host);
        let base = self.base_url.read().unwrap_or_else(|p| p.into_inner()).clone();
        let mesh = build_mesh(&self.shared, &m.at(&self.shared.now()), base)?;
        *self.aliases.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(m);
        *self.mesh.write().unwrap_or_else(|p| p.into_inner()) = mesh;
        Ok(())
    }

    /// The base URL the endpoint serves on, when one is served (`serve`): the origin of the
    /// loopback links (CONTRACT-1.8b L7).
    pub(crate) fn loopback_origin(&self) -> Option<String> {
        self.base_url.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Serves the mesh's links under this base URL (`serve`: `http://127.0.0.1:<port>`).
    pub(crate) fn set_base_url(&self, base: Option<String>) -> Result<(), Error> {
        let mesh = build_mesh(&self.shared, &self.aliases_at(None), base.clone())?;
        *self.base_url.write().unwrap_or_else(|p| p.into_inner()) = base;
        *self.mesh.write().unwrap_or_else(|p| p.into_inner()) = mesh;
        Ok(())
    }

    /// Whether the resolver is offline.
    pub fn offline(&self) -> bool {
        self.shared.offline.load(Ordering::Acquire)
    }

    /// Goes offline (no fetch at all) or online.
    pub fn set_offline(&self, offline: bool) {
        self.shared.offline.store(offline, Ordering::Release);
    }

    fn label_of(&self, reply: &mut Reply) {
        let Some(r) = reply.receipt() else { return };
        reply.label = self.signer_label(&r);
    }

    /// The label of a receipt's signer here: this machine's host is [`LOCAL_LABEL`], a trusted
    /// machine's host is `trusted machine <host>` (CONTRACT-1.8b L4); none otherwise.
    pub(crate) fn signer_label(&self, r: &Receipt) -> Option<String> {
        let s = &self.shared;
        if r.signer_host() == s.config.host {
            return Some(LOCAL_LABEL.to_string());
        }
        // An unreadable trust file labels nothing: the receipt verifies or not on its own.
        match trust::trusted(&s.config.config_dir, r.signer_host()) {
            Ok(true) => Some(format!("trusted machine {}", r.signer_host())),
            _ => None,
        }
    }

    pub(crate) fn mesh_reply(&self, r: datumesh_http::envelope::Reply, source: Source) -> Reply {
        let mut reply = Reply::new(r.status, r.body.to_vec(), source);
        reply.canonical = r.canonical;
        self.label_of(&mut reply);
        reply
    }

    /// The answer to `request`: a call (`name(arg: value, ...)`, `name@<hash>(...)`, with
    /// `?asof=`), a cell URI, or the rest of one; short names expanded by the alias map. See
    /// the crate documentation for where answers come from.
    pub fn get(&self, request: &str) -> Result<Reply, Error> {
        let aliases = self.aliases_at(request_asof(request).as_deref());
        let text = aliases.expand_prefix(request);
        let parsed = crate::classes::note_prefix(self.parse_request(&text, &aliases), request, &text, &aliases);
        match parsed {
            Parse::Call { rest, resolved, fetched } => {
                let reply = self.answer(&rest, &Cell::new(fetched))?;
                Ok(match resolved {
                    Some(r) => with_resolved(reply, &r),
                    None => reply,
                })
            }
            Parse::Answered(reply) => Ok(reply),
            // A call the parse refuses: the mesh's own answer to it, which needs no network.
            Parse::Refused(why) => Err(Error::Request(why)),
            Parse::Malformed => Ok(self.mesh_reply(self.mesh().cell(&text, None), Source::Local)),
            Parse::Plain => self.answer(&aliases.expand_request(request), &Cell::new(false)),
        }
    }

    /// The answer to a cell URI's rest (or a URI, `cell://`), fetching what it needs from its host
    /// when it is not held (`fetched` notes that a fetch happened).
    pub(crate) fn answer(&self, text: &str, fetched: &Cell<bool>) -> Result<Reply, Error> {
        let s = &self.shared;
        let rest = text.strip_prefix("cell://").unwrap_or(text).to_string();
        let name = name_of(&rest).to_string();
        let host = host_of(&name).to_string();
        let mesh = self.mesh();
        let foreign = !is_unrooted(&host) && datumesh_formula::names::is_function_name(&name);
        if foreign {
            let pin = pin_of(&rest);
            let missing = match &pin {
                Some(p) => s.store.descriptor(p).is_none(),
                None => s.store.resolve_binding(&name, END).map_err(|e| Error::Store(e.to_string()))?.is_none(),
            };
            if missing {
                if let Err(why) = s.may_fetch(&host) {
                    return Ok(Reply::unavailable(why));
                }
                let r = match &pin {
                    Some(p) => s.import_descriptor(&host, p).map(|_| true),
                    None => s.import_name(&name),
                };
                match r {
                    Ok(_) => fetched.set(true),
                    Err(e) => {
                        if let Some(why) = network_failure(&e) {
                            return Ok(Reply::unavailable(why));
                        }
                        if e.code != ErrorCode::NameUnbound {
                            return Ok(Reply::error(e.code, rejection_detail(&e)));
                        }
                    }
                }
            }
        }
        let target = match mesh.resolve(&rest, false) {
            Ok(t) => t,
            Err(_) => return Ok(self.mesh_reply(mesh.cell(&rest, None), Source::Local)),
        };
        if let Some(f) = s.index.forwarded(target.rest())? {
            let mut reply = Reply::new(f.status, f.body, Source::Forwarded { mesh: f.mesh });
            reply.detail = Some("forwarded by the licence gate (cached)".into());
            return Ok(reply);
        }
        let stored = target.bound.descriptor.lang == "stored";
        if foreign && stored {
            let id = target.cell_id().to_string();
            if s.store.latest(&id).map_err(|e| Error::Store(e.to_string()))?.is_none() {
                if let Err(why) = s.may_fetch(&host) {
                    return Ok(Reply::unavailable(why));
                }
                let mut reference = target.pinned_reference();
                reference.asof = None;
                let f = match s.fetch_cell(&host, &reference) {
                    Ok(f) => f,
                    Err(e) => {
                        return Ok(match network_failure(&e) {
                            Some(why) => Reply::unavailable(why),
                            None => Reply::error(e.code, rejection_detail(&e)),
                        })
                    }
                };
                match f.answer.receipt.as_ref() {
                    Some(r) => {
                        let (imported, _) = fetch::with_ctx(|| {
                            s.import_receipt(r, true, None)?;
                            s.import_blob(&host, r)
                        });
                        if let Err(e) = imported {
                            return Ok(match network_failure(&e) {
                                Some(why) => Reply::unavailable(why),
                                None => Reply::error(e.code, rejection_detail(&e)),
                            });
                        }
                        mesh.stored_version_added(&id);
                        fetched.set(true);
                    }
                    // The host's answer without a receipt (not_covered, unlicensed, ...).
                    None => return Ok(Reply::new(f.status, f.body, Source::Fetched)),
                }
            }
            let source = if fetched.get() { Source::Fetched } else { Source::Local };
            return Ok(self.mesh_reply(mesh.cell(&rest, None), source));
        }
        // Evaluated here (or a local stored cell): foreign inputs fetched, inputs gated.
        let (reply, ctx) = fetch::with_ctx(|| mesh.cell(&rest, None));
        for c in &ctx.added {
            mesh.stored_version_added(c);
        }
        if let Some(why) = ctx.unlicensed {
            let mut r = Reply::new(403, datumesh_http::envelope::unlicensed(), Source::Local);
            r.detail = Some(why);
            return Ok(r);
        }
        if let Some(why) = ctx.refused {
            // A local name (a formula here) reading a declared restricted computation: no
            // declaration of its own, so it is refused here, not forwarded (CONTRACT-1.8b L3).
            if is_unrooted(host_of(&target.cell.ns)) {
                let mut r = Reply::new(403, datumesh_http::envelope::unlicensed(), Source::Local);
                r.detail = Some(why);
                return Ok(r);
            }
            return self.forward(&target, why);
        }
        if let Some(why) = ctx.unavailable {
            return Ok(Reply::unavailable(why));
        }
        if let (Some(e), Some(o)) = (ctx.remote, reply.outcome()) {
            if o == "error" {
                return Ok(Reply::error(e.code, rejection_detail(&e)));
            }
        }
        let source = if fetched.get() || !ctx.added.is_empty() { Source::Fetched } else { Source::Local };
        let reply = self.mesh_reply(reply, source);
        if let Some(r) = reply.receipt() {
            if r.signer_host() == s.config.host && !r.inputs().is_empty() {
                s.index.answered(r.hash(), &r.cell().id(), &s.now())?;
            }
        }
        Ok(reply)
    }

    /// Forwards a request the gate refused to the configured mesh; its answer is kept.
    fn forward(&self, target: &datumesh_http::call::Target, why: String) -> Result<Reply, Error> {
        let s = &self.shared;
        let Some(mesh) = s.config.forward_mesh.clone() else {
            let mut r = Reply::new(403, datumesh_http::envelope::unlicensed(), Source::Local);
            r.detail = Some(format!("{why}; no forward mesh is configured"));
            return Ok(r);
        };
        if let Err(w) = s.may_fetch(&mesh) {
            return Ok(Reply::unavailable(w));
        }
        // The publisher's key document (its declarations) brought up to date, as for a
        // prefetch: what the forwarded answer is checked and kept against.
        if let Err(e) = s.ensure_publisher(host_of(&target.cell.ns)) {
            if let Some(w) = network_failure(&e) {
                return Ok(Reply::unavailable(w));
            }
        }
        let mut req = Request::new(target.cell_id(), target.asof()).map_err(|e| Error::Request(format!("{e:?}")))?;
        if let Some(p) = &target.pin {
            req = req.with_pin(p).map_err(|e| Error::Request(format!("{e:?}")))?;
        }
        let f = match s.patient(|| s.client.cell(&mesh, target.rest(), &req)) {
            Ok(f) => f,
            Err(e) => {
                return Ok(match network_failure(&e) {
                    Some(w) => Reply::unavailable(w),
                    None => Reply::error(e.code, rejection_detail(&e)),
                })
            }
        };
        if f.status == 200 {
            s.index.put_forwarded(
                &Forwarded {
                    rest: target.rest().to_string(),
                    mesh: mesh.clone(),
                    status: f.status,
                    body: f.body.clone(),
                    cell_id: target.cell_id().to_string(),
                },
                &s.now(),
            )?;
            // Kept in the store too (verified again there), for export and verification.
            if let Some(r) = f.answer.receipt.as_ref() {
                let _ = s.import_receipt(r, false, None);
            }
        }
        let mut reply = Reply::new(f.status, f.body, Source::Forwarded { mesh });
        reply.detail = Some(why);
        Ok(reply)
    }

    /// Defines a formula function `name` (a full or short name) with `source` and
    /// `signature` (a JSON signature with `order`), bound from now. Its callees are as bound
    /// now (foreign ones fetched when not held). A function under a rooted namespace whose
    /// inputs name an unrooted one is refused ([`Error::UnrootedInput`]).
    ///
    /// A formula with no inputs is refused ([`Error::NoInputs`]).
    pub fn define_formula(&self, name: &str, source: &str, signature: &Value) -> Result<Defined, Error> {
        self.define_formula_at(name, source, signature, None)
    }

    /// [`Resolver::define_formula`], bound from `start` (a publish line's `bound`) rather than now.
    fn define_formula_at(
        &self,
        name: &str,
        source: &str,
        signature: &Value,
        start: Option<&str>,
    ) -> Result<Defined, Error> {
        let s = &self.shared;
        let aliases = self.aliases_at(None);
        let name = aliases.expand(name);
        // A prefix alias in the source (`@ecb/fx_rate`) is expanded first: the hash is that of the full name.
        let source = aliases.expand_source(source);
        let now = s.now();
        let start = start.unwrap_or(&now).to_string();
        let callees = Callees::new(s);
        let def = match datumesh_formula::definition::define_function(
            &name,
            &source,
            signature,
            &aliases.bare(),
            &callees,
            &now,
        ) {
            Ok(d) => d,
            Err(e) => {
                // A callee under an unrooted host that is neither this machine's nor trusted here.
                if let Some(input) = callees.untrusted.take() {
                    return Err(Error::UntrustedInput { function: name, input });
                }
                return Err(Error::Define(e.to_string()));
            }
        };
        if def.descriptor.inputs.is_empty() {
            return Err(Error::NoInputs { function: name });
        }
        check_rooting(&name, &def.descriptor)?;
        self.bind(&name, &def.bytes, &def.fn_hash, &start)
    }

    /// Defines a stored function `name` with `signature`, bound from now.
    pub fn define_stored(&self, name: &str, signature: &Value) -> Result<Defined, Error> {
        let name = self.aliases_at(None).expand(name);
        let d = FunctionDescriptor::from_value(&fetch::stored_descriptor(signature))
            .map_err(|e| Error::Define(e.to_string()))?;
        let now = self.shared.now();
        self.bind(&name, &d.canonical(), &d.hash(), &now)
    }

    /// Defines a function from a descriptor (JSON) as `name`, bound from now: the rooting
    /// rule checked on its `inputs`.
    pub fn define(&self, name: &str, descriptor: &[u8]) -> Result<Defined, Error> {
        let name = self.aliases_at(None).expand(name);
        let d = FunctionDescriptor::from_json(descriptor).map_err(|e| Error::Define(e.to_string()))?;
        if d.lang != "stored" && d.inputs.is_empty() {
            return Err(Error::NoInputs { function: name });
        }
        check_rooting(&name, &d)?;
        let now = self.shared.now();
        self.bind(&name, &d.canonical(), &d.hash(), &now)
    }

    fn bind(&self, name: &str, bytes: &[u8], fn_hash: &str, now: &str) -> Result<Defined, Error> {
        let s = &self.shared;
        if !datumesh_formula::names::is_function_name(name) {
            return Err(Error::Define(format!("{name} is not a full function name")));
        }
        let h = s.store.put_descriptor(bytes).map_err(|e| Error::Store(e.to_string()))?;
        debug_assert_eq!(h, fn_hash);
        let bound = match s.store.resolve_binding(name, END).map_err(|e| Error::Store(e.to_string()))? {
            Some(b) if b.fn_hash == h => false,
            _ => {
                s.store.bind_name(name, &h, now).map_err(|e| Error::Store(e.to_string()))?;
                true
            }
        };
        s.index.hold_name(name, host_of(name), now)?;
        self.mesh().clear_memo();
        Ok(Defined { name: name.to_string(), fn_hash: h, bound })
    }

    /// Publishes local functions and stored cells (`define` plus `put_version`), under
    /// `local/` only: a JSON lines file in the format of `datumesh-mesh publish-stored`
    /// (function, formula and cell lines; names may be short). Cells are signed with this
    /// machine's key.
    pub fn publish(&self, path: &Path) -> Result<datumesh_http::publish::Published, Error> {
        self.publish_cells(path).map(|p| p.counts)
    }

    /// As [`Resolver::publish`], and each cell line published with its call form, URI and
    /// receipt hash, in file order ([`PublishedCells`]).
    pub fn publish_cells(&self, path: &Path) -> Result<PublishedCells, Error> {
        let s = &self.shared;
        let host = s.config.host.clone();
        let text = std::fs::read_to_string(path).map_err(|e| Error::Request(format!("{}: {e}", path.display())))?;
        let aliases = self.aliases_at(None);
        let mut kept = String::new();
        let mut formulas = Vec::new();
        let mut names = Vec::new();
        let mut cells = Vec::new();
        let now = s.now();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let v = json::parse(line.as_bytes()).map_err(|e| Error::Request(format!("line {}: {e}", i + 1)))?;
            let Value::Object(mut m) = v else {
                return Err(Error::Request(format!("line {}: not an object", i + 1)));
            };
            let mut formula = None;
            for (k, x) in m.iter_mut() {
                let full = match (k.as_str(), &*x) {
                    ("function" | "formula" | "class", Value::String(n)) => Some(aliases.expand(n)),
                    _ => None,
                };
                if let Some(full) = full {
                    if host_of(&full) != host {
                        return Err(Error::Request(format!("line {}: {full}: publish is under local/ only", i + 1)));
                    }
                    if k == "formula" {
                        formula = Some(full.clone());
                    } else if k == "function" {
                        names.push(full.clone());
                    }
                    *x = Value::String(full);
                }
                if k == "cell" {
                    if let Value::Object(cm) = x {
                        for (ck, cx) in cm.iter_mut() {
                            if ck == "fn" {
                                let full = aliases.expand(cx.as_str().unwrap_or(""));
                                if host_of(&full) != host {
                                    return Err(Error::Request(format!(
                                        "line {}: {full}: publish is under local/ only",
                                        i + 1
                                    )));
                                }
                                *cx = Value::String(full);
                            }
                        }
                    }
                }
            }
            let v = Value::Object(m);
            if let Some(name) = formula {
                // Rule A (CONTRACT-1.8b H1, L): a start left out is now, and an earlier one is refused
                // before anything is written.
                let start = v.get("bound").and_then(Value::as_str).map_or_else(|| now.clone(), str::to_string);
                if start < now {
                    return Err(Error::StartBeforeNow { name, start, now: now.clone() });
                }
                formulas.push((name, v, start));
                continue;
            }
            if v.get("cell").is_some() {
                cells.push(v.clone());
            }
            kept.push_str(&String::from_utf8_lossy(&v.to_jcs()));
            kept.push('\n');
        }
        let tmp = s.config.data_dir.join(format!("publish-{}.jsonl", std::process::id()));
        std::fs::write(&tmp, kept).map_err(|e| Error::Request(e.to_string()))?;
        // Blob files are relative to the published file: copy nothing, point there.
        let out = if path.parent().is_some_and(|p| p != s.config.data_dir) {
            let near = path.with_file_name(format!(".datumesh-publish-{}.jsonl", std::process::id()));
            std::fs::rename(&tmp, &near).map_err(|e| Error::Request(e.to_string()))?;
            let r = publish_local(s, &near);
            let _ = std::fs::remove_file(&near);
            r
        } else {
            let r = publish_local(s, &tmp);
            let _ = std::fs::remove_file(&tmp);
            r
        };
        let mut published = out?;
        for n in &names {
            s.index.hold_name(n, &host, &s.now())?;
        }
        // Formulas after the stored functions they may read.
        for (name, v, start) in &formulas {
            let src = v.get("source").and_then(Value::as_str).unwrap_or("");
            let sig = v.get("signature").cloned().unwrap_or(Value::Null);
            if self.define_formula_at(name, src, &sig, Some(start))?.bound {
                published.functions += 1;
            }
        }
        let mesh = self.mesh();
        let mut published_cells = Vec::new();
        for c in cells {
            if let Some(pc) = self.published_cell(&c) {
                published_cells.push(pc);
            }
            if let Some(id) = self.cell_id_of_line(&c) {
                let rest =
                    c.get("cell").and_then(|x| x.get("fn")).and_then(Value::as_str).and_then(|n| {
                        self.shared.store.resolve_name(n, END).ok().flatten().map(|h| (n.to_string(), h))
                    });
                if let Some((n, h)) = rest {
                    let cell = datumesh_core::Cell {
                        ns: n.rsplit_once('/').map(|x| x.0).unwrap_or("").to_string(),
                        fn_name: n.rsplit_once('/').map(|x| x.1).unwrap_or("").to_string(),
                        fn_hash: h,
                        args: args_of(&c),
                    };
                    let held = crate::index::HeldCell {
                        cell_id: id.clone(),
                        host: host.clone(),
                        rest: fetch::cell_rest(&cell),
                        slice: None,
                    };
                    s.index.hold_cell(&held, &s.now())?;
                }
                mesh.stored_version_added(&id);
            }
        }
        mesh.clear_memo();
        Ok(PublishedCells { counts: published, cells: published_cells })
    }

    /// The call form, URI and receipt hash of a published cell line (its version is stored).
    fn published_cell(&self, line: &Value) -> Option<PublishedCell> {
        let name = line.get("cell")?.get("fn")?.as_str()?;
        let tx = line.get("tx")?.as_str()?;
        let fn_hash = self.shared.store.resolve_name(name, tx).ok().flatten()?;
        let (ns, fn_name) = name.rsplit_once('/')?;
        let cell =
            datumesh_core::Cell { ns: ns.to_string(), fn_name: fn_name.to_string(), fn_hash, args: args_of(line) };
        let receipt_hash = self.shared.store.version_asof(&cell.id(), tx).ok().flatten()?.receipt_hash;
        Some(PublishedCell { call: call_form(&cell), uri: cell_uri(&cell), receipt_hash })
    }

    fn cell_id_of_line(&self, line: &Value) -> Option<String> {
        let c = line.get("cell")?;
        let name = c.get("fn")?.as_str()?;
        let tx = line.get("tx")?.as_str()?;
        let h = self.shared.store.resolve_name(name, tx).ok().flatten()?;
        let (ns, f) = name.rsplit_once('/')?;
        Some(datumesh_core::Cell { ns: ns.to_string(), fn_name: f.to_string(), fn_hash: h, args: args_of(line) }.id())
    }

    /// The lineage of a cell as nested envelopes (`/explain`, depth-bounded), from the store.
    pub fn explain(&self, request: &str, depth: Option<usize>) -> Result<Reply, Error> {
        let aliases = self.aliases_at(request_asof(request).as_deref());
        let rest = match self.parse_request(&aliases.expand_prefix(request), &aliases) {
            Parse::Answered(reply) => return Ok(reply),
            Parse::Refused(why) => return Err(Error::Request(why)),
            Parse::Call { rest, .. } => rest,
            Parse::Malformed | Parse::Plain => {
                let text = aliases.expand_request(request);
                text.strip_prefix("cell://").unwrap_or(&text).to_string()
            }
        };
        let mut path = format!("/explain/{}", crate::serve::encode_path(&rest));
        if let Some(d) = depth {
            path.push(if rest.contains('?') { '&' } else { '?' });
            path.push_str(&format!("depth={d}"));
        }
        let (status, body) = crate::serve::oneshot(&self.mesh(), &path)?;
        let mut reply = Reply::new(status, body, Source::Local);
        self.label_of(&mut reply);
        Ok(reply)
    }

    /// Names held here whose name contains `query` (case-insensitive); with `remote`, also
    /// those the forward mesh's `/find` gives (an explicit fetch).
    pub fn find(&self, query: &str, remote: bool) -> Result<Vec<Found>, Error> {
        let s = &self.shared;
        let q = query.to_lowercase();
        let mut out = Vec::new();
        for (name, _) in s.index.names()? {
            if name.to_lowercase().contains(&q) {
                if let Some(b) = s.store.resolve_binding(&name, END).map_err(|e| Error::Store(e.to_string()))? {
                    out.push(Found { name, fn_hash: b.fn_hash, held: true });
                } else if let Some(c) =
                    s.store.resolve_class_binding(&name, END).map_err(|e| Error::Store(e.to_string()))?
                {
                    out.push(Found { name, fn_hash: c.class_hash, held: true });
                }
            }
        }
        if remote {
            let mesh = s.config.forward_mesh.clone().ok_or_else(|| Error::Request("no forward mesh".into()))?;
            s.may_fetch(&mesh).map_err(Error::ForeignUnavailable)?;
            let doc = s.client.resolve(&mesh).map_err(|e| client_error(&e))?;
            let url = doc.url("find", &[]).unwrap_or_else(|| format!("https://{mesh}/find")).to_string();
            let url = format!("{url}?q={}", datumesh_http::client::pct(query));
            let (status, body) = s.client.get(&url, 16 << 20).map_err(|e| Error::ForeignUnavailable(e.to_string()))?;
            if status != 200 {
                return Err(Error::Client { code: "E_INPUT_UNAVAILABLE".into(), detail: format!("{url}: {status}") });
            }
            let v = json::parse(&body)
                .map_err(|e| Error::Client { code: "E_INPUT_UNAVAILABLE".into(), detail: e.to_string() })?;
            if let Some(Value::Array(rs)) = v.get("results") {
                for r in rs {
                    let name = r.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                    let h = r.get("fn_hash").and_then(Value::as_str).unwrap_or("").to_string();
                    if !out.iter().any(|f| f.name == name) {
                        out.push(Found { name, fn_hash: h, held: false });
                    }
                }
            }
        }
        Ok(out)
    }

    /// Verifies a receipt (SPEC 10.1) with this machine's keys: the store's key documents
    /// (this machine's own for `local.invalid`), at level `recomputed` with the engine as the
    /// resolver. The result as JSON (`ok`, `level`, `code`, `hash`, ...).
    pub fn verify(&self, receipt: &[u8]) -> Value {
        let v = self.mesh().verify(receipt, false, None);
        json::parse(&v.body).unwrap_or(Value::Null)
    }

    /// The receipts kept aside at import: signed by another machine's local key that is not
    /// trusted here.
    pub fn quarantined(&self) -> Result<Vec<Quarantined>, Error> {
        Ok(self
            .shared
            .index
            .quarantined()?
            .into_iter()
            .map(|(hash, key_id, bytes)| Quarantined { hash, key_id, label: crate::UNVERIFIABLE_LABEL.into(), bytes })
            .collect())
    }

    /// Prefetches `target`: a slice (a call or URI with `*` for open arguments) of a foreign
    /// stored function, its pages (each member's receipt checked and imported), descriptors
    /// and keys; or a function name (its binding, descriptor and key document); or one cell.
    /// `page` is the page size asked for (the mesh's default when `None`); `blobs` fetches
    /// the content of blob values too (one request each).
    pub fn prefetch(&self, target: &str, page: Option<usize>, blobs: bool) -> Result<Prefetched, Error> {
        let s = &self.shared;
        let asof = request_asof(target);
        let text = self.aliases_at(asof.as_deref()).expand_request(target);
        self.check_bare_class(request_head(target), request_head(&text), asof.as_deref())?;
        let rest = text.strip_prefix("cell://").unwrap_or(&text).to_string();
        let name = name_of(&rest).to_string();
        let host = host_of(&name).to_string();
        let mut out = Prefetched { target: rest.clone(), ..Default::default() };
        if is_unrooted(&host) {
            return Err(Error::Request(format!("{name} is local: nothing to prefetch")));
        }
        s.may_fetch(&host).map_err(Error::ForeignUnavailable)?;
        if s.import_name(&name).map_err(|e| client_error(&e))? {
            out.names = 1;
        } else {
            return Err(Error::Client { code: "E_NAME_UNBOUND".into(), detail: name });
        }
        if rest == name {
            return Ok(out);
        }
        if !rest.contains('*') {
            let r = self.get(&rest)?;
            out.members = usize::from(r.receipt().is_some());
            return Ok(out);
        }
        let mesh = self.mesh();
        let t = mesh.resolve(&rest, true).map_err(|e| Error::Request(format!("{rest}: {e:?}")))?;
        let slice_rest = t.rest().to_string();
        self.pull_slice(&host, &slice_rest, &t, page, blobs, &mut out)?;
        s.index.hold_slice(&slice_rest, &host, out.members, &s.now())?;
        out.target = slice_rest;
        Ok(out)
    }

    fn pull_slice(
        &self,
        host: &str,
        slice_rest: &str,
        t: &datumesh_http::call::Target,
        page: Option<usize>,
        blobs: bool,
        out: &mut Prefetched,
    ) -> Result<(), Error> {
        let s = &self.shared;
        let mesh = self.mesh();
        let open: Vec<String> =
            t.reference.args.iter().filter(|(_, a)| matches!(a, Arg::Slice)).map(|(k, _)| k.clone()).collect();
        let doc = s.patient(|| s.client.resolve(host)).map_err(|e| client_error(&e))?;
        let first = doc.url("slice", &[("cell", slice_rest)]).ok_or_else(|| Error::Client {
            code: "E_MESH_DOC".into(),
            detail: format!("{host} has no slice endpoint"),
        })?;
        let mut url = match page {
            Some(n) => format!("{first}{}limit={n}", if first.contains('?') { '&' } else { '?' }),
            None => first,
        };
        let mut after: Option<BTreeMap<String, Scalar>> = None;
        let mut seen = 0usize;
        loop {
            let f = s.patient(|| {
                let f = s.client.slice_page(host, slice_rest, Some(&url), after.clone())?;
                if f.status == 429 {
                    return Err(ClientError { code: ErrorCode::InputUnavailable, detail: "status 429".into() });
                }
                Ok(f)
            });
            let f = f.map_err(|e| match network_failure(&e) {
                Some(w) => Error::ForeignUnavailable(w),
                None => client_error(&e),
            })?;
            out.pages += 1;
            let Some(p) = f.answer.page.as_ref() else {
                return Err(Error::Client {
                    code: f
                        .answer
                        .error
                        .map(|c| c.as_str().to_string())
                        .unwrap_or_else(|| "E_INPUT_UNAVAILABLE".into()),
                    detail: format!("{slice_rest}: {}", f.answer.outcome.as_str()),
                });
            };
            let (r, _) = fetch::with_ctx(|| -> Result<(), ClientError> {
                for m in &p.members {
                    let Some(r) = m.receipt.as_ref() else { continue };
                    seen += 1;
                    if s.import_receipt(r, true, Some(slice_rest))? {
                        out.versions += 1;
                        mesh.stored_version_added(&r.cell().id());
                    }
                    if blobs && s.import_blob(host, r)? {
                        out.blobs += 1;
                    }
                }
                Ok(())
            });
            r.map_err(|e| client_error(&e))?;
            match (&p.next, p.members.last().and_then(|m| m.receipt.as_ref())) {
                (Some(next), Some(last)) => {
                    let key: BTreeMap<String, Scalar> = open
                        .iter()
                        .filter_map(|k| match last.cell().args.get(k) {
                            Some(Arg::Value(v)) => Some((k.clone(), v.clone())),
                            _ => None,
                        })
                        .collect();
                    after = Some(key);
                    url = next.clone();
                }
                _ => break,
            }
        }
        out.members = seen;
        Ok(())
    }

    /// Re-pulls the latest versions of what is held: names' bindings, prefetched slices
    /// (their pages again), single cells (through `/versions` where the host lists it, else
    /// their envelope now), and forwarded answers; then reloads the alias map.
    pub fn refresh(&self) -> Result<Refreshed, Error> {
        let s = &self.shared;
        let mut out = Refreshed::default();
        if self.offline() {
            return Err(Error::ForeignUnavailable("offline: the resolver is offline".into()));
        }
        let mesh = self.mesh();
        for (name, host) in s.index.names()? {
            if is_unrooted(&host) || s.may_fetch(&host).is_err() {
                continue;
            }
            out.names += 1;
            if let Err(e) = s.import_name(&name) {
                out.failed.push((name, rejection_detail(&e)));
            }
        }
        let slices = s.index.slices()?;
        for (rest, host) in &slices {
            let t = match mesh.resolve(rest, true) {
                Ok(t) => t,
                Err(e) => {
                    out.failed.push((rest.clone(), format!("{e:?}")));
                    continue;
                }
            };
            let mut p = Prefetched::default();
            match self.pull_slice(host, rest, &t, None, false, &mut p) {
                Ok(()) => {
                    out.slices += 1;
                    out.versions += p.versions;
                    s.index.hold_slice(rest, host, p.members, &s.now())?;
                }
                Err(e) => out.failed.push((rest.clone(), e.to_string())),
            }
        }
        for c in s.index.cells()? {
            if c.slice.is_some() || is_unrooted(&c.host) || s.may_fetch(&c.host).is_err() {
                continue;
            }
            out.cells += 1;
            match self.refresh_cell(&c) {
                Ok(n) => out.versions += n,
                Err(e) => out.failed.push((c.rest.clone(), e.to_string())),
            }
        }
        for f in s.index.all_forwarded()? {
            let Ok(t) = mesh.resolve(&f.rest, false) else { continue };
            let why = "refreshed".to_string();
            match self.forward(&t, why) {
                Ok(r) if r.status == 200 => out.forwarded += 1,
                Ok(r) => out.failed.push((f.rest.clone(), String::from_utf8_lossy(&r.body).into_owned())),
                Err(e) => out.failed.push((f.rest.clone(), e.to_string())),
            }
        }
        mesh.clear_memo();
        self.reload_aliases()?;
        Ok(out)
    }

    fn refresh_cell(&self, c: &crate::index::HeldCell) -> Result<usize, Error> {
        let s = &self.shared;
        let mesh = self.mesh();
        let doc = s.patient(|| s.client.resolve(&c.host)).map_err(|e| client_error(&e))?;
        let local = s.store.latest(&c.cell_id).map_err(|e| Error::Store(e.to_string()))?;
        let mut added = 0;
        if doc.url("versions", &[("cell", &c.rest)]).is_some() {
            let mut page = s.patient(|| s.client.versions(&c.host, &c.rest)).map_err(|e| client_error(&e))?;
            loop {
                for v in &page.versions {
                    if local.as_ref().is_some_and(|l| l.tx.as_str() >= v.tx.as_str()) {
                        continue;
                    }
                    let bytes = s.patient(|| s.client.receipt(&c.host, &v.receipt)).map_err(|e| client_error(&e))?;
                    let r = Receipt::from_json(&bytes)
                        .map_err(|e| Error::Client { code: e.code().as_str().into(), detail: e.to_string() })?;
                    if s.import_receipt(&r, true, c.slice.as_deref()).map_err(|e| client_error(&e))? {
                        added += 1;
                    }
                }
                match page.next.clone() {
                    Some(n) => page = s.patient(|| s.client.versions_at(&n)).map_err(|e| client_error(&e))?,
                    None => break,
                }
            }
        } else {
            let t = mesh.resolve(&c.rest, false).map_err(|e| Error::Request(format!("{}: {e:?}", c.rest)))?;
            let mut reference = t.pinned_reference();
            reference.asof = None;
            let f = s.fetch_cell(&c.host, &reference).map_err(|e| client_error(&e))?;
            if let Some(r) = f.answer.receipt.as_ref() {
                if s.import_receipt(r, true, c.slice.as_deref()).map_err(|e| client_error(&e))? {
                    added += 1;
                }
            }
        }
        if added > 0 {
            mesh.stored_version_added(&c.cell_id);
        }
        Ok(added)
    }
}

/// This machine's key document in the store: the current key, the retired keys (each with its
/// `valid_to`), and the store's earlier ones (a reinstall's: their receipts verify). Trusted
/// machines' keys are not in it (CONTRACT-1.8b L4).
fn publish_key_document(s: &Shared) -> Result<(), Error> {
    let host = s.config.host.as_str();
    let mut entries = vec![s.key().entry()];
    entries.extend(retired_entries(&s.config.config_dir)?);
    let stored = s.store.namespace(host).map_err(|e| Error::Store(e.to_string()))?;
    if let Some(n) = &stored {
        entries.extend(key_entries(&n.keydoc));
    }
    let doc = local_key_document(host, &entries)?;
    if stored.map(|n| n.keydoc) != Some(doc.clone()) {
        s.store.put_namespace(host, &doc, &s.now()).map_err(|e| Error::Store(e.to_string()))?;
    }
    Ok(())
}

/// The envelope's `detail` of a failure (`<kind>: <why>`): a host not allowed is `offline`
/// (the resolver is configured not to use the network for it).
fn kind_of(detail: &str) -> datumesh_core::ForeignDetail {
    let k = detail.split(':').next().unwrap_or("").trim();
    match k {
        "not allowed" => datumesh_core::ForeignDetail::Offline,
        k => datumesh_core::ForeignDetail::from_name(k).unwrap_or(datumesh_core::ForeignDetail::Network),
    }
}

pub(crate) fn client_error(e: &ClientError) -> Error {
    match network_failure(e) {
        Some(w) => Error::ForeignUnavailable(w),
        None => Error::Client { code: e.code.as_str().to_string(), detail: e.detail.clone() },
    }
}

fn args_of(line: &Value) -> datumesh_core::Args {
    let mut args = datumesh_core::Args::new();
    if let Some(m) = line.get("cell").and_then(|c| c.get("args")).and_then(Value::as_object) {
        for (k, a) in m {
            if let Ok(s) = Scalar::from_json(&a.to_jcs()) {
                args.insert(k.clone(), Arg::Value(s));
            }
        }
    }
    args
}

/// Publishes the lines of `path` under `local/` as this machine (its clock, rule A; the
/// library's errors: a start earlier than now is [`Error::StartBeforeNow`]).
fn publish_local(s: &Shared, path: &Path) -> Result<datumesh_http::publish::Published, Error> {
    let opts = datumesh_http::publish::PublishOptions { clock: s.clock.clone(), backfill: None };
    let key = s.key();
    let host = s.config.host.as_str();
    datumesh_http::publish::publish_stored_with(&*s.store, key.signing_key(), host, &[host.to_string()], path, &opts)
        .map_err(|e| match e.kind {
            datumesh_http::publish::PublishErrorKind::StartBeforeNow { name, start, now } => {
                Error::StartBeforeNow { name, start, now }
            }
            _ => Error::Request(e.to_string()),
        })
}

/// The call form of a cell: `<ns>/<fn>(k: v, ...)`, every argument labelled, the parts sorted as
/// strings; strings and blobs in double quotes (a `"` written `""`), other literals in their
/// lexical form, an open argument as `*`.
pub(crate) fn call_form(cell: &datumesh_core::Cell) -> String {
    let mut parts: Vec<String> = cell
        .args
        .iter()
        .map(|(k, a)| {
            let lit = match a {
                datumesh_core::Arg::Value(v @ (Scalar::String(_) | Scalar::Blob(_))) => {
                    format!("\"{}\"", v.lexical().replace('"', "\"\""))
                }
                datumesh_core::Arg::Value(v) => v.lexical().to_string(),
                datumesh_core::Arg::Slice => "*".to_string(),
            };
            format!("{k}: {lit}")
        })
        .collect();
    parts.sort();
    format!("{}/{}({})", cell.ns, cell.fn_name, parts.join(", "))
}

/// The URI of a cell (`cell://<ns>/<fn>?k=v&...`, no asof): core's `format_uri`.
pub(crate) fn cell_uri(cell: &datumesh_core::Cell) -> String {
    datumesh_core::format_uri(&datumesh_core::CellRef {
        ns: cell.ns.clone(),
        fn_name: cell.fn_name.clone(),
        args: cell.args.clone(),
        asof: None,
    })
}

/// The pin of a request: `name@<hash>(...)` or `fn_hash=` in a URI.
fn pin_of(rest: &str) -> Option<String> {
    let path = rest.split('(').next().unwrap_or(rest);
    if let Some((_, h)) = path.split_once('@') {
        return Some(h.trim().to_string());
    }
    let q = rest.split_once('?')?.1;
    q.split('&').find_map(|p| p.strip_prefix("fn_hash=")).map(str::to_string)
}

/// SPEC 2.5: a function under a rooted namespace may not read one under an unrooted one.
fn check_rooting(name: &str, d: &FunctionDescriptor) -> Result<(), Error> {
    if is_unrooted(host_of(name)) {
        return Ok(());
    }
    match d.inputs.iter().find(|i| is_unrooted(host_of(i))) {
        Some(i) => Err(Error::UnrootedInput { function: name.to_string(), input: i.clone() }),
        None => Ok(()),
    }
}

/// Callees for a definition: the store's bindings, a foreign name fetched when not held.
pub(crate) struct Callees<'a> {
    pub(crate) shared: &'a Shared,
    /// The first callee under an unrooted host that is neither this machine's nor trusted here.
    pub(crate) untrusted: RefCell<Option<String>>,
}

impl<'a> Callees<'a> {
    pub(crate) fn new(shared: &'a Shared) -> Callees<'a> {
        Callees { shared, untrusted: RefCell::new(None) }
    }
}

impl datumesh_formula::definition::Callees for Callees<'_> {
    fn class_descriptor(&self, class_hash: &str) -> Result<Option<datumesh_core::ClassDescriptor>, String> {
        match self.shared.store.class(class_hash).map_err(|e| e.to_string())? {
            Some(b) => datumesh_core::ClassDescriptor::from_json(&b).map(Some).map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    fn function_descriptor(&self, fn_hash: &str) -> Result<Option<Value>, String> {
        match self.shared.store.descriptor(fn_hash) {
            Some(b) => json::parse(&b).map(Some).map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    fn callee(&self, name: &str, at: &str) -> Result<Option<datumesh_formula::definition::Callee>, String> {
        let s = self.shared;
        let host = host_of(name);
        // SPEC 2.5 and CONTRACT-1.8b L4: an unrooted host's names are read only when it is this
        // machine's or trusted here and not revoked.
        if is_unrooted(host)
            && host != s.config.host
            && !trust::trusted_live(&s.config.config_dir, host).map_err(|e| e.to_string())?
        {
            *self.untrusted.borrow_mut() = Some(name.to_string());
            return Err(format!("{name}: an unrooted host that is neither this machine's nor trusted here"));
        }
        if !is_unrooted(host) && s.store.resolve_binding(name, END).map_err(|e| e.to_string())?.is_none() {
            s.may_fetch(host)?;
            s.import_name(name).map_err(|e| e.to_string())?;
        }
        let Some(b) = s.store.resolve_binding(name, at).map_err(|e| e.to_string())? else { return Ok(None) };
        let bytes = s.store.descriptor(&b.fn_hash).ok_or_else(|| format!("descriptor {} is not stored", b.fn_hash))?;
        let descriptor = json::parse(&bytes).map_err(|e| e.to_string())?;
        Ok(Some(datumesh_formula::definition::Callee { fn_hash: b.fn_hash, start: b.tx, descriptor }))
    }
}

/// The mesh over the local store: `local.invalid`, no cross-mesh resolution (fetches are the
/// resolver's), no rate limits, the formula runtime behind the gate.
pub(crate) fn build_mesh(shared: &Arc<Shared>, aliases: &AliasMap, base_url: Option<String>) -> Result<Mesh, Error> {
    let mut c = MeshConfig::new(&shared.config.host);
    c.cross_mesh = false;
    c.rate.enabled = false;
    c.pool_size = 8;
    c.aliases = aliases.bare();
    c.base_url = base_url;
    let formula: Arc<dyn datumesh_engine::Runtime> =
        Arc::new(datumesh_formula::runtime::FormulaRuntime::new(shared.store.clone()));
    let rt = Arc::new(Gated::new(formula, shared.clone()));
    Mesh::builder(c, shared.store.clone())
        .without_formula()
        .clock(shared.clock.clone())
        .runtime(rt)
        .signing_key(shared.key().signing_key().clone())
        .licence(shared.config.licence.clone())
        .build()
        .map_err(|e| Error::Config(format!("mesh: {e:?}")))
}
