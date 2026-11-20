//! The MCP verb `whatif` (CONTRACT-build2-part3-calculus E6): the spreadsheet what-if, a scenario
//! and never a fact. A cell of a local namespace is evaluated here, by the shared engine
//! (`Engine::whatif`, E1 and E2). Any other cell is asked of its mesh's `whatif` endpoint (spec-0.6.0
//! 5.9), and the answer is checked against the request with core's `check_answer`. Nothing is
//! stored, signed or memoised in either case, and the answer carries no receipt.

use datumesh_core::json::{self, Value};
use datumesh_core::{
    check_answer, lexical, Answer, Arg, Args, Cell, CellRef, Keys, Request, Scalar, Scenario, Slice, Type,
};
use datumesh_engine::{EngineError, Overrides, Solve};

use crate::fetch::{host_of, END};
use crate::gate::is_unrooted;
use crate::resolver::{call_form, call_form_of, client_error};
use crate::{Error, Resolver};

/// An override of a `whatif` request as the tool is given it: a cell with a plain literal, or a
/// stored slice with members added (cell and literal) and removed (cells).
#[derive(Clone, Debug)]
pub(crate) enum OverrideArg {
    /// `cell` answers `value`.
    Value {
        /// The cell (a call, or a cell URI).
        cell: String,
        /// The plain literal, typed by the cell's return type.
        value: Value,
    },
    /// The slice `slice` changes: `add` (cell and literal) and `remove` (cells).
    Slice {
        /// The slice (a call whose open arguments are left open).
        slice: String,
        /// The added members.
        add: Vec<(String, Value)>,
        /// The removed members.
        remove: Vec<String>,
    },
}

/// The solve of a `whatif` request: find `vary` (a number cell) in `[lo, hi]` at which `target`
/// (a number cell) equals `equals`.
#[derive(Clone, Debug)]
pub(crate) struct SolveArg {
    /// The cell to vary.
    pub(crate) vary: String,
    /// The cell whose value must equal `equals`.
    pub(crate) target: String,
    /// The value the target must have.
    pub(crate) equals: Value,
    /// The interval's lower end.
    pub(crate) lo: Value,
    /// The interval's upper end.
    pub(crate) hi: Value,
}

/// A `whatif` request as the tool reads it.
#[derive(Clone, Debug)]
pub(crate) struct WhatifArgs {
    /// The cell asked for: a call (or a cell URI).
    pub(crate) request: String,
    /// The overrides, in the order given.
    pub(crate) overrides: Vec<OverrideArg>,
    /// The instant, when given.
    pub(crate) asof: Option<String>,
    /// The solve, when given.
    pub(crate) solve: Option<SolveArg>,
}

/// A cell with its reference, its cell and its function's return type.
#[derive(Clone, Debug)]
struct Resolved {
    reference: CellRef,
    cell: Cell,
    ty: Type,
}

/// A slice with its reference and its function's hash (a slice is not a cell).
#[derive(Clone, Debug)]
struct SliceRef {
    reference: CellRef,
    fn_hash: String,
}

/// A resolved override, with its typed values.
#[derive(Clone, Debug)]
enum Ov {
    Value { cell: Resolved, value: Scalar },
    Slice { slice: SliceRef, id: String, add: Vec<(Resolved, Scalar)>, remove: Vec<Resolved> },
}

impl Ov {
    /// The id the canonical order sorts by (5.9): the cell's, or the slice's.
    fn id(&self) -> String {
        match self {
            Ov::Value { cell, .. } => cell.cell.id(),
            Ov::Slice { id, .. } => id.clone(),
        }
    }
}

/// The solve of a resolved request.
struct SolveReq {
    vary: Resolved,
    target: Resolved,
    equals: Scalar,
    lo: Scalar,
    hi: Scalar,
}

/// The literal of a plain JSON value (a string, a number or a boolean), as its text.
pub(crate) fn literal_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        other => other.as_number_text().map(str::to_string),
    }
}

/// A plain literal typed by `ty` (E_CANON_TYPE when it does not fit).
fn typed_literal(ty: Type, v: &Value) -> Result<Scalar, Error> {
    let text = literal_text(v).ok_or_else(|| Error::Client {
        code: "E_CANON_TYPE".into(),
        detail: "a value is a plain literal: a string, a number or a boolean".into(),
    })?;
    Scalar::from_lexical(ty, &text)
        .map_err(|e| Error::Client { code: "E_CANON_TYPE".into(), detail: format!("{text}: {e}") })
}

