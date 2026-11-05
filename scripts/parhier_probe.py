#!/usr/bin/env python3
"""What netlist-db resolves for each case of parhier_probe.sp, under
parhier=global and parhier=local; with HSPICE's .mt0 files, also what HSPICE
measured, and whether they agree.

usage: parhier_probe.py [--hspice GLOBAL.mt0 LOCAL.mt0] [--model-select PATH]

Run HSPICE on parhier_probe.sp as is (global) and with `.option
parhier=local` (local), then pass the two .mt0 files. Cases 1-6 compare the
values of parameters; cases 7-11 the bins chosen (HSPICE's drain current
tells: above 1 uA bin .1, below bin .2).
"""
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
CASES = {
    "case1": ("Xc1.X1.M1", "top .param p=1, default p=3, passed p=2"),
    "case2": ("Xc2.X1.M1", "top .param p=1, default p=3, nothing passed"),
    "case3": ("Xc3.X1.X1.M1", "top .param p=1, p=5 passed two levels up"),
    "case4": ("Xc4.X1.X1.M1", "s='k+q' passed, computed in the new instance: global 7+8, local 100+200"),
    "case5": ("Xc5.X1.M1", "default p=3, .param p=5 inside, passed p=2"),
    "case6": ("Xc6.X1.M1", "a=6 passed, default b='a*2'"),
    "case7": ("Xc7.M1", "bin of a global model, lmax='lb' (global 2e-7, subckt 1e-6)"),
    "case8": ("M8", "W=4u NF=8, no WNFLAG: the width per finger, .1"),
    "case9": ("M9", "W=4u NF=8, WNFLAG=1 in the model changes nothing: .1"),
    "case10": ("M10", "L 0.5 nm below an edge: .1, the tolerance is far below 1 nm"),
    "case11": ("M11", "L on an edge: .2"),
}
BINS = ("case7", "case8", "case9", "case10", "case11")


def netlist_db(ms, deck):
    out = subprocess.run(ms + [deck], capture_output=True, text=True, check=True).stdout
    rows = {}
    for line in out.splitlines()[1:]:
        f = line.split("\t")
        rows[f[0]] = f
    res = {}
    for case, (path, _) in CASES.items():
        f = rows.get(path)
        if f is None:
            res[case] = "missing"
        elif case in BINS:
            res[case] = f[5]
        else:
            # W is the value in um
            res[case] = f"{float(f[3]) * 1e6:.6g}"
    return res


def mt0(path):
    """The measured values of an HSPICE .mt0 file"""
    words = open(path).read().split()
    # after the title: the names (up to alter#), then as many values
    try:
        start = max(i for i, w in enumerate(words) if w.lower().startswith(".title"))
    except ValueError:
        start = -1
    words = words[start + 1:]
    while words and not words[0].lower().startswith("case"):
        words.pop(0)
    names = []
    while words and not re.match(r"^[-+0-9.]", words[0]):
        names.append(words.pop(0).lower())
    return {n: v for n, v in zip(names, words)}


def main():
    ms = ["cargo", "run", "--quiet", "--release", "--example", "model_select", "--"]
    if "--model-select" in sys.argv:
        ms = [sys.argv[sys.argv.index("--model-select") + 1]]
    text = open(os.path.join(HERE, "parhier_probe.sp")).read()
    ndb = {}
    with tempfile.TemporaryDirectory() as tmp:
        for mode in ("global", "local"):
            deck = os.path.join(tmp, f"probe_{mode}.sp")
            with open(deck, "w") as f:
                f.write(text.replace(".option parhier=global", f".option parhier={mode}"))
            ndb[mode] = netlist_db(ms, deck)
    hsp = {}
    if "--hspice" in sys.argv:
        i = sys.argv.index("--hspice")
        hsp = {"global": mt0(sys.argv[i + 1]), "local": mt0(sys.argv[i + 2])}
    print(f"{'case':6} {'netlist-db global':>18} {'netlist-db local':>17}  "
          + ("HSPICE global / local  " if hsp else "") + "what")
    differ = 0
    for case, (_, what) in CASES.items():
        g, l = ndb["global"][case], ndb["local"][case]
        line = f"{case:6} {g:>18} {l:>17}  "
        if hsp:
            hg, hl = hsp["global"].get(case, "?"), hsp["local"].get(case, "?")
            if case in BINS:
                # the bin HSPICE chose, from the drain current
                hg, hl = (h if h == "?" else ".1" if abs(float(h)) > 1e-6 else ".2" for h in (hg, hl))
                ok = g.endswith(hg) and l.endswith(hl)
            else:
                ok = all(h != "?" and abs(float(h) - float(n)) <= 1e-6 * abs(float(n))
                         for h, n in ((hg, g), (hl, l)))
            differ += not ok
            line += f"{hg:>9} / {hl:<9} {'agree' if ok else 'DIFFER'}  "
        print(line + what)
    if hsp:
        print("all agree" if not differ else f"{differ} case(s) differ")


if __name__ == "__main__":
    main()
