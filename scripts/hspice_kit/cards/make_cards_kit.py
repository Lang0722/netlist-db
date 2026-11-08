#!/usr/bin/env python3
"""Make the HSPICE kit of the card values: decks that ask where HSPICE
evaluates the parameters of a `.model` card, netlist-db's answers to them
(from `resolve_model`), and the scripts that run HSPICE on the decks.
compare_cards.py then compares HSPICE's results with these answers.

usage: make_cards_kit.py OUTDIR [--resolve BINARY]

--resolve: the `resolve_model` example of the query crate (default
target/release/examples/resolve_model; build it with `cargo build --release
-p netlist-query --example resolve_model`).

How a deck tells HSPICE's value of a card parameter:
- `vth0` of an NMOS card (level 54): the other threshold terms of each card
  are zeroed (as in make_kit.py), and the bodies are at the sources, so the
  threshold LV9 of each MOSFET is its card's VTH0. A control MOSFET with a
  card of `vth0=0.3` gives the offset of LV9 from VTH0, if any.
- `rsh` of an R card: the line gives L and W (L/W = 10), so the current at 1 V
  gives the resistance RSH*L/W. A control R with `rsh=100` gives the scale.
- `is` of a diode card: the current at 0.6 V is IS*(exp(V/Vt)-1). A control
  diode with `is=1e-14` gives the scale.

Each probe lists the value that each rule would give ("hypotheses"), so a
difference tells which rule HSPICE follows. The kit in `kit/` next to this
script is made by this script, and `query/tests/cards_kit.rs` checks that
its answers are netlist-db's.
"""
import json
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
RESOLVE = os.path.join(REPO, "target", "release", "examples", "resolve_model")

# the threshold terms that make LV9 differ from VTH0 (make_kit.py)
ZEROED = "k1=0 k2=0 k3=0 dvt0=0 dvt0w=0 eta0=0 etab=0 dvtp0=0 lpe0=0 lpeb=0 lint=0 wint=0 xl=0 xw=0"
# and the temperature terms, for a deck at another temperature
NO_TEMP = "kt1=0 kt1l=0 kt2=0"

# the library of the corner decks, next to the decks
LIBRARY = "cards_pdk.lib"


def card(name, vth0, temp=False, extra=""):
    """An NMOS card of level 54 whose threshold is its VTH0"""
    zero = ZEROED + (" " + NO_TEMP if temp else "")
    return f".model {name} nmos level=54 {extra}vth0={vth0}\n+ {zero}\n"


PDK = f"""* cards kit: a PDK library in the macro-model style: corner sections set
* a global shift and load the core section, whose wrapper subckt passes its
* size to the binned cards inside it
.lib tt
.param dvg=0
.lib '{LIBRARY}' core
.endl tt
.lib ff
.param dvg=-0.05
.lib '{LIBRARY}' core
.endl ff
.lib core
.subckt nch_mac d g s b l=100n w=1u nf=1
.param dvl='1e-8/l'
.model nch.1 nmos level=54 lmin=1e-8 lmax=2e-7 wmin=1e-8 wmax=1e-4 vth0='0.30+dvg+dvl'
+ {ZEROED}
.model nch.2 nmos level=54 lmin=2e-7 lmax=1e-4 wmin=1e-8 wmax=1e-4 vth0='0.45+dvg+dvl'
+ {ZEROED}
M0 d g s b nch l=l w=w nf=nf
.ends nch_mac
.endl core
"""


def mac(parhier, top_param=""):
    return (f".option parhier={parhier}\n{top_param}.subckt mac d g s dv=0.03\n"
            + card("mn", "'0.2+dv'") + "M1 d g s s mn l=1u w=1u\n.ends mac\n"
            "X1 d g 0 mac\nX2 d g 0 mac dv=0.07\n")


