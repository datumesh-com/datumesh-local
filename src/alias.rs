//! The alias map: short names and prefixes to full names, loaded from a JSON file at startup
//! (never compiled in) and treated as a refreshable snapshot.
//!
//! The file is one JSON object `{"entries": [{"short", "full", "tx"}, ...]}`. Each entry says
//! that `short` stands for `full` from the instant `tx` on. A `short` ending in `/` is a prefix
//! entry (`{"short": "ecb/", "full": "mesh-test.datumesh.com/ecb/", ...}`, `full` then ends in
//! `/` too); any other `short` is a bare name. A request that resolves a bare name to a class
//! not under `datumesh.com/std/` is refused with a local error (see the resolver).
//! In step 2.2 the file is produced from the cells `datumesh.com/std/default(name: "<short>")`:
//! each cell's value is `full`, and its receipt's `tx` is the entry's `tx`.
//!
//! The older shape, `{"<short>": "<full>", ..., "tx": "<instant>"}`, is still read: every entry
//! gets that `tx`.
//!
//! A request is resolved at its asof (`?asof=`, else now): for each short name, the entry with
//! the greatest `tx` not after the asof ([`AliasMap::at`]). Entries with a later `tx` do not
//! exist yet. The default map, installed at first run, is `local/` → `local.invalid/` and the test
//! mesh's `scripture/` and `ecb/`.
//!
//! A request written with a short name is resolved to its full name before anything else, so
//! the receipt answering it, and every envelope, records the full name (SPEC 5.8: short names
//! never appear in a response).

use datumesh_core::json::{self, Value};
use datumesh_core::lexical;

use std::collections::BTreeMap;

use crate::Error;

/// One entry of the alias map: `short` stands for `full` from the instant `tx` on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The short name (a prefix when it ends in `/`).
    pub short: String,
    /// The full name it stands for.
    pub full: String,
    /// The instant the entry took effect (a lexical instant, SPEC 3.4).
    pub tx: String,
}

/// The alias map (see the module documentation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasMap {
    entries: Vec<Entry>,
}

fn first_segment(name: &str) -> &str {
    name.split('/').next().unwrap_or(name)
}

