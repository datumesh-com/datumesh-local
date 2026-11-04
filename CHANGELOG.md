# Changelog

## 0.1.0 (unreleased)

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
  fetched, behind one constant; written by this crate until datumesh-core 0.3.7.
- The configuration file with comments, the alias map as a refreshable file snapshot, the
  licence gate's open list.
- `serve`: the mesh's handlers on 127.0.0.1 only, `/cell` through the resolver.
- `cli`: the `datumesh` command (crate `datumesh`).
- Feature `wasm`: `verify` and `parse_call` for `wasm32-unknown-unknown`.
