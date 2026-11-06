#!/usr/bin/env python3
"""Text variations of the reduced deck s2148. usage: reduce2148.py OUTDIR"""
import os, re, shutil, sys
S = os.environ['NDB_ROUNDS']
base = open(S + '/dd3/reduced.sp').read()
D = {'base': base}
def v(name, a, b):
    assert a in base, a
    D[name] = base.replace(a, b)
v('no_mac0', ".subckt dev_mac0 d g s b ll='abs(768969995.6311N)*sqrt(4)/2' ww=21516941.986745U nn=1\n.ends dev_mac0\n", "")
v('no_local', ".su local_dev d g s b ll=\"max(7433351737.0100n,3716675.868505U)\" ww='2521736334524.65p'\n.ENDS\n", "")
v('mac0_plain', "ll='abs(768969995.6311N)*sqrt(4)/2'", "ll=1")
v('local_plain', "ll=\"max(7433351737.0100n,3716675.868505U)\"", "ll=1")
v('local_single', "ll=\"max(7433351737.0100n,3716675.868505U)\"", "ll='max(7433351737.0100n,3716675.868505U)'")
v('no_m2', "M2 b q2 a q1 nch l='75631544.1606n*kl*0.871'\n+ w=421567510332.45p\n", "")
v('no_scale', ".OPTION scale=1e-06\n", "")
v('nodes', "M1 a\n+ c b q1 nch", "M1 a c b 0 nch")
v('one_line', "M1 a\n+ c b q1 nch", "M1 a c b q1 nch")
v('group_short', "(46365950.5132n*1)", "(1u*1)")
v('w_short', "'2.900919e+00+7613.602745291m'", "'2+1'")
v('w_plain', "'2.900919e+00+7613.602745291m'", "2")
v('w_key', "'2.900919e+00+7613.602745291m'", "w=2")
v('w_none', " '2.900919e+00+7613.602745291m'", "")
v('no_k', "+ k=1.652\n", "")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