def top_card(parhier, top_param):
    return (f".option parhier={parhier}\n{top_param}" + card("mt", "'0.2+dv'")
            + ".subckt c5 d g s dv=0.05\nM1 d g s s mt l=1u w=1u\n.ends c5\n"
            ".subckt c7 d g s dv=0.07\nM1 d g s s mt l=1u w=1u\n.ends c7\n"
            "X1 d g 0 c5\nX2 d g 0 c7\n")


def nested(parhier):
    return (f".option parhier={parhier}\n.subckt outer d g s dv=0.03\n" + card("mo", "'0.2+dv'")
            + ".subckt inner d g s dv=0.05\nM1 d g s s mo l=1u w=1u\n.ends inner\n"
            "X1 d g s inner\n.ends outer\nX1 d g 0 outer\n")


def r_cards(parhier):
    return (f".option parhier={parhier}\n.param rs=50\n.model rtop r rsh='rs*2'\n"
            ".subckt rcell a b rs=10\n.model rloc r rsh='rs*3'\nR1 a b rloc l=10u w=1u\n.ends rcell\n"
            "X1 a 0 rcell\nX2 a 0 rcell rs=20\nR0 a 0 rtop l=10u w=1u\n")


def probe(path, param, *hypotheses, check="value"):
    """A question: the value of `param` of the card of the element `path`,
    and the value that each rule would give"""
    return {"path": path, "param": param, "hypotheses": [list(h) for h in hypotheses], "check": check}


