#!/usr/bin/env python3
"""Round 25: a key after a value that is an expression. usage: forms25.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n.param k=2u j=3u\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
for i, tail in enumerate([
    "1u k w=5u", "1u k*1 w=5u", "1u 2u*1 w=5u", "k 2u l=5u", "k*1 2u l=5u", "2u*1 3u l=5u", "1u 'k' w=5u", "1u '2u' w=5u", "l=1u k w=5u", "l=1u 'k' w=5u", "l=1u '2u' w=5u", "1u (k) w=5u",
    "1u (k*1) w=5u", "l=1u (k*1) w=5u", "l=1u 2u*1 w=5u", "1u '2u*1' w=5u", "l=1u '2u*1' w=5u", "1u 2umos w=5u", "1u 2n7002 w=5u", "1u 2e-6 w=5u", "1u +2u w=5u", "1u +k w=5u",
    "k j w=5u l=6u", "w=5u 1u k", "w=5u 1u 2u", "l=5u k", "l=5u k w=6u", "l=5u 1u", "l=5u 1u w=6u", "k l=5u", "1u l=5u", "'1u' l=5u", "1u*1 l=5u", "k*1 l=5u",
]):
    deck(f"kx_{i:02d}", N + f"M1 d g 0 0 n {tail}\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
