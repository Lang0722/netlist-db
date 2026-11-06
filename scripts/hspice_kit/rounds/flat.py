#!/usr/bin/env python3
"""A flat deck of N MOSFET lines. usage: flat.py OUTDIR N"""
import os, sys
out, n = sys.argv[1], int(sys.argv[2])
os.makedirs(out, exist_ok=True)
with open(out + "/chip.sp", "w", buffering=1 << 24) as f:
    f.write("* flat\n.option parhier=local\n.param k=2\n")
    f.write(".model n.1 nmos level=54 lmin=1n lmax=500.5n wmin=1n wmax=1\n")
    f.write(".model n.2 nmos level=54 lmin=500.5n lmax=1 wmin=1n wmax=1\n")
    for chunk in range(0, n, 1 << 16):
        rows = []
        for i in range(chunk, min(n, chunk + (1 << 16))):
            l = 100 + i % 900
            kind = i % 4
            if kind == 0:
                rows.append(f"M{i} d{i % 1000} g{i % 777} s 0 n l={l}n w=1u nf=2\n")
            elif kind == 1:
                rows.append(f"M{i} d{i % 1000} g s 0 n {l}n 1u\n")
            elif kind == 2:
                rows.append(f"M{i} d g{i % 777} s 0 n l='{l}n' w=1u\n")
            else:
                rows.append(f"M{i} d g s 0 n l='{l}n*k/2' w='1u'\n")
        f.write("".join(rows))
    f.write(".end\n")
# L <= 500n: i % 900 <= 400
full, rest = n // 900, n % 900
first = full * 401 + min(rest, 401)
with open(out + "/expected.tsv", "w") as f:
    f.write(f"total\t{n}\nn.1\t{first}\nn.2\t{n - first}\n")
print(n, "lines", os.path.getsize(out + "/chip.sp") >> 20, "MB")
