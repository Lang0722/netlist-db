#!/usr/bin/env python3
"""Round 28: an expression with an exponent, on the M line. usage: forms28.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n.param k=2\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
E = ["2.900919e+00+7613.602745291m", "2e+00+1", "2e+0+1", "2e0+1", "2e-00+1", "2e+01+1", "2+1", "2.5+1", "2e+00-1", "2e+00*3", "2e+00+1m", "2e-6+1u", "2e-06+1u", "2u+1u", "k+1", "1+k", "2e+00+k", "(2e+00)+1", "2e+00 + 1", "2e+00 +1"]
for i, e in enumerate(E):
    deck(f"xa_{i:02d}", N + f"M1 d g 0 0 n 1u '{e}'\n")
    deck(f"xb_{i:02d}", N + f"M1 d g 0 0 n l=1u w='{e}'\n")
    deck(f"xc_{i:02d}", N + f"M1 d g 0 0 n '{e}' 1u\n")
    deck(f"xd_{i:02d}", N + f"M1 d g 0 0 n l=1u '{e}'\n")
    deck(f"xe_{i:02d}", N + f".param p='{e}'\nM1 d g 0 0 n l=1u w=p\n")
    deck(f"xf_{i:02d}", N + f".subckt s d g w0='{e}'\nM1 d g 0 0 n l=1u w=w0\n.ends\nX1 d g s\n")
    deck(f"xg_{i:02d}", N + f".subckt s d g w0=1\nM1 d g 0 0 n l=1u w=w0\n.ends\nX1 d g s w0='{e}'\n")
    if " " not in e:
        deck(f"xh_{i:02d}", N + f"M1 d g 0 0 n 1u {e}\n")
        deck(f"xi_{i:02d}", N + f"M1 d g 0 0 n l=1u w={e}\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
