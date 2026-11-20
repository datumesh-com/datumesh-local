//! `calc` (Build 2 Part 3 addition; CONTRACT-build2-part3-calc C1 to C4; D27 to D30): a formula over
//! cells, answered in one call with its lineage.
//!
//! On this machine (`calc`) the formula is canonicalised (15.6, its printed AST without a final
//! LF), defined as the local function `local.invalid/calc_<h>` with no arguments, where `<h>` is
//! the first 16 hex digits of the SHA-256 of the canonical formula (D28), and its cell is evaluated
//! and kept by the engine like any computed cell. The function is bound and catalogued only when
//! the evaluation answers, so a refused formula binds nothing. The evaluation names the function by
//! its hash (`name@<hash>(...)`), so an `asof` before the binding works too. A formula with no
//! inputs is answered, but neither bound nor catalogued (`name` null, D30).
//!
//! With `remote`, the forward mesh's `x_calc` is asked (D27): its answer is shown as it is, and
//! nothing is memoised here.
//!
//! A slice answered by `get` (C4, D29) lists its members here: up to 100 as they are, and above
//! 100 a summary (count, first and last key, and for numbers min, max and mean), the first 100
//! members and the `calc` call that aggregates the slice (`mean(...)` for numbers, else `count(...)`).

use std::cmp::Ordering;
use std::collections::BTreeSet;

use datumesh_core::json::{self, Value};
use datumesh_core::{lexical, Arg, Cell, CellRef, Manifest, Receipt, Slice, Type};
use datumesh_formula::definition::{define_function, Callees as FormulaCallees, Definition};
use datumesh_formula::{Ast, Code, DefineError, Env, Node, ReadArg, Signature};

use crate::fetch::host_of;
use crate::gate::is_unrooted;
use crate::resolver::{call_form, call_form_of, client_error, Callees};
use crate::{Error, Reply, Resolver, Source};

/// The most members a slice answer gives as they are, and the page of a large slice (D29).
pub(crate) const PAGE: usize = 100;

/// The return types a calculation is tried at, in order: a body compiled against a type other than its
/// own is refused with `E_FORMULA_RETURN_TYPE`, and the next type is tried.
const RETURNS: [&str; 6] = ["number", "string", "boolean", "date", "instant", "blob"];

/// The label of a calculation's catalogue entry (D28).
const LABEL: &str = "calc";

/// The `outcome` of a slice answer.
pub(crate) const SLICE: &str = "slice";

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// A string value.
fn js(v: impl Into<String>) -> Value {
    Value::String(v.into())
}

/// A number from its text (a JSON number when the text is one, else the text).
fn number_of(text: &str) -> Value {
    json::parse(text.as_bytes()).unwrap_or_else(|_| Value::String(text.to_string()))
}

/// The formula error as a tool error: its code (`E_FORMULA_*`) and detail.
fn formula_error(e: datumesh_formula::Error) -> Error {
    Error::Client { code: e.code.as_str().to_string(), detail: e.to_string() }
}

/// A definition error as a tool error: its code (the core's or the formula's).
fn define_error(e: DefineError) -> Error {
    Error::Client { code: e.code().to_string(), detail: e.to_string() }
}

/// The local spelling of a formula: `local/<name>` reads `local.invalid/<name>` (D30). A name is
/// only rewritten where it starts, not inside another name.
fn expand_local(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (i, _) in text.match_indices("local/") {
        let starts_name = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/')));
        if starts_name {
            out.push_str(&text[last..i]);
            out.push_str("local.invalid/");
            last = i + "local/".len();
        }
    }
    out.push_str(&text[last..]);
    out
}

/// The error of an evaluation that did not answer a value: its code (the envelope's `error` or
/// `fault`, else its outcome) and its detail.
fn answer_failure(reply: &Reply) -> Error {
    let envelope = reply.envelope().unwrap_or(Value::Null);
    let code = ["error", "fault"]
        .iter()
        .find_map(|k| envelope.get(k).and_then(Value::as_str))
        .map(str::to_string)
        .or_else(|| reply.outcome())
        .unwrap_or_else(|| format!("status {}", reply.status));
    if code == "not_covered" {
        // The engine names no fault for a read of an absent cell: the formula's own code for it.
        return Error::Client {
            code: "E_FORMULA_ABSENT".into(),
            detail: "a cell the formula reads has no value as of the instant".into(),
        };
    }
    let detail = reply.detail.clone().unwrap_or_else(|| code.clone());
    Error::Client { code, detail }
}

