#!/usr/bin/env python3
"""Round 11: which values a condition reads under `parhier=global`, the 4th word
naming a model, and empty listings. usage: forms11.py OUTDIR"""
import os, shutil, sys
D = {}
FILES = {}
TAIL = "Vd d 0 1\nVg g 0 1\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body, files=None, tail=TAIL):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + tail
    FILES.update(files or {})
MAC = ".subckt mac d g ll=1u\n.if (ll<1.5u)\nM0 d g 0 0 n l=ll w=1u\n.else\nM0 d g 0 0 n l=ll w=2u\n.endif\n.ends\n"
BLK = ".subckt blk a b ka=1\nX1 a b mac ll='1u*ka'\n.ends\n"
for hier, opt in (("g", ".option parhier=global\n"), ("l", ".option parhier=local\n"), ("d", "")):
    # the chain of s1218: a top value, the same name as a default on the way
    deck(f"ch_{hier}_chain", opt + N + ".param p=2\n" + MAC + BLK + ".subckt blk1 a b p=1\nXs a b blk ka='p*1'\n.ends\nX1 d g blk1\n")
    # one level less
    deck(f"ch_{hier}_two", opt + N + ".param p=2\n" + MAC + ".subckt blk1 a b p=1\nXs a b mac ll='1u*p'\n.ends\nX1 d g blk1\n")
    deck(f"ch_{hier}_direct", opt + N + ".param p=2\n.subckt s a b p=1\n.if (p<1.5)\nM0 a b 0 0 n l=1u w=1u\n.else\nM0 a b 0 0 n l=1u w=2u\n.endif\n.ends\nX1 d g s\n")
    deck(f"ch_{hier}_passed", opt + N + ".param p=2\n.subckt s a b p=1\n.if (p<1.5)\nM0 a b 0 0 n l=1u w=1u\n.else\nM0 a b 0 0 n l=1u w=2u\n.endif\n.ends\nX1 d g s p=1.2\n")
    # a `.param` in the middle, not a default
    deck(f"ch_{hier}_param_mid", opt + N + ".param p=2\n" + MAC + ".subckt blk1 a b\n.param p=1\nXs a b mac ll='1u*p'\n.ends\nX1 d g blk1\n")
    # the value passed is a number, no name in the way
    deck(f"ch_{hier}_number", opt + N + MAC + ".subckt blk1 a b p=1\nXs a b mac ll='1u*p'\n.ends\nX1 d g blk1 p=2\n")
    # a default that passes through a default
    deck(f"ch_{hier}_default_expr", opt + N + ".param p=2\n.subckt s a b p=1 q='p*1'\n.if (q<1.5)\nM0 a b 0 0 n l=1u w=1u\n.else\nM0 a b 0 0 n l=1u w=2u\n.endif\n.ends\nX1 d g s\n")
    # a local `.param` read by the condition, its name is also at the top
    deck(f"ch_{hier}_param_read", opt + N + ".param p=2\n.subckt s a b\n.param p=1\n.param q='p*1'\n.if (q<1.5)\nM0 a b 0 0 n l=q*1u w=1u\n.else\nM0 a b 0 0 n l=q*1u w=2u\n.endif\n.ends\nX1 d g s\n")
    # the value read in a value at the same level, no condition
    deck(f"ch_{hier}_no_if", opt + N + ".param p=2\n.subckt s a b ll=1u\nM0 a b 0 0 n l=ll w=1u\n.ends\n.subckt blk1 a b p=1\nXs a b s ll='1u*p'\n.ends\nX1 d g blk1\n")
    # at the top level: a `.if` that reads a top value
    deck(f"ch_{hier}_top", opt + N + ".param p=2\n.if (p<1.5)\nM0 d g 0 0 n l=1u w=1u\n.else\nM0 d g 0 0 n l=1u w=2u\n.endif\n")
    # three levels
    deck(f"ch_{hier}_three", opt + N + ".param p=2\n" + MAC + BLK + ".subckt b2 a b kb=1\nXs a b blk ka='kb*1'\n.ends\n.subckt b3 a b p=1\nXt a b b2 kb='p*1'\n.ends\nX1 d g b3\n")
    # a name at the top only (no default on the way)
    deck(f"ch_{hier}_top_only", opt + N + ".param p=2\n" + MAC + BLK + ".subckt blk1 a b\nXs a b blk ka='p*1'\n.ends\nX1 d g blk1\n")
    # the name is passed to the level and is at the top
    deck(f"ch_{hier}_passed_top", opt + N + ".param p=2\n" + MAC + BLK + ".subckt blk1 a b p=1\nXs a b blk ka='p*1'\n.ends\nX1 d g blk1 p=3\n")
    # a model in a branch under such a condition
    deck(f"ch_{hier}_model_branch", opt + ".model n.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n.param p=2\n.subckt s a b p=1\n.if (p<1.5)\n.model q.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n.else\n.model q.2 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n.endif\nM0 a b 0 0 q l=1u w=1u\n.ends\nX1 d g s\n")
# the 4th word names a model
PL = ".model n1 nmos level=54\n.model plain nmos level=54\n"
deck("w4_param_named", PL + ".param plain=2u\nM1 d g 0 n1 plain w=1u\n")
deck("w4_undefined", PL + "M1 d g 0 n1 plain w=1u\n")
deck("w4_keys", PL + "M1 d g 0 n1 l=2u w=1u\n")
deck("w4_words", PL + "M1 d g 0 n1 2u 1u\n")
deck("w4_only_fifth", ".model plain nmos level=54\nM1 d g 0 n1 plain l=2u w=1u\n")
deck("w3_names", PL + ".param plain=2u\nM1 d g n1 plain l=2u w=1u\n")
deck("w3_third", PL + "M1 d g n1 l=2u w=1u\n")
deck("w4_model_then_number", PL + "M1 d g 0 n1 2u w=1u\n")
deck("w4_subckt_local", ".model n1 nmos level=54\n.subckt s d g\n.model plain nmos level=54\nM1 d g 0 n1 plain l=2u w=1u\n.ends\n.param plain=3u\nX1 d g s\n")
deck("w4_local_first", ".model plain nmos level=54\n.subckt s d g\n.model n1 nmos level=54\n.param plain=3u\nM1 d g 0 n1 plain w=1u\n.ends\nX1 d g s\n")
deck("w4_bin", ".model n1.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n.model plain nmos level=54\n.param plain=2u\nM1 d g 0 n1 plain w=1u\n")
deck("w4_not_mos_model", ".model n1 d\n.model plain nmos level=54\nM1 d g 0 n1 plain l=2u w=1u\n")
deck("w5_two", ".model n1 nmos level=54\n.model n2 nmos level=54\n.param n2=2u\nM1 d g 0 0 n1 n2 w=1u\n")
# a number with an exponent and a unit
deck("nu_exp_p", N + "M1 d g 0 0 n l=2.5e+06p w=1u\n")
deck("nu_exp_u", N + "M1 d g 0 0 n l=2e-6u w=1u\n")
deck("nu_exp_quoted", N + "M1 d g 0 0 n l='2e+06p' w=1u\n")
deck("nu_exp_meg", N + "M1 d g 0 0 n l=2e-12meg w=1u\n")
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
