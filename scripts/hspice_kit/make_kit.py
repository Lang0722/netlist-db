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
  pocket, lateral doping, and body effect: with its drain above its source
  a PMOS swaps them, which biases its body) are zeroed: LV9 tells the bin
  chosen;
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
ZEROED = "k1=0 k2=0 k3=0 dvt0=0 dvt0w=0 eta0=0 etab=0 dvtp0=0 lpe0=0 lpeb=0 lint=0 wint=0 xl=0 xw=0"
# HSPICE stops on a binned model without its four limits ("model name X ...
# contains a character period (.). you must set all four parameters"); those
# a deck leaves out are given here, wide enough not to limit
WIDE = (("lmin", "1e-10"), ("lmax", "1"), ("wmin", "1e-10"), ("wmax", "1"))
# ends a `.model` line whose limits are the question: none are added
AS_IS = "$ as is"


def bin_number(name):
    m = re.fullmatch(r"(.+)\.(\d+)", name.lower())
    return int(m.group(2)) if m else 0


def prepare_models(text):
    """Every model: level 54 (if the deck gives no level), its VTH0 telling
    its bin, the threshold terms zeroed (but those the deck sets), and the
    four limits on a name with a dot"""
    out = []
    for line in text.splitlines():
        m = re.match(r"(?i)^\.model\s+(\S+)\s+(\S+)(.*)$", line)
        if m:
            name, kind, rest = m.groups()
            as_is = AS_IS in rest
            rest = rest.replace(AS_IS, "").rstrip()
            has = lambda key: re.search(rf"(?i)\b{key}\s*=", rest)  # noqa: E731
            sign = -1 if kind.lower().startswith("p") else 1
            vth = sign * (VTH_BASE + VTH_STEP * bin_number(name))
            if not has("level"):
                rest = " level=54" + rest
            if "." in name and not as_is:
                rest += "".join(f" {k}={v}" for k, v in WIDE if not has(k))
            zeroed = [kv for kv in ZEROED.split() if not has(kv.split("=")[0])]
            line = " ".join([f".model {name} {kind}{rest}", f"vth0={vth:.3f}"] + zeroed)
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
    # the transistors HSPICE stops on go to the question decks
    for name in ("M3", "M9", "M10"):
        text = re.sub(rf"(?m)^{name} .*\n", "", text)
    out.append(("bins", text, "overlapping bins, a model and bins of one name, case, non-numeric suffixes, "
                "fingers, edges through float noise, local bins"))
    out.append(("scale", stext, ".option scale=1e-6"))
    lib, _ = torture.library_case()
    lib = re.sub(r"(?m)^\.end\s*$", "X1 d g 0 inv\nX2 d g 0 top2\nX3 d g 0 later\n.end", lib)
    out.append(("library", lib, "a nested subckt hiding a top-level one, case, a subckt defined after use"))
    q = [
        ("q_gap", ".model gp.1 nmos level=54 lmin=1e-7 lmax=2e-7\n.model gp.2 nmos level=54 lmin=3e-7 lmax=4e-7\n"
                  "M1 d g 0 0 gp l=0.25u w=1u\n", "L between two bins: no bin, HSPICE stops"),
        ("q_dotted", ".model n.ch.1 nmos level=54 lmin=0 lmax=1\nM1 d g 0 0 n l=0.5u w=1u\n",
         "a device of model `n` with only `n.ch.1`: no bin of n, HSPICE stops"),
        ("q_nonnumeric", ".model nm.1 nmos level=54 lmin=1e-7 lmax=1e-6\n.model nm.mc nmos level=54\n"
                         ".model nm.1a nmos level=54\nM1 d g 0 0 nm l=5u w=1u\n",
         "nm.mc and nm.1a are bins of nm too: L holds in the first of them"),
        ("q_localhide", ".model loc.1 nmos level=54 lmin=0 lmax=1e-6\n.model loc.2 nmos level=54 lmin=1e-6 lmax=1\n"
                        ".subckt hide d g s\n.model loc.1 nmos level=54 lmin=0 lmax=5e-7\n"
                        "M1 d g s s loc l=0.8u w=1u\n.ends hide\nX1 d g 0 hide\n",
         "a local bin that does not hold hides nothing: a global bin"),
        ("q_nf_default", ".model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6\n"
                         ".model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3\n"
                         "M1 d g 0 0 a w=4u l=1u nf=8\n",
         "W=4u NF=8, no WNFLAG: the width per finger, a.1"),
        ("q_wnflag_model", ".model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6 wnflag=1\n"
                           ".model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3 wnflag=1\n"
                           "M1 d g 0 0 a w=4u l=1u nf=8\n",
         "W=4u NF=8, WNFLAG=1 in the models changes nothing: a.1"),
        ("q_wnflag_instance", ".model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6\n"
                              ".model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3\n"
                              "M1 d g 0 0 a w=4u l=1u nf=8 wnflag=1\n",
         "W=4u NF=8, wnflag=1 on the instance: a.1"),
        ("q_wnflag_option", ".option wnflag=1\n.model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6\n"
                            ".model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3\n"
                            "M1 d g 0 0 a w=4u l=1u nf=8\n",
         "W=4u NF=8, .option wnflag=1: a.1"),
        ("q_edge", ".model e.1 nmos level=54 lmin=1e-8 lmax=1e-6\n.model e.2 nmos level=54 lmin=1e-6 lmax=1e-3\n"
                   "M1 d g 0 0 e l=0.9995u w=1u\nM2 d g 0 0 e l=1u w=1u\nM3 d g 0 0 e l='0.1u*10' w=1u\n",
         "L 0.5 nm below an edge: e.1; on it: e.2; on it through float noise: e.2"),
    ]
    a_models = (".model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6{0}\n"
                ".model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3{0}\n")
    d_models = "".join(f".model d.{n} nmos level=54 lmin={lo} lmax={lh} wmin={wo} wmax={wh}\n"
                       for n, lo, lh, wo, wh in ((1, 1e-8, 1e-5, 1e-8, 1e-5), (2, 1e-8, 1e-5, 1e-5, 1),
                                                 (3, 1e-5, 1, 1e-8, 1e-5), (4, 1e-5, 1, 1e-5, 1)))
    e_models = (".model e.1 nmos level=54 lmin=1e-8 lmax=1e-6 wmin=1e-8 wmax=1e-6\n"
                ".model e.2 nmos level=54 lmin=1e-6 lmax=1e-3 wmin=1e-8 wmax=1e-6\n"
                ".model e.3 nmos level=54 lmin=1e-8 lmax=1e-6 wmin=1e-6 wmax=1e-3\n"
                ".model e.4 nmos level=54 lmin=1e-6 lmax=1e-3 wmin=1e-6 wmax=1e-3\n")
    offsets = ("-1e-6", "-1e-8", "-2e-9", "-5e-10", "-1e-12", "-1e-15", "1e-15", "1e-12", "5e-10", "2e-9", "1e-8")
    s_models = (".model s.1 nmos level=54 lmin=0.1 lmax=1 wmin=0.1 wmax=100\n"
                ".model s.2 nmos level=54 lmin=1 lmax=100 wmin=0.1 wmax=100\n")
    q += [
        ("q_wnflag0_model", a_models.format(" wnflag=0") + "M1 d g 0 0 a w=4u l=1u nf=8\n",
         "WNFLAG=0 in the models changes nothing: W/NF is compared, a.1"),
        ("q_wnflag0_instance", a_models.format("") + "M1 d g 0 0 a w=4u l=1u nf=8 wnflag=0\n",
         "wnflag=0 on the instance: the total width, a.2"),
        ("q_wnflag0_option", ".option wnflag=0\n" + a_models.format("") + "M1 d g 0 0 a w=4u l=1u nf=8\n",
         ".option wnflag=0: the total width, a.2"),
        ("q_m", a_models.format("") + "M1 d g 0 0 a w=4u l=1u m=8\n", "M=8: not fingers (a.2)"),
        ("q_nf_m", a_models.format("") + "M1 d g 0 0 a w=4u l=1u nf=8 m=2\n", "NF=8 M=2: W/NF (a.1)"),
        ("q_defaults", d_models + "M1 d g 0 0 d\n", "no L, W: DEFL, DEFW are 100u (d.4)"),
        ("q_defl", ".option defl=1u defw=2e-5\n" + d_models + "M1 d g 0 0 d\n", ".option defl, defw (d.2)"),
        ("q_scalm", ".option scalm=1e-6\n" + s_models + "M1 d g 0 0 s l=0.5u w=2u\nM2 d g 0 0 s l=2u w=2u\n",
         "limits times SCALM (s.1, s.2)"),
        ("q_scale_scalm", ".option scale=1e-6 scalm=1e-6\n" + s_models + "M1 d g 0 0 s l=0.5 w=2\nM2 d g 0 0 s l=2 w=2\n",
         "L, W times SCALE; limits times SCALM (s.1, s.2)"),
        # XL, LINT: L+XL-2*LINT stays L, so that LV1 still reads the drawn L
        ("q_xl", ".model x.1 nmos level=54 lmin=1e-8 lmax=1e-6 wmin=1e-8 wmax=1e-3 xl=1e-7 lint=5e-8\n"
                 ".model x.2 nmos level=54 lmin=1e-6 lmax=1e-3 wmin=1e-8 wmax=1e-3 xl=1e-7 lint=5e-8\n"
                 "M1 d g 0 0 x l=0.95u w=1u\n", "XL=100n, L=0.95u: by L x.1, by L+XL x.2"),
        ("q_xw", ".model x.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6 xw=1e-7 wint=5e-8\n"
                 ".model x.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3 xw=1e-7 wint=5e-8\n"
                 "M1 d g 0 0 x l=1u w=0.95u\n", "XW=100n, W=0.95u: by W x.1, by W+XW x.2"),
        ("q_edge_tol_l", e_models + "".join(f"M{i} d g 0 0 e l='1u*(1+({o}))' w=5u\n" for i, o in enumerate(offsets)),
         "L around a limit by these offsets: " + " ".join(offsets)),
        ("q_edge_tol_w", e_models + "".join(f"M{i} d g 0 0 e l=5u w='1u*(1+({o}))'\n" for i, o in enumerate(offsets)),
         "W around a limit by these offsets: " + " ".join(offsets)),
        ("q_exact_out", ".model ex nmos level=54 lmin=1e-6 lmax=2e-6 wmin=1e-6 wmax=2e-6\nM1 d g 0 0 ex l=5u w=5u\n",
         "a model of that name, L and W outside its limits: its limits do not matter (ex)"),
        ("q_bin_direct", e_models + "M1 d g 0 0 e.4 l=0.5u w=0.5u\n",
         "a bin named on the instance line, L and W outside its limits: HSPICE stops"),
        ("q_exact_and_bin", ".model eb nmos level=54\n.model eb.1 nmos level=54\nM1 d g 0 0 eb l=1u w=1u\n",
         "a model of that name and a bin of it: the model"),
        ("q_mc_as_bin", ".model nm.mc nmos level=54\nM1 d g 0 0 nm l=1u w=1u\n",
         "only nm.mc, with its four limits: a bin of nm"),
        ("q_mc_direct", ".model nm.mc nmos level=54 $ as is\nM1 d g 0 0 nm.mc l=1u w=1u\n",
         "nm.mc named on the instance line, no limits: HSPICE stops"),
        ("q_mc_unused", ".model nm.mc nmos level=54 $ as is\n.model ok nmos level=54\nM1 d g 0 0 ok l=1u w=1u\n",
         "an unused nm.mc without limits: HSPICE does not stop on it"),
        ("q_bin_nolimits_unused", ".model nb.1 nmos level=54 lmin=1e-8 lmax=1 $ as is\n.model ok nmos level=54\n"
                                  "M1 d g 0 0 ok l=1u w=1u\n", "an unused nb.1 with two limits: HSPICE does not stop on it"),
        ("q_dot_two", ".model n.ch.1 nmos level=54\nM1 d g 0 0 n.ch l=1u w=1u\n", "n.ch.1 is no bin of n.ch: HSPICE stops"),
        ("q_model_dynamic", ".model dy.1 nmos level=54\n.subckt leaf d g s\nM1 d g s s dy l=1u w=1u\n.ends leaf\n"
                            ".subckt host d g s\n.model dy.2 nmos level=54\nX1 d g s leaf\n.ends host\n"
                            "X1 d g 0 leaf\nX2 d g 0 host\n",
         "a bin in the subckt of an instance above, not around, the MOSFET's definition: dy.2"),
        ("q_subckt_dynamic", ".model nv nmos level=54\n.subckt cell d g s\nM1 d g s s nv l=1u w=1u\n.ends cell\n"
                             ".subckt leaf d g s\nX1 d g s cell\n.ends leaf\n"
                             ".subckt host d g s\n.subckt cell d g s\nM1 d g s s nv l=1u w=2u\n.ends cell\n"
                             "X1 d g s leaf\n.ends host\nX1 d g 0 leaf\nX2 d g 0 host\n",
         "X2.X1.X1: the nested cell of host, on its instance path"),
        ("q_positional", ".model a nmos level=54\nM1 d g 0 0 a 2u 1u\n", "two numbers after the model name: L, then W"),
    ]
    # where the limits of a bin that read a parameter are evaluated
    tg = (".param lb=2e-7\n.model tg.1 nmos level=54 lmin=1e-8 lmax='lb'\n"
          ".model tg.2 nmos level=54 lmin='lb' lmax=1e-3\n")
    s1 = ".subckt s1 d g s lb=1e-6\nM1 d g s s tg l=5e-7 w=1u\n.ends s1\n"
    s2 = ".subckt s2 d g s lb=1e-7\nM1 d g s s tg l=1.5e-7 w=1u\n.ends s2\n"
    s3 = ".subckt s3 d g s lb=1e-7\nM1 d g s s tg l=5e-7 w=1u\n.ends s3\n"
    local = ".option parhier=local\n"
    mac = (".subckt mac d g s lb=1e-6\n.model lo.1 nmos level=54 lmin=1e-8 lmax='lb'\n"
           ".model lo.2 nmos level=54 lmin='lb' lmax=1e-3\nM1 d g s s lo l=5e-7 w=1u\n.ends mac\n"
           "X1 d g 0 mac\nX2 d g 0 mac lb=1e-7\n")
    nested = (".subckt outer d g s lb=1e-6\n.model lo.1 nmos level=54 lmin=1e-8 lmax='lb'\n"
              ".model lo.2 nmos level=54 lmin='lb' lmax=1e-3\n.subckt inner d g s lb=1e-7\n"
              "M1 d g s s lo l=5e-7 w=1u\n.ends inner\nX1 d g s inner\n.ends outer\nX1 d g 0 outer\n")
    q += [
        ("q_limits_s1_s2", local + tg + s1 + s2 + "X1 d g 0 s1\nX2 d g 0 s2\n",
         "known gap, lb=1u, then 0.1u: .1 .1; the first MOSFET of a name reads its own lb"),
        ("q_limits_s2_s1", local + tg + s1 + s2 + "X1 d g 0 s2\nX2 d g 0 s1\n",
         "known gap, lb=0.1u, then 1u: .2 .2; the first MOSFET of a name reads its own lb"),
        ("q_limits_s1_s3", local + tg + s1 + s3 + "X1 d g 0 s1\nX2 d g 0 s3\n",
         "known gap, one L, W; lb=1u, then 0.1u: .1 .2; the first MOSFET of a name reads its own lb"),
        ("q_limits_bfs", local + tg + s1 + s2 + ".subckt wrap d g s\nX1 d g s s1\n.ends wrap\n"
                                                "X1 d g 0 wrap\nX2 d g 0 s2\n",
         "known gap, the first by the text is deeper: .2 .2, the first is breadth-first"),
        ("q_limits_notop_local", local + ".model tg.1 nmos level=54 lmin=1e-8 lmax='lb'\n"
                                         ".model tg.2 nmos level=54 lmin='lb' lmax=1e-3\n" + s1 + "X1 d g 0 s1\n",
         "limits of a top-level model reading a name only the MOSFET's subckt defines (local)"),
        ("q_limits_mac_local", local + mac, "a model in a subckt, two instances, lb=1u and 0.1u (local)"),
        ("q_limits_mac_global", ".option parhier=global\n" + mac, "a model in a subckt, two instances (global)"),
        ("q_limits_nested_local", local + nested, "a model in outer read by a MOSFET in inner, nested (local)"),
        ("q_limits_nested_global", ".option parhier=global\n" + nested, "the same (global)"),
        ("q_bin_direct_in", e_models + "M1 d g 0 0 e.4 l=5u w=5u\n", "a bin named on the instance line, in its limits"),
        ("q_two_dots_base", ".model n.ch.1 nmos level=54\nM1 d g 0 0 n l=1u w=1u\n", "n.ch.1 as a bin of n"),
        ("q_two_dots_direct", ".model n.ch.1 nmos level=54\nM1 d g 0 0 n.ch.1 l=1u w=1u\n", "n.ch.1 named directly"),
        ("q_1a_as_bin", ".model nm.1a nmos level=54\nM1 d g 0 0 nm l=1u w=1u\n", "only nm.1a: a bin of nm"),
        ("q_incomplete_after", ".model nm.1 nmos level=54\n.model nm.mc nmos level=54 $ as is\n"
                               "M1 d g 0 0 nm l=1u w=1u\n", "nm.1 holds; nm.mc after it has no limits"),
        ("q_incomplete_before", ".model nm.mc nmos level=54 $ as is\n.model nm.1 nmos level=54\n"
                                "M1 d g 0 0 nm l=1u w=1u\n", "nm.mc without limits, before nm.1, which holds"),
        ("q_local_exact", ".model x.1 nmos level=54\n.subckt h d g s\n.model x nmos level=54\n"
                          "M1 d g s s x l=1u w=1u\n.ends h\nX1 d g 0 h\n", "local x, global bin x.1"),
        ("q_localbin_globalexact", ".model x nmos level=54\n.subckt h d g s\n.model x.1 nmos level=54\n"
                                   "M1 d g s s x l=1u w=1u\n.ends h\nX1 d g 0 h\n", "local bin x.1 holds, global x"),
        ("q_localbin_out_globalexact", ".model x nmos level=54\n.subckt h d g s\n.model x.1 nmos level=54 lmax=5e-7\n"
                                       "M1 d g s s x l=1u w=1u\n.ends h\nX1 d g 0 h\n",
         "local bin x.1 does not hold, global x"),
        ("q_order", ".model ov.2 nmos level=54\n.model ov.1 nmos level=54\nM1 d g 0 0 ov l=1u w=1u\n",
         "two bins hold: the first in the text (ov.2)"),
        ("q_wnflag_i1_o0", ".option wnflag=0\n" + a_models.format("") + "M1 d g 0 0 a w=4u l=1u nf=8 wnflag=1\n",
         "instance wnflag=1 against .option wnflag=0"),
        ("q_wnflag_i0_o1", ".option wnflag=1\n" + a_models.format("") + "M1 d g 0 0 a w=4u l=1u nf=8 wnflag=0\n",
         "instance wnflag=0 against .option wnflag=1"),
    ]
    fine = ("-1e-11", "-3e-12", "-1e-12", "-3e-13", "-1e-13", "-3e-14", "-1e-14", "-3e-15", "-1e-15")
    tol = ""
    for k, limit in enumerate((1e-8, 1e-7, 1e-6, 1e-5, 1e-4)):
        tol += (f".model t{k}.1 nmos level=54 lmin={limit / 10:g} lmax={limit:g}\n"
                f".model t{k}.2 nmos level=54 lmin={limit:g} lmax={limit * 10:g}\n")
        tol += "".join(f"M{k}{i} d g 0 0 t{k} l='{limit:g}*(1+({o}))' w=1u\n" for i, o in enumerate(fine))
    q.append(("q_edge_tol_fine", tol, "L just below a limit, at five sizes of limit: the tolerance is relative"))
    scope_deck = (".param lb=2e-7\n"
                  ".model tg.1 nmos level=54 lmin=1e-8 lmax='lb'\n.model tg.2 nmos level=54 lmin='lb' lmax=1e-3\n"
                  ".subckt outer d g s lb=1e-6\n"
                  ".model lo.1 nmos level=54 lmin=1e-8 lmax='lb'\n.model lo.2 nmos level=54 lmin='lb' lmax=1e-3\n"
                  ".subckt inner d g s lb=1e-7\n"
                  "M1 d g s s lo l=5e-7 w=1u\nM2 d g s s tg l=5e-7 w=1u\n.ends inner\n"
                  "X1 d g s inner\nM3 d g s s tg l=5e-7 w=1u\nM4 d g s s lo l=5e-7 w=1u\n.ends outer\n"
                  "X1 d g 0 outer\nM5 d g 0 0 tg l=5e-7 w=1u\n")
    override_deck = (".param w=4 q=2\n.model nv nmos level=54\n"
                     ".subckt cc d g s w=2 l=1\nM1 d g s s nv w='w*1u' l='l*1u'\n.ends cc\n"
                     ".subckt cb d g s\n.param v=2 w=2\nM1 d g s s nv w='w*1u' l='v*1u'\n.ends cb\n"
                     ".subckt cu d g s\nM1 d g s s nv w='q*1u' l=1u\n.ends cu\n"
                     ".subckt cm d g s w=3\nX1 d g s cc w='w*2'\n.ends cm\n"
                     "X1 d g 0 cc l='w*3'\nX2 d g 0 cc w=5 l='w*3'\nX3 d g 0 cc w='w*3'\n"
                     "X4 d g 0 cc w='w*2' l='w*3'\nX5 d g 0 cu q=5\nX6 d g 0 cb w=7\nX7 d g 0 cb w='w*3'\nX8 d g 0 cm\n")
    func_deck = (".param f(x)='x*2' k=2 h(x)='x*k'\n.model nv nmos level=54\n"
                 ".subckt cf d g s\n.param f(x)='x*3'\nM1 d g s s nv l=1u w='f(1)*1u'\n.ends cf\n"
                 ".subckt ck d g s k=7\nM1 d g s s nv l=1u w='h(1)*1u'\n.ends ck\n"
                 ".subckt cl d g s\n.param g(x)='x*kk' kk=2\nX1 d g s cn\n.ends cl\n"
                 ".subckt cn d g s kk=9\nM1 d g s s nv l=1u w='g(1)*1u'\n.ends cn\n"
                 "X1 d g 0 cf\nX2 d g 0 ck\nX3 d g 0 cl\n")
    for mode in ("local", "global"):
        option = f".option parhier={mode}\n"
        q += [(f"q_model_scope_{mode}", option + scope_deck,
               f"limits reading lb, defined at 3 levels ({mode}): where the model is, or where the MOSFET is?"),
              (f"q_override_{mode}", option + override_deck,
               f"values passed: computed in the new instance's scope, their own name above it ({mode})"),
              (f"q_func_{mode}", option + func_deck,
               f".param functions: which definition, and where a name in the body is read ({mode})")]
    q.append(("q_model_scope_notop", ".model tg.1 nmos level=54 lmin=1e-8 lmax='lb'\n"
                                     ".model tg.2 nmos level=54 lmin='lb' lmax=1e-3\n"
                                     ".subckt s1 d g s lb=1e-6\nM1 d g s s tg l=5e-7 w=1u\n.ends s1\nX1 d g 0 s1\n",
              "limits of a top-level model reading a name only the MOSFET's subckt defines (global)"))
    q.append(("q_override_self_notop", ".model nv nmos level=54\n.subckt cc d g s w=2\nM1 d g s s nv w='w*1u' l=1u\n"
                                       ".ends cc\nX1 d g 0 cc w='w*3'\n",
              "a value passed that reads its own name, defined nowhere above (HSPICE: cannot redefine)"))
    # a value passed, and a .param of the new instance, that read each other
    nv = ".model nv nmos level=54\n"
    show = "M1 d g s s nv l='a*1u' w='c*1u'\n"
    cycles = [
        ("pass_param", ".param a=1 c=2\n", ".subckt s d g s\n.param c='a*3'\n" + show + ".ends s\n", "X1 d g 0 s a='c'\n",
         "a='c' passed, .param c='a*3' in the new instance; both defined above"),
        ("pass_default", ".param a=1 c=2\n", ".subckt s d g s a=5\n.param c='a*3'\n" + show + ".ends s\n", "X1 d g 0 s a='c'\n",
         "the same, with a default a=5 on the .subckt line"),
        ("no_above", ".param c=2\n", ".subckt s d g s\n.param c='a*3'\n" + show + ".ends s\n", "X1 d g 0 s a='c'\n",
         "the same, with no `a` above the new instance"),
        ("three", ".param a=1 b=2 c=3\n",
         ".subckt s d g s\n.param b='c*2' c='a*3'\nM1 d g s s nv l='a*1u' w='b*1u'\n.ends s\n", "X1 d g 0 s a='b'\n",
         "a='b' passed, .param b='c*2' c='a*3' in the new instance"),
        ("swap", ".param a=1 c=2\n", ".subckt s d g s\n" + show + ".ends s\n", "X1 d g 0 s a='c' c='a'\n",
         "two values passed that read each other"),
        ("swap_rev", ".param a=1 c=2\n", ".subckt s d g s\n" + show + ".ends s\n", "X1 d g 0 s c='a' a='c'\n",
         "the same, the other order on the line"),
        ("plain", ".param a=1 c=2\n", ".subckt s d g s\n.param a='c*2' c='a*3'\n" + show + ".ends s\n", "X1 d g 0 s\n",
         "two .param of a subckt that read each other, nothing passed"),
        ("plain_top", ".param a='c*2' c='a*3'\n", "", "M1 d g 0 0 nv l='a*1u' w='c*1u'\n",
         "two top-level .param that read each other"),
        ("nothing_above", "", ".subckt s d g s\n.param c='a*3'\n" + show + ".ends s\n", "X1 d g 0 s a='c'\n",
         "a='c' passed, .param c='a*3' in the new instance; neither name above it"),
        ("chain_passed", ".param a=1 b=2 c=3\n", ".subckt s d g s\n" + show + ".ends s\n", "X1 d g 0 s a='b' b='c' c='a'\n",
         "three values passed that read each other in a ring"),
        ("self_order", ".param a=4\n", ".subckt s d g s c=1 a=2\nM1 d g s s nv l='a*1u' w='c*1u'\n.ends s\n",
         "X1 d g 0 s c='a*3' a='a*2'\n", "c reads a, passed later on the line, which reads its own name"),
        ("body_self", ".param a=1 c=2\n", ".subckt s d g s\n.param c='c*2'\n" + show + ".ends s\n", "X1 d g 0 s\n",
         "a .param of a subckt that reads its own name, defined above"),
        ("default_self", ".param a=1 c=2\n", ".subckt s d g s c='c*2'\n" + show + ".ends s\n", "X1 d g 0 s\n",
         "a default of a subckt that reads its own name, defined above"),
        # the other names of such a text: read in the new instance, or above it?
        ("text_pass_self", ".param a=1 c=2 k=2\n", ".subckt s d g s k=3\nM1 d g s s nv l='k*1u' w='c*1u'\n.ends s\n",
         "X1 d g 0 s c='c*k'\n", "a value passed that reads its own name, and k: k=2 above it, k=3 in the new instance"),
        ("text_pass_loop", ".param a=1 c=2 k=2\n", ".subckt s d g s k=3\n.param c='a*3'\nM1 d g s s nv l='a*1u' w='k*1u'\n.ends s\n",
         "X1 d g 0 s a='c*k'\n", "a='c*k' passed, .param c='a*3': k=2 above it, k=3 in the new instance"),
        ("text_pass_sibling", ".param a=1 c=2 k=2\n", ".subckt s d g s\n.param c='a*3'\nM1 d g s s nv l='a*1u' w='k*1u'\n.ends s\n",
         "X1 d g 0 s k=7 a='c*k'\n", "k=7 a='c*k' passed, .param c='a*3': k=2 above it, k=7 on the line"),
        ("text_body_self", ".param a=1 c=2 k=2\n", ".subckt s d g s\n.param k=3 c='c*k'\nM1 d g s s nv l='k*1u' w='c*1u'\n.ends s\n",
         "X1 d g 0 s\n", "a .param that reads its own name, and k: k=2 above it, k=3 at its level"),
        ("text_default_self", ".param a=1 c=2 k=2\n", ".subckt s d g s k=3 c='c*k'\nM1 d g s s nv l='k*1u' w='c*1u'\n.ends s\n",
         "X1 d g 0 s\n", "a default that reads its own name, and k: k=2 above it, k=3 at its level"),
        ("text_body_self_pass", ".param a=1 c=2 k=2\n", ".subckt s d g s\n.param c='c*k'\nM1 d g s s nv l='k*1u' w='c*1u'\n.ends s\n",
         "X1 d g 0 s k=7\n", "a .param that reads its own name, and k: k=2 above it, k=7 passed"),
    ]
    for name, top, sub, inst, what in cycles:
        for mode in ("local", "global"):
            q.append((f"q_cycle_{name}_{mode}", f".option parhier={mode}\n{top}{nv}{sub}{inst}", f"{what} ({mode})"))
    a_model = ".param pl=2u pw=1u\n.model a nmos level=54\n"
    q += [("q_positional_expr", a_model + "M1 d g 0 0 a 'pl' 'pw'\n", "quoted expressions after the model name"),
          ("q_positional_names", a_model + "M1 d g 0 0 a pl pw\n", "names of parameters after the model name"),
          ("q_positional_mix", a_model + "M1 d g 0 0 a 2u pw\nM2 d g 0 0 a pl 1u\nM3 d g 0 a 'pl' 1u\nM4 d g 0 0 a pl\n",
           "numbers, names and quoted expressions after the model name"),
          ("q_positional_names_3", a_model + "M1 d g 0 a pl pw\n",
           "3 nodes, then names after the model name: netlist-db reads a 4th node and the model `pl`")]
    q.append(("q_limits_same_scope",
              local + tg + ".subckt s1 d g s lb=1e-6\nM1 d g s s tg l=5e-7 w=1u\nM2 d g s s tg l=5e-7 w=1u\n.ends s1\n"
              ".subckt s2 d g s lb=1e-6\nM1 d g s s tg l=5e-7 w=1u\n.ends s2\nX1 d g 0 s1\nX2 d g 0 s2\n",
              "known gap: the first MOSFET of a name only, or all of its instance, read their own lb?"))
    # expressions and numbers, read through L
    exprs = ("(2**3**2)*1n", "(2^3^2)*1n", "(2**-3**2)*1u", "(1+0**0)*1u", "(1+0**-1)*1u", "(1+0**0.5)*1u",
             "(3+sqrt(-4))*1u", "(5+log(-100))*1u", "(3+log10(-100))*1u", "(10+(-2)**3)*1u", "(-2**2+10)*1u",
             "(2**0.5)*1u", "pow(2,2.7)*1u", "(3+pwr(-2,0.5))*1u")
    units = ("1me*1e-12", "1meter*1e-12", "1mi", "1mil", "1min*1u", "1hr*1n", "1day*1n", "1yr*1p", "20db*1u",
             "1z*1e15", "1e-3k*1m", "1x*1e-12", "10pF*1e5", "1mohm*1m")
    words = ("1minute*1n", "1mile", "1hrs*1n", "1days*1n", "1y*1e18", "1years*1e18", "20dbm*1u", "1zz*1e15",
             "1megs*1e-12")
    mosfets = lambda texts: "".join(f"M{i} d g 0 0 nv l='{t}' w=1u\n" for i, t in enumerate(texts))  # noqa: E731
    q += [("q_expr", nv + mosfets(exprs), "** and ^ associate to the left, a zero base, roots and logs of negatives"),
          ("q_expr_negative_base", nv + mosfets(("(3+(-2)**0.5)*1u", "(10+(-2)**1.5)*1u", "(10+(-2)**2.5)*1u")),
           "a negative base, a fractional exponent"),
          ("q_units", nv + mosfets(units) + "Mq d g 0 0 nv l=2umos w=1u\n", "scale factors and time units"),
          ("q_units_words", nv + mosfets(words), "time units and scale factors with more letters")]
    # how the option is read: L=1u under parhier=global, 2u under local
    deep = (".param p=1\n.model a nmos level=54\n.subckt c d g s p=2\n"
            "M1 d g s s a l='p*1u' w=1u\n.ends c\nX1 d g 0 c\n")
    q += [("q_parhier_none", deep, "no option: global, L=1u"),
          ("q_parhier_plural", ".options parhier=local\n" + deep, "`.options`: local, L=2u"),
          ("q_parhier_upper", ".OPTION PARHIER=LOCAL\n" + deep, "capital letters: local, L=2u"),
          ("q_parhier_spaces", ".option parhier = local\n" + deep, "spaces around `=`: local, L=2u"),
          ("q_parhier_many", ".option post=0 parhier=local ingold=2\n" + deep, "among other options: local, L=2u"),
          ("q_parhier_plus", ".option post=0\n+ parhier=local\n" + deep, "on a `+` line: local, L=2u"),
          ("q_parhier_quoted", ".option parhier='local'\n" + deep, "quoted: local, L=2u"),
          ("q_parhier_after", deep + ".option parhier=local\n", "after the circuit: local, L=2u"),
          ("q_parhier_last_global", ".option parhier=local\n.option parhier=global\n" + deep,
           "two lines, the last one wins: global, L=1u"),
          ("q_parhier_last_local", ".option parhier=global\n.option parhier=local\n" + deep,
           "two lines, the last one wins: local, L=2u"),
          ("q_parhier_in_subckt", deep.replace("p=2\n", "p=2\n.option parhier=local\n"),
           "inside a subckt: netlist-db does not read it (global, L=1u)")]
    for name, body, what in q:
        out.append((name, f"* {name}\n{body}.end\n", what))
    return out


def random_decks(n):
    """Decks of pgen.py that HSPICE can run: every MOSFET has a model, NF
    whole, sizes below 1 mm"""
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
            rows = listing(deck)
            ok = (rows and len(rows) <= 400
                  and all(not r[5].startswith("error") for r in rows)
                  and all(float(r[4]) == int(float(r[4])) for r in rows)
                  and all(float(r[2]) < 1e-3 and float(r[3]) < 1e-3 for r in rows))
            if ok:
                found += 1
                what = {"ngspice": "random, parhier=local, as ngspice reads HSPICE",
                        "local": "random, parhier=local: nested subckts, .param functions, WNFLAG",
                        "global": "random, parhier=global: nested subckts, .param functions, WNFLAG"}[kind]
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
