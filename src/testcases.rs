//! The tests of a defined function (CONTRACT-build2-part3-calculus E5 and E6, the local side).
//!
//! A test is kept in the format of E5 with full names, as `define` normalises it:
//! `{"test": <name>, "call": <call>, "asof"?: <instant>, "inputs"?: {<call>: {"type", "value"}},
//! "expect": {"type", "value"} | {"fault": <code>}}`. The kept tests travel with the function: in
//! the rooting bundle and the publish bundle, where their names are rewritten to the namespace
//! ([`rooted_test`]).
//!
//! The public rule (E5, the mesh's, run here before a publish): every derived case of the
//! function's body is covered by a test, and every branch arm is taken by some test. The
//! evaluation of the tests is the formula evaluator's own, with the arms recorded
//! ([`datumesh_formula::coverage`]). A test's inputs are the ones it gives, or else the real cells
//! at its asof, which are fetched when a read finds none.

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::sync::Arc;

use datumesh_core::json::{self, Value};
use datumesh_core::{lexical, Arg, Args, Cell, FunctionDescriptor, Scalar, Type};
use datumesh_formula::coverage::{self, BranchKind, Coverage, EvalObserver};
use datumesh_formula::eval::{
    self, Args as FormulaArgs, Binding, CellKey, ClassBinding, SliceKey, Stop, Version, World,
};
use datumesh_formula::memory::MemoryWorld;
use datumesh_formula::{check_descriptor, Code, Compiled, Env, Extent, Function};

use crate::fetch::{host_of, name_of, END};
use crate::resolver::call_form;
use crate::{Error, Resolver};

/// A function whose tests miss something (the public rule, E5): the derived cases and the branch
/// arms no test covers. `function` is the name the report is about (the rooted one, for a publish).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingTests {
    /// The function's name.
    pub function: String,
    /// What is missing, as text: each derived case not covered, each branch arm not taken.
    pub cases: Vec<String>,
    /// The branch arms of the function.
    pub branches_total: usize,
    /// The branch arms some test takes.
    pub branches_covered: usize,
}

/// A function of the closure of a body: its name, its descriptor and when its binding began.
struct Bound {
    name: String,
    value: Value,
    start: String,
}

/// The expectation of a test.
enum Expect {
    Value,
    Fault(String),
}

/// A stored test, read back: its call, its asof, its given inputs and its expectation.
struct Spec {
    call: datumesh_formula::call::Call,
    asof: String,
    inputs: Vec<(String, Scalar)>,
    /// A slice input's members (datumesh-local 0.5.2, item 14): the slice's call, and each member's
    /// call with its value.
    slices: Vec<(String, Vec<(String, Scalar)>)>,
    expect: Expect,
}

/// A slice input's members, when the input is `{"members": {<call>: <value>, ...}}` and nothing else
/// (datumesh-local 0.5.2, item 14): the map, else `None`.
fn members_of(v: &Value) -> Option<&Vec<(String, Value)>> {
    match v {
        Value::Object(m) if m.len() == 1 && m[0].0 == "members" => match &m[0].1 {
            Value::Object(members) => Some(members),
            _ => None,
        },
        _ => None,
    }
}

/// `text` as a plain literal: a JSON string, number or boolean, as its text.
fn literal_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        other => other.as_number_text().map(str::to_string),
    }
}

/// Whether a number's lexical form is zero.
fn is_zero(lex: &str) -> bool {
    let digits: String = lex.chars().filter(char::is_ascii_digit).collect();
    !digits.is_empty() && digits.bytes().all(|b| b == b'0')
}

/// Whether a number's lexical form is below zero.
fn is_negative(lex: &str) -> bool {
    lex.starts_with('-') && !is_zero(lex)
}

/// The value of an argument (a single value), as its lexical form.
fn lexical_of(a: &Arg) -> Option<&str> {
    match a {
        Arg::Value(v) => Some(v.lexical()),
        Arg::Slice => None,
    }
}

