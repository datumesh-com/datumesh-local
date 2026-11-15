//! The machine key: an Ed25519 key generated at first run, kept in a file of mode 0600 in
//! the configuration directory (never logged or printed), whose 16-hex key id is the machine
//! id. Receipts this machine signs say `computed_by` `<host>#<key id>` (the host is this
//! machine's, `local.invalid` by default). A new key (a reinstall) is a new machine; a key
//! rotated out (`datumesh key rotate`) is kept as a retired key: its file renamed beside the
//! current one, its public entry in `retired-keys.json` with its `valid_to` (CONTRACT-1.8b L4).
//!
//! The file holds the 32-byte seed as 64 hex digits on its first line and, on the second,
//! `valid_from=<instant>`: when the key was made, the start of its entry in the key document.

use std::io::Write;
use std::path::{Path, PathBuf};

use datumesh_core::json::Value;
use datumesh_core::lexical;
use datumesh_core::SigningKey;
use datumesh_engine::{Clock, SystemClock};

use crate::Error;

/// The machine key (see the module documentation). `Debug` shows the key id only.
#[derive(Clone)]
pub struct MachineKey {
    key: SigningKey,
    valid_from: String,
    path: PathBuf,
}

impl std::fmt::Debug for MachineKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachineKey").field("key_id", &self.key.key_id()).field("path", &self.path).finish()
    }
}

fn parse_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

impl MachineKey {
    /// The key in `path`, or a new one written there (mode 0600, the directory created). A
    /// file that others may read (any group or other permission bit) is refused.
    pub fn load_or_create(path: &Path) -> Result<MachineKey, Error> {
        Self::load_or_create_at(path, &SystemClock)
    }

    /// As [`MachineKey::load_or_create`], with `clock` for the `valid_from` of a new key.
    pub fn load_or_create_at(path: &Path, clock: &dyn Clock) -> Result<MachineKey, Error> {
        let err = |e: String| Error::Key(format!("{}: {e}", path.display()));
        if path.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(path).map_err(|e| err(e.to_string()))?.permissions().mode();
                if mode & 0o077 != 0 {
                    return Err(err(format!("mode {:o}: the machine key must have mode 0600", mode & 0o777)));
                }
            }
            let text = std::fs::read_to_string(path).map_err(|e| err(e.to_string()))?;
            let mut lines = text.lines();
            let seed = lines.next().map(str::trim).and_then(parse_hex).ok_or_else(|| err("not a key file".into()))?;
            let valid_from = lines
                .find_map(|l| l.trim().strip_prefix("valid_from=").map(str::to_string))
                .filter(|v| lexical::check_instant(v, "$").is_ok())
                .ok_or_else(|| err("no valid_from".into()))?;
            return Ok(MachineKey { key: SigningKey::from_seed(&seed), valid_from, path: path.to_path_buf() });
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| err(e.to_string()))?;
        }
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| err(e.to_string()))?;
        let valid_from = clock.now();
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(path).map_err(|e| err(e.to_string()))?;
        f.write_all(format!("{}\nvalid_from={valid_from}\n", lexical::to_hex(&seed)).as_bytes())
            .map_err(|e| err(e.to_string()))?;
        f.sync_all().map_err(|e| err(e.to_string()))?;
        Ok(MachineKey { key: SigningKey::from_seed(&seed), valid_from, path: path.to_path_buf() })
    }

    /// The key id: the machine id.
    pub fn key_id(&self) -> &str {
        self.key.key_id()
    }

    /// When the key was made: its entry's `valid_from`.
    pub fn valid_from(&self) -> &str {
        &self.valid_from
    }

    /// The signing key.
    pub fn signing_key(&self) -> &SigningKey {
        &self.key
    }

    /// The key's entry in a key document (SPEC 9.2).
    pub fn entry(&self) -> Value {
        Value::Object(vec![
            ("alg".into(), Value::String("ed25519".into())),
            ("id".into(), Value::String(self.key.key_id().to_string())),
            ("public".into(), Value::String(lexical::to_hex(&self.key.public_key()))),
            ("valid_from".into(), Value::String(self.valid_from.clone())),
        ])
    }
}

/// The file of retired keys' public entries, in the configuration directory.
pub const RETIRED_KEYS: &str = "retired-keys.json";

/// The retired keys' entries (`retired-keys.json`, a JSON array), none when there is no file.
pub fn retired_entries(config_dir: &Path) -> Result<Vec<Value>, Error> {
    let path = config_dir.join(RETIRED_KEYS);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read(&path).map_err(|e| Error::Key(format!("{}: {e}", path.display())))?;
    match datumesh_core::json::parse(&text) {
        Ok(Value::Array(a)) => Ok(a),
        _ => Err(Error::Key(format!("{}: not a JSON array of key entries", path.display()))),
    }
}

/// Writes `bytes` to `path` with mode 0600, replacing it through a temporary file beside it.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let tmp = path.with_file_name(format!("{name}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// The key document of `host` with these key entries (each `{alg, id, public, valid_from,
/// valid_to?}`), in key id order, without duplicates (the first of an id is kept); checked as
/// SPEC 9.2 says.
pub fn local_key_document(host: &str, entries: &[Value]) -> Result<Vec<u8>, Error> {
    let mut keys: Vec<Value> = Vec::new();
    for e in entries {
        let id = e.get("id").and_then(Value::as_str).unwrap_or("");
        if !keys.iter().any(|k| k.get("id").and_then(Value::as_str) == Some(id)) {
            keys.push(e.clone());
        }
    }
    keys.sort_by(|a, b| a.get("id").and_then(Value::as_str).cmp(&b.get("id").and_then(Value::as_str)));
    let doc = Value::Object(vec![
        ("host".into(), Value::String(host.into())),
        ("keys".into(), Value::Array(keys)),
        ("proto".into(), Value::String("datumesh/1".into())),
    ])
    .to_jcs();
    datumesh_core::keys_from_response(host, 200, Some(&doc))
        .map_err(|e| Error::Key(format!("the local key document: {e}")))?;
    Ok(doc)
}

/// The key entries of a key document.
pub fn key_entries(doc: &[u8]) -> Vec<Value> {
    match datumesh_core::json::parse(doc).ok().and_then(|v| v.get("keys").cloned()) {
        Some(Value::Array(ks)) => ks,
        _ => Vec::new(),
    }
}
