#!/usr/bin/env python3
"""Round 43: long chains of parameters. usage: forms43.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + N + body + TAIL
def chain(n, rev=False, sub=False):
    rows = [".param p0=1n"] + [f".param p{k}='p{k-1}+1n'" for k in range(1, n + 1)]
    return rows[::-1] if rev else rows
for n in (50, 99, 100, 101, 150, 1000, 5000):
    deck(f"c_{n}", "\n".join(chain(n)) + f"\nM1 d g 0 0 n l=p{n} w=1u\n")
    deck(f"r_{n}", "\n".join(chain(n, True)) + f"\nM1 d g 0 0 n l=p{n} w=1u\n")
    deck(f"s_{n}", ".subckt c d g\n" + "\n".join(chain(n)) + f"\nM1 d g 0 0 n l=p{n} w=1u\n.ends\nX1 d g c\n")
    deck(f"sr_{n}", ".subckt c d g\n" + "\n".join(chain(n, True)) + f"\nM1 d g 0 0 n l=p{n} w=1u\n.ends\nX1 d g c\n")
    deck(f"f_{n}", "\n".join(chain(n)) + f"\n.subckt c d g\nM1 d g 0 0 n l=p{n} w=1u\n.ends\nX1 d g c\n")
    deck(f"i_{n}", "\n".join(chain(n)) + f"\n.if (p{n}>1n)\nM1 d g 0 0 n l=p{n} w=1u\n.else\nM1 d g 0 0 n l=1u w=1u\n.endif\n")
# a loop of parameters: HSPICE and netlist-db must both stop
deck("loop_2", ".param a='b' b='a'\nM1 d g 0 0 n l=a w=1u\n")
deck("loop_150", ".param p0='p150+1n'\n" + "\n".join(f".param p{k}='p{k-1}+1n'" for k in range(1, 151)) + "\nM1 d g 0 0 n l=p150 w=1u\n")
# a depth ladder with a model at the top
for d in (10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 120, 150):
    rows = [".subckt s0 d g v=1n", "M0 d g 0 0 n l=v w=1u", ".ends"]
    for k in range(1, d):
        rows += [f".subckt s{k} d g v=1n", f"X0 d g s{k-1} v='v+1n'", ".ends"]
    rows.append(f"X0 d g s{d-1} v=100n")
    deck(f"d_{d}", ".option parhier=local\n" + "\n".join(rows) + "\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
