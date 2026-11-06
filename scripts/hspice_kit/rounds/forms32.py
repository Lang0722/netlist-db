#!/usr/bin/env python3
"""Round 32: decks where HSPICE leaves an empty listing. usage: forms32.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
IF = ".if(ll<1u)\nM0 d g s b n l=ll w=1u\n.else\nM0 d g s b n l=2u w=1u\n.endif\n"
LO = ".option parhier=local\n"
for tag, opt in (("lo", LO), ("gl", "")):
    # a default that reads a top value that is missing, a passed value that makes it unused
    deck(f"c1_{tag}", N + opt + f".subckt m d g s b ll='3e-7*gl'\n{IF}.ends\nX1 d g s 0 m ll=2u\n")
    deck(f"c2_{tag}", N + opt + f".param gl=2\n.subckt m d g s b ll='3e-7*gl'\n{IF}.ends\nX1 d g s 0 m ll=2u\n")
    deck(f"c3_{tag}", N + opt + f".subckt m d g s b ll='3e-7*gl'\n{IF}.ends\nX1 d g s 0 m\n")
    deck(f"c4_{tag}", N + opt + f".subckt m d g s b ll='3e-7*gl'\n{IF}.ends\n.subckt t d g s\nX1 d g s 0 m ll=2u\n.ends\nX0 d g s t\n")
    deck(f"c5_{tag}", N + opt + f".subckt m d g s b ll=3e-7\n{IF}.ends\n.subckt t d g s ka=1\nX1 d g s 0 m ll='2u*ka'\n.ends\nX0 d g s t\n")
    deck(f"c6_{tag}", N + opt + f".subckt m d g s b ll=3e-7\n{IF}.ends\n.subckt t d g s\nX1 d g s 0 m ll='2u*ka'\n.ends\nX0 d g s t\n")
    deck(f"c7_{tag}", N + opt + f".subckt m d g s b ll=3e-7\n{IF}.ends\n.subckt t d g s ka=1\n.param kl='ka*2'\n.ends\n.subckt u d g s\nX1 d g s 0 m ll='2u*kl'\n.ends\nX0 d g s u\n")
    deck(f"c8_{tag}", N + opt + f".subckt m d g s b ll='3e-7*gl'\n.if(ll<1u)\n.endif\n.ends\nX1 d g s 0 m ll=2u\n")
    deck(f"c9_{tag}", N + opt + f".subckt m d g s b ll=1\n.if(ll<1u)\n.endif\n.ends\n.subckt t d g s\n.param kl=1\n.ends\n.subckt u d g s\nX1 d g s 0 m ll='3u*ka'\n.ends\nX0 d g s u\n")
    deck(f"ca_{tag}", N + opt + f".subckt m d g s b ll=1\n.if(ll<1u)\n.endif\n.ends\n.subckt u d g s ka=1\nX1 d g s 0 m ll='3u*ka'\n.ends\nX0 d g s u\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
