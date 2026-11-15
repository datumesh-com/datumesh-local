//! Rooting (CONTRACT-1.8b L5): this machine's named functions and their stored cells, republished
//! under a rooted namespace whose key this machine holds. Stored cells are re-signed by the
//! namespace key at now; formulas are rewritten to the rooted names; computed cells are listed,
//! never copied. The export is a bundle 1 (`datumesh_http::bundle::build`) that the mesh imports
//! with `import_signed`. Nothing is written unless every check passes; the key is read in place
//! and never stored, logged or printed.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use datumesh_core::json::{self, Value};
use datumesh_core::{Cell, FunctionDescriptor, Receipt, ReceiptBuilder, SigningKey};
use datumesh_formula::definition::{define_function, Callee};

use crate::fetch::{host_of, END};
use crate::gate::is_unrooted;
use crate::key::parse_hex;
use crate::resolver::{call_form, cell_uri, rejection_detail, Callees as ResolverCallees};
use crate::{Error, Resolver};

/// A rooting plan (CONTRACT-1.8b L5): the rooted namespace, the key file of its host, the names of
/// this machine (`local/...` or full: the rooting set) and, when given, the bundle to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootPlan {
    /// The rooted namespace (`<host>/<segments>`, the host not under `.invalid`).
    pub namespace: String,
    /// The namespace host's key file, read in place (mode 0600: a 64-hex seed on the first line,
    /// or a machine key file).
    pub key_path: PathBuf,
    /// This machine's names, the rooting set.
    pub names: Vec<String>,
    /// The bundle 1 to write (a new file, mode 0600); `None` for a dry run.
    pub export: Option<PathBuf>,
}

/// The kind of a function rooted.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootKind {
    /// A stored function: its descriptor is kept, so its hash is kept.
    Stored,
    /// A formula: its inputs are rewritten to the rooted names, so its hash changes.
    Formula,
}

impl RootKind {
    /// `stored` or `formula`.
    pub fn as_str(self) -> &'static str {
        match self {
            RootKind::Stored => "stored",
            RootKind::Formula => "formula",
        }
    }
}

impl std::fmt::Display for RootKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A function rooted: its name here and its rooted name, and both hashes.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootedFunction {
    /// The full name here (`<host>/<...>`).
    pub from: String,
    /// The rooted name (`<namespace>/<...>`).
    pub to: String,
    /// The function hash here.
    pub from_hash: String,
    /// The function hash rooted.
    pub to_hash: String,
    /// Stored or formula.
    pub kind: RootKind,
}

/// A stored cell re-signed under the namespace.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootedCell {
    /// The rooted call (`<namespace>/<fn>(k: v, ...)`).
    pub call: String,
    /// The rooted cell's URI (no asof).
    pub uri: String,
    /// The receipt hash of this machine's version, the one re-signed.
    pub from_receipt: String,
    /// The value, lexical.
    pub value: String,
}

/// What a rooting did (or, for a dry run, would do).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rooted {
    /// The rooted namespace.
    pub namespace: String,
    /// The namespace key's id (the key that signs the bundle and the receipts).
    pub key_id: String,
    /// The functions of the rooting set, by their names.
    pub functions: Vec<RootedFunction>,
    /// The stored cells re-signed, in call order.
    pub cells: Vec<RootedCell>,
    /// The computed cells this machine holds of the set's formulas: listed, not copied (call
    /// forms under the rooted namespace, so they can be recomputed).
    pub computed: Vec<String>,
}

impl Rooted {
    /// The plan's result as JSON (the CLI prints it as one line).
    pub fn to_json(&self) -> Value {
        let s = |x: &str| Value::String(x.to_string());
        let functions = self
            .functions
            .iter()
            .map(|f| {
                Value::Object(vec![
                    ("from".into(), s(&f.from)),
                    ("from_hash".into(), s(&f.from_hash)),
                    ("kind".into(), s(f.kind.as_str())),
                    ("to".into(), s(&f.to)),
                    ("to_hash".into(), s(&f.to_hash)),
                ])
            })
            .collect();
        let cells = self
            .cells
            .iter()
            .map(|c| {
                Value::Object(vec![
                    ("call".into(), s(&c.call)),
                    ("from_receipt".into(), s(&c.from_receipt)),
                    ("uri".into(), s(&c.uri)),
                    ("value".into(), s(&c.value)),
                ])
            })
            .collect();
        Value::Object(vec![
            ("cells".into(), Value::Array(cells)),
            ("computed".into(), Value::Array(self.computed.iter().map(|c| s(c)).collect())),
            ("functions".into(), Value::Array(functions)),
            ("key_id".into(), s(&self.key_id)),
            ("namespace".into(), s(&self.namespace)),
        ])
    }
}

