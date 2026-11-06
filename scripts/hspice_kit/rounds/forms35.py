#!/usr/bin/env python3
"""Round 35: W of a FinFET line against scalm. usage: forms35.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
CARD = ".MODEL q.3 nmos level=72 lmin=1e-3 lmax=1e6 nfinmin=0.5 nfinmax=23\n"
O = ".option scalm=1e-6\n"
for i, line in enumerate([
    "M1 d g 0 0 q l=4u w=4u nfin=4",
    "M1 d g 0 0 q l=4u nfin=4",
    "M1 d g 0 0 q l='2u+2u' w=4u nfin=4",
    "M1 d g 0 0 q l=4u w='2u+2u' nfin=4",
    "M1 d g 0 0 q l='2u+2u' w='2u+2u' nfin=4",
    "M1 d g 0 0 q l=4u w=1 nfin=4",
    "M1 d g 0 0 q l=4u w='k' nfin=4",
    "M1 d g 0 0 q l=4u nfin='2+2'",
    "M1 d g 0 0 q l=4u nfin='k'",
    "M1 d g 0 0 q l=4u nfin=k",
    "M1 d g 0 0 q l='k*1u' nfin=4",
    "M1 d g 0 0 q l=k1 nfin=4",
    "M1 d g 0 0 q l=k1 w=k1 nfin=k",
    "M1 d g 0 0 q 4u 4u nfin=4",
    "M1 d g 0 0 q 4u '4u' nfin=4",
    "M1 d g 0 0 q '4u' 4u nfin=4",
    "M1 d g 0 0 q k1 nfin=4",
]):
    deck(f"h1_{i:02d}", O + CARD + ".param k=4 k1=4u\n" + line + "\n")
    deck(f"h2_{i:02d}", CARD.replace("lmin=1e-3 lmax=1e6", "lmin=1e-9 lmax=1") + ".param k=4 k1=4u\n" + line + "\n")
    deck(f"h3_{i:02d}", ".option scalm=2\n" + CARD.replace("lmin=1e-3 lmax=1e6", "lmin=1e-9 lmax=1") + ".param k=4 k1=4u\n" + line + "\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