fn is_full(name: &str) -> bool {
    lexical::check_dns_name(first_segment(name), "$").is_ok() && first_segment(name).contains('.')
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Checks one entry: a short name, a full name, and the same kind (prefix or not).
fn check_entry(short: &str, full: &str) -> Result<(), Error> {
    if short.is_empty() || !is_full(full) || short.ends_with('/') != full.ends_with('/') {
        return Err(Error::Config(format!("alias map: {short} -> {full} is not a short name and a full name")));
    }
    Ok(())
}

impl AliasMap {
    /// The map in a file's bytes: the entries shape, or the older shape (see the module
    /// documentation).
    pub fn from_json(bytes: &[u8]) -> Result<AliasMap, Error> {
        let v = json::parse(bytes).map_err(|e| Error::Config(format!("alias map: {e}")))?;
        let m = v.as_object().ok_or_else(|| Error::Config("alias map: not an object".into()))?;
        if let Some((_, list)) = m.iter().find(|(k, _)| k == "entries") {
            if m.len() != 1 {
                return Err(Error::Config("alias map: entries is the only member of the file".into()));
            }
            let Value::Array(list) = list else {
                return Err(Error::Config("alias map: entries is not an array".into()));
            };
            let mut entries = Vec::new();
            for e in list {
                let field = |k: &str| {
                    e.get(k)
                        .and_then(Value::as_str)
                        .ok_or_else(|| Error::Config(format!("alias map: an entry has no string {k}")))
                };
                let (short, full, tx) = (field("short")?, field("full")?, field("tx")?);
                lexical::check_instant(tx, "$.tx")
                    .map_err(|err| Error::Config(format!("alias map: {short}: tx: {err}")))?;
                check_entry(short, full)?;
                entries.push(Entry { short: short.to_string(), full: full.to_string(), tx: tx.to_string() });
            }
            return Ok(AliasMap { entries });
        }
        // The older shape: every entry gets the file's tx.
        let mut tx = None;
        let mut pairs = Vec::new();
        for (k, x) in m {
            let s = x.as_str().ok_or_else(|| Error::Config(format!("alias map: {k} is not a string")))?;
            if k == "tx" {
                lexical::check_instant(s, "$.tx").map_err(|e| Error::Config(format!("alias map: tx: {e}")))?;
                tx = Some(s.to_string());
                continue;
            }
            check_entry(k, s)?;
            pairs.push((k.clone(), s.to_string()));
        }
        let tx = tx.ok_or_else(|| Error::Config("alias map: no tx".into()))?;
        let entries = pairs.into_iter().map(|(short, full)| Entry { short, full, tx: tx.clone() }).collect();
        Ok(AliasMap { entries })
    }

    /// The map for this machine's host: an entry naming `local.invalid` (the default map's
    /// `local/` → `local.invalid/`) names `host` instead (CONTRACT-1.8b L4).
    pub(crate) fn for_host(mut self, host: &str) -> AliasMap {
        for e in self.entries.iter_mut() {
            if let Some(rest) = e.full.strip_prefix(crate::LOCAL_HOST).and_then(|r| r.strip_prefix('/')) {
                e.full = format!("{host}/{rest}");
            }
        }
        self
    }

    /// The map in the file at `path`.
    pub fn load(path: &std::path::Path) -> Result<AliasMap, Error> {
        let bytes = std::fs::read(path).map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
        AliasMap::from_json(&bytes)
    }

    /// The entries, every one as the file gives it.
    pub fn all(&self) -> &[Entry] {
        &self.entries
    }

    /// The map as it stands at `asof`: for each short name the entry with the greatest `tx` not
    /// after `asof`. Its [`AliasMap::tx`] is the greatest `tx` among the entries it keeps, which
    /// is the `resolved.aliases_tx` of a request at `asof`.
    pub fn at(&self, asof: &str) -> AliasMap {
        let mut best: BTreeMap<&str, &Entry> = BTreeMap::new();
        for e in self.entries.iter().filter(|e| e.tx.as_str() <= asof) {
            let slot = best.entry(e.short.as_str()).or_insert(e);
            if e.tx > slot.tx {
                *slot = e;
            }
        }
        AliasMap { entries: best.into_values().cloned().collect() }
    }

    /// The instant of the map: the greatest `tx` among its entries, `""` for an empty map. For a
    /// map from [`AliasMap::at`], the snapshot's instant (`resolved.aliases_tx`).
    pub fn tx(&self) -> &str {
        self.entries.iter().map(|e| e.tx.as_str()).max().unwrap_or("")
    }

    /// The short name to full name map, each short name's entry with the greatest `tx`.
    pub fn entries(&self) -> BTreeMap<String, String> {
        let mut best: BTreeMap<String, &Entry> = BTreeMap::new();
        for e in &self.entries {
            let slot = best.entry(e.short.clone()).or_insert(e);
            if e.tx > slot.tx {
                *slot = e;
            }
        }
        best.into_iter().map(|(k, e)| (k, e.full.clone())).collect()
    }

    /// The bare names (no `/`), for the formula's `@alias` syntax.
    pub fn bare(&self) -> BTreeMap<String, String> {
        self.entries().into_iter().filter(|(k, _)| !k.ends_with('/')).collect()
    }

    /// The full name of `name`: a full name as it is; a bare name's entry; the longest
    /// matching prefix replaced; otherwise unchanged.
    pub fn expand(&self, name: &str) -> String {
        if is_full(name) {
            return name.to_string();
        }
        let map = self.entries();
        if let Some(v) = map.get(name) {
            if !name.ends_with('/') {
                return v.clone();
            }
        }
        let best =
            map.iter().filter(|(k, _)| k.ends_with('/') && name.starts_with(k.as_str())).max_by_key(|(k, _)| k.len());
        match best {
            Some((k, v)) => format!("{v}{}", &name[k.len()..]),
            None => name.to_string(),
        }
    }

    /// A request with its callee's prefix entry expanded (`ecb/fx_pair(...)`, the `ecb/` entry), and
    /// nothing else: a bare name is left to the call's own alias resolution, which the answer records
    /// (16.7), and a `cell://` URI is as it is.
    pub(crate) fn expand_prefix(&self, text: &str) -> String {
        let t = text.trim();
        if t.starts_with("cell://") {
            return t.to_string();
        }
        let end = t.find(['(', '@', '?', '[']).unwrap_or(t.len());
        let head = t[..end].trim_end();
        if !head.contains('/') || is_full(head) {
            return t.to_string();
        }
        let map = self.entries();
        let best =
            map.iter().filter(|(k, _)| k.ends_with('/') && head.starts_with(k.as_str())).max_by_key(|(k, _)| k.len());
        match best {
            Some((k, v)) => format!("{v}{}{}", &head[k.len()..], &t[head.len()..]),
            None => t.to_string(),
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

    /// A formula source with its prefix aliases (`@ecb/fx_rate`, an alias with a `/`) expanded to
    /// full names, before the formula is defined: the hash is then that of the source written with
    /// full names. A bare `@name`, a pin, a string literal and an alias with no entry are as they
    /// are (the formula's own alias check refuses one with no entry, `E_FORMULA_ALIAS`).
    pub fn expand_source(&self, source: &str) -> String {
        let b = source.as_bytes();
        let mut out: Vec<u8> = Vec::with_capacity(b.len());
        let mut i = 0;
        let mut in_string = false;
        while i < b.len() {
            let c = b[i];
            if in_string {
                out.push(c);
                if c == b'\\' && i + 1 < b.len() {
                    out.push(b[i + 1]);
                    i += 2;
                    continue;
                }
                in_string = c != b'"';
                i += 1;
                continue;
            }
            if c == b'"' {
                in_string = true;
                out.push(c);
                i += 1;
                continue;
            }
            if c != b'@' {
                out.push(c);
                i += 1;
                continue;
            }
            let mut j = i + 1;
            loop {
                while j < b.len() && is_word(b[j]) {
                    j += 1;
                }
                if j + 1 < b.len() && b[j] == b'/' && is_word(b[j + 1]) {
                    j += 1;
                    continue;
                }
                break;
            }
            // `i + 1` and `j` are at ASCII boundaries (after the `@`, and at a byte that is not a
            // word character or `/`), so the slice is valid text.
            let alias = &source[i + 1..j];
            let full = if alias.contains('/') { self.expand(alias) } else { alias.to_string() };
            if alias.contains('/') && full != alias {
                out.extend_from_slice(full.as_bytes());
            } else {
                out.extend_from_slice(&b[i..j]);
            }
            i = j;
        }
        String::from_utf8(out).unwrap_or_else(|_| source.to_string())
    }
}

/// The head of a request: its name as written, before any `(`, `@`, `?` or `[`, trimmed.
pub(crate) fn request_head(text: &str) -> &str {
    let t = text.trim();
    let end = t.find(['(', '@', '?', '[']).unwrap_or(t.len());
    t[..end].trim_end()
}

/// The asof of a request text (`?asof=<instant>` or `&asof=<instant>`), when it has one.
pub(crate) fn request_asof(text: &str) -> Option<String> {
    let at = text.find("?asof=").or_else(|| text.find("&asof="))? + "?asof=".len();
    let rest = &text[at..];
    let end = rest.find(['&', '?', ')', ' ']).unwrap_or(rest.len());
    let v = rest[..end].trim();
    (!v.is_empty()).then(|| v.to_string())
}
