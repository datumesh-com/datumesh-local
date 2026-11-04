//! wasm32 bindings (feature `wasm`): the core's receipt verification and the call parser,
//! not the engine. Build with `cargo build --lib --no-default-features --features wasm
//! --target wasm32-unknown-unknown` and generate the JavaScript glue with `wasm-bindgen`
//! (the version of the `wasm-bindgen` crate in `Cargo.lock`).
//!
//! Every function takes and returns JSON text.

use datumesh_core::json::{self, Value};
use datumesh_core::{verify as core_verify, Cell, Keys, MapResolver, Scalar};
use datumesh_formula::call::{parse_call as formula_parse_call, MapCallEnv};
use wasm_bindgen::prelude::wasm_bindgen;

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn failure(code: &str, detail: &str) -> String {
    String::from_utf8_lossy(
        &Value::Object(vec![("code".into(), s(code)), ("detail".into(), s(detail)), ("ok".into(), Value::Bool(false))])
            .to_jcs(),
    )
    .into_owned()
}

/// Verifies a receipt (SPEC 10.1). `keys`: a JSON object mapping each signer host to its key
/// document (an object, or its text). `recompute`: empty, or a JSON object with the receipt
/// cell's `descriptor` and recomputed `value` (a typed scalar), for level `recomputed`.
/// Returns `{"ok", "level"?, "code"?, "hash"?, "computed_by"?, "detail"?}`.
#[wasm_bindgen]
pub fn verify(receipt: &str, keys: &str, recompute: &str) -> String {
    let mut ks = Keys::new();
    match json::parse(keys.as_bytes()) {
        Ok(Value::Object(m)) => {
            for (host, doc) in m {
                let bytes = match &doc {
                    Value::String(t) => t.as_bytes().to_vec(),
                    other => other.to_jcs(),
                };
                ks.insert_response(&host, 200, Some(&bytes));
            }
        }
        _ => return failure("E_ARGUMENT", "keys: not a JSON object"),
    }
    let mut resolver = MapResolver::new();
    let mut with_resolver = false;
    if !recompute.trim().is_empty() {
        let (Ok(rc), Ok(r)) = (json::parse(recompute.as_bytes()), json::parse(receipt.as_bytes())) else {
            return failure("E_ARGUMENT", "recompute: not JSON");
        };
        let cell = r.get("cell").and_then(|c| Cell::from_value(c).ok());
        let value = rc.get("value").and_then(|v| Scalar::from_json(&v.to_jcs()).ok());
        match (cell, rc.get("descriptor"), value) {
            (Some(cell), Some(d), Some(value)) => {
                resolver.insert_descriptor(cell.fn_hash.as_str(), d.to_jcs());
                resolver.insert_value(cell.id(), value);
                with_resolver = true;
            }
            _ => return failure("E_ARGUMENT", "recompute: needs descriptor and value for the receipt's cell"),
        }
    }
    let out = core_verify(
        receipt.as_bytes(),
        &ks,
        if with_resolver { Some(&resolver as &dyn datumesh_core::Resolver) } else { None },
        None,
    );
    let mut m = vec![("ok".to_string(), Value::Bool(out.ok))];
    if let Some(l) = out.level {
        m.push(("level".into(), s(l.as_str())));
    }
    if let Some(c) = out.code {
        m.push(("code".into(), s(c.as_str())));
    }
    if let Some(h) = &out.hash {
        m.push(("hash".into(), s(h)));
    }
    if let Some(c) = &out.computed_by {
        m.push(("computed_by".into(), s(c)));
    }
    if !out.detail.is_empty() {
        m.push(("detail".into(), s(&out.detail)));
    }
    String::from_utf8_lossy(&Value::Object(m).to_jcs()).into_owned()
}

/// Parses a call (SPEC 5.8, 15.3: `name(arg: value, ...)`, `@alias(...)`, a pin
/// `name@<hash>(...)`, `?asof=`) against `env`, `{"functions": {name: descriptor},
/// "aliases": {alias: name}, "descriptors": [descriptor]}` (each optional). Returns `{"ok":
/// true, "uri", "reference"}` (the cell URI and the reference as JSON) or `{"ok": false,
/// "code", "detail"}`.
#[wasm_bindgen]
pub fn parse_call(text: &str, env: &str) -> String {
    let env = match json::parse(env.as_bytes())
        .map_err(|e| e.to_string())
        .and_then(|v| MapCallEnv::from_value(&v).map_err(|e| e.to_string()))
    {
        Ok(e) => e,
        Err(d) => return failure("E_ARGUMENT", &format!("env: {d}")),
    };
    match formula_parse_call(text, &env) {
        Ok(call) => {
            let r = call.reference();
            let mut reference = vec![
                ("args".to_string(), Value::Object(r.args.iter().map(|(k, a)| (k.clone(), a.to_value())).collect())),
                ("fn".to_string(), s(&r.fn_name)),
                ("ns".to_string(), s(&r.ns)),
            ];
            if let Some(a) = &r.asof {
                reference.push(("asof".into(), s(a)));
            }
            if let Some(h) = &r.fn_hash {
                reference.push(("fn_hash".into(), s(h)));
            }
            String::from_utf8_lossy(
                &Value::Object(vec![
                    ("ok".into(), Value::Bool(true)),
                    ("reference".into(), Value::Object(reference)),
                    ("uri".into(), s(&call.uri())),
                ])
                .to_jcs(),
            )
            .into_owned()
        }
        Err(e) => failure(e.code(), &e.to_string()),
    }
}