/// The names a formula reads (its cells and slices), by a walk of the canonical AST from its root. A
/// class's dispatch is not a calculation here (its class is not in the environment this compiles
/// in), and is refused.
fn read_names(ast: &Ast) -> Result<BTreeSet<String>, Error> {
    let mut names = BTreeSet::new();
    let mut stack = vec![ast.root()];
    while let Some(id) = stack.pop() {
        match ast.node(id) {
            Node::Read { fn_name, args, .. } => {
                names.insert(fn_name.clone());
                for (_, arg) in args {
                    match arg {
                        ReadArg::Expr(n) => stack.push(*n),
                        ReadArg::Range(lo, hi) => stack.extend([*lo, *hi]),
                        ReadArg::Open => {}
                    }
                }
            }
            Node::Dispatch { .. } => {
                return Err(Error::Client {
                    code: "E_CALC_CLASS".into(),
                    detail:
                        "a calc over a class's methods is not supported here: define the function, then calc its cell"
                            .into(),
                })
            }
            Node::Op { args, .. } => stack.extend(args.iter().copied()),
            Node::If { cond, then, els } => stack.extend([*cond, *then, *els]),
            Node::Let { value, body, .. } => stack.extend([*value, *body]),
            Node::Call { args, named, .. } => {
                stack.extend(args.iter().copied());
                stack.extend(named.iter().map(|(_, n)| *n));
            }
            Node::Lambda { body, .. } => stack.push(*body),
            Node::Num { .. } | Node::Str(_) | Node::Bool(_) | Node::Date(_) | Node::Instant(_) | Node::Var(_) => {}
            Node::Agg(_) | Node::Key(_) => {}
        }
    }
    Ok(names)
}

/// A body's unit as a declared unit (7.1): a literal, or `{"of": X, "per": Y}` for one atom over
/// one atom. A unit the signature cannot declare is refused with `E_CALC_UNIT`.
fn declared_unit(text: &str) -> Result<Value, Error> {
    if text == "1" || text.starts_with("a bare number") {
        return Ok(js("1"));
    }
    let cannot = || {
        Error::Client {
        code: "E_CALC_UNIT".into(),
        detail: format!(
            "the result's unit {text} cannot be declared as a function's return (a unit of one atom, or one atom per one atom)"
        ),
    }
    };
    let (mut of, mut per) = (Vec::new(), Vec::new());
    for part in text.split(' ') {
        let (atom, exp) = match part.split_once('^') {
            Some((a, e)) => (a, e.parse::<i64>().ok()),
            None => (part, Some(1)),
        };
        match exp {
            Some(1) => of.push(atom),
            Some(-1) => per.push(atom),
            _ => return Err(cannot()),
        }
    }
    match (of.as_slice(), per.as_slice()) {
        ([a], []) => Ok(js(*a)),
        ([], [b]) => Ok(obj(vec![("of", js("1")), ("per", js(*b))])),
        ([a], [b]) => Ok(obj(vec![("of", js(*a)), ("per", js(*b))])),
        _ => Err(cannot()),
    }
}

/// The type and unit a body returns (15.6), compiled against the functions it reads: the first of
/// [`RETURNS`] it is, since the body is checked against the type declared. A number's unit is the
/// body's own (`None` for the other types).
fn inferred_returns(
    name: &str,
    canonical: &str,
    functions: &[(String, Value)],
) -> Result<(String, Option<String>), Error> {
    for ty in RETURNS {
        let sig = obj(vec![
            ("args", Value::Object(Vec::new())),
            ("order", Value::Array(Vec::new())),
            ("returns", obj(vec![("type", js(ty))])),
        ]);
        let signature = Signature::from_value(&sig).map_err(|e| Error::Define(e.0))?;
        let mut env = Env::new(name, signature);
        for (fn_name, descriptor) in functions {
            env.add_function(fn_name, descriptor).map_err(|e| Error::Define(e.0))?;
        }
        match datumesh_formula::define(canonical, &env) {
            Ok(c) => return Ok((c.returns.ty.as_str().to_string(), c.returns.unit.clone())),
            Err(e) if e.code == Code::ReturnType => continue,
            Err(e) => return Err(formula_error(e)),
        }
    }
    Err(Error::Client {
        code: "E_FORMULA_RETURN_TYPE".into(),
        detail: "the body's type is not one a calculation can return".into(),
    })
}