/// The `expect` member of a normalised test, as the value or the fault it states.
fn expect_of(e: &Value) -> Result<Expect, Error> {
    let ty = e.get("type").and_then(Value::as_str).and_then(Type::from_name);
    if let Some(f) = e.get("fault").and_then(Value::as_str) {
        return Ok(Expect::Fault(f.to_string()));
    }
    let (Some(ty), Some(text)) = (ty, e.get("value").and_then(literal_text)) else {
        return Err(Error::Define("a test's expect is a typed value or a fault".into()));
    };
    Scalar::from_lexical(ty, &text).map(|_| Expect::Value).map_err(|e| Error::Define(e.to_string()))
}

/// The real cell of `cell` at `asof` as a receipt (through the resolver, fetched when allowed),
/// or `None` when it has no value.
struct Probe<'a> {
    world: MemoryWorld,
    resolver: &'a Resolver,
    asof: String,
    /// The reads already tried against the resolver (once each).
    tried: BTreeSet<String>,
    /// The cells read that have no value: their names.
    misses: BTreeSet<String>,
    /// The slices read with no members: their names.
    empty: BTreeSet<String>,
}

impl Probe<'_> {
    fn fetch(&mut self, cell: &CellKey) -> Option<Value> {
        let binding = World::function(&mut self.world, &cell.name, None).ok()??;
        let (ns, fn_name) = cell.name.rsplit_once('/')?;
        let args = cell.args.iter().map(|(k, v)| (k.clone(), Arg::Value(v.clone()))).collect();
        let c = Cell { ns: ns.to_string(), fn_name: fn_name.to_string(), fn_hash: binding.function.hash.clone(), args };
        let reply = self.resolver.get(&format!("{}?asof={}", call_form(&c), self.asof)).ok()?;
        if reply.status != 200 {
            return None;
        }
        reply.envelope()?.get("receipt").cloned()
    }
}

impl World for Probe<'_> {
    type Error = Infallible;

    fn function(&mut self, name: &str, at: Option<&str>) -> Result<Option<Binding>, Infallible> {
        World::function(&mut self.world, name, at)
    }

    fn read(&mut self, cell: &CellKey, at: Option<&str>) -> Result<Option<Version>, Stop<Infallible>> {
        if let Some(v) = World::read(&mut self.world, cell, at)? {
            return Ok(Some(v));
        }
        let key = format!("{}{:?}", cell.name, cell.args);
        if self.tried.insert(key) {
            if let Some(receipt) = self.fetch(cell) {
                if self.world.add_receipt(&receipt).is_ok() {
                    return World::read(&mut self.world, cell, at);
                }
            }
        }
        self.misses.insert(cell.name.clone());
        Ok(None)
    }

    fn members(&mut self, slice: &SliceKey) -> Result<Vec<(FormulaArgs, Version)>, Stop<Infallible>> {
        let m = World::members(&mut self.world, slice)?;
        if m.is_empty() {
            self.empty.insert(slice.name.clone());
        }
        Ok(m)
    }

    fn versions(&mut self, cell: &CellKey, from: &str, to: &str) -> Result<Vec<Version>, Stop<Infallible>> {
        World::versions(&mut self.world, cell, from, to)
    }

    fn charge(&mut self, n: u64) -> Result<(), Code> {
        World::charge(&mut self.world, n)
    }

    fn asof(&mut self) -> Result<String, Stop<Infallible>> {
        World::asof(&mut self.world)
    }

    fn blob(&mut self, sha256: &str, length: u64) -> Result<Option<Vec<u8>>, Stop<Infallible>> {
        World::blob(&mut self.world, sha256, length)
    }

    fn rebound(&mut self, name: &str, binding: &Binding) -> Result<Option<Arc<Compiled>>, Stop<Infallible>> {
        World::rebound(&mut self.world, name, binding)
    }

    fn class(&mut self, name: &str, at: Option<&str>) -> Result<Option<ClassBinding>, Infallible> {
        World::class(&mut self.world, name, at)
    }

    fn function_by_hash(&mut self, fn_hash: &str) -> Result<Option<Arc<Function>>, Infallible> {
        World::function_by_hash(&mut self.world, fn_hash)
    }

    fn read_pinned(&mut self, fn_hash: &str, cell: &CellKey) -> Result<Option<Version>, Stop<Infallible>> {
        World::read_pinned(&mut self.world, fn_hash, cell)
    }
}

