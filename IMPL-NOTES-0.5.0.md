# datumesh-local / datumesh spec-0.5.0 (classes in requests, describe, loopback): notes for the implementer

For `/root/datumesh-libs/CONTRACT-local-0.5.0.md`. The tests are written (one commit per repo, on `main`);
yours are the implementation commits. This file is committed with the datumesh-local tests commit and covers
both repos. The contract's limits hold: do not push, tag, publish or deploy; do not touch the running mesh
containers or their volumes, `/root/crates-api-token` or `/root/cf-key`; remove build directories you
create (`target-0.5.0/` in each repo: about 6 GB for datumesh-local, 1.5 GB for datumesh).

## 0. How to build

* **Dependencies** are unpublished. The tests commit raises the minima in `Cargo.toml` (core 0.3.8, formula
  0.3.0, store 0.4.1, engine 0.4.2, http 0.2.2). Locally an uncommitted `.cargo/config.toml` (excluded through
  `.git/info/exclude`; already in both working trees) patches them to paths inside the container:

  ```toml
  [patch.crates-io]
  datumesh-core = { path = "/core/crates/datumesh-core" }       # core 0.3.8, 6780c7c
  datumesh-store = { path = "/store/crates/datumesh-store" }    # store 0.4.1, db69efc (step-1.5-namespace-validation)
  datumesh-engine = { path = "/engine" }                        # engine 0.4.2, eb727b7
  datumesh-formula = { path = "/formula" }                      # formula 0.3.0, b6dd484
  datumesh-http = { path = "/http" }                            # http 0.2.2, a170092 (step-1.5-domain-validation)
  # the datumesh repo's file also has:
  datumesh-local = { path = "/libs/datumesh-local" }            # the sibling checkout
  ```

* **The wrapper** is `/root/datumesh-libs/cargo-local-0.5.0.sh <datumesh-local|datumesh> <cargo args...>` (outside
  both repos, not committed). It is `tools/cargo.sh` with the mounts, read-only: core `/core`, store `/store`,
  engine `/engine`, formula `/formula`, http `/http`, datumesh-local `/libs/datumesh-local`; the repo itself at
  `/w`, `CARGO_TARGET_DIR=/w/target-0.5.0`, the `datumesh` network, the shared registry cache. `IMAGE=` overrides
  the image: `IMAGE=datumesh-rust-1.99-lint` for `fmt` and `clippy` (the plain `rust:1.99` has neither). The
  script is:

  ```sh
  L=/root/datumesh-libs
  docker run --rm -i --network datumesh \
    -v "$HOME/.cache/datumesh-cargo:/usr/local/cargo/registry" -v "$L/$repo:/w" -w /w \
    -v "$L/datumesh-core:/core:ro" -v "$L/datumesh-store:/store:ro" -v "$L/datumesh-engine:/engine:ro" \
    -v "$L/datumesh-formula:/formula:ro" -v "$L/datumesh-http:/http:ro" -v "$L/datumesh-local:/libs/datumesh-local:ro" \
    -e CARGO_TARGET_DIR=/w/target-0.5.0 -e CARGO_TERM_COLOR=never -e UPSTREAM_DATABASE_URL \
    "${IMAGE:-rust:1.99}" cargo "$@"
  ```

  (`datumesh-http` has uncommitted changes in its `Cargo.lock` only; the mount is fine.) `tools/wasm_smoke.sh` and
  `tools/public_api.sh` take `DATUMESH_PATCH=<dir with config.toml>` and mount only that: for them, give a
  directory whose `config.toml` is the patch above and add the mounts to a copy of the wrapper.
* `Cargo.lock` is not committed with the tests (a build rewrites it for the patches): `git checkout Cargo.lock`
  before committing; locks are fixed at release time.
* **Run the tests:** `../cargo-local-0.5.0.sh datumesh-local test --features wasm --no-fail-fast`. The
  `wasm_classes` tests are `#![cfg(feature = "wasm")]` (they call `datumesh_local::wasm::parse_call` natively);
  without the feature they are an empty target. SQLite only: the upstream of the tests is the datumesh-http
  library over SQLite (`UPSTREAM_DATABASE_URL` still switches it to PostgreSQL, as before: run both).
