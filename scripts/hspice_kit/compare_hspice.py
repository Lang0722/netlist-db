#!/usr/bin/env python3
"""Compare HSPICE's results for the kit's decks with netlist-db's answers.

usage: compare_hspice.py KIT_DIR RESULTS_DIR [--verbose]

KIT_DIR: what make_kit.py wrote (decks/, expected/); RESULTS_DIR: the
results/ directory that run_hspice.sh filled (hspice_results.tar.gz).

For each deck: whether HSPICE ran it (errors in the .lis), and for each
MOSFET its L (LV1), W (LV2: W/NF with NF > 1) and bin (from LV9: VTH0 is
0.2 + 0.02*N for a bin `name.N`) and the model its operating point table
names against netlist-db's. For the probes, each case's value against
netlist-db's under each parhier. Exits with 1 if anything differs.
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


def directory(lines):
    """The circuit name directory of a listing: circuit number -> its path
    with the final dot ('' for the main circuit). (The `subckt` rows of the
    operating point tables cut long paths and skip the main circuit.)"""
    paths = {0: ""}
    for i, line in enumerate(lines):
        if "circuit name directory" not in line:
            continue
        for row in lines[i + 1:]:
            m = re.match(r"\s+(\d+)\s+(\S+\.)(?:\s|$)", row)
            if m:
                paths[int(m.group(1))] = m.group(2).lower()
            elif not re.match(r"\s*(circuit number|number\s|0 main|\S+\s+\d+\.\d+\s*$)", row):
                break
        break
    return paths


def op_models(lis):
    """The model of each MOSFET in the operating point tables of a listing:
    path -> model. An `element` row names MOSFETs by circuit number (`5:m0`),
    the `model` row below it gives their models (`0:nch.4`)."""
    found = {}
    if not os.path.exists(lis):
        return found
    lines = open(lis, errors="replace").read().splitlines()
    paths = directory(lines)
    in_mos = False
    element = None
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
        if parts[0] == "element":
            element = parts[1:]
        elif parts[0] == "model" and element:
            for e, m in zip(element, parts[1:]):
                number, local = e.split(":", 1)
                path = paths.get(int(number), f"?{number}.") + local
                found[path.lower()] = m.split(":", 1)[-1].lower()
            element = None
    return found


# HSPICE stops as netlist-db's error says: no model or bin for the MOSFET, or
# a value it cannot compute. Any other stop does not answer the question.
NO_ANSWER = re.compile(r"definition of model/subckt .* is not found|does not fit into the wmax/wmin"
                       r"|must set all four|cannot redefine parameter|undefined parameter"
                       r"|loop reference|must not be negative", re.I)

# Where netlist-db differs from HSPICE on purpose. Reported, but they do not
# fail this comparison.
# - The first MOSFET that looks at a bin reads the bin's limits in its own
#   instance (see the known gap in src/models.rs).
# - `M1 d g 0 a pl pw`: 3 nodes, then names of parameters after the model name.
#   HSPICE knows the models when it reads the line; netlist-db's parser takes
#   `a` for a 4th node and `pl` for the model, and then reports no model `pl`.
KNOWN_GAPS = {"q_limits_s1_s2", "q_limits_s2_s1", "q_limits_s1_s3", "q_limits_bfs", "q_limits_nested_local",
              "q_limits_same_scope", ("case7", "local"), "q_positional_names_3"}


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
        # netlist-db finds no model for some MOSFET: HSPICE should stop, on
        # that (a stop on anything else does not tell)
        if any(NO_ANSWER.search(e) for e in errs):
            verdict = "agree (HSPICE stops too)"
        elif errs:
            verdict = "HSPICE stopped (on something else)"
        else:
            verdict = "DIFFER: HSPICE ran it"
        return verdict, errs[:3], report, diffs
    if errs and not meas:
        return "DIFFER: HSPICE stopped", errs[:3], report, diffs
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
        # the bin, from the threshold (which is no more VTH0 alone at an L of
        # a few nm: the `.op` table below still tells)
        if hv is None or hv == 0:
            report["bin unknown (LV9 missing or 0)"] += 1
        elif l < 1e-8:
            report["bin unknown (L below 10 nm)"] += 1
        else:
            n = round((abs(hv) - VTH_BASE) / VTH_STEP)
            if n == bin_of(model):
                report["bin agree (LV9)"] += 1
            else:
                diffs.append(f"{path}: bin {n} from LV9={hv:g} (netlist-db {model})")
        # the bin, from the operating point table
        if path not in ops:
            report["no .op row"] += 1
        elif ops[path] == model.lower():
            report["model agree (.op)"] += 1
        else:
            diffs.append(f"{path}: .op model {ops[path]} (netlist-db {model})")
    verdict = "agree" if not diffs else f"DIFFER ({len(diffs)})"
    return verdict, errs[:3], report, diffs


def n_agrees(n, h):
    """netlist-db's answer `n` to a probe case against HSPICE's value `h`"""
    if h is None:
        return False
    if "." in n and not re.fullmatch(r"[\d.e+-]+", n):
        # a bin: HSPICE's drain current tells .1 (above 1 uA) or .2
        return n.endswith(".1" if abs(h) > 1e-6 else ".2")
    return abs(h - float(n)) <= 1e-6 * abs(float(n))


def compare_probe(kit, results):
    """Prints the probe's table; returns how many cases differ"""
    differ = 0
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
            is_bin = "." in n and not re.fullmatch(r"[\d.e+-]+", n)
            cells.append((".1" if abs(h) > 1e-6 else ".2") if is_bin else f"{h:g}")
            ok &= n_agrees(n, h)
        # (the known gap is under parhier=local: netlist-db must keep the
        # global answer)
        known = not ok and (case, "local") in KNOWN_GAPS and n_agrees(exp[case]["global"], hs["global"].get(case))
        differ += not ok and not known
        nd = f"{exp[case]['global']} / {exp[case]['local']}"
        verdict = "known " if known else "agree " if ok else "DIFFER"
        print(f"{case:7}{nd:>26}{' / '.join(cells):>26}  {verdict}   {exp[case]['what']}")
    return differ


def main():
    kit, results = sys.argv[1], sys.argv[2]
    verbose = "--verbose" in sys.argv
    print("== the probes (parhier_probe.sp)")
    differ = compare_probe(kit, results)
    print()
    print("== the decks")
    totals = collections.Counter()
    for path in sorted(glob.glob(os.path.join(kit, "expected", "*.tsv"))):
        name = os.path.basename(path)[:-4]
        if name.startswith("probe_"):
            continue
        verdict, errs, report, diffs = compare_deck(kit, results, name, verbose)
        if name in KNOWN_GAPS and verdict.startswith("DIFFER"):
            verdict = "known gap: " + verdict
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
    sys.exit(1 if differ or totals["decks DIFFER"] or totals["decks DIFFER:"] else 0)


if __name__ == "__main__":
    main()