/// The lexical form of a scalar, as the tool's values are written.
fn lexical_value(s: &Scalar) -> Value {
    match s {
        Scalar::Boolean(b) => Value::Bool(*b),
        other => Value::String(other.lexical().to_string()),
    }
}

/// A slice's JSON form (5.6, 5.9): its open arguments are `"*"`. Its id is the slice's.
pub(crate) fn slice_value(c: &CellRef, fn_hash: &str) -> Value {
    let args = c
        .args
        .iter()
        .map(|(k, a)| {
            let v = match a {
                Arg::Value(s) => s.to_value(),
                Arg::Slice => Value::String("*".into()),
            };
            (k.clone(), v)
        })
        .collect();
    Value::Object(vec![
        ("args".into(), Value::Object(args)),
        ("fn".into(), Value::String(c.fn_name.clone())),
        ("fn_hash".into(), Value::String(fn_hash.to_string())),
        ("ns".into(), Value::String(c.ns.clone())),
    ])
}

/// A slice's id, from its JSON form.
fn slice_id(c: &CellRef, fn_hash: &str) -> Result<String, Error> {
    Slice::from_value(&slice_value(c, fn_hash))
        .map(|s| s.id().to_string())
        .map_err(|e| Error::Request(format!("slice: {e}")))
}

