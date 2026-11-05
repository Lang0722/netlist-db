#!/usr/bin/env python3
"""Compare HSPICE's results for the kit's decks with netlist-db's answers.

usage: compare_hspice.py KIT_DIR RESULTS_DIR [--verbose]

KIT_DIR: what make_kit.py wrote (decks/, expected/); RESULTS_DIR: the
results/ directory that run_hspice.sh filled (hspice_results.tar.gz).

For each deck: whether HSPICE ran it (errors in the .lis), and for each
MOSFET its L (LV1), W (LV2: W/NF with NF > 1) and bin (from LV9: VTH0 is
0.2 + 0.02*N for a bin `name.N`) against netlist-db's; the bins named in the
.lis operating point tables too, when they can be read. For the probes,
each case's value against netlist-db's under each parhier.
"""
import collections
import glob
import os
import re
import sys

VTH_BASE, VTH_STEP = 0.2, 0.02


def measures(path):
    """The values of an HSPICE measure file (.mt0, .ms0): name -> number or
    None (a measure that failed)"""
    if not os.path.exists(path):
        return None
    lines = open(path, errors="replace").read().splitlines()
    start = 0
    for i, line in enumerate(lines):
        if line.strip().lower().startswith(".title"):
            start = i + 1
    words = " ".join(lines[start:]).split()
    names = []
    while words:
        w = words.pop(0)
        names.append(w.lower())
        if w.lower() == "alter#":
            break
    values = words[:len(names)]

    def number(v):
        try:
            return float(v)
        except ValueError:
            return None

    return {n: number(v) for n, v in zip(names, values)}


def errors(lis):
    """The error lines of an HSPICE listing"""
    if not os.path.exists(lis):
        return ["no .lis"]
    out = []
    for line in open(lis, errors="replace"):
        if re.search(r"\*\*error\*\*|\berror:|aborted|\bfatal\b", line, re.I):
            out.append(line.strip())
    return out


def op_models(lis):
    """The model of each MOSFET in the operating point tables of a listing,
    by element name as printed (`1:m0`, with the subckt row's path when
    there is one): name -> model. Best effort: the format varies."""
    found = {}
    if not os.path.exists(lis):
        return found
    lines = open(lis, errors="replace").read().splitlines()
    in_mos = False
    subckt, element = None, None
    for line in lines:
        s = line.strip()
        if s.startswith("****"):
            in_mos = "mosfet" in s.lower()
            continue
        if not in_mos:
            continue
        parts = s.split()
        if not parts:
            continue
        if parts[0] == "subckt":
            subckt = parts[1:]
        elif parts[0] == "element":
            element = parts[1:]
        elif parts[0] == "model" and element:
            models = parts[1:]
            for i, (e, m) in enumerate(zip(element, models)):
                local = e.split(":", 1)[-1]
                path = local
                if subckt and len(subckt) == len(element):
                    path = f"{subckt[i]}.{local}"
                found[path.lower()] = m.split(":", 1)[-1].lower()
            subckt, element = None, None
    return found


def bin_of(model):
    m = re.fullmatch(r"(.+)\.(\d+)", model.lower())
    return int(m.group(2)) if m else 0


