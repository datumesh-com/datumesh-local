#!/bin/sh
# For the operator of the live test mesh, not run by the tests: brief test 3's "refresh picks
# up a new version published on the mesh" against mesh-test.datumesh.com.
#
#   tools/live_refresh.sh publish   publishes one test version under mesh-test.datumesh.com/ecb
#                                   (fx_rate EUR/USD on 2026-10-30, value 1.1700, tx now; test
#                                   data) with `datumesh-mesh publish-stored` (the mesh's own key,
#                                   as deploy/seed.sh loads the seed), then restarts the mesh so
#                                   its in-memory latest answers are dropped
#   tools/live_refresh.sh check     a fresh resolver: get the cell, refresh, get it again; prints
#                                   both values (run `check` before and after `publish`)
#
# Writes to the production data of the test mesh: run `publish` only on purpose.
set -e
root="$(cd "$(dirname "$0")/.." && pwd)"
dir="${DATUMESH_DIR:-/root/datumesh-mesh-test}"
case "$1" in
publish)
  image="${DATUMESH_MESH_IMAGE:-$(docker inspect datumesh-mesh-test-mesh-1 --format '{{.Config.Image}}')}"
  t="$(date -u +%Y-%m-%dT%H:%M:%S.000000Z)"
  mkdir -p "$dir/seed/out"
  printf '{"asof": "%s", "cell": {"args": {"base": {"type": "string", "value": "EUR"}, "date": {"type": "date", "value": "2026-10-30"}, "quote": {"type": "string", "value": "USD"}}, "fn": "mesh-test.datumesh.com/ecb/fx_rate"}, "licence": "CC-BY-4.0", "summary": "Synthetic test data, not ECB data: a test version of 1 EUR in USD on 2026-10-30 for the local resolver'"'"'s refresh test. Licence CC-BY-4.0.", "tx": "%s", "value": {"type": "number", "value": "1.1700"}}\n' "$t" "$t" > "$dir/seed/out/refresh-test.jsonl"
  chmod a+r "$dir/seed/out/refresh-test.jsonl"
  docker run --rm --name datumesh-mesh-test-refresh-load --network datumesh-mesh-test_backend --env-file "$dir/secrets/db.env" \
    -e DATUMESH_HOST=mesh-test.datumesh.com -e DATUMESH_KEY_FILE=/run/secrets/mesh.key -v "$dir/secrets:/run/secrets:ro" \
    -v "$dir/seed/out:/seed:ro" "$image" publish-stored /seed/refresh-test.jsonl
  docker restart datumesh-mesh-test-mesh-1 > /dev/null
  echo "published a version at $t; the mesh restarted"
  ;;
check)
  home="$(mktemp -d)"
  trap 'rm -rf "$home"' EXIT
  q='ecb/fx_rate(base: "EUR", quote: "USD", date: 2026-10-30)'
  dm() { "${DATUMESH:-datumesh}" --home "$home" "$@"; }
  dm prefetch 'ecb/fx_rate(base: *, quote: *, date: *)' > /dev/null
  echo "held:      $(dm get "$q" 2>/dev/null | sed -n 's/.*"tx":"\([^"]*\)".*"value":"\([^"]*\)".*/tx \1 value \2/p')"
  echo "refresh:   $(dm refresh)"
  echo "refreshed: $(dm get "$q" 2>/dev/null | sed -n 's/.*"tx":"\([^"]*\)".*"value":"\([^"]*\)".*/tx \1 value \2/p')"
  ;;
*)
  echo "usage: tools/live_refresh.sh publish|check" >&2
  exit 2
  ;;
esac
