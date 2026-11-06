#!/usr/bin/env python3
"""Round 9: words after a list value, FinFET bins with both kinds of limit,
nested sections in a deck, `hertz` in a condition, DEFL 0. usage: forms9.py OUTDIR"""
import os, shutil, sys
D = {}
FILES = {}
TAIL = "Vd d 0 1\nVg g 0 1\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body, files=None, tail=TAIL):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + tail
    FILES.update(files or {})
# bins that tell L and W apart by the model name
B = "".join(f".model n.{i} nmos level=54 lmin={l1} lmax={l2} wmin={w1} wmax={w2}\n" for i, (l1, l2, w1, w2) in enumerate([("1e-9","2u","1e-9","2u"),("1e-9","2u","2u","1"),("2u","1","1e-9","2u"),("2u","1","2u","1")], 1))
for k, tail in {
    "ic_only": "ic=1,1,1",
    "ic_then_kv": "ic=1,1,1 l=1u w=3u",
    "ic_l_then_word": "l=1u ic=1,1,1 3u",
    "ic_then_words": "ic=1,1,1 1u 3u",
    "ic_two": "ic=1,2 1u 3u",
    "ic_one_then_word": "ic=1 1u 3u",
    "ic_spaces": "ic=1, 1, 1 l=1u",
    "l_list": "l=1u,3u",
    "w_list": "l=1u w=1u,3u",
    "l_list_w": "l=1u,3u w=1u",
    "words_many": "1u 3u 1p 1p 1u 1u 1 1 1 1",
    "ad_list": "l=1u ad=1p,3u",
}.items():
    deck(f"lt_{k}", B + f"M1 d g 0 0 n {tail}\n")
