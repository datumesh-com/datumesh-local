#!/usr/bin/env python3
"""Captures the production responses tests/data/production/routes.jsonl holds (datumesh-local 0.5.1
tests). Read-only: GET only. Run: python3 tools/capture_production_fixtures.py > tests/data/production/routes.jsonl
With paths as arguments it captures only those (to append a route without re-recording the rest):
python3 tools/capture_production_fixtures.py '<path>' >> tests/data/production/routes.jsonl"""
import json, sys, urllib.request, urllib.error, urllib.parse

BASE = "https://datumesh.com"
FNS = ["relay/treasury.gov/par_yield", "relay/bankofengland.co.uk/bank_rate", "relay/ecb.europa.eu/fx_reference"]
METHODS = ["licence_clauses", "licence_hash", "licence_page", "name", "retrieved", "status"]
paths = ["/.well-known/datumesh.json", "/.well-known/datumesh-mesh.json", "/find?summary=1",
         "/describe/datumesh.com/register/dataset",
         "/bindings?name=" + urllib.parse.quote("datumesh.com/register/dataset", safe="")]
for f in FNS:
    paths += ["/bindings?name=" + urllib.parse.quote("datumesh.com/" + f, safe=""),
              "/describe/datumesh.com/" + f, "/fn/datumesh.com/" + f + "?format=json"]
paths += ["/cell/datumesh.com/relay/treasury.gov/par_yield?date=2026-11-20&tenor=10Y",
          "/cell/datumesh.com/relay/bankofengland.co.uk/bank_rate?date=2026-11-20",
          "/cell/datumesh.com/relay/ecb.europa.eu/fx_reference?currency=USD&date=2026-11-20"]
for m in METHODS:
    paths += ["/bindings?name=" + urllib.parse.quote("datumesh.com/register/dataset/" + m, safe=""),
              "/slice/datumesh.com/register/dataset/%s(source:*,code:*)" % m,
              "/describe/datumesh.com/register/dataset/" + m]
paths += ["/x/interval/datumesh.com/relay/treasury.gov/par_yield?tenor=10Y&from=2026-11-16&to=2026-11-24",
          "/x/interval/datumesh.com/relay/ecb.europa.eu/fx_reference?currency=USD&from=2024-01-01&to=2024-06-30",
          "/x/interval/datumesh.com/relay/treasury.gov/par_yield?tenor=10Y&from=1999-01-01&to=2026-11-24",
          # The window of Bank Rate the fake serves as its whole slice (local 0.5.2, item 12).
          "/x/interval/datumesh.com/relay/bankofengland.co.uk/bank_rate?from=2026-09-01&to=2026-11-20",
          "/x/interval/datumesh.com/relay/nothing/here?from=2024-01-01&to=2024-01-02",
          # A ranged slice has no URI on production (SPEC 4.4): its refusal (local 0.5.2, item 12).
          "/slice/datumesh.com/relay/bankofengland.co.uk/bank_rate(date:[2026-10-06..2026-11-05])"]
if len(sys.argv) > 1:
    paths = sys.argv[1:]
seen = set()
def get(p):
    if p in seen: return None
    seen.add(p)
    try:
        r = urllib.request.urlopen(urllib.request.Request(BASE + p, headers={"User-Agent": "datumesh-local-tests"}))
        return r.status, r.headers.get("content-type", ""), r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.headers.get("content-type", ""), e.read()
def emit(p):
    x = get(p)
    if x is None: return None
    st, ct, body = x
    print(json.dumps({"path": p, "status": st, "ct": ct, "body": body.decode()}, ensure_ascii=False, separators=(",", ":")))
    return st, body
for p in paths:
    r = emit(p)
    if r is None or r[0] != 200: continue
    d = json.loads(r[1])
    if p.startswith("/bindings"):
        for v in d["bindings"].values():
            if "fn_hash" in v: emit("/descriptor/" + v["fn_hash"])
    if p.startswith("/slice/") and "licence_clauses" in p:
        for m in d["members"]:
            emit("/blob/" + m["receipt"]["value"].split(":")[0])
    if p.startswith("/describe/datumesh.com/register/dataset") and "methods" in d:
        for m in d["methods"].values():
            emit("/descriptor/" + m["fn_hash"])
