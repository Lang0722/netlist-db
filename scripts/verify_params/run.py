#!/usr/bin/env python3
"""Random decks that stress parameter passing into MOSFETs: netlist-db's
every query mode against an independent reference, and (with ngspice
installed) the reference against ngspice.

usage: run.py FIRST_SEED LAST_SEED [--keep DIR]

For each seed, three decks (pgen.py): one that ngspice reads as HSPICE does
(parhier=local), one with parhier=local and one with parhier=global (with
WNFLAG, .option wnflag, names defined only in subckts, .param functions).
For each deck:
- the listing (model_select DECK) against the reference (pref.py): the
  path, L, W, NF and model of every MOSFET
- the counts (model_select --summary) against the reference's listing
- for two subckts, `--subckt NAME` listing and counts likewise
- (ngspice decks) the reference against ngspice: L, W, NF; and the models,
  with ngspice's own rule at bin edges (a value within 1e-9 of a limit, on
  either side, matches; the model defined last wins), and with HSPICE's

netlist-db's model_select is $MODEL_SELECT, or target/release/examples/
model_select of this repository (cargo build --release --example
model_select).
"""
import collections
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
MS = os.environ.get("MODEL_SELECT", os.path.join(REPO, "target/release/examples/model_select"))
NGSPICE = shutil.which("ngspice") is not None

stats = collections.Counter()
fails = []


def run(cmd, out):
    with open(out, "w") as f:
        return subprocess.run(cmd, stdout=f, stderr=subprocess.PIPE, text=True, timeout=600).returncode


def compare(a, b, exact, label):
    """Two listings, MOSFET by MOSFET"""
    r = subprocess.run(["python3", os.path.join(HERE, "compare.py"), a, b] + (["--exact"] if exact else []),
                       capture_output=True, text=True)
    if not r.stdout:
        fails.append(f"{label} {a}: {r.stderr[-400:]}")
        return
    stats[label + ": MOSFETs"] += int(r.stdout.split()[0])
    stats[label + ": decks"] += 1
    if r.returncode:
        stats[label + ": decks that differ"] += 1
        fails.append(f"{label} {a}: {r.stdout.strip()[:600]}")


def tally(listing):
    c = collections.Counter()
    for line in open(listing).read().splitlines()[1:]:
        m = line.split("\t")[-1]
        c["error" if m.startswith("error") else m.lower()] += 1
    return c


def summary(path):
    c = collections.Counter()
    for line in open(path).read().splitlines()[1:]:
        f = line.split("\t")
        name = f[0].split(":", 1)[1] if ":" in f[0] and not f[0].startswith("error") else f[0]
        if name == "total":
            continue
        c["error" if name.startswith("error") else name.lower()] += int(f[1])
    return c


def compare_counts(listing, summ, label):
    a, b = tally(listing), summary(summ)
    stats[label + ": decks"] += 1
    stats[label + ": MOSFETs"] += sum(a.values())
    if a != b:
        stats[label + ": decks that differ"] += 1
        fails.append(f"{label} {summ}: {dict(a - b)} vs {dict(b - a)}")


def main():
    first, last = int(sys.argv[1]), int(sys.argv[2])
    keep = sys.argv[sys.argv.index("--keep") + 1] if "--keep" in sys.argv else None
    work = keep or tempfile.mkdtemp(prefix="verify_params_")
    os.makedirs(work, exist_ok=True)
    if not os.path.exists(MS):
        sys.exit(f"no {MS}: cargo build --release --example model_select (or set MODEL_SELECT)")
    py = lambda script: ["python3", os.path.join(HERE, script)]  # noqa: E731
    for seed in range(first, last + 1):
        rng = random.Random(seed)
        for kind in ("ngspice", "local", "global"):
            deck = os.path.join(work, f"{kind}{seed}.sp")
            args = py("pgen.py") + [str(seed), deck]
            args += ["--ngspice"] if kind == "ngspice" else ["--parhier", kind]
            subprocess.run(args, check=True)
            ref, ndb = deck + ".ref", deck + ".ndb"
            if run(py("pref.py") + [deck], ref):
                fails.append(f"the reference failed on {deck}")
                continue
            run([MS, deck], ndb)
            compare(ndb, ref, True, f"{kind} listing")
            run([MS, "--summary", deck], deck + ".sum")
            compare_counts(ref, deck + ".sum", f"{kind} --summary")
            subs = re.findall(r"(?m)^\.subckt (\S+)", open(deck).read())
            top_subs = [s for s in subs if not s.startswith("n")]
            for sub in rng.sample(top_subs, min(2, len(top_subs))):
                sref, sndb, ssum = deck + f".{sub}.ref", deck + f".{sub}.ndb", deck + f".{sub}.sum"
                run(py("pref.py") + [deck, "--subckt", sub], sref)
                run([MS, "--subckt", sub, deck], sndb)
                # (model_select prefixes the paths with `SUBCKT:`)
                lines = open(sndb).read().splitlines()
                with open(sndb, "w") as f:
                    f.write("\n".join([lines[0]] + [l.split(":", 1)[1] for l in lines[1:]]) + "\n")
                compare(sndb, sref, True, f"{kind} --subckt listing")
                run([MS, "--summary", "--subckt", sub, deck], ssum)
                compare_counts(sref, ssum, f"{kind} --subckt --summary")
            if kind == "ngspice" and NGSPICE:
                ngs = deck + ".ngs"
                if run(py("ngs.py") + [deck], ngs) or len(open(ngs).read().splitlines()) < 2:
                    # (a random expression can give a size no bin holds:
                    # ngspice stops)
                    stats["ngspice: decks it did not run"] += 1
                    continue
                run(py("pref.py") + [deck, "--ngspice-rule"], deck + ".refng")
                compare(deck + ".refng", ngs, False, "reference (ngspice's rule) vs ngspice")
                compare(ndb, ngs, False, "netlist-db vs ngspice (HSPICE's rule)")
    for k in sorted(stats):
        print(f"{k:55s} {stats[k]}")
    if not NGSPICE:
        print("(ngspice is not installed: no comparison with it)")
    # the comparisons with ngspice are informational: they differ at bin
    # edges (a value within 1 nm of a limit, where ngspice's choice between
    # the two bins depends on its order of models); netlist-db against the
    # reference must agree exactly
    real = [f for f in fails if "ngspice" not in f]
    for f in fails:
        if "ngspice" in f:
            print("(ngspice) " + f.splitlines()[0])
    print(f"{len(real)} failure(s)")
    for f in real[:20]:
        print(f)
    if not keep:
        shutil.rmtree(work)
    sys.exit(1 if real else 0)


if __name__ == "__main__":
    main()
