//! The [Datumesh](https://datumesh.com) local resolver: the mesh's engine on a user's or an
//! agent's machine, over a local store (SQLite by default, one file; PostgreSQL by URL), with
//! the unrooted `local.invalid` namespace, no network except explicit fetches, a `prefetch`
//! that makes a working set warm, a licence gate that forwards what may not be computed here,
//! export and import of working sets, and a localhost endpoint with the mesh's handlers. The
//! `datumesh` binary (crate `datumesh`) is [`cli`]. It is the cache tier of the memory
//! hierarchy, and the thing agents publish to.
//!
//! ```no_run
//! use datumesh_local::{Config, Resolver};
//!
//! let r = Resolver::open(Config::load()?)?;
//! // A cell of the test mesh: fetched once (an explicit fetch), then answered from the store.
//! let reply = r.get(r#"ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)"#)?;
//! println!("{} {}", reply.status, String::from_utf8_lossy(&reply.body));
//! # Ok::<(), datumesh_local::Error>(())
//! ```
//!
//! # The `local.invalid` namespace
//!
//! Cells this machine defines live under **`local.invalid`** (users write `local/…`; the
//! alias map's default entry maps the prefix `local/` to `local.invalid/`, and every receipt
//! and envelope records the full name). Each machine has its own Ed25519 key, generated at
//! first run and kept in a file of mode 0600 ([`MachineKey`]); its 16-hex key id is the
//! machine id, and receipts this machine signs say `computed_by` `local.invalid#<key id>`.
//!
//! Why `.invalid`: the name is reserved by RFC 2606 and, by RFC 6761 section 6.4, never
//! resolves, so it can never be a real host and no certificate names it (SPEC 2.5, since
//! spec-0.4.3). Local receipts therefore stay in the one receipt format of SPEC section 8,
//! signed like any other, and verify exactly where the machine's key document is known: here
//! (the resolver supplies its own key document, so its receipts verify at `signed` and
//! `recomputed` on this machine), and wherever that document is supplied out of band
//! ([`Resolver::import`] with `--trust-key`). Anywhere else their verification stops at
//! `E_KEY_UNAVAILABLE`. Output labels the signer: [`LOCAL_LABEL`].
//!
//! A function under a rooted namespace may not read a local one: [`Resolver::define`]
//! refuses it ([`Error::UnrootedInput`]).
//!
//! # Answers
//!
//! [`Resolver::get`] answers a call (`name(arg: value, ...)`, a pin `name@<hash>(...)`) or a
//! cell URI with the envelope of SPEC 5.5, exactly as the mesh's `/cell` would, built by the
//! mesh's own handlers ([`datumesh_http::Mesh`]) over the local store:
//!
//! - a cell held (fetched before, or local) answers from the store: no network;
//! - a foreign stored cell not held is fetched once through the HTTP client (checked:
//!   envelope, signature against the publisher's key document, hashes), imported into the
//!   store, then answered from it, so later answers, online or offline, have the same bytes;
//! - a computed cell evaluates here, its foreign inputs fetched the same way, when the
//!   [`LicenceGate`] permits every input; otherwise the request is forwarded to the configured
//!   mesh and that mesh's answer (its receipt, its signature) is returned and kept;
//! - a foreign cell that is not held and cannot be fetched (offline, the network down, a host
//!   not allowed) answers `error` with [`FOREIGN_UNAVAILABLE`] (HTTP 502), never
//!   `not_covered`.
//!
//! # Network
//!
//! Only explicit fetches reach the network, through the Datumesh HTTP client, and only to the
//! hosts of the configured allowlist: a `get` of a cell not held, `prefetch`, `refresh`, a
//! forwarded request, `find --remote`. The resolver's mesh never resolves names across meshes
//! by itself.

//! # The localhost endpoint
//!
//! [`serve`] binds 127.0.0.1 only and serves the mesh's handlers over the local store. The
//! mesh document it serves is **local-only**: its URLs are `http://127.0.0.1:<port>/...`,
//! which spec-0.4.3 (9.6) does not allow (mesh-document URLs are `https://`), until
//! spec-0.4.4 lets an unrooted host (a name under `.invalid`) use `http://` on a loopback
//! address only (`127.0.0.1`, `[::1]`, `localhost`); rooted hosts stay https-only. Links in answers
//! (`poll`, a page's `next`, `rel="canonical"`) point at `https://local.invalid/...` and
//! cannot be followed.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(feature = "resolver")]
pub mod alias;
#[cfg(feature = "resolver")]
pub mod bundle;
#[cfg(feature = "resolver")]
pub mod cli;
#[cfg(feature = "resolver")]
pub mod config;
#[cfg(feature = "resolver")]
mod fetch;
#[cfg(feature = "resolver")]
pub mod gate;
#[cfg(feature = "resolver")]
pub mod index;
#[cfg(feature = "resolver")]
pub mod key;
#[cfg(feature = "resolver")]
mod resolver;
#[cfg(feature = "resolver")]
pub mod serve;
#[cfg(feature = "wasm")]
pub mod wasm;

