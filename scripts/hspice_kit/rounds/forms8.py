#!/usr/bin/env python3
"""Round 8: the open questions of the fixes. usage: forms8.py OUTDIR"""
import os, shutil, sys
D = {}
FILES = {}
TAIL = "Vd d 0 1\nVg g 0 1\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body, files=None, tail=TAIL):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + tail
    FILES.update(files or {})
# values in quotes after a key
deck("rk_key_quoted", N + "M1 d g 0 0 n l=1u '2u'\n")
deck("rk_key_quoted_name", N + ".param pl=2u\nM1 d g 0 0 n l=1u 'pl'\n")
deck("rk_quoted_key_quoted", N + "M1 d g 0 0 n '1u' w=2u '3u'\n")
deck("rk_dq_after_key", N + 'M1 d g 0 0 n l=1u "2u"\n')
deck("rk_two_keys_two_vals", N + "M1 d g 0 0 n l=1u w=2u 3u 4u\n")
deck("rk_values_after_two", N + "M1 d g 0 0 n l=1u w=2u 3u 4u off\n")
# an undefined name at a place past W
deck("ru_place3", N + "M1 d g 0 0 n 1u 2u foo\n")
deck("ru_after_keys", N + "M1 d g 0 0 n l=1u w=2u foo\n")
deck("ru_after_keys4", N + "M1 d g 0 0 n l=1u w=2u 1u foo\n")
deck("ru_defined", N + ".param foo=1p\nM1 d g 0 0 n l=1u w=2u foo\n")
# simulation variables
deck("sv_hertz", N + "M1 d g 0 0 n l='hertz+1u' w=1u\n")
deck("sv_time", N + "M1 d g 0 0 n l='time+1u' w=1u\n")
# logarithm of small values
for k, e in {"log_30": "log(1e-30)", "log10_30": "log10(1e-30)", "log_neg30": "log(-1e-30)", "log_27": "log(1e-27)", "log10_27": "log10(1e-27)", "log_29": "log(1e-29)"}.items():
    deck(f"lg_{k}", N + f".param c='{e}'\nM1 d g 0 0 n l='(c+1000)*1e-9' w=1u\n")
