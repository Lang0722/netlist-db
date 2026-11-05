#!/usr/bin/env python3
"""Make the HSPICE kit: decks to run on a machine with HSPICE, netlist-db's
answers for each of them, and a script that runs HSPICE on all of them.
compare_hspice.py then compares HSPICE's results with these answers.

usage: make_kit.py OUTDIR [--random N] [--bin STATIC_MODEL_SELECT]

--bin: a model_select to put in the kit's bin/ (a static one runs on any
x86-64 Linux: cargo build --release --example model_select --target
x86_64-unknown-linux-musl)

What HSPICE reports, per MOSFET of each deck (but the probes):
- LV1, LV2: its effective L and W (W per finger with NF > 1): the models of
  the kit zero what makes them differ from the drawn ones (LINT, WINT, XL,
  XW), so that they are L and W/NF;
- LV9: its threshold voltage. Each model's VTH0 is 0.2 + 0.02*N V (N: the
  number of a bin `name.N`, 0 for a model without one; negative for PMOS),
  and the terms that move the threshold (DIBL, short and narrow channel,
  pocket, lateral doping; the bodies are at the sources) are zeroed: LV9
  tells the bin chosen;
- the `.op` table of the .lis, which names each MOSFET's model (a second
  way to read the bin).
The probes (parhier_probe.sp) read their answers through `.measure` of
voltages and currents instead.
"""
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPTS = os.path.dirname(HERE)
REPO = os.path.dirname(SCRIPTS)
sys.path.insert(0, os.path.join(SCRIPTS, "verify_params"))
sys.path.insert(0, SCRIPTS)
import torture  # noqa: E402
import parhier_probe  # noqa: E402

MS = os.environ.get("MODEL_SELECT", os.path.join(REPO, "target/release/examples/model_select"))
VTH_BASE, VTH_STEP = 0.2, 0.02
ZEROED = "k3=0 dvt0=0 dvt0w=0 eta0=0 etab=0 dvtp0=0 lpe0=0 lpeb=0 lint=0 wint=0 xl=0 xw=0"


def bin_number(name):
    m = re.fullmatch(r"(.+)\.(\d+)", name.lower())
    return int(m.group(2)) if m else 0


def prepare_models(text):
    """Every model: level 54, its VTH0 telling its bin, the threshold terms
    zeroed"""
    out = []
    for line in text.splitlines():
        m = re.match(r"(?i)^\.model\s+(\S+)\s+(\S+)(.*)$", line)
        if m:
            name, kind, rest = m.groups()
            sign = -1 if kind.lower().startswith("p") else 1
            vth = sign * (VTH_BASE + VTH_STEP * bin_number(name))
            if not re.search(r"(?i)\blevel\s*=", rest):
                rest = " level=54" + rest
            line = f".model {name} {kind}{rest} vth0={vth:.3f} {ZEROED}"
        out.append(line)
    return "\n".join(out) + "\n"


def listing(deck):
    """netlist-db's listing of a deck: (path, model name, L, W, NF, model)"""
    out = subprocess.run([MS, deck], capture_output=True, text=True, check=True).stdout
    return [line.split("\t") for line in out.splitlines()[1:]]


def finish(path, text):
    """Adds the analyses and the measures of every MOSFET to the deck `text`,
    writes it to `path`; returns netlist-db's listing of it"""
    body = re.sub(r"(?im)^\.end\s*$", "", text).rstrip() + "\n"
    with open(path, "w") as f:
        f.write(body + ".end\n")
    rows = listing(path)
    source = re.search(r"(?im)^(v\w*)\s", body).group(1)
    cards = [".option post=0 ingold=2 numdgt=10 measdgt=10", ".op", f".dc {source} 1 1 1"]
    for i, row in enumerate(rows):
        el = row[0].lower()
        cards += [f".meas dc l{i} find lv1({el}) at=1",
                  f".meas dc w{i} find lv2({el}) at=1",
                  f".meas dc v{i} find lv9({el}) at=1"]
    with open(path, "w") as f:
        f.write(body + "\n".join(cards) + "\n.end\n")
    # (the analyses change nothing for netlist-db)
    assert listing(path) == rows, path
    return rows


def drive(text):
    if re.search(r"(?im)^v\w*\s+d\s", text):
        return text
    return re.sub(r"(?im)^\.end\s*$", "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.end", text)