/// A test's arguments as the evaluator takes them: single values (a slice is not a test's argument).
fn scalar_args(args: &Args) -> Result<FormulaArgs, Error> {
    args.iter()
        .map(|(k, a)| match a {
            Arg::Value(v) => Ok((k.clone(), v.clone())),
            Arg::Slice => Err(Error::Request(format!("{k}: a test's arguments are single values"))),
        })
        .collect()
}

/// The E5 test's `call` and `inputs` with their names rewritten by `map` (from, to), and its
/// `test` name rewritten too: the test as the rooted bundle carries it.
pub(crate) fn rooted_test(t: &Value, map: &BTreeMap<String, String>) -> Value {
    let rename = |s: &str| crate::rooting::rename_calls(s, map);
    let mut m: Vec<(String, Value)> = Vec::new();
    if let Value::Object(members) = t {
        for (k, v) in members {
            let v = match (k.as_str(), v) {
                ("test", Value::String(n)) => Value::String(map.get(n).cloned().unwrap_or_else(|| n.clone())),
                ("call", Value::String(c)) => Value::String(rename(c)),
                ("inputs", Value::Object(inputs)) => {
                    Value::Object(inputs.iter().map(|(call, typed)| (rename(call), typed.clone())).collect())
                }
                _ => v.clone(),
            };
            m.push((k.clone(), v));
        }
    }
    Value::Object(m)
}

impl Resolver {
    /// The tests kept for the function `fn_hash`, normalised (E5), in order.
    pub(crate) fn tests_of(&self, fn_hash: &str) -> Result<Vec<Value>, Error> {
        self.shared
            .index
            .tests(fn_hash)?
            .iter()
            .map(|bytes| json::parse(bytes).map_err(|e| Error::Store(format!("a kept test: {e}"))))
            .collect()
    }

    /// The return type of the function `name`, as it is bound now.
    fn returns_of(&self, name: &str) -> Result<Type, Error> {
        let b = self
            .shared
            .store
            .resolve_binding(name, END)
            .map_err(|e| Error::Store(e.to_string()))?
            .ok_or_else(|| Error::Request(format!("{name} is not bound here")))?;
        let bytes =
            self.shared.store.descriptor(&b.fn_hash).ok_or_else(|| Error::Store("a descriptor is missing".into()))?;
        let d = FunctionDescriptor::from_json(&bytes).map_err(|e| Error::Store(e.to_string()))?;
        Ok(d.signature.returns.ty)
    }

