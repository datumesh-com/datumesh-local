# Changelog

## Unreleased (0.5.5)

The agent tools (CONTRACT-patch-0.6.1 item 8a.8: NEXT-PATCH-QUEUE items 17 to 27 and the 4 KiB small profile).
Tests first: `tests/l055_agent_tools.rs`; readings in CONTRACT-local-0.5.5.md.

- **get's description points to many dates.** Both profiles' `get` descriptions say that many dates of one series are read
  with `interval`, or with a calc over a slice (a key written as `*`). When a get reads a function at a third date or later,
  the answer adds one line saying so.
- **Interval and slice answers are compact rows.** Each member is its call, value and receipt hash (and `trust` for a slice).
  The keys and the as-of are no longer repeated in every row.
- **Dates in get.** A quoted canonical date (`"2026-10-30"`) and `date(2026, 10, 30)` read the same cell as `2026-10-30`, in
  `get` (the CLI too). `calc` still refuses a quoted date, as the formula's type rule does; a non-canonical date is still refused.
- **Unlicensed answers.** The answer names the `licence accept` command as before, and says that with `remote` the calculation
  runs on the mesh and needs no licence.
- **Failed reads say why.** A missing cell is named with its nearest dates and the `last_before` lookup that reads the nearest
  value. A wrong key value lists the values the key takes. A name the mesh does not hold says so and points to find; a guessed
  name gets the near names in its namespace. An unqualified function in calc gets its full name from find.
- **A label is not read as a call.** Text with parentheses that is neither a call nor a reference is refused as a label (E_LABEL)
  for `get` and `describe`, with the find mesh's top matches; it is not run as a find.
- **A refused long interval says why.** An interval over more than 10 years is refused with the most it takes and the first
  page's arguments, from the same start to ten years on.
- **The small profile is about 4 KiB an answer.** Its find lists 10 results (with the total and descriptions cut to 100
  characters); its slices and intervals give pages of 15 members; its calc gives 8 lineage entries, the formula once and no
  pointer to explain; its get inlines a text of up to 1.5 KiB, once, and otherwise gives the text's address. Its `calc` declares
  `remote`, and its `interval` declares `keys`. The full profile keeps its 64 KiB budget and its 100 members and 20 lineage entries.

## 0.5.4

The client side of CONTRACT-patch-0.6.1 item 4c. Stays on datumesh-http 0.5.5 (no change to its public API).

- **The client follows a job (4c).** When a mesh answers `202 pending` with a `poll` URL, the cell's fetch (`fetch_cell`, a
  foreign stored cell) and the forward to the forward mesh (`forward`) poll that URL through the configured peers, waiting the
  answer's estimate clamped to 1 to 10 s before each poll, until the job answers. The original `/cell` request is not sent
  again while the job is known. On `E_JOB_UNKNOWN` (a mesh restart) the original request is sent once more; a second
  `E_JOB_UNKNOWN` ends the wait with that answer. At the client's own `timeout` (from the first request) the last `202` is
  returned as it came. A poll's answer is checked against the request the job answers, as a cell's is.

## 0.5.3

The guard patch's part A, agent answers (CONTRACT-guard-patch.md, A1 to A5; the user's items 12 to 14, 37 and the usage line).

