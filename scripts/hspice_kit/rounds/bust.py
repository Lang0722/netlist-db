#!/usr/bin/env python3
"""A deck where each leaf MOSFET has its own L, so that caching cannot merge leaves.
usage: bust.py OUTDIR N1 N2 N3 [KIND]
KIND: plain, or `if` (the leaf holds a `.if` that reads the passed values).
L = 100n + k*1p with k = i*N2*N3 + j*N3 + m unique for each leaf. Writes chip.sp and
expected.tsv (closed form: no walk here)."""
import os, sys
out, n1, n2, n3 = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
kind = sys.argv[5] if len(sys.argv) > 5 else "plain"
os.makedirs(out, exist_ok=True)
total = n1 * n2 * n3
# the bins: n.1 holds k <= 900000, n.2 the rest (the limit stands between two sizes)
LMAX = 100e-9 + 900000.5e-12
first = min(total, 900001)
lines = ["* cache-defeating deck", ".option parhier=local",
         f".model n.1 nmos level=54 lmin=1n lmax={LMAX:.12e} wmin=1n wmax=1",
         f".model n.2 nmos level=54 lmin={LMAX:.12e} lmax=1 wmin=1n wmax=1",
         f".subckt c d g s b pa=0 pb=0 pc=0"]
LEXPR = f"100n+(pa*{n2 * n3}+pb*{n3}+pc)*1p"
if kind == "if":
    # (the first half of the leaves take a first line, the rest another one)
    half = total // 2
    lines += [f".if ((pa*{n2 * n3}+pb*{n3}+pc)<{half})",
              f"M1 d g s b n l='{LEXPR}' w=1u",
              ".else",
              f"M1 d g s b n l='{LEXPR}' w=2u",
              ".endif"]
else:
    lines += [f"M1 d g s b n l='{LEXPR}' w=1u"]
lines += [".ends", ".subckt b d g s bb pa=0 pb=0"]
lines += [f"X{m} d g s bb c pa=pa pb=pb pc={m}" for m in range(n3)]
lines += [".ends", ".subckt a d g s bb pa=0"]
lines += [f"X{j} d g s bb b pa=pa pb={j}" for j in range(n2)]
lines += [".ends"]
lines += [f"X{i} d g s 0 a pa={i}" for i in range(n1)]
lines += ["Vd d 0 1", "Vg g 0 1", "Vs s 0 0", ".option list nomod post=0", ".end"]
open(os.path.join(out, "chip.sp"), "w").write("\n".join(lines) + "\n")
with open(os.path.join(out, "expected.tsv"), "w") as f:
    f.write(f"total\t{total}\n")
    f.write(f"n.1\t{first}\n")
    if total > first:
        f.write(f"n.2\t{total - first}\n")
print(total, "MOSFETs", os.path.getsize(os.path.join(out, "chip.sp")), "bytes")