    /// The tests as `define` keeps them, normalised (E5, full names), checked against the function
    /// `name` with descriptor `d` (not yet bound). Atomic: any refusal means nothing is kept.
    pub(crate) fn normalize_tests(
        &self,
        name: &str,
        d: &FunctionDescriptor,
        tests: &Value,
    ) -> Result<Vec<Value>, Error> {
        let Value::Array(items) = tests else {
            return Err(Error::Request("tests: an array of tests".into()));
        };
        let aliases = self.aliases_at(None);
        let returns = d.signature.returns.ty;
        let mut out = Vec::new();
        for (i, t) in items.iter().enumerate() {
            let p = format!("tests[{i}]");
            let Some(call_text) = t.get("call").and_then(Value::as_str) else {
                return Err(Error::Request(format!("{p}: a call of the function")));
            };
            let call = self.call_of(call_text, Some((name, d)))?;
            let of = format!("{}/{}", call.cell.ns, call.cell.fn_name);
            if of != name {
                return Err(Error::Request(format!("{p}: its call is of {of}, not of {name}, the function it tests")));
            }
            if call.cell.asof.is_some() {
                return Err(Error::Request(format!("{p}: the instant goes in asof, not in the call")));
            }
            let mut m: Vec<(String, Value)> = vec![
                ("test".into(), Value::String(name.to_string())),
                ("call".into(), Value::String(aliases.expand_prefix(call_text))),
            ];
            match t.get("asof") {
                None => {}
                Some(Value::String(a)) => {
                    lexical::check_instant(a, &format!("{p}.asof")).map_err(|e| Error::Request(e.to_string()))?;
                    m.push(("asof".into(), Value::String(a.clone())));
                }
                Some(_) => return Err(Error::Request(format!("{p}.asof: an instant"))),
            }
            if let Some(inputs) = t.get("inputs") {
                let Value::Object(given) = inputs else {
                    return Err(Error::Request(format!("{p}.inputs: an object of calls to values")));
                };
                let mut typed = Vec::new();
                for (key, v) in given {
                    let kcall = self.call_of(key, None)?;
                    let iname = format!("{}/{}", kcall.cell.ns, kcall.cell.fn_name);
                    let ty = self.returns_of(&iname)?;
                    if kcall.cell.args.values().any(|a| matches!(a, Arg::Slice)) {
                        // A slice (item 14): its members, each a cell of it with a plain literal value.
                        let members = members_of(v).ok_or_else(|| {
                            Error::Request(format!(
                                "{p}.inputs: {key} is a slice, given as {{\"members\": {{<cell call>: <value>, ...}}}}"
                            ))
                        })?;
                        let mut kept = Vec::new();
                        for (mk, mv) in members {
                            let mcall = self.call_of(mk, None).map_err(|_| {
                                Error::Request(format!("{p}.inputs: member {mk} is not a cell of {key}"))
                            })?;
                            let same = mcall.cell.ns == kcall.cell.ns
                                && mcall.cell.fn_name == kcall.cell.fn_name
                                && mcall.cell.args.keys().eq(kcall.cell.args.keys())
                                && kcall.cell.args.iter().all(|(k, a)| match a {
                                    Arg::Value(x) => mcall.cell.args.get(k) == Some(&Arg::Value(x.clone())),
                                    Arg::Slice => matches!(mcall.cell.args.get(k), Some(Arg::Value(_))),
                                });
                            if !same {
                                return Err(Error::Request(format!("{p}.inputs: member {mk} is not a cell of {key}")));
                            }
                            let text = literal_text(mv)
                                .ok_or_else(|| Error::Request(format!("{p}.inputs: member {mk} is a plain literal")))?;
                            let scalar = Scalar::from_lexical(ty, &text)
                                .map_err(|e| Error::Request(format!("{p}.inputs: {e}")))?;
                            kept.push((aliases.expand_prefix(mk), scalar.to_value()));
                        }
                        typed.push((
                            aliases.expand_prefix(key),
                            Value::Object(vec![("members".into(), Value::Object(kept))]),
                        ));
                        continue;
                    }
                    let text = literal_text(v)
                        .ok_or_else(|| Error::Request(format!("{p}.inputs: {key} is a plain literal")))?;
                    let scalar =
                        Scalar::from_lexical(ty, &text).map_err(|e| Error::Request(format!("{p}.inputs: {e}")))?;
                    typed.push((aliases.expand_prefix(key), scalar.to_value()));
                }
                m.push(("inputs".into(), Value::Object(typed)));
            }
            let expect = match t.get("expect") {
                None => return Err(Error::Request(format!("{p}: an expect, a value or {{\"fault\": code}}"))),
                Some(e) => match e.get("fault").and_then(Value::as_str) {
                    Some(code) if code.starts_with("E_FORMULA_") => {
                        Value::Object(vec![("fault".into(), Value::String(code.to_string()))])
                    }
                    Some(code) => return Err(Error::Request(format!("{p}.expect: {code} is not a formula fault"))),
                    None => {
                        let text =
                            literal_text(e).ok_or_else(|| Error::Request(format!("{p}.expect: a plain literal")))?;
                        let scalar = Scalar::from_lexical(returns, &text)
                            .map_err(|e| Error::Request(format!("{p}.expect: {e}")))?;
                        scalar.to_value()
                    }
                },
            };
            m.push(("expect".into(), expect));
            out.push(Value::Object(m));
        }
        Ok(out)
    }

    /// The closure of `name`: the function and every function its descriptor reads, transitively,
    /// with the descriptors and the starts of their bindings.
    fn closure(&self, name: &str) -> Result<Vec<Bound>, Error> {
        let mut todo = vec![name.to_string()];
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        while let Some(n) = todo.pop() {
            if !seen.insert(n.clone()) {
                continue;
            }
            let Some(b) = self.shared.store.resolve_binding(&n, END).map_err(|e| Error::Store(e.to_string()))? else {
                continue;
            };
            let bytes = self
                .shared
                .store
                .descriptor(&b.fn_hash)
                .ok_or_else(|| Error::Store(format!("descriptor {} is not stored", b.fn_hash)))?;
            let value = json::parse(&bytes).map_err(|e| Error::Store(e.to_string()))?;
            let d = FunctionDescriptor::from_json(&bytes).map_err(|e| Error::Store(e.to_string()))?;
            todo.extend(d.inputs.iter().cloned());
            out.push(Bound { name: n, value, start: b.tx });
        }
        Ok(out)
    }