- **One lineage entry per slice read (item 12).** A calc's `lineage` gives a slice read one entry, `{entry, slice, members,
  members_hash}`: the receipt's slice input, the slice as the mesh's explain gives it, the member count and the members hash. Its
  members are no longer listed. A cell keeps `{call, receipt, publisher, trust}`. A calc's answer also names its own `call` and
  `receipt_hash`.
- **The short answer (item 13).** The text part of `get`, `calc`, `find`, `explain`, `whatif`, `interval`, `verify` and `describe` is a
  short statement of the answer, never the JCS of the structured content. `get` and `calc` give the value and its unit, the as-of,
  the signer (`get`) or the call and receipt's hash, in under 1 KiB. The structured content keeps the envelope. For `get`, the items
  before the last (a reference's label and call, a text blob's checked text, a blob's note) are as they were.
- **Budgets on tool answers (item 14).** A calc's lineage is cut to its first 20 entries, with `lineage_total` and an `explain` pointer
  (`{request, page, limit}`) to the next page. `explain` pages its inputs with `page` (from 1) and `limit` (default 20, at most 40);
  the node gives `inputs_total`, `page` and `limit`. A slice `get` gives 100 members at a time, each with its `call`, `value` and
  `receipt_hash` (not the whole receipt), the total as `summary.count`, and a `next` pointer (`<slice>?page=<n>`, which `get` takes).
- **`notifications/tools/list_changed` (item 37).** `initialize` and `server/discover` advertise `tools.listChanged: true`. When a
  retry of the catalogue summary succeeds on stdio, the server sends the notification once, after the answer to the request that
  retried. The `find` description then carries the held sentence (and, in the small profile, the subjects' labels). A summary fetched
  at start changes nothing.
- **Nested explain (item 14, 25 Nov).** `explain` cuts every nested `inputs` list to 20, at every depth up to 16, with
  `inputs_total` and an `explain` pointer (`{request, page: 2, limit: 20}`) to the rest. Nested inputs are slim: an input
  with inputs gives its call, value and receipt hash; a leaf gives its receipt hash alone (its call and value would not fit
  the 64 KiB budget at limit 40 over the nested fixture). The full envelope is only at the top.
- **Interval members (item 14).** A listed member carries its `call`, `value`, `receipt_hash`, `keys` and `asof`, not the
  whole receipt; the receipt is still verified, and the answer still lists the first 100.
- **`describe` of a catalogue root.** `describe <host>/catalog` (`kind: catalog`, spec-0.7.0 C7) is checked as a catalogue: its host
  and its domains. It was refused as an answer that names no class.
- **The usage line (item 12).** `datumesh --help` lists every command the dispatcher takes: whatif, mcp, licence, trust, root and allow
  were missing.

Fixes before the 0.5.3 release (datumesh-local 0.5.3, the three defects of REPORT-c-codes-and-small-profile.md).

- **A failed answer says its code, and is an error.** A failed `get`, `explain` or `describe` (status 400 or more) sets `isError`,
  and its text is `CODE: detail` as the other tools give it: the code, then every detail the structured content carries (`error`,
  `fault`, `detail`, `reason`, `outcome`) and, for `get`, the resolver's detail (such as "offline: the resolver is offline"). It was
  the status or the echoed id alone. The structured content keeps the envelope and the status.
- **A slice `get` is bound by `slice_fetch_max`.** A `get` of a foreign slice that the store does not hold is counted page by page as
  calc's read is: the first page over the bound is refused with `E_SLICE_FETCH_LIMIT` (the slice, its size so far, the bound and
  `prefetch`), and nothing it fetched is kept. A slice within the bound is kept as before; `prefetch` stays unbounded.
- **A foreign slice's `next` page is read.** `get <slice>?page=<n>` for a foreign slice (and a local one) answers that page, as `next`
  names it. It answered `E_PARSE_URI` before any fetch.

On the guard releases (CONTRACT-guard-patch.md, part G): datumesh-store 0.6.5, datumesh-engine 0.6.2 and datumesh-http 0.5.5 are the
minimums.

- **Recorded time and backdating.** Each stored version carries the time it was recorded; a version whose signed time is not later than
  its cell's latest is refused (`E_BACKDATED`). A read that names an asof answers by recorded time.
- **A read that names an asof stores nothing.** It is evaluated and signed each time; only a read with no asof is stored. Two such reads
  state the same answer and differ in `issued` and `sig`.
- **A slice read keeps a member-set row, not its manifest.** A calc's lineage entry for a slice comes from that row.
- **A bundle carries every slice manifest its receipts read.** A manifest that is not stored is rebuilt as of the asof of the receipt
  that cites it. An export that cannot rebuild one fails, naming the slice; it left the manifest out before.
- **SQLite:** a read waits behind a long write in another process (the store's barrier).

## 0.5.2

Protocol round, Part 1 (CONTRACT-next-patch, items 12, 13, 14 and 18; datumesh-local 0.5.2 with datumesh-engine 0.6.1).

- **Slices fetched on demand (item 12).** A local evaluation that reads a slice of a foreign function the store does not hold
  fetches the members it reads, held as `prefetch` holds them. A whole slice comes in JSON pages, following `next` (production's
  `https://datumesh.com/slice/...` links; the client follows only `https://<host>/` links). A lookup narrowed by its bound
  (`last_before(f(*), x, b)` reads `[x - b..x]`) fetches that range through production's `/x/interval?from&to`, since production
  refuses a ranged `/slice` with `E_PARSE_URI`. Every member is verified. A second evaluation over a whole slice held this way
  fetches nothing.
- **The bound (item 12).** `slice_fetch_max` in `config.json`, default 50,000 members per evaluation, summed over the slices it
  fetches; a new home's file states it. Past it the evaluation answers `E_SLICE_FETCH_LIMIT`, naming the slice, its size so far and
  the bound, with `prefetch` as the way to hold it first. Nothing it fetched is kept, and nothing is computed. Stated in
  `datumesh --help`, the README and the configuration.
- **`datumesh whatif` (item 13).** `whatif <request> --set '<cell>=<value>' [--set ...] [--asof <instant>] [--json]`, printing as the
  MCP tool does (`Scenario (not a fact): ...`, then one `with <cell> = <value>` line per override); `--json` prints the structured
  answer. Exit 0 for a scenario, 3 otherwise, with the tool's code on stderr. The value is typed by the overridden cell's type.