# (name, kind, what it asks, deck body, probes). The kind is the observable:
# `lv9` (an NMOS card's vth0), `r` (an R card's rsh) or `d` (a diode card's is).
DECKS = [
    ("c_mac_global", "lv9",
     "a card inside a subckt: is it evaluated for each instance? (parhier=global, no top-level dv)",
     mac("global"),
     [probe("x1.m1", "vth0", ("this instance: its default dv=0.03", 0.23)),
      probe("x2.m1", "vth0", ("this instance: dv=0.07 on its X line", 0.27),
            ("once for the subckt, in its first instance", 0.23))]),
    ("c_mac_local", "lv9",
     "a card inside a subckt, parhier=local, with a top-level dv",
     mac("local", ".param dv=0.01\n"),
     [probe("x1.m1", "vth0", ("this instance: its default dv=0.03", 0.23), ("the top-level dv=0.01", 0.21)),
      probe("x2.m1", "vth0", ("this instance: dv=0.07 on its X line", 0.27),
            ("once for the subckt, in its first instance", 0.23), ("the top-level dv=0.01", 0.21))]),
    ("c_mac_global_top", "lv9",
     "a card inside a subckt, parhier=global, with a top-level dv: does the top level win, as for an element?",
     mac("global", ".param dv=0.01\n"),
     [probe("x1.m1", "vth0", ("the top-level dv=0.01 (parhier=global)", 0.21), ("this instance's default dv=0.03", 0.23)),
      probe("x2.m1", "vth0", ("the top-level dv=0.01 (parhier=global)", 0.21), ("this instance: dv=0.07", 0.27))]),
    ("c_top_card_local", "lv9",
     "a top-level card used only by MOSFETs in subckts that define dv too (parhier=local): the known gap of the bin limits",
     top_card("local", ".param dv=0.01\n"),
     [probe("x1.m1", "vth0", ("where the card is defined: the top-level dv=0.01", 0.21),
            ("the MOSFET's own instance: dv=0.05 (the first MOSFET, as for the limits of a bin)", 0.25)),
      probe("x2.m1", "vth0", ("where the card is defined: the top-level dv=0.01", 0.21),
            ("the MOSFET's own instance: dv=0.07", 0.27), ("the value of the first MOSFET, x1.m1", 0.25))]),
    ("c_top_card_notop", "lv9",
     "a top-level card that reads a name only the MOSFETs' subckts define (parhier=global)",
     top_card("global", ""),
     [probe("x1.m1", "vth0", ("the MOSFET's own instance: dv=0.05", 0.25)),
      probe("x2.m1", "vth0", ("the MOSFET's own instance: dv=0.07", 0.27), ("the value of the first MOSFET, x1.m1", 0.25))]),
    ("c_nested_local", "lv9",
     "a card in outer, read by a MOSFET in inner, which defines dv too (parhier=local)",
     nested("local"),
     [probe("x1.x1.m1", "vth0", ("where the card is defined: outer's dv=0.03", 0.23),
            ("the MOSFET's own instance: inner's dv=0.05", 0.25))]),
    ("c_nested_global", "lv9",
     "the same, parhier=global",
     nested("global"),
     [probe("x1.x1.m1", "vth0", ("outer's dv=0.03 (where the card is defined, and the outermost)", 0.23),
            ("inner's dv=0.05", 0.25))]),
    ("c_dynamic_local", "lv9",
     "a card in mid, found along the instance path by a MOSFET of the top-level subckt leaf, which defines dv too",
     ".option parhier=local\n.subckt leaf d g s dv=0.05\nM1 d g s s md l=1u w=1u\n.ends leaf\n"
     ".subckt mid d g s dv=0.03\n" + card("md", "'0.2+dv'") + "X1 d g s leaf\n.ends mid\nX1 d g 0 mid\n",
     [probe("x1.x1.m1", "vth0", ("where the card is defined: mid's dv=0.03", 0.23),
            ("the MOSFET's own instance: leaf's dv=0.05", 0.25))]),
    ("c_body_param", "lv9",
     "a card that reads a body .param computed from the instance's L, as a PDK macro model does",
     ".subckt nch_mac d g s l=1u w=1u\n.param dvl='5e-8/l'\n" + card("nm", "'0.2+dvl'")
     + "M0 d g s s nm l=l w=w\n.ends nch_mac\n"
     "X1 d g 0 nch_mac l=0.5u\nX2 d g 0 nch_mac l=2u\nX3 d g 0 nch_mac\n",
     [probe("x1.m0", "vth0", ("this instance: l=0.5u", 0.30), ("the defaults: l=1u", 0.25)),
      probe("x2.m0", "vth0", ("this instance: l=2u", 0.225), ("the defaults: l=1u", 0.25),
            ("once for the subckt, in its first instance", 0.30)),
      probe("x3.m0", "vth0", ("this instance: the defaults, l=1u", 0.25),
            ("once for the subckt, in its first instance", 0.30))]),
    ("c_corner_tt", "lv9",
     f"a PDK library ({LIBRARY}): corner tt, binned cards in the wrapper subckt nch_mac",
     f".lib '{LIBRARY}' tt\nX1 d g 0 0 nch_mac l=50n\nX2 d g 0 0 nch_mac l=500n w=2u nf=2\n",
     [probe("x1.m0", "vth0", ("this instance (l=50n) in corner tt", 0.50), ("the defaults (l=100n)", 0.40)),
      probe("x2.m0", "vth0", ("this instance (l=500n) in corner tt", 0.47), ("the defaults (l=100n)", 0.55))]),
    ("c_corner_ff", "lv9",
     f"the same library, corner ff (dvg=-0.05)",
     f".lib '{LIBRARY}' ff\nX1 d g 0 0 nch_mac l=50n\nX2 d g 0 0 nch_mac l=500n w=2u nf=2\n",
     [probe("x1.m0", "vth0", ("this instance (l=50n) in corner ff", 0.45), ("this instance in corner tt", 0.50),
            ("the defaults (l=100n) in corner ff", 0.35)),
      probe("x2.m0", "vth0", ("this instance (l=500n) in corner ff", 0.42), ("this instance in corner tt", 0.47))]),
    ("c_func_global", "lv9",
     "a card that calls a top-level .param function with a subckt parameter",
     ".param f(x)='x*2'\n.subckt fm d g s dv=0.02\n" + card("mf", "'0.2+f(dv)'")
     + "M1 d g s s mf l=1u w=1u\n.ends fm\nX1 d g 0 fm\nX2 d g 0 fm dv=0.05\n",
     [probe("x1.m1", "vth0", ("this instance: f(0.02)", 0.24)),
      probe("x2.m1", "vth0", ("this instance: f(0.05)", 0.30), ("once for the subckt, in its first instance", 0.24))]),
    ("c_temper", "lv9",
     "a card that reads temper, at .temp 50 (the temperature terms zeroed)",
     ".temp 50\n" + card("mtp", "'0.2+temper*2m'", temp=True) + "M1 d g 0 0 mtp l=1u w=1u\n",
     [probe("m1", "vth0", ("temper is the .temp value, 50", 0.30), ("temper is TNOM, 25", 0.25))]),
    ("c_stat_param", "lv9",
     "a card that reads a .param with agauss (no Monte Carlo: the nominal value)",
     ".param mis=agauss(0,0.02,3)\n" + card("ms", "'0.25+mis'") + "M1 d g 0 0 ms l=1u w=1u\n",
     [probe("m1", "vth0", ("the nominal value of agauss, 0", 0.25))]),
    ("c_stat_direct", "lv9",
     "agauss directly in a card (HSPICE rejects it directly in an element expression: does it in a card?)",
     card("ms", "'0.25+agauss(0,0.02,3)'") + "M1 d g 0 0 ms l=1u w=1u\n",
     [probe("m1", "vth0", ("the nominal value of agauss, 0", 0.25))]),
    ("c_dup_key", "lv9",
     "a card that sets vth0 twice: which one does HSPICE use? (netlist-db lists both)",
     f".model md2 nmos level=54 vth0=0.24\n+ {ZEROED} vth0=0.32\nM1 d g 0 0 md2 l=1u w=1u\n",
     [probe("m1", "vth0", ("the first vth0", 0.24), ("the last vth0", 0.32), check="probe")]),
    ("c_r_global", "r",
     "R cards: one at the top level, one inside a subckt read by its instances (parhier=global)",
     r_cards("global"),
     [probe("x1.r1", "rsh", ("the top-level rs=50 (parhier=global)", 150), ("this instance's default rs=10", 30)),
      probe("x2.r1", "rsh", ("the top-level rs=50 (parhier=global)", 150), ("this instance: rs=20", 60)),
      probe("r0", "rsh", ("the top-level rs=50", 100))]),
    ("c_r_local", "r",
     "the same, parhier=local",
     r_cards("local"),
     [probe("x1.r1", "rsh", ("this instance's default rs=10", 30), ("the top-level rs=50", 150)),
      probe("x2.r1", "rsh", ("this instance: rs=20", 60), ("once for the subckt, in its first instance", 30),
            ("the top-level rs=50", 150)),
      probe("r0", "rsh", ("the top-level rs=50", 100))]),
    ("c_r_dynamic", "r",
     "an R card: the one on the instance path (mid's), or the one that the line's definition sees (the top level)?",
     ".model rm r rsh=200\n.subckt leaf a b\nR1 a b rm l=10u w=1u\n.ends leaf\n"
     ".subckt mid a b\n.model rm r rsh=300\nX1 a b leaf\n.ends mid\nX1 a 0 mid\nX2 a 0 leaf\n",
     [probe("x1.x1.r1", "rsh", ("the card on the instance path: mid's", 300),
            ("the card that the line's definition sees: the top-level one", 200)),
      probe("x2.r1", "rsh", ("the top-level card", 200))]),
    ("c_diode", "d",
     "a diode card inside a subckt, evaluated for each instance?",
     ".subckt dcell p n k=1\n.model dm d is='1e-14*k' n=1\nD1 p n dm\n.ends dcell\n"
     "X1 p 0 dcell\nX2 p 0 dcell k=4\n",
     [probe("x1.d1", "is", ("this instance: k=1", 1e-14)),
      probe("x2.d1", "is", ("this instance: k=4", 4e-14), ("once for the subckt, in its first instance", 1e-14))]),
]

