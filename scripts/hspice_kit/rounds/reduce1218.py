#!/usr/bin/env python3
"""Variations of deck s1218, as text changes. usage: reduce1218.py OUTDIR"""
import os, shutil, sys
S = os.environ['NDB_ROUNDS']
top = open(S + '/r2/decks/s1218.sp').read()
inc = open(S + '/r2/decks/s1218_cells.inc').read()
lib = open(S + '/r2/decks/s1218_pdk.l').read()
ELSEIF = """.elseif (ww>4.4412e-06)
M0 d g s
+ b n1
+ l='le*1.5' w=ww
+ nf=nn
"""
DEV = inc[:inc.index(".SUBCKT pch_mac1")]
MAC2 = inc[inc.index(".subckt pch_mac2"):inc.index(".subckt\n+ s1218_blk0")]
BLK0M = """M0 0 a
* a comment
+ c b plain L='1.275672e-07' W='max(0.000919366m,4.596828e-07)' $ note l=9u w=9u
"""
BLK1X0 = """X0 a n2 0 c pch_mac1 ll='48.0674n*scale_l'
+ ww=0.0147461m nn=1
"""
V = {
    "base": (None, None),
    "no_elseif": (None, (ELSEIF, "")),
    "unquoted_if": (None, (".IF ('ll<4.4412e-06')", ".IF (ll<4.4412e-06)")),
    "no_blk1x0": (None, (BLK1X0, "")),
    "no_xs00": (None, ("Xs00 n1 a c s1218_blk0 $ note l=9u w=9u\n", "")),
    "no_xs02": (None, ("Xs02 b c a s1218_blk0\n", "")),
    "no_top_x0": (("X0 d g s s1218_blk0 ka=1.862 $ note l=9u w=9u\n", ""), None),
    "no_scale": ((".option scale=2\n", ""), None),
    "no_dev": (None, (DEV, "")),
    "no_mac2": (None, (MAC2, "")),
    "no_blk0m": (None, (BLK0M, "")),
    "no_kl": (None, (".param kl='ka*1.376'\n", "")),
    "no_mt0": (("Mt0 d g s 0 plain '0.00132302m' 0.427715U\n", ""), None),
    "no_nn": (None, ("nf=nn", "")),
    "no_top_params": ((".para k=1.0815\n", ".para k=1\n"), None),
    "no_scale_l": ((".parameters scale_l=1.3737\n", ""), None),
    "x1_name": (None, ("X1 n1 0\n+ b a pch_mac1", "X1 q9 0\n+ b a pch_mac1")),
    "le_inline": (None, ("+ l=le w=ww nf=nn\n", "+ l='ll+dl' w=ww nf=nn\n")),
}
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(out + "/decks")
for name, (tc, ic) in V.items():
    t, i = top, inc
    if tc:
        assert tc[0] in t, tc
        t = t.replace(*tc)
    if ic:
        assert ic[0] in i, ic
        i = i.replace(*ic)
    stem = "v_" + name
    t = t.replace("s1218_cells.inc", stem + "_c.inc").replace("s1218_pdk.l", stem + "_p.l")
    open(f"{out}/decks/{stem}.sp", "w").write(t)
    open(f"{out}/decks/{stem}_c.inc", "w").write(i)
    open(f"{out}/decks/{stem}_p.l", "w").write(lib.replace("s1218_pdk.l", stem + "_p.l"))
print(len(V))
