import json, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cmp_list import table, num
base = sys.argv[1]
meta = json.load(open(f"{base}/meta.json"))
res = f"{base}/results"
def tab(deck):
    lines = open(f"{res}/{deck}.lis", errors="replace").read().splitlines()
    errs = [x.strip() for x in lines if "**error**" in x]
    return table(lines), errs
for deck in ("cmp_ternary", "cmp_if"):
    t, errs = tab(deck)
    print(deck, len(t), "rows", errs[:1])
    print(" deltas:", " ".join(f"{i}:{d:g}" for i, d in enumerate(meta["deltas"])))
    for bi, b in enumerate(meta["bases"]):
        for oi, op in enumerate(meta["ops"]):
            for side in (0, 1):
                row = ""
                for di in range(len(meta["deltas"])):
                    r = t.get(f"m{oi}{side}{bi}{di:02d}")
                    row += "?" if r is None else ("T" if abs(num(r["l drawn"]) - 1e-6) < 1e-9 else ".")
                print(f" base {b:<7g} {'a'+op+'b' if side==0 else 'b'+op+'a':6} {row}")
t, errs = tab("cmp_truth")
print("cmp_truth", errs[:1])
for i, v in enumerate(meta["vals"]):
    print(" ", v, " ".join(f"{k}={'T' if abs(num(t[f'm{k}{i}']['l drawn'])-1e-6)<1e-9 else '.'}" for k in "itnao" if f"m{k}{i}" in t))
t, errs = tab("cmp_funcs")
print("cmp_funcs", errs[:1])
for i, v in enumerate(meta["fv"]):
    if f"mf{i}" in t:
        print(" ", v, t[f"mf{i}"].get("l drawn"), t[f"mf{i}"].get("l eff"))