# the control of each kind of deck: its lines, sources, sweep, and the value
# that its card sets
CONTROLS = {
    "lv9": {"value": 0.3, "measure": "lv9(mctl)", "at": 1},
    "r": {"value": 100.0, "measure": "i(rctl)", "at": 1},
    "d": {"value": 1e-14, "measure": "i(dctl)", "at": 0.6},
}


def control_lines(kind, temp):
    if kind == "lv9":
        return card("ctl", "0.3", temp=temp) + "mctl d g 0 0 ctl l=1u w=1u\nVd d 0 1\nVg g 0 1\n", ".dc Vd 1 1 1"
    if kind == "r":
        return ".model rc r rsh=100\nrctl a 0 rc l=10u w=1u\nVa a 0 1\n", ".dc Va 1 1 1"
    return ".model dcc d is=1e-14 n=1\ndctl p 0 dcc\nVp p 0 0.6\n", ".dc Vp 0.6 0.6 0.6"


def measure_of(kind, path):
    return f"lv9({path})" if kind == "lv9" else f"i({path})"


def deck_text(name, kind, what, body, probes):
    temp = ".temp" in body
    lines, sweep = control_lines(kind, temp)
    at = CONTROLS[kind]["at"]
    meas = [f".meas dc ctl find {CONTROLS[kind]['measure']} at={at}"]
    for i, p in enumerate(probes):
        meas.append(f".meas dc p{i} find {measure_of(kind, p['path'])} at={at}")
    return (f"* {name}: {what}\n{body}{lines}"
            ".option post=0 ingold=2 numdgt=10 measdgt=10\n.op\n"
            f"{sweep}\n" + "\n".join(meas) + "\n.end\n")