#[cfg(feature = "resolver")]
pub use alias::AliasMap;
#[cfg(feature = "resolver")]
pub use bundle::Imported;
#[cfg(feature = "resolver")]
pub use config::Config;
#[cfg(feature = "resolver")]
pub use gate::LicenceGate;
#[cfg(feature = "resolver")]
pub use key::MachineKey;
#[cfg(feature = "resolver")]
pub use resolver::{Defined, Found, Prefetched, Quarantined, Refreshed, Reply, Resolver, Source};

/// The unrooted host of this machine's cells.
pub const LOCAL_HOST: &str = "local.invalid";

/// The label of receipts this machine signed, in CLI and HTTP output.
pub const LOCAL_LABEL: &str = "local, verifies only on this machine";

/// The label of an imported receipt signed by another machine's local key, not trusted.
pub const UNVERIFIABLE_LABEL: &str = "unverifiable: signed by another machine's local key";

/// The `error` code of a foreign cell that is not held and cannot be fetched (offline, the
/// network or the host down, a host not allowed): `E_FOREIGN_UNAVAILABLE` (spec-0.4.3, D78:
/// outcome `error`, HTTP 502, a required envelope member `detail`). The one place the code
/// is named.
///
/// datumesh-core 0.3.6's `ErrorCode` does not have it (it comes with core 0.3.7), so the
/// envelope is written here ([`foreign_unavailable_envelope`]) and core 0.3.6 cannot check it.
pub const FOREIGN_UNAVAILABLE: &str = "E_FOREIGN_UNAVAILABLE";

/// The HTTP status of [`FOREIGN_UNAVAILABLE`].
pub const FOREIGN_UNAVAILABLE_STATUS: u16 = 502;

/// The values of the envelope's `detail` with [`FOREIGN_UNAVAILABLE`] (spec-0.4.3, a closed
/// set): `offline` (no network, or configured not to use it: offline mode, a host outside
/// the allowlist), `network`, `dns`, `tls`, `timeout`, `refused`.
pub const FOREIGN_UNAVAILABLE_DETAILS: [&str; 6] = ["offline", "network", "dns", "tls", "timeout", "refused"];

/// The envelope of [`FOREIGN_UNAVAILABLE`] with `detail` (canonical JSON, SPEC 5.5's `error`
/// outcome). A `detail` outside [`FOREIGN_UNAVAILABLE_DETAILS`] is written `network`.
pub fn foreign_unavailable_envelope(detail: &str) -> Vec<u8> {
    let d = if FOREIGN_UNAVAILABLE_DETAILS.contains(&detail) { detail } else { "network" };
    format!(r#"{{"detail":"{d}","error":"{FOREIGN_UNAVAILABLE}","outcome":"error"}}"#).into_bytes()
}

/// What went wrong outside an answer (an answer, `error` envelopes included, is a [`Reply`]).
#[cfg(feature = "resolver")]
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The configuration or the alias map.
    Config(String),
    /// The machine key.
    Key(String),
    /// The store or the index.
    Store(String),
    /// Another mesh's answer failed, or failed its check: the client's code and why.
    Client {
        /// The code (SPEC 10.2).
        code: String,
        /// Why.
        detail: String,
    },
    /// Nothing could be fetched: [`FOREIGN_UNAVAILABLE`], with the failure (`offline`,
    /// `network`, `dns`, `tls`, `timeout`, `refused`, `not allowed`).
    ForeignUnavailable(String),
    /// The request is malformed or not supported.
    Request(String),
    /// A function under a rooted namespace reads one under an unrooted namespace (SPEC 2.5):
    /// refused at define time. No code of SPEC 10.2 or 15.11 names this refusal.
    UnrootedInput {
        /// The function defined.
        function: String,
        /// The unrooted input it reads.
        input: String,
    },
    /// The definition failed (the formula's checks).
    Define(String),
    /// A bundle could not be read or written.
    Bundle(String),
    /// `serve` was asked to bind an address other than 127.0.0.1.
    Address(String),
}

#[cfg(feature = "resolver")]
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Config(d) => write!(f, "configuration: {d}"),
            Error::Key(d) => write!(f, "machine key: {d}"),
            Error::Store(d) => write!(f, "store: {d}"),
            Error::Client { code, detail } => write!(f, "{code}: {detail}"),
            Error::ForeignUnavailable(d) => write!(f, "{FOREIGN_UNAVAILABLE}: {d}"),
            Error::Request(d) => write!(f, "request: {d}"),
            Error::UnrootedInput { function, input } => write!(
                f,
                "{function} is under a rooted namespace and reads {input}, under an unrooted one (SPEC 2.5): refused"
            ),
            Error::Define(d) => write!(f, "define: {d}"),
            Error::Bundle(d) => write!(f, "bundle: {d}"),
            Error::Address(d) => write!(f, "address: {d}"),
        }
    }
}

#[cfg(feature = "resolver")]
impl std::error::Error for Error {}
