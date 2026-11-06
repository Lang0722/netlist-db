#!/usr/bin/env python3
"""Round 37: random `.if` conditions, and some open forms. usage: forms37.py OUTDIR SEED COUNT"""
import os, random, re, shutil, subprocess, sys
S = os.environ['NDB_ROUNDS']
D = {}
HEAD = ".model n nmos level=54\n.param a=2 b=3 c=-1.5 d=0 e1=1e-3 u1=1u\n"
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
out, seed, count = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
r = random.Random(seed)
src = subprocess.run([sys.executable, S + '/hard/fuzz.py', str(seed), str(count * 12)], capture_output=True, text=True).stdout.split('\n')
conds = [x for x in src if x and re.search(r'==|!=|<|>|&&|\|\||!', x) and '?' not in x]
r.shuffle(conds)
atom = lambda: r.choice(['a', 'b', 'c', 'd', 'e1', 'u1', '1', '0', '2', '0.5'])
cmp_op = lambda: r.choice(['==', '!=', '<', '>', '<=', '>='])
simple = lambda: f"{atom()}{cmp_op()}{atom()}"
def two():
    return f"{simple()}{r.choice(['&&', '||'])}{simple()}"
def wrap(x):
    k = r.random()
    if k < 0.70:
        return f"({x})"
    if k < 0.90:
        return f" ( {x} ) "
    return f"(({x}))"
n = 0
for i in range(count):
    kind = r.random()
    if kind < 0.5 and conds:
        cond = conds.pop()
    elif kind < 0.8:
        cond = simple()
    else:
        cond = two()
    if len(cond) > 80:
        cond = simple()
    shape = r.random()
    if shape < 0.5:
        body = f".if {wrap(cond)}\nM1 d g 0 0 n l=1u w=1u\n.else\nM1 d g 0 0 n l=2u w=1u\n.endif\n"
    elif shape < 0.8:
        c2 = simple()
        body = f".if {wrap(cond)}\nM1 d g 0 0 n l=1u w=1u\n.elseif {wrap(c2)}\nM1 d g 0 0 n l=2u w=1u\n.else\nM1 d g 0 0 n l=3u w=1u\n.endif\n"
    else:
        # the same in a subckt: the condition reads passed values
        c = cond.replace('a', 'pa') if re.fullmatch(r'[a-z0-9.<>=!&|() -]*', cond) and 'abs' not in cond and 'atan' not in cond and 'max' not in cond and 'asin' not in cond and 'acos' not in cond and 'tan' not in cond and 'nint' not in cond else cond
        body = f".subckt s d g pa=1\n.if {wrap(c)}\nM1 d g 0 0 n l=1u w=1u\n.else\nM1 d g 0 0 n l=2u w=1u\n.endif\n.ends\nX1 d g s pa=2\n"
    deck(f"i{i:04d}", HEAD + body)
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
