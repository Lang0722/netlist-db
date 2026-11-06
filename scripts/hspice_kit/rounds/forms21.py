#!/usr/bin/env python3
"""Round 21: groups and bare expressions as values on an M line. usage: forms21.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n.param k=2u j=3u\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
for i, tail in enumerate([
    "1u (2u*1)", "1u 2u*1", "(1u*1) (2u*1)", "(1u*1) w=2u", "k*1 j", "(k*1) j", "(k*1) (j*1)", "l=1u (k*1)", "l=1u k*1", "1u (k)", "(k) (j)",
    "1u (2u + 0u)", "l=1u ( 2u*1 )", "1u*1 2u", "1u*1 w=2u", "k*1 w=2u", "l=1u 2u*1", "w=1u (k*1)", "w=1u k*1", "(1u+1u) (2u+1u)", "1u+1u 2u+1u", "k+1u j+1u",
    "l=1u (2u)", "1u (2u)", "(1u) 2u", "(1u)", "1u 2u (1p*1)", "1u 2u 1p*1", "l=1u w=2u (1p*1)", "l=1u w=2u 1p*1", "k/2 j", "(k/2) j", "2u-1u 3u", "(2u-1u) 3u", "-1u 3u", "(-1u+3u) 3u",
    "k-1u 3u", "(k-1u) 3u", "k**1 3u", "2**1 3u",
]):
    deck(f"gv_{i:02d}", N + f"M1 d g 0 0 n {tail}\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
