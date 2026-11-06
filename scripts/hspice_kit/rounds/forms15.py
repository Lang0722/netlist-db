#!/usr/bin/env python3
"""Round 15: the 4th word names a model and values follow the 5th word; orders of
instances with an `.elseif` branch first. usage: forms15.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
PL = ".model n1 nmos level=54\n.model plain nmos level=54\n.param plain=2u\n"
for k, tail in {
    "kw": "w=1u",
    "num_kw": "3u w=1u",
    "quoted_kw": "'3u' w=1u",
    "num": "3u",
    "quoted": "'3u'",
    "num_num": "3u 1u",
    "quoted_quoted": "'3u' '1u'",
    "kw_num": "w=1u 3u",
    "kw_quoted": "w=1u '3u'",
    "off": "off",
    "off_kw": "off w=1u",
    "name_kw": "plain w=1u",
    "three": "3u 1u 2p",
}.items():
    deck(f"w6_{k}", PL + f"M1 d g 0 n1 plain {tail}\n")
    deck(f"w7_{k}", PL + f"M1 d g 0 e1 plain {tail}\n")
    deck(f"w8_{k}", PL + f"M1 d g 0 0 plain {tail}\n")
NOP = ".model n1 nmos level=54\n.model plain nmos level=54\n"
for k, tail in {"num_kw": "3u w=1u", "quoted": "'3u'", "num_num": "3u 1u", "kw": "w=1u", "kw_num": "w=1u 3u"}.items():
    deck(f"w9_{k}", NOP + f"M1 d g 0 n1 plain {tail}\n")
deck("wa_bin_4th", ".model n1.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n.model plain nmos level=54\n.param plain=2u\nM1 d g 0 n1 plain 3u w=1u\n")
deck("wa_bin_5th", ".model n1 nmos level=54\n.model plain.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n.param plain=2u\nM1 d g 0 n1 plain 3u w=1u\n")
deck("wa_two_after", ".model n1 nmos level=54\n.model p2 nmos level=54\n.param p2=2u\nM1 d g 0 x n1 p2 w=1u\n")
deck("wa_two_after_val", ".model n1 nmos level=54\n.model p2 nmos level=54\n.param p2=2u\nM1 d g 0 x n1 p2 3u w=1u\n")
deck("wa_three_names", ".model n1 nmos level=54\n.model n2 nmos level=54\n.model n3 nmos level=54\n.param n2=2u n3=3u\nM1 d g 0 n1 n2 n3 w=1u\n")
deck("wa_three_names_val", ".model n1 nmos level=54\n.model n2 nmos level=54\n.model n3 nmos level=54\n.param n2=2u n3=3u\nM1 d g 0 n1 n2 n3 4u w=1u\n")
deck("wa_node_is_0", ".model 0 nmos level=54\n")
del D["wa_node_is_0"]
def mac(cond1, cond2):
    return f".subckt mac d g s b ll=1u ww=1u\n.if ({cond1})\nM0 d g s b n l=ll w=ww\n.elseif ({cond2})\nM0 d g s b n l='ll*1.5' w=ww\n.else\nM0 d g s b n l='ll*2' w='ww/2'\n.endif\n.ends\n"
INST = {"I": "ll=3u ww=1u", "E": "ll=5u ww=1u", "F": "ll=5u ww=5u", "i": "ll=3u ww=5u"}
def calls(order):
    return "".join(f"X{k} d g s 0 mac {INST[c]}\n" for k, c in enumerate(order))
for order in ["FIE", "FEI", "FIEI", "FEIE"]:
    deck(f"o_{order}_plain", N + mac("ll<4u", "ww>4u") + calls(order))
    deck(f"o_{order}_quoted", N + mac("'ll<4u'", "ww>4u") + calls(order))
    deck(f"o_{order}_dq", N + mac('"ll<4u"', "ww>4u") + calls(order))
    deck(f"o_{order}_both", N + mac("'ll<4u'", "'ww>4u'") + calls(order))
    deck(f"o_{order}_second", N + mac("ll<4u", "'ww>4u'") + calls(order))
    deck(f"o_{order}_group", N + mac("(ll<4u)", "ww>4u") + calls(order))
deck("o_two_blocks", N + ".subckt mac d g s b ll=1u ww=1u\n.if ('ll<4u')\nM0 d g s b n l=ll w=ww\n.else\n.if (ww>4u)\nM0 d g s b n l='ll*1.5' w=ww\n.else\nM0 d g s b n l='ll*2' w='ww/2'\n.endif\n.endif\n.ends\n" + calls("FIE"))
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
