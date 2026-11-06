#!/usr/bin/env python3
"""Round 24: what follows a `)`, odd bare words, keys after values. usage: forms24.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n.param k=2u j=3u\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
for i, tail in enumerate([
    # keys after values, with and without a `)`
    "1u 2u w=5u", "1u 2u l=5u", "1u (2u) w=5u", "l=1u (2u) w=5u", "l=1u 2u w=5u", "1u 2u) w=5u", "1u (2u w=5u", "1u (2u w=5u)", "1u (2u nf=2)", "1u (2u) nf=2", "1u (2u) m=2",
    "l=1u (2u) nf=2", "l=1u 2u nf=2", "l=1u (2u) m=2", "1u 2u) nf=2", "1u (2u nf=2", "w=5u (1u)", "w=5u (1u) l=3u", "(l=1u w=2u)", "(w=2u) l=1u", "(l=1u) w=2u", "l=1u (w=2u)", "l=1u (w=2u) nf=2",
    "l=1u (nf=2) w=2u", "l=1u w=2u (nf=2)", "l=1u w=2u) nf=2", "l=1u) w=2u", "l=(1u) w=2u", "l=(1u) w=(2u) nf=2", "l=(1u)) w=2u", "1u,2u nf=2", "1u,2u,3p nf=2", "l=1u,2u nf=2", "l=1u,2u w=5u",
    # odd bare words
    "1u 2n7002", "1u 2**2", "1u 2^2", "1u k^2", "1u k**2", "1u 2*3^2", "1u 2<3", "1u 2>3", "1u 2==2", "1u 2!=3", "1u 2&&3", "1u 2||3", "1u 2?3:4", "1u k<j", "1u k?j:1", "1u k.5", "1u k$",
    "l=1u w=2^2", "l=1u w=2**2", "l=1u w=2<3", "l=1u w=2>3", "l=1u w=2u.5", "l=1u w=k^2", "l=1u w=k**2", "l=1u w=2*3^2", "l=1u w=2?3:4", "l=1u w=k?j:1",
    "1u 2u%", "1u 2u#", "1u 2u@3", "1u 2u~3", "1u 2u:3", "1u 2u;3", "1u 2u_3", "1u 2u[3]", "1u 2u{3}", "1u 2u\\3", "1u 2u|3", "1u 2u&3",
    "1e-6 2u*1", "1u 2e-6*1", "1u 2E-6/2", "1u 2e-6-1u", "1u 2e-6+1u", "1u 2-1", "1u 2u-1u", "1u -2u*-1", "1u +2u*1", "1u -k*-1", "1u +k", "1u -k",
]):
    deck(f"hx_{i:02d}", N + f"M1 d g 0 0 n {tail}\n")
for i, tail in enumerate(["1u 2u.5", "1u 2<3", "1u 2n7002", "1u 2u*1", "2u.5 1u", "2n7002 1u", "2u*1 1u", "k 2u.5", "k 2n7002"]):
    deck(f"hy_{i:02d}", N + f"M1 d g 0 n {tail}\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
