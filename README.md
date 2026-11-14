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
  machine's key; over an undeclared one the answer is `unlicensed`, here.

## Use

```text
$ cargo install datumesh
$ datumesh get 'ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)'
{"fn_hash":"f041cf…","outcome":"ok","receipt":{…},"trust":"primary"}
ok (fetched); signer mesh-test.datumesh.com#5ef994cc05faf506
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
| `get <request>...` | the envelope (SPEC 5.5) of each, on stdout; a line per answer on stderr (outcome, source, signer, label) |
| `define <name> --formula <source> --signature <json>` | a formula function (also `--stored`, `--descriptor <file>`) |
| `define <name> --class <file>` | a class (16.1: keys, methods, order, steward), under `local/` only (owner `local.invalid`) |
| `publish <file.jsonl>` | `define` plus `put_version` for stored cells, under `local/` only (the `publish-stored` format); class lines too |
| `describe <id>` | a class name, a key encoding, an object id, a short id or a cell id (16.8): the answer on stdout, exit 0 for 200, 3 for anything else |
| `explain <request> [--depth n]` | the lineage, as nested envelopes |
| `find <text> [--remote]` | names held here (and the forward mesh's) |
| `prefetch <slice or function>... [--page n] [--blobs]` | pull a slice's pages (or a function's binding and descriptor), keys included |
| `refresh` | re-pull the latest versions of what is held (`/versions` where a host lists it) |
| `export [--out file]`, `import <file> [--trust-key <key id>]` | move a working set between resolvers |
| `verify <file or ->` | SPEC 10.1 with this machine's keys |
| `serve [--listen 127.0.0.1:<port>]` | the localhost endpoint (127.0.0.1 only) |
| `key` | the machine id and this machine's (public) key document |

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
(`Error::UnrootedInput`). No spec code names this refusal (SPEC 2.5 calls it define's).

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

The gate decides computation only. Restricted cells the mesh delivers are held like any other
(fetched, prefetched, exported): delivery is allowed, and the licence binds computation. When
per-principal delivery arrives (plan 3.4), a cell the mesh answers `unlicensed` never
arrives, so there is nothing to hold.

## Export and import

`export` writes the working set as one JSON bundle (`datumesh-local/1`): names, descriptors,
every version of held cells, the receipts this machine answered with and their lineage,
blobs, forwarded answers, and every signer's key document, this machine's included. `import`
verifies every receipt again. Signed (rooted) receipts answer with identical bytes; a receipt
signed by another machine's local key is kept aside, labelled `unverifiable: signed by
another machine's local key`, unless you name that key: `--trust-key <key id>` adds it to
this machine's `local.invalid` key document, and its receipts verify and answer.

## Configuration

`config.json` in the configuration directory (`$XDG_CONFIG_HOME/datumesh`,
`~/Library/Application Support/datumesh`, `%APPDATA%\datumesh`), installed with comments at
first run, beside the machine key and the alias map; the store and the resolver's index in
the data directory (`$XDG_DATA_HOME/datumesh` on Linux). Members: `allowlist` (default
`datumesh.com` and the test mesh `mesh-test.datumesh.com`), `forward_mesh`, `offline`,
`licence` (the licence of receipts for cells without inputs), `peers`, `timeout_seconds`,
`store`, `key_file`, `aliases_file`. `open_licences`, the open list of 0.1.0 and 0.1.1, is
deprecated: still accepted, ignored (the gate needs no configuration).

The alias map (`aliases.json`) is a file, read at start and by `refresh`: one JSON object
mapping short names and prefixes to full names, plus `tx`, the instant it was taken. The
default is `local/` → `local.invalid/`, `ecb/` and `scripture/` → the test mesh's.

## wasm

With `--no-default-features --features wasm`, the crate is the core's verification and the
call parser for `wasm32-unknown-unknown` (`verify`, `parse_call`; JSON in and out, through
`wasm-bindgen`), without the engine. `tools/wasm_smoke.sh` builds it and runs a Node test.

## Versions

datumesh-local 0.2.2 (0.2.1 was tagged, not published) follows spec-0.5.0 (classes in requests, describe, the loopback mesh
document), on datumesh-core 0.3.8, datumesh-engine 0.4.2, datumesh-formula 0.3.0, datumesh-http
0.2.2 and datumesh-store 0.4.1. 0.2.0 followed spec-0.4.2 (with spec-0.4.3's
`E_FOREIGN_UNAVAILABLE` and unrooted namespaces). MSRV 1.97.

## License

Apache-2.0.
