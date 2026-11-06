#!/usr/bin/env python3
"""Text variations of the reduced deck s3214. usage: reduce3214.py OUTDIR"""
import os, re, shutil, sys
T = ".option list nomod post=0\nVd d 0 1\nVg g 0 1\nVs s 0 0\n"
O = ".OPTION scalm=1e-06 defl=1u defw=2u\n"
C = ".MODEL fin.3 nmos level=72 lmin=1.041587e-03 lmax='2.000000e+06/2' nfinmin=0.5 nfinmax=23\n"
M1 = "Mt1 d g s 0 fin L=715538.60p w=3.64905e-07 nfin=10 $ note l=9u w=9u\n"
M2 = "Mt2 d g s 0 fin\n+ l='2678546.13p+2161.7210N'\n$ a comment\n+ W='3218.3942n+0.000848290m' nfin=4\n"
D = {
 'a': O + C + M1 + M2,
 'b': ".OPTION scalm=1e-06\n" + C + M1 + M2,
 'c': O + C.replace("'2.000000e+06/2'", "1e6") + M1 + M2,
 'd': O + C + M2,
 'e': O + C + M1,
 'f': O + C + M1 + "Mt2 d g s 0 fin l=4u w=4u nfin=4\n",
 'g': O + C + M1 + "Mt2 d g s 0 fin l=4u nfin=4\n",
 'h': O + C + "Mt1 d g s 0 fin l=4u nfin=4\n",
 'i': O + C + "Mt1 d g s 0 fin l=4u\n",
 'j': O + C + "Mt1 d g s 0 fin l=4u w=4u nfin=4\n",
 'k': ".option defl=1u\n" + ".option scalm=1e-6\n" + C + "Mt1 d g s 0 fin l=4u nfin=4\n",
 'l': ".option defw=2u\n" + ".option scalm=1e-6\n" + C + "Mt1 d g s 0 fin l=4u nfin=4\n",
 'm': ".option defw=2u\n" + C.replace("lmin=1.041587e-03", "lmin=1e-9").replace("'2.000000e+06/2'", "1") + "Mt1 d g s 0 fin l=4u nfin=4\n",
 'n': ".option defw=2u scalm=2\n" + C.replace("lmin=1.041587e-03", "lmin=1e-9").replace("'2.000000e+06/2'", "1") + "Mt1 d g s 0 fin l=4u nfin=4\n",
 'o': ".option defl=1u scalm=2\n" + C.replace("lmin=1.041587e-03", "lmin=1e-9").replace("'2.000000e+06/2'", "1") + "Mt1 d g s 0 fin l=4u nfin=4\n",
 'p': ".option defw=3 scalm=2\n" + C.replace("lmin=1.041587e-03", "lmin=1e-9").replace("'2.000000e+06/2'", "1") + "Mt1 d g s 0 fin l=4u nfin=4\n",
 'q': ".option defw=3\n" + C.replace("lmin=1.041587e-03", "lmin=1e-9").replace("'2.000000e+06/2'", "1") + "Mt1 d g s 0 fin l=4u nfin=4\n",
 'r': ".option defw=1m\n" + C.replace("lmin=1.041587e-03", "lmin=1e-9").replace("'2.000000e+06/2'", "1") + "Mt1 d g s 0 fin l=4u nfin=4\n",
 's': ".option defw=100\n" + C.replace("lmin=1.041587e-03", "lmin=1e-9").replace("'2.000000e+06/2'", "1") + "Mt1 d g s 0 fin l=4u nfin=4\n",
}
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write("* " + name + "\n" + text + T + ".end\n")
print(len(D), "decks")
