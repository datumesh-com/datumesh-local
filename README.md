# datumesh-local

The [Datumesh](https://datumesh.com) local resolver: the mesh's engine on your machine (or an
agent's), over a local store, so a working set of cells answers fast, offline, and with the
same bytes every time. It is the cache tier of the memory hierarchy and the place agents
publish to. The `datumesh` command is in the crate [`datumesh`](https://crates.io/crates/datumesh)
(`cargo install datumesh`); this crate is the library behind it.

- **Same engine, local store.** The engine, formula runtime and mesh handlers of
  `datumesh-engine`, `datumesh-formula` and `datumesh-http`, over `datumesh-store`'s SQLite
  backend (one file, no server; PostgreSQL by URL).
- **No network except explicit fetches.** A `get` of a cell not held, `prefetch`, `refresh`,
  a forwarded request, `find --remote`, and nothing else; only to the hosts of the
  allowlist; every fetched receipt checked against its publisher's key document and kept with
  its signature.
- **`local/` is `local.invalid`.** Your own functions and cells, signed with this machine's
  key, verifiable here and wherever its key document is trusted.
- **A licence gate.** The engine's licence inheritance (spec-0.4.3 8.5): a function evaluates
  here over open and local inputs (the result carrying the inherited licence); over a
  restricted input its publisher declared, the request goes to the configured mesh and its
  answer (its signature) is returned, since a restricted licence is never signed by this
  machine's key; over an undeclared one the answer is `unlicensed`, here, unless the licence is
  accepted at the command line (below).

## Use

```text
$ cargo install datumesh
$ datumesh get 'ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)'
{"fn_hash":"f041cf…","outcome":"ok","receipt":{…},"trust":"primary"}
ok (fetched); signer datumesh.com#eddd0c12fedb022e
$ datumesh prefetch 'scripture/verse_text(corpus: "quran", book: *, chapter: *, verse: *, edition: "uthmani")'
$ datumesh --offline get 'scripture/verse_text(corpus: "quran", book: "2", chapter: 1, verse: 255, edition: "uthmani")'
$ datumesh define local/closed --formula 'mesh-test.datumesh.com/ecb/holiday(calendar: "TARGET", date: date)' \
    --signature '{"args": {"date": {"type": "date", "valid": true}}, "order": ["date"], "returns": "boolean"}'
$ datumesh get 'local/closed(date: 2026-12-25)'
… ok (fetched); signer local.invalid#d7817a1eb0c608ad (local, verifies only on this machine)
$ datumesh serve        # http://127.0.0.1:7733, the mesh's endpoints over the local store
```

| Command | |
|---|---|
| `get <request>...` | the envelope (SPEC 5.5) of each, on stdout; a line per answer on stderr (outcome, source, signer, label). A slice of more than 100 members gives a summary, its first 100 members and the `calc` call that aggregates it |
| `calc <formula> [--asof <instant>] [--remote]` | a formula over cells, answered in one call with its lineage, on stdout. Here, the result is kept as the local function `local/calc_<h>` (`<h>` the first 16 hex digits of the SHA-256 of the canonical formula), so the same calculation is not run twice; with `--remote`, the forward mesh's `x_calc` evaluates it in a scope and nothing is kept |
| `define <name> --formula <source> --signature <json> [--tests <json array>]` | a formula function (also `--stored`, `--descriptor <file>`). A test's `inputs` may give a slice as `{"<slice call>": {"members": {}}}` (empty) or with a members map of cell calls to plain literals (since 0.5.2) |
| `define <name> --class <file>` | a class (16.1: keys, methods, order, steward), under `local/` only (owner `local.invalid`) |
| `publish <file.jsonl>` | `define` plus `put_version` for stored cells, under `local/` only (the `publish-stored` format); class lines too |
| `describe <id>` | a class name, a key encoding, an object id, a short id or a cell id (16.8): the answer on stdout, exit 0 for 200, 3 for anything else; a function's tests after it, read from its page `https://<host>/fn/<full name>?format=json` |
| `licence accept <licence>`, `licence list`, `licence revoke <licence>` | accept a restricted licence on this machine (see [Licence acceptance](#licence-acceptance)); the register's clauses and licence hash are shown first |
| `explain <request> [--depth n]` | the lineage, as nested envelopes |
| `find <text> [--remote]` | names held here (and the forward mesh's) |
| `prefetch <slice or function>... [--page n] [--blobs]` | pull a slice's pages (or a function's binding and descriptor), keys included; a short name works (`prefetch 'boe/bank_rate(*)'`) |
| `whatif <request> --set '<cell>=<value>' [--set ...] [--asof <instant>] [--json]` | a scenario (never a fact): the value a cell would have if some cells held other values, as the MCP tool `whatif` prints it (`Scenario (not a fact): ...`); `--json` prints the structured answer. Exit 0 for a scenario, 3 otherwise (the code on stderr) |
| `refresh` | re-pull the latest versions of what is held (`/versions` where a host lists it) |
| `export [--out file]`, `import <file> [--trust-key <key id>]` | move a working set between resolvers |
| `verify <file or ->` | SPEC 10.1 with this machine's keys |
| `serve [--listen 127.0.0.1:<port>]` | the localhost endpoint (127.0.0.1 only), with `POST /whatif` (the MCP tool's answer, for a local function over production's inputs) |
| `mcp [--profile small\|full]` | the MCP server over stdio; `full` (the default) is every tool, `small` is `find`, `get`, `calc` and `interval` with short descriptions, about 400 tokens (see [`docs/mcp-tokens.md`](docs/mcp-tokens.md)) |
| `key` | the machine id and this machine's (public) key document |

The MCP server (`datumesh mcp`) has the same reads and writes, and one tool more since 0.5.1:
`interval`, `{function, keys?, from, to}`, the forward mesh's members of a function over a range of
keys, each with its receipt (aliases resolved; over 100 members, a summary, the first 100 and the
call for the rest; the mesh's errors, such as `E_RANGE_TOO_LARGE`, passed through). No MCP tool
accepts a licence.

A request is a call, `name(arg: value, ...)` (positional by the function's order, an
`@alias`, a pin `name@<hash>(...)`, `?asof=<instant>`), or a cell URI. Since 0.2.1 a call may
be a method call of a class, `N(keys).m(args)` or `N[label]` (through the class's `lookup`),
with unquoted values by position type and a bare name through the alias map; it gives the same
cell and receipt as its plain read. The envelope then carries `resolved` (the canonical call,
each alias, method, default and lookup resolution, and the alias map's `tx`), and the stderr line
of `get` names the canonical call. A class is fetched from its forward mesh once, through its
bindings and its describe answer (checked against the class hash), and then answered from the
store. Exit status: 0 when every answer carries a receipt, 3 when one does not, 2 for a usage
error, 1 for a failure.
`--offline` (or `DATUMESH_OFFLINE=1`) fetches nothing; `--home <dir>` (or `DATUMESH_HOME`)
keeps configuration and data in one directory.

As a library:

```rust,no_run
use datumesh_local::{Config, Resolver};

fn main() -> Result<(), datumesh_local::Error> {
    let r = Resolver::open(Config::load()?)?;
    r.prefetch("ecb/fx_rate(base: *, quote: *, date: *)", None, false)?;
    r.set_offline(true);
    let reply = r.get(r#"ecb/fx_rate(base: "EUR", quote: "GBP", date: 2026-10-29)"#)?;
    assert_eq!(reply.status, 200);
    Ok(())
}
```

## Answers

`get` answers exactly as the mesh's `/cell` would, built by the mesh's own handlers over the
local store:

| The cell | The answer |
|---|---|
| held (fetched before, or local) | from the store, no request |
| a foreign stored cell not held | fetched once, checked, imported, then from the store (the same bytes from then on) |
| computed, every input open or local | evaluated here (foreign inputs fetched the same way), signed `local.invalid#<key id>` with the inherited licence, trust `computed`, kept |
| computed over a restricted input (any licence but 8.5's open ones, every `https://` one included) that the function's publisher declared | forwarded to `forward_mesh`; its answer (signed by that mesh) returned and kept |
| computed over a restricted input not declared | `unlicensed` (403) here, no request |
| foreign, not held, not fetchable (offline, network down, host not allowed) | `error` `E_FOREIGN_UNAVAILABLE` (HTTP 502), never `not_covered` |

`E_FOREIGN_UNAVAILABLE` is spec-0.4.3's: outcome `error`, HTTP 502, and a required `detail`,
one of `offline` (offline mode, or a host outside the allowlist), `network`, `dns`, `tls`,
`timeout`, `refused`, e.g.
`{"detail":"offline","error":"E_FOREIGN_UNAVAILABLE","outcome":"error"}`, built by
datumesh-core 0.3.7 (`Answer::foreign_unavailable`); the full failure is on stderr and in
the `Datumesh-Detail` header.

## `local.invalid`

Users write `local/…`; the alias map turns it into `local.invalid/…`, and every receipt and
envelope records the full name. `.invalid` is reserved (RFC 2606, RFC 6761): it never
resolves, so it can never be a real host (SPEC 2.5, spec-0.4.3). Each machine has its own
Ed25519 key, made at first run and kept with mode 0600 in the configuration directory, never
logged or printed; its 16-hex key id is the machine id. Local receipts are ordinary receipts
in the one receipt format. The resolver supplies its own key document for `local.invalid`,
so they verify here at `signed` and `recomputed`; elsewhere they stop at
`E_KEY_UNAVAILABLE`. Output labels them `local, verifies only on this machine`.

A function under a rooted namespace may not read a local one: `define` refuses it
(`Error::UnrootedInput`, code `E_UNROOTED_INPUT`; SPEC 2.5).

The host is configurable (`host` in `config.json`, default `local.invalid`; it must be a name
under `.invalid`). `local/` means `<host>/`, and receipts say `computed_by` `<host>#<key id>`.

## Key rotation and trusted machines

`datumesh key rotate` makes a new machine key from now. The old key file is renamed
`<key file>.retired-<old id>` (mode 0600, kept, never deleted), and its public entry, with
`valid_to` now, is appended to `retired-keys.json` in the configuration directory. This
machine's key document then lists the current key and every retired one, so receipts the old
key signed before the rotation still verify, and a receipt it signs after it fails with
`E_KEY_NOT_VALID`. A rotation is refused unless its instant is later than the current key's
`valid_from`. It prints the old key id, the new one and the instant as JSON.

Another machine is trusted with `datumesh trust add <key document file>`: its key document
(as its `key` command prints it) is kept in `trust.json` (mode 0600), with the instant it was
trusted. Its effective document, with every key's `valid_to` cut to the revocation, is kept
in the store under its host; this machine's own document never includes it. Receipts from a
trusted host verify and answer, labelled `trusted machine <host>`. A document for an
already trusted host replaces the old one only if it still lists every key trusted before
(same `id` and `public`, retired or not). `datumesh trust revoke <host>` ends the trust from
now: its earlier receipts still verify, later ones fail with `E_KEY_NOT_VALID`. `datumesh
trust list` shows the trusted machines. A formula may read a trusted machine's cells; reading
any other unrooted host is refused at define time (`Error::UntrustedInput`).

## Slices a read fetches

A local evaluation that reads a slice of a foreign function the local store does not hold completely
fetches the members it reads, as a prefetch would hold them: the whole slice in JSON pages (following
`next`, which production writes as `https://datumesh.com/slice/...`), or a lookup's range narrowed by its
bound through production's `/x/interval` (a ranged `/slice` is refused by production, so the range comes
from `/x/interval`). Every member is verified like any fetched cell, and nothing is kept if the evaluation
refuses.

The bound is `slice_fetch_max` in `config.json`: 50,000 members per evaluation, summed over the slices it
fetches. The default covers Bank Rate's whole history (13,118 members) and one currency of the ECB
references (about 7,000). Past the bound the evaluation answers `E_SLICE_FETCH_LIMIT`, naming the slice,
its size so far and the bound, and nothing is computed or stored; `prefetch` the slice to hold it first.
A second evaluation over a whole slice held this way fetches nothing.

## The localhost endpoint

`datumesh serve` binds 127.0.0.1 only (any other address is refused) and serves the mesh's
endpoints over the local store, `/cell` through the resolver (fetch, gate, forward) and
`/describe/{id}` as `describe` does. Its mesh document is the unrooted host's, and its URLs are
`http://127.0.0.1:<port>/...`: spec-0.5.0 (sections 9.6 and 2.5) lets an unrooted host (a name
under `.invalid`) use `http://` on a loopback address only (127.0.0.1, [::1], localhost); rooted
hosts stay https-only. The links in answers (a job's `poll`, a slice page's `next`,
`Link: rel="canonical"`) point at `https://local.invalid/...` and cannot be followed: `.invalid`
never resolves.

## The licence gate

Since 0.2.0 the gate is the engine's rule, licence inheritance (spec-0.4.3 8.5, with the
publisher's `declarations` from the key document of the function's namespace host, which the
resolver fetches and keeps on every import of a name and before every forward), plus one rule
of its own: a restricted licence is never signed by this machine's key, whose receipts verify
only here. So, for each input an evaluation reads:

- local, or under an open licence (CC0-1.0, PDDL-1.0, MIT, Apache-2.0, CC-BY-4.0, OGL-UK-3.0,
  CC-BY-SA-4.0, ODbL-1.0): computed here, the result carrying the inherited licence;
- restricted and declared by the function's publisher: forwarded, the publisher's mesh's
  receipt returned (for example `ecb/fx_cross` over the EUR/NOK test cell, and
  `scripture/strip_niqqud` over the UXLC, whose licence URL is restricted under 8.5);
- restricted and not declared: `unlicensed` (403) here, without a round trip.
- restricted and accepted here (below): computed here, the result carrying the accepted licence,
  or `LicenseRef-datumesh-local-derivation` over two or more.

The gate decides computation only. Restricted cells the mesh delivers are held like any other
(fetched, prefetched, exported): delivery is allowed, and the licence binds computation. When
per-principal delivery arrives (plan 3.4), a cell the mesh answers `unlicensed` never
arrives, so there is nothing to hold.

### Licence acceptance

Since 0.5.1 an operator can accept a restricted licence on this machine, at the command line:

```text
$ datumesh licence accept 'https://www.law.cornell.edu/uscode/text/17/105'
https://www.law.cornell.edu/uscode/text/17/105
register dataset treasury.gov/par_yield_curve
  licence hash e0f4…
  clauses:
    …
recorded: https://www.law.cornell.edu/uscode/text/17/105 accepted at 2026-11-24T10:00:00.000000Z
```

The register is read from the forward mesh's `register/dataset` class: every dataset whose licence
page is the licence is shown, with its quoted clauses and licence hash. A licence the register does
not hold is said so, and the acceptance is recorded anyway. The record is in `licences.json` in the
configuration directory: `{licence, accepted_at}`, with the register's `register_licence_hash` when
there is one. `licence list` shows the licences in force, `licence revoke <licence>` removes one and
keeps its history. A non-interactive run records without a prompt.

With the licence accepted, a calculation or a local function over that licence's inputs is computed
here. One restricted licence gives the result that licence; two or more give
`LicenseRef-datumesh-local-derivation`. The acceptance covers the licence from every publisher. The
result is signed by this machine's key, and it is **never published or exported**: `export`,
`root`, `publish --to`, `publish_public`, bundle export and the MCP `publish` refuse it with
`E_LICENCE_NOT_PUBLISHABLE`, naming the licence. `serve` and `mcp --http` stay on loopback.

No MCP tool accepts a licence. An agent that meets the refusal is told the user must accept the
licence at the command line. A running `mcp` or `serve` picks up an acceptance or revocation made
by another process.

## Export and import

`export` writes the working set as one JSON bundle (`datumesh-local/1`): names, descriptors,
every version of held cells, the receipts this machine answered with and their lineage,
blobs, forwarded answers, and every signer's key document, this machine's included. `import`
verifies every receipt again. Signed (rooted) receipts answer with identical bytes; a receipt
signed by another machine's local key is kept aside, labelled `unverifiable: signed by
another machine's local key`, unless you name that key: `--trust-key <key id>` adds it to
this machine's `local.invalid` key document, and its receipts verify and answer.
(Since 0.3.0, `--trust-key` trusts the host of the bundle's key document that carries the
key, as `trust add` does, and is refused for this machine's own host: see the section above.
The bundle's key document is never merged into this machine's.)

## Rooting

`datumesh root --namespace <ns> --key <file> [--export <file>] <name>...` republishes this
machine's names under a rooted namespace `<ns>` (for example `mesh-test.datumesh.com/scratch_1_8b`)
whose key this machine holds (CONTRACT-1.8b L5). The names (`local/...` or full) are the rooting
set; `local.invalid/x` becomes `<ns>/x`. The key file is read in place: mode 0600, a 64-hex seed
on its first line (or a machine key file). It is never written to the configuration, the store
or the output. The host's key document is fetched through the client (the allowlist or `peers`),
and the key's id must be in it with a window holding now.

- A stored function keeps its descriptor, so its hash is kept. The latest version of each held
  cell of it is re-signed by the namespace key: the same cell (rooted), `fn_hash`, arguments,
  `value`, `valid`, `licence` and `summary`, with `tx` = `asof` = `issued` = now, `publisher` the
  namespace, and the namespace's host as signer.
- A formula's inputs in the set are rewritten to the rooted names, so its hash changes. An input
  that is not in the set, under this machine's host, is refused (`E_UNROOTED_INPUT`).
- Computed cells are never copied or re-signed: they are listed under `computed`, as rooted calls,
  so the mesh can recompute them.

Without `--export` this is a dry run: it writes nothing and prints the plan as one JSON line
(`namespace`, `key_id`, `functions` with `from`, `to`, `from_hash`, `to_hash`, `kind`, `cells`,
`computed`). With `--export <file>` it also writes a bundle 1 (`datumesh_http::bundle::build`,
mode 0600, a new file) that `datumesh-mesh import-signed` imports. Checks run in order and the
first failure is reported with nothing written, not even the export file. Nothing is sent
anywhere except the key document request.

## Configuration

`config.json` in the configuration directory (`$XDG_CONFIG_HOME/datumesh`,
`~/Library/Application Support/datumesh`, `%APPDATA%\datumesh`), installed with comments at
first run, beside the machine key and the alias map; the store and the resolver's index in
the data directory (`$XDG_DATA_HOME/datumesh` on Linux). Members: `allowlist` (default
`datumesh.com` and the test mesh `mesh-test.datumesh.com`), `forward_mesh` (default `datumesh.com`;
a home made before 0.5.1 keeps its `mesh-test.datumesh.com`, and is told on every command), `offline`,
`licence` (the licence of receipts for cells without inputs), `peers`, `timeout_seconds`,
`store`, `key_file`, `aliases_file`, `slice_fetch_max` (the most members one evaluation fetches for the
slices it reads and does not hold, default 50000; see [Slices a read fetches](#slices-a-read-fetches)).
`open_licences`, the open list of 0.1.0 and 0.1.1, is
deprecated: still accepted, ignored (the gate needs no configuration).

The alias map (`aliases.json`) is a file, read at start and by `refresh`. Its default (0.5.1) is
`local/` → `local.invalid/`, and production's short names: `ecb/` → `datumesh.com/relay/ecb.europa.eu/`,
`treasury/` → `datumesh.com/relay/treasury.gov/`, `boe/` → `datumesh.com/relay/bankofengland.co.uk/`,
`scripture/` → `datumesh.com/scripture/`, `earth/` → `datumesh.com/earth/`. An existing home's file is
not rewritten; a short name its file lacks (`boe/` in a home made before 0.5.1, say) is taken from the
defaults in memory, and a short name the file has is the file's.

### Alias map file

The file is `{"entries": [{"short": ..., "full": ..., "tx": ...}, ...]}`. Each entry says that
`short` stands for `full` from the instant `tx` on (a lexical instant, SPEC 3.4).

- A `short` ending in `/` is a prefix entry, and its `full` ends in `/` too:
  `{"short": "ecb/", "full": "mesh-test.datumesh.com/ecb/", "tx": "2026-11-10T00:00:00.000000Z"}`.
  A call `ecb/fx_rate(...)` names `mesh-test.datumesh.com/ecb/fx_rate(...)`.
- A bare `short` (no `/`) may stand for any full name. A request (`get`, `prefetch`, `describe`)
  that resolves a bare name to a class (a class is bound at that name) not under
  `datumesh.com/std/` is refused with a local error (`Error::Request`; the message names
  `datumesh.com/std/`). It is not an envelope: a define code is not an answer's error. A bare
  name for a function (not a class) is not limited.
- A request is resolved at its asof (`?asof=`, else now): for each short name, the entry with
  the greatest `tx` not after that instant. An entry with a later `tx` does not exist yet.
  The answer's `resolved.aliases_tx` is the greatest `tx` among the entries that applied.
- A formula source may write a prefix alias as `@ecb/fx_rate(...)`: it is expanded to the full
  name before the formula is defined, so its hash is that of the source written in full. A
  bare `@name` is looked up among the bare entries.
- The older shape, `{"<short>": "<full>", ..., "tx": "<instant>"}`, is still read: every entry
  gets that `tx`.

Step 2.2 generates this file from the cells `datumesh.com/std/default(name: "<short>")`: each
cell's value is the entry's `full`, and its receipt's `tx` is the entry's `tx`.

## wasm

With `--no-default-features --features wasm`, the crate is the core's verification and the
call parser for `wasm32-unknown-unknown` (`verify`, `parse_call`; JSON in and out, through
`wasm-bindgen`), without the engine. `tools/wasm_smoke.sh` builds it and runs a Node test.

## Versions

datumesh-local 0.3.1 fixes `publish` across file systems; 0.3.0 added the MCP server (stdio and Streamable HTTP, MCP 2026-07-28), key
rotation, a host per machine and trusted machines, rooting into bundle 1, alias entries with
`tx`, and rule A, on datumesh-core 0.3.8, datumesh-engine 0.4.2, datumesh-formula 0.3.0,
datumesh-http 0.3.0 and datumesh-store 0.4.2 (spec-0.5.0). 0.2.2 (0.2.1 was tagged, not
published) followed spec-0.5.0 (classes in requests, describe, the loopback mesh document).
0.2.0 followed spec-0.4.2 (with spec-0.4.3's `E_FOREIGN_UNAVAILABLE` and unrooted namespaces). MSRV 1.97.

## License

Apache-2.0.
