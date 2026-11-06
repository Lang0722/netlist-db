#!/usr/bin/env python3
"""Round 39: a group with spaces after a key of an M line, a lone operator word,
`.param` forms. usage: forms39.py OUTDIR"""
import os, shutil, sys
D = {}
HEAD = ".model n nmos level=54\n.param k=4u j=5u\n"
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
LINES = """l=1u w=(2u + 3u)
l=1u w=(2u +3u)
l=1u w=(2u+ 3u)
l=1u w=( 2u+3u )
l=1u w=( 2u+3u)
l=1u w=(2u+3u )
l=1u w=( 2u + 3u )
l=1u w=(2u 3u)
l=1u w=(2u,3u)
l=1u w=(2u)
l=1u w=( 2u )
l=1u w=(k)
l=1u w=( k )
l=1u w=(k + j)
l=1u w=(k +j)
l=1u w=(k+ j)
l=1u w=(k+j)
l=1u w=( k+j )
l=1u w=(k j)
l=1u w=(k*1)
l=1u w=(k * 1)
l=1u w=(k *1)
l=1u w=(k* 1)
l=1u w=(k/1)
l=1u w=(k / 1)
l=1u w=(k - 1u)
l=1u w=(k -1u)
l=1u w=(k- 1u)
l=1u w=((k+j))
l=1u w=((k + j))
l=1u w=((k)+(j))
l=1u w=((k) + (j))
l=1u w=(k)+(j)
l=1u w=(k) + (j)
l=1u w=(2u)+(3u)
l=1u w=(2u) + (3u)
l=1u w=(2u)*2
l=1u w=2*(2u)
l=1u w=2u*(1+1)
l=1u w=2u * 2
l=1u w=k * 2
l=1u w=k + j
l=1u w=2u + 3u
l=(2u * 3) w=1u
l=(2u / 3) w=1u
l=(2u - 3n) w=1u
l=(2u + 3n) w=1u
l=(2u ** 3) w=1u
l=(2u ^ 3) w=1u
l=(2u == 3) w=1u
l=(k * 3) w=1u
l=(k / 3) w=1u
l=(k - 3n) w=1u
l=(k + 3n) w=1u
l=1u * w=2u
l=1u *w=2u
l=1u * 3 w=2u
l=1u / w=2u
l=1u /w=2u
l=1u w=2u *
l=1u w=2u * c
l=1u w=2u / c
l=1u w=2u - c
l=1u w=2u + c
l=1u w=2u ** c
l=1u w=2u ^ c
1u * 2u
1u *2u
1u / 2u
1u + 2u
1u - 2u
1u ^ 2u
1u % 2u
1u 2u * w=3u
1u 2u + w=3u
l=1u w=(2u + 3u) nf=2
l=1u w=(2u + 3u) ad=1p
l=(2u + 3u) (4u)
l=(2u + 3u) w=9u
l=(2u + 3u) l=9u
w=9u l=(2u + 3u) (4u)
l='(2u + 3u)' w=1u
l="(2u + 3u)" w=1u
l=1u w=(2u + 3u)*2
l=1u m=(2 + 3)
l=1u nf=(2 + 3)
l=1u nf=(2 +3)
l=1u nf=(2+3)""".split("\n")
for i, x in enumerate(LINES):
    deck(f"g{i:03d}", HEAD + f"M1 d g 0 0 n {x}\n")
deck("p_plus_eq", HEAD + ".param q\n+ =1u\nM1 d g 0 0 n l=q w=1u\n")
deck("p_plus_eq2", HEAD + ".param q\n+=1u\nM1 d g 0 0 n l=q w=1u\n")
deck("p_plus_eq3", HEAD + ".param q=\n+ 1u\nM1 d g 0 0 n l=q w=1u\n")
deck("p_plus_eq4", HEAD + ".param q =\n+ 1u\nM1 d g 0 0 n l=q w=1u\n")
deck("p_space_eq", HEAD + ".param q =1u\nM1 d g 0 0 n l=q w=1u\n")
deck("p_space_eq2", HEAD + ".param q = 1u\nM1 d g 0 0 n l=q w=1u\n")
deck("p_two_plus", HEAD + ".param q\n+ =1u r\n+ =2u\nM1 d g 0 0 n l=q w=r\n")
deck("m_plus_eq", HEAD + "M1 d g 0 0 n l\n+ =1u w\n+ =2u\n")
deck("x_plus_eq", HEAD + ".subckt s d g c=1u\nM1 d g 0 0 n l=c w=1u\n.ends\nX1 d g s c\n+ =2u\n")
deck("s_plus_eq", HEAD + ".subckt s d g c\n+ =1u\nM1 d g 0 0 n l=c w=1u\n.ends\nX1 d g s\n")
deck("o_plus_eq", HEAD + ".option scale\n+ =2\nM1 d g 0 0 n l=1 w=1\n")
deck("mo_plus_eq", ".model n nmos level\n+ =54\n.param k=4u\nM1 d g 0 0 n l=k w=k\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
