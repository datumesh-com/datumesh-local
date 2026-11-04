#!/bin/sh
# Brief test 3's refresh against an upstream mesh on PostgreSQL 16 (as the test mesh runs),
# not the live mesh: a postgres:16 container of its own on the `datumesh` network
# (datumesh-local-refresh-pg-<pid>), created here and removed after; the upstream mesh is
# the datumesh-http library, in the test process.
#   tools/refresh_pg.sh [toolchain]
set -e
root="$(cd "$(dirname "$0")/.." && pwd)"
tag="${1:-1.99}"
name="datumesh-local-refresh-pg-$$"
docker run -d --rm --name "$name" --network datumesh -e POSTGRES_PASSWORD=postgres postgres:16 >/dev/null
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT
i=0
until docker exec "$name" pg_isready -U postgres -q 2>/dev/null; do
  i=$((i + 1)); [ "$i" -lt 60 ] || { echo "postgres did not start" >&2; exit 1; }; sleep 1
done
sleep 1
UPSTREAM_DATABASE_URL="postgres://postgres:postgres@$name:5432/postgres" "$root/tools/cargo.sh" "$tag" test --test prefetch refresh_picks_up_a_new_version_published_on_the_mesh
