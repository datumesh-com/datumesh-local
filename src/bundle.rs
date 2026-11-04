//! Export and import of a working set: a bundle of receipts, descriptors, bindings, versions,
//! manifests, blobs, key documents and forwarded answers, so one resolver's working set can
//! seed another's. The bundle is signed where its contents are signed: every receipt carries
//! its signer's signature and is verified again on import; nothing else is trusted
//! (descriptors and blobs are content-addressed, key documents are checked as SPEC 9.2 says).
//!
//! The exporting machine's key document for `local.invalid` travels in the bundle. On import,
//! a receipt signed by another machine's local key is kept aside and labelled
//! [`crate::UNVERIFIABLE_LABEL`], unless that key id is named with `--trust-key`: the key is
//! then added to this machine's `local.invalid` key document (and remembered as trusted), and
//! its receipts verify and answer. Receipts signed under rooted namespaces verify normally.
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
use crate::{Error, Resolver, LOCAL_HOST};

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
    if s.len() % 4 != 0 {
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
        Err(Error::Request("not implemented yet".into()))
    }

    /// Imports a bundle (see the module documentation). `trust` names other machines' key
    /// ids (from the bundle's `local.invalid` key document) to accept.
    pub fn import(&self, bundle: &[u8], trust: &[String]) -> Result<Imported, Error> {
        Err(Error::Request("not implemented yet".into()))
    }
}