# FinFET bins with both kinds of limit, and NSHEET
BOTH = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4 wsheetmin=1e-8 wsheetmax=1e-6\n.model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=4 nfinmax=9 wsheetmin=1e-8 wsheetmax=1e-6\n"
deck("hb_nfin_only", BOTH + "M1 d g 0 0 f l=500n nfin=6\n")
deck("hb_sheet_only", BOTH + "M1 d g 0 0 f l=500n wsheet=2e-8\n")
deck("hb_neither", BOTH + "M1 d g 0 0 f l=500n\n")
deck("hb_both_fit", BOTH + "M1 d g 0 0 f l=500n nfin=6 wsheet=2e-8\n")
deck("hb_both_w", BOTH + "M1 d g 0 0 f l=500n w=2e-8\n")
OTHER = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4 wsheetmin=1e-8 wsheetmax=1e-7\n.model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4 wsheetmin=1e-7 wsheetmax=1e-6\n"
deck("hb_sheet_tells", OTHER + "M1 d g 0 0 f l=500n nfin=2 wsheet=2e-7\n")
deck("hb_sheet_only_tells", OTHER + "M1 d g 0 0 f l=500n wsheet=2e-7\n")
deck("hb_sheet_keys_nfin_only", OTHER + "M1 d g 0 0 f l=500n nfin=2\n")
MIX = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6\n.model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=9\n"
deck("hb_mix_nfin", MIX + "M1 d g 0 0 f l=500n nfin=2\n")
deck("hb_mix_sheet", MIX + "M1 d g 0 0 f l=500n wsheet=2e-8\n")
deck("hb_mix_both", MIX + "M1 d g 0 0 f l=500n nfin=2 wsheet=2e-8\n")
deck("hb_nfin_bin_sheet_line", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\nM1 d g 0 0 f l=500n wsheet=2e-8\n")
deck("hb_sheet_bin_nfin_line", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6\nM1 d g 0 0 f l=500n nfin=2 wsheet=2e-8\n")
deck("hb_nsheet_out", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6 nsheetmin=3 nsheetmax=9\nM1 d g 0 0 f l=500n wsheet=2e-8 nsheet=1\n")
deck("hb_nsheet_none", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6 nsheetmin=3 nsheetmax=9\nM1 d g 0 0 f l=500n wsheet=2e-8\n")
deck("hb_nsheet_only", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nsheetmin=1 nsheetmax=9\nM1 d g 0 0 f l=500n nsheet=2\n")
deck("hb_wsheet_zero_line", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=0 wsheetmax=1e-6\nM1 d g 0 0 f l=500n wsheet=0\n")
deck("hb_nfin_zero_line", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=0 nfinmax=4\nM1 d g 0 0 f l=500n nfin=0\n")
deck("hb_nfin_min_zero", ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=0 nfinmax=4\nM1 d g 0 0 f l=500n\n")
# nested section definitions in a deck, not called
deck("ln_deck_uncalled", ".lib a\n.lib b\n.param k=1u\n.endl b\n.endl a\n.param k=2u\n" + N + "M1 d g 0 0 n l=k w=1u\n")
deck("ln_deck_one_called", ".lib 'ln_deck_one_called.sp' b\n.lib a\n.lib b\n.param k=1u\n.endl b\n.endl a\n" + N + "M1 d g 0 0 n l=k w=1u\n")
deck("ln_file_extra_endl", ".lib 'lx.l' a\n" + N + "M1 d g 0 0 n l=k w=1u\n", {"lx.l": ".lib a\n.param k=1u\n.endl a\n.endl a\n"})
deck("ln_deck_stray_endl", ".endl\n.param k=1u\n" + N + "M1 d g 0 0 n l=k w=1u\n")
deck("ln_three", ".lib 'ly.l' a\n" + N + "M1 d g 0 0 n l=k w=1u\n", {"ly.l": ".lib a\n.lib b\n.lib c\n.param k=1u\n.endl c\n.endl b\n.endl a\n"})
deck("ln_three_call", ".lib 'lz.l' c\n" + N + "M1 d g 0 0 n l=k w=1u\n", {"lz.l": ".lib a\n.param k=2u\n.lib b\n.lib c\n.param k=1u\n.endl c\n.endl b\n.endl a\n"})
deck("ln_two_after_inner", ".lib 'lw.l' a\n" + N + "M1 d g 0 0 n l=k w=1u\n", {"lw.l": ".lib a\n.lib b\n.param k=1u\n.endl b\n.param k=3u\n.endl a\n"})
deck("ln_sibling_outer_called", ".lib 'lv.l' a\n" + N + "M1 d g 0 0 n l=k w=1u\n", {"lv.l": ".lib a\n.param k=1u\n.endl a\n.lib c\n.param k=9u\n.endl c\n"})
deck("ln_wrong_endl_name", ".lib 'lu.l' a\n" + N + "M1 d g 0 0 n l=k w=1u\n", {"lu.l": ".lib a\n.param k=1u\n.endl zzz\n.param k=5u\n.lib c\n.endl c\n"})
# `hertz` in a condition
deck("sv_hertz_if", ".if (hertz>1)\n.param k=1u\n.else\n.param k=2u\n.endif\n" + N + "M1 d g 0 0 n l=k w=1u\n")
deck("sv_hertz_param_if", ".param h='hertz'\n.if (h>1)\n.param k=1u\n.else\n.param k=2u\n.endif\n" + N + "M1 d g 0 0 n l=k w=1u\n")
deck("sv_hertz_bin", ".model n.1 nmos level=54 lmin='hertz+1e-9' lmax=1 wmin=1e-9 wmax=1\nM1 d g 0 0 n l=1u w=1u\n")
# DEFL and DEFW of 0, with bins
BZ = ".model n.1 nmos level=54 lmin=1e-9 lmax=2u wmin=1e-9 wmax=1\n.model n.2 nmos level=54 lmin=2u lmax=1 wmin=1e-9 wmax=1\n"
deck("zd_defl0_bins", ".option defl=0\n" + BZ + "M1 d g 0 0 n w=1u\n")
deck("zd_defl1u_bins", ".option defl=1u\n" + BZ + "M1 d g 0 0 n w=1u\n")
deck("zd_defl_neg", ".option defl=-1u\n" + N + "M1 d g 0 0 n w=1u\n")
deck("zd_defl_neg_bins", ".option defl=-1u\n" + BZ + "M1 d g 0 0 n w=1u\n")
deck("zd_scale0", ".option scale=0\n" + N + "M1 d g 0 0 n l=1u w=1u\n")
deck("zd_scale_neg", ".option scale=-1\n" + N + "M1 d g 0 0 n l=1u w=1u\n")
deck("zd_defw0_bins", ".option defw=0\n.model n.1 nmos level=54 lmin=1e-9 lmax=1 wmin=0 wmax=1\nM1 d g 0 0 n l=1u\n")
deck("zd_defw0_nobin", ".option defw=0\n" + N + "M1 d g 0 0 n l=1u\n")
deck("zd_w0_bins", ".model n.1 nmos level=54 lmin=1e-9 lmax=1 wmin=0 wmax=1\nM1 d g 0 0 n l=1u w=0\n")
deck("zd_l0_xl_bins", ".model n.1 nmos level=54 lmin=1e-9 lmax=2u wmin=1e-9 wmax=1 xl=1u\n.model n.2 nmos level=54 lmin=2u lmax=1 wmin=1e-9 wmax=1 xl=3u\nM1 d g 0 0 n l=0 w=1u\n")
deck("zd_lneg_xl", ".model n nmos level=54 xl=3u\nM1 d g 0 0 n l=-1u w=1u\n")
deck("zd_l_small_xl_neg", ".model n nmos level=54 xl=-3u\nM1 d g 0 0 n l=1u w=1u\n")
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
