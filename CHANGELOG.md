# Changelog

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
