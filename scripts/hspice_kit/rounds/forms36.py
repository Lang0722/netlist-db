#!/usr/bin/env python3
"""Round 36: order of FinFET lines under scalm. usage: forms36.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
C = lambda f: (f".model q.1 nmos level=72 lmin={1e-9/f:g} lmax={1/f:g} nfinmin=0.5 nfinmax=8\n"
               f".model q.2 nmos level=72 lmin={1e-9/f:g} lmax={1/f:g} nfinmin=8 nfinmax=1000\n")
SEQ = {"5_10": (5, 10), "10_5": (10, 5), "5_5": (5, 5), "10_10": (10, 10), "5_5_10": (5, 5, 10), "5_10_5": (5, 10, 5), "10_5_10": (10, 5, 10), "1_14_20": (1, 14, 20), "20_14_1": (20, 14, 1), "5_12_3": (5, 12, 3), "3_12_5": (3, 12, 5), "2_3_4_5": (2, 3, 4, 5), "15_3_15": (15, 3, 15)}
for s, f in (("2", 2.0), ("h", 0.5), ("4", 4.0), ("n", 1.0)):
    opt = f".option scalm={f:g}\n" if f != 1.0 else ""
    for k, seq in SEQ.items():
        lines = "".join(f"M{i} d g 0 0 q l=1u nfin={n}\n" for i, n in enumerate(seq))
        deck(f"o{s}_{k}", opt + C(f) + lines)
    # the same nfin, another L
    deck(f"o{s}_sameN_otherL", opt + C(f) + "M0 d g 0 0 q l=1u nfin=5\nM1 d g 0 0 q l=2u nfin=5\n")
    deck(f"o{s}_sameN_sameL", opt + C(f) + "M0 d g 0 0 q l=1u nfin=5\nM1 d g 0 0 q l=1u nfin=5\n")
    deck(f"o{s}_other_w", opt + C(f) + "M0 d g 0 0 q l=1u w=1u nfin=5\nM1 d g 0 0 q l=1u w=2u nfin=5\n")
    # in a subckt: two instances with a passed nfin
    deck(f"o{s}_sub", opt + C(f) + ".subckt c d g k=1\nM0 d g 0 0 q l=1u nfin=k\n.ends\nX0 d g c k=5\nX1 d g c k=10\nX2 d g c k=3\n")
    # a plain nmos with bins by W, for the same order: no nfin
    deck(f"o{s}_wbins", opt + f".model w.1 nmos level=54 lmin={1e-9/f:g} lmax={1/f:g} wmin={1e-9/f:g} wmax={1e-5/f:g}\n.model w.2 nmos level=54 lmin={1e-9/f:g} lmax={1/f:g} wmin={1e-5/f:g} wmax={1/f:g}\nM0 d g 0 0 w l=1u w=1u\nM1 d g 0 0 w l=1u w=20u\nM2 d g 0 0 w l=2u w=2u\n")
    # two models
    deck(f"o{s}_two_models", opt + C(f) + C(f).replace("q.", "r.") + "M0 d g 0 0 q l=1u nfin=5\nM1 d g 0 0 r l=1u nfin=10\nM2 d g 0 0 r l=1u nfin=5\nM3 d g 0 0 q l=1u nfin=10\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
