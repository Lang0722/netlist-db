#!/usr/bin/env python3
"""Round 40: signs and groups as words of an M line. usage: forms40.py OUTDIR"""
import os, shutil, sys
D = {}
HEAD = ".model n nmos level=54\n.param k=4u j=5u\n"
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
LINES = [
 "1u + 2u", "1u +2u", "1u +(2u)", "1u + (2u)", "1u +k", "1u + k", "1u -k", "1u - k", "1u -2u", "1u - 2u",
 "1u + 2u 3u", "1u + w=3u", "1u + l=3u", "+ 1u", "+1u", "+ 1u 2u", "+ l=1u w=2u", "- 1u", "l=1u + 2u", "l=1u + k", "l=1u +2u", "l=1u +k", "l=1u -2u",
 "l=1u w=2u + 3u", "l=1u w=k + j", "w=2u + 3u", "w=2u + l=3u", "w=2u + 3u l=1u", "w=2u +3u", "w=2u + 3u + 4u",
 "l=(k)+(j)", "l=(k)+(j) w=1u", "l=(2u)+(3u)", "l=(2u)+(3u) w=1u", "l=(k)-(j)", "l=(k)-(j) w=1u", "l=(k)/(j)", "l=(k)*(j)", "l=(k)^2", "l=(k)**2", "l=(k)+1u", "l=(k)-1u", "l=(k)+1u w=9u", "l=(k)-1u w=9u", "l=(2u)-1u", "l=(2u)+1u",
 "l=(k+j)+(j)", "l=(k+j)-(j)", "l=(k+j)+1u", "l=(k+j)*2", "l=(k+j)/2",
 "l=(1u\n+ +2u)", "l=(1u\n+ 2u)", "l=(1u\n+ 2u) w=3u", "w=(1u\n+ 2u) l=3u", "l=(k\n+ +j)", "l=(k\n+ j)", "l=(k+\n+ j)", "l=(\n+ 1u)", "l=(\n+ 1u+2u)", "l=(1u+2u\n+ )", "l=(1u+2u\n+ ) w=3u",
 "l=(2u) w=1u", "l=((2u)) w=1u", "l=((2u)+1u) w=1u", "l=(1u+(2u)) w=1u", "l=(1u+(2u)*2) w=1u", "l=(1u*(2+1)) w=1u", "l=((1u+2u)*2) w=1u", "l=((1u+2u)*(1+1)) w=1u", "l=(2*(k)) w=1u", "l=(sqrt(4)*1u) w=1u", "l=(max(1u,2u)) w=1u", "l=(max(1u,2u)*2) w=1u", "l=(abs(2u)) w=1u", "l=(abs(2u)*2) w=1u", "l=(2*abs(2u)) w=1u",
]
for i, x in enumerate(LINES):
    deck(f"w{i:03d}", HEAD + f"M1 d g 0 0 n {x}\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
