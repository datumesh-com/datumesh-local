#!/bin/sh
# Brief test 9, the fresh install (run after datumesh-local and datumesh are on crates.io):
# a clean rust container; `cargo install datumesh`; `datumesh get` of one mesh cell;
# `datumesh prefetch` of one slice; then, with the network disabled (the same container
# disconnected from every network), an offline `get` of a prefetched member succeeds and an
# unfetched cell answers E_FOREIGN_UNAVAILABLE. Nothing but the container is touched.
#   tools/fresh_install.sh [toolchain] [version]
set -e
tag="${1:-1.99}"
version="${2:-0.1.1}"
name="datumesh-fresh-install-$$"
docker run -d --name "$name" "rust:$tag" sleep infinity > /dev/null
trap 'docker rm -f "$name" > /dev/null 2>&1 || true' EXIT
run() { docker exec "$name" sh -c "$1"; }
run "cargo install --locked datumesh --version $version 2>&1 | tail -n 3"
run 'datumesh --version'
echo "== get one mesh cell (online)"
run 'datumesh get "ecb/fx_rate(base: \"EUR\", quote: \"USD\", date: 2026-10-30)"'
echo "== prefetch one slice"
run 'datumesh prefetch "ecb/fx_rate(base: *, quote: *, date: *)"'
echo "== the network disabled"
docker network disconnect bridge "$name"
if run 'getent hosts mesh-test.datumesh.com || curl -sS -m 5 https://mesh-test.datumesh.com/health' > /dev/null 2>&1; then
  echo "the network is still reachable" >&2
  exit 1
fi
echo "== offline get of a prefetched member (not --offline: the network is simply gone)"
run 'datumesh get "ecb/fx_rate(base: \"EUR\", quote: \"GBP\", date: 2026-10-29)"'
echo "== an unfetched cell: E_FOREIGN_UNAVAILABLE, exit 3"
set +e
run 'datumesh get "ecb/holiday(calendar: \"TARGET\", date: 2026-12-25)"'
code=$?
set -e
test "$code" = 3
echo "fresh install passed (datumesh $version, rust:$tag)"
