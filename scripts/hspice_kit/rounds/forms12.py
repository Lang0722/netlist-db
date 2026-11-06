#!/usr/bin/env python3
"""Round 12: reduce deck s1218, where HSPICE chose another `.if` branch.
usage: forms12.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
MAC = ".subckt mac d g s b ll=1.19u ww=0.1u\n.if (ll<4.4412e-06)\nM0 d g s b n l=ll w=ww\n.else\nM0 d g s b n l='ll*2' w='ww/2'\n.endif\n.ends\n"
BLK0 = ".subckt blk0 a b c ka=1.096\nX1 n1 0 b a mac ll='4009.5n*ka' ww=1.99e-7\n.ends\n"
def blk1(lines, default="scale_l=1.683"):
    return f".subckt blk1 a b c {default}\n" + "".join(l + "\n" for l in lines) + ".ends\n"
S0 = "Xs00 n1 a c blk0"
S1 = "Xs01 a b c blk0 ka='scale_l*1.137'"
S2 = "Xs02 b c a blk0"
TOP0 = "X0 d g s blk0 ka=1.862\n"
TOP1 = "X1 d g s blk1\n"
GP = ".param scale_l=1.3737\n"
deck("rd_all", N + GP + MAC + BLK0 + blk1([S0, S1, S2]) + TOP0 + TOP1)
deck("rd_scale", ".option scale=2\n" + N + GP + MAC + BLK0 + blk1([S0, S1, S2]) + TOP0 + TOP1)
deck("rd_no_top0", N + GP + MAC + BLK0 + blk1([S0, S1, S2]) + TOP1)
deck("rd_s1_only", N + GP + MAC + BLK0 + blk1([S1]) + TOP1)
deck("rd_s1_first", N + GP + MAC + BLK0 + blk1([S1, S0, S2]) + TOP1)
deck("rd_s0_s1", N + GP + MAC + BLK0 + blk1([S0, S1]) + TOP1)
deck("rd_s1_s2", N + GP + MAC + BLK0 + blk1([S1, S2]) + TOP1)
deck("rd_number", N + GP + MAC + BLK0 + blk1([S0, "Xs01 a b c blk0 ka=1.5619", S2]) + TOP1)
deck("rd_quoted_number", N + GP + MAC + BLK0 + blk1([S0, "Xs01 a b c blk0 ka='1.5619'", S2]) + TOP1)
deck("rd_two_numbers", N + MAC + BLK0 + blk1([S0, "Xs01 a b c blk0 ka=1.5619"], "") + TOP1)
deck("rd_other_name", N + ".param other=1.3737\n" + MAC + BLK0 + blk1([S0, "Xs01 a b c blk0 ka='other*1.137'", S2], "") + TOP1)
deck("rd_no_global", N + MAC + BLK0 + blk1([S0, S1, S2]) + TOP1)
deck("rd_local", ".option parhier=local\n" + N + GP + MAC + BLK0 + blk1([S0, S1, S2]) + TOP1)
deck("rd_global", ".option parhier=global\n" + N + GP + MAC + BLK0 + blk1([S0, S1, S2]) + TOP1)
deck("rd_top_level_two", N + MAC + BLK0 + "X0 d g s blk0\nX1 d g s blk0 ka=1.5619\n")
deck("rd_top_level_expr", N + ".param q=1.5619\n" + MAC + BLK0 + "X0 d g s blk0\nX1 d g s blk0 ka='q*1'\n")
deck("rd_top_level_rev", N + ".param q=1.5619\n" + MAC + BLK0 + "X1 d g s blk0 ka='q*1'\nX0 d g s blk0\n")
deck("rd_one_level", N + ".param q=6.26u\n" + MAC + "X0 d g s 0 mac\nX1 d g s 0 mac ll=q\nX2 d g s 0 mac ll='q*1'\n")
deck("rd_one_level_rev", N + ".param q=6.26u\n" + MAC + "X2 d g s 0 mac ll='q*1'\nX0 d g s 0 mac\n")
deck("rd_flip_if", N + GP + MAC.replace("ll<4.4412e-06", "ll>4.4412e-06") + BLK0 + blk1([S0, S1, S2]) + TOP1)
deck("rd_three_values", N + ".param q=2\n" + MAC + BLK0 + ".subckt blk1 a b c\nXa a b c blk0 ka=0.5\nXb a b c blk0 ka='q*1'\nXc a b c blk0 ka=1\n.ends\n" + TOP1)
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
