//! The forward mesh's interval (datumesh-local 0.5.1, L7; DECISIONS-round-protocol D8): the MCP
//! tool `interval`, `{function, keys?, from, to}`, asks the forward mesh's `x_interval`
//! (`GET <origin>/x/interval/<full name>?<keys>&from=…&to=…`), with the function's alias resolved.
//! Its members come back with their receipts. Over [`LISTED`] members the answer summarises them,
//! gives the first [`LISTED`] and the call for the rest. The mesh's errors are passed through.

use datumesh_core::json::{self, Value};
use datumesh_http::client::pct;

use crate::{Error, Resolver};

/// The members listed in full; over this many the answer summarises and gives the call for the rest.
pub const LISTED: usize = 100;

fn num(n: usize) -> Value {
    json::parse(n.to_string().as_bytes()).unwrap_or(Value::Null)
}

/// A member as the answer gives it: the mesh's member, with its value (the receipt's) when it has none.
fn member(m: &Value) -> Value {
    let mut pairs = match m {
        Value::Object(p) => p.clone(),
        _ => Vec::new(),
    };
    if m.get("value").is_none() {
        if let Some(v) = m.get("receipt").and_then(|r| r.get("value")) {
            pairs.push(("value".into(), v.clone()));
        }
    }
    Value::Object(pairs)
}

fn keys_of(m: &Value) -> Value {
    m.get("keys").cloned().unwrap_or(Value::Null)
}

impl Resolver {
    /// The interval of `function` (a full or aliased name) over `keys` and `from`..`to`, from the
    /// forward mesh (see the module documentation).
    pub fn interval(&self, function: &str, keys: &[(String, String)], from: &str, to: &str) -> Result<Value, Error> {
        let s = &self.shared;
        let full = self.aliases_at(None).expand(function);
        if !datumesh_formula::names::is_function_name(&full) {
            return Err(Error::Request(format!("{function}: not a function name")));
        }
        let mesh = s.config.forward_mesh.clone().ok_or_else(|| Error::Client {
            code: "E_NO_FORWARD_MESH".into(),
            detail: "no forward mesh is configured for an interval".into(),
        })?;
        s.may_fetch(&mesh).map_err(Error::ForeignUnavailable)?;
        let mut query: Vec<String> = keys.iter().map(|(k, v)| format!("{}={}", pct(k), pct(v))).collect();
        query.push(format!("from={}", pct(from)));
        query.push(format!("to={}", pct(to)));
        let url = self.peer_url(&format!("https://{mesh}/x/interval/{full}?{}", query.join("&")), &mesh);
        let (status, bytes) = crate::net::get(&url, s.config.timeout).map_err(Error::ForeignUnavailable)?;
        let v = json::parse(&bytes).map_err(|e| Error::Client { code: "E_ENVELOPE".into(), detail: e.to_string() })?;
        if status != 200 {
            let code = ["error", "outcome"]
                .iter()
                .find_map(|k| v.get(k).and_then(Value::as_str))
                .map(str::to_string)
                .unwrap_or_else(|| format!("status {status}"));
            return Err(Error::Client { code, detail: format!("{url}: status {status}") });
        }
        let Some(Value::Array(members)) = v.get("members") else {
            return Err(Error::Client { code: "E_ENVELOPE".into(), detail: format!("{url}: no members") });
        };
        let mut out = vec![
            ("function".to_string(), Value::String(full.clone())),
            (
                "keys".to_string(),
                Value::Object(keys.iter().map(|(k, x)| (k.clone(), Value::String(x.clone()))).collect()),
            ),
            ("from".to_string(), Value::String(from.to_string())),
            ("to".to_string(), Value::String(to.to_string())),
            ("count".to_string(), num(members.len())),
            ("members".to_string(), Value::Array(members.iter().take(LISTED).map(member).collect())),
        ];
        if members.len() > LISTED {
            out.push(("summary".to_string(), summary(members)));
            // The call for the rest: the members after the listed ones, to the same end.
            let next_from = members
                .get(LISTED)
                .and_then(|m| keys_of(m).get("date").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| from.to_string());
            let args = Value::Object(vec![
                ("function".into(), Value::String(full)),
                (
                    "keys".into(),
                    Value::Object(keys.iter().map(|(k, x)| (k.clone(), Value::String(x.clone()))).collect()),
                ),
                ("from".into(), Value::String(next_from)),
                ("to".into(), Value::String(to.to_string())),
            ]);
            out.push((
                "rest".to_string(),
                Value::Object(vec![("tool".into(), Value::String("interval".into())), ("arguments".into(), args)]),
            ));
        }
        Ok(Value::Object(out))
    }
}

/// The summary of more than [`LISTED`] members: the count, the first and last keys and, for
/// numbers, the minimum, maximum and mean.
fn summary(members: &[Value]) -> Value {
    let values: Vec<f64> = members
        .iter()
        .filter_map(|m| m.get("receipt").and_then(|r| r.get("value")).and_then(Value::as_str))
        .filter_map(|t| t.parse::<f64>().ok())
        .collect();
    let stat = |x: Option<f64>| match x {
        Some(x) => json::parse(format!("{x}").as_bytes()).unwrap_or(Value::Null),
        None => Value::Null,
    };
    let min = values.iter().copied().reduce(f64::min);
    let max = values.iter().copied().reduce(f64::max);
    let mean = (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64);
    Value::Object(vec![
        ("count".into(), num(members.len())),
        ("first".into(), members.first().map_or(Value::Null, keys_of)),
        ("last".into(), members.last().map_or(Value::Null, keys_of)),
        ("min".into(), stat(min)),
        ("max".into(), stat(max)),
        ("mean".into(), stat(mean)),
    ])
}
