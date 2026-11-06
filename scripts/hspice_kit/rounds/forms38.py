#!/usr/bin/env python3
"""Round 38: `.if` against `.param` for a few expressions. usage: forms38.py OUTDIR"""
import os, shutil, sys
D = {}
HEAD = ".model n nmos level=54\n.param a=2 b=3 c=-1.5 d=0 e1=1e-3 u1=1u\n"
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
IF = ".if ({})\nM1 d g 0 0 n l=1u w=1u\n.else\nM1 d g 0 0 n l=2u w=1u\n.endif\n"
TRUTH = ["!c", "!(c)", "!(-2)", "!(2)", "!a", "!d", "!(d)", "!(log(8))", "!(log(-8))", "log(-8)", "log(8)", "!(sign(-8,-1))", "sign(-8,-1)", "sign(8,-1)", "sign(-8,1)", "!(sign(-8,1))", "!(1^a-3*3)", "1^a-3*3", "!(-8)", "!-8", "-8", "c", "-c", "!(-c)", "!(a-3)", "!(a-1)", "a-3", "a-2", "!(a-2)", "!(c+1.5)", "!(c*2)", "!(0-1)", "!(0.5-1)", "!(0.5)", "!(-0.5)", "!(1e-30)", "!(-1e-30)", "1e-30", "-1e-30", "!(sgn(-2))", "sgn(-2)", "!(abs(-2))", "!(int(-2.5))", "!(exp(-1))", "!(sin(-1))", "sin(-1)", "!!a", "!(!a)", "!(!c)", "!(a<b)", "!(a>b)", "!(c<a)", "!(c>a)"]
for i, c in enumerate(TRUTH):
    deck(f"t{i:02d}", HEAD + IF.format(c))
ASSOC = [
    ("2**-3^2", "0.01"), ("2**-3^2", "0.1"), ("2^-3^2", "0.01"), ("2^-3^2", "0.1"),
    (".5**-e1^b", "1.001"), (".5**-e1^b", "1.0000001"), ("2**3^2", "65"), ("2**3^2", "63"), ("2**3**2", "65"), ("2**3**2", "63"), ("2^3^2", "65"), ("2^3^2", "63"),
    ("-2**2", "0"), ("-2^2", "0"), ("(-2)**2", "0"), ("a**-b", "0.2"), ("a**-b", "0.1"), ("a**-b^2", "0.1"), ("a**-b^2", "0.01"), ("a**-b**2", "0.1"), ("a**-b**2", "0.01"),
    ("2**(-3)^2", "0.1"), ("2**(-3)^2", "0.01"), ("2*3**2", "19"), ("2*3**2", "17"),
]
for i, (x, t) in enumerate(ASSOC):
    deck(f"a{i:02d}_if", HEAD + IF.format(f"{x}<{t}"))
    deck(f"a{i:02d}_pa", HEAD + f".param z='{x}<{t}'\n.if (z)\nM1 d g 0 0 n l=1u w=1u\n.else\nM1 d g 0 0 n l=2u w=1u\n.endif\n")
# the open forms
deck("o_param_plus_eq", HEAD + ".param q\n+ =1u\nM1 d g 0 0 n l=q w=1u\n")
deck("o_glo", HEAD + ".glo d\nM1 d g 0 0 n l=1u w=1u\n")
deck("o_glob", HEAD + ".glob d\nM1 d g 0 0 n l=1u w=1u\n")
deck("o_w_paren_spaces", HEAD + "M1 d g 0 0 n l=1u w=(2u + 3u)\n")
deck("o_w_paren_nospace", HEAD + "M1 d g 0 0 n l=1u w=(2u+3u)\n")
deck("o_l_paren_spaces", HEAD + "M1 d g 0 0 n l=(2u * 3) w=1u\n")
deck("o_w_paren_max", HEAD + "M1 d g 0 0 n l=1u w=(max(2u,3u))\n")
deck("o_x_paren_spaces", HEAD + ".subckt s d g k=1\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s k=(2u + 3u)\n")
deck("o_param_paren_spaces", HEAD + ".param q=(2u + 3u)\nM1 d g 0 0 n l=q w=1u\n")
deck("o_param_bare_spaces", HEAD + ".param q=2u + 3u\nM1 d g 0 0 n l=q w=1u\n")
deck("o_param_bare_nospace", HEAD + ".param q=2u+3u\nM1 d g 0 0 n l=q w=1u\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
