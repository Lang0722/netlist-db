#!/usr/bin/env python3
"""Round 45: the depth rule of parhier=global, loops of parameters, `temper` at the
end of a chain, a lone `+` in the node list, the reach of a model. usage: forms45.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body, model=N):
    assert name not in D, name
    D[name] = f"* {name}\n" + model + body + TAIL
def tower(depth, below, top, leaf="M0 d g 0 0 n l=v w=1u", head="", inner=""):
    rows = [head, f".subckt s0 d g{below[0]}", inner, leaf, ".ends"]
    for k in range(1, depth):
        rows += [f".subckt s{k} d g{below[0]}", inner, f"X0 d g s{k-1}{below[1]}", ".ends"]
    rows.append(f"X0 d g s{depth-1}{top}")
    return "\n".join(r for r in rows if r) + "\n"
DEPTHS = (10, 30, 31, 32, 33, 40, 62, 63, 64, 65, 96)
for d in DEPTHS:
    deck(f"g_self_{d}", tower(d, (" v=1n", " v='v+1n'"), " v=100n"))
    deck(f"g_const_{d}", tower(d, (" v=1n", " v=5n"), " v=100n"))
    deck(f"g_top_{d}", tower(d, (" v=1n", " v=5n"), " v=5n", head=".param v=100n"))
    deck(f"g_def_{d}", tower(d, (" v=5n", ""), "", head=".param v=100n"))
    deck(f"g_inner_{d}", tower(d, ("", ""), "", head=".param v=100n", inner=".param v=5n"))
    deck(f"g_other_{d}", tower(d, (" v=1n", " v='u+v'"), " v=100n", head=".param u=1n"))
    deck(f"l_top_{d}", ".option parhier=local\n" + tower(d, (" v=1n", " v=5n"), " v=5n", head=".param v=100n"))
# loops of parameters
deck("lp_two", ".param a='b+1u' b='a+1u'\nM1 d g 0 0 n l=a w=b\n")
deck("lp_two_swap", ".param b='a+1u' a='b+1u'\nM1 d g 0 0 n l=a w=b\n")
deck("lp_read_b", ".param a='b+1u' b='a+1u'\nM1 d g 0 0 n l=b w=a\n")
deck("lp_three", ".param a='b+1u' b='c+1u' c='a+1u'\nM1 d g 0 0 n l=a w=b\nM2 d g 0 0 n l=c w=1u\n")
deck("lp_self", ".param a='a+1u'\nM1 d g 0 0 n l=a w=1u\n")
deck("lp_self2", ".param a='a+1u' b='a+1u'\nM1 d g 0 0 n l=a w=b\n")
deck("lp_unused", ".param a='b+1u' b='a+1u'\nM1 d g 0 0 n l=1u w=1u\n")
deck("lp_sub", ".subckt c d g\n.param a='b+1u' b='a+1u'\nM1 d g 0 0 n l=a w=b\n.ends\nX1 d g c\n")
deck("lp_sub_top", ".param a=7u\n.subckt c d g\n.param a='b+1u' b='a+1u'\nM1 d g 0 0 n l=a w=b\n.ends\nX1 d g c\n.option parhier=local\n")
deck("lp_pass", ".subckt c d g a='b+1u' b='a+1u'\nM1 d g 0 0 n l=a w=b\n.ends\nX1 d g c\n")
# `temper` at the end of a chain, in a condition
def chain(n, first):
    return first + "\n" + "\n".join(f".param p{k}='p{k-1}+1n'" for k in range(1, n + 1)) + "\n"
for n in (5, 99, 100, 150):
    deck(f"tc_{n}", chain(n, ".param p0='temper'") + f".if (p{n}>1n)\nM1 d g 0 0 n l=1u w=1u\n.else\nM1 d g 0 0 n l=2u w=1u\n.endif\n")
# a lone `+` in the node list
deck("pl_d", "M1 + d g 0 0 n 1u 2u\n")
deck("pl_g", "M1 d + g 0 0 n 1u 2u\n")
deck("pl_s", "M1 d g + 0 0 n 1u 2u\n")
deck("pl_b", "M1 d g 0 + 0 n 1u 2u\n")
deck("pl_m", "M1 d g 0 0 + n 1u 2u\n")
deck("pl_v", "M1 d g 0 0 n + 1u 2u\n")
deck("pl_k", "M1 d g 0 0 + n l=1u w=2u\n")
deck("pl_paren", "M1 d g 0 0 n l=1u + w=2u\n")
deck("pl_key", "M1 d g 0 0 n + l=1u w=2u\n")
deck("pl_end", "M1 d g 0 0 n l=1u w=2u +\n")
# the reach of a model, from the leaf to a level above
for reach in (97, 98, 99, 100, 101, 150):
    rows = [".subckt s0 d g v=1n", "M0 d g 0 0 n l=v w=1u", ".ends"]
    for k in range(1, 160):
        rows.append(f".subckt s{k} d g v=1n")
        if k == reach:
            rows.append(".model n nmos level=54")
        rows += [f"X0 d g s{k-1}", ".ends"]
    rows.append("X0 d g s159 v=100n")
    deck(f"rch_{reach}", "\n".join(rows) + "\n", model="")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
