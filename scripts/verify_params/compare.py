#!/usr/bin/env python3
"""compare.py A.tsv B.tsv [--exact]: compare two listings (path, ..., L, W, NF,
model) by path: L, W and NF within 1e-5 (relative; ngspice prints 6 digits) or
1e-9 with --exact, and the model (a model local to a subckt instance is
compared by its own name)."""
import sys
def load(p):
    out = {}
    lines = open(p).read().splitlines()
    head = lines[0].split("\t")
    for line in lines[1:]:
        f = line.split("\t")
        row = dict(zip(head, f))
        out[row["path"].lower()] = row
    return out
a, b = load(sys.argv[1]), load(sys.argv[2])
# `PREFIX.*` rows: MOSFETs (under PREFIX) that one side could not name
skipped = 0
for side, other in ((a, b), (b, a)):
    for p in [p for p in side if p.endswith(".*")]:
        del side[p]
        for q in [q for q in other if q.startswith(p[:-1])]:
            del other[q]
            skipped += 1
tol = 1e-9 if "--exact" in sys.argv else 1e-5
def num(x):
    try:
        return float(x)
    except (TypeError, ValueError):
        return None
def close(x, y):
    x, y = num(x), num(y)
    if x is None or y is None:
        return x is None and y is None
    return abs(x - y) <= tol * max(abs(x), abs(y))
bad = []
for p in sorted(set(a) | set(b)):
    if p not in a or p not in b:
        bad.append(f"{p}: only in {'A' if p in a else 'B'}")
        continue
    ra, rb = a[p], b[p]
    ma, mb = ra["model"].lower(), rb["model"].lower()
    if ma.startswith("error") or mb.startswith("error"):
        if not (ma.startswith("error") and mb.startswith("error")):
            bad.append(f"{p}: model {ra['model']} vs {rb['model']}")
        continue
    for k in ("L", "W"):
        if not close(ra[k], rb[k]):
            bad.append(f"{p}: {k} {ra[k]} vs {rb[k]}")
    if not close(ra["NF"], rb["NF"]):
        bad.append(f"{p}: NF {ra['NF']} vs {rb['NF']}")
    if ma != mb:
        bad.append(f"{p}: model {ra['model']} vs {rb['model']} (L={ra['L']} W={ra['W']} NF={ra['NF']})")
print(f"{len(a)} vs {len(b)} MOSFETs, {len(bad)} differ" + (f" ({skipped} not compared)" if skipped else ""))
for x in bad[:15]:
    print("  " + x)
sys.exit(1 if bad else 0)
