# Changelog

## 0.2.1 (2026-11-14)

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
