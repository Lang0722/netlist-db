#!/usr/bin/env python3
"""Compare netlist-db's model selection with HSPICE's `.option list` table.

usage: cmp_list.py DECKS_DIR RESULTS_DIR [--only PREFIX] [--show N] [--ms PATH]

Per deck: did HSPICE stop, did netlist-db report an error, and per MOSFET: the
path, the model with its bin, drawn L and W, NF. A deck that has no analysis
line costs HSPICE no solve, so this scales to what its memory holds.
"""
import collections, glob, os, re, subprocess, sys

MS = os.environ.get("MODEL_SELECT", "")
TOL = 6e-4  # the table prints 4 digits
stats = collections.Counter()


def errors(lines):
    out = []
    for line in lines:
        if re.search(r"\*\*error\*\*|\berror:|aborted|\bfatal\b", line, re.I):
            out.append(line.strip())
    return out


def directory(lines):
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


def table(lines):
    """path -> dict(model, l, w, nf, m) from the first job's element summary"""
    paths = directory(lines)
    rows = {}
    order = []
    in_summary = in_mos = False
    current = None
    for line in lines:
        if "circuit element summary" in line:
            in_summary = True
            continue
        if not in_summary:
            continue
        s = line.strip()
        if "job concluded" in s or "option summary" in s or "**** subcircuit calls" in s:
            if rows and ("job concluded" in s or "option summary" in s):
                break
            if "subcircuit calls" in s:
                in_mos = False
            continue
        if s.startswith("****"):
            low = s.lower()
            if "mosfets" in low:
                in_mos = True
            elif in_mos and "model" in low:
                pass
            else:
                in_mos = False
            continue
        if not in_mos or not s:
            continue
        parts = s.split()
        if s.startswith("element name"):
            current = parts[2:]
            for e in current:
                number, local = e.split(":", 1)
                path = paths.get(int(number), f"?{number}.") + local.lower()
                rows[e] = {"path": path}
                order.append(e)
            continue
        if current is None:
            continue
        for key, n in (("model", 1), ("w drawn", 2), ("l drawn", 2), ("w eff", 2), ("l eff", 2), ("nf", 1), ("m", 1)):
            if parts[:n] == key.split():
                for e, v in zip(current, parts[n:]):
                    rows[e][key] = v
                break
    return {rows[e]["path"]: rows[e] for e in order}


SUFFIX = {"a": 1e-18, "f": 1e-15, "p": 1e-12, "n": 1e-9, "u": 1e-6, "m": 1e-3, "k": 1e3, "x": 1e6, "g": 1e9, "t": 1e12}


def num(x):
    """A number of a listing: `1.000e-06`, `1.0000u`, `500.0000m`, `0.`"""
    if x is None:
        return None
    try:
        return float(x)
    except ValueError:
        pass
    m = re.fullmatch(r"([-+]?[\d.]+)([afpnumkxgt])", x.lower())
    return float(m.group(1)) * SUFFIX[m.group(2)] if m else None


def first_run_l(results, name):
    """The L of the first run from a `.meas`: its `.ms0` has a column `l0`. The
    table shows the 25 C value of a `temper` expression, and it mixes the runs of
    `.alter`, so it cannot tell these."""
    path = os.path.join(results, name + ".ms0")
    if not os.path.exists(path):
        return None
    rows = [x.split() for x in open(path, errors="replace").read().splitlines() if x.strip()]
    for k, row in enumerate(rows[:-1]):
        if row and row[0].lower() == "l0":
            return num(rows[k + 1][0])
    return None


def plain_scale(text):
    """The last plain number of `.option scale`, if one is above 0"""
    found = re.findall(r"(?im)^\s*\.opt[^\n]*\bscale\s*=\s*([0-9.][0-9.e+-]*)(?:\s|$)", text)
    if found and float(found[-1]) > 0:
        return float(found[-1])
    return 1.0


