//! The licence gate (brief: the evaluation rule; user decision 4a): a function evaluates on
//! this machine when every input's licence is on the open list, or the input is local (its
//! signer's host is unrooted, under `.invalid`). Otherwise the request is forwarded to the
//! mesh named in the configuration, and the receipt returned is that mesh's.
//!
//! The decision is made here and nowhere else: [`LicenceGate::permits`], asked for every
//! input receipt an evaluation reads, before the value is used. Share-alike entries are "open
//! for delivery". Licence inheritance on derived cells (spec-0.4.3 8.5, with publishers'
//! `declarations` in the key document) is the engine's job and the lineage policy step
//! 3.4's: when they come, this is the one place that takes them.

/// The gate's decision (see the module documentation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LicenceGate {
    open: Vec<String>,
}

/// Whether a host is unrooted (SPEC 2.5: its last label is `invalid`).
pub fn is_unrooted(host: &str) -> bool {
    host == "invalid" || host.ends_with(".invalid")
}

impl LicenceGate {
    /// The gate with this open list (SPDX ids and licence URLs, compared exactly).
    pub fn new(open: &[String]) -> LicenceGate {
        LicenceGate { open: open.to_vec() }
    }

    /// Whether an input under `licence`, signed by a key of `signer_host`, may be computed
    /// over here.
    pub fn permits(&self, licence: &str, signer_host: &str) -> bool {
        is_unrooted(signer_host) || self.open.iter().any(|l| l == licence)
    }

    /// The open list.
    pub fn open(&self) -> &[String] {
        &self.open
    }
}