/// The trust label of a receipt, from the receipt alone (5.5, D29): `relayed` when it names a source,
/// `primary` when the namespace's host signed it with no inputs, else `computed`. The mesh's own
/// answer label (which also knows verified and unverified) is not kept with a receipt.
fn trust_of(r: &Receipt) -> &'static str {
    if r.source().is_some() {
        "relayed"
    } else if r.inputs().is_empty() && r.signer_host() == host_of(&r.cell().ns) {
        "primary"
    } else {
        "computed"
    }
}

/// The key of a member: its open arguments' values, in the order of `open`.
fn key_of(m: &datumesh_store::SliceValue, open: &[String]) -> Vec<datumesh_core::Scalar> {
    open.iter().filter_map(|k| m.cell.args.get(k).and_then(Arg::scalar).cloned()).collect()
}

/// The key as an object: each open argument with its lexical value.
fn key_object(m: &datumesh_store::SliceValue, open: &[String]) -> Value {
    Value::Object(
        open.iter()
            .filter_map(|k| m.cell.args.get(k).and_then(Arg::scalar).map(|v| (k.clone(), js(v.lexical().to_string()))))
            .collect(),
    )
}

/// The unit a calculation's number has, as its answer states it (none for a bare number).
fn unit_of(unit: Option<&String>) -> Option<String> {
    unit.filter(|u| !u.starts_with("a bare number")).cloned()
}

impl Resolver {
    /// The answer to `calc` (C1 to C3): here, or through the forward mesh's `x_calc` when `remote`.
    pub(crate) fn calc(&self, formula: &str, asof: Option<&str>, remote: bool) -> Result<Value, Error> {
        if let Some(a) = asof {
            lexical::check_instant(a, "$.asof").map_err(|e| Error::Request(e.to_string()))?;
        }
        if remote {
            return self.calc_remote(formula, asof);
        }
        self.calc_here(formula, asof)
    }

    /// The local calculation (C2, D28, D30).
    fn calc_here(&self, formula: &str, asof: Option<&str>) -> Result<Value, Error> {
        let s = &self.shared;
        let expanded = expand_local(formula);
        let canonical = datumesh_formula::canonical_ast(&expanded, None).map_err(formula_error)?.print();
        let h = datumesh_core::sha256_hex(canonical.as_bytes())[..16].to_string();
        let name = format!("local.invalid/calc_{h}");
        let now = s.now();
        let def = self.calc_definition(&name, &canonical, &now)?;
        let constant = def.descriptor.inputs.is_empty();
        s.store.put_descriptor(&def.bytes).map_err(|e| Error::Store(e.to_string()))?;
        let pinned = format!("{name}@{}()", def.fn_hash);
        let request = match asof {
            Some(a) => format!("{pinned}?asof={a}"),
            None => pinned,
        };
        let reply = self.get(&request)?;
        let receipt = match reply.receipt() {
            Some(r) if reply.status == 200 => r,
            _ => return Err(answer_failure(&reply)),
        };
        if !constant {
            self.bind(&name, &def.bytes, &def.fn_hash, &now)?;
            s.index.hold_catalogue(&name, LABEL, &canonical, &now)?;
        }
        let mut members = vec![
            ("kind", js("calculation")),
            ("value", js(receipt.value().lexical().to_string())),
            ("type", js(receipt.value().ty().as_str())),
        ];
        if let Some(unit) = unit_of(def.compiled.returns.unit.as_ref()) {
            members.push(("unit", js(unit)));
        }
        members.push(("formula", js(formula)));
        members.push(("canonical", js(&canonical)));
        members.push(("asof", js(asof.unwrap_or(&now))));
        members.push(("lineage", Value::Array(self.calc_lineage(&receipt)?)));
        members.push(("name", if constant { Value::Null } else { js(format!("local/calc_{h}")) }));
        Ok(obj(members))
    }

    /// The definition of `name` (a formula with no arguments), its return type and unit inferred:
    /// the body is compiled against its callees as they are bound now (nothing is bound), and the
    /// result is declared as the function's returns, which the definition then checks (15.2 to 15.10).
    fn calc_definition(&self, name: &str, canonical: &str, now: &str) -> Result<Definition, Error> {
        let s = &self.shared;
        let aliases = self.aliases_at(None).bare();
        let ast = datumesh_formula::canonical_ast(canonical, None).map_err(formula_error)?;
        let callees = Callees::new(s);
        let mut functions = Vec::new();
        for fn_name in read_names(&ast)? {
            let found = FormulaCallees::callee(&callees, &fn_name, now).map_err(Error::Store)?;
            if let Some(c) = found {
                functions.push((fn_name, c.descriptor));
            }
        }
        let (ty, unit) = inferred_returns(name, canonical, &functions)?;
        let mut returns = vec![("type", js(ty.as_str()))];
        if ty == "number" {
            returns.push(("unit", declared_unit(unit.as_deref().unwrap_or("1"))?));
        }
        let sig = obj(vec![
            ("args", Value::Object(Vec::new())),
            ("order", Value::Array(Vec::new())),
            ("returns", obj(returns)),
        ]);
        match define_function(name, canonical, &sig, &aliases, &callees, now) {
            Ok(def) => Ok(def),
            Err(e) => {
                if let Some(input) = callees.untrusted.take() {
                    return Err(Error::UntrustedInput { function: name.to_string(), input });
                }
                Err(define_error(e))
            }
        }
    }