    /// The coverage report of the tests of `name` (`tests` normalised, E5), with `shown` as the
    /// report's name. Every derived case of the body and every branch arm is listed when missing.
    pub(crate) fn coverage_report(&self, name: &str, shown: &str, tests: &[Value]) -> Result<MissingTests, Error> {
        let closure = self.closure(name)?;
        let target = closure
            .iter()
            .find(|b| b.name == name)
            .ok_or_else(|| Error::Request(format!("{name} is not bound here")))?;
        let function = Function::from_value(&target.value).map_err(|e| Error::Define(e.to_string()))?;
        let mut env = Env::new(name, function.signature.clone());
        for b in closure.iter().filter(|b| b.name != name) {
            env.functions
                .insert(b.name.clone(), Function::from_value(&b.value).map_err(|e| Error::Define(e.to_string()))?);
        }
        let compiled = check_descriptor(name, &target.value, &env).map_err(|e| Error::Define(e.to_string()))?;
        let points = coverage::branches(&compiled.ast);
        let facts = coverage::edge_facts(&compiled, &function.signature);
        let mut cov = Coverage::new();
        let (mut absent_hits, mut empty_hits) = (BTreeSet::new(), BTreeSet::new());
        let (mut zero_hits, mut neg_hits, mut end_hits) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        for t in tests {
            let spec = self.spec_of(t)?;
            let mut world = MemoryWorld::new(&spec.asof);
            for b in &closure {
                world.bind(&b.name, &b.value, &b.start).map_err(|e| Error::Define(e.to_string()))?;
            }
            for (text, value) in &spec.inputs {
                let call = self.call_of(text, None)?;
                let fname = format!("{}/{}", call.cell.ns, call.cell.fn_name);
                let fn_hash = self.binding_hash(&fname)?;
                let cell = call.cell.resolve(&fn_hash).map_err(|e| Error::Request(e.to_string()))?;
                let receipt = input_receipt(&cell, value, &spec.asof);
                world.add_receipt(&receipt).map_err(|e| Error::Define(e.to_string()))?;
            }
            for (_, members) in &spec.slices {
                for (text, value) in members {
                    let call = self.call_of(text, None)?;
                    let fname = format!("{}/{}", call.cell.ns, call.cell.fn_name);
                    let fn_hash = self.binding_hash(&fname)?;
                    let cell = call.cell.resolve(&fn_hash).map_err(|e| Error::Request(e.to_string()))?;
                    let receipt = input_receipt(&cell, value, &spec.asof);
                    world.add_receipt(&receipt).map_err(|e| Error::Define(e.to_string()))?;
                }
            }
            let mut probe = Probe {
                world,
                resolver: self,
                asof: spec.asof.clone(),
                tried: BTreeSet::new(),
                misses: BTreeSet::new(),
                empty: BTreeSet::new(),
            };
            let _ = eval::evaluate_observed(
                &compiled,
                &function,
                &scalar_args(&spec.call.cell.args)?,
                &spec.asof,
                &mut probe,
                &mut cov as &mut dyn EvalObserver,
            );
            for (arg, a) in &spec.call.cell.args {
                let Some(lex) = lexical_of(a) else { continue };
                if function.signature.args.get(arg).is_some_and(|d| d.ty == Type::Number) {
                    if is_zero(lex) {
                        zero_hits.insert(arg.clone());
                    }
                    if is_negative(lex) {
                        neg_hits.insert(arg.clone());
                    }
                }
                if let Some(Extent::Range(lo, hi)) = function.signature.args.get(arg).and_then(|d| d.extent.clone()) {
                    if lex == lo {
                        end_hits.insert(format!("{arg}\u{1}lo"));
                    }
                    if lex == hi {
                        end_hits.insert(format!("{arg}\u{1}hi"));
                    }
                }
            }
            if matches!(&spec.expect, Expect::Fault(c) if c == "E_FORMULA_ABSENT") {
                absent_hits.extend(probe.misses.iter().cloned());
            }
            empty_hits.extend(probe.empty.iter().cloned());
        }
        let mut cases = Vec::new();
        for d in &facts.divisors {
            if !zero_hits.contains(d) {
                cases.push(format!("zero: {d} is a divisor, and no test has {d} = 0"));
            }
        }
        for n in &facts.number_args {
            if !neg_hits.contains(n) {
                cases.push(format!("negative: {n} is a number, and no test has {n} below 0"));
            }
        }
        for (arg, extent) in &facts.extent_args {
            let (lo, hi) = match extent {
                Extent::Range(lo, hi) => (lo.as_str(), hi.as_str()),
                Extent::Values(v) => match (v.first(), v.last()) {
                    (Some(lo), Some(hi)) => (lo.as_str(), hi.as_str()),
                    _ => continue,
                },
            };
            if !end_hits.contains(&format!("{arg}\u{1}lo")) {
                cases.push(format!("extent: {arg} at its lower end {lo}, no test has it"));
            }
            if !end_hits.contains(&format!("{arg}\u{1}hi")) {
                cases.push(format!("extent: {arg} at its upper end {hi}, no test has it"));
            }
        }
        for x in &facts.cell_inputs {
            if !absent_hits.contains(x) {
                cases.push(format!("absent: {x} has no value in a test that expects E_FORMULA_ABSENT"));
            }
        }
        for x in &facts.slice_inputs {
            if !empty_hits.contains(x) {
                cases.push(format!("empty: the slice {x} has no members in no test"));
            }
        }
        for p in coverage::branches(&compiled.ast) {
            for arm in 0..p.arms {
                if !cov.contains(p.id, arm) {
                    let kind = if p.kind == BranchKind::If { "if" } else { "otherwise" };
                    let which =
                        if p.kind == BranchKind::If { ["then", "else"][arm] } else { ["value", "fallback"][arm] };
                    cases.push(format!("branch: the {which} arm of an {kind} is taken by no test"));
                }
            }
        }
        Ok(MissingTests {
            function: shown.to_string(),
            cases,
            branches_total: Coverage::total(&points),
            branches_covered: cov.covered(&points),
        })
    }

