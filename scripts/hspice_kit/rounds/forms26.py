#!/usr/bin/env python3
"""Round 26: an expression at a place against an expression key. usage: forms26.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n.param k=2u j=3u\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
for i, tail in enumerate([
    "w='5u' 1u 2u", "w=j 1u 2u", "w=j 1u k", "w=j k", "w=j k 1u", "w='j' k", "w='j' 'k'", "1u k w=j", "1u k w='j'", "1u 'k' w='j'", "1u 2u w=j", "1u 2u w='5u'", "k l=j", "k l='5u'", "'1u' l='5u'",
    "1u l='5u'", "1u l=j", "l=j 1u", "l=j k", "l='5u' k w=j", "l='5u' 1u w=j", "l='5u' 1u w='6u'", "l=j 'k'", "1u w=j k", "1u w='j' k", "1u w='j' 2u", "1u w=j 2u 3p", "1u w=2u k", "1u w=5u 2u",
    "1u w=5u k w=6u", "1u w=5u 2u w=6u", "w=5u 1u w=6u", "w=5u k w=6u", "w=5u 'k' w='6u'", "w=j w=k", "w=k w=j", "1u w=5u w=6u", "l=1u l=2u w=3u", "l=j l=k w=3u",
    "k j k", "k j j", "k j 'k' w=7u l=7u", "1u k nf=2 w=5u", "1u k m=2 w=5u", "nf=2 1u k", "nf=2 k 1u", "nf=2 m=2 1u k", "nf=2 k l=5u", "nf=2 k w=5u",
]):
    deck(f"ky_{i:02d}", N + f"M1 d g 0 0 n {tail}\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