# a subckt on an M line
SUBQ = ".subckt q d g s b\nMa d g s b n l=3u w=4u\n.ends\n"
deck("mc_no_option", N + SUBQ + "M1 d g 0 0 q\n")
deck("mc_macmod1", ".option macmod=1\n" + N + SUBQ + "M1 d g 0 0 q\n")
deck("mc_macmod2", ".option macmod=2\n" + N + SUBQ + "M1 d g 0 0 q\n")
deck("mc_macmod3", ".option macmod=3\n" + N + SUBQ + "M1 d g 0 0 q\n")
deck("mc_macmod_params", ".option macmod=1\n" + N + ".subckt q d g s b l=3u w=4u\nMa d g s b n l=l w=w\n.ends\nM1 d g 0 0 q l=5u w=6u\n")
deck("mc_macmod_both", ".option macmod=1\n" + N + ".model q nmos level=54\n" + SUBQ + "M1 d g 0 0 q l=5u w=6u\n")
deck("mc_macmod_bins", ".option macmod=3\n" + N + ".subckt q.1 d g s b\nMa d g s b n l=3u w=4u\n.ends\nM1 d g 0 0 q\n")
deck("mc_x_model", N + "X1 d g 0 0 n l=1u w=1u\n")
deck("mc_x_model_macmod", ".option macmod=1\n" + N + "X1 d g 0 0 n l=1u w=1u\n")
# L=0 with other levels and with XL
deck("z_l0_xl", ".model n nmos level=54 xl=1u\nM1 d g 0 0 n l=0 w=1u\n")
deck("z_l0_lv1", ".model n nmos level=1\nM1 d g 0 0 n l=0 w=1u\n")
deck("z_l0_lv49", ".model n nmos level=49 version=3.2\nM1 d g 0 0 n l=0 w=1u\n")
deck("z_l0_lv72", ".model n nmos level=72\nM1 d g 0 0 n l=0\n")
deck("z_w0_lv1", ".model n nmos level=1\nM1 d g 0 0 n l=1u w=0\n")
deck("z_defl0", ".option defl=0\n" + N + "M1 d g 0 0 n w=1u\n")
deck("z_m_expr0", N + ".param k=0\nM1 d g 0 0 n l=1u w=1u m=k\n")
deck("z_m_half", N + "M1 d g 0 0 n l=1u w=1u m=0.5\n")
# FinFET bins: details
A = ".model f.1 nmos level=72 lmin=1e-8 lmax=1e-5 nfinmin=1 nfinmax=4\n.model f.2 nmos level=72 lmin=1e-8 lmax=1e-5 nfinmin=4 nfinmax=9\n"
deck("h_scalm_nfin", ".option scalm=1e-1\n" + A + "M1 d g 0 0 f l=500n nfin=6\n")
deck("h_scalm_nfin_big", ".option scalm=10\n.model f.1 nmos level=72 lmin=1e-9 lmax=1e-6 nfinmin=1 nfinmax=4\n.model f.2 nmos level=72 lmin=1e-9 lmax=1e-6 nfinmin=4 nfinmax=9\nM1 d g 0 0 f l=500n nfin=6\n")
deck("h_scalm_sheet", ".option scalm=10\n.model f.1 nmos level=72 lmin=1e-9 lmax=1e-6 wsheetmin=1e-9 wsheetmax=1e-8\n.model f.2 nmos level=72 lmin=1e-9 lmax=1e-6 wsheetmin=1e-8 wsheetmax=1e-6\nM1 d g 0 0 f l=500n wsheet=5e-8\n")
deck("h_scale_sheet", ".option scale=10\n.model f.1 nmos level=72 lmin=1e-9 lmax=1e-3 wsheetmin=1e-9 wsheetmax=1e-7\n.model f.2 nmos level=72 lmin=1e-9 lmax=1e-3 wsheetmin=1e-7 wsheetmax=1e-5\nM1 d g 0 0 f l=500n wsheet=5e-8\n")
deck("h_six_w_out", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wmin=1e-3 wmax=1 nfinmin=1 nfinmax=4\nM1 d g 0 0 f l=500n w=1u nfin=2\n")
deck("h_both_nfin_fits", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4 wsheetmin=1e-3 wsheetmax=1\nM1 d g 0 0 f l=500n nfin=2 wsheet=1e-8\n")
deck("h_both_sheet_fits", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=5 nfinmax=9 wsheetmin=1e-9 wsheetmax=1e-6\nM1 d g 0 0 f l=500n nfin=2 wsheet=1e-8\n")
deck("h_model_nfin_only", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=5 nfinmax=9 nfin=6\nM1 d g 0 0 f l=500n\n")
deck("h_two_pass", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=9\n.model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=9\nM1 d g 0 0 f l=500n nfin=2\n")
deck("h_nfin_quoted", A + "M1 d g 0 0 f l=500n nfin='6'\n")
deck("h_sheet_no_line", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=0 wsheetmax=1e-6\nM1 d g 0 0 f l=500n\n")
deck("h_fins_limit_name", ".param a=4\n.model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=a\n.model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=a nfinmax=9\nM1 d g 0 0 f l=500n nfin=6\n")
# `$`
deck("dl_node_digit", N + "M1 1$a g 0 0 n l=1u w=1u\n")
deck("dl_model_end", N + ".model q$ nmos level=54\nM1 d g 0 0 q$ l=1u w=1u\n")
deck("dl_model_comment", N + "M1 d g 0 0 n$ comment\n")
deck("dl_after_name_kv", N + ".param k=1u\nM1 d g 0 0 n l=k$X=1 w=1u\n")
deck("dl_after_word", N + "M1 d g 0 0 n$X=1 l=1u w=1u\n")
# items of a list
deck("cc_param_trailing", N + ".param a=1u,\nM1 d g 0 0 n l=a w=1u\n")
deck("cc_option_trailing", N + ".option defl=1u,\nM1 d g 0 0 n w=1u\n")
# includes
deck("in_over_plus", ".inc\n+ 'in_a.inc'\n" + N + "M1 d g 0 0 n l=k w=1u\n", {"in_a.inc": ".param k=1u\n"})
deck("se_from_sub", ".inc 'se_sub/a.inc'\n" + N + "M1 d g 0 0 n l=k w=1u\n",
     {"se_sub/a.inc": ".option search='se_d'\n.inc 'se_x.inc'\n", "se_sub/se_d/se_x.inc": ".param k=1u\n", "se_d/se_x.inc": ".param k=2u\n"})
deck("se_from_sub_only", ".inc 'se2_sub/a.inc'\n" + N + "M1 d g 0 0 n l=k w=1u\n",
     {"se2_sub/a.inc": ".option search='se2_d'\n.inc 'se2_x.inc'\n", "se2_sub/se2_d/se2_x.inc": ".param k=1u\n"})
deck("se_after_found_elsewhere", ".inc 'se3_a.inc'\n" + N + "M1 d g 0 0 n l=k w=1u\n.option search='se3_d'\n", {"se3_a.inc": ".inc 'se3_x.inc'\n", "se3_d/se3_x.inc": ".param k=1u\n"})
# a nested library section
deck("ln_two", ".lib 'ln.l' a\n" + N + "M1 d g 0 0 n l=k w=1u\n", {"ln.l": ".lib a\n.lib b\n.param k=1u\n.endl b\n.endl a\n"})
deck("ln_inner", ".lib 'ln2.l' b\n" + N + "M1 d g 0 0 n l=k w=1u\n", {"ln2.l": ".lib a\n.lib b\n.param k=1u\n.endl b\n.endl a\n"})
deck("ln_before", ".lib 'ln3.l' a\n" + N + "M1 d g 0 0 n l=k w=1u\n", {"ln3.l": ".lib a\n.param k=1u\n.lib b\n.param k=2u\n.endl b\n.endl a\n"})
deck("ln_call_inner", ".lib 'ln4.l' a\n" + N + "M1 d g 0 0 n l=k w=1u\n", {"ln4.l": ".lib a\n.lib 'ln4.l' b\n.lib b\n.param k=2u\n.endl b\n.endl a\n"})
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
