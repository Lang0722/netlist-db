#!/usr/bin/env python3
"""Round 33: scalm and scale against nfin limits. usage: forms33.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
def two(lo1, hi1, lo2, hi2):
    return (f".model q.1 nmos level=72 lmin=1e-9 lmax=1 nfinmin={lo1} nfinmax={hi1}\n"
            f".model q.2 nmos level=72 lmin=1e-9 lmax=1 nfinmin={lo2} nfinmax={hi2}\n")
for s, sv in (("n", None), ("2", "2"), ("h", "0.5"), ("u", "1e-6"), ("k", "1000")):
    opt = f".option scalm={sv}\n" if sv else ""
    for nf_name, nf in (("none", ""), ("1", " nfin=1"), ("5", " nfin=5"), ("10", " nfin=10"), ("30", " nfin=30"), ("100", " nfin=100"), ("3m", " nfin=3m")):
        for lim, cards in (("a", two("0.5", "8", "8", "1000")), ("b", two("4", "8", "8", "20"))):
            deck(f"fs_{s}_{nf_name}_{lim}", opt + cards + f"M1 d g 0 0 q l=1u{nf}\n")
    # only one card: is `nfin` read at all
    deck(f"fs_{s}_one", opt + ".model q nmos level=72\n" + "M1 d g 0 0 q l=1u nfin=5\n")
    # with lmin/lmax that scalm also scales
    deck(f"fs_{s}_scaled_l", opt + f".model q.1 nmos level=72 lmin=1e-3 lmax=1e6 nfinmin=0.5 nfinmax=1000\n" + "M1 d g 0 0 q l=1u nfin=5\n")
for s, sv in (("2", "2"), ("h", "0.5"), ("u", "1e-6")):
    opt = f".option scale={sv}\n"
    for nf_name, nf in (("none", ""), ("5", " nfin=5"), ("10", " nfin=10"), ("30", " nfin=30")):
        deck(f"ft_{s}_{nf_name}", opt + two("0.5", "8", "8", "20") + f"M1 d g 0 0 q l=1{nf}\n")
    deck(f"ft_{s}_w", opt + f".model q.1 nmos level=72 lmin=1e-9 lmax=1e6 wmin=1e-9 wmax=1e-9 nfinmin=0.5 nfinmax=1000\n" + "M1 d g 0 0 q l=1 w=1 nfin=5\n")
# wsheet with scalm, for the same reading
for s, sv in (("2", "2"), ("u", "1e-6")):
    opt = f".option scalm={sv}\n"
    deck(f"fw_{s}_a", opt + ".model q.1 nmos level=72 lmin=1e-9 lmax=1e6 wsheetmin=10n wsheetmax=30n\n" + "M1 d g 0 0 q l=1u wsheet=20n\n")
    deck(f"fw_{s}_b", opt + ".model q.1 nmos level=72 lmin=1e-9 lmax=1e6 wsheetmin=10n wsheetmax=30n\n" + "M1 d g 0 0 q l=1u wsheet=40n\n")
    deck(f"fw_{s}_c", opt + ".model q.1 nmos level=72 lmin=1e-9 lmax=1e6 wsheetmin=10n wsheetmax=30n\n" + "M1 d g 0 0 q l=1u wsheet=25p\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