    /// The fn hash the name is bound to now.
    fn binding_hash(&self, name: &str) -> Result<String, Error> {
        self.shared
            .store
            .resolve_binding(name, END)
            .map_err(|e| Error::Store(e.to_string()))?
            .map(|b| b.fn_hash)
            .ok_or_else(|| Error::Request(format!("{name} is not bound here")))
    }

    /// A normalised test read back: its call parsed, its inputs typed, its expectation.
    fn spec_of(&self, t: &Value) -> Result<Spec, Error> {
        let call_text =
            t.get("call").and_then(Value::as_str).ok_or_else(|| Error::Store("a kept test without a call".into()))?;
        let call = self.call_of(call_text, None)?;
        let asof = match t.get("asof").and_then(Value::as_str) {
            Some(a) => a.to_string(),
            None => self.shared.now(),
        };
        let mut inputs = Vec::new();
        let mut slices = Vec::new();
        if let Some(Value::Object(given)) = t.get("inputs") {
            for (k, typed) in given {
                if let Some(members) = members_of(typed) {
                    let mut kept = Vec::new();
                    for (mk, mv) in members {
                        let mc = self.call_of(mk, None)?;
                        let ty = self.returns_of(&format!("{}/{}", mc.cell.ns, mc.cell.fn_name))?;
                        let text =
                            literal_text(mv).ok_or_else(|| Error::Store("a kept slice member is typed".into()))?;
                        kept.push((
                            mk.clone(),
                            Scalar::from_lexical(ty, &text).map_err(|e| Error::Store(e.to_string()))?,
                        ));
                    }
                    slices.push((k.clone(), kept));
                    continue;
                }
                let ty = typed.get("type").and_then(Value::as_str).and_then(Type::from_name);
                let text = typed.get("value").and_then(literal_text);
                let (Some(ty), Some(text)) = (ty, text) else {
                    return Err(Error::Store("a kept test's input is typed".into()));
                };
                let scalar = Scalar::from_lexical(ty, &text).map_err(|e| Error::Store(e.to_string()))?;
                inputs.push((k.clone(), scalar));
            }
        }
        let expect = expect_of(t.get("expect").ok_or_else(|| Error::Store("a kept test without an expect".into()))?)?;
        Ok(Spec { call, asof, inputs, slices, expect })
    }
}

