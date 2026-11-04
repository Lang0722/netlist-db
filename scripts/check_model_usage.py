#!/usr/bin/env python3
"""Compare the model counts of `model_select --summary` with those that
gen_big_netlist.py computed for the deck it wrote.

usage: check_model_usage.py EXPECTED.tsv SUMMARY.txt

EXPECTED.tsv: `total N`, then `model N` or `error family L=..n W=..n N` lines.
SUMMARY.txt: the output of `model_select --summary` (`model N share`,
`error: ... N share e.g. path`, `total N`). Exits with 1 if they differ.
"""
import re
import sys


def main():
    expected, got = {}, {}
    total_expected = total_got = None
    for line in open(sys.argv[1]):
        f = line.rstrip("\n").split("\t")
        if f[0] == "total":
            total_expected = int(f[1])
        elif f[0] == "error":
            # error, family, L=..n, W=..n, count
            expected[("error", f[1], int(f[2][2:-1]), int(f[3][2:-1]))] = int(f[4])
        else:
            expected[f[0]] = int(f[1])
    for line in open(sys.argv[2]):
        f = line.rstrip("\n").split("\t")
        if f[0] == "model":
            continue
        if f[0] == "total":
            total_got = int(f[1])
        elif f[0].startswith("error: "):
            m = re.match(r"error: none of the \d+ bins `(.+)\.N` holds L=(\S+), W=(\S+)$", f[0])
            key = (
                ("error", m.group(1), round(float(m.group(2)) * 1e9), round(float(m.group(3)) * 1e9))
                if m
                else ("other", f[0])
            )
            got[key] = got.get(key, 0) + int(f[1])
        else:
            got[f[0]] = got.get(f[0], 0) + int(f[1])
    differ = 0
    for key in sorted(set(expected) | set(got), key=str):
        if expected.get(key) != got.get(key):
            differ += 1
            print(f"differs: {key}: expected {expected.get(key)}, got {got.get(key)}")
    print(f"MOSFETs: expected {total_expected}, got {total_got}")
    ok = differ == 0 and total_expected == total_got
    print(f"{len(expected)} models (and errors) expected, {differ} differ" + (": all agree" if ok else ""))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
