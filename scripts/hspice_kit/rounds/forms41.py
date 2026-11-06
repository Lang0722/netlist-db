#!/usr/bin/env python3
"""Round 41: random words after the model of an M line. usage: forms41.py OUTDIR SEED COUNT"""
import os, random, shutil, sys
HEAD = ".model n nmos level=54\n.param k=4u j=5u\n"
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
PLAIN = ["1u", "2u", "3u", "+2u", "2e-6", "1.5u", "2umos"]
EXPR = ["k", "j", "'k'", "'2u'", "\"2u\"", "k*1", "2u*1", "+k", "'k*2'", "2u+1u", "k+j"]
GROUP = ["(2u)", "(k)", "(2u+1u)", "(k+j)", "( k )", "(2u", "2u)", "(2u 3u)", "(k j)", "(2u,3u)", "(k + j)", "(2u + 1u)"]
KEYS = ["l=5u", "w=6u", "l=k", "w=j", "l='7u'", "w='8u'", "w=(2u)", "w=(k)", "l=+2u", "w=2u*2", "l=(2u+1u)", "w=(k+j)", "l=(2u + 1u)", "w=(k + j)", "w=(k j)", "l=(2u 3u)", "l=2u,", "w=3u,", "L=9u", "W=9u"]
OTHER = ["nf=2", "m=2", "ad=1p", "as=2p", "pd=1u", "nrd=1", "off", "OFF", "sa=1u", "delvto=0.1", "ic=1,2", "ic=1"]
SEP = [",", "+", "(", ")"]
def word(r, first):
    k = r.random()
    if k < 0.25:
        return r.choice(PLAIN)
    if k < 0.45:
        return r.choice(EXPR)
    if k < 0.58 and not first:
        return r.choice(GROUP)
    if k < 0.82:
        return r.choice(KEYS)
    if k < 0.94:
        return r.choice(OTHER)
    return r.choice(SEP) if not first else r.choice(PLAIN)
def line(r):
    n = r.randint(1, 6)
    words = [word(r, i == 0) for i in range(n)]
    out = words[0]
    for w in words[1:]:
        out += r.choice(["\n+ ", " ", " ", " ", " ", "  "]) + w
    return out
out, seed, count = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
r = random.Random(seed)
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
seen = set()
i = 0
while i < count:
    x = line(r)
    if x in seen:
        continue
    seen.add(x)
    open(os.path.join(out, "decks", f"z{i:04d}.sp"), "w").write(f"* z{i:04d}\n{HEAD}M1 d g 0 0 n {x}\n{TAIL}")
    i += 1
print(count, "decks")
