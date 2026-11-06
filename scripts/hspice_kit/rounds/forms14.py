#!/usr/bin/env python3
"""Round 14: `.if` blocks with a quoted condition and `.elseif`, for many orders of
instances. usage: forms14.py OUTDIR"""
import itertools, os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
def mac(cond1="'ll<4u'", cond2="ww>4u", else_if=True):
    t = f".subckt mac d g s b ll=1u ww=1u\n.if ({cond1})\nM0 d g s b n l=ll w=ww\n"
    if else_if:
        t += f".elseif ({cond2})\nM0 d g s b n l='ll*1.5' w=ww\n"
    return t + ".else\nM0 d g s b n l='ll*2' w='ww/2'\n.endif\n.ends\n"
INST = {"I": "ll=3u ww=1u", "E": "ll=5u ww=1u", "F": "ll=5u ww=5u", "i": "ll=3u ww=5u"}
def calls(order):
    return "".join(f"X{k} d g s 0 mac {INST[c]}\n" for k, c in enumerate(order))
for order in ["EIEI", "IEIE", "EEII", "IIEE", "EIIE", "IEEI", "EII", "EIE", "IEI", "EI", "IE", "FIE", "FEI", "EFI", "IFE", "iEI", "EiI", "EIF", "EEEIII", "EIEIEI"]:
    deck(f"q_{order}_quoted", N + mac() + calls(order))
for order in ["EIEI", "IEIE", "EEII", "EIE"]:
    deck(f"q_{order}_plain", N + mac("ll<4u") + calls(order))
    deck(f"q_{order}_both", N + mac("'ll<4u'", "'ww>4u'") + calls(order))
    deck(f"q_{order}_second", N + mac("ll<4u", "'ww>4u'") + calls(order))
    deck(f"q_{order}_no_elseif", N + mac(else_if=False) + calls(order))
    deck(f"q_{order}_dq", N + mac('"ll<4u"') + calls(order))
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