* Formula 0.3.0 compiled with no change to the library (the tests commit changes `Cargo.toml` and adds two
  stubs, below, only).

## 1. What fails now

datumesh-local, `cargo test --features wasm`: **18 fail**, 41 old tests pass, 2 new ones pass.

| File | Tests | Fail now | Item |
|---|---|---|---|
| `tests/classes.rs` | 8 | 8 | 1 requests (6 tests: equivalence, fetch-once-then-offline, `resolved` and defaults and aliases, labels, explain, malformed), 5 local classes (2) |
| `tests/classes_describe.rs` | 7 | 7 | 2 describe: class, object/id/short id/cell id, refusals, local classes, the command, `define --class`, `serve` |
| `tests/wasm_classes.rs` | 4 | 3 | 6 WASM (the 4th, the old environment, is a guard and passes) |
| `tests/loopback.rs` | 1 | 0 | 3 loopback: **passes already** (core 0.3.8's `resolve_mesh` accepts the local document); a guard |

datumesh (binary), `cargo test`: **2 fail** (`tests/classes.rs`), the 2 old tests pass.

Items 4 (codes pass through) and 7 (L1 C gate, L2 i) need no test of their own here: item 7 is the old
`gate`, `inheritance` and `offline` tests, which pass; item 4 is exercised through describe
(`describe_refusals_pass_through`: 404, `bad_request` `E_KEY_ENCODING`). The documentation (the exit-status
table, README, CHANGELOG, the "local-only until spec-0.5.0" notes in `lib.rs`, `serve.rs` and the README) is
yours.

## 2. The API the tests call (stubs are in the tree: `todo!()`)

```rust
impl Resolver {
    /// `local/` only; owner `local.invalid`: define_class, put_class, bind_class from now.
    /// `Defined { name: "local.invalid/pair", fn_hash: <the CLASS hash>, bound }`.
    pub fn define_class(&self, name: &str, descriptor: &[u8]) -> Result<Defined, Error>;
    /// id: class name (short names through the alias map), cell id, object id, short id, key encoding.
    pub fn describe(&self, id: &str) -> Result<Reply, Error>;
}
```

* `Reply::source` of a describe: `Source::Local` from local classes; `Source::Forwarded { mesh }` (the forward
  mesh's host) for the mesh's answer. `status` 200 / 404 / 400 as the mesh gave it; the body is the mesh's, unchanged.
* `Resolver::publish` accepts `{"class": "local/pair", "descriptor": {...}, "bound": ...}` (the http crate's
  `Published.classes` counts newly bound classes); a class line not under `local/` is `Error::Request`.
* `define_class` outside `local/`: `Error::Request` or `Error::Define` (the test accepts either) and nothing stored.
* CLI: `datumesh describe <id>` (exit 0 for 200, 3 for anything else; usage 2), `define <name> --class <file>`,
  both in the usage text; stdout of `describe` is the body plus a newline, like `get`.
* WASM: `parse_call(text, env)`'s `env` accepts `classes`, `now`, `lookups`, `aliases_tx` (formula 0.3.0's
  `MapCallEnv::from_value` already reads them: the call parses and gives the same `uri` and `reference` as the
  plain read today); what is missing is the member **`resolved`** in the result (`{"call", "resolutions":
  [{"from","kind","to","asof"?}], "aliases_tx"?}`, as the envelope has it; absent when `Call::resolved` is `None`).
* Public API: all of the above is additive, but formula 0.3.0's types in the public API may still make the
  release 0.3.0; `cargo public-api` decides (contract, Versions).

## 3. Behaviour the tests pin (read the tests; the notes are the short list)

* **Equivalence.** `N(k).m(a)` and the plain read give the same cell id, byte-identical receipt and the same
  envelope apart from `resolved` (`without_resolved` in `tests/common/mod.rs`); a plain read has no `resolved`.
  Spellings tested: quoted, unquoted by position type, labelled (`corpus: quran, ...`), `.m(edition: uthmani)`,
  the alias map's prefixes (`scripture/verse(...)`, `ecb/fx_pair(...)`).
* **Environment of `parse_call`.** Classes, method functions and lookup cells come from the local store or, when
  not held, from the forward mesh (the bindings resource of 9.7 answers a class name with `class_hash` and its
  methods; `datumesh-http`'s client has no class method yet: add one in the local crate or use `get`). What is
  fetched is kept: the second call, `set_offline(true)`, is answered from the store with the same bytes
  (`Source::Local`); a never-fetched class offline is `E_FOREIGN_UNAVAILABLE` (502), not a 404.
* **`now`** is the resolver's clock (`Shared::now`); the date default is the `asof`'s date (the call's, else now's,
  UTC). A lookup is read as of the call's `asof` (else now); `resolved.resolutions[0].asof` carries it.
* **`resolved`** order: alias, lookup, method, default. `aliases_tx` is the alias map file's `tx` and is present
  exactly when a bare name/`@alias` was resolved through the map. (Open: whether the map's *prefix* entries
  such as `ecb/` count as an alias resolution in `resolved`. The tests do not say: they check `aliases_tx` only
  for the bare `xcross`.)
* **A malformed call** (a lexical failure) is `bad_request` (400) and reaches no network (the test runs offline and
  expects 400, not 502).
* **explain** answers from the store as before; it takes the call form too (the class must be held: `get` first).
* **Known limitation, not in scope:** `get "cell://<fn>?args"` of a function never fetched is `E_INPUT_UNAVAILABLE`
  (503) today; the new tests use call text for plain reads.
* **describe** checks the answer with core's `check_describe_with(body, &held, &functions)`, `held` the class
  descriptors the local store holds (fetch the class descriptors the answer names, as for a call) and `functions`
  their methods' descriptors; a check that fails is a client error, not a pass-through. `serve` serves
  `/describe/{id}` (local classes, else forwarded) and its mesh document lists it at
  `http://127.0.0.1:<port>/describe/{id}`.
* **Local class descriptor** (the tests' shape): keys `base`, `quote` (`key_normalisation: none`), method `rate`
  (a local stored function defined first), `steward` `{namespace: local.invalid, process:
  https://local.invalid/classes/change}`. If core refuses that steward, say so: it is a test fixture to adjust,
  not a requirement. The class is written with a short name (`local/pair`) everywhere: expand it through the alias
  map before core sees it (`publish` of function lines already does; a class line with `local/pair` failed with
  `E_CANON_NAME` in the old tree).

## 4. The test world

`tests/common/mod.rs` (`upstream_classes()`) publishes `tests/data/classes-functions.jsonl` (the method
functions, bound 2026-02-01), then three classes bound 2026-02-02 (written at run time: their descriptors name
the functions' hashes), then `tests/data/classes-cells.jsonl` (cells at 2026-03-02T16:00): `verse`
(`text` stored with a blob, `consonantal` a formula), `fx_pair` (`rate` stored, `cross` a formula with
`"default": "asof"` on `date`), `fx_named` (`rate`, `named`, the `lookup`; `cable` is `GBP/USD`). `ASOF` is
2026-03-02T18:00, `DAY` 2026-03-02. `held_classes(&up)` gives core's describe check what a client holds.
Not tested, on purpose: `E_SHORT_ID_AMBIGUOUS` (the datumesh-http test finds a 48-bit collision for its own
class hash with a minutes-long search; the local world's hashes differ: a pass-through, like 404, if you want a
test, build one with that tool), `E_KEY_NORMALISATION` (no class here has a rule), `E_NAMESPACE_SUSPENDED`,
`E_FORMULA_KEY` and `E_FORMULA_DISPATCH_KEY` (they pass through the existing `not_covered` mapping, as in
datumesh-http's `classes_faults` guards).

## 5. The binary

`datumesh/tests/classes.rs` runs the real binary offline over a local class: `--help` names `describe` and
`--class`; `define ... --class`, `describe local/pair` (class, object, short id), `publish` of a cell, `get` of
the method call (stdout has `"resolved":{`, stderr has the canonical call `local.invalid/pair/rate(base: "EUR",
date: 2026-03-02, quote: "USD")`) and the plain read (same `hash`, no `resolved`). The binary has no code of its
own: its work is the version, the dependency on datumesh-local, the CHANGELOG and README.
