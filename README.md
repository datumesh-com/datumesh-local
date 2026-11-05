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
- **A licence gate.** A function evaluates here when every input is open-licensed or local;
  otherwise the request goes to the configured mesh, and its answer (its signature) is
  returned.

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
| `publish <file.jsonl>` | `define` plus `put_version` for stored cells, under `local/` only (the `publish-stored` format) |
| `explain <request> [--depth n]` | the lineage, as nested envelopes |
| `find <text> [--remote]` | names held here (and the forward mesh's) |
| `prefetch <slice or function>... [--page n] [--blobs]` | pull a slice's pages (or a function's binding and descriptor), keys included |
| `refresh` | re-pull the latest versions of what is held (`/versions` where a host lists it) |
| `export [--out file]`, `import <file> [--trust-key <key id>]` | move a working set between resolvers |
| `verify <file or ->` | SPEC 10.1 with this machine's keys |
| `serve [--listen 127.0.0.1:<port>]` | the localhost endpoint (127.0.0.1 only) |
| `key` | the machine id and this machine's (public) key document |

A request is a call, `name(arg: value, ...)` (positional by the function's order, an
`@alias`, a pin `name@<hash>(...)`, `?asof=<instant>`), or a cell URI. Exit status: 0 when
every answer carries a receipt, 3 when one does not, 2 for a usage error, 1 for a failure.
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
| computed, every input open or local | evaluated here (foreign inputs fetched the same way), signed `local.invalid#<key id>`, trust `computed`, kept |
| computed over an input not open | forwarded to `forward_mesh`; its answer (signed by that mesh) returned and kept |
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
endpoints over the local store, `/cell` through the resolver (fetch, gate, forward). The mesh
document it serves is **local-only**: its URLs are `http://127.0.0.1:<port>/...`, which
spec-0.4.3 (section 9.6) does not allow, since mesh-document URLs are `https://`. It is not
valid under spec-0.4.3 until spec-0.4.4, which is to say: "for an unrooted host (a name under
.invalid), mesh-document URLs and poll may be http:// on a loopback address only (127.0.0.1,
[::1], localhost); rooted hosts stay https-only". The links in answers (a job's `poll`, a
slice page's `next`, `Link: rel="canonical"`) point at `https://local.invalid/...` and cannot
be followed: `.invalid` never resolves.

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
`datumesh.com` and the test mesh `mesh-test.datumesh.com`), `open_licences` (CC0-1.0,
CC-BY-4.0, CC-BY-SA-4.0, ODbL-1.0, OGL-UK-3.0, PDDL-1.0, Apache-2.0, MIT,
`https://tanach.us/License.html`), `forward_mesh`, `offline`, `licence`, `peers`,
`timeout_seconds`, `store`, `key_file`, `aliases_file`.

The alias map (`aliases.json`) is a file, read at start and by `refresh`: one JSON object
mapping short names and prefixes to full names, plus `tx`, the instant it was taken. The
default is `local/` → `local.invalid/`, `ecb/` and `scripture/` → the test mesh's.

## wasm

With `--no-default-features --features wasm`, the crate is the core's verification and the
call parser for `wasm32-unknown-unknown` (`verify`, `parse_call`; JSON in and out, through
`wasm-bindgen`), without the engine. `tools/wasm_smoke.sh` builds it and runs a Node test.

## Versions

datumesh-local 0.1.1 follows spec-0.4.2 (plus spec-0.4.3's `E_FOREIGN_UNAVAILABLE` and
unrooted namespaces), on datumesh-core 0.3.7, datumesh-engine 0.4.0, datumesh-formula 0.2.0,
datumesh-http 0.2.0 and datumesh-store 0.4.0. MSRV 1.97.

## License

Apache-2.0.
