#!/usr/bin/env python3
"""Round 27: duplicate keys of two kinds, words before the keys, `wl`. usage: forms27.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n.model a nmos level=54\n.param k=2u j=3u b=4u\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
for i, tail in enumerate([
    "w=k w=5u", "w='2u' w=5u", "w=5u w=k", "w=5u w='2u'", "l=k l=1u w=1u", "l=1u w=k w=5u w=6u", "l=1u w=k*1 w=5u", "l=1u w=2u*1 w=5u", "l=1u w=(2u) w=5u", "l=1u w=(k) w=5u", "l=1u w=+2u w=5u", "l=1u w=2umos w=5u", "l=1u w=2e-6 w=5u",
    "l=1u w=+k w=5u", "l=1u w=-k*-1 w=5u",
]):
    deck(f"ma_{i:02d}", N + f"M1 d g 0 0 n {tail}\n")
for i, tail in enumerate([
    "a 1u b l=5u", "a k b l=5u", "a k w=5u 3u", "a k l=5u 3u", "a 1u l=5u 3u", "a b w=5u", "a 1u j w=5u", "a 1u 2u j w=5u", "a b j", "a b j w=5u l=5u", "a 1u b w=5u", "a 1u b l=5u w=6u", "a b 2u l=5u w=6u",
    "a 1u b w=5u 3p", "a k l=j", "a 1u l=j", "a k w=j 2u", "a k 2u w=j", "a k 2u w=j 3p", "a l=5u k", "a l=5u k w=6u", "a l=5u 1u w=6u",
]):
    deck(f"mb_{i:02d}", N + f"M1 d g 0 0 {tail}\n")
W = ".option wl\n"
for i, tail in enumerate([
    "k 1u w=5u", "1u k l=5u", "1u 2u l=5u w=6u", "k j l=5u w=6u", "w=5u 1u", "l=5u 1u", "l=5u k", "w=5u k",
]):
    deck(f"mc_{i:02d}", N + W + f"M1 d g 0 0 n {tail}\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