    /// The lineage of an answer (C1): the cells its receipt reads, each with its call, receipt
    /// hash, publisher (the namespace's host) and trust, as `explain` gives them.
    fn calc_lineage(&self, receipt: &Receipt) -> Result<Vec<Value>, Error> {
        let store = &self.shared.store;
        let mut out = Vec::new();
        for input in receipt.inputs() {
            // A slice input names its members (its manifest): each member is a cell the answer read.
            let members = if input.starts_with("slice:") {
                let bytes = store
                    .manifest(input)
                    .map(|b| b.into_owned())
                    .ok_or_else(|| Error::Store(format!("the slice {input} is not held")))?;
                Manifest::from_json(&bytes).map_err(|e| Error::Store(e.to_string()))?.members().to_vec()
            } else {
                vec![input.clone()]
            };
            for hash in members {
                let bytes = store
                    .receipt(&hash)
                    .map(|b| b.into_owned())
                    .ok_or_else(|| Error::Store(format!("the input receipt {hash} is not held")))?;
                let r = Receipt::from_json(&bytes).map_err(|e| Error::Store(e.to_string()))?;
                out.push(obj(vec![
                    ("call", js(call_form(r.cell()))),
                    ("receipt", js(hash)),
                    ("publisher", js(host_of(&r.cell().ns))),
                    ("trust", js(trust_of(&r))),
                ]));
            }
        }
        Ok(out)
    }

    /// The forward mesh's `x_calc` (C3, D27): the answer as the mesh gives it, memoised nowhere.
    fn calc_remote(&self, formula: &str, asof: Option<&str>) -> Result<Value, Error> {
        let s = &self.shared;
        let mesh = s.config.forward_mesh.clone().ok_or_else(|| Error::Client {
            code: "E_NO_FORWARD_MESH".into(),
            detail: "no forward mesh is configured for a remote calc".into(),
        })?;
        s.may_fetch(&mesh).map_err(Error::ForeignUnavailable)?;
        // `x_calc` is an extension member of the mesh document (D27), which the core's document
        // does not keep: it is read from the document as published, as `x_bundles` is.
        s.client.resolve(&mesh).map_err(|e| client_error(&e))?;
        let document = self.peer_url(&format!("https://{mesh}/.well-known/datumesh-mesh.json"), &mesh);
        let (status, bytes) = crate::net::get(&document, s.config.timeout).map_err(Error::ForeignUnavailable)?;
        let url = json::parse(&bytes)
            .ok()
            .filter(|_| status == 200)
            .and_then(|d| d.get("endpoints").and_then(|e| e.get("x_calc")).and_then(Value::as_str).map(str::to_string))
            .ok_or_else(|| Error::Client {
                code: "E_MESH_DOC".into(),
                detail: format!("{mesh} has no x_calc endpoint in its mesh document"),
            })?;
        let mut body = vec![("formula".to_string(), Value::String(formula.to_string()))];
        if let Some(a) = asof {
            body.push(("asof".to_string(), Value::String(a.to_string())));
        }
        let (status, bytes) =
            crate::net::post(&self.peer_url(&url, &mesh), &Value::Object(body).to_jcs(), s.config.timeout)
                .map_err(Error::ForeignUnavailable)?;
        let v = json::parse(&bytes).map_err(|e| Error::Client { code: "E_ENVELOPE".into(), detail: e.to_string() })?;
        if status == 200 && v.get("kind").and_then(Value::as_str) == Some("calculation") {
            return Ok(v);
        }
        let code = ["error", "fault", "outcome"]
            .iter()
            .find_map(|k| v.get(k).and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| format!("status {status}"));
        let detail = v.get("detail").and_then(crate::calculus::literal_text).unwrap_or_else(|| code.clone());
        Err(Error::Client { code, detail })
    }

