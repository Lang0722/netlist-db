#!/usr/bin/env python3
"""Round 44: where a top model goes out of reach. usage: forms44.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
def tower(d, leaf_model="", top_model=N):
    rows = [".subckt s0 d g v=1n"] + ([leaf_model] if leaf_model else []) + ["M0 d g 0 0 n l=v w=1u", ".ends"]
    for k in range(1, d):
        rows += [f".subckt s{k} d g v=1n", f"X0 d g s{k-1} v='v+1n'", ".ends"]
    rows.append(f"X0 d g s{d-1} v=100n")
    return top_model + "\n".join(rows) + "\n"
for d in range(91, 100):
    deck(f"t_{d}", tower(d))
for d in (100, 150, 200, 255, 256):
    deck(f"l_{d}", tower(d, ".model n nmos level=54", ""))
# a one-word path name (`x0` against a long name)
for d in (60, 90, 95):
    t = tower(d).replace("X0 d g", "Xlonginstancename d g")
    deck(f"n_{d}", t)
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
