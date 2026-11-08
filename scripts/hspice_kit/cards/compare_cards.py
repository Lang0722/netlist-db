#!/usr/bin/env python3
"""Compare HSPICE's results for the decks of the card kit with netlist-db's
answers (`resolve_model`).

usage: compare_cards.py KIT_DIR RESULTS_DIR [--verbose]

KIT_DIR: what make_cards_kit.py wrote (decks/, expected.json); RESULTS_DIR:
the results/ directory that run_cards.sh (or run_cards.ps1) filled.

For each probe (a card parameter of one element): HSPICE's value, read from
its measure and calibrated with the deck's control (the threshold LV9 for an
NMOS card's vth0, the current for an R card's rsh and a diode card's is),
against netlist-db's value; when they differ, the rules of the probe that
HSPICE's value fits. For a MOSFET, also the card that the `.op` table names
against netlist-db's card. Needs only Python 3. Exits with 1 if anything
differs.
"""
import json
import os
import re
import sys

# how close HSPICE's value must be to a rule's value: 5 mV for a threshold,
# 2% for a sheet resistance or a saturation current (the rules of a probe are
# at least 20 mV, or a factor of 1.5, apart)
TOLERANCE = {"lv9": ("abs", 5e-3), "r": ("rel", 0.02), "d": ("rel", 0.02)}


def measures(path):
    """The values of an HSPICE measure file (.ms0, .mt0): name -> number or
    None (a measure that failed). As in compare_hspice.py."""
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


def warnings(lis):
    """The warning lines of an HSPICE listing"""
    if not os.path.exists(lis):
        return []
    return [line.strip() for line in open(lis, errors="replace") if "**warning**" in line.lower()]


def directory(lines):
    """The circuit name directory of a listing: circuit number -> its path
    with the final dot ('' for the main circuit). As in compare_hspice.py."""
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
    path -> model (without its circuit number). As in compare_hspice.py."""
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


def model_listing(lis):
    """The `vth0`, `rsh` and `is` values that the model parameter listing of
    the .lis prints, by the model name as printed (`1:nch.1`): for --verbose"""
    out = {}
    if not os.path.exists(lis):
        return out
    name = None
    for line in open(lis, errors="replace"):
        m = re.search(r"model name:\s*(\S+)", line)
        if m:
            name = m.group(1)
            continue
        if name:
            for key, value in re.findall(r"\b(vth0|rsh|is)\s*=\s*(\S+)", line, re.I):
                out.setdefault(name, {})[key.lower()] = value
    return out


def hspice_value(kind, control, meas, measure):
    """HSPICE's value of the card parameter of a probe, from its measure and
    the deck's control: vth0 = LV9 - (LV9 of the control - its vth0); rsh =
    the control's rsh * its current / the probe's (the same L/W); is = the
    control's is * the probe's current / the control's (the same voltage)"""
    m, c = meas.get(measure), meas.get("ctl")
    if m is None or c is None:
        return None
    if kind == "lv9":
        return m - (c - control)
    if m == 0 or c == 0:
        return None
    if kind == "r":
        return control * abs(c) / abs(m)
    return control * abs(m) / abs(c)


def near(kind, a, b):
    how, tol = TOLERANCE[kind]
    if how == "abs":
        return abs(a - b) <= tol
    return abs(a - b) <= tol * max(abs(a), abs(b))


def fits(kind, value, hypotheses):
    """The rules of a probe whose value `value` fits"""
    return [label for label, v in hypotheses if near(kind, value, v)]


def compare_deck(kit, results, deck, verbose):
    """Prints the probes of `deck`; returns (differences, agreements, probed)"""
    name, kind = deck["deck"], deck["kind"]
    lis = os.path.join(results, f"{name}.lis")
    errs = errors(lis)
    meas = measures(os.path.join(results, f"{name}.ms0"))
    stopped = bool(errs) and not meas
    ops = op_models(lis) if kind == "lv9" else {}
    print(f"== {name}: {deck['what']}")
    for e in errs[:3]:
        print(f"   HSPICE: {e[:160]}")
    if verbose:
        for w in warnings(lis)[:5]:
            print(f"   HSPICE: {w[:160]}")
    differ = agree = probed = 0
    for p in deck["probes"]:
        nd = p["netlist_db"]
        want = nd.get("values") or []
        nv = want[0] if len(want) == 1 else None
        h = None if stopped or meas is None else hspice_value(kind, deck["control"], meas, p["measure"])
        rules = fits(kind, h, p["hypotheses"]) if h is not None else []
        shown = "stopped" if stopped else "no result" if h is None else f"{h:.6g}"
        if p["check"] == "probe":
            probed += 1
            verdict = "probed"
            detail = f"HSPICE {shown}: {', '.join(rules) or 'no listed rule'} (netlist-db lists {want})"
        elif "error" in nd:
            differ += 1
            verdict = "DIFFER"
            detail = f"netlist-db: {nd['error']}"
        elif nv is None:
            # netlist-db has no value: HSPICE should stop on the deck
            if stopped:
                agree += 1
                verdict = "agree"
                detail = "HSPICE stops, and netlist-db has no value"
            else:
                differ += 1
                verdict = "DIFFER"
                fit = ", ".join(rules) or "no listed rule"
                why = (nd.get("unresolved") or [nd.get("card_unresolved")])[0]
                detail = f"HSPICE {shown} ({fit}); netlist-db has no value: {why}"
        elif h is None:
            differ += 1
            verdict = "DIFFER"
            detail = f"HSPICE {shown}; netlist-db {nv:.6g}"
        elif near(kind, h, nv):
            agree += 1
            verdict = "agree"
            detail = f"HSPICE {shown}, netlist-db {nv:.6g} ({', '.join(rules)})"
        else:
            differ += 1
            verdict = "DIFFER"
            fit = ", ".join(rules) or "no listed rule"
            detail = f"HSPICE {shown} fits: {fit}; netlist-db {nv:.6g}"
        print(f"   {p['path']:10} {p['param']:5} {verdict:7} {detail}")
        # the card of a MOSFET, from the operating point table
        card = (nd.get("card") or "").lower()
        if kind == "lv9" and card and not stopped:
            op = ops.get(p["path"].lower())
            if op is None:
                print(f"   {'':10} {'card':5} {'?':7} no .op row (netlist-db {card})")
            elif op != card:
                differ += 1
                print(f"   {'':10} {'card':5} DIFFER  .op names {op} (netlist-db {card})")
    if verbose:
        for model, values in sorted(model_listing(lis).items()):
            print(f"   listing {model}: {values}")
    return differ, agree, probed


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    kit, results = sys.argv[1], sys.argv[2]
    verbose = "--verbose" in sys.argv
    expected = json.load(open(os.path.join(kit, "expected.json")))
    version = open(os.path.join(kit, "VERSION")).read().strip()
    hv = os.path.join(results, "hspice_version.txt")
    print(f"netlist-db's answers: {version}")
    if os.path.exists(hv):
        first = next((line.strip() for line in open(hv, errors="replace") if line.strip()), "")
        print(f"HSPICE: {first[:120]}")
    print()
    totals = [0, 0, 0]
    for deck in expected["decks"]:
        for i, n in enumerate(compare_deck(kit, results, deck, verbose)):
            totals[i] += n
        print()
    differ, agree, probed = totals
    print(f"probes: {agree} agree, {differ} differ, {probed} probed (no netlist-db answer to check)")
    sys.exit(1 if differ else 0)


if __name__ == "__main__":
    main()
