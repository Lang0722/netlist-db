#!/usr/bin/env python3
"""ngs.py DECK.sp: run ngspice on DECK (its parameter expansion and model
binning), print each MOSFET as path, L, W, NF, model (TSV).

Two runs: the first lists the MOSFETs (`listing e`, which gives their full
names), the second asks each one's model, L, W and NF by name (`show`
prints names cut at 21 characters, so a table of all of them is not
enough). The path is ngspice's name without its `m.` parts
(`m.x1.m.x2.m1` is `x1.x2.m1`).
"""
import os
import re
import subprocess
import sys
import tempfile


def ngspice(deck, control):
    text = re.sub(r"(?im)^\.end\s*$", f".control\n{control}.endc\n.end", deck)
    with tempfile.NamedTemporaryFile("w", suffix=".sp", delete=False) as f:
        f.write(text)
        name = f.name
    try:
        r = subprocess.run(["ngspice", "-b", name], capture_output=True, text=True, timeout=3600)
    finally:
        os.unlink(name)
    return r.stdout + r.stderr


def main():
    deck = open(sys.argv[1]).read()
    out = ngspice(deck, "listing e\n")
    names = []
    for line in out.splitlines():
        m = re.match(r"^\s*\d+ : (m\S*)", line)
        if m and m.group(1).lower() not in names:
            names.append(m.group(1).lower())
    if not names:
        sys.stderr.write(out[-3000:])
        sys.exit(1)
    control = "op\n" + "".join(f"show {n} : model l w nf\n" for n in names)
    out = ngspice(deck, control)
    # one block per `show`, in order
    blocks, cur = [], None
    for line in out.splitlines():
        parts = line.split()
        if not parts:
            continue
        if parts[0] == "device":
            cur = {}
            blocks.append(cur)
        elif cur is not None and parts[0] in ("model", "l", "w", "nf") and parts[0] not in cur:
            cur[parts[0]] = parts[1] if len(parts) > 1 else ""
    if len(blocks) != len(names):
        sys.stderr.write(f"{len(names)} MOSFETs, {len(blocks)} answers\n" + out[-3000:])
        sys.exit(1)
    print("path\tL\tW\tNF\tmodel")
    for n, v in sorted(zip(names, blocks)):
        path = ".".join(t for t in n.split(".") if t != "m")
        model = v.get("model", "").split(":")[-1]
        print(f"{path}\t{v.get('l', '')}\t{v.get('w', '')}\t{v.get('nf', '')}\t{model}")


if __name__ == "__main__":
    main()
