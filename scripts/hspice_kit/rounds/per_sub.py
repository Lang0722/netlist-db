#!/usr/bin/env python3
"""Per-subckt counts of the big decks. usage: per_sub.py DECKDIR BIN"""
import glob, os, re, subprocess, sys
d, b = sys.argv[1], sys.argv[2]
tot = bad = 0
for f in sorted(glob.glob(d + "/expected_*.tsv")):
    n = os.path.basename(f)[9:-4]
    out = subprocess.run([b, "--summary", "--subckt", n, "chip.sp"], cwd=d, capture_output=True, text=True, timeout=120).stdout
    rows = [re.sub("^" + re.escape(n) + ":", "", x) for x in out.splitlines()]
    u = d + "/u_" + n + ".txt"
    open(u, "w").write("\n".join(rows) + "\n")
    r = subprocess.run([sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "check_model_usage.py"), f, u], capture_output=True, text=True)
    last = r.stdout.strip().splitlines()[-1]
    tot += 1
    if r.returncode:
        bad += 1
        print(n, r.stdout.strip().splitlines()[:6])
    print(n, last[:60])
    os.remove(u)
print(d.split("/")[-1], "subckts", tot, "with a difference", bad)
