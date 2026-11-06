#!/usr/bin/env python3
"""Turns gen_big_netlist.py decks into decks HSPICE runs with no analysis.
usage: mk.py OUTDIR NAME:ARGS... (ARGS with commas, e.g. l1m:--small,--clusters,0,--mos,1000000)
       a name that starts with `nolist` has no `.option list`."""
import filecmp, os, re, shutil, subprocess, sys
G = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gen_big_netlist.py")
def simplify(path):
    """Keeps the type, level, version and limits of each model card: the random
    numbers of the others make BSIM4 stop (`Effective channel width <= 0`)."""
    res, keep_paren = [], False
    for line in open(path):
        if line.startswith("+") and in_model:
            if "lmin" in line.lower():
                res.append(line.rstrip("\n").replace(")", "") + "\n")
            if ")" in line:
                res.append("+ )\n")
                in_model = False
            continue
        in_model = line.lower().startswith(".model")
        res.append(line)
    open(path, "w").writelines(res)

in_model = False
out = sys.argv[1]
decks = os.path.join(out, "decks")
os.makedirs(decks, exist_ok=True)
for spec in sys.argv[2:]:
    name, args = spec.split(":", 1)
    tmp = os.path.join(out, "gen_" + name)
    subprocess.run(["python3", G, tmp] + args.split(","), check=True, stdout=subprocess.DEVNULL)
    text = open(os.path.join(tmp, "chip.sp")).read()
    # the first run only: `.alter` has no use here, and a no-analysis listing mixes the runs
    text = text[: re.search(r"^\.alter", text, re.M).start()]
    # the on-purpose decap: HSPICE stops at a MOSFET no bin holds
    text, n = re.subn(r"^XDCAPB\d+ .*\n", "", text, flags=re.M)
    assert n == 4, n
    text = text.replace(".include 'stdcell.sp'", ".include 'stdcell.inc'")
    opt = "nomod post=0" if name.startswith("nolist") else "list nomod post=0"
    text += f".model pnp_bjt pnp\n.option {opt}\n.end\n"
    open(os.path.join(decks, name + ".sp"), "w").write(text)
    for src, dst in (("stdcell.sp", "stdcell.inc"), ("analog.inc", "analog.inc"), ("models.l", "models.l")):
        d = os.path.join(decks, dst)
        s = os.path.join(tmp, src)
        if os.path.exists(d):
            assert dst == "models.l" or filecmp.cmp(s, d, shallow=False), src
        else:
            shutil.copy(s, d)
            if dst == "models.l":
                simplify(d)
    exp = open(os.path.join(tmp, "expected.tsv")).read()
    open(os.path.join(out, name + ".expected.tsv"), "w").write(exp)
    shutil.rmtree(tmp)
    print(name, os.path.getsize(os.path.join(decks, name + ".sp")) // 1024, "KB;", re.search(r"total\t(\d+)", exp).group(1), "in expected.tsv (with 4 no-bin decaps)")
