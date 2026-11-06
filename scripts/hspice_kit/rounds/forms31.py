#!/usr/bin/env python3
"""Round 31: a group right after the model, and another line uses the model. usage: forms31.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
PL = ".model n nmos level=54\n.param k=2u j=3u\n"
U = "Mb d g s 0 n l=1u w=1u\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
for i, tail in enumerate([
    "(1u*1) (2u*1)", "(1u*1) w=2u", "(k*1) j", "(k*1) (j*1)", "(k) (j)", "(1u+1u) (2u+1u)", "(1u) 2u", "(1u)", "(k/2) j", "(2u-1u) 3u", "(-1u+3u) 3u", "(k-1u) 3u",
    "(1u*1) '2+1'", "(1u*1) '2u+1u'", "(1u*1) 2+1", "(1u*1) 2u+1u", "(1u*1) 'k'", "(1u*1) k", "(1u*1) 2u", "(1u*1) 2u 3p", "(1u*1) 2u w=5u", "(1u*1) w=5u 2u", "(k)*2 1u", "(k)", "(k*2)", "(1u*1)", "(l=1u w=2u)", "(l=1u) w=2u",
    "(1u) (2u)", "(1u)(2u)", "( 1u ) 2u", "(1u 2u)", "(1u,2u)", "(1u, 2u)", "(1u*1 2u*1)", "(1u*1,2u*1)", "(1u*1, 2u*1)", "(1u*1) (2u*1) (3p*1)", "(k j)", "(k,j)", "(k*1 j*1)",
    "('1u') '2u'", "('1u*1') '2u'", "('k') '2u'", "('k*1') 'j*1'", "(1u*1) \"2u\"", "(1u*1) \"2+1\"",
    "(k*1) 2u l=7u", "(k*1) 2u w=7u", "(1u) 2u l=7u", "(1u) 2u w=7u", "(1u*1) 2u nf=2",
]):
    deck(f"zu_{i:02d}", PL + U + f"M1 d g 0 0 n {tail}\n")
    deck(f"zv_{i:02d}", PL + f"M1 d g 0 0 n {tail}\n" + U)
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
