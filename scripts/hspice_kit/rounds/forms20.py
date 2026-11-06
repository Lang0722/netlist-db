#!/usr/bin/env python3
"""Round 20: which forms of a condition lose track of a value (order F, I, E).
usage: forms20.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
def mac(c1, c2="ww>4u"):
    return f".subckt mac d g s b ll=1u ww=1u\n.if ({c1})\nM0 d g s b n l=ll w=ww\n.elseif ({c2})\nM0 d g s b n l='ll*1.5' w=ww\n.else\nM0 d g s b n l='ll*2' w='ww/2'\n.endif\n.ends\n"
FIE = "X0 d g s 0 mac ll=5u ww=5u\nX1 d g s 0 mac ll=3u ww=1u\nX2 d g s 0 mac ll=5u ww=1u\n"
for k, c in {
    "plain": "ll<4u", "group": "(ll<4u)", "and_groups": "(ll<4u)&&(ww<9u)", "and_plain": "ll<4u&&ww<9u", "group_and": "((ll<4u)&&(ww<9u))",
    "func": "max(ll,1u)<4u", "not": "!(ll>=4u)", "times": "ll*1<4u", "group_times": "(ll*1)<4u", "ternary": "ll<4u?1:0", "reverse": "4u>ll",
    "param_read": "lq<4u", "abs": "abs(ll)<4u", "spaces": " ll < 4u ",
}.items():
    extra = ".param lq='ll*1'\n" if k == "param_read" else ""
    deck(f"cf_{k}", N + mac(c).replace(".if", extra + ".if", 1) + FIE)
# the second condition in other forms, the first plain
deck("cf2_second_group", N + mac("ll<4u", "(ww>4u)") + FIE)
deck("cf2_second_quoted", N + mac("ll<4u", "'ww>4u'") + FIE)
# the first condition reads a value that the second one reads too
deck("cf3_both_read", N + mac("'ll<4u&&ww<9u'", "ww>4u") + FIE)
# three values, order changes the outcome: a real wrapper, with a default
deck("cf4_wrapper", N + ".subckt mac d g s b l=1u w=1u mode=0\n.if ((l<4u)&&(mode==0))\nM0 d g s b n l=l w=w\n.elseif (w>4u)\nM0 d g s b n l='l*1.5' w=w\n.else\nM0 d g s b n l='l*2' w='w/2'\n.endif\n.ends\nX0 d g s 0 mac l=5u w=5u\nX1 d g s 0 mac l=3u w=1u\nX2 d g s 0 mac l=5u w=1u\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
