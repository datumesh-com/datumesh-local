#!/bin/sh
# Brief test 8: build the wasm32 bindings (feature `wasm`, no engine), generate the Node glue
# with wasm-bindgen (the CLI of the version Cargo.lock has), and run wasm-smoke/smoke.mjs.
#   tools/wasm_smoke.sh            in rust:1.99 and node:22-slim containers
#   tools/wasm_smoke.sh --native   with the local toolchain (target wasm32-unknown-unknown,
#                                  wasm-bindgen on PATH) and node (CI)
set -e
root="$(cd "$(dirname "$0")/.." && pwd)"
build='
  set -e
  rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
  cargo rustc --lib --release --no-default-features --features wasm --target wasm32-unknown-unknown --crate-type cdylib
  v="$(cargo metadata --format-version 1 --no-default-features --features wasm | sed -n "s/.*\"name\":\"wasm-bindgen\",\"version\":\"\([^\"]*\)\".*/\1/p" | head -n 1)"
  if ! command -v wasm-bindgen >/dev/null 2>&1 || [ "$(wasm-bindgen --version | cut -d" " -f2)" != "$v" ]; then
    cargo install --locked --root "$CARGO_TARGET_DIR/tools" wasm-bindgen-cli --version "$v"
    PATH="$CARGO_TARGET_DIR/tools/bin:$PATH"
  fi
  rm -rf "$CARGO_TARGET_DIR/pkg"
  wasm-bindgen --target nodejs --out-dir "$CARGO_TARGET_DIR/pkg" "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/datumesh_local.wasm"
  ls -l "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/datumesh_local.wasm"
'
if [ "$1" = "--native" ]; then
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target-wasm}"
  (cd "$root" && sh -c "$build")
  node "$root/wasm-smoke/smoke.mjs" "$CARGO_TARGET_DIR/pkg" "$root/wasm-smoke/stored-number.json"
else
  # DATUMESH_PATCH: as in tools/cargo.sh (a cargo config file, config.toml, mounted at /patch).
  patch_mount=""
  if [ -n "$DATUMESH_PATCH" ]; then
    patch_mount="-v $DATUMESH_PATCH:/patch:ro"
  fi
  # shellcheck disable=SC2086
  docker run --rm --name "datumesh-local-wasm-$$" $patch_mount \
    -v "$HOME/.cache/datumesh-cargo:/usr/local/cargo/registry" -v "$root:/w" -w /w \
    -e CARGO_TARGET_DIR=/w/target-wasm -e CARGO_TERM_COLOR=never -e DATUMESH_PATCH \
    rust:1.99 sh -c "if [ -n \"\$DATUMESH_PATCH\" ]; then mkdir -p /usr/local/cargo && cp /patch/config.toml /usr/local/cargo/config.toml; fi; $build"
  docker run --rm --name "datumesh-local-node-$$" -v "$root:/w:ro" -w /w node:22-slim \
    node wasm-smoke/smoke.mjs target-wasm/pkg wasm-smoke/stored-number.json
fi
