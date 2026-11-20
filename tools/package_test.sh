#!/bin/sh
# Packaging test: `cargo package`, check the .crate carries no test material, then build a
# new project on the packaged crate (and datumesh-core from crates.io) and run a program
# that uses it offline in a directory of its own, twice (the second run reopens the store):
# a local stored function and cell published, answered and verified at signed.
#   tools/package_test.sh [toolchain]   in fresh rust:<toolchain> containers (tools/cargo.sh)
#   tools/package_test.sh --native      with the local cargo (CI)
set -e
root="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
if [ "$1" = "--native" ]; then
  (cd "$root" && cargo package --locked)
  pkg="$root/target/package"
  in_fresh() { (cd "$work" && sh -c "$1"); }
else
  tag="${1:-1.99}"
  # With a scratch patch (unreleased dependencies) the packaged crate is built below instead.
  if [ -n "$DATUMESH_PATCH" ]; then verify="--no-verify"; else verify="--locked"; fi
  "$root/tools/cargo.sh" "$tag" package --allow-dirty $verify
  pkg="$root/target-$tag/package"
  patch_mount=""
  if [ -n "$DATUMESH_PATCH" ]; then patch_mount="-v $DATUMESH_PATCH:/patch:ro"; fi
  in_fresh() {
    # shellcheck disable=SC2086
    docker run --rm --name "datumesh-local-package-$$" $patch_mount -v "$HOME/.cache/datumesh-cargo:/usr/local/cargo/registry" -v "$work:/p" -w /p "rust:$tag" \
      sh -c "if [ -f /patch/config.toml ]; then cp /patch/config.toml /usr/local/cargo/config.toml; fi; $1"
  }
fi
cp "$pkg/datumesh-local-$version.crate" "$work/"
(cd "$work" && tar xzf "datumesh-local-$version.crate")
if tar tzf "$work/datumesh-local-$version.crate" | grep -E '/tests/|/tools/|/wasm-smoke/|/bench/'; then
  echo "test material in the packaged crate" >&2
  exit 1
fi
tar tzf "$work/datumesh-local-$version.crate" | grep -q '/defaults/config.json$'
mkdir -p "$work/app/src"
cat > "$work/app/src/main.rs" <<'RS'
use datumesh_local::{Config, Resolver};

/// The cells' instant: the resolver binds at its now (rule A), so it runs on this clock.
struct At;
impl datumesh_engine::Clock for At {
    fn now(&self) -> String {
        "2026-11-02T00:00:00.000000Z".to_string()
    }
}

fn main() {
    let dir = std::env::current_dir().unwrap().join("home");
    let mut c = Config::in_dir(&dir).unwrap();
    c.offline = true;
    let r = Resolver::open_with_clock(c, std::sync::Arc::new(At)).unwrap();
    let f = dir.join("notes.jsonl");
    std::fs::write(&f, concat!(
        r#"{"function": "local/note", "signature": {"args": {"k": {"type": "string", "valid": false}}, "order": ["k"], "returns": "string"}}"#, "\n",
        r#"{"cell": {"fn": "local/note", "args": {"k": {"type": "string", "value": "a"}}}, "value": {"type": "string", "value": "hello"}, "tx": "2026-11-02T00:00:00.000000Z", "asof": "2026-11-02T00:00:00.000000Z", "licence": "CC0-1.0", "summary": "A note."}"#, "\n",
    )).unwrap();
    r.publish(&f).unwrap();
    let a = r.get(r#"local/note(k: "a")"#).unwrap();
    assert_eq!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
    let rc = datumesh_core::json::parse(&a.body).unwrap().get("receipt").unwrap().to_jcs();
    let v = r.verify(&rc);
    assert_eq!(v.get("ok"), Some(&datumesh_core::json::Value::Bool(true)));
    println!("{} {}", r.key_id(), String::from_utf8_lossy(&a.body));
}
RS
printf '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\ndatumesh-local = { path = "../datumesh-local-%s" }\ndatumesh-core = "0.3.10"\ndatumesh-engine = { version = "0.5.0", default-features = false }\n' "$version" > "$work/app/Cargo.toml"
in_fresh "cd app && cargo run -q && cargo run -q"
echo "package test passed ($version)"
