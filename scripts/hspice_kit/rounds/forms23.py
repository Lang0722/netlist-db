#!/usr/bin/env python3
"""Round 23: groups and bare expressions with more forms. usage: forms23.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n.param k=2u j=3u\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
for i, tail in enumerate([
    "1u (2u + 3u)", "1u (k * 2)", "l=1u (2u + 3u)", "1u (2u 3u)", "1u (2u,3u)", "l=1u (2u 3u)", "1u ( 2u )", "1u (2u) 3p", "1u (2u*1) w=5u",
    "l=1u (2u*1) w=5u", "1u ((2u*1))", "1u ((2u+1u)*2)", "1u (max(2u,3u))", "1u max(2u,3u)", "1u 'max(2u,3u)'", "l=1u (max(2u,3u))", "1u (2u)(3u)",
    "1u (2u", "1u 2u)", "1u (2u) )", "1u +2u", "1u -2u", "1u 2u+", "1u 2u*", "1u 2*k", "1u 2*(k+1u)", "1u k*(1+1)", "1u (k)*2", "l=1u (k)*2", "(k)*2 1u",
    "1u 2u/2", "1u 4u/2+1u", "1u 2^2", "1u 1e-6*2", "1e-6*2 1u", "1u 1e-6+1u", "1u 1e+0*2u", "1u 2u*1 $ comment", "1u\n+ 2u*1", "1u\n+ (2u*1)", "l=1u\n+ (2u*1)",
    "1u 2umos", "1u 2umos*1", "1u 2u*1mos", "1u 2u**1", "1u 2u.5", "1u 2.u*1", "1u .5u*2", "l=.5u*2 w=1u", "l=2u*1", "1u w=2u*1",
    "1u 2<3", "1u 2u*(1)", "1u 2u*(1)*(1)", "1u (2u*1) (3p*1)", "(1u*1)", "l=1u w=2u (3p*1) (4p*1)", "1u (2u*1)$c", "1u (2u*1)*", "1u 2u*1,3p*1", "1u,(2u*1)", "l=1u,(2u*1)",
]):
    deck(f"gx_{i:02d}", N + f"M1 d g 0 0 n {tail}\n")
deck("gy_three_nodes", N + "M1 d g 0 n 1u 2u*1\n")
deck("gy_three_nodes_group", N + "M1 d g 0 n 1u (2u*1)\n")
deck("gy_three_nodes_first", N + "M1 d g 0 n 1u*1 2u\n")
deck("gy_node_expr", N + "M1 d g 2u*1 0 n 1u 2u\n")
deck("gy_node_plus", N + "M1 1+1 g 0 0 n l=1u w=1u\n")
deck("gy_node_group", N + "M1 (d g 0 0) n 1u 2u\n")
deck("gy_model_group", N + "M1 d g 0 0 (n) 1u 2u\n")
deck("gy_all_group", N + "M1 (d g 0 0 n) 1u 2u\n")
deck("gy_all_group2", N + "M1 (d g 0 0 n 1u 2u)\n")
def mac(c1, c2="ww>4u"):
    return f".subckt mac d g s b ll=1u ww=1u\n.if ({c1})\nM0 d g s b n l=ll w=ww\n.elseif ({c2})\nM0 d g s b n l='ll*1.5' w=ww\n.else\nM0 d g s b n l='ll*2' w='ww/2'\n.endif\n.ends\n"
FIE = "X0 d g s 0 mac ll=5u ww=5u\nX1 d g s 0 mac ll=3u ww=1u\nX2 d g s 0 mac ll=5u ww=1u\n"
M = ".model n nmos level=54\n"
for k, c in {
    "and_same": "(ll<4u)&&(ll<9u)", "group_and_same": "((ll<4u)&&(ll<9u))", "or_same": "(ll<4u)||(ll<1u)", "quoted_and_same": "'ll<4u&&ll<9u'",
    "group3": "((ll<4u))", "plain_and_same": "ll<4u&&ll<9u", "other_param": "(ll*2<8u)", "not_group": "!(ll>=4u)", "quoted_not": "'!(ll>=4u)'", "quoted_max": "'max(ll,1u)<4u'",
}.items():
    deck(f"gz_{k}", M + mac(c) + FIE)
# the three common wrapper styles, first condition reads one value only, order F I E
deck("gz_real_wrapper", M + ".subckt mac d g s b l=1u w=1u\n.if ((l<4u))\nM0 d g s b n l=l w=w\n.elseif (w>4u)\nM0 d g s b n l='l*1.5' w=w\n.endif\n.ends\nX0 d g s 0 mac l=5u w=5u\nX1 d g s 0 mac l=3u w=1u\nX2 d g s 0 mac l=5u w=7u\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