    /// The catalogue label of a name this machine catalogued (`calc`), when it has one.
    pub(crate) fn catalogue_label(&self, name: &str) -> Result<Option<String>, Error> {
        Ok(self.shared.index.catalogue(name)?.map(|(label, _)| label))
    }

    /// The answer to a slice request (C4, D29), when `rest` is a slice of a stored function: its
    /// members (up to 100 as they are, else a summary, the first 100 and the `calc` call), with the
    /// members of a foreign slice fetched first. `None` when the request is not a slice of a stored
    /// function (the caller gives the mesh's own answer then).
    pub(crate) fn slice_answer(&self, rest: &str) -> Result<Option<Reply>, Error> {
        let s = &self.shared;
        let mesh = self.mesh();
        let Ok(t) = mesh.resolve(rest, true) else { return Ok(None) };
        if t.bound.descriptor.lang != "stored" {
            return Ok(None);
        }
        let cell: &Cell = &t.cell;
        let host = host_of(&cell.ns).to_string();
        let mut fetched = false;
        if !is_unrooted(&host) && !s.index.slice_held(t.rest())? {
            s.may_fetch(&host).map_err(Error::ForeignUnavailable)?;
            self.prefetch(t.rest(), None, false)?;
            fetched = true;
        }
        let now = s.now();
        let asof = t.asof().unwrap_or(&now).to_string();
        let slice_json = crate::calculus::slice_value(
            &CellRef { ns: cell.ns.clone(), fn_name: cell.fn_name.clone(), args: cell.args.clone(), asof: None },
            &cell.fn_hash,
        );
        let slice = Slice::from_value(&slice_json).map_err(|e| Error::Request(format!("slice: {e}")))?;
        let mut values = s.store.slice_values(&slice, &asof).map_err(|e| Error::Store(e.to_string()))?;
        let open: Vec<String> =
            cell.args.iter().filter(|(_, a)| matches!(a, Arg::Slice)).map(|(k, _)| k.clone()).collect();
        values.sort_by(|a, b| key_cmp(&key_of(a, &open), &key_of(b, &open)));
        let call = call_form_of(&cell.ns, &cell.fn_name, &cell.args, None, None);
        let count = values.len();
        let numbers: Option<Vec<f64>> = (count > 0 && values.iter().all(|m| m.value.ty() == Type::Number))
            .then(|| values.iter().map(|m| m.value.lexical().parse::<f64>().ok()).collect::<Option<Vec<f64>>>())
            .flatten();
        let mut members = Vec::new();
        for m in values.iter().take(PAGE) {
            let bytes = s
                .store
                .receipt(&m.version.receipt_hash)
                .map(|b| b.into_owned())
                .ok_or_else(|| Error::Store(format!("the receipt {} is not held", m.version.receipt_hash)))?;
            let r = Receipt::from_json(&bytes).map_err(|e| Error::Store(e.to_string()))?;
            let receipt = json::parse(&bytes).map_err(|e| Error::Store(e.to_string()))?;
            members.push(obj(vec![("outcome", js("ok")), ("trust", js(trust_of(&r))), ("receipt", receipt)]));
        }
        let mut out = vec![("outcome", js(SLICE)), ("slice", js(&call)), ("asof", js(&asof))];
        if count > PAGE {
            let mut summary = vec![
                ("count", number_of(&count.to_string())),
                ("first", key_object(&values[0], &open)),
                ("last", key_object(&values[count - 1], &open)),
            ];
            if let Some(nums) = &numbers {
                let sum: f64 = nums.iter().sum();
                let min = nums.iter().copied().fold(f64::INFINITY, f64::min);
                let max = nums.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                summary.push(("min", number_of(&min.to_string())));
                summary.push(("max", number_of(&max.to_string())));
                summary.push(("mean", number_of(&(sum / count as f64).to_string())));
            }
            let aggregate = if numbers.is_some() { "mean" } else { "count" };
            out.push(("summary", obj(summary)));
            out.push(("members", Value::Array(members)));
            out.push(("calc", obj(vec![("formula", js(format!("{aggregate}({call})")))])));
        } else {
            out.push(("members", Value::Array(members)));
        }
        let source = if fetched { Source::Fetched } else { Source::Local };
        Ok(Some(Reply::new(200, obj(out).to_jcs(), source)))
    }
}

/// The order of two keys (SPEC 15.13): their scalars in turn, then by length.
fn key_cmp(a: &[datumesh_core::Scalar], b: &[datumesh_core::Scalar]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let o = datumesh_formula::eval::cmp_scalar(x, y);
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}
