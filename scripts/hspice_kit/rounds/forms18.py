#!/usr/bin/env python3
"""Round 18: the 4th word names bins that do not hold the size. usage: forms18.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
N1_NO = ".model n1.1 nmos level=54 lmin=1e-9 lmax=1u wmin=1e-9 wmax=1u\n"
N1_YES = ".model n1.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n"
P1_YES = ".model p1.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n"
P1_NO = ".model p1.1 nmos level=54 lmin=1e-9 lmax=1u wmin=1e-9 wmax=1u\n"
P1_PLAIN = ".model p1 nmos level=54\n"
LINE = "M1 d g 0 n1 p1 l=2u w=2u\n"
for a, n in (("no", N1_NO), ("yes", N1_YES)):
    for b, p in (("yes", P1_YES), ("no", P1_NO), ("plain", P1_PLAIN)):
        deck(f"f4_{a}_{b}_top", n + p + LINE)
        deck(f"f4_{a}_{b}_sub", n + p + ".subckt s d g\n" + LINE + ".ends\nX1 d g s\n")
        deck(f"f4_{a}_{b}_pos", n + p + "M1 d g 0 n1 p1 2u 2u\n")
        deck(f"f4_{a}_{b}_param", n + p + ".param p1=3u\nM1 d g 0 n1 p1 w=2u\n")
# a word at the 5th place, 4th a node, whose bins do not hold
deck("f5_no", ".model p1.1 nmos level=54 lmin=1e-9 lmax=1u wmin=1e-9 wmax=1u\n.param p1=3u\nM1 d g 0 x p1 w=2u\n")
deck("f5_two_no", ".model a.1 nmos level=54 lmin=1e-9 lmax=1u wmin=1e-9 wmax=1u\n.model b.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\nM1 d g 0 0 a b l=2u w=2u\n")
deck("f5_two_yes", ".model a.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n.model b.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n.param b=2u\nM1 d g 0 0 a b w=2u\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
