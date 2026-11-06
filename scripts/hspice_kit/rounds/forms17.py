#!/usr/bin/env python3
"""Round 17: a word that names a model, in a deck with an `.if` block.
usage: forms17.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
BIN = lambda b: f".model {b}.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n"
PLAIN = lambda b: f".model {b} nmos level=54\n"
IFB = ".if (r>1)\n.endif\n"
IFM = ".if (r>1)\nMq d g 0 0 n1 l=3u w=3u\n.endif\n"
R = ".param r=2\n"
for four, cards4 in (("b", BIN("n1")), ("p", PLAIN("n1")), ("x", "")):
    for five, cards5 in (("b", BIN("p1")), ("p", PLAIN("p1"))):
        cards = cards4 + cards5
        for pos, body in {
            "sub_after": R + cards + ".subckt s d g\nM1 d g 0 n1 p1 l=2u w=1u\n.ends\nX1 d g s\n" + IFB,
            "sub_before": R + IFB + cards + ".subckt s d g\nM1 d g 0 n1 p1 l=2u w=1u\n.ends\nX1 d g s\n",
            "top_after": R + cards + "M1 d g 0 n1 p1 l=2u w=1u\n" + IFB,
            "top_before": R + IFB + cards + "M1 d g 0 n1 p1 l=2u w=1u\n",
            "in_sub": R + cards + ".subckt s d g\n.if (r>1)\n.endif\nM1 d g 0 n1 p1 l=2u w=1u\n.ends\nX1 d g s\n",
            "in_branch": R + cards + ".if (r>1)\nM1 d g 0 n1 p1 l=2u w=1u\n.endif\n",
            "sub_in_branch": R + cards + ".subckt s d g\n.if (r>1)\nM1 d g 0 n1 p1 l=2u w=1u\n.endif\n.ends\nX1 d g s\n",
            "cards_in_branch": R + ".if (r>1)\n" + cards + ".endif\n.subckt s d g\nM1 d g 0 n1 p1 l=2u w=1u\n.ends\nX1 d g s\n",
        }.items():
            deck(f"ib_{four}{five}_{pos}", body)
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