def netlist_db(resolve, deck, probes):
    """netlist-db's answer for each probe of `deck`: the card, its scope, and
    the values of the item (all its items, for a key set twice), or why it
    has none"""
    args = [resolve, deck] + [f"{p['path']}={p['param']}" for p in probes]
    out = subprocess.run(args, capture_output=True, text=True, check=True).stdout
    answers = []
    for p, line in zip(probes, out.splitlines()):
        row = json.loads(line)
        assert row["path"] == p["path"], (deck, row)
        if "error" in row:
            answers.append({"error": row["error"]})
            continue
        a = row["answer"]
        items = [i for i in a["params"]["items"] if i["name"].lower() == p["param"]]
        answers.append({
            "card": a["card"]["name"] if a["card"] else None,
            "scope": a["card"]["scope"] if a["card"] else None,
            "card_unresolved": a["unresolved"],
            "values": [i["value"] for i in items],
            "unresolved": [i["unresolved"] for i in items if i["unresolved"]],
        })
    return answers


RUN_SH = """#!/bin/bash
# Runs HSPICE on each deck of decks/, from inside decks/ (so that the `.lib`
# file next to the decks is found), writes results/, and packs them into
# hspice_card_results.tar.gz.
#   ./run_cards.sh            (HSPICE=/path/to/hspice ./run_cards.sh)
cd "$(dirname "$0")"
HSPICE=${HSPICE:-hspice}
mkdir -p results
"$HSPICE" -v > results/hspice_version.txt 2>&1 || true
cd decks
for deck in *.sp; do
  name=$(basename "$deck" .sp)
  echo "== $name"
  "$HSPICE" -i "$deck" -o "../results/$name" > "../results/$name.stdout" 2>&1 \\
    || echo "   (hspice exited with status $?: see results/$name.stdout and results/$name.lis)"
done
cd ..
uname -a > results/machine.txt 2>/dev/null || true
tar czf hspice_card_results.tar.gz results
echo "done: hspice_card_results.tar.gz"
"""

