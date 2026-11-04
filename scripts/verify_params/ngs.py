#!/usr/bin/env python3
"""ngs.py DECK.sp: run ngspice on DECK (its parameter expansion and model
binning), print each MOSFET as path, L, W, NF, model (TSV)."""
import re, subprocess, sys, tempfile, os
deck = open(sys.argv[1]).read()
ctl = ".control\nop\nlisting e\nshow all : model l w nf\n.endc\n"
deck = re.sub(r"(?im)^\.end\s*$", ctl + ".end", deck)
with tempfile.NamedTemporaryFile("w", suffix=".sp", delete=False) as f:
    f.write(deck)
    name = f.name
try:
    r = subprocess.run(["ngspice", "-b", name], capture_output=True, text=True, timeout=600)
finally:
    os.unlink(name)
out = r.stdout + r.stderr
devices, rows = [], {}
result = {}
def flush():
    for i, d in enumerate(devices):
        if not d.startswith("m"):
            continue
        result[d] = {k: rows.get(k, [None] * len(devices))[i] for k in ("model", "l", "w", "nf")}
# `show` cuts device names at 21 characters: the full names, from the
# expanded listing
full = set()
for line in out.splitlines():
    m = re.match(r"^\s*\d+ : (m\S*)", line)
    if m:
        full.add(m.group(1).lower())
for line in out.splitlines():
    parts = line.split()
    if not parts:
        continue
    if parts[0] == "device":
        flush()
        devices, rows = parts[1:], {}
    elif devices and parts[0] in ("model", "l", "w", "nf") and parts[0] not in rows:
        rows[parts[0]] = parts[1:]
flush()
if not result:
    sys.stderr.write(out[-3000:])
    sys.exit(1)
named = {}
ambiguous = []
for d, v in result.items():
    if d not in full:
        candidates = [n for n in full if n.startswith(d)]
        if len(candidates) != 1:
            # a cut name that several MOSFETs share: those are not compared
            ambiguous.append(".".join(t for t in d.split(".")[:-1] if t != "m"))
            continue
        d = candidates[0]
    named[d] = v
print("path\tL\tW\tNF\tmodel")
for prefix in ambiguous:
    print(f"{prefix}.*\t\t\t\tnot compared")
for d, v in sorted(named.items()):
    path = ".".join(t for t in d.split(".") if t != "m")
    model = v["model"].split(":")[-1]
    print(f"{path}\t{v['l']}\t{v['w']}\t{v['nf']}\t{model}")