- **`POST /whatif` on `datumesh serve` (item 13).** The body of SPEC 5.9 (`cell`, `asof`, `overrides`, `solve`) is answered as the MCP
  tool answers it: `{"outcome": "scenario", "scenario": {...}}`, or the refusal with its code in `error` (503
  `E_EVAL_UNAVAILABLE` before this version, for a local function over production's inputs).
- **`define --tests` takes slice inputs (item 14).** A case's `inputs` may give a slice as `{"<slice call>": {"members": {}}}` (empty)
  or with a members map of cell calls to plain literals, typed by the function's return type. A plain literal given for a slice,
  a member that is not a cell of it, or any other shape is refused at define, and nothing is defined. `root --export` carries the
  inputs unchanged.
- **The agent tool surface (item 18).** `datumesh mcp --profile small|full` (`full`, the default, is unchanged). `small` lists
  `find`, `get`, `calc` and `interval` with one- or two-sentence descriptions and minimal schemas, and instructions of two sentences
  and the mesh's held sentence: about 380 tokens on the approximation. `tools/mcp_tokens.py` counts each definition's tokens (with
  `ANTHROPIC_API_KEY`, through the count_tokens endpoint; without it, approximately, the method named), and `docs/mcp-tokens.md`
  is its report for both profiles.
- **Prefetch and calc of a short name (fixed).** A home whose `aliases.json` predates `boe/` (made before 0.5.1, when the defaults
  gained it) failed `prefetch 'boe/bank_rate(*)'` with `E_FOREIGN_UNAVAILABLE: not allowed: boe is not in the allowlist`, because
  `boe` was read as a host. The defaults' short names the file lacks are now taken from the defaults in memory; the file is not
  rewritten, and a short name the file has is its own.
- **Fixed.** A `calc` over a slice read named its slice's call (`bank_rate(*)`) where a function's name belongs, and asked production
  for a binding of that name. The client's slice rest is the canonical URI form, as `prefetch` holds it.
- **`datumesh allow add|list|remove <domain>` (item 30).** Manages the CLI's allowlist in `config.json`: only the
  `allowlist` array's text is rewritten, so every comment line is kept, and the file is read back to check it. Add is
  idempotent; removing a domain that is not there says so, exit 0, and changes nothing. A domain that is a URL, a path, has
  an uppercase letter or a trailing dot is refused (exit 2) and the file is not touched. No MCP tool does this.
- **A `peers` origin must be `http://` or `https://` (item 31).** Any other origin is refused when the configuration loads,
  naming the entry, its host and the origin.
- **Local `calc` takes its result type from the body (item 33).** A text, date, instant or boolean result is answered with
  its type and value, not refused with `E_CANON_UNIT`: a non-number's signature `returns` is its type name, as SPEC 7.1
  writes it. Numbers are answered as before, with their unit. (`/x/calc` on the forward mesh is datumesh-http's, not this
  crate's.)
- **The installed `config.json` says the forward mesh is asked too with `--remote` (item 32).** The comment above
  `find_meshes` (`Resolver::find_in` asks `forward_mesh` whenever `remote` is true).
- **What the mesh holds is carried once (item 36).** The held sentence is in the instructions only, in both profiles; it is no
  longer appended to the `find` and `get` descriptions.
- **No silent fallback (item 37).** When the catalogue summary (`GET /find?summary=1`) cannot be fetched, the instructions say so in
  one sentence with the reason: "What this mesh holds could not be fetched (<reason>); find still works." The summary is tried
  again on the next `find`, `get`, `tools/list`, `server/discover` or `initialize` once a minute has passed on the resolver's
  clock; after a success the real text is carried and no further summary request is made. (`notifications/tools/list_changed`
  is not sent yet.)
- **The small profile lists the mesh's subjects (item 38).** One line per catalogue subject in the instructions: its label and the
  publisher of its rank-1 source, from the find mesh's `describe/datumesh.com/catalog`. Without that root the summary still
  stands and the list is left out.
- **`find` answers definitely (item 39).** When nothing is held for a query and every place searched answered, the answer says
  "nothing on this mesh matches; look elsewhere" (CLI JSON `message`, MCP structured content `message`). Each unavailable
  `searched` entry carries `retry`: `true` for a network failure, a timeout or a 5xx; `false` for not allowed, offline, a 4xx and
  a TLS failure. With no match, a mesh that is unavailable means no "nothing matches" message.
- **Fixed.** Replacing the mesh (`reload_aliases`, the endpoint's base URL) gave the new engine no licences in force until they
  changed, so a restricted input a `serve` answer read was refused as not accepted. The new mesh is given them at once.

## 0.5.1

Protocol round, Part 1 (CONTRACT-protocol-part1, datumesh-local 0.5.1; with datumesh-engine 0.6.1's local acceptance, E1).

- **Production defaults (L1).** A new home's `forward_mesh` is `datumesh.com`, and its aliases are `ecb/`, `treasury/`,
  `boe/`, `scripture/` and `earth/` on production, `local/` unchanged. The test mesh stays on the allowlist, so full
  `mesh-test.datumesh.com/...` names still work. An existing home's `config.json` and `aliases.json` are not rewritten; a home
  whose `forward_mesh` is still the test mesh gets one line on stderr from every command, with the file to edit.
- **`describe` (L2)** asks a function's tests at `https://<host>/fn/<full name>?format=json` (with the host), not the name
  without it. A `describe` answer that carries an `order` member (datumesh-http 0.5.3) is checked without it.
- **`verify` (L3)** reports `signed` for a stored or relayed receipt (no inputs), which is checked by its signature; it reports
  `recomputed` only for a computed receipt, whose value step 7 recomputes.
- **Licence acceptance (L4).** `datumesh licence accept <licence>` shows the licence, the register's datasets whose licence
  page it is (their quoted clauses and licence hash, read from the forward mesh's `register/dataset`), and records
  `{licence, accepted_at}` (and the register's `register_licence_hash`, when there is one) in `licences.json`;
  `licence list` shows the licences in force; `licence revoke <licence>` appends a revocation and keeps the history. A
  restricted input whose licence is in force is computed here; the result carries that licence, or
  `LicenseRef-datumesh-local-derivation` over two or more. Without an acceptance the refusal stays `unlicensed`, and names
  the command (`datumesh licence accept '<licence>'`); the MCP text says the user must accept it at the command line. No MCP
  tool accepts a licence. A long-running `mcp` or `serve` picks up an acceptance or revocation made by another process
  (`licences.json` is read again when its mtime changes). `whatif` over such inputs follows the same rule.
- **Never published or exported (L5).** `export`, `root`, `publish --to`, `publish_public`, bundle export and the MCP
  `publish` refuse a result this machine signed under a restricted licence (or the local derivation), with
  `E_LICENCE_NOT_PUBLISHABLE`, naming the licence. Open-licence local results and fetched receipts still export.
- **Positional formula calls (L6).** A positional call in a formula takes the argument order of the function's descriptor
  (`signature.order`, fetched by hash), so `par_yield(10Y, 2026-11-20)` parses where `/cell` takes it.
- **The MCP tool `interval` (L7, D8).** `{function, keys?, from, to}` asks the forward mesh's `x_interval`, resolving aliases,
  and gives the members with their receipts. Over 100 members it gives a summary, the first 100 and the call for the rest.
  The mesh's errors (`E_RANGE_TOO_LARGE`, `not_covered`) are passed through. Each member's receipt is verified against
  the publisher's keys; a member that does not verify makes the tool answer an error naming it.
- A computation the engine refuses for a restricted input licence not accepted (`EngineError::NotAccepted`) is answered
  `unlicensed` with the same message as the gate's refusal, naming `datumesh licence accept '<licence>'`.
- The engine is datumesh-engine 0.6.1 (`EngineBuilder::accepted_licences`, `Engine::set_accepted_licences`,
  `LOCAL_DERIVATION_LICENCE`, `EngineError::NotAccepted`).

## 0.5.0

CONTRACT-round-0.7 R1, R2, R8 (with core 0.3.13, formula 0.6.0, store 0.6.0, engine 0.6.0, http 0.5.0).

- A withdrawn cell is the null version (R1): `get` of it (MCP and CLI) says in words that the answer is not covered, with
  the reason (the withdrawal's `withdrawn` text). The CLI's status line says so too, and exits 3. A label whose lookup cell
  is withdrawn is not covered with the lookup's reason, and has no receipt. The envelope is the mesh's, as it gave it.
- A foreign cell whose latest version is a withdrawal is stored as that version (no value). A read as of an instant the store
  holds no version for is answered with the mesh's answer for it, its receipt checked and held.
- The resolver accepts a `not_covered` envelope whose receipt is a withdrawal (core 0.3.13's `check_answer`), and
  `verify` (the MCP tool and the resolver) reports a withdrawal receipt ok at level `signed`.
- A closed name (R2): `describe` of an unbound name says it is unbound (`E_NAME_UNBOUND`), in words in the MCP text and
  on the CLI's stderr; the mesh's 404 and its body are unchanged. An object id's 404 is not reworded.
- The MCP instructions use the mesh's `held` sentence verbatim (R8: it is a whole sentence), with nothing added before it.
  Without `held`, the text is as before ("This mesh (<host>) holds: ...").

CONTRACT-classes-round C8: the MCP instructions use the mesh's catalogue `held` summary.

- The instructions' sentence "This mesh (<host>) holds: …" and the `find` and `get` descriptions use the
  `held` string of the mesh's `find?summary=1`, verbatim, when it is there (a mesh with no facets, or an
  older mesh, has none). With `held`, nothing is added after it; without it, the text built from the
  classes and functions is as before.

## 0.4.3 (unreleased)

CONTRACT-0.4.9 L1: `describe` of a function lists its tests (MCP and CLI).

- MCP `describe` of a function gives `tests` in the structured content: `count`, `cases_total`,
  `last_run` (`{result, at}`, or null), and `cases` (the first 100: `call`, `expect`, `result`). The
  text content adds "N tests" (or "no tests"), the last run, and one line per case.
- A public function's tests are read from its mesh's function page, `/fn/<name>?format=json` (the
  host's configured peer). The page's 0.4.9 `last_run` members (`pass`, `fail`, `error`, `not_run`)
  are accepted and not shown. A page that cannot be read leaves `tests` null, with `tests_note` giving
  the reason; `describe` itself still answers.
- A local formula lists its own tests (as `define` kept them); each case's result is "not run" and
  `last_run` is null.
- CLI `describe <id>` prints the describe answer on its first line as before, then `tests: N` (or
  `no tests`), `last run <at> <result>`, and one line per case. A function that cannot be read says so.

## 0.4.2 (unreleased)

CONTRACT-build2-part3-calc: the `calc` verb, and a slice `get` that answers (C1 to C5; D27 to D30).

- MCP `calc` and CLI `calc <formula> [--asof <instant>] [--remote]`: a formula over cells, answered
  in one call with `kind: "calculation"`, `value`, `type`, `unit` (when the result has one),
  `formula` (as given), `canonical`, `asof`, `lineage` (`[{call, receipt, publisher, trust}]`, the
  cells read) and `name`. Here, the formula is canonicalised (15.6), its return type and unit are
  inferred from the body, and it is defined as the local function `local/calc_<h>` with no arguments
  (`<h>` the first 16 hex digits of the SHA-256 of the canonical formula). Its cell is evaluated and
  kept by the engine, so an identical calculation is not run again unless an input has a newer
  version. The function is catalogued with the label `calc` and the formula as its description, so
  `find` shows it; `local/` in a formula reads `local.invalid/`. A formula with no inputs is answered
  but neither bound nor catalogued (`name` null). A calculation whose unit is not one atom over one
  atom (or a literal) is refused with `E_CALC_UNIT`, since a signature cannot declare it.
- `remote` (`--remote`): the forward mesh's `x_calc` (read from its mesh document) evaluates the
  formula in a scope; its answer is shown as it is, and nothing is memoised. A mesh without `x_calc`
  is an error naming it (`E_MESH_DOC`).
- Errors carry the formula library's codes (`E_FORMULA_*`); a formula that reads an absent cell is
  `E_FORMULA_ABSENT`. A refused formula binds nothing.
- `get` of a slice (MCP and CLI) answers instead of `E_SLICE_AT_CELL`, for a stored function's slice
  (a foreign one is prefetched first). Up to 100 members are answered as they are. Above 100, the
  answer has `summary` (`count`, `first` and `last` keys, and for numbers `min`, `max` and `mean`),
  the first 100 `members` and `calc`: `{"formula": "mean(<slice>)"}` for numbers, else
  `count(<slice>)`, the exact call that aggregates the slice. The CLI exits 0 for a slice answer.
- The MCP tool `calc` is described as C5 asks (calculate here, not in code; try `find` first; the
  result is memoised on this machine). `get`'s description says a large slice gives a `calc` call.
- The index keeps a `catalogue` table (the label and description of a local calculation).

## 0.4.1 (2026-11-20)

spec-0.6.1: names with a DNS name as a namespace segment after the host
(`datumesh.com/relay/ecb.europa.eu/fx_reference`) are read, resolved, defined against and verified,
through datumesh-core 0.3.10, datumesh-formula 0.5.1 and datumesh-http 0.4.2. `describe` passes the
mesh's `publisher` through. A domain inside a name is a label only: the publisher is always the host.

- `find` results (MCP and CLI) gain `publisher` (the host; for a held name, its own host) and, for a
  relayed function, `source` (its source domain), as separate members (`Found::publisher`,
  `Found::source`).
- A mesh's `describe` answer for a function is checked by core's `check_describe_with` (with
  `publisher`) and passed through; before, a function answer was refused as naming no class.
- A plain name that 2.3 refuses (an uppercase DNS segment, a DNS name as the last segment) is
  `E_CANON_NAME` at `get` and `define`, and nothing is asked of a mesh (was `E_PARSE_URI` at `get`).
- Looking for a class, a name that is not a function name is answered "no class" before the store
  is asked; `get` of any name with a DNS segment failed with `E_NAME_UNBOUND` before.

## 0.4.0 (2026-11-19)

Requires datumesh-http 0.4.0 and datumesh-store 0.5.1. A rooting bundle with tests is built by
http's `bundle::build_with_tests` (local no longer signs the manifest itself; `ed25519-dalek` is
no longer a dependency).

CONTRACT-build2-part3-calculus E6 and CONTRACT-build2-part4-publish P1 (local side): the `whatif`
tool, `define` with tests, and publishing a rooted namespace to a public mesh.

- MCP `whatif` (the spreadsheet what-if, a scenario never signed or stored): `request` (a call),
  `overrides` (`{cell, value}` or `{slice, add, remove}`, values plain literals typed by the cell's
  return type), optional `asof` and `solve` (`vary`, `target`, `equals`, `lo`, `hi`). A cell of a
  local namespace is evaluated here by the shared engine (`Engine::whatif`, nothing written). Any
  other cell is asked of its mesh's `whatif` endpoint (spec-0.6.0 5.9, from its mesh document,
  through the configured peers), and the answer is checked with core's `check_answer`; a failed check
  is `E_ENVELOPE`. The answer's first text item starts with `Scenario (not a fact):`; the structured
  content has `outcome: "scenario"`, `call`, `type`, `value`, `asof`, `overrides` and, with a solve,
  `solved`. It has no receipt and no trust. An error answer is reported with its code (a
  `not_covered` one with its `fault`).
- MCP `define` and CLI `define --tests <json array>`: a formula may come with its tests in the same
  call (`{call, expect, asof?, inputs?}`; `expect` is a plain literal typed by the return type or
  `{"fault": code}`). They are checked before anything is bound (a test of another function, a
  malformed expectation, an input that does not fit its type refuses the whole call). Tests are
  optional locally, and a stored function takes none. The result gains `tests`, the number kept
  with the function. Tests are kept in the resolver's index by function hash.
- MCP `publish_public` and CLI `datumesh publish --to <url> --namespace <ns> --key <file>
  [--export <file>] <name>...`: roots the names under the namespace (in memory, with the catalogue
  and the tests), checks the public rule locally (every formula's tests cover its derived cases, and
  every branch arm is taken; a shortfall is `E_TESTS_MISSING`, listing per function the cases and the
  branch counts, and nothing is sent), then posts the bundle to the mesh's `x_bundles` endpoint, or
  `<url>/bundles`. The mesh's `201` is the result (`imported`, `names`, `functions` with each page);
  its refusals are passed on with their code and detail. `publish <file>` is as before.
- The rooting bundle carries a `tests` section (E5) when the rooted functions have tests; the
  manifest signs it with the names, the receipts and the catalogue. Without tests the bundle is the
  datumesh-http crate's, unchanged.
- The public rule uses formula's coverage observer (`datumesh_formula::coverage`): derived cases are
  the divisors (zero), the number arguments (negative), the extent ends, the cell inputs (absent,
  `E_FORMULA_ABSENT`) and the slice inputs (empty); branch arms are those of the body.
- Depends on datumesh-engine 0.5.0, datumesh-formula 0.5.0, datumesh-store 0.5.0 and datumesh-core
  0.3.9 (released), and on datumesh-http 0.4.0 (not yet released; path-patched in development). New
  direct dependencies: `ureq` (plain HTTP for the two POSTs, as datumesh-http uses it) and
  `ed25519-dalek` (the manifest of a bundle with tests), both optional under `resolver`.

CONTRACT-build2-part2-read R4, R5 (local side): one-step `get` for a plain reference, tool
descriptions and instructions written from the mesh's catalogue, `verify` of a quoted text, and
the catalogue in the rooting export.

- MCP `get` takes a plain reference in its `request` (such as "John 3:16 in Greek"), as well as a
  call or a cell URI. The first find mesh's `/find?q=` resolves it (http 0.4.0's answer):
  - `exact`: the answer is read in the same call. Its first item is the resolution's label and
    canonical call, then the value (a text blob's checked text) and the answer block, which
    gains `resolved: {label, call}`;
  - `ambiguous`: not an error; `{resolution: "ambiguous", candidates}` with the candidates'
    labels and calls, and no value;
  - a range: `isError` with `E_RANGE_REFUSED`, the detail saying ranges are read verse by verse
    or as a slice;
  - `none`: `isError` with `not_covered`, and the find results as `suggestions`.
- A query-string form (`name?key=value`) is refused with `E_PARSE_URI`, as a call that does not
  parse, by `get` in the MCP tool and by `Resolver::get`; it is never sent to `/find`. `?asof=` is
  unchanged.
- `Resolver::get` keeps its signature and its answers for calls and cell URIs; the plain-reference
  resolution is the MCP tool's (`Resolver` gives it to the server only, `Resolver::get` still
  answers a plain reference as before).
- Tool descriptions and the server instructions are written at start from the first find
  mesh's catalogue summary (`GET /find?summary=1`): its classes and functions with labels, cell
  counts and the labels of small dictionaries (at most 50 values per key). They name no cell and
  no reference. When the summary cannot be had, or the mesh holds nothing, the 0.3.3 text is used.
- MCP `verify` takes `call` (a call or a cell URI, whose receipt is verified) as well as
  `receipt`, and `text`: the quoted text is compared with the cell's text blob after NFC on both
  sides. The result gains `text_matches`, and when false `first_difference` (the first code point
  after NFC where they differ, or the shorter's length). A blob that fails its hash is
  `E_BLOB_MISMATCH`.
- The rooting export bundle (`Resolver::root` with `export`) carries the catalogue: one entry
  per rooted function, with its label, a description made from its name, its signature's
  arguments, its examples (the rooted stored cells' calls and values) and an empty dictionary.
  It is built by `datumesh_http::bundle::build_with_catalog`, so the manifest signs it. The
  member is an array of entries, each with its `name`, as http 0.4.0 writes it.
- Depends on datumesh-http 0.4.0, datumesh-store 0.5.0, datumesh-formula 0.4.0 and
  datumesh-core 0.3.9.

## 0.3.3 (2026-11-18)

CONTRACT-1.7a-text L1-L4: MCP `get` returns a text blob's content, checked; the `blob` resource.

- MCP `get`, for a receipt whose value is a blob (`<sha256>:<length>:<media_type>`): a text
  media type of at most 65,536 bytes gives three text items, the text itself (exactly the blob's
  bytes), `{"blob_checked":true,"length","media_type","sha256"}` and the answer block;
  `structuredContent` is the answer block with `text` and `blob_checked`. Other blobs (not text,
  or over the limit) give two items, the `blob` address (`uri`, `url` when there is a host)
  with `blob_checked: false`, and the answer block. A text blob that cannot be had here (offline,
  not held, the mesh unreachable) is as the latter with `blob_unavailable`; the receipt stands.
- The content comes from the local store, else from the answering mesh's `/blob/<sha256>` (the
  forward mesh when forwarded, else the cell's host), and is kept. It is checked: its SHA-256,
  its length, and UTF-8. A mismatch, or the resolver's own fetch failing with `E_BLOB_MISMATCH`,
  is `isError` with `E_BLOB_MISMATCH: <detail>` and no receipt or text.
- `Resolver::blob_of(&reply) -> Option<BlobAnswer>` and `BlobContent`: the blob a reply names,
  with its content (`Text`, `NotText`, `Unavailable`, `Mismatch`).
- `blob://<sha256>` resource (`resources/templates/list` gives `blob`): a held blob, checked
  against its SHA-256; text (UTF-8, up to 64 KiB) with `text/plain; charset=utf-8`, else base64
  with `application/octet-stream`. Not held: -32602; a malformed sha: -32602; stored bytes that do
  not hash to it: -32603 with `E_BLOB_MISMATCH`.
- The `get` description and the instructions say what `get` returns for a text value.

## 0.3.2 (2026-11-16)

CONTRACT-1.7a L1-L5: `find` falls through to the configured meshes.

- Configuration: `find_meshes` (default `["datumesh.com"]`, in `defaults/config.json`): the hosts
  `find` asks when its local names give no answer, and always with `remote`. A host must also be
  on the allowlist to be asked.
- `Resolver::find_in(query, remote) -> FindAnswer { results, searched }`: `results` are `Found`
  (now with `kind`, `description`, and for a class found on a mesh its `methods`); `searched`
  lists `local` and each mesh asked, with its status (`ok`, `cached`, `not allowed`, `offline`, or
  an error code and detail). A failing mesh is recorded there, not raised. `Resolver::find` keeps
  its signature and returns `find_in`'s results.
- An answer of a mesh is kept 24 hours in the index, keyed by the mesh and the lowercased query;
  a failure is not kept. Offline, nothing is asked.
- MCP `find` gives `searched`, and each result `kind` and `description`; a remote class is
  followed by its methods, with call templates from the mesh's arguments.
- CLI `find` prints `searched:` on standard error when there are no results.

## 0.3.1 (2026-11-16)

- `publish` of a file on another file system than the data directory works: the kept lines are
  written beside the input file directly, with no rename across file systems (EXDEV). The bug
  dates from 0.1.0; the 1.8b live rooting check found it.

## 0.3.0 (2026-11-16)

CONTRACT-1.8b items 3, 7 and 8 and difference 1.

- Forwarding and holding (L3): a local name that reads a declared restricted computation without a
  declaration of its own is refused with `unlicensed` (403) here, not forwarded to the mesh.
- Alias map (L6): the file is `{"entries": [{"short", "full", "tx"}, ...]}`; each entry takes
  effect at its `tx`, and a request is resolved at its asof (`?asof=`, else now). `resolved`
  carries `aliases_tx`, the greatest `tx` of the entries that applied. The older shape
  `{<short>: <full>, ..., "tx": T}` is still read. A prefix alias in a formula source (`@ecb/...`)
  is expanded before the definition, so the hash equals that of the full name. A bare name for a
  class outside `datumesh.com/std/` is refused with a local error.
- Serve (L7): the `poll` and `next` links of this machine's host are on the loopback origin
  (`http://127.0.0.1:<port>/...`); `Link: rel="canonical"` stays `https://<host>/...`, and the
  endpoint now sends it on answers about now.
- Rooting (L5): `datumesh root --namespace <ns> --key <file> [--export <file>] <name>...` and
  `Resolver::root(&RootPlan)` republish this machine's names under a rooted namespace, signed by the
  namespace host's key (read in place, mode 0600, never stored). Stored cells are re-signed with
  `tx` = `asof` = `issued` = now; formulas are rewritten to the rooted names; computed cells are
  listed, not copied. Without `--export` it is a dry run that prints the plan as JSON; with it, a
  bundle 1 is written to a new file (mode 0600). New `Error::Rooting`.
- Defaults: the default alias map's `tx` is 2026-01-01.
- Key rotation (L4): `datumesh key rotate` and `Resolver::rotate_key()` make a new key from now;
  the old one is kept as `<key file>.retired-<id>` and listed, with `valid_to`, in
  `retired-keys.json`. The key document lists the current and the retired keys.
- Host (L4): config member `host` (default `local.invalid`, a name under `.invalid`); `local/`
  means `<host>/`, receipts say `computed_by` `<host>#<key id>`.
- Trusted machines (L4): `datumesh trust add|revoke|list` and `Resolver::trust_machine`,
  `revoke_machine`, `trusted_machines`, kept in `trust.json`. A trusted host's receipts verify
  with its effective document (labelled `trusted machine <host>`); a revocation ends them from
  its instant. A replacement document must keep every trusted key. Defining a formula over an
  unrooted host that is neither this machine's nor trusted is refused (`Error::UntrustedInput`).
- `import --trust-key <key id>` trusts the host of the bundle's key document that carries the key
  (refused for this machine's own host); it no longer merges that key into this machine's
  document.
- Clock (L4): the mesh signs with the resolver's clock, so `issued` follows `open_with_clock`.

## 0.2.2 (2026-11-14)

The first published release for spec-0.5.0: 0.2.1, below, with one fix. The tag `v0.2.1` was
pushed, but its release run stopped before publishing: on windows-latest a peer name that does
not resolve gave `E_FOREIGN_UNAVAILABLE` detail `network`, not `dns`. datumesh-http 0.2.2
names only the Linux resolver's wording `dns`; its catch-all `network` is now refined by the
failure text, which knows the Windows resolver's codes (os error 11001-11004). Tags are not
moved, so the fix ships as 0.2.2, and 0.2.1 was never on crates.io.

## 0.2.1 (2026-11-14, tagged, not published)

Additive (the public API adds `Resolver::define_class` and `Resolver::describe`), and the
behaviour of spec-0.5.0 (classes in requests, describe, the loopback mesh document).

- Requests: `get` and `explain` take the method form of a call, `N(keys).m(args)` and
  `N[label]`, through formula 0.3.0's `parse_call`, with unquoted values by position type,
  the asof default (the call's asof, or now) and bare names through the alias map. The
  envelope carries `resolved` when the call resolved something (its canonical call, each
  alias, method, default and lookup resolution, the alias map's `tx` for a bare name). A
  method call and its plain read give the same cell, receipt and envelope apart from
  `resolved`. The stderr line of `get` names the canonical call.
- Classes: a class the store does not hold is fetched from its forward mesh once (the
  bindings resource names it by its class hash, and its describe answer is checked against
  that hash), with its method functions and its lookup cell; a later call is answered from
  the store, offline too. A call that does not parse or bind is answered by the mesh, with
  no network. Alias prefixes (`ecb/`) are expanded on the callee and are not alias
  resolutions.
- describe (16.8): `Resolver::describe(id)` and `datumesh describe <id>` (exit 0 for 200, 3
  for anything else): a class name, a key encoding, an object id, a short id or a cell id.
  Answered from the store where it can, else by the host's describe endpoint, checked with
  core's `check_describe_with` (a failed check is an error). `serve` answers
  `/describe/{id}`, and its mesh document lists it.
- Local classes: `Resolver::define_class` and `define <name> --class <file>` under `local/`
  (owner `local.invalid`); `publish` accepts class lines under `local/`.
- WASM: `parse_call`'s result carries `resolved`; its environment takes `classes`, `now`,
  `lookups` and `aliases_tx`.
- Loopback: the mesh document of the unrooted host is valid on the loopback origin
  (spec-0.5.0 9.6, 2.5); the notes that said it was local-only until spec-0.5.0 are gone.
- Dependencies: datumesh-core 0.3.8, datumesh-formula 0.3.0, datumesh-store 0.4.1,
  datumesh-engine 0.4.2, datumesh-http 0.2.2.

## 0.2.0 (2026-11-13)

Released as 0.2.0, not 0.1.2: the gate's public API changes (`LicenceGate::new(&[String])`, `open()` and `permits(&str, &str) -> bool` replaced), which is breaking for a 0.1.x crate (the user's decision, 2026-11-13).

- The licence gate is the engine's rule (the user's decision L1 C): licence inheritance
  (spec-0.4.3 8.5, datumesh-engine 0.4.1, core's `inherit_licence`) with the publisher's
  declarations from the key document held for the function's namespace host, plus "a
  restricted licence is never signed by an unrooted key". Open and local inputs are computed
  here, the result carrying the inherited licence; a declared restricted input is forwarded
  (brief test 5 unchanged); an undeclared one is answered `unlicensed` (403) here without a
  round trip. Consequence: `strip_niqqud` over the UXLC (an `https://` licence, restricted
  under 8.5) is forwarded, no longer computed here.
- `LicenceGate::permits(function, licence, signer_host, declarations, issued)` returns a
  `GateDecision` (`Compute`, `Forward`, `Refuse(reason)`); `LicenceGate::new()` takes no list.
  Breaking for callers of 0.1.1's `LicenceGate::new(&open)` and `permits(licence, signer)`.
- The open list is no longer a computation gate and is not installed; `open_licences` in an
  existing configuration file is accepted and ignored (deprecated).
- The publisher's key document (its declarations) is fetched and kept on every import of a
  name (get, prefetch, refresh) and before every forward.
- Restricted cells the mesh delivers are still held (the user's decision L2 i): delivery is
  allowed; the licence binds computation.
- datumesh-engine 0.4.1, datumesh-http 0.2.1; receipts signed here carry the inherited
  licence (the configured `licence` only for cells without inputs).

## 0.1.1 (2026-11-05)

- Fixed: on Windows, a host name that does not resolve answered `E_FOREIGN_UNAVAILABLE` with
  `detail` `network` instead of `dns` (the resolver's errors are worded differently there:
  WSAHOST_NOT_FOUND 11001, WSATRY_AGAIN 11002, WSANO_RECOVERY 11003, WSANO_DATA 11004). The
  failure is now named from the error's platform shapes (Linux, macOS, Windows; ureq's own
  `host not found`), timeouts (10060) and refusals (10061) included, and the URL in the
  error no longer counts ([#1](https://github.com/datumesh-com/datumesh-local/issues/1)).

## 0.1.0 (2026-11-04)

The first release (step 1.8a): the Datumesh local resolver.

- `Resolver`: `get` (held cells from the store; foreign stored cells fetched once, checked and
  imported; computed cells evaluated here behind the licence gate, or forwarded), `define`
  (formula, stored, descriptor; the rooting rule of SPEC 2.5), `publish` (under `local/`
  only), `explain`, `find`, `prefetch` (slices by page, functions, cells; blobs on request),
  `refresh` (bindings, slices, `/versions`, forwarded answers, the alias map), `export` and
  `import` (with `--trust-key`), `verify`, offline mode.
- `local.invalid`: the machine key (Ed25519, mode 0600, the machine id), its key document in
  the store, receipts signed `local.invalid#<key id>` and labelled `local, verifies only on
  this machine`.
- `E_FOREIGN_UNAVAILABLE` (spec-0.4.3; HTTP 502, `detail` offline, network, dns, tls, timeout or refused) for a foreign cell not held that cannot be
  fetched, behind one constant; the envelope is datumesh-core 0.3.7's.
- The configuration file with comments, the alias map as a refreshable file snapshot, the
  licence gate's open list.
- `serve`: the mesh's handlers on 127.0.0.1 only, `/cell` through the resolver.
- `cli`: the `datumesh` command (crate `datumesh`).
- Feature `wasm`: `verify` and `parse_call` for `wasm32-unknown-unknown`.
