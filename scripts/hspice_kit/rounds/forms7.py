#!/usr/bin/env python3
"""Round 7: FinFET bins (level 72), `wl` values, log(0). usage: forms7.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\n.option list nomod post=0\n.end\n"
def deck(name, body, tail=TAIL):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + tail
A = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\n.model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=4 nfinmax=9\n"
# which bin: the fin count against the limits
for nfin in (1, 2, 3.5, 4, 5, 8.5, 9, 10, 0.5):
    deck(f"f_nfin_{str(nfin).replace('.', 'p')}", A + f"M1 d g 0 0 f l=500n nfin={nfin}\n")
deck("f_no_nfin", A + "M1 d g 0 0 f l=500n\n")
deck("f_no_nfin_w", A + "M1 d g 0 0 f l=500n w=1u\n")
deck("f_nfin_w", A + "M1 d g 0 0 f l=500n w=1u nfin=6\n")
deck("f_nfin_nf", A + "M1 d g 0 0 f l=500n nfin=6 nf=2\n")
deck("f_nfin_nf_big", A + "M1 d g 0 0 f l=500n nfin=3 nf=2\n")
deck("f_nfin_m", A + "M1 d g 0 0 f l=500n nfin=3 m=2\n")
deck("f_nfin_expr", A + ".param k=6\nM1 d g 0 0 f l=500n nfin=k\n")
deck("f_nfin_expr2", A + ".param k=3\nM1 d g 0 0 f l=500n nfin='k*2'\n")
deck("f_l_out", A + "M1 d g 0 0 f l=2u nfin=3\n")
deck("f_l_edge", A + "M1 d g 0 0 f l=1u nfin=3\n")
deck("f_scale", ".option scale=1e-6\n" + A + "M1 d g 0 0 f l=0.5 nfin=6\n")
deck("f_scalm", ".option scalm=1e-1\n" + A + "M1 d g 0 0 f l=500n nfin=6\n")
deck("f_scale_nfin", ".option scale=2\n" + A + "M1 d g 0 0 f l=250n nfin=3\n")
deck("f_positional", A + "M1 d g 0 0 f 500n 1u nfin=6\n")
# keys: which sets of limits a bin may have
deck("k_all_six", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3 nfinmin=1 nfinmax=4\n.model f.2 nmos level=72 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3 nfinmin=4 nfinmax=9\nM1 d g 0 0 f l=500n w=1u nfin=6\n")
deck("k_no_lmin", ".model f.1 nmos level=72 lmax=1u nfinmin=1 nfinmax=4\nM1 d g 0 0 f l=500n nfin=2\n")
deck("k_no_nfinmax", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1\nM1 d g 0 0 f l=500n nfin=2\n")
deck("k_lw", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\nM1 d g 0 0 f l=500n w=1u\n")
deck("k_lw_nfin_line", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\nM1 d g 0 0 f l=500n w=1u nfin=2\n")
WS = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-9 wsheetmax=1e-8 nsheetmin=1 nsheetmax=4\n.model f.2 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6 nsheetmin=1 nsheetmax=4\n"
deck("w_sheet_two", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-9 wsheetmax=1e-8\n.model f.2 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6\nM1 d g 0 0 f l=500n wsheet=2e-8\n")
deck("w_sheet_four", WS + "M1 d g 0 0 f l=500n wsheet=2e-8 nsheet=2\n")
deck("w_sheet_w", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-9 wsheetmax=1e-8\n.model f.2 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6\nM1 d g 0 0 f l=500n w=2e-8\n")
# a plain model of level 72, and a mix of plain and bins
deck("p_plain", ".model f nmos level=72\nM1 d g 0 0 f l=500n nfin=6\n")
deck("p_plain_no_nfin", ".model f nmos level=72\nM1 d g 0 0 f l=500n\n")
deck("p_other_level_nfin", ".model f.1 nmos level=54 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\nM1 d g 0 0 f l=500n w=1u nfin=2\n")
deck("p_70_lw", ".model f.1 nmos level=70 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\nM1 d g 0 0 f l=500n w=1u\n")
deck("p_mixed_levels", ".model f.1 nmos level=54 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n.model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\nM1 d g 0 0 f l=500n w=1u nfin=2\n")
deck("p_level_expr", ".param lv=72\n.model f.1 nmos level=lv lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\nM1 d g 0 0 f l=500n nfin=2\n")
deck("p_level_quoted", ".model f.1 nmos level='72' lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\nM1 d g 0 0 f l=500n nfin=2\n")
deck("p_nfin_key_model", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4 nfin=2\nM1 d g 0 0 f l=500n\n")
deck("p_nfin_key_model2", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4 nfin=9\nM1 d g 0 0 f l=500n nfin=2\n")
# `wl` values after `wl`
P = ".model n nmos level=54\nM1 d g 0 0 n 1u 2u\n"
deck("wv_then_quoted", ".option wl\n.option wl='1'\n" + P)
deck("wv_then_named", ".param q=1\n.option wl\n.option wl=q\n" + P)
deck("wv_quoted_then", ".option wl='1'\n.option wl\n" + P)
deck("wv_then_two", ".option wl\n.option wl=2\n" + P)
deck("wv_float", ".option wl=1.0\n" + P)
deck("wv_upper", ".OPTION WL\n" + P)
deck("wv_half", ".option wl=0.5\n" + P)
deck("wv_scale", ".option wl scale=1e-6\n.model n nmos level=54\nM1 d g 0 0 n 1 2\n")
deck("wv_x_values", ".option wl\n.model n nmos level=54\n.subckt s d g\nM1 d g 0 0 n 1u 2u\n.ends\nX1 d g s\n")
# log(0)
deck("g_log0", ".model n nmos level=54\n.param c='log(0)'\nM1 d g 0 0 n l='(c+1000)*1e-9' w=1u\n")
deck("g_log100", ".model n nmos level=54\n.param c='log10(0)'\nM1 d g 0 0 n l='(c+1000)*1e-9' w=1u\n")
deck("g_logneg", ".model n nmos level=54\n.param c='log(-2)'\nM1 d g 0 0 n l='(c+1000)*1e-9' w=1u\n")
deck("g_sqrtneg", ".model n nmos level=54\n.param c='sqrt(-4)'\nM1 d g 0 0 n l='(c+1000)*1e-9' w=1u\n")
deck("g_powneg", ".model n nmos level=54\n.param c='(-8)**0.5'\nM1 d g 0 0 n l='(c+1000)*1e-9' w=1u\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
