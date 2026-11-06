#!/usr/bin/env python3
"""Round 47: the forms and limits of the parser repairs: a `\\` line after a `$` in a
name, the end of `.prot freelib`, bytes that are not UTF-8 in names, long expressions,
functions that fan out, chains and diamonds of includes. usage: forms47.py OUTDIR"""
import os, shutil, sys
D = {}
FILES = {}
TAIL = b"Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = b".model n nmos level=54\n"
def deck(name, body):
    assert name not in D, name
    body = body.encode() if isinstance(body, str) else body
    D[name] = f"* {name}\n".encode() + N + body + TAIL
# a `\` line after a `$`
deck("ds_name", "M$1 d g 0 0 n l=1u \\\nw=2u\n")
deck("ds_value", ".param a$b=3u\nM1 d g 0 0 n l=a$b \\\nw=2u\n")
deck("ds_word", "M1 d g 0 0 n l=1u w=2u $ note \\\nM2 d g 0 0 n l=3u w=3u\n")
deck("ds_num", "M1 d g 0 0 n l=1u w=2u$ note \\\nM2 d g 0 0 n l=3u w=3u\n")
# `.prot freelib`
deck("fl_unprot", "M0 d g 0 0 n l=1u w=1u\n.prot freelib\nabc\n.unprot\nM1 d g 0 0 n l=2u w=2u\n")
deck("fl_unpr", "M0 d g 0 0 n l=1u w=1u\n.prot freelib\nabc\n.unpr\nM1 d g 0 0 n l=2u w=2u\n")
deck("fl_noend", "M0 d g 0 0 n l=1u w=1u\n.prot freelib\nabc\nM1 d g 0 0 n l=2u w=2u\n")
deck("pl_unpr", "M0 d g 0 0 n l=1u w=1u\n.prot\nM2 d g 0 0 n l=3u w=3u\n.unpr\nM1 d g 0 0 n l=2u w=2u\n")
# bytes that are not UTF-8
deck("b_subckt", b".subckt \xe9 d g\nM1 d g 0 0 n l=1u w=1u\n.ends\n.subckt \xe8 d g\nM1 d g 0 0 n l=2u w=2u\n.ends\nX1 d g \xe9\nX2 d g \xe8\n")
deck("b_param", b".param p\xe9=1u p\xe8=2u\nM1 d g 0 0 n l=p\xe9 w=p\xe8\n")
deck("b_model", b".model m\xe9 nmos level=54 vth0=0.1\n.model m\xe8 nmos level=54 vth0=0.2\nM1 d g 0 0 m\xe9 l=1u w=1u\nM2 d g 0 0 m\xe8 l=2u w=2u\n")
# long expressions
for n in (1000, 30000):
    deck(f"e_sum_{n}", ".param a='1" + "+1" * n + f"'\nM1 d g 0 0 n l='a*1n' w=1u\n")
    deck(f"e_pow_{n}", ".param a='2" + "**1" * n + "'\nM1 d g 0 0 n l='a*1u' w=1u\n")
# functions that fan out
def fan(n):
    rows = [f".param f{i}(x)='f{i+1}(x)+f{i+1}(x)'" for i in range(n)]
    return "\n".join(rows) + f"\n.param f{n}(x)='x'\nM1 d g 0 0 n l='f0(1n)' w=1u\n"
for n in (10, 19, 22, 26):
    deck(f"fn_fan_{n}", fan(n))
# chains and diamonds of includes
for n in (100, 255, 256, 300, 1000):
    for i in range(n):
        FILES[f"c{n}_{i}.inc"] = f".inc 'c{n}_{i+1}.inc'\n"
    FILES[f"c{n}_{n}.inc"] = "M1 d g 0 0 n l=1u w=1u\n"
    deck(f"i_chain_{n}", f".inc 'c{n}_0.inc'\n")
for n in (10, 16, 20):
    for i in range(n):
        FILES[f"d{n}_{i}.inc"] = f".inc 'd{n}_{i+1}.inc'\n.inc 'd{n}_{i+1}.inc'\n"
    FILES[f"d{n}_{n}.inc"] = "* leaf\n"
    deck(f"i_dia_{n}", f".inc 'd{n}_0.inc'\nM1 d g 0 0 n l=1u w=1u\n")
out = sys.argv[1] + "/decks"
shutil.rmtree(out, ignore_errors=True)
os.makedirs(out)
for k, v in D.items():
    open(f"{out}/{k}.sp", "wb").write(v)
for k, v in FILES.items():
    open(f"{out}/{k}", "w").write(v)
print(len(D), "decks", len(FILES), "files")
