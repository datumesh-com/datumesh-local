//! The alias map: short names and prefixes to full names, loaded from a JSON file at startup
//! (never compiled in) and treated as a refreshable snapshot.
//!
//! The file is one JSON object mapping a name to its full name, plus `tx`, the instant the
//! map was taken (in step 2.2 it will be produced from the binding cell
//! `datumesh.com/std/default(name)` on the mesh). A key ending in `/` is a prefix
//! (`"local/": "local.invalid/"`); any other key is a bare name (`"fx": "example.org/fx_rate"`).
//! `tx` is reserved. The default map, installed at first run, is `local/` → `local.invalid/`
//! and the test mesh's `scripture/` and `ecb/`.
//!
//! A request written with a short name is resolved to its full name before anything else, so
//! the receipt answering it, and every envelope, records the full name (SPEC 5.8: short names
//! never appear in a response).

use std::collections::BTreeMap;

use datumesh_core::json;
use datumesh_core::lexical;

use crate::Error;

/// The alias map (see the module documentation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasMap {
    entries: BTreeMap<String, String>,
    tx: String,
}

fn first_segment(name: &str) -> &str {
    name.split('/').next().unwrap_or(name)
}

fn is_full(name: &str) -> bool {
    lexical::check_dns_name(first_segment(name), "$").is_ok() && first_segment(name).contains('.')
}

impl AliasMap {
    /// The map in a file's bytes.
    pub fn from_json(bytes: &[u8]) -> Result<AliasMap, Error> {
        let v = json::parse(bytes).map_err(|e| Error::Config(format!("alias map: {e}")))?;
        let m = v.as_object().ok_or_else(|| Error::Config("alias map: not an object".into()))?;
        let mut entries = BTreeMap::new();
        let mut tx = None;
        for (k, x) in m {
            let s = x.as_str().ok_or_else(|| Error::Config(format!("alias map: {k} is not a string")))?;
            if k == "tx" {
                lexical::check_instant(s, "$.tx").map_err(|e| Error::Config(format!("alias map: tx: {e}")))?;
                tx = Some(s.to_string());
                continue;
            }
            if k.is_empty() || !is_full(s) || k.ends_with('/') != s.ends_with('/') {
                return Err(Error::Config(format!("alias map: {k} -> {s} is not a short name and a full name")));
            }
            entries.insert(k.clone(), s.to_string());
        }
        let tx = tx.ok_or_else(|| Error::Config("alias map: no tx".into()))?;
        Ok(AliasMap { entries, tx })
    }

    /// The map in the file at `path`.
    pub fn load(path: &std::path::Path) -> Result<AliasMap, Error> {
        let bytes = std::fs::read(path).map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
        AliasMap::from_json(&bytes)
    }

    /// When the map was taken.
    pub fn tx(&self) -> &str {
        &self.tx
    }

    /// The entries, short name to full name.
    pub fn entries(&self) -> &BTreeMap<String, String> {
        &self.entries
    }

    /// The bare names (no `/`), for the formula's `@alias` syntax.
    pub fn bare(&self) -> BTreeMap<String, String> {
        self.entries.iter().filter(|(k, _)| !k.ends_with('/')).map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// The full name of `name`: a full name as it is; a bare name's entry; the longest
    /// matching prefix replaced; otherwise unchanged.
    pub fn expand(&self, name: &str) -> String {
        if is_full(name) {
            return name.to_string();
        }
        if let Some(v) = self.entries.get(name) {
            if !name.ends_with('/') {
                return v.clone();
            }
        }
        let best = self
            .entries
            .iter()
            .filter(|(k, _)| k.ends_with('/') && name.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len());
        match best {
            Some((k, v)) => format!("{v}{}", &name[k.len()..]),
            None => name.to_string(),
        }
    }

    /// A request (a call `name(args)`, `name@<hash>(args)`, a slice, or a function name) with
    /// its name expanded; a `cell://` URI as it is.
    pub fn expand_request(&self, text: &str) -> String {
        let t = text.trim();
        if t.starts_with("cell://") {
            return t.to_string();
        }
        let end = t.find(['(', '@', '?']).unwrap_or(t.len());
        format!("{}{}", self.expand(t[..end].trim_end()), &t[end..])
    }
}