RUN_PS1 = """# Runs HSPICE on each deck of decks\\, from inside decks\\ (so that the .lib
# file next to the decks is found), writes results\\, and packs them into
# hspice_card_results.zip.
#   powershell -NoProfile -ExecutionPolicy Bypass -File run_cards.ps1
# Set $env:HSPICE to the path of hspice.exe first if it is not on the PATH
# (for example C:\\synopsys\\Hspice_P-2019.06-SP1-1\\WIN64\\hspice.exe).
$ErrorActionPreference = "Continue"
$kit = Split-Path -Parent $MyInvocation.MyCommand.Path
$hspice = if ($env:HSPICE) { $env:HSPICE } else { "hspice" }
$results = Join-Path $kit "results"
New-Item -ItemType Directory -Force -Path $results | Out-Null
Push-Location (Join-Path $kit "decks")
try {
    & $hspice -v *> (Join-Path $results "hspice_version.txt")
    foreach ($deck in Get-ChildItem -Filter *.sp | Sort-Object Name) {
        $name = $deck.BaseName
        Write-Host "== $name"
        & $hspice -i $deck.Name -o (Join-Path $results $name) *> (Join-Path $results "$name.stdout")
        if ($LASTEXITCODE -ne 0) {
            Write-Host "   (hspice exited with status $LASTEXITCODE: see results\\$name.stdout and results\\$name.lis)"
        }
    }
} finally {
    Pop-Location
}
Compress-Archive -Force -Path $results -DestinationPath (Join-Path $kit "hspice_card_results.zip")
Write-Host "done: hspice_card_results.zip"
"""


def close(a, b, kind):
    if kind == "lv9":
        return abs(a - b) <= 1e-9
    return abs(a - b) <= 1e-9 * max(abs(a), abs(b))


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    out = sys.argv[1]
    resolve = sys.argv[sys.argv.index("--resolve") + 1] if "--resolve" in sys.argv else RESOLVE
    if not os.path.exists(resolve):
        sys.exit(f"no {resolve}: cargo build --release -p netlist-query --example resolve_model")
    # the commit of the answers, before the kit's own files change
    commit = subprocess.run(["git", "-C", REPO, "describe", "--always", "--dirty"],
                            capture_output=True, text=True).stdout.strip()
    if os.path.exists(out):
        shutil.rmtree(out)
    decks_dir = os.path.join(out, "decks")
    os.makedirs(decks_dir)
    with open(os.path.join(decks_dir, LIBRARY), "w", newline="\n") as f:
        f.write(PDK)
    manifest = ["deck\tprobes\twhat it asks"]
    expected = []
    for name, kind, what, body, probes in DECKS:
        path = os.path.join(decks_dir, f"{name}.sp")
        with open(path, "w", newline="\n") as f:
            f.write(deck_text(name, kind, what, body, probes))
        answers = netlist_db(resolve, path, probes)
        rows = []
        for i, (p, a) in enumerate(zip(probes, answers)):
            # the card sets the name once, and netlist-db's value (if any) is
            # the value of one of the rules of the probe
            if p["check"] == "value" and "values" in a:
                assert len(a["values"]) == 1, (name, p["path"], a)
                v = a["values"][0]
                if v is not None:
                    assert any(close(v, h, kind) for _, h in p["hypotheses"]), (name, p["path"], v)
            rows.append({"measure": f"p{i}", **p, "netlist_db": a})
        expected.append({"deck": name, "kind": kind, "what": what,
                         "control": CONTROLS[kind]["value"], "probes": rows})
        manifest.append(f"{name}\t{len(probes)}\t{what}")
    with open(os.path.join(out, "expected.json"), "w", newline="\n") as f:
        json.dump({"decks": expected}, f, indent=1)
        f.write("\n")
    with open(os.path.join(out, "MANIFEST.tsv"), "w", newline="\n") as f:
        f.write("\n".join(manifest) + "\n")
    for script, text in (("run_cards.sh", RUN_SH), ("run_cards.ps1", RUN_PS1)):
        with open(os.path.join(out, script), "w", newline="\n") as f:
            f.write(text)
    os.chmod(os.path.join(out, "run_cards.sh"), 0o755)
    with open(os.path.join(out, "VERSION"), "w", newline="\n") as f:
        f.write(f"netlist-db {commit} (the answers in expected.json)\n")
    print(f"{len(DECKS)} decks in {out}")


if __name__ == "__main__":
    main()