/// The receipt, in the shape the evaluator reads a stored cell from, of `cell` with `value` as of
/// `asof` (a test's input).
fn input_receipt(cell: &Cell, value: &Scalar, asof: &str) -> Value {
    let typed = value.to_value();
    Value::Object(vec![
        ("cell".into(), cell.to_value()),
        ("type".into(), typed.get("type").cloned().unwrap_or(Value::Null)),
        ("value".into(), typed.get("value").cloned().unwrap_or(Value::Null)),
        ("tx".into(), Value::String(asof.to_string())),
        ("asof".into(), Value::String(asof.to_string())),
    ])
}

/// The cases describe lists of a function's tests (CONTRACT-0.4.9 L1, S3 and H1).
pub(crate) const LISTED_CASES: usize = 100;

/// What describe says of a function's tests (datumesh-local 0.4.3, L1): the `tests` member, or
/// `null` with `note` saying why they cannot be read.
#[derive(Clone, Debug)]
pub(crate) struct TestsListing {
    pub(crate) tests: Value,
    pub(crate) note: Option<String>,
}

impl TestsListing {
    /// A listing whose tests cannot be read, and why.
    pub(crate) fn unread(note: impl Into<String>) -> TestsListing {
        TestsListing { tests: Value::Null, note: Some(note.into()) }
    }

    /// The count of the tests, or `None` when they are unread.
    fn count(&self) -> Option<u64> {
        self.tests.get("count").and_then(number_of)
    }

    /// The text of describe: a headline (`N tests`, or `tests: N` for the command line, or
    /// `no tests`), the last run, and one line per listed case. When the tests cannot be read, one
    /// line says so.
    pub(crate) fn lines(&self, cli: bool) -> Vec<String> {
        if let Some(note) = &self.note {
            return vec![format!("tests: unavailable ({note})")];
        }
        let count = self.count().unwrap_or(0);
        if count == 0 {
            return vec!["no tests".to_string()];
        }
        let mut out = vec![if cli {
            format!("tests: {count}")
        } else {
            format!("{count} {}", if count == 1 { "test" } else { "tests" })
        }];
        match self.tests.get("last_run") {
            Some(lr @ Value::Object(_)) => {
                let result = lr.get("result").and_then(Value::as_str).unwrap_or("");
                let at = lr.get("at").and_then(Value::as_str).unwrap_or("");
                out.push(format!("last run {at} {result}"));
            }
            _ => out.push("last run: none".to_string()),
        }
        let cases = match self.tests.get("cases") {
            Some(Value::Array(a)) => a.as_slice(),
            _ => &[],
        };
        for c in cases {
            let call = c.get("call").and_then(Value::as_str).unwrap_or("");
            let expect = c.get("expect").and_then(Value::as_str).unwrap_or("");
            let result = c.get("result").and_then(Value::as_str).unwrap_or("");
            out.push(format!("  {call}  expect {expect}  result {result}"));
        }
        let total = self.tests.get("cases_total").and_then(number_of).unwrap_or(count);
        if total > cases.len() as u64 {
            out.push(format!("  {} of {total} cases shown", cases.len()));
        }
        out
    }
}

/// A JSON number's value as a `u64`, when it is one.
fn number_of(v: &Value) -> Option<u64> {
    v.as_number_text()?.parse().ok()
}

/// A count as a JSON number.
fn number(n: u64) -> Value {
    json::parse(n.to_string().as_bytes()).unwrap_or(Value::Null)
}

/// One listed case: its call, its expectation and its result.
fn case_value(call: &str, expect: &str, result: &str) -> Value {
    Value::Object(vec![
        ("call".into(), Value::String(call.to_string())),
        ("expect".into(), Value::String(expect.to_string())),
        ("result".into(), Value::String(result.to_string())),
    ])
}

/// The `tests` member: the count, the total, the last run (or null) and the listed cases.
fn tests_value(count: u64, total: u64, last_run: Value, cases: Vec<Value>) -> Value {
    Value::Object(vec![
        ("count".into(), number(count)),
        ("cases_total".into(), number(total)),
        ("last_run".into(), last_run),
        ("cases".into(), Value::Array(cases)),
    ])
}