def decks():
    """(name, deck text, what it checks)"""
    out = []
    for mode in ("local", "global"):
        text, _, _ = torture.chain(mode)
        out.append((f"chain_{mode}", text,
                    f"24 levels passing w='w*1.05' l='l+1n' down; lk: the top .param or the default ({mode})"))
        sub = re.sub(r"(?m)^X0 d g 0 c23 .*$", "X0 d g 0 c10", text)
        out.append((f"chain_sub_{mode}", sub,
                    f"an instance of c10 with its defaults (model_select --subckt c10), {mode}"))
        for top in (True, False):
            text, exp = torture.matrix(mode, top)
            # (HSPICE stops at an undefined parameter: those combinations out)
            for path, e in exp.items():
                if e[3] == "error":
                    c = path.split(".")[0]
                    text = re.sub(rf"(?m)^{c} d g 0 .*\n", "", text)
            out.append((f"matrix_{mode}_{'top' if top else 'notop'}", text,
                        f"v defined/passed at 3 levels in every combination, {mode}, "
                        f"top-level .param v {'defined' if top else 'not defined'}"))
        out.append((f"lattice4_{mode}", torture.lattice(4, mode),
                    f"three instances per level passing a='a', a='a', a='a+1' ({mode})"))
    out.append(("tree6", torture.tree(6).replace("floor(", "int("),
                "a binary tree passing p='2*p' and p='2*p+1' (each leaf's p is its path)"))
    text, exp, stext, sexp = torture.bins_case()
    # the transistors no bin holds go to the question decks
    for name in ("M3", "M10", "M12"):
        text = re.sub(rf"(?m)^{name} .*\n", "", text)
    text = re.sub(r"(?m)^M1 d g s s loc l=0.8u w=1u\n", "", text)
    out.append(("bins", text, "overlapping, open-ended bins, a model and bins of one name, case, "
                "dotted names, WNFLAG in one bin, edges through float noise, local bins"))
    out.append(("scale", stext, ".option scale=1e-6"))
    lib, _ = torture.library_case()
    lib = re.sub(r"(?m)^\.end\s*$", "X1 d g 0 inv\nX2 d g 0 top2\nX3 d g 0 later\n.end", lib)
    out.append(("library", lib, "a nested subckt hiding a top-level one, case, a subckt defined after use"))
    q = [
        ("q_gap", ".model gp.1 nmos level=54 lmin=1e-7 lmax=2e-7\n.model gp.2 nmos level=54 lmin=3e-7 lmax=4e-7\n"
                  "M1 d g 0 0 gp l=0.25u w=1u\n", "L between two bins: no bin (netlist-db: error)"),
        ("q_dotted", ".model n.ch.1 nmos level=54 lmin=0 lmax=1\nM1 d g 0 0 n l=0.5u w=1u\n",
         "a device of model `n` with only `n.ch.1`: no bin of n (netlist-db: error)"),
        ("q_nonnumeric", ".model nm.1 nmos level=54 lmin=1e-7 lmax=1e-6\n.model nm.mc nmos level=54\n"
                         ".model nm.1a nmos level=54\nM1 d g 0 0 nm l=5u w=1u\n",
         "nm.mc and nm.1a are not bins of nm (netlist-db: error, L in no bin)"),
        ("q_localhide", ".model loc.1 nmos level=54 lmin=0 lmax=1e-6\n.model loc.2 nmos level=54 lmin=1e-6 lmax=1\n"
                        ".subckt hide d g s\n.model loc.1 nmos level=54 lmin=0 lmax=5e-7\n"
                        "M1 d g s s loc l=0.8u w=1u\n.ends hide\nX1 d g 0 hide\n",
         "local bins hide the global ones (netlist-db: error, L in no local bin)"),
        ("q_nf_default", ".model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6\n"
                         ".model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3\n"
                         "M1 d g 0 0 a w=4u l=1u nf=8\n",
         "W=4u NF=8, no WNFLAG (netlist-db: total width, a.2)"),
        ("q_wnflag_model", ".model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6 wnflag=1\n"
                           ".model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3 wnflag=1\n"
                           "M1 d g 0 0 a w=4u l=1u nf=8\n",
         "W=4u NF=8, WNFLAG=1 in the models (netlist-db: width per finger, a.1)"),
        ("q_wnflag_instance", ".model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6\n"
                              ".model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3\n"
                              "M1 d g 0 0 a w=4u l=1u nf=8 wnflag=1\n",
         "W=4u NF=8, wnflag=1 on the instance (netlist-db: a.1; HSPICE may not know it)"),
        ("q_wnflag_option", ".option wnflag=1\n.model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6\n"
                            ".model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3\n"
                            "M1 d g 0 0 a w=4u l=1u nf=8\n",
         ".option wnflag=1 (ngspice's; netlist-db: a.1; HSPICE may not know it)"),
        ("q_edge", ".model e.1 nmos level=54 lmin=1e-8 lmax=1e-6\n.model e.2 nmos level=54 lmin=1e-6 lmax=1e-3\n"
                   "M1 d g 0 0 e l=0.9995u w=1u\nM2 d g 0 0 e l=1u w=1u\nM3 d g 0 0 e l='0.1u*10' w=1u\n",
         "L 0.5 nm below an edge (netlist-db: e.1), on it (e.2), on it through float noise (e.2)"),
    ]
    for name, body, what in q:
        out.append((name, f"* {name}\n{body}.end\n", what))
    return out