def is_default(value, text, option, scale):
    """Whether `value` is the size of a line without it: DEFL or DEFW, or a size
    that the deck can reach through them (an include can also set them)"""
    near = lambda a, b: abs(a - b) <= 1e-3 * max(abs(a), abs(b))  # noqa: E731
    if re.search(r"(?i)\bscale\s*=", text) and scale == 1.0:
        # (a form of `scale` that the comparator does not read)
        return True
    candidates = [1e-4, 1e-4 * scale]
    for found in re.findall(rf"(?i)\b{option}\s*=\s*([0-9.][0-9.e+-]*[a-z]*)", text):
        v = num(found.lower()) or 0
        candidates += [v, v * scale]
    return any(near(value, c) for c in candidates) or "inc" in text.lower() or ".lib" in text.lower()


def ndb(deck, cwd, ms):
    p = subprocess.run([ms, os.path.basename(deck)], cwd=cwd, capture_output=True, text=True, errors="replace", timeout=120)
    rows = [line.split("\t") for line in p.stdout.splitlines()[1:]]
    return rows, p.stderr, p.returncode


def compare(decks, results, name, ms):
    lis = os.path.join(results, name + ".lis")
    if not os.path.exists(lis):
        return "no-lis", [], {}
    lines = open(lis, errors="replace").read().splitlines()
    if not lines:
        # (HSPICE printed nothing: no message tells why)
        return "empty-lis", [], {}
    errs = errors(lines)
    done = any("job concluded" in x for x in lines)
    # A job killed at the deadline, after it printed its whole MOSFET table
    # (the next section starts): the table can still be compared.
    table_done = any("**** subcircuit calls" in x for x in lines)
    # `--partial`: the rows of a table cut by the deadline
    partial = "--partial" in sys.argv and not done and not table_done and not errs
    stopped = bool(errs) or not (done or table_done or partial)
    rows, stderr, rc = ndb(os.path.join(decks, name + ".sp"), decks, ms)
    text_err = "the netlist has errors" in stderr
    row_err = [r for r in rows if r[5:] and r[5].startswith("error")] if rows else []
    crash = rc != 0
    info = {"hspice": errs[:2], "ndb_rows": len(rows), "text_err": text_err, "row_err": [r[0] + ": " + r[5] for r in row_err[:2]]}
    if crash:
        return "NDB-CRASH", [stderr[-300:]], info
    if stopped and (text_err or row_err):
        return "both-stop", [], info
    if stopped:
        return "HSPICE-STOPS-ONLY", [], info
    text = open(os.path.join(decks, name + ".sp"), errors="replace").read()
    alter = bool(re.search(r"(?im)^\s*\.alt", text))
    measured = first_run_l(results, name)
    if alter and measured is None:
        # (the table mixes the runs, and no `.meas` tells the first one)
        return "alter-unmeasured", [], info
    if measured is None and re.search(r"(?im)^\s*\.temp", text) and re.search(r"(?i)\btemper\b", text):
        # (the table shows the 25 C value of a `temper` expression)
        return "temper-unmeasured", [], info
    scale = plain_scale(text)
    tab = table(lines)
    diffs = []
    count = None
    for x in lines:
        m = re.search(r"# mosfets\s*=\s*(\d+)", x)
        if m:
            count = int(m.group(1))
            break
    info["count"] = count
    if count is None and table_done:
        count = len(table(lines))
        info["table_only"] = True
    if not alter and count is not None and count != len(rows):
        # (a MOSFET inside `.prot` has no row in the table, but it counts)
        diffs.append(f"count: HSPICE {count}, netlist-db {len(rows)}")
    protected = (count is not None and count > len(tab)) or partial
    want = {r[0].lower(): r for r in rows}
    for path in tab:
        if path not in want and not alter:
            diffs.append(f"HSPICE has {path} ({tab[path].get('model')}), netlist-db does not")
    for path, r in want.items():
        if path not in tab:
            if protected or alter:
                stats["not in the table (protected)"] += 1
            else:
                diffs.append(f"netlist-db has {path} ({r[5] if r[5:] else '?'}), HSPICE does not")
            continue
        h = tab[path]
        if len(r) < 6 or r[5].startswith("error"):
            diffs.append(f"{path}: netlist-db {r[5] if r[5:] else '?'}; HSPICE {h.get('model')}")
            continue
        hm = h.get("model", "?").split(":", 1)[-1].lower()
        nm = r[5].lower()
        if hm != nm and not (len(hm) >= 8 and nm.startswith(hm)):
            diffs.append(f"{path}: model HSPICE {hm}, netlist-db {nm}")
        near = lambda a, b: abs(a - b) <= TOL * max(abs(a), abs(b))  # noqa: E731
        nv = num(r[2])
        drawn, eff = num(h.get("l drawn")), num(h.get("l eff"))
        drawn = drawn * scale if drawn else drawn
        if measured is not None:
            drawn, eff = measured, None
        if drawn is not None and drawn != 0:  # (0: no L on the line; the table's `l eff` then is not DEFL)
            stats["L compared"] += 1
            if nv is None or not (near(drawn, nv) or (eff is not None and near(eff, nv))):
                diffs.append(f"{path}: L HSPICE {drawn:g} (eff {h.get('l eff')}), netlist-db {r[2]}")
        else:
            stats["L not on the line"] += 1
            if nv is not None and nv != 0 and not is_default(nv, text, "defl", scale):
                diffs.append(f"{path}: L not on the line in HSPICE, netlist-db {r[2]}")
        if alter:
            # (the table mixes the runs, and the `.meas` tells only L)
            continue
        nv = num(r[3])
        drawn, eff, hnf, hm_ = num(h.get("w drawn")), num(h.get("w eff")), num(h.get("nf")) or 1, num(h.get("m")) or 1
        drawn = drawn * scale if drawn else drawn
        if drawn is not None and drawn != 0:
            stats["W compared"] += 1
            if nv is None or not (near(drawn, nv) or (eff is not None and near(eff * hnf / hm_, nv))):
                diffs.append(f"{path}: W HSPICE {drawn:g} (eff {h.get('w eff')} nf {h.get('nf')} m {h.get('m')}), netlist-db {r[3]}")
        else:
            stats["W not on the line"] += 1
            if nv is not None and nv != 0 and not is_default(nv, text, "defw", scale):
                diffs.append(f"{path}: W not on the line in HSPICE, netlist-db {r[3]}")
        hv, nv = num(h.get("nf")), num(r[4])
        if hv is not None and nv is not None and abs(hv - nv) > TOL * max(abs(hv), abs(nv)):
            diffs.append(f"{path}: NF HSPICE {hv:g}, netlist-db {r[4]}")
    info["mosfets"] = len(tab)
    if text_err or row_err:
        return ("NDB-ERROR-ONLY" if not diffs else "NDB-ERROR+DIFF"), diffs, info
    return ("agree" if not diffs else "DIFFER"), diffs, info