/// A rooted function, as the rooting computed it: its bytes for the bundle and its callee entry.
struct Entry {
    from_hash: String,
    to_hash: String,
    kind: RootKind,
    bytes: Vec<u8>,
}

/// The callees of a rooted definition: the rooted functions done so far, and the store's bindings
/// for every other name (nothing is fetched: rooting makes requests only for the key document).
struct RootCallees<'a> {
    resolver: &'a Resolver,
    inner: ResolverCallees<'a>,
    rooted: &'a BTreeMap<String, Callee>,
}

impl datumesh_formula::definition::Callees for RootCallees<'_> {
    fn callee(&self, name: &str, at: &str) -> Result<Option<Callee>, String> {
        if let Some(c) = self.rooted.get(name) {
            return Ok(Some(c.clone()));
        }
        let store = &self.resolver.shared.store;
        let Some(b) = store.resolve_binding(name, at).map_err(|e| e.to_string())? else { return Ok(None) };
        let bytes = store.descriptor(&b.fn_hash).ok_or_else(|| format!("descriptor {} is not stored", b.fn_hash))?;
        let descriptor = json::parse(&bytes).map_err(|e| e.to_string())?;
        Ok(Some(Callee { fn_hash: b.fn_hash, start: b.tx, descriptor }))
    }

    fn class_descriptor(&self, class_hash: &str) -> Result<Option<datumesh_core::ClassDescriptor>, String> {
        datumesh_formula::definition::Callees::class_descriptor(&self.inner, class_hash)
    }

    fn class_name(&self, class_hash: &str) -> Result<Option<String>, String> {
        datumesh_formula::definition::Callees::class_name(&self.inner, class_hash)
    }
}

fn rooting(m: impl Into<String>) -> Error {
    Error::Rooting(m.into())
}

/// Whether `c` can be part of a function name (ASCII letters, digits and `_ - . /`).
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/')
}

