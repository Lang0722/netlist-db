#!/usr/bin/env python3
"""Large decks with one unusual shape each, for HSPICE. usage: mods.py OUTDIR"""
import os, random, shutil, sys
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(out + "/decks")
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, text):
    open(f"{out}/decks/{name}.sp", "w").write(f"* {name}\n{text}{TAIL}")
r = random.Random(1)
# many bins
lines = [f".model n.{k} nmos level=54 lmin={k*10}n lmax={(k+1)*10}n wmin=1n wmax=1" for k in range(1, 2001)]
order = list(range(1, 2001)) * 2
r.shuffle(order)
lines += [f"M{i} d g s 0 n l={k*10+5}n w=1u" for i, k in enumerate(order)]
deck("bins2k", "\n".join(lines) + "\n")
# a long chain of parameters
lines = [".model n nmos level=54", ".param p0=1n"] + [f".param p{k}='p{k-1}+1n'" for k in range(1, 50001)]
lines += [f"M{i} d g s 0 n l=p{50000 - i*50} w=1u" for i in range(1000)]
deck("params50k", "\n".join(lines) + "\n")
# many subckts
lines = [".model n nmos level=54"]
for k in range(20000):
    lines += [f".subckt s{k} d g s q=1n", f"M0 d g s 0 n l='q*{k+1}' w=1u", ".ends"]
order = list(range(20000)); r.shuffle(order)
lines += [f"X{i} d g s s{k} q={1 + (k % 7)}n" for i, k in enumerate(order)]
deck("sub20k", "\n".join(lines) + "\n")
# a wide fan-out: distinct values
lines = [".model n nmos level=54", ".subckt c d g s p=1", "M0 d g s 0 n l='(p+1)*1n' w='(p%3+1)*1u'".replace("(p%3+1)", "1"), ".ends"]
lines += [f"X{i} d g s c p={i}" for i in range(100000)]
deck("fan100k", "\n".join(lines) + "\n")
# deep: 200 levels, a value that grows on the way, and two instances at some levels
lines = [".model n nmos level=54", ".option parhier=local", ".subckt s0 d g v=1n", "M0 d g 0 0 n l=v w=1u", ".ends"]
for k in range(1, 200):
    lines.append(f".subckt s{k} d g v=1n")
    lines.append(f"X0 d g s{k-1} v='v+1n'")
    if k % 20 == 0:
        lines.append(f"X1 d g s{k-1} v='v+2n'")
    lines.append(".ends")
lines.append("X0 d g s199 v=100n")
deck("deep200", "\n".join(lines) + "\n")
# many `.if` blocks in one cell, 1000 instances
lines = [".model n nmos level=54", ".subckt c d g s p=0"]
for k in range(64):
    lines += [f".if (p<{(k+1)*16})", f"M{k} d g s 0 n l={k+1}u w=1u", ".else", f"M{k} d g s 0 n l={k+1}u w=2u", ".endif"]
lines.append(".ends")
lines += [f"X{i} d g s c p={i}" for i in range(1000)]
deck("if64", "\n".join(lines) + "\n")
# global parameters
lines = [".model n nmos level=54", ".option parhier=global", ".param g=3n", ".subckt c d g s g=1n h=2n", "M0 d g s 0 n l='g+h' w=1u", ".ends"]
lines += [f"X{i} d g s c h={i+1}n" for i in range(30000)]
deck("global30k", "\n".join(lines) + "\n")
print(len(os.listdir(out + "/decks")), "decks")