def main():
    decks, results = sys.argv[1], sys.argv[2]
    show = int(sys.argv[sys.argv.index("--show") + 1]) if "--show" in sys.argv else 6
    ms = sys.argv[sys.argv.index("--ms") + 1] if "--ms" in sys.argv else MS
    only = sys.argv[sys.argv.index("--only") + 1] if "--only" in sys.argv else ""
    tally = collections.Counter()
    mos = 0
    details = []
    for path in sorted(glob.glob(os.path.join(decks, "*.sp"))):
        name = os.path.basename(path)[:-3]
        if only and not name.startswith(only):
            continue
        verdict, diffs, info = compare(decks, results, name, ms)
        tally[verdict] += 1
        mos += info.get("mosfets", 0)
        if verdict not in ("agree", "both-stop") or "--all" in sys.argv:
            details.append((name, verdict, diffs, info))
    for name, verdict, diffs, info in details:
        print(f"{name:26} {verdict}  n={info.get('ndb_rows')} text_err={info.get('text_err')}")
        for e in info.get("hspice", []):
            print(f"     H: {e[:130]}")
        for e in info.get("row_err", []):
            print(f"     N: {e[:130]}")
        for d in diffs[:show]:
            print(f"     {d[:130]}")
    print()
    for k, v in sorted(tally.items()):
        print(f"{k:24} {v}")
    print(f"{'MOSFETs compared':24} {mos}")
    for k, v in sorted(stats.items()):
        print(f"{k:24} {v}")


if __name__ == "__main__":
    main()
