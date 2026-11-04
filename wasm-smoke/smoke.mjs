// Brief test 8: the wasm32 bindings (feature `wasm`: the core's verification and the call
// parser) in Node. Run by tools/wasm_smoke.sh:
//   node wasm-smoke/smoke.mjs <dir of the wasm-bindgen --target nodejs output> <vector.json>
// The vector is the spec's receipt/stored-number.json (spec-0.2.0 and later, unchanged).
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";
import assert from "node:assert/strict";

const require = createRequire(import.meta.url);
const dm = require(join(resolve(process.argv[2]), "datumesh_local.js"));
const v = JSON.parse(readFileSync(process.argv[3], "utf8"));

// verify: the vector's receipt at level recomputed, its hash; a tampered copy fails.
const keys = {};
for (const [host, env] of Object.entries(v.keys)) keys[host] = env.https.body;
const recompute = JSON.stringify({ descriptor: v.recompute.descriptor, value: v.recompute.value });
const ok = JSON.parse(dm.verify(v.input, JSON.stringify(keys), recompute));
assert.equal(ok.ok, true, JSON.stringify(ok));
assert.equal(ok.level, "recomputed");
assert.equal(ok.hash, v.sha256);
const signed = JSON.parse(dm.verify(v.input, JSON.stringify(keys), ""));
assert.equal(signed.level, "signed");
const tampered = JSON.parse(dm.verify(v.input.replace('"1.0892"', '"1.0893"'), JSON.stringify(keys), ""));
assert.equal(tampered.ok, false);
assert.equal(tampered.code, "E_SIGNATURE");
const nokeys = JSON.parse(dm.verify(v.input, "{}", ""));
assert.equal(nokeys.code, "E_KEY_UNAVAILABLE");

// parse_call: a call to its cell URI (arguments by name, positional by order, an alias, a
// pin, asof), and the codes of a call that names no cell.
const d = v.recompute.descriptor;
const env = JSON.stringify({ functions: { "rates.example.org/fx_rate": d }, aliases: { fx: "rates.example.org/fx_rate" } });
const uri = "cell://rates.example.org/fx_rate?base=EUR&date=2024-03-15&quote=USD";
for (const call of [
  'rates.example.org/fx_rate(base: "EUR", quote: "USD", date: 2024-03-15)',
  'rates.example.org/fx_rate(quote: "USD", date: 2024-03-15, base: "EUR")',
  '@fx(base: "EUR", quote: "USD", date: 2024-03-15)',
]) {
  const p = JSON.parse(dm.parse_call(call, env));
  assert.equal(p.ok, true, call + " " + JSON.stringify(p));
  assert.equal(p.uri, uri);
  assert.equal(p.reference.ns, "rates.example.org");
  assert.equal(p.reference.fn, "fx_rate");
}
const hash = JSON.parse(v.input).cell.fn_hash;
const pinned = JSON.parse(
  dm.parse_call(`rates.example.org/fx_rate@${hash}(base: "EUR", quote: "USD", date: 2024-03-15)?asof=2024-03-16T00:00:00.000000Z`, env),
);
assert.equal(pinned.ok, true, JSON.stringify(pinned));
assert.equal(pinned.uri, `${uri}&fn_hash=${hash}&asof=2024-03-16T00:00:00.000000Z`);
assert.equal(pinned.reference.fn_hash, hash);
const unknown = JSON.parse(dm.parse_call('rates.example.org/fx_rate(base: "EUR", quote: "USD", day: 2024-03-15)', env));
assert.equal(unknown.ok, false);
assert.equal(unknown.code, "E_ARG_UNKNOWN");
const unbound = JSON.parse(dm.parse_call('example.org/nothing(a: 1)', env));
assert.equal(unbound.ok, false);
assert.equal(unbound.code, "E_NAME_UNBOUND");
console.log("wasm smoke passed");
