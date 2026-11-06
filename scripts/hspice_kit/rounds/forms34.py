#!/usr/bin/env python3
"""Round 34: W on the line of a FinFET bin, with scalm. usage: forms34.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
def card(k, f, lo, hi):
    return f".model q.{k} nmos level=72 lmin={1e-9/f:g} lmax={1/f:g} nfinmin={lo} nfinmax={hi}\n"
for s, sv, f in (("n", None, 1.0), ("2", "2", 2.0), ("t", "0.001", 1e-3), ("u", "1e-6", 1e-6), ("v", "1e-9", 1e-9)):
    opt = f".option scalm={sv}\n" if sv else ""
    for lim, cards in (("a", card(1, f, "0.5", "8") + card(2, f, "8", "1000")), ("b", card(1, f, "4", "8") + card(2, f, "8", "20"))):
        for ln, tail in (("none", ""), ("n5", "nfin=5"), ("n1", "nfin=1"), ("n30", "nfin=30"), ("w1u", "w=1u"), ("w1un5", "w=1u nfin=5"), ("w1", "w=1"), ("w5", "w=5"), ("w5n1", "w=5 nfin=1"), ("w10", "w=10"), ("w5un5", "w=5u nfin=5"), ("w5un30", "w=5u nfin=30"), ("w5u", "w=5u"), ("w2m", "w=2m"), ("w1p", "w=1p")):
            deck(f"g{s}_{lim}_{ln}", opt + cards + f"M1 d g 0 0 q l=1u {tail}\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