def random_decks(n):
    """Decks of pgen.py that HSPICE can run: every MOSFET has a model, NF
    whole, sizes below 1 mm; without the WNFLAG of instances and options
    (asked by the question decks)"""
    out = []
    pgen = os.path.join(SCRIPTS, "verify_params", "pgen.py")
    tmp = os.path.join(os.environ.get("TMPDIR", "/tmp"), "hspice_kit_random")
    os.makedirs(tmp, exist_ok=True)
    for kind in ("ngspice", "local", "global"):
        found, seed = 0, 0
        while found < n:
            seed += 1
            deck = os.path.join(tmp, f"{kind}{seed}.sp")
            args = ["python3", pgen, str(seed), deck] + (["--ngspice"] if kind == "ngspice" else ["--parhier", kind])
            subprocess.run(args, check=True)
            text = open(deck).read()
            text = re.sub(r" wnflag=[01]", "", text)
            text = re.sub(r"(?m)^\.option wnflag=1\n", "", text)
            with open(deck, "w") as f:
                f.write(text)
            rows = listing(deck)
            ok = (rows and len(rows) <= 400
                  and all(not r[5].startswith("error") for r in rows)
                  and all(float(r[4]) == int(float(r[4])) for r in rows)
                  and all(float(r[2]) < 1e-3 and float(r[3]) < 1e-3 for r in rows))
            if ok:
                found += 1
                what = {"ngspice": "random, parhier=local, as ngspice reads HSPICE",
                        "local": "random, parhier=local: nested subckts, .param functions, WNFLAG models",
                        "global": "random, parhier=global: nested subckts, .param functions, WNFLAG models"}[kind]
                out.append((f"r_{kind}{seed}", text, what))
    shutil.rmtree(tmp)
    return out


RUN = """#!/bin/bash
# Runs HSPICE on every deck of decks/ (each on its own: a deck HSPICE stops
# on does not stop the others), writes results/, and packs them into
# hspice_results.tar.gz to bring back.
#   ./run_hspice.sh            (HSPICE=/path/to/hspice ./run_hspice.sh)
cd "$(dirname "$0")"
HSPICE=${HSPICE:-hspice}
mkdir -p results
"$HSPICE" -v > results/hspice_version.txt 2>&1 || true
for deck in decks/*.sp; do
  name=$(basename "$deck" .sp)
  echo "== $name"
  "$HSPICE" -i "$deck" -o "results/$name" > "results/$name.stdout" 2>&1 \\
    || echo "   (hspice exited with status $?: see results/$name.stdout and results/$name.lis)"
done
uname -a > results/machine.txt
tar czf hspice_results.tar.gz results
echo "done: bring back hspice_results.tar.gz"
"""


def main():
    out = sys.argv[1]
    n = int(sys.argv[sys.argv.index("--random") + 1]) if "--random" in sys.argv else 15
    if os.path.exists(out):
        shutil.rmtree(out)
    os.makedirs(os.path.join(out, "decks"))
    os.makedirs(os.path.join(out, "expected"))
    manifest = ["deck\tMOSFETs\twhat it checks"]
    # the probes, as they are
    probe = open(os.path.join(SCRIPTS, "parhier_probe.sp")).read()
    for mode in ("global", "local"):
        path = os.path.join(out, "decks", f"probe_{mode}.sp")
        with open(path, "w") as f:
            f.write(probe.replace(".option parhier=global", f".option parhier={mode}"))
        cases = parhier_probe.netlist_db([MS], path)
        with open(os.path.join(out, "expected", f"probe_{mode}.cases.tsv"), "w") as f:
            f.write("case\tnetlist-db\twhat\n")
            for case, (_, what) in parhier_probe.CASES.items():
                f.write(f"{case}\t{cases[case]}\t{what}\n")
        manifest.append(f"probe_{mode}\t-\tparameter scoping and binning questions (parhier={mode})")
    for name, text, what in decks() + random_decks(n):
        path = os.path.join(out, "decks", f"{name}.sp")
        rows = finish(path, prepare_models(drive(text)))
        with open(os.path.join(out, "expected", f"{name}.tsv"), "w") as f:
            f.write("index\tpath\tmodel name\tL\tW\tNF\tmodel\n")
            for i, r in enumerate(rows):
                f.write(f"{i}\t" + "\t".join(r) + "\n")
        manifest.append(f"{name}\t{len(rows)}\t{what}")
    with open(os.path.join(out, "MANIFEST.tsv"), "w") as f:
        f.write("\n".join(manifest) + "\n")
    with open(os.path.join(out, "run_hspice.sh"), "w") as f:
        f.write(RUN)
    os.chmod(os.path.join(out, "run_hspice.sh"), 0o755)
    for extra in ("README.md", "compare_hspice.py"):
        shutil.copy(os.path.join(HERE, extra), os.path.join(out, extra))
    os.makedirs(os.path.join(out, "bin"))
    shutil.copy(os.path.join(SCRIPTS, "gen_big_netlist.py"), os.path.join(out, "bin", "gen_big_netlist.py"))
    if "--bin" in sys.argv:
        dst = os.path.join(out, "bin", "model_select")
        shutil.copy(sys.argv[sys.argv.index("--bin") + 1], dst)
        os.chmod(dst, 0o755)
    commit = subprocess.run(["git", "-C", REPO, "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
    with open(os.path.join(out, "VERSION"), "w") as f:
        f.write(f"netlist-db {commit}\n")
    print(f"{len(manifest) - 1} decks in {out}")


if __name__ == "__main__":
    main()
