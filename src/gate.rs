//! The licence gate: whether this machine may compute over an input (the user's decision L1 C,
//! 2026-11-13). Since 0.1.2 it is the engine's rule, licence inheritance (spec-0.4.3 8.5,
//! datumesh-core's `inherit_licence`, with the declarations of the function's publisher from
//! the key document of `cell.ns`'s host held in the local store), plus one rule of this
//! resolver's own: **a restricted licence is never signed by an unrooted key**. A receipt
//! signed `local.invalid#<key id>` verifies only on this machine, so a restricted result signed
//! here could not be shown to anyone as the publisher's entitled computation.
//!
//! For each input an evaluation reads, [`LicenceGate::permits`] answers:
//!
//! | The input | Decision | The request |
//! |---|---|---|
//! | local (its signer's host is unrooted) | [`GateDecision::Compute`] | evaluated here |
//! | under an open licence (8.5's tiers 1-3: CC0-1.0, PDDL-1.0, MIT, Apache-2.0, CC-BY-4.0, OGL-UK-3.0, CC-BY-SA-4.0, ODbL-1.0) | [`GateDecision::Compute`] | evaluated here; the engine gives the result the inherited licence |
//! | restricted (any other licence, every `https://` one included), declared by the publisher for this function and open now | [`GateDecision::Forward`] | forwarded to the configured mesh; its receipt is returned |
//! | restricted, not declared (or the declaration not open) | [`GateDecision::Refuse`] | `unlicensed` (403) here, without a round trip |
//!
//! Conflicts among several inputs (two share-alike licences, two restricted ones, share-alike
//! with restricted) are the engine's to refuse when it signs (`unlicensed`, 403), by the same
//! rule. The decision is made here and nowhere else.
//!
//! **Delivery.** The gate decides computation only. A restricted cell the mesh delivers is
//! held here like any other (fetched, prefetched, exported): delivery is allowed; the licence
//! binds computation. When per-principal delivery arrives (plan 3.4), a cell the mesh answers
//! `unlicensed` never arrives, so there is nothing to hold.
//!
//! The open list of 0.1.0 and 0.1.1 (`open_licences` in the configuration) is no longer a
//! computation gate and nothing uses it; it is still accepted in the configuration file, and
//! ignored (deprecated).

use datumesh_core::{inherit_licence, Declarations, LicenceReason};

/// What the gate decides for one input of an evaluation (see the module documentation).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateDecision {
    /// Evaluate here.
    Compute,
    /// Forward the request to the configured mesh: a declared restricted input.
    Forward,
    /// Answer `unlicensed` here: the inheritance rule refuses (`not_declared`,
    /// `declaration_not_open`).
    Refuse(LicenceReason),
}

/// The gate's decision (see the module documentation). It holds no state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LicenceGate {
    _private: (),
}

/// Whether a host is unrooted (SPEC 2.5: its last label is `invalid`).
pub fn is_unrooted(host: &str) -> bool {
    host == "invalid" || host.ends_with(".invalid")
}

impl LicenceGate {
    /// The gate.
    pub fn new() -> LicenceGate {
        LicenceGate::default()
    }

    /// The decision for an input under `licence`, signed by a key of `signer_host`, read by
    /// the function `function` (its full name) whose publisher's declarations are
    /// `declarations` (the key document of the host of the function's namespace, as held;
    /// `None` when it has none), for a receipt to be issued at `issued`.
    pub fn permits(
        &self,
        function: &str,
        licence: &str,
        signer_host: &str,
        declarations: Option<&Declarations>,
        issued: &str,
    ) -> GateDecision {
        if is_unrooted(signer_host) {
            return GateDecision::Compute;
        }
        let rule = inherit_licence([licence], declarations, function, issued, "");
        match (rule.licence, rule.reason) {
            (Some(_), LicenceReason::Declared) => GateDecision::Forward,
            (Some(_), _) => GateDecision::Compute,
            (None, reason) => GateDecision::Refuse(reason),
        }
    }
}
