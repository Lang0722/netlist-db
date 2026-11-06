#!/usr/bin/env python3
"""Round 6: which contexts compare with a tolerance; division by small values. usage: forms6.py OUTDIR"""
import os, shutil, sys
D = {}
N = ".model n nmos level=54\n"
TAIL = "Vd d 0 1\nVg g 0 1\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
# a<b differs by 1e-15 (within the tolerance); t=1e-30 (below the truth limit)
AB = ".param a=1 b='1+1e-15' t=1e-30\n"
EXPRS = {"lt": "(a<b)?1u:2u", "gt": "(b>a)?1u:2u", "tr": "(t)?1u:2u", "and": "(t&&1)?1u:2u", "not": "(!t)?1u:2u"}
BAND = ".model b.1 nmos level=54 lmin=1e-8 lmax=1.5u wmin=1e-8 wmax=1e-3\n.model b.2 nmos level=54 lmin=1.5u lmax=1e-3 wmin=1e-8 wmax=1e-3\n"
for k, e in EXPRS.items():
    # on the element line of the top level, and of a subckt
    deck(f"c_{k}_line", N + AB + f"M1 d g 0 0 n l='{e}' w=1u\n")
    deck(f"c_{k}_line_sub", N + AB + f".subckt s d g\nM1 d g 0 0 n l='{e}' w=1u\n.ends\nX1 d g s\n")
    # in a `.param` read by the line
    deck(f"c_{k}_param", N + AB + f".param c='{e}'\nM1 d g 0 0 n l=c w=1u\n")
    deck(f"c_{k}_param_expr", N + AB + f".param c='{e}'\nM1 d g 0 0 n l='c*1' w=1u\n")
    # a `.param` in a subckt, a default, a value passed
    deck(f"c_{k}_param_sub", N + AB + f".subckt s d g\n.param c='{e}'\nM1 d g 0 0 n l=c w=1u\n.ends\nX1 d g s\n")
    deck(f"c_{k}_default", N + AB + f".subckt s d g c='{e}'\nM1 d g 0 0 n l=c w=1u\n.ends\nX1 d g s\n")
    deck(f"c_{k}_passed", N + AB + f".subckt s d g c=9u\nM1 d g 0 0 n l=c w=1u\n.ends\nX1 d g s c='{e}'\n")
    # a function
    deck(f"c_{k}_func", N + AB + f".param f(x)='x*({e})'\nM1 d g 0 0 n l='f(1)' w=1u\n")
    # in a limit of a model
    deck(f"c_{k}_limit", AB + f".model b.1 nmos level=54 lmin=1e-8 lmax='{e}*1.25' wmin=1e-8 wmax=1e-3\n.model b.2 nmos level=54 lmin='{e}*1.25' lmax=1e-3 wmin=1e-8 wmax=1e-3\nM1 d g 0 0 b l=1.2u w=1u\n")
# a value of a comparison read in a line
deck("v_param_lt", N + AB + ".param c='a<b'\nM1 d g 0 0 n l='(c)?1u:2u' w=1u\n")
deck("v_line_lt", N + AB + "M1 d g 0 0 n l='(a<b)*1u+1u' w=1u\n")
deck("v_param_lt_num", N + AB + ".param c='(a<b)*1u+1u'\nM1 d g 0 0 n l=c w=1u\n")
deck("v_param_eq_edge", N + ".param a=1 b='1+1e-12'\n.param c='(a==b)?1u:2u'\nM1 d g 0 0 n l=c w=1u\n")
deck("v_param_eq_near", N + ".param a=1 b='1+1e-14'\n.param c='(a==b)?1u:2u'\nM1 d g 0 0 n l=c w=1u\n")
deck("v_line_eq_near", N + ".param a=1 b='1+1e-14'\nM1 d g 0 0 n l='(a==b)?1u:2u' w=1u\n")
# division
for k, e in {"zero": "1/0", "neg": "-1/0", "tiny": "1/1e-30", "edge": "1/1e-28", "small": "1/1e-20", "zz": "0/0", "tiny2": "1e-40/1e-30", "num0": "2/0"}.items():
    deck(f"d_{k}_line", N + f"M1 d g 0 0 n l='({e})*1e-34+1u' w=1u\n")
    deck(f"d_{k}_param", N + f".param c='({e})*1e-34+1u'\nM1 d g 0 0 n l=c w=1u\n")
for k, e in {"pow_big": "10**400", "exp_big": "exp(1000)", "log0": "log(0)", "log100": "log10(0)", "sqrt0": "sqrt(0)", "pow00": "0**0"}.items():
    deck(f"f_{k}", N + f".param c='{e}'\nM1 d g 0 0 n l='c*1e-40+1u' w=1u\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