/// `src` with every whole occurrence of a name in `map` (from, to) replaced by its `to`, outside
/// string literals. A name is whole when neither neighbour is a name character.
fn rename_calls(src: &str, map: &BTreeMap<String, String>) -> String {
    let chars: Vec<char> = src.chars().collect();
    let pats: Vec<(Vec<char>, &String)> = map.iter().map(|(f, t)| (f.chars().collect(), t)).collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            // A string literal: copied through its closing quote (`""` is a quote inside it).
            out.push('"');
            i += 1;
            while i < chars.len() {
                if chars[i] == '"' {
                    if chars.get(i + 1) == Some(&'"') {
                        out.push_str("\"\"");
                        i += 2;
                        continue;
                    }
                    out.push('"');
                    i += 1;
                    break;
                }
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        let before_ok = i == 0 || !is_name_char(chars[i - 1]);
        if before_ok {
            let hit = pats
                .iter()
                .find(|(p, _)| chars[i..].starts_with(p) && chars.get(i + p.len()).is_none_or(|c| !is_name_char(*c)));
            if let Some((p, to)) = hit {
                out.push_str(to);
                i += p.len();
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// The receipt's bytes as the mesh serves them: its payload with `sig`, canonical.
fn receipt_bytes(r: &Receipt) -> Result<Vec<u8>, Error> {
    let Value::Object(mut m) = json::parse(r.payload()).map_err(|e| rooting(e.to_string()))? else {
        return Err(rooting("a receipt payload is not an object"));
    };
    m.push(("sig".to_string(), Value::String(r.sig().to_string())));
    m.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::Object(m).to_jcs())
}

/// The namespace: a rooted host (not under `.invalid`) that is a DNS name, then name segments.
fn check_namespace(ns: &str) -> Result<(), Error> {
    let segs: Vec<&str> = ns.split('/').collect();
    let host = segs[0];
    if is_unrooted(host) {
        return Err(rooting(format!("{ns}: the host {host} is under .invalid, so the namespace is not rooted")));
    }
    if segs.len() < 2
        || !datumesh_formula::names::is_dns_name(host)
        || !segs[1..].iter().all(|s| datumesh_formula::names::is_name(s))
    {
        return Err(rooting(format!("{ns} is not a namespace (SPEC 2)")));
    }
    Ok(())
}

/// The key file's seed: mode 0600 (unix), then 64 hex digits on the first line (a machine key
/// file's first line too). The file is read in place; its contents are never echoed.
fn read_key(path: &Path) -> Result<[u8; 32], Error> {
    let err = |e: String| Error::Key(format!("{}: {e}", path.display()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).map_err(|e| err(e.to_string()))?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(err(format!("mode {:o}: the key file must have mode 0600", mode & 0o777)));
        }
    }
    let text = std::fs::read_to_string(path).map_err(|e| err(e.to_string()))?;
    let first = text.lines().next().map(str::trim).unwrap_or("");
    parse_hex(first).ok_or_else(|| err("not a key file: a 64-hex seed on the first line".into()))
}

/// Writes `bytes` to a new file at `path`, mode 0600; an existing file is refused.
fn write_new_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let err = |e: std::io::Error| Error::Bundle(format!("{}: {e}", path.display()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).map_err(err)?;
    f.write_all(bytes).and_then(|()| f.sync_all()).map_err(err)
}

impl Resolver {
    /// Roots the names of this machine under `plan.namespace`, signed by the key file of
    /// `plan.key_path` (CONTRACT-1.8b L5). The checks run in order and the first failure is
    /// reported, with nothing written. The result lists the rooted functions, the stored cells
    /// re-signed (`tx` = `asof` = `issued` = now), and the computed cells held (not copied). With
    /// `plan.export`, the bundle 1 is written to that new file (mode 0600).
    pub fn root(&self, plan: &RootPlan) -> Result<Rooted, Error> {
        let s = &self.shared;
        let ns = plan.namespace.as_str();
        let host = host_of(ns).to_string();
        // 1. The namespace.
        check_namespace(ns)?;
        // 2. The key file.
        let seed = read_key(&plan.key_path)?;
        let key = SigningKey::from_seed(&seed);
        // 3. The host's key document: the key's id, its window holding now.
        let now = s.now();
        self.check_key_document(&host, key.key_id(), &now)?;
        // 4. The names.
        let aliases = self.aliases_at(None);
        let mut set: BTreeSet<String> = BTreeSet::new();
        for n in &plan.names {
            let full = aliases.expand(n);
            if host_of(&full) != s.config.host {
                return Err(rooting(format!("{full} is not under this machine's host {}", s.config.host)));
            }
            if !datumesh_formula::names::is_function_name(&full) {
                return Err(rooting(format!("{full} is not a full function name")));
            }
            if s.store.resolve_binding(&full, END).map_err(|e| Error::Store(e.to_string()))?.is_none() {
                return Err(rooting(format!("{full} is not bound here")));
            }
            set.insert(full);
        }
        if set.is_empty() {
            return Err(rooting("no names to root"));
        }
        let to_of = |from: &str| format!("{ns}/{}", &from[s.config.host.len() + 1..]);
        let map: BTreeMap<String, String> = set.iter().map(|f| (f.clone(), to_of(f))).collect();
        // The formulas after the functions they read (a name's inputs in the set first).
        let mut pending: Vec<String> = set.iter().cloned().collect();
        let mut done: BTreeMap<String, Entry> = BTreeMap::new();
        let mut rooted_callees: BTreeMap<String, Callee> = BTreeMap::new();
        while !pending.is_empty() {
            let ready = pending.iter().position(|f| {
                let inputs = self.inputs_of(f).unwrap_or_default();
                inputs.iter().all(|i| !set.contains(i) || done.contains_key(i))
            });
            let Some(pos) = ready else {
                return Err(rooting("the formulas of the set read each other (a cycle)"));
            };
            let from = pending.remove(pos);
            let (binding, desc_bytes) = self.binding_of(&from)?;
            let d = FunctionDescriptor::from_json(&desc_bytes).map_err(|e| Error::Store(e.to_string()))?;
            let to = to_of(&from);
            let entry = if d.lang == "stored" {
                let canon = d.canonical();
                let to_hash = d.hash();
                Entry { from_hash: binding.clone(), to_hash, kind: RootKind::Stored, bytes: canon }
            } else {
                let source = rename_calls(&d.body, &map);
                let signature = json::parse(&desc_bytes)
                    .map_err(|e| Error::Store(e.to_string()))?
                    .get("signature")
                    .cloned()
                    .unwrap_or(Value::Null);
                let rc = RootCallees { resolver: self, inner: ResolverCallees::new(s), rooted: &rooted_callees };
                let def = define_function(&to, &source, &signature, &BTreeMap::new(), &rc, &now)
                    .map_err(|e| Error::Define(format!("{to}: {e}")))?;
                if let Some(input) = def.descriptor.inputs.iter().find(|i| is_unrooted(host_of(i))) {
                    return Err(Error::UnrootedInput { function: to.clone(), input: input.clone() });
                }
                Entry {
                    from_hash: binding.clone(),
                    to_hash: def.fn_hash.clone(),
                    kind: RootKind::Formula,
                    bytes: def.bytes.clone(),
                }
            };
            let value = json::parse(&entry.bytes).map_err(|e| Error::Define(e.to_string()))?;
            rooted_callees
                .insert(to.clone(), Callee { fn_hash: entry.to_hash.clone(), start: now.clone(), descriptor: value });
            done.insert(from, entry);
        }
        // The stored cells: the latest version of each held cell of a stored function in the set.
        let mut cells: BTreeMap<String, RootedCell> = BTreeMap::new();
        let mut receipts: Vec<Vec<u8>> = Vec::new();
        let mut seen_cells: BTreeSet<String> = BTreeSet::new();
        for c in s.index.cells().map_err(|e| Error::Store(e.to_string()))? {
            if c.slice.is_some() || !seen_cells.insert(c.cell_id.clone()) {
                continue;
            }
            let Some(v) = s.store.latest(&c.cell_id).map_err(|e| Error::Store(e.to_string()))? else { continue };
            let Some(bytes) = s.store.receipt(&v.receipt_hash) else { continue };
            let old = Receipt::from_json(&bytes).map_err(|e| Error::Store(e.to_string()))?;
            let fname = format!("{}/{}", old.cell().ns, old.cell().fn_name);
            let Some(entry) = done.get(&fname) else { continue };
            if entry.kind != RootKind::Stored || entry.from_hash != old.cell().fn_hash {
                continue;
            }
            let to = to_of(&fname);
            let rooted_cell = Cell {
                ns: ns.to_string(),
                fn_name: to[ns.len() + 1..].to_string(),
                fn_hash: entry.to_hash.clone(),
                args: old.cell().args.clone(),
            };
            let fresh = ReceiptBuilder::new()
                .cell(rooted_cell.clone())
                .value(old.value().clone())
                .valid(old.valid().clone())
                .asof(now.clone())
                .tx(now.clone())
                .issued(now.clone())
                .publisher(ns)
                .signer_host(host.clone())
                .licence(old.licence())
                .summary(old.summary())
                .sign(&key)
                .map_err(|e| rooting(format!("{}: {e}", call_form(&rooted_cell))))?;
            receipts.push(receipt_bytes(&fresh)?);
            cells.insert(
                call_form(&rooted_cell),
                RootedCell {
                    call: call_form(&rooted_cell),
                    uri: cell_uri(&rooted_cell),
                    from_receipt: old.hash().to_string(),
                    value: old.value().lexical().to_string(),
                },
            );
        }
        // The computed cells held of the set's formulas: listed under the rooted namespace.
        let mut computed: BTreeSet<String> = BTreeSet::new();
        for (h, _cell_id) in s.index.all_answered().map_err(|e| Error::Store(e.to_string()))? {
            let Some(bytes) = s.store.receipt(&h) else { continue };
            let r = Receipt::from_json(&bytes).map_err(|e| Error::Store(e.to_string()))?;
            let fname = format!("{}/{}", r.cell().ns, r.cell().fn_name);
            let Some(entry) = done.get(&fname) else { continue };
            if entry.kind != RootKind::Formula || entry.from_hash != r.cell().fn_hash {
                continue;
            }
            let rooted_cell = Cell {
                ns: ns.to_string(),
                fn_name: to_of(&fname)[ns.len() + 1..].to_string(),
                fn_hash: entry.to_hash.clone(),
                args: r.cell().args.clone(),
            };
            computed.insert(call_form(&rooted_cell));
        }
        let functions: Vec<RootedFunction> = done
            .iter()
            .map(|(from, e)| RootedFunction {
                from: from.clone(),
                to: to_of(from),
                from_hash: e.from_hash.clone(),
                to_hash: e.to_hash.clone(),
                kind: e.kind,
            })
            .collect();
        let rooted = Rooted {
            namespace: ns.to_string(),
            key_id: key.key_id().to_string(),
            functions,
            cells: cells.into_values().collect(),
            computed: computed.into_iter().collect(),
        };
        // 5. The bundle, written last, to a new file.
        if let Some(path) = &plan.export {
            let descriptors: Vec<Vec<u8>> = done.values().map(|e| e.bytes.clone()).collect();
            let names: Vec<(String, String)> = done.iter().map(|(f, e)| (to_of(f), e.to_hash.clone())).collect();
            let bundle = datumesh_http::bundle::build(ns, descriptors, names, receipts, &seed)
                .map_err(|e| Error::Bundle(e.to_string()))?;
            write_new_private(path, &bundle)?;
        }
        Ok(rooted)
    }

    /// The key document of `host` holds `key_id` with a window holding `now` (the key document
    /// fetched through the client: the host must be on the allowlist, or a peer).
    fn check_key_document(&self, host: &str, key_id: &str, now: &str) -> Result<(), Error> {
        let s = &self.shared;
        s.may_fetch(host).map_err(rooting)?;
        s.patient(|| s.client.key_document(host, false))
            .map_err(|e| rooting(format!("{host}'s key document: {}", rejection_detail(&e))))?;
        let bytes = s
            .client
            .key_document_bytes(host)
            .ok_or_else(|| rooting(format!("{host}'s key document is not available")))?;
        let doc = datumesh_core::keys_from_response(host, 200, Some(&bytes))
            .map_err(|e| rooting(format!("{host}'s key document: {e}")))?;
        match doc.key(key_id) {
            Some(k) if k.usable_at(now) => Ok(()),
            _ => Err(rooting(format!("key {key_id} is not in {host}'s key document"))),
        }
    }

    /// The binding of a name here: its function hash and its descriptor's bytes.
    fn binding_of(&self, from: &str) -> Result<(String, Vec<u8>), Error> {
        let s = &self.shared;
        let b = s
            .store
            .resolve_binding(from, END)
            .map_err(|e| Error::Store(e.to_string()))?
            .ok_or_else(|| rooting(format!("{from} is not bound here")))?;
        let bytes = s
            .store
            .descriptor(&b.fn_hash)
            .ok_or_else(|| Error::Store(format!("descriptor {} is not stored", b.fn_hash)))?;
        Ok((b.fn_hash, bytes.into_owned()))
    }

    /// The inputs of a name's descriptor here (none when it is not held).
    fn inputs_of(&self, from: &str) -> Option<Vec<String>> {
        let (_, bytes) = self.binding_of(from).ok()?;
        FunctionDescriptor::from_json(&bytes).ok().map(|d| d.inputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_whole_names_outside_strings() {
        let map: BTreeMap<String, String> = [
            ("local.invalid/level".to_string(), "h.example/ns/level".to_string()),
            ("local.invalid/level2".to_string(), "h.example/ns/level2".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            rename_calls(r#"local.invalid/level(day: day) * 2 + local.invalid/level2(day: day)"#, &map),
            r#"h.example/ns/level(day: day) * 2 + h.example/ns/level2(day: day)"#
        );
        assert_eq!(rename_calls(r#""local.invalid/level" + x"#, &map), r#""local.invalid/level" + x"#);
        assert_eq!(rename_calls("xlocal.invalid/level", &map), "xlocal.invalid/level");
    }
}
