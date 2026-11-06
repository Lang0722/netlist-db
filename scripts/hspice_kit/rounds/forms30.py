#!/usr/bin/env python3
"""Round 30: a group after the model and a value after the group. usage: forms30.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
PL = ".model n nmos level=54\n.param k=2\n"
BI = ".model n.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=100\n.param k=2\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
for tag, cards in (("pl", PL), ("bi", BI)):
    for i, tail in enumerate([
        "(1u*1) '2+1'", "(1u*1) '2e+00+1'", "(1u*1) '2*3'", "(1u*1) 'k+1'", "(1u*1) '1u'", "(1u*1) 'k'", "(1u) '2+1'", "(k*1u) '2+1'", "(1u*1) '2+1' 3p", "(1u*1) '2+1' w=3", "(1u*1) \"2+1\"",
    ]):
        deck(f"z{tag}_{i:02d}", cards + f"M1 d g 0 0 n {tail}\n")
        deck(f"z{tag}_s{i:02d}", cards + f".subckt s d g\nM1 d g 0 q n {tail}\n.ends\nX1 d g s\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
