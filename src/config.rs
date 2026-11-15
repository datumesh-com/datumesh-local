//! The resolver's configuration: `config.json` in the configuration directory, installed
//! with its comments at first run (JSON; a line whose first non-blank characters are `//` is
//! a comment), with the machine key and the alias map beside it, and the store and the
//! resolver's index in the data directory.
//!
//! | | Linux | macOS | Windows |
//! |---|---|---|---|
//! | configuration | `$XDG_CONFIG_HOME/datumesh` (`~/.config/datumesh`) | `~/Library/Application Support/datumesh` | `%APPDATA%\datumesh` |
//! | data | `$XDG_DATA_HOME/datumesh` (`~/.local/share/datumesh`) | the same | the same |
//!
//! The environment overrides: `DATUMESH_HOME` (one directory for both), `DATUMESH_CONFIG_DIR`,
//! `DATUMESH_DATA_DIR`, `DATUMESH_KEY_FILE`, `DATUMESH_STORE`, and `DATUMESH_OFFLINE=1`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use datumesh_core::json::{self, Value};

use crate::Error;

/// The configuration file as installed at first run.
pub const DEFAULT_CONFIG: &str = include_str!("../defaults/config.json");
/// The alias map as installed at first run.
pub const DEFAULT_ALIASES: &str = include_str!("../defaults/aliases.json");

/// The resolver's configuration.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct Config {
    /// Where `config.json`, the machine key and the alias map are.
    pub config_dir: PathBuf,
    /// This machine's host (config member `host`, default `local.invalid`): a name under
    /// `.invalid` (CONTRACT-1.8b L4). Its cells are `<host>/...`, its receipts say `computed_by`
    /// `<host>#<key id>`, and `local/` in names and aliases means `<host>/`.
    pub host: String,
    /// Where the store and the resolver's index are.
    pub data_dir: PathBuf,
    /// The store: a SQLite path or a `postgres://` URL (default `<data dir>/store.sqlite`).
    pub store: Option<String>,
    /// The machine key's file (default `<config dir>/machine.key`).
    pub key_file: PathBuf,
    /// The alias map's file (default `<config dir>/aliases.json`).
    pub aliases_file: PathBuf,
    /// The only hosts fetched from.
    pub allowlist: Vec<String>,
    /// Deprecated since 0.2.0, ignored: the open list of 0.1.0 and 0.1.1, no longer a
    /// computation gate (the gate is the engine's licence inheritance, [`crate::gate`]). Still
    /// read from a configuration file that has it.
    pub open_licences: Vec<String>,
    /// The mesh requests the gate refuses are forwarded to.
    pub forward_mesh: Option<String>,
    /// No fetch at all.
    pub offline: bool,
    /// The licence of the receipts this machine signs for cells without inputs (SPEC 8.5:
    /// the mesh's own licence); every other receipt carries the inherited licence.
    pub licence: String,
    /// Origins to use instead of `https://<host>` for some hosts.
    pub peers: BTreeMap<String, String>,
    /// The time limit of one request to a mesh.
    pub timeout: Duration,
}

/// Whether `host` is a machine's host: a name under `.invalid` (RFC 2606), lowercase labels of
/// letters, digits and hyphens, at least one label before `.invalid` (CONTRACT-1.8b L4).
pub fn is_machine_host(host: &str) -> bool {
    host.strip_suffix(".invalid").is_some_and(|stem| {
        !stem.is_empty()
            && stem
                .split('.')
                .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
    })
}

/// Checks a configured host (see [`is_machine_host`]): `Error::Config` otherwise.
pub fn check_host(host: &str) -> Result<(), Error> {
    if is_machine_host(host) {
        Ok(())
    } else {
        Err(Error::Config(format!("host {host}: must be a name under .invalid (CONTRACT-1.8b L4)")))
    }
}

/// The platform's configuration and data directories for Datumesh.
pub fn default_dirs() -> Result<(PathBuf, PathBuf), Error> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(h) = env("DATUMESH_HOME") {
        return Ok((h.clone(), h));
    }
    let (mut config, mut data) = platform_dirs()?;
    if let Some(c) = env("DATUMESH_CONFIG_DIR") {
        config = c;
    }
    if let Some(d) = env("DATUMESH_DATA_DIR") {
        data = d;
    }
    Ok((config, data))
}

#[cfg(target_os = "windows")]
fn platform_dirs() -> Result<(PathBuf, PathBuf), Error> {
    let appdata = std::env::var_os("APPDATA").ok_or_else(|| Error::Config("APPDATA is not set".into()))?;
    let d = PathBuf::from(appdata).join("datumesh");
    Ok((d.clone(), d))
}

#[cfg(target_os = "macos")]
fn platform_dirs() -> Result<(PathBuf, PathBuf), Error> {
    let home = std::env::var_os("HOME").ok_or_else(|| Error::Config("HOME is not set".into()))?;
    let d = PathBuf::from(home).join("Library/Application Support/datumesh");
    Ok((d.clone(), d))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_dirs() -> Result<(PathBuf, PathBuf), Error> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = env("HOME");
    let config = env("XDG_CONFIG_HOME")
        .or_else(|| home.as_ref().map(|h| h.join(".config")))
        .ok_or_else(|| Error::Config("neither XDG_CONFIG_HOME nor HOME is set".into()))?;
    let data = env("XDG_DATA_HOME")
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")))
        .ok_or_else(|| Error::Config("neither XDG_DATA_HOME nor HOME is set".into()))?;
    Ok((config.join("datumesh"), data.join("datumesh")))
}

