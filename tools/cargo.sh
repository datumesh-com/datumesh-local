#!/bin/sh
# Run cargo in a toolchain container on the `datumesh` Docker network:
#   tools/cargo.sh <image-tag> <cargo args...>, e.g. tools/cargo.sh 1.99 test
# Tags: a Rust version (image rust:<tag>), nightly-tools (cargo-public-api), lint.
# The container is named datumesh-local-cargo-<tag>-<pid> and removed when cargo exits.
#
# DATUMESH_PATCH=<dir>: a directory holding a cargo config file `config.toml` (e.g. a
# [patch.crates-io] of unreleased dependencies, with paths under /patch), mounted read-only
# at /patch and passed to cargo with --config. Unset: crates.io only.
set -e
tag="$1"; shift
case "$tag" in nightly) image=rustlang/rust:nightly ;; nightly-tools) image=datumesh-nightly-tools ;; lint) image=datumesh-rust-1.99-lint ;; *) image="rust:$tag" ;; esac
root="$(cd "$(dirname "$0")/.." && pwd)"
patch_mount=""
patch_arg=""
if [ -n "$DATUMESH_PATCH" ]; then
  patch_mount="-v $DATUMESH_PATCH:/patch:ro"
  patch_arg="--config /patch/config.toml"
fi
# shellcheck disable=SC2086
exec docker run --rm -i --name "datumesh-local-cargo-$tag-$$" --network datumesh $patch_mount \
  -v "$HOME/.cache/datumesh-cargo:/usr/local/cargo/registry" -v "$root:/w" -w /w \
  -e CARGO_TARGET_DIR="/w/target-$tag" -e CARGO_TERM_COLOR=never \
  -e RUST_LOG -e RUST_BACKTRACE -e RUSTDOCFLAGS -e RUST_TEST_THREADS -e DATUMESH_LIVE -e DATUMESH_BENCH_OUT \
  "$image" cargo $patch_arg "$@"
