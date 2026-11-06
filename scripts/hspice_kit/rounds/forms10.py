#!/usr/bin/env python3
"""Round 10: size options of 0 or below, `.lib` definitions in a deck, the first
run of `.alter` decks (through `.meas`, because the table mixes the runs).
usage: forms10.py OUTDIR"""
import os, shutil, sys
D = {}
FILES = {}
TAIL = "Vd d 0 1\nVg g 0 1\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body, files=None, tail=TAIL):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + tail
    FILES.update(files or {})
L50 = ".model n.1 nmos level=54 lmin=1e-9 lmax=50u wmin=1e-9 wmax=1\n.model n.2 nmos level=54 lmin=50u lmax=1 wmin=1e-9 wmax=1\n"
W50 = ".model n.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=50u\n.model n.2 nmos level=54 lmin=1e-9 lmax=1 wmin=50u wmax=1\n"
L2 = ".model n.1 nmos level=54 lmin=1e-9 lmax=2u wmin=1e-9 wmax=1\n.model n.2 nmos level=54 lmin=2u lmax=1 wmin=1e-9 wmax=1\n"
deck("zq_defl0_50", ".option defl=0\n" + L50 + "M1 d g 0 0 n w=1u\n")
deck("zq_defl_neg_50", ".option defl=-1u\n" + L50 + "M1 d g 0 0 n w=1u\n")
deck("zq_defl_then0", ".option defl=1u\n.option defl=0\n" + L2 + "M1 d g 0 0 n w=1u\n")
deck("zq_defl_then_neg", ".option defl=1u defl=-1u\n" + L2 + "M1 d g 0 0 n w=1u\n")
deck("zq_defl1_scale", ".option defl=1u scale=10\n" + L2 + "M1 d g 0 0 n w=1u\n")
deck("zq_defl0_scale", ".option defl=0 scale=1e-2\n" + L2 + "M1 d g 0 0 n w=1u\n")
deck("zq_defw0_50", ".option defw=0\n" + W50 + "M1 d g 0 0 n l=1u\n")
deck("zq_defw_neg_50", ".option defw=-1u\n" + W50 + "M1 d g 0 0 n l=1u\n")
deck("zq_defw_small", ".option defw=1u\n" + W50 + "M1 d g 0 0 n l=1u\n")
deck("zq_scale_then0", ".option scale=2\n.option scale=0\n" + N + "M1 d g 0 0 n l=1u w=1u\n")
deck("zq_scale_then_neg", ".option scale=2 scale=-1\n" + N + "M1 d g 0 0 n l=1u w=1u\n")
deck("zq_scale0_bins", ".option scale=0\n" + L2 + "M1 d g 0 0 n l=1u w=1u\n")
deck("zq_scalm0", ".option scalm=0\n" + L2 + "M1 d g 0 0 n l=1u w=1u\n")
deck("zq_scalm_neg", ".option scalm=-1\n" + L2 + "M1 d g 0 0 n l=1u w=1u\n")
deck("zq_scalm_then0", ".option scalm=1e-3 scalm=0\n.model n.1 nmos level=54 lmin=1e-3 lmax=2e-3 wmin=1e-9 wmax=1e3\nM1 d g 0 0 n l=1.5u w=1u\n")
deck("zq_w_neg", N + "M1 d g 0 0 n l=1u w=-1u\n")
deck("zq_scale_neg_l", ".option scale=-1\n" + N + "M1 d g 0 0 n l=-1u w=1u\n")
# `.lib` definitions in a deck and in an `.inc`
deck("lf_deck_flat", ".lib a\n.param k=1u\n.endl a\n.param k=2u\n" + N + "M1 d g 0 0 n l=k w=1u\n")
deck("lf_deck_self", ".lib 'lf_deck_self.sp' a\n.lib a\n.param k=1u\n.endl a\n" + N + "M1 d g 0 0 n l=k w=1u\n")
deck("lf_deck_self_after", ".lib a\n.param k=1u\n.endl a\n.lib 'lf_deck_self_after.sp' a\n" + N + "M1 d g 0 0 n l=k w=1u\n")
deck("lf_inc_flat", ".inc 'lif.inc'\n.param k=2u\n" + N + "M1 d g 0 0 n l=k w=1u\n", {"lif.inc": ".lib a\n.param k=1u\n.endl a\n"})
deck("lf_lib_in_subckt", ".subckt s d g\n.lib a\n.param k=1u\n.endl a\nM1 d g 0 0 n l=1u w=1u\n.ends\n" + N + "X1 d g s\n")
# the first run of a deck with `.alter`
M = ".option ingold=2 numdgt=10 measdgt=10\n.dc vd 1 1 1\n.meas dc l0 find lv1(m1) at=1\n"
deck("al_param", N + ".param k=1u\nM1 d g 0 0 n l=k w=1u\n" + M + ".alter\n.param k=2u\n")
deck("al_alt", N + ".param k=1u\nM1 d g 0 0 n l=k w=1u\n" + M + ".alt\n.param k=2u\n")
deck("al_alte", N + ".param k=1u\nM1 d g 0 0 n l=k w=1u\n" + M + ".alte\n.param k=2u\n")
deck("al_adds_m", N + "M1 d g 0 0 n l=1u w=1u\n" + M + ".alter\nM2 d g 0 0 n l=2u w=2u\n")
deck("al_wl_after", N + "M1 d g 0 0 n l=1u w=2u\n" + M + ".alter\n.option wl\n")
deck("al_scale_after", N + "M1 d g 0 0 n l=1u w=2u\n" + M + ".alter\n.option scale=2\n")
deck("al_parhier_after", ".param k=1u\n.subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\n" + N + "X1 d g s\n.option ingold=2 numdgt=10 measdgt=10\n.dc vd 1 1 1\n.meas dc l0 find lv1(x1.m1) at=1\n.alter\n.option parhier=global\n")
deck("al_inside_inc", N + "M1 d g 0 0 n l=k w=1u\n.inc 'ali.inc'\n" + M, {"ali.inc": ".param k=1u\n.alter\n.param k=2u\n"})
deck("al_temp_after", N + ".param k='(1+temper/100)*1u'\nM1 d g 0 0 n l=k w=1u\n" + M + ".alter\n.temp 75\n")
deck("al_two", N + ".param k=1u\nM1 d g 0 0 n l=k w=1u\n" + M + ".alter\n.param k=2u\n.alter\n.param k=3u\n")
deck("al_model_after", ".model n.1 nmos level=54 lmin=1e-9 lmax=2u wmin=1e-9 wmax=1\nM1 d g 0 0 n l=1u w=1u\n" + M + ".alter\n.model n.2 nmos level=54 lmin=2u lmax=1 wmin=1e-9 wmax=1\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
for rel, content in FILES.items():
    path = os.path.join(out, "decks", rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, "w").write(content)
print(len(D), "decks")
