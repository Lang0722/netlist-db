#!/usr/bin/env python3
"""Round 16: the matrix of the 4th word and the 5th word naming models, and all
orders of 3 and 4 instances for a condition in quotes. usage: forms16.py OUTDIR"""
import itertools, os, shutil, sys
D = {}
FILES = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
N = ".model n nmos level=54\n"
def deck(name, body, files=None):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
    FILES.update(files or {})
BIN = lambda b: f".model {b}.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n"
PLAIN = lambda b: f".model {b} nmos level=54\n"
for where in ("top", "sub"):
    for four in ("bin", "plain"):
        for five in ("bin", "plain"):
            for how in ("direct", "lib", "after"):
                cards = (BIN("n1") if four == "bin" else PLAIN("n1")) + (BIN("p1") if five == "bin" else PLAIN("p1"))
                line = "M1 d g 0 n1 p1 l=2u w=1u\n"
                body = f".subckt s d g\n{line}.ends\nX1 d g s\n" if where == "sub" else line
                name = f"m_{where}_{four}_{five}_{how}"
                if how == "direct":
                    deck(name, cards + body)
                elif how == "after":
                    deck(name, body + cards)
                else:
                    lib = name + ".l"
                    deck(name, f".lib '{lib}' a\n" + body, {lib: f".lib a\n{cards}.endl a\n"})
def mac(c1="'ll<4u'", c2="ww>4u"):
    return f".subckt mac d g s b ll=1u ww=1u\n.if ({c1})\nM0 d g s b n l=ll w=ww\n.elseif ({c2})\nM0 d g s b n l='ll*1.5' w=ww\n.else\nM0 d g s b n l='ll*2' w='ww/2'\n.endif\n.ends\n"
INST = {"I": "ll=3u ww=1u", "E": "ll=5u ww=1u", "F": "ll=5u ww=5u", "i": "ll=3u ww=5u"}
def calls(order):
    return "".join(f"X{k} d g s 0 mac {INST[c]}\n" for k, c in enumerate(order))
for n in (3, 4):
    for order in itertools.product("IEFi", repeat=n):
        order = "".join(order)
        deck("oq_" + order.replace("i", "j"), N + mac() + calls(order))
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
