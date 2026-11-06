#!/usr/bin/env python3
"""Round 29: the expression on a 2-word line, in a subckt, with `scale`, against other lines. usage: forms29.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
E = "'2.900919e+00+7613.602745291m'"
L = "(46365950.5132n*1)"
line = f"M1 d g 0 0 n {L} {E}\n"
deck("y_top", N + line)
deck("y_sub", N + f".subckt s d g\n{line}.ends\nX1 d g s\n")
deck("y_scale", N + ".option scale=1e-6\n" + line)
deck("y_scale_sub", N + f".option scale=1e-6\n.subckt s d g\n{line}.ends\nX1 d g s\n")
deck("y_l1", N + f"M1 d g 0 0 n 1u {E}\n")
deck("y_l2", N + f"M1 d g 0 0 n 1 {E}\n")
deck("y_l3", N + f"M1 d g 0 0 n 0.0463 {E}\n")
deck("y_l4", N + f"M1 d g 0 0 n {L} '2e+00+1'\n")
deck("y_l5", N + f"M1 d g 0 0 n {L} '2+1'\n")
deck("y_l6", N + f"M1 d g 0 0 n (1u*1) '2+1'\n")
deck("y_l7", N + f"M1 d g 0 0 n 1u*1 '2+1'\n")
deck("y_l8", N + f"M1 d g 0 0 n 1u (2u*1) '2+1'\n")
deck("y_l9", N + f"M1 d g 0 0 n 1u*1 '2+1' '3p'\n")
deck("y_la", N + f"M1 d g 0 0 n 1u*1 2+1\n")
deck("y_lb", N + f"M1 d g 0 0 n k*1 '2+1'\n.param k=1u\n")
deck("y_lc", N + f"M1 d g 0 0 n k '2+1'\n.param k=1u\n")
deck("y_ld", N + f"M1 d g 0 0 n '1u' '2+1'\n")
deck("y_le", N + f"M1 d g 0 0 n '1u*1' '2+1'\n")
deck("y_lf", N + f"M1 d g 0 0 n 1u '2+1'\n")
deck("y_lg", N + f"M1 d g 0 0 n 1u '2*3'\n")
deck("y_lh", N + f"M1 d g 0 0 n 1u*1 '2*3'\n")
deck("y_li", N + f"M1 d g 0 0 n 1u*1 '2*3+1'\n")
deck("y_lj", N + f"M1 d g 0 0 n 1u*1 'k+1'\n.param k=2\n")
deck("y_lk", N + f"M1 d g 0 0 n 1u*1 w='2+1'\n")
deck("y_ll", N + f"M1 d g 0 0 n l=(1u*1) '2+1'\n")
deck("y_lm", N + f"M1 d g 0 0 n l='1u*1' '2+1'\n")
deck("y_ln", N + f"M1 d g 0 0 n l=1u*1 '2+1'\n")
deck("y_lo", N + f"M1 d g 0 0 n (1u*1) '2+1' w=5u\n")
deck("y_lp", N + f"M1 d g 0 0 n 1u+1u '2+1'\n")
deck("y_lq", N + f"M1 d g 0 0 n 1u/1 '2+1'\n")
deck("y_lr", N + f"M1 d g 0 0 n 1u-0 '2+1'\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
