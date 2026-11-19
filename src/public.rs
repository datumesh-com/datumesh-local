//! Publishing to a public mesh (CONTRACT-build2-part4-publish P1, the local side): the rooting of
//! this machine's names under a namespace, checked here against the public rule (every formula's
//! tests cover its derived cases and take every branch arm, E5), then posted as a bundle 1 with its
//! tests to the mesh's bundles endpoint. The mesh does the rest (its checks answer with a code,
//! which is passed on).

use datumesh_core::json::{self, Value};

use crate::net;
use crate::{Error, Resolver, RootPlan};

/// What a publish to a public mesh was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicPlan {
    /// The public mesh's URL (`https://<host>`, or the origin of a local one).
    pub to: String,
    /// The rooting plan: the namespace, its host's key file, this machine's names, and the bundle
    /// to keep (`export`), when asked.
    pub root: RootPlan,
}

impl Resolver {
    /// Roots the names of the plan, checks them against the public rule, and posts the bundle to
    /// the public mesh (P1). The mesh's `201` answer is returned: `{imported, names, functions}`,
    /// with `namespace` and `to` added. A mesh's refusal is `Error::Refused` with its code and
    /// detail. Nothing is sent when the rooting or the public rule refuses.
    pub fn publish_public(&self, plan: &PublicPlan) -> Result<Value, Error> {
        let (rooted, bundle) = self.root_with(&plan.root, true)?;
        if let Some(path) = &plan.root.export {
            std::fs::write(path, &bundle).map_err(|e| Error::Bundle(format!("{}: {e}", path.display())))?;
        }
        let endpoint = self.bundles_endpoint(&plan.to)?;
        let timeout = self.shared.config.timeout;
        let (status, answer) = net::post(&endpoint, &bundle, timeout).map_err(Error::ForeignUnavailable)?;
        let body = json::parse(&answer).ok();
        if status == 201 {
            let Some(Value::Object(mut m)) = body else {
                return Err(Error::Client {
                    code: "E_MESH".into(),
                    detail: format!("{endpoint}: a 201 without a JSON answer"),
                });
            };
            m.retain(|(k, _)| k != "namespace" && k != "to");
            m.push(("namespace".into(), Value::String(rooted.namespace.clone())));
            m.push(("to".into(), Value::String(plan.to.clone())));
            return Ok(Value::Object(m));
        }
        let code = body
            .as_ref()
            .and_then(|b| b.get("error"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("status {status}"));
        let detail = body.as_ref().and_then(|b| b.get("detail")).cloned().unwrap_or(Value::Null);
        Err(Error::Refused { code, detail: String::from_utf8_lossy(&detail.to_jcs()).into_owned() })
    }

    /// The bundles endpoint of the public mesh at `to`: the `x_bundles` member of its mesh
    /// document (`<to>/.well-known/datumesh-mesh.json`), or `<to>/bundles` when there is no
    /// document or no such member.
    fn bundles_endpoint(&self, to: &str) -> Result<String, Error> {
        let base = to.trim_end_matches('/');
        let timeout = self.shared.config.timeout;
        let doc = net::get(&format!("{base}/.well-known/datumesh-mesh.json"), timeout).ok();
        let declared = match doc {
            Some((200, bytes)) => json::parse(&bytes).ok().and_then(|d| {
                d.get("endpoints").and_then(|e| e.get("x_bundles")).and_then(Value::as_str).map(str::to_string)
            }),
            _ => None,
        };
        Ok(declared.unwrap_or_else(|| format!("{base}/bundles")))
    }
}