/// The overrides in the envelope's form (5.9), in the canonical order given.
fn core_overrides(ovs: &[Ov]) -> Result<Value, Error> {
    let mut items = Vec::new();
    for o in ovs {
        items.push(match o {
            Ov::Value { cell, value } => {
                Value::Object(vec![("cell".into(), cell.cell.to_value()), ("value".into(), value.to_value())])
            }
            Ov::Slice { slice, add, remove, .. } => {
                let slice_json = Slice::from_value(&slice_value(&slice.reference, &slice.fn_hash))
                    .map_err(|e| Error::Request(format!("slice: {e}")))?;
                Value::Object(vec![
                    (
                        "add".into(),
                        Value::Array(
                            add.iter()
                                .map(|(c, v)| {
                                    Value::Object(vec![
                                        ("cell".into(), c.cell.to_value()),
                                        ("value".into(), v.to_value()),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                    ("remove".into(), Value::Array(remove.iter().map(|c| c.cell.to_value()).collect())),
                    ("slice".into(), slice_json.to_value()),
                ])
            }
        });
    }
    Ok(Value::Array(items))
}

/// The error of an engine refusal, as the tool reports it: `not_covered` with its fault, or the
/// override's code.
fn engine_error(e: EngineError) -> Error {
    match e {
        EngineError::Override(code) => Error::Client { code: code.clone(), detail: code },
        EngineError::NotCovered { fault } => {
            Error::Client { code: "not_covered".into(), detail: fault.map(|f| f.to_string()).unwrap_or_default() }
        }
        EngineError::WhatifNoSolution => {
            Error::Client { code: "not_covered".into(), detail: "E_WHATIF_NO_SOLUTION".into() }
        }
        other => Error::Client { code: "E_EVAL_UNAVAILABLE".into(), detail: other.to_string() },
    }
}

impl Resolver {
    /// The reference of a call, its function's hash as bound at `asof` (or now), and its return type.
    fn resolve_fn(&self, text: &str, asof: Option<&str>) -> Result<(CellRef, String, Type), Error> {
        let call = self.call_of(text, None)?;
        let name = format!("{}/{}", call.cell.ns, call.cell.fn_name);
        let fn_hash = match call.fn_hash.clone() {
            Some(h) => h,
            None => self
                .shared
                .store
                .resolve_binding(&name, asof.unwrap_or(END))
                .map_err(|e| Error::Store(e.to_string()))?
                .map(|b| b.fn_hash)
                .ok_or_else(|| Error::Request(format!("{name} is not bound as of the request")))?,
        };
        let bytes = self
            .shared
            .store
            .descriptor(&fn_hash)
            .ok_or_else(|| Error::Store(format!("descriptor {fn_hash} is not stored")))?;
        let d = datumesh_core::FunctionDescriptor::from_json(&bytes).map_err(|e| Error::Store(e.to_string()))?;
        Ok((call.cell, fn_hash, d.signature.returns.ty))
    }

    /// A cell (every argument a single value) of a call.
    fn resolve_cell(&self, text: &str, asof: Option<&str>) -> Result<Resolved, Error> {
        let (reference, fn_hash, ty) = self.resolve_fn(text, asof)?;
        let cell = reference.resolve(&fn_hash).map_err(|e| Error::Request(e.to_string()))?;
        Ok(Resolved { reference, cell, ty })
    }

    /// The scenario of a `whatif` request (E6), as [`scenario_result`] gives it.
    pub(crate) fn whatif(&self, a: &WhatifArgs) -> Result<Value, Error> {
        if a.overrides.is_empty() && a.solve.is_none() {
            return Err(Error::Request("whatif: overrides, or solve, or both".into()));
        }
        if let Some(t) = &a.asof {
            lexical::check_instant(t, "$.asof").map_err(|e| Error::Request(e.to_string()))?;
        }
        let asof = a.asof.as_deref();
        let target = self.resolve_cell(&a.request, asof)?;
        let mut ovs = Vec::new();
        for o in &a.overrides {
            ovs.push(match o {
                OverrideArg::Value { cell, value } => {
                    let c = self.resolve_cell(cell, asof)?;
                    let v = typed_literal(c.ty, value)?;
                    Ov::Value { cell: c, value: v }
                }
                OverrideArg::Slice { slice, add, remove } => {
                    let (reference, fn_hash, _) = self.resolve_fn(slice, asof)?;
                    let id = slice_id(&reference, &fn_hash)?;
                    let mut members = Vec::new();
                    for (c, v) in add {
                        let r = self.resolve_cell(c, asof)?;
                        let value = typed_literal(r.ty, v)?;
                        members.push((r, value));
                    }
                    let mut gone = Vec::new();
                    for c in remove {
                        gone.push(self.resolve_cell(c, asof)?);
                    }
                    Ov::Slice { slice: SliceRef { reference, fn_hash }, id, add: members, remove: gone }
                }
            });
        }
        ovs.sort_by_key(Ov::id);
        let solve = match &a.solve {
            None => None,
            Some(s) => Some(SolveReq {
                vary: self.resolve_cell(&s.vary, asof)?,
                target: self.resolve_cell(&s.target, asof)?,
                equals: typed_literal(Type::Number, &s.equals)?,
                lo: typed_literal(Type::Number, &s.lo)?,
                hi: typed_literal(Type::Number, &s.hi)?,
            }),
        };
        if is_unrooted(host_of(&target.cell.ns)) {
            self.whatif_here(&target, asof, &ovs, solve.as_ref())
        } else {
            self.whatif_remote(&target, asof, &ovs, solve.as_ref())
        }
    }

    /// A scenario of a local cell, evaluated by the shared engine: nothing is written.
    fn whatif_here(
        &self,
        target: &Resolved,
        asof: Option<&str>,
        ovs: &[Ov],
        solve: Option<&SolveReq>,
    ) -> Result<Value, Error> {
        let mut overrides = Overrides::new();
        for o in ovs {
            overrides = match o {
                Ov::Value { cell, value } => overrides.value(cell.reference.clone(), value.clone()),
                Ov::Slice { slice, add, remove, .. } => overrides.slice(
                    slice.reference.clone(),
                    add.iter().map(|(c, v)| (c.reference.clone(), v.clone())).collect(),
                    remove.iter().map(|c| c.reference.clone()).collect(),
                ),
            };
        }
        let solve = solve.map(|s| {
            Solve::new(
                s.vary.reference.clone(),
                s.target.reference.clone(),
                s.equals.clone(),
                s.lo.clone(),
                s.hi.clone(),
            )
        });
        let engine = self.mesh().engine().clone();
        let answer = engine.whatif(&target.reference, asof, &overrides, solve.as_ref()).map_err(engine_error)?;
        let mut sc = Scenario::new(answer.cell.clone(), answer.value.clone(), &answer.asof, &core_overrides(ovs)?)
            .map_err(|e| Error::Client { code: "E_ENVELOPE".into(), detail: e.to_string() })?;
        if let Some((vary, value)) = &answer.solved {
            sc = sc
                .with_solved(vary.clone(), value.clone())
                .map_err(|e| Error::Client { code: "E_ENVELOPE".into(), detail: e.to_string() })?;
        }
        let envelope = Answer::scenario(sc).to_value();
        scenario_result(&envelope.get("scenario").cloned().unwrap_or(Value::Null))
    }

    /// A scenario of a foreign cell: the answering mesh's `whatif` endpoint is asked (its URL from
    /// its mesh document), and the answer is checked against the request.
    fn whatif_remote(
        &self,
        target: &Resolved,
        asof: Option<&str>,
        ovs: &[Ov],
        solve: Option<&SolveReq>,
    ) -> Result<Value, Error> {
        let s = &self.shared;
        let host = host_of(&target.cell.ns).to_string();
        s.may_fetch(&host).map_err(Error::ForeignUnavailable)?;
        let doc = s.client.resolve(&host).map_err(|e| client_error(&e))?;
        let url = doc.url("whatif", &[]).ok_or_else(|| Error::Client {
            code: "E_MESH_DOC".into(),
            detail: format!("{host} has no whatif endpoint in its mesh document"),
        })?;
        let body = request_body(target, asof, ovs, solve);
        let (status, bytes) = crate::net::post(&self.peer_url(&url, &host), &body.to_jcs(), s.config.timeout)
            .map_err(Error::ForeignUnavailable)?;
        // Every answer, errors included, is checked against the request (spec-0.6.0 5.9, as core
        // checks it): a solver code is valid only for a request with solve.
        let vary_id = solve.map(|s| s.vary.cell.id());
        let request = Request::new(&target.cell.id(), asof)
            .map_err(|e| Error::Request(e.to_string()))?
            .with_whatif(&core_overrides(ovs)?, vary_id.as_deref())
            .map_err(|e| Error::Request(e.to_string()))?;
        check_answer(status, &bytes, &Keys::new(), &request).map_err(|e| Error::Client {
            code: "E_ENVELOPE".into(),
            detail: format!("the mesh's answer is not the answer asked for: {e}"),
        })?;
        let envelope =
            json::parse(&bytes).map_err(|e| Error::Client { code: "E_ENVELOPE".into(), detail: e.to_string() })?;
        let outcome = envelope.get("outcome").and_then(Value::as_str).unwrap_or("").to_string();
        if outcome == "scenario" {
            return scenario_result(envelope.get("scenario").unwrap_or(&Value::Null));
        }
        // A checked error answer is reported with its code. It carries no value to present as a fact.
        let code = envelope.get("error").and_then(Value::as_str).unwrap_or(&outcome).to_string();
        let detail = match outcome.as_str() {
            "not_covered" => envelope.get("fault").and_then(Value::as_str).unwrap_or_default().to_string(),
            _ => envelope.get("detail").and_then(literal_text).unwrap_or_default(),
        };
        Err(Error::Client { code, detail })
    }

    /// The URL of a mesh endpoint as this resolver reaches it: a configured peer's origin for the
    /// host, as the http client makes of `https://<host>/...`.
    pub(crate) fn peer_url(&self, url: &str, host: &str) -> String {
        match (url.strip_prefix(&format!("https://{host}")), self.shared.config.peers.get(host)) {
            (Some(rest), Some(origin)) => format!("{origin}{rest}"),
            _ => url.to_string(),
        }
    }
}

/// The request body of 5.9 for a foreign cell: calls in full, values typed, the solve's numbers
/// as lexical forms.
fn request_body(target: &Resolved, asof: Option<&str>, ovs: &[Ov], solve: Option<&SolveReq>) -> Value {
    let mut m = vec![("cell".to_string(), Value::String(call_form(&target.cell)))];
    if let Some(a) = asof {
        m.push(("asof".into(), Value::String(a.to_string())));
    }
    let overrides = ovs
        .iter()
        .map(|o| match o {
            Ov::Value { cell, value } => Value::Object(vec![
                ("cell".into(), Value::String(call_form(&cell.cell))),
                ("value".into(), value.to_value()),
            ]),
            Ov::Slice { slice, add, remove, .. } => Value::Object(vec![
                (
                    "add".into(),
                    Value::Array(
                        add.iter()
                            .map(|(c, v)| {
                                Value::Object(vec![
                                    ("cell".into(), Value::String(call_form(&c.cell))),
                                    ("value".into(), v.to_value()),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("remove".into(), Value::Array(remove.iter().map(|c| Value::String(call_form(&c.cell))).collect())),
                ("slice".into(), Value::String(slice_call(&slice.reference))),
            ]),
        })
        .collect();
    m.push(("overrides".into(), Value::Array(overrides)));
    if let Some(s) = solve {
        m.push((
            "solve".into(),
            Value::Object(vec![
                ("equals".into(), lexical_value(&s.equals)),
                ("hi".into(), lexical_value(&s.hi)),
                ("lo".into(), lexical_value(&s.lo)),
                ("target".into(), Value::String(call_form(&s.target.cell))),
                ("vary".into(), Value::String(call_form(&s.vary.cell))),
            ]),
        ));
    }
    Value::Object(m)
}

/// A slice's call form: its open arguments as `*`, no pin.
fn slice_call(reference: &CellRef) -> String {
    let args: Args = reference.args.clone();
    call_form_of(&reference.ns, &reference.fn_name, &args, None, None)
}

/// The scenario member of an answer, as the tool reports it (E6): the call, its type and value,
/// the asof, the overrides and, when solved, the solved cell and value. Values are lexical.
pub(crate) fn scenario_result(sc: &Value) -> Result<Value, Error> {
    let envelope_err = |e: datumesh_core::Error| Error::Client { code: "E_ENVELOPE".into(), detail: e.to_string() };
    let cell = Cell::from_value(sc.get("cell").unwrap_or(&Value::Null)).map_err(envelope_err)?;
    let mut overrides = Vec::new();
    if let Some(Value::Array(items)) = sc.get("overrides") {
        for o in items {
            overrides.push(match o.get("slice") {
                Some(slice) => {
                    let add: Vec<Value> = match o.get("add") {
                        Some(Value::Array(a)) => a.iter().map(|m| member(m.get("cell"), m.get("value"))).collect(),
                        _ => Vec::new(),
                    };
                    let remove: Vec<Value> = match o.get("remove") {
                        Some(Value::Array(a)) => a.iter().map(|c| Value::String(cell_call(Some(c)))).collect(),
                        _ => Vec::new(),
                    };
                    Value::Object(vec![
                        ("add".into(), Value::Array(add)),
                        ("remove".into(), Value::Array(remove)),
                        ("slice".into(), Value::String(slice_form(slice))),
                    ])
                }
                None => member(o.get("cell"), o.get("value")),
            });
        }
    }
    let mut m = vec![
        ("asof".to_string(), sc.get("asof").cloned().unwrap_or(Value::Null)),
        ("call".to_string(), Value::String(call_form(&cell))),
        ("outcome".to_string(), Value::String("scenario".into())),
        ("overrides".to_string(), Value::Array(overrides)),
        ("type".to_string(), sc.get("type").cloned().unwrap_or(Value::Null)),
        ("value".to_string(), lexical_of(sc.get("value"))),
    ];
    if let Some(s) = sc.get("solved") {
        let vary = Cell::from_value(s.get("vary").unwrap_or(&Value::Null)).map_err(envelope_err)?;
        m.push((
            "solved".into(),
            Value::Object(vec![
                ("value".into(), lexical_of(s.get("value"))),
                ("vary".into(), Value::String(call_form(&vary))),
            ]),
        ));
    }
    Ok(Value::Object(m))
}

/// A `{cell, value}` member of an envelope's overrides, as the tool reports it.
fn member(cell: Option<&Value>, value: Option<&Value>) -> Value {
    Value::Object(vec![("cell".into(), Value::String(cell_call(cell))), ("value".into(), lexical_of(value))])
}

/// The lexical value of an envelope's value: a typed object's `value`, or the raw value.
fn lexical_of(v: Option<&Value>) -> Value {
    match v {
        Some(Value::Object(_)) => v.and_then(|x| x.get("value")).cloned().unwrap_or(Value::Null),
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

/// The call form of a cell object of an envelope (empty when it is not one).
fn cell_call(v: Option<&Value>) -> String {
    v.and_then(|c| Cell::from_value(c).ok()).map(|c| call_form(&c)).unwrap_or_default()
}

/// The call form of a slice object of an envelope: `ns/fn(arg: literal, ..., arg: *)`, sorted.
fn slice_form(slice: &Value) -> String {
    let ns = slice.get("ns").and_then(Value::as_str).unwrap_or_default();
    let fname = slice.get("fn").and_then(Value::as_str).unwrap_or_default();
    let mut parts: Vec<String> = match slice.get("args") {
        Some(Value::Object(args)) => args
            .iter()
            .map(|(k, a)| {
                let lit = match a {
                    Value::String(s) if s == "*" => "*".to_string(),
                    Value::Object(_) => {
                        let ty = a.get("type").and_then(Value::as_str).unwrap_or_default();
                        let v = a.get("value").and_then(literal_text).unwrap_or_default();
                        if ty == "string" {
                            format!("\"{}\"", v.replace('"', "\"\""))
                        } else {
                            v
                        }
                    }
                    _ => "*".to_string(),
                };
                format!("{k}: {lit}")
            })
            .collect(),
        _ => Vec::new(),
    };
    parts.sort();
    format!("{ns}/{fname}({})", parts.join(", "))
}