/// Strips whole-line `//` comments.
fn strip_comments(text: &str) -> String {
    text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

/// Writes `contents` to `path` unless it exists.
fn install(path: &Path, contents: &str) -> Result<(), Error> {
    if path.exists() {
        return Ok(());
    }
    std::fs::write(path, contents).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
}

fn strings(v: &Value, what: &str) -> Result<Vec<String>, Error> {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|x| x.as_str().map(str::to_string).ok_or_else(|| Error::Config(format!("{what}: not a string"))))
            .collect(),
        _ => Err(Error::Config(format!("{what}: not an array"))),
    }
}

impl Config {
    /// The configuration of this machine: the directories of [`default_dirs`], the files
    /// there (installed at first run), then the environment's overrides.
    pub fn load() -> Result<Config, Error> {
        let (c, d) = default_dirs()?;
        let mut cfg = Config::with_dirs(&c, &d)?;
        if let Some(k) = std::env::var_os("DATUMESH_KEY_FILE").filter(|v| !v.is_empty()) {
            cfg.key_file = PathBuf::from(k);
        }
        if let Ok(s) = std::env::var("DATUMESH_STORE") {
            if !s.is_empty() {
                cfg.store = Some(s);
            }
        }
        if std::env::var("DATUMESH_OFFLINE").is_ok_and(|v| v == "1" || v == "true") {
            cfg.offline = true;
        }
        Ok(cfg)
    }

    /// The configuration in one directory (configuration and data both; tests, portable
    /// installs), installing the default files there at first run.
    pub fn in_dir(dir: impl AsRef<Path>) -> Result<Config, Error> {
        Config::with_dirs(dir.as_ref(), dir.as_ref())
    }

    /// The configuration with these directories, installing the default files at first run.
    pub fn with_dirs(config_dir: &Path, data_dir: &Path) -> Result<Config, Error> {
        for d in [config_dir, data_dir] {
            std::fs::create_dir_all(d).map_err(|e| Error::Config(format!("{}: {e}", d.display())))?;
        }
        let path = config_dir.join("config.json");
        install(&path, DEFAULT_CONFIG)?;
        install(&config_dir.join("aliases.json"), DEFAULT_ALIASES)?;
        let text = std::fs::read_to_string(&path).map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
        let v = json::parse(strip_comments(&text).as_bytes())
            .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
        let mut c = Config {
            config_dir: config_dir.to_path_buf(),
            host: crate::LOCAL_HOST.into(),
            data_dir: data_dir.to_path_buf(),
            store: None,
            key_file: config_dir.join("machine.key"),
            aliases_file: config_dir.join("aliases.json"),
            allowlist: Vec::new(),
            open_licences: Vec::new(),
            forward_mesh: None,
            offline: false,
            licence: "CC-BY-4.0".into(),
            peers: BTreeMap::new(),
            timeout: Duration::from_secs(10),
        };
        let m = v.as_object().ok_or_else(|| Error::Config(format!("{}: not an object", path.display())))?;
        let path_of = |x: &Value, k: &str| -> Result<Option<PathBuf>, Error> {
            match x {
                Value::Null => Ok(None),
                Value::String(s) => {
                    let p = PathBuf::from(s);
                    Ok(Some(if p.is_absolute() { p } else { config_dir.join(p) }))
                }
                _ => Err(Error::Config(format!("{k}: not a string or null"))),
            }
        };
        for (k, x) in m {
            match k.as_str() {
                "host" => {
                    c.host = match x {
                        Value::String(s) => s.clone(),
                        _ => return Err(Error::Config("host: not a string".into())),
                    };
                    check_host(&c.host)?;
                }
                "allowlist" => c.allowlist = strings(x, k)?,
                "open_licences" => c.open_licences = strings(x, k)?,
                "forward_mesh" => {
                    c.forward_mesh = match x {
                        Value::Null => None,
                        Value::String(s) => Some(s.clone()),
                        _ => return Err(Error::Config("forward_mesh: not a string or null".into())),
                    }
                }
                "offline" => {
                    c.offline = match x {
                        Value::Bool(b) => *b,
                        _ => return Err(Error::Config("offline: not a boolean".into())),
                    }
                }
                "licence" => {
                    c.licence = x.as_str().ok_or_else(|| Error::Config("licence: not a string".into()))?.to_string()
                }
                "peers" => {
                    let o = x.as_object().ok_or_else(|| Error::Config("peers: not an object".into()))?;
                    for (h, origin) in o {
                        let origin =
                            origin.as_str().ok_or_else(|| Error::Config(format!("peers.{h}: not a string")))?;
                        c.peers.insert(h.clone(), origin.to_string());
                    }
                }
                "timeout_seconds" => {
                    let n = x
                        .as_number_text()
                        .and_then(|n| n.parse::<u64>().ok())
                        .ok_or_else(|| Error::Config("timeout_seconds: not a whole number".into()))?;
                    c.timeout = Duration::from_secs(n.max(1));
                }
                "store" => {
                    c.store = match x {
                        Value::Null => None,
                        Value::String(s) => Some(s.clone()),
                        _ => return Err(Error::Config("store: not a string or null".into())),
                    }
                }
                "key_file" => {
                    if let Some(p) = path_of(x, k)? {
                        c.key_file = p;
                    }
                }
                "aliases_file" => {
                    if let Some(p) = path_of(x, k)? {
                        c.aliases_file = p;
                    }
                }
                other => return Err(Error::Config(format!("{}: unknown member {other}", path.display()))),
            }
        }
        Ok(c)
    }
}
