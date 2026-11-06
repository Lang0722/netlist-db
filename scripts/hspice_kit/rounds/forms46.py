#!/usr/bin/env python3
"""Round 46: the reach of a name, read from a deep level. usage: forms46.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + N + body + TAIL
def tower(depth, top_param, leaf, mode, sub_param=(0, "")):
    rows = [mode, top_param, ".subckt s0 d g", leaf, ".ends"]
    for k in range(1, depth):
        rows.append(f".subckt s{k} d g")
        if k == sub_param[0]:
            rows.append(sub_param[1])
        rows += [f"X0 d g s{k-1}", ".ends"]
    rows.append(f"X0 d g s{depth-1}")
    return "\n".join(r for r in rows if r) + "\n"
for d in (10, 30, 31, 32, 33, 40, 64, 99, 100, 150):
    for tag, mode in (("g", ""), ("l", ".option parhier=local")):
        deck(f"{tag}_read_{d}", tower(d, ".param u=7n", "M0 d g 0 0 n l=u w=1u", mode))
        deck(f"{tag}_expr_{d}", tower(d, ".param u=7n", "M0 d g 0 0 n l='u*2' w=1u", mode))
        deck(f"{tag}_fun_{d}", tower(d, ".param f(x)='x*2'", "M0 d g 0 0 n l='f(3n)' w=1u", mode))
        deck(f"{tag}_mid_{d}", tower(d, "", "M0 d g 0 0 n l=u w=1u", mode, (max(1, d - 1), ".param u=7n")))
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