/// A stored expectation as text: the fault code, or the value's literal text.
fn expect_text(e: Option<&Value>) -> String {
    let Some(e) = e else { return String::new() };
    if let Some(code) = e.get("fault").and_then(Value::as_str) {
        return format!("fault {code}");
    }
    match e.get("value") {
        Some(v) => literal_text(v).unwrap_or_else(|| String::from_utf8_lossy(&v.to_jcs()).into_owned()),
        None => String::new(),
    }
}

impl Resolver {
    /// The tests of the local function with fn hash `hash` (its own, as `define` kept them). No run
    /// is recorded for a local function, so each case's result is "not run" and `last_run` is null.
    pub(crate) fn local_tests_listing(&self, hash: &str) -> Result<TestsListing, Error> {
        let tests = self.tests_of(hash)?;
        let cases = tests
            .iter()
            .take(LISTED_CASES)
            .map(|t| {
                let call = t.get("call").and_then(Value::as_str).unwrap_or("");
                case_value(call, &expect_text(t.get("expect")), "not run")
            })
            .collect();
        let n = tests.len() as u64;
        Ok(TestsListing { tests: tests_value(n, n, Value::Null, cases), note: None })
    }
}

/// The largest function page the tests are read from (as a describe answer's fetch).
const PAGE_MAX: u64 = 16 << 20;

impl Resolver {
    /// The tests of the public function `full`, read from its function page
    /// (`https://<host>/fn/<name>?format=json`; the host's configured peer answers it). A page that
    /// cannot be read leaves the tests null, with the reason as the note.
    pub(crate) fn public_tests_listing(&self, full: &str) -> TestsListing {
        let s = &self.shared;
        let name = name_of(full);
        let host = host_of(name);
        // The page is asked by the full name, with its host (datumesh-local 0.5.1, L2).
        if let Err(why) = s.may_fetch(host) {
            return TestsListing::unread(why);
        }
        let url = format!("https://{host}/fn/{name}?format=json");
        let body = match s.client.get(&url, PAGE_MAX) {
            Ok((200, body)) => body,
            Ok((status, _)) => return TestsListing::unread(format!("{url}: status {status}")),
            Err(e) => return TestsListing::unread(format!("{url}: {e}")),
        };
        let Ok(page) = json::parse(&body) else {
            return TestsListing::unread(format!("{url}: not JSON"));
        };
        let Some(t @ Value::Object(_)) = page.get("tests") else {
            return TestsListing::unread(format!("{url}: no tests member"));
        };
        let Some(count) = t.get("count").and_then(number_of) else {
            return TestsListing::unread(format!("{url}: no tests count"));
        };
        let total = t.get("cases_total").and_then(number_of).unwrap_or(count);
        let cases = match t.get("cases") {
            Some(Value::Array(a)) => a
                .iter()
                .take(LISTED_CASES)
                .map(|c| {
                    let call = c.get("call").and_then(Value::as_str).unwrap_or("");
                    let result = c.get("result").and_then(Value::as_str).unwrap_or("");
                    case_value(call, &plain_text(c.get("expect")), result)
                })
                .collect(),
            _ => Vec::new(),
        };
        // The 0.4.8 last run ({result, at}) and the 0.4.9 one, which adds pass/fail/error/not_run.
        let last_run = match t.get("last_run") {
            Some(lr @ Value::Object(_)) => match lr.get("result").and_then(Value::as_str) {
                Some(result) => Value::Object(vec![
                    ("result".into(), Value::String(result.to_string())),
                    (
                        "at".into(),
                        lr.get("at").and_then(Value::as_str).map_or(Value::Null, |a| Value::String(a.to_string())),
                    ),
                ]),
                None => Value::Null,
            },
            _ => Value::Null,
        };
        TestsListing { tests: tests_value(count, total, last_run, cases), note: None }
    }
}

/// A page's expectation as text: a string as it is, anything else as its literal or JSON text.
fn plain_text(v: Option<&Value>) -> String {
    match v {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => literal_text(other).unwrap_or_else(|| String::from_utf8_lossy(&other.to_jcs()).into_owned()),
    }
}