def compare_deck(kit, results, name, verbose):
    exp = [line.split("\t") for line in open(os.path.join(kit, "expected", f"{name}.tsv")).read().splitlines()[1:]]
    lis = os.path.join(results, f"{name}.lis")
    errs = errors(lis)
    meas = measures(os.path.join(results, f"{name}.ms0"))
    want_error = [r for r in exp if r[6].startswith("error")]
    report = collections.Counter()
    diffs = []
    if want_error:
        # netlist-db finds no model for some MOSFET: HSPICE should stop
        verdict = "agree (HSPICE stops too)" if errs else "DIFFER: HSPICE ran it"
        return verdict, errs[:3], report, diffs
    if errs and not meas:
        return "HSPICE stopped", errs[:3], report, diffs
    if meas is None:
        return "no results", errs[:3], report, diffs
    ops = op_models(lis)
    for r in exp:
        i, path, l, w, nf, model = int(r[0]), r[1].lower(), float(r[3]), float(r[4]), float(r[5]), r[6]
        hl, hw, hv = meas.get(f"l{i}"), meas.get(f"w{i}"), meas.get(f"v{i}")
        report["MOSFETs"] += 1
        # L
        if hl is not None and abs(hl - l) <= 1e-6 * l:
            report["L agree"] += 1
        else:
            diffs.append(f"{path}: L {hl} (netlist-db {l:g})")
        # W: per finger, or the total
        if hw is not None and abs(hw * nf - w) <= 1e-6 * w:
            report["W agree (W/NF)"] += 1
        elif hw is not None and abs(hw - w) <= 1e-6 * w:
            report["W agree (total W)"] += 1
        else:
            diffs.append(f"{path}: W {hw} (netlist-db {w:g}, NF {nf:g})")
        # the bin, from the threshold
        if hv is None or hv == 0:
            report["bin unknown (LV9 missing or 0)"] += 1
        else:
            n = round((abs(hv) - VTH_BASE) / VTH_STEP)
            if n == bin_of(model):
                report["bin agree (LV9)"] += 1
            else:
                diffs.append(f"{path}: bin {n} from LV9={hv:g} (netlist-db {model})")
        # the bin, from the operating point table
        if path in ops:
            if ops[path] == model.lower():
                report["model agree (.op)"] += 1
            else:
                diffs.append(f"{path}: .op model {ops[path]} (netlist-db {model})")
    verdict = "agree" if not diffs else f"DIFFER ({len(diffs)})"
    return verdict, errs[:3], report, diffs


def compare_probe(kit, results):
    exp = {}
    for mode in ("global", "local"):
        for line in open(os.path.join(kit, "expected", f"probe_{mode}.cases.tsv")).read().splitlines()[1:]:
            case, value, what = line.split("\t")
            exp.setdefault(case, {})[mode] = value
            exp[case]["what"] = what
    hs = {mode: measures(os.path.join(results, f"probe_{mode}.mt0")) or {} for mode in ("global", "local")}
    print(f"{'case':7}{'netlist-db global/local':>26}{'HSPICE global/local':>26}  verdict  what")
    for case in sorted(exp, key=lambda c: int(c[4:])):
        cells, ok = [], True
        for mode in ("global", "local"):
            h = hs[mode].get(case)
            n = exp[case][mode]
            if h is None:
                cells.append("?")
                ok = False
                continue
            if "." in n and not re.fullmatch(r"[\d.e+-]+", n):
                # a bin: HSPICE's drain current tells .1 (above 1 uA) or .2
                got = ".1" if abs(h) > 1e-6 else ".2"
                cells.append(got)
                ok &= n.endswith(got)
            else:
                cells.append(f"{h:g}")
                ok &= abs(h - float(n)) <= 1e-6 * abs(float(n))
        nd = f"{exp[case]['global']} / {exp[case]['local']}"
        print(f"{case:7}{nd:>26}{' / '.join(cells):>26}  {'agree ' if ok else 'DIFFER'}   {exp[case]['what']}")


def main():
    kit, results = sys.argv[1], sys.argv[2]
    verbose = "--verbose" in sys.argv
    print("== the probes (parhier_probe.sp)")
    compare_probe(kit, results)
    print()
    print("== the decks")
    totals = collections.Counter()
    for path in sorted(glob.glob(os.path.join(kit, "expected", "*.tsv"))):
        name = os.path.basename(path)[:-4]
        if name.startswith("probe_"):
            continue
        verdict, errs, report, diffs = compare_deck(kit, results, name, verbose)
        totals.update(report)
        totals["decks " + verdict.split(" ")[0]] += 1
        detail = ", ".join(f"{k} {v}" for k, v in sorted(report.items()))
        print(f"{name:24} {verdict:28} {detail}")
        for e in errs:
            print(f"    HSPICE: {e[:160]}")
        for d in diffs[: (50 if verbose else 5)]:
            print(f"    {d}")
    print()
    for k in sorted(totals):
        print(f"{k:40} {totals[k]}")


if __name__ == "__main__":
    main()
