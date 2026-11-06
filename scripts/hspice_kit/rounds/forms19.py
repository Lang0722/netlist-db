#!/usr/bin/env python3
"""Round 19: the 4th word names a model that another line uses. usage: forms19.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
BIN = lambda b: f".model {b}.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n"
PLAIN = lambda b: f".model {b} nmos level=54\n"
MB = "Mb d g s 0 n1 l=1u w=1u\n"
M1 = "M1 d g 0 n1 p1 l=2u w=2u\n"
SUB = ".subckt s d g\n" + M1 + ".ends\nX1 d g s\n"
for k4, c4 in (("b", BIN("n1")), ("p", PLAIN("n1"))):
    for k5, c5 in (("b", BIN("p1")), ("p", PLAIN("p1"))):
        c = c4 + c5
        tag = k4 + k5
        deck(f"u_{tag}_top_then_sub", c + MB + SUB)
        deck(f"u_{tag}_sub_then_top", c + SUB + MB)
        deck(f"u_{tag}_top_before", c + MB + M1)
        deck(f"u_{tag}_top_after", c + M1 + MB)
        deck(f"u_{tag}_two_subs", c + ".subckt a d g s\n" + MB + ".ends\n.subckt b d g\n" + M1 + ".ends\nXa d g s a\nXb d g b\n")
        deck(f"u_{tag}_two_subs_rev", c + ".subckt a d g s\n" + MB + ".ends\n.subckt b d g\n" + M1 + ".ends\nXb d g b\nXa d g s a\n")
        deck(f"u_{tag}_param", c + ".param p1=3u\n" + MB + "M1 d g 0 n1 p1 w=2u\n")
        deck(f"u_{tag}_param_sub", c + ".param p1=3u\n" + MB + ".subckt s d g\nM1 d g 0 n1 p1 w=2u\n.ends\nX1 d g s\n")
        deck(f"u_{tag}_p1_used", c + "Mc d g s 0 p1 l=1u w=1u\n" + MB + M1)
        deck(f"u_{tag}_p1_used_sub", c + "Mc d g s 0 p1 l=1u w=1u\n" + MB + SUB)
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
