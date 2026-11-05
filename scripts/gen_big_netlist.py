#!/usr/bin/env python3
"""Generate a large hierarchical HSPICE deck, to test a netlist tool at scale.

Writes into OUTDIR:
  chip.sp       the netlist: about 200,000 lines and 30 MB, whose hierarchy
                holds 300,000,000 MOSFETs once flattened (with the defaults)
  stdcell.sp    a standard-cell library (.include'd by chip.sp)
  analog.inc    analog cells, with models of their own (.include'd)
  models.l      a binned model library: corners TT SS FF SF FS, each loading
                the sections CORE SRAM IO RF (.lib 'models.l' TT)
  expected.tsv  how many MOSFETs use each model, computed here from the
                generator's own numbers (not by reading the netlist back);
                expected_<SUBCKT>.tsv the same for some subckts on their own

Nearly every MOSFET names a family of the library (nch_svt, pch_lvt, ...)
whose model is the bin `name.N` that holds its L and W (LMIN <= L < LMAX,
WMIN <= W/NF < WMAX; the models' WNFLAG changes nothing in HSPICE). A few use
other models: exact-named ones from analog.inc, the local bins of an RF
macro model, a model local to a subckt. A few have no bin, on purpose.

The deck uses .option parhier=local (or global, --parhier global), and a
top-level .param that a subckt default must win over under the local rule.
With --instance-ids, every instance of a block, unit, core or cluster passes
a parameter that nothing reads, unique to it (the counts do not change).

usage: gen_big_netlist.py OUTDIR [--seed N] [--clusters N] [--mos N]
           [--small] [--block-scale F] [--parhier local|global] [--instance-ids]

Check a tool's counts with check_model_usage.py, e.g. for netlist-db:
  cargo run --release --example model_select -- --summary OUTDIR/chip.sp > usage.txt
  scripts/check_model_usage.py OUTDIR/expected.tsv usage.txt
"""

import argparse
import functools
import itertools
import os
import random
from collections import Counter
from fractions import Fraction

# ---------------------------------------------------------------------------
# The binned model library (lengths in nm)
# ---------------------------------------------------------------------------

L_CORE = [30, 36, 50, 80, 150, 500, 2000, 20000]
W_CORE = [80, 120, 200, 400, 1000, 3000, 100000]
L_SRAM = [30, 40, 60]
W_SRAM = [50, 100, 200]
L_IO = [150, 300, 1000, 20000]
W_IO = [300, 1000, 10000, 100000]

# family: (type, L edges, W edges, WNFLAG)
FAMILIES = {
    "nch_svt": ("nmos", L_CORE, W_CORE, 0),
    "pch_svt": ("pmos", L_CORE, W_CORE, 0),
    "nch_lvt": ("nmos", L_CORE, W_CORE, 1),
    "pch_lvt": ("pmos", L_CORE, W_CORE, 1),
    "nch_hvt": ("nmos", L_CORE, W_CORE, 0),
    "pch_hvt": ("pmos", L_CORE, W_CORE, 0),
    "nch_ulvt": ("nmos", L_CORE, W_CORE, 1),
    "pch_ulvt": ("pmos", L_CORE, W_CORE, 1),
    "nchpd_sr": ("nmos", L_SRAM, W_SRAM, 0),
    "nchpg_sr": ("nmos", L_SRAM, W_SRAM, 0),
    "pchpu_sr": ("pmos", L_SRAM, W_SRAM, 0),
    "nch_18": ("nmos", L_IO, W_IO, 0),
    "pch_18": ("pmos", L_IO, W_IO, 0),
}
SECTION = {f: "CORE" for f in FAMILIES}
SECTION.update({"nchpd_sr": "SRAM", "nchpg_sr": "SRAM", "pchpu_sr": "SRAM"})
SECTION.update({"nch_18": "IO", "pch_18": "IO"})


def bin_table(family):
    _, ls, ws, _ = FAMILIES[family]
    nw = len(ws) - 1
    return [
        (f"{family}.{1 + i * nw + j}", ls[i], ls[i + 1], ws[j], ws[j + 1])
        for i in range(len(ls) - 1)
        for j in range(nw)
    ]


BINS = {f: bin_table(f) for f in FAMILIES}


def choose(family, l, w, nf=1):
    """The bin of `family` for L=l, W=w (nm, exact) and nf fingers, or an
    error key"""
    weff = Fraction(w) / nf
    for name, l0, l1, w0, w1 in BINS[family]:
        if l0 <= l < l1 and w0 <= weff < w1:
            return name
    return f"error\t{family}\tL={l}n\tW={w}n"


# the bins of the RF macro model nch_rf (local to its subckt): limits are
# 'edge - dxl_rf', dxl_rf = dl_rf / 2 (a subckt parameter)
RF_L = [60, 120, 240, 1000]
RF_W = [400, 2000, 10000]


def choose_rf(l, w, dl_rf, nf):
    dxl = Fraction(dl_rf, 2)
    weff = Fraction(w) / nf
    n = 1
    for i in range(len(RF_L) - 1):
        for j in range(len(RF_W) - 1):
            if RF_L[i] - dxl <= l < RF_L[i + 1] - dxl and RF_W[j] <= weff < RF_W[j + 1]:
                return f"nch_rf.{n}"
            n += 1
    return f"error\tnch_rf\tL={l}n\tW={w}n"


def sci(nm, style):
    """A length in nm, written as HSPICE would read it, in one of several
    styles"""
    if style == "n":
        return f"{nm}n"
    if style == "u":
        return f"{nm / 1000:g}u"
    if style == "e9":
        return f"{nm}e-9"
    if style == "e3":  # 3.6e-008: the 3-digit exponents of some libraries
        m, e = f"{nm * 1e-9:.4e}".split("e")
        return f"{float(m):g}e{e[0]}{int(e[1:]):03d}"
    return f"{nm * 1e-9:.4e}"  # 3.6000e-08


# ---------------------------------------------------------------------------
# Output with line wrapping
# ---------------------------------------------------------------------------


class Out:
    def __init__(self):
        self.lines = []

    def add(self, *lines):
        self.lines.extend(lines)

    def element(self, head, tokens, tail="", width=150):
        """An element line: `head`, then the tokens, then `tail`, wrapped
        with `+` continuation lines"""
        cur = head
        for t in list(tokens) + ([tail] if tail else []):
            if len(cur) + 1 + len(t) > width and cur != head:
                self.lines.append(cur)
                cur = "+ " + t
            else:
                cur += " " + t
        self.lines.append(cur)

    def text(self):
        return "\n".join(self.lines) + "\n"


def header(out, lib, cell):
    out.add("", f"** Library name: {lib}", f"** Cell name: {cell}", "** View name: schematic")


def footer(out, cell):
    out.add(f".ends {cell}", "** End of subcircuit definition.")


# ---------------------------------------------------------------------------
# The model library file
# ---------------------------------------------------------------------------

CORNERS = {
    # corner: (dvth_n, dvth_p, dtox, dxl, dxw, dl_io)
    "TT": (0, 0, 0, 0, 0, 0),
    "SS": (0.045, -0.045, 3e-11, 2e-9, -4e-9, 5e-9),
    "FF": (-0.045, 0.045, -3e-11, -2e-9, 4e-9, -5e-9),
    "SF": (0.045, 0.045, 0, 0, 0, 0),
    "FS": (-0.045, -0.045, 0, 0, 0, 0),
}


def model_params(rng, family, kind):
    """The (selection-irrelevant) parameters of a BSIM4 bin"""
    n = kind == "nmos"
    sgn = "n" if n else "p"
    names = (
        "toxe toxp toxm dtox epsrox wint lint ll wl lln wln lw ww lwn wwn lwl wwl "
        "xpart toxref vth0 k1 k2 k3 k3b w0 dvt0 dvt1 dvt2 dvt0w dvt1w dvt2w dsub minv "
        "voffl dvtp0 dvtp1 lpe0 lpeb xj ngate ndep nsd phin cdsc cdscb cdscd cit voff "
        "nfactor eta0 etab vfb u0 ua ub uc ud eu vsat a0 ags a1 a2 b0 b1 keta dwg dwb "
        "pclm pdiblc1 pdiblc2 pdiblcb drout pvag delta pscbe1 pscbe2 fprout pdits "
        "pditsd pditsl rsh rdsw rsw rdw rdswmin rdwmin rswmin prwg prwb wr alpha0 "
        "alpha1 beta0 agidl bgidl cgidl egidl aigbacc bigbacc cigbacc nigbacc aigbinv "
        "bigbinv cigbinv eigbinv nigbinv aigc bigc cigc aigsd bigsd cigsd nigc poxedge "
        "pigcd ntox xrcrg1 xrcrg2 cgso cgdo cgbo cgdl cgsl ckappas ckappad acde moin "
        "noff voffcv kt1 kt1l kt2 ute ua1 ub1 uc1 prt at"
    ).split()
    out = []
    for name in names:
        if name == "vth0":
            v = f"'{0.38 if n else -0.41:.3f}+dvth_{sgn}'"
        elif name == "toxe":
            v = "'1.25e-09+dtox'"
        elif name == "lint":
            v = "'5e-09+dxl'"
        elif name == "wint":
            v = "'3e-09+dxw'"
        else:
            v = f"{rng.uniform(-2, 2) * 10 ** rng.randint(-12, 2):.6g}"
        out.append(f"{name}={v}")
    return out


def gen_models(rng):
    out = Out()
    out.add(
        "* Synthetic binned MOSFET model library (28nm-class), for testing",
        "* Corners TT SS FF SF FS load the sections CORE SRAM IO RF.",
        "",
    )
    for corner, (dvn, dvp, dtox, dxl, dxw, dlio) in CORNERS.items():
        out.add(
            f".lib {corner}",
            f".param dvth_n={dvn:g} dvth_p={dvp:g} dtox={dtox:g}",
            f"+ dxl={dxl:g} dxw={dxw:g} dl_io={dlio:g}",
            ".lib 'models.l' CORE",
            ".lib 'models.l' SRAM",
            ".lib 'models.l' IO",
            ".lib 'models.l' RF",
            f".endl {corner}",
            "",
        )
    for section in ("CORE", "SRAM", "IO"):
        out.add(f".lib {section}")
        for fi, family in enumerate(f for f in FAMILIES if SECTION[f] == section):
            kind, _, _, wnflag = FAMILIES[family]
            for name, l0, l1, w0, w1 in BINS[family]:
                if section == "IO":
                    # limits computed from a corner parameter (0 in TT)
                    lim = [
                        f"lmin='{sci(l0, 'sci')}+dl_io'",
                        f"lmax='{sci(l1, 'sci')}+dl_io'",
                        f"wmin={sci(w0, 'sci')}",
                        f"wmax={sci(w1, 'sci')}",
                    ]
                    out.add(f".model {name} {kind} level=54 version=4.7 binunit=1")
                    out.element("+", lim + [f"wnflag={wnflag}"])
                elif fi % 2 == 0:
                    # the style of some PDKs: ( ... ), spaces around '='
                    out.add(f".model {name} {kind} ( level = 54 version = 4.7 binunit = 1")
                    out.add(
                        f"+ lmin = {sci(l0, 'e3')} lmax = {sci(l1, 'e3')} "
                        f"wmin = {sci(w0, 'e3')} wmax = {sci(w1, 'e3')} wnflag = {wnflag}"
                    )
                else:
                    out.add(f".model {name} {kind} level=54 version=4.7 binunit=1")
                    out.add(
                        f"+ lmin={sci(l0, 'sci')} lmax={sci(l1, 'sci')} "
                        f"wmin={sci(w0, 'sci')} wmax={sci(w1, 'sci')} wnflag={wnflag}"
                    )
                out.element("+", model_params(rng, family, kind), width=100)
                if section != "IO" and fi % 2 == 0:
                    out.add("+ )")
        if section == "IO":
            # an exact-named model: no bins
            out.add(".model nch_na nmos level=54 version=4.7 vth0='0.05+dvth_n'")
            out.element("+", model_params(rng, "nch_na", "nmos"), width=100)
        out.add(f".endl {section}", "")
    # an RF macro model: a subckt with its own bins, whose limits use one of
    # its parameters
    out.add(
        ".lib RF",
        ".subckt nch_rf d g s b lr=60n wr=1u nr=1 dl_rf=2n",
        ".param dxl_rf='dl_rf/2'",
    )
    n = 1
    for i in range(len(RF_L) - 1):
        for j in range(len(RF_W) - 1):
            out.add(
                f".model nch_rf.{n} nmos level=54 version=4.7",
                f"+ lmin='{sci(RF_L[i], 'sci')}-dxl_rf' lmax='{sci(RF_L[i + 1], 'sci')}-dxl_rf'"
                f" wmin={sci(RF_W[j], 'sci')} wmax={sci(RF_W[j + 1], 'sci')}",
            )
            out.element("+", model_params(rng, "nch_rf", "nmos")[:40], width=100)
            n += 1
    out.add(
        "Mmain d gi s b nch_rf l=lr w=wr nf=nr",
        "Rg g gi r='10+20/nr'",
        "Cgd d gi 1f",
        ".ends nch_rf",
        ".endl RF",
    )
    return out.text()


# ---------------------------------------------------------------------------
# Standard cells
# ---------------------------------------------------------------------------

VTS = {
    "SVT": ("nch_svt", "pch_svt", 30),
    "LVT": ("nch_lvt", "pch_lvt", 30),
    "HVT": ("nch_hvt", "pch_hvt", 36),
    "ULVT": ("nch_ulvt", "pch_ulvt", 30),
}
WN0, WP0 = 150, 220


def topo(inputs, outputs, pairs, rng):
    """Devices (n/p, drain, gate, source, width factor) of a cell made of
    `pairs` n/p pairs: a plausible tree of stages"""
    devs = []
    nets = list(inputs)
    internal = 0
    for i in range(pairs):
        last = i >= pairs - len(outputs)
        g = nets[i] if i < len(nets) else rng.choice(nets[-4:])
        if last:
            d = outputs[i - (pairs - len(outputs))]
        else:
            d = f"net{internal}"
            internal += 1
        f = rng.choice([1, 1, 1.5, 2])
        devs.append(("n", d, g, "VSS", f))
        devs.append(("p", d, g, "VDD", f * 1.5 if f > 1 else f))
        nets.append(d)
    return devs


CELL_TYPES = {
    # name: (inputs, outputs, n/p pairs, drives, weight)
    "INV": (["A"], ["Y"], 1, [1, 2, 4, 8, 16], 12),
    "BUF": (["A"], ["Y"], 2, [1, 2, 4, 8, 16], 6),
    "CKINV": (["A"], ["Y"], 1, [2, 4, 8, 16], 1),
    "CKBUF": (["A"], ["Y"], 2, [2, 4, 8, 16], 1),
    "NAND2": (["A", "B"], ["Y"], 2, [1, 2, 4, 8], 12),
    "NAND3": (["A", "B", "C"], ["Y"], 3, [1, 2, 4], 4),
    "NAND4": (["A", "B", "C", "D"], ["Y"], 4, [1, 2, 4], 2),
    "NOR2": (["A", "B"], ["Y"], 2, [1, 2, 4, 8], 7),
    "NOR3": (["A", "B", "C"], ["Y"], 3, [1, 2, 4], 2),
    "AND2": (["A", "B"], ["Y"], 3, [1, 2, 4], 3),
    "OR2": (["A", "B"], ["Y"], 3, [1, 2, 4], 2),
    "AOI21": (["A1", "A2", "B"], ["Y"], 3, [1, 2, 4], 6),
    "OAI21": (["A1", "A2", "B"], ["Y"], 3, [1, 2, 4], 6),
    "AOI22": (["A1", "A2", "B1", "B2"], ["Y"], 4, [1, 2, 4], 4),
    "OAI22": (["A1", "A2", "B1", "B2"], ["Y"], 4, [1, 2, 4], 3),
    "XOR2": (["A", "B"], ["Y"], 6, [1, 2, 4], 3),
    "XNOR2": (["A", "B"], ["Y"], 6, [1, 2, 4], 2),
    "MUX2": (["I0", "I1", "S"], ["Y"], 6, [1, 2, 4], 5),
    "HA": (["A", "B"], ["S", "CO"], 7, [1, 2], 1),
    "FA": (["A", "B", "CI"], ["S", "CO"], 14, [1, 2], 1.5),
    "LATCH": (["D", "E"], ["Q"], 8, [1, 2], 1),
    "DFF": (["D", "CK"], ["Q", "QN"], 12, [1, 2, 4], 8),
    "DFFR": (["D", "CK", "RN"], ["Q", "QN"], 14, [1, 2, 4], 4),
    "SDFF": (["D", "SI", "SE", "CK"], ["Q", "QN"], 16, [1, 2, 4], 3),
    "ICG": (["E", "TE", "CK"], ["Q"], 10, [1, 2, 4, 8], 0.5),
    "TIEH": ([], ["Y"], 1, [1], 0.3),
    "TIEL": ([], ["Y"], 1, [1], 0.2),
    "DLY4": (["A"], ["Y"], 4, [1, 2], 0.5),
    "DCAP4": ([], [], 2, [1], 2),
}


class Cell:
    """A subckt with fixed contents: its pins, and how many MOSFETs use
    each model"""

    def __init__(self, name, pins, usage):
        self.name, self.pins, self.usage = name, pins, usage


def gen_stdcells(rng):
    out = Out()
    out.add("* Standard-cell library (synthetic), HSPICE netlist")
    cells = {}
    for ctype, (inputs, outputs, pairs, drives, _) in CELL_TYPES.items():
        trng = random.Random(ctype)
        if ctype == "TIEH":
            devs = [("n", "net0", "net0", "VSS", 1), ("p", "Y", "net0", "VDD", 1)]
        elif ctype == "TIEL":
            devs = [("p", "net0", "net0", "VDD", 1), ("n", "Y", "net0", "VSS", 1)]
        elif ctype == "DCAP4":
            devs = [
                ("p", "net0", "net1", "VDD", 1),
                ("n", "net1", "net0", "VSS", 1),
                ("n", "VSS", "net0", "VSS", 4),
                ("p", "VDD", "net1", "VDD", 4),
            ]
        else:
            devs = topo(inputs, outputs, pairs, trng)
        for drive in drives:
            for vt, (nmod, pmod, lvt) in VTS.items():
                name = f"{ctype}_X{drive}_{vt}"
                pins = inputs + outputs + ["VDD", "VSS"]
                header(out, "stdcell", name)
                out.add(f".subckt {name} {' '.join(pins)}")
                usage = Counter()
                for i, (k, d, g, s, f) in enumerate(devs):
                    model = nmod if k == "n" else pmod
                    w0 = WN0 if k == "n" else WP0
                    wf = int(round(w0 * f / 5)) * 5
                    l = 150 if ctype == "DLY4" and i < 4 else lvt
                    if ctype == "DCAP4" and i >= 2:
                        l = 150
                    nf = drive if (d in outputs) else 1
                    w = wf * nf
                    usage[choose(model, l, w, nf)] += 1
                    b = "VSS" if k == "n" else "VDD"
                    ad = w * 1e-9 * 7e-8
                    pd = 2 * (w + 70) * 1e-9
                    out.add(
                        f"MM{i} {d} {g} {s} {b} {model.upper() if vt == 'ULVT' else model} "
                        f"L={sci(l, 'u')} W={sci(w, 'u')} "
                        f"nf={nf} AD={ad:.4g} AS={ad:.4g} PD={pd:.4g} PS={pd:.4g} "
                        f"SA=7e-08 SB=7e-08 SD=1e-07 m=1"
                    )
                footer(out, name)
                cells[name] = Cell(name, pins, usage)
    return out.text(), cells


# ---------------------------------------------------------------------------
# The chip
# ---------------------------------------------------------------------------

WORDS = (
    "addr data ctrl valid ready tag idx way hit miss pc inst op src dst imm res carry "
    "sum sel en wdata rdata mask ptr cnt state req ack grant credit flush stall kill "
    "fwd byp rob lq sq rs ghist btb ras tlb vpn ppn asid perm dirty lru evict fill "
    "snoop resp beat burst wstrb prot qos len size lock cache user id"
).split()


FUNCS = (
    "ctrl dpath arb fifo dec enc pipe alu mul div sched cam tbl ecc crc sync cdc bpred "
    "fetch align queue issue wakeup sel rf byp agu tlbctl pwalk mshr wb snp csr irq "
    "perf trace dbg"
).split()


class Chip:
    def __init__(self, rng, cells, args):
        self.rng, self.cells, self.args = rng, cells, args
        self.out = Out()
        self.case_rng = random.Random(args.seed + 3)
        self.defs = {}  # name -> Cell (usage with default parameters)
        self.cell_names = list(cells)
        self.cell_weights = []
        for name in self.cell_names:
            ctype, drive, vt = name.split("_")
            w = CELL_TYPES[ctype][4]
            w *= {"X1": 50, "X2": 25, "X4": 15, "X8": 7, "X16": 3}[drive]
            w *= {"SVT": 50, "LVT": 25, "HVT": 20, "ULVT": 5}[vt]
            self.cell_weights.append(w)
        self.cum_weights = list(itertools.accumulate(self.cell_weights))

    # -- custom cells --------------------------------------------------------

    def ckdly_usage(self, ld, nfd):
        u = Counter()
        u[choose("nch_hvt", ld, 200 * nfd, nfd)] += 1
        u[choose("pch_hvt", ld, 300 * nfd, nfd)] += 1
        u[choose("nch_svt", 30, 200)] += 1
        u[choose("pch_svt", 30, 300)] += 1
        return u

    def gen_custom(self):
        o = self.out
        # a delay cell: its default ld must win over the top-level .param ld
        # (.option parhier=local)
        header(o, "custom", "CKDLY")
        o.add(
            ".subckt CKDLY A Y VDD VSS ld=150n nfd=1",
            "MM0 n1 A VSS VSS nch_hvt l=ld w='200n*nfd' nf=nfd $ long-channel delay stage",
            "MM1 n1 A VDD VDD pch_hvt l=ld w='300n*nfd' nf=nfd",
            "MM2 Y n1 VSS VSS nch_svt l=30n w=200n",
            "MM3 Y n1 VDD VDD pch_svt l=30n w=300n",
        )
        footer(o, "CKDLY")
        # SRAM periphery, with parameters
        header(o, "sram", "SA_LATCH")
        o.add(".subckt SA_LATCH bl blb saen q qb VDD VSS wn=400n wp=300n ls=40n")
        sa = [
            ("MM0 q qb n0 VSS nch_lvt", "l=ls w=wn nf=2"),
            ("MM1 qb q n0 VSS nch_lvt", "l=ls w=wn nf=2"),
            ("MM2 q qb VDD VDD pch_lvt", "l=ls w=wp"),
            ("MM3 qb q VDD VDD pch_lvt", "l=ls w=wp"),
            ("MM4 n0 saen VSS VSS nch_svt", "l=ls w='2*wn'"),
            ("MM5 q saen bl VDD pch_svt", "l=30n w=wp"),
            ("MM6 qb saen blb VDD pch_svt", "l=30n w=wp"),
            ("MM7 q qb VDD VDD pch_svt", "l=30n w='wp/2'"),
        ]
        for a, b in sa:
            o.add(f"{a} {b}")
        footer(o, "SA_LATCH")
        header(o, "sram", "WLDRV")
        o.add(
            ".subckt WLDRV a en wl VDD VSS nfw=2",
            "MM0 n0 a n1 VSS nch_svt l=30n w=300n",
            "MM1 n1 en VSS VSS nch_svt l=30n w=300n",
            "MM2 n0 a VDD VDD pch_svt l=30n w=220n",
            "MM3 n0 en VDD VDD pch_svt l=30n w=220n",
            "MM4 wl n0 VSS VSS nch_svt l=30n w='180n*nfw' nf=nfw",
            "MM5 wl n0 VDD VDD pch_svt l=30n w='270n*nfw' nf=nfw",
        )
        footer(o, "WLDRV")
        fixed = {
            "PRECHG": (
                ["pre_n", "bl", "blb", "VDD"],
                [("p", "bl", "pre_n", "VDD", "pch_svt", 30, 300), ("p", "blb", "pre_n", "VDD", "pch_svt", 30, 300),
                 ("p", "bl", "pre_n", "blb", "pch_svt", 30, 200)],
            ),
            "WRDRV": (
                ["we", "d", "bl", "blb", "VDD", "VSS"],
                [("n", "bl", "db", "n0", "nch_svt", 30, 400), ("n", "blb", "d", "n0", "nch_svt", 30, 400),
                 ("n", "n0", "we", "VSS", "nch_svt", 30, 800), ("n", "db", "d", "VSS", "nch_svt", 30, 150),
                 ("p", "db", "d", "VDD", "pch_svt", 30, 220), ("n", "VSS", "VDD", "VSS", "nch_hvt", 36, 400)],
            ),
            "COLMUX4": (
                ["sel<0>", "sel<1>", "sel<2>", "sel<3>", "bl<0>", "bl<1>", "bl<2>", "bl<3>", "blb<0>", "blb<1>",
                 "blb<2>", "blb<3>", "dl", "dlb", "VDD", "VSS"],
                [("n", f"{b}<{i}>", f"sel<{i}>", d, "nch_svt", 30, 600) for i in range(4)
                 for b, d in (("bl", "dl"), ("blb", "dlb"))],
            ),
        }
        for name, (pins, devs) in fixed.items():
            header(o, "sram", name)
            o.add(f".subckt {name} {' '.join(pins)}")
            u = Counter()
            for i, (k, d, g, s, model, l, w) in enumerate(devs):
                b = "VSS" if k == "n" else "VDD"
                o.add(f"MM{i} {d} {g} {s} {b} {model} l={sci(l, 'n')} w={sci(w, 'n')}")
                u[choose(model, l, w)] += 1
            footer(o, name)
            self.defs[name] = Cell(name, pins, u)
        # the bitcell
        header(o, "sram", "SRAM6T")
        o.add(
            ".subckt SRAM6T bl blb wl VDD VSS",
            "MPU0 q qb VDD VDD pchpu_sr l=30n w=70n",
            "MPU1 qb q VDD VDD pchpu_sr l=30n w=70n",
            "MPD0 q qb VSS VSS nchpd_sr l=30n w=110n",
            "MPD1 qb q VSS VSS nchpd_sr l=30n w=110n",
            "MPG0 bl wl q VSS nchpg_sr l=36n w=80n",
            "MPG1 blb wl qb VSS nchpg_sr l=36n w=80n",
        )
        footer(o, "SRAM6T")
        u = Counter()
        for fam, l, w in (("pchpu_sr", 30, 70),) * 2 + (("nchpd_sr", 30, 110),) * 2 + (("nchpg_sr", 36, 80),) * 2:
            u[choose(fam, l, w)] += 1
        self.defs["SRAM6T"] = Cell("SRAM6T", ["bl", "blb", "wl", "VDD", "VSS"], u)
        # level shifters, ESD, pads (IO devices)
        header(o, "io", "LVLSHIFT")
        o.add(
            ".subckt LVLSHIFT a y VDD VDDIO VSS",
            "MM0 ab a VSS VSS nch_svt l=30n w=200n",
            "MM1 ab a VDD VDD pch_svt l=30n w=300n",
            "MM2 n0 a VSS VSS nch_18 l=150n w=1u",
            "MM3 n1 ab VSS VSS nch_18 l=150n w=1u",
            "MM4 n0 n1 VDDIO VDDIO pch_18 l=300n w=500n",
            "MM5 n1 n0 VDDIO VDDIO pch_18 l=300n w=500n",
            "MM6 y n1 VSS VSS nch_18 l=150n w=4u nf=4",
            "MM7 y n1 VDDIO VDDIO pch_18 l=150n w=8u nf=8",
        )
        footer(o, "LVLSHIFT")
        u = Counter()
        for fam, l, w, nf in (("nch_svt", 30, 200, 1), ("pch_svt", 30, 300, 1), ("nch_18", 150, 1000, 1),
                              ("nch_18", 150, 1000, 1), ("pch_18", 300, 500, 1), ("pch_18", 300, 500, 1),
                              ("nch_18", 150, 4000, 4), ("pch_18", 150, 8000, 8)):
            u[choose(fam, l, w, nf)] += 1
        self.defs["LVLSHIFT"] = Cell("LVLSHIFT", ["a", "y", "VDD", "VDDIO", "VSS"], u)
        header(o, "io", "ESD_CLAMP")
        o.add(
            ".subckt ESD_CLAMP pad VDDIO VSS",
            "MM0 pad g VSS VSS nch_18 l=300n w=60u nf=30",
            "MM1 g pad VSS VSS nch_na l=1u w=10u",
            "Rg g VSS 20k",
        )
        footer(o, "ESD_CLAMP")
        u = Counter()
        u[choose("nch_18", 300, 60000, 30)] += 1
        u["nch_na"] += 1
        self.defs["ESD_CLAMP"] = Cell("ESD_CLAMP", ["pad", "VDDIO", "VSS"], u)
        header(o, "io", "IO_PAD")
        o.add(".subckt IO_PAD core pad oe VDD VDDIO VSS")
        o.add("XLS0 core d18 VDD VDDIO VSS LVLSHIFT")
        o.add("XLS1 oe oe18 VDD VDDIO VSS LVLSHIFT")
        o.add("XESD pad VDDIO VSS ESD_CLAMP")
        o.add("MDRVN pad n_g VSS VSS nch_18 l=150n w=40u nf=20")
        o.add("MDRVP pad p_g VDDIO VDDIO pch_18 l=150n w=80u nf=40")
        o.add("XG0 d18 oe18 n_g VDD VSS NAND2_X4_SVT")
        o.add("XG1 d18 oe18 p_g VDD VSS NOR2_X4_SVT")
        footer(o, "IO_PAD")
        u = self.defs["LVLSHIFT"].usage + self.defs["LVLSHIFT"].usage + self.defs["ESD_CLAMP"].usage
        u[choose("nch_18", 150, 40000, 20)] += 1
        u[choose("pch_18", 150, 80000, 40)] += 1
        u += self.cells["NAND2_X4_SVT"].usage + self.cells["NOR2_X4_SVT"].usage
        self.defs["IO_PAD"] = Cell("IO_PAD", ["core", "pad", "oe", "VDD", "VDDIO", "VSS"], u)
        # a decap no bin holds (W above every WMAX), on purpose
        header(o, "io", "DCAP_BIG")
        o.add(".subckt DCAP_BIG VDD VSS", "MM0 VSS VDD VSS VSS nch_svt l=1u w=120u", "MM1 VDD VSS VDD VDD pch_svt l=1u w=40u")
        footer(o, "DCAP_BIG")
        u = Counter()
        u[choose("nch_svt", 1000, 120000)] += 1
        u[choose("pch_svt", 1000, 40000)] += 1
        self.defs["DCAP_BIG"] = Cell("DCAP_BIG", ["VDD", "VSS"], u)
        # fillers: FILLD0 has one MOSFET, FILLDk ten FILLD(k-1)
        header(o, "fill", "FILLD0")
        o.add(".subckt FILLD0 VDD VSS", "MM0 VSS VDD VSS VSS nch_svt l=150n w=1u")
        footer(o, "FILLD0")
        self.defs["FILLD0"] = Cell("FILLD0", ["VDD", "VSS"], Counter({choose("nch_svt", 150, 1000): 1}))
        for k in range(1, 7):
            name = f"FILLD{k}"
            header(o, "fill", name)
            o.add(f".subckt {name} VDD VSS")
            for i in range(10):
                o.add(f"XF{i} VDD VSS FILLD{k - 1}")
            footer(o, name)
            u = Counter()
            for m, n in self.defs[f"FILLD{k - 1}"].usage.items():
                u[m] = 10 * n
            self.defs[name] = Cell(name, ["VDD", "VSS"], u)

    # -- SRAM macros ---------------------------------------------------------

    def gen_sram(self, rows, cols):
        o = self.out
        rng = self.rng
        bc = "BC2X2"
        if bc not in self.defs:
            header(o, "sram", bc)
            o.add(f".subckt {bc} bl<0> bl<1> blb<0> blb<1> wl<0> wl<1> VDD VSS")
            for r in range(2):
                for c in range(2):
                    o.add(f"XC{r}{c} bl<{c}> blb<{c}> wl<{r}> VDD VSS SRAM6T")
            footer(o, bc)
            u = Counter({m: 4 * n for m, n in self.defs["SRAM6T"].usage.items()})
            self.defs[bc] = Cell(bc, None, u)
        row = f"ROW2_C{cols}"
        if row not in self.defs:
            header(o, "sram", row)
            pins = [f"bl<{c}>" for c in range(cols)] + [f"blb<{c}>" for c in range(cols)] + ["wl<0>", "wl<1>", "VDD", "VSS"]
            o.element(f".subckt {row}", pins)
            for c in range(0, cols, 2):
                o.add(f"XBC{c // 2} bl<{c}> bl<{c + 1}> blb<{c}> blb<{c + 1}> wl<0> wl<1> VDD VSS {bc}")
            footer(o, row)
            u = Counter({m: cols // 2 * n for m, n in self.defs[bc].usage.items()})
            self.defs[row] = Cell(row, pins, u)
        grp = f"ROW16_C{cols}"
        if grp not in self.defs:
            header(o, "sram", grp)
            pins = [f"bl<{c}>" for c in range(cols)] + [f"blb<{c}>" for c in range(cols)]
            pins += [f"wl<{r}>" for r in range(16)] + ["VDD", "VSS"]
            o.element(f".subckt {grp}", pins)
            for r in range(0, 16, 2):
                nets = [f"bl<{c}>" for c in range(cols)] + [f"blb<{c}>" for c in range(cols)]
                nets += [f"wl<{r}>", f"wl<{r + 1}>", "VDD", "VSS"]
                o.element(f"XR{r // 2}", nets, row)
            footer(o, grp)
            u = Counter({m: 8 * n for m, n in self.defs[row].usage.items()})
            self.defs[grp] = Cell(grp, pins, u)
        arr = f"ARRAY_{rows}X{cols}"
        header(o, "sram", arr)
        pins = [f"bl<{c}>" for c in range(cols)] + [f"blb<{c}>" for c in range(cols)]
        pins += [f"wl<{r}>" for r in range(rows)] + ["VDD", "VSS"]
        o.element(f".subckt {arr}", pins)
        for g in range(rows // 16):
            nets = [f"bl<{c}>" for c in range(cols)] + [f"blb<{c}>" for c in range(cols)]
            nets += [f"wl<{16 * g + r}>" for r in range(16)] + ["VDD", "VSS"]
            o.element(f"XG{g}", nets, grp)
        footer(o, arr)
        arr_u = Counter({m: rows // 16 * n for m, n in self.defs[grp].usage.items()})
        # the macro: array, wordline drivers, column periphery, control
        name = f"SRAM_{rows}X{cols}"
        abits = (rows - 1).bit_length()
        pins = [f"a<{i}>" for i in range(abits)] + [f"d<{i}>" for i in range(cols // 4)]
        pins += [f"q<{i}>" for i in range(cols // 4)] + ["clk", "cen", "wen", "VDD", "VSS"]
        header(o, "sram", name)
        o.element(f".subckt {name}", pins, "wsa=400n nfw=2")
        # (on one line, as some netlisters write it: tens of kB)
        o.element("XARR", [f"bl<{c}>" for c in range(cols)] + [f"blb<{c}>" for c in range(cols)]
                  + [f"wl<{r}>" for r in range(rows)] + ["VDD", "VSS"], arr, width=10**9)
        u = Counter(arr_u)
        for r in range(rows):
            o.add(f"XWL{r} xdec<{r}> wlen wl<{r}> VDD VSS WLDRV nfw=nfw")
        for c in range(cols):
            o.add(f"XPC{c} pre_n bl<{c}> blb<{c}> VDD PRECHG")
            o.add(f"XWD{c} we d<{c // 4}> bl<{c}> blb<{c}> VDD VSS WRDRV")
            u += self.defs["PRECHG"].usage + self.defs["WRDRV"].usage
        for c in range(cols // 4):
            sel = " ".join(f"ysel<{i}>" for i in range(4))
            bls = " ".join(f"bl<{4 * c + i}>" for i in range(4)) + " " + " ".join(f"blb<{4 * c + i}>" for i in range(4))
            o.add(f"XCM{c} {sel} {bls} dl<{c}> dlb<{c}> VDD VSS COLMUX4")
            o.add(f"XSA{c} dl<{c}> dlb<{c}> saen q<{c}> qb<{c}> VDD VSS SA_LATCH wn='wsa' wp='wsa*0.75'")
            u += self.defs["COLMUX4"].usage
        # control: std cells
        ctrl = Counter()
        for i in range(150):
            cell = self.pick_cell(rng)
            c = self.cells[cell]
            nets = [f"ctl_n{rng.randrange(400)}" for _ in c.pins[:-2]]
            o.add(f"XCTL{i} {' '.join(nets)} VDD VSS {cell}")
            ctrl += c.usage
        u += ctrl
        footer(o, name)

        @functools.cache
        def usage(wsa=400, nfw=2):
            v = Counter(u)
            for _ in range(rows):
                v += self.wldrv_usage(nfw)
            for _ in range(cols // 4):
                v += self.sa_usage(wsa, Fraction(wsa * 3, 4), 40)
            return v

        self.defs[name] = Cell(name, pins, None)
        self.defs[name].usage_fn = usage
        return name

    def wldrv_usage(self, nfw):
        u = Counter()
        u[choose("nch_svt", 30, 300)] += 2
        u[choose("pch_svt", 30, 220)] += 2
        u[choose("nch_svt", 30, 180 * nfw, nfw)] += 1
        u[choose("pch_svt", 30, 270 * nfw, nfw)] += 1
        return u

    def sa_usage(self, wn, wp, ls):
        u = Counter()
        u[choose("nch_lvt", ls, wn, 2)] += 2
        u[choose("pch_lvt", ls, wp)] += 2
        u[choose("nch_svt", ls, 2 * wn)] += 1
        u[choose("pch_svt", 30, wp)] += 2
        u[choose("pch_svt", 30, wp / 2)] += 1
        return u

    # -- logic ---------------------------------------------------------------

    def next_id(self):
        self.ids = getattr(self, "ids", 0) + 1
        return self.ids

    def pick_cell(self, rng):
        return rng.choices(self.cell_names, cum_weights=self.cum_weights)[0]

    def gen_block(self, name, n_cells):
        """A block of standard cells, as a netlister writes a synthesized
        module: long net names, buses"""
        rng = self.rng
        o = self.out
        unit, func = name[4:].split("_", 1)
        prefix = f"u_{unit}_u_{func}"
        n_in = rng.randint(3, 10)
        n_out = rng.randint(3, 10)
        ins, outs = [], []
        for _ in range(n_in):
            w = rng.choice(WORDS)
            ins += [f"{prefix}_{w}_i[{k}]" for k in range(rng.choice([1, 4, 8, 16, 32]))]
        for _ in range(n_out):
            w = rng.choice(WORDS)
            outs += [f"{prefix}_{w}_o[{k}]" for k in range(rng.choice([1, 4, 8, 16, 32]))]
        ins = list(dict.fromkeys(ins))
        outs = [n for n in dict.fromkeys(outs) if n not in ins]
        pins = ins + outs + ["clk", "rst_n", "VDD", "VSS"]
        header(o, "core", name)
        o.element(f".subckt {name}", pins)
        pool = ins + ["clk", "rst_n"]
        usage = Counter()
        used = Counter()
        drv = 0
        for i in range(n_cells):
            r = rng.random()
            if r < 0.004:
                # a delay cell, with or without parameters
                p = rng.choice(["", "", "", "ld=500n", "ld=1u nfd=2", "nfd=4", "ld='2*250n'"])
                a = pool[rng.randrange(max(0, len(pool) - 200), len(pool))]
                y = f"{prefix}_dly_{i}"
                o.add(f"XDLY{i} {a} {y} VDD VSS CKDLY {p}".rstrip())
                # (with parhier=global, the top-level ld=2u wins over all)
                ld, nfd = (150 if self.args.parhier == "local" else 2000), 1
                for kv in p.split():
                    k, v = kv.split("=")
                    if k != "ld":
                        nfd = int(v)
                    elif self.args.parhier == "local":
                        ld = {"500n": 500, "1u": 1000, "'2*250n'": 500}[v]
                usage += self.ckdly_usage(ld, nfd)
                pool.append(y)
                continue
            cell = self.pick_cell(rng)
            c = self.cells[cell]
            ctype = cell.split("_")[0]
            n_inputs = len(CELL_TYPES[ctype][0])
            n_outputs = len(CELL_TYPES[ctype][1])
            nets = []
            for pin in c.pins[:n_inputs]:
                if pin in ("CK",):
                    nets.append(f"{prefix}_clk_gated_{rng.randrange(8)}")
                elif pin == "RN":
                    nets.append("rst_n")
                else:
                    nets.append(pool[rng.randrange(max(0, len(pool) - 300), len(pool))])
            for k in range(n_outputs):
                if drv < len(outs) and rng.random() < 0.05:
                    y = outs[drv]
                    drv += 1
                else:
                    w = rng.choice(WORDS)
                    w2 = rng.choice(WORDS)
                    y = f"{prefix}_{w}_{w2}_{rng.choice(['q', 'd', 'nxt', 'r', 'w'])}_reg_{i}_n{k}"
                pool.append(y)
                nets.append(y)
            iname = f"X{prefix}_U{i}"
            o.add(f"{iname} {' '.join(nets)} VDD VSS {cell.lower() if self.case_rng.random() < 0.1 else cell}")
            used[cell] += 1
        # the outputs not driven yet
        for y in outs[drv:]:
            a = pool[rng.randrange(max(0, len(pool) - 300), len(pool))]
            o.add(f"X{prefix}_obuf_{y.replace('[', '_').replace(']', '')} {a} {y} VDD VSS BUF_X2_SVT")
            used["BUF_X2_SVT"] += 1
        for cell, n in used.items():
            for m, k in self.cells[cell].usage.items():
                usage[m] += n * k
        footer(o, name)
        self.defs[name] = Cell(name, pins, usage)
        return name

    def gen_unit(self, name, blocks, rng):
        """A unit: instances of blocks, their pins on the unit's nets"""
        o = self.out
        pins = ["clk", "rst_n", "VDD", "VSS"]
        exported = []
        for b in blocks:
            for p in self.defs[b].pins[:-4]:
                if rng.random() < 0.08:
                    exported.append(f"{name}_{p}")
        exported = list(dict.fromkeys(exported))
        header(o, "core", name)
        o.element(f".subckt {name}", exported + pins)
        u = Counter()
        exp = set(exported)
        for i, b in enumerate(blocks):
            nets = []
            for p in self.defs[b].pins[:-4]:
                n = f"{name}_{p}"
                nets.append(n if n in exp or rng.random() < 0.9 else f"{name}_{p}_x{i}")
            nets += ["clk", "rst_n", "VDD", "VSS"]
            o.element(f"X{b[4:]}_{i}", nets, b + (f" id={self.next_id()}" if self.args.instance_ids else ""))
            u += self.defs[b].usage
        footer(o, name)
        self.defs[name] = Cell(name, exported + pins, u)
        return name


def instantiate(o, iname, cell, nets_for, params="", ids=None):
    pins = cell.pins
    if ids is not None:
        params = f"{params} id={ids()}"
    o.element(iname, [nets_for(p) for p in pins], (cell.name + " " + params).strip())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("outdir")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--clusters", type=int, default=9)
    ap.add_argument("--mos", type=int, default=None, help="MOSFETs in total (default 300M; 0: no fillers)")
    ap.add_argument("--small", action="store_true", help="a small deck (fewer, smaller blocks)")
    ap.add_argument("--parhier", choices=["local", "global"], default="local")
    ap.add_argument("--instance-ids", action="store_true",
                    help="pass a parameter id=N, unique and read by nothing, on every instance of a block, unit, core or cluster")
    ap.add_argument("--block-scale", type=float, default=0.64, help="scales the cells per block (the deck's size)")
    args = ap.parse_args()
    if args.mos is None:
        args.mos = 0 if args.small else 300_000_000
    rng = random.Random(args.seed)
    os.makedirs(args.outdir, exist_ok=True)

    models = gen_models(random.Random(args.seed + 1))
    stdcell_text, cells = gen_stdcells(random.Random(args.seed + 2))
    chip = Chip(rng, cells, args)
    o = chip.out
    o.add(
        "* chip_top: synthetic hierarchical HSPICE netlist (gen_bignet.py)",
        "* nearly all MOSFETs use the bins of a binned model library",
        "",
        ".option post=2 probe=1 ingold=2 numdgt=7 measdgt=7",
        f"+ parhier={args.parhier} runlvl=5 accurate=1",
        ".temp 25",
        ".global VDD VSS VDDIO",
        ".param supply=0.9 vio=1.8",
        "* also a default of the subckt CKDLY: with parhier=local the default wins, with",
        "* parhier=global this value",
        ".param ld=2u",
        ".lib 'models.l' TT",
        ".include 'stdcell.sp'",
        ".include 'analog.inc'",
    )
    chip.gen_custom()

    small = args.small
    sram_cfg = [(128, 32), (256, 64), (512, 64)] if small else [(256, 64), (512, 64), (512, 128), (1024, 128), (2048, 128)]
    srams = [chip.gen_sram(r, c) for r, c in sram_cfg]

    # blocks and units
    def blocks_for(unit, n, lo, hi):
        funcs = rng.sample(FUNCS, n)
        return [chip.gen_block(f"blk_{unit}_{f}{i}", int(rng.randint(lo, hi) * args.block_scale))
                for i, f in enumerate(funcs)]

    lo, hi = (40, 120) if small else (500, 1500)
    big_units = ["ifu", "bpu", "idu", "ren", "rob", "isq", "exu0", "exu1", "exu2", "exu3", "lsu", "dtlb", "fpu0", "fpu1",
                 "vpu0", "vpu1", "mmu", "biu"]
    lit_units = ["lifu", "lidu", "lexu", "llsu", "lfpu", "lbiu"]
    clu_units = ["noc", "l2ctl", "snp", "pmu", "dbg"]
    top_units = ["ddr0", "ddr1", "pcie", "usb", "eth", "dma", "sec", "gpio", "clkgen", "rst"]
    if small:
        big_units, lit_units, clu_units, top_units = big_units[:3], lit_units[:2], clu_units[:2], top_units[:2]
    units = {}
    for group, names, nb in (("big", big_units, (5, 9)), ("lit", lit_units, (4, 7)), ("clu", clu_units, (4, 8)),
                             ("top", top_units, (3, 7))):
        for u in names:
            bl = blocks_for(u, rng.randint(*nb), lo, hi)
            # some blocks used twice in a unit
            bl += rng.sample(bl, k=min(len(bl), rng.randint(0, 2)))
            units[u] = chip.gen_unit(f"unit_{u}", bl, rng)

    def container(name, children):
        """A subckt instantiating `children`: (instance name, def, params,
        usage of that instance)"""
        header(o, "core", name)
        pins = ["clk", "rst_n", "VDD", "VSS"]
        o.add(f".subckt {name} {' '.join(pins)}")
        u = Counter()
        for iname, d, params, du in children:
            cell = chip.defs[d]
            nets = lambda p: p if p in pins else f"{iname}_{p}"
            instantiate(o, iname, cell, nets, params, chip.next_id if args.instance_ids and not d.startswith("SRAM") else None)
            u += du
        footer(o, name)
        chip.defs[name] = Cell(name, pins, u)
        return u

    def du(d):
        c = chip.defs[d]
        return c.usage if c.usage is not None else c.usage_fn()

    s = srams
    # a big core replicates some of its units (execution units, ...)
    core_big = [(f"X{u}_{j}" if j else f"X{u}", units[u], "", du(units[u]))
                for u in big_units for j in range(1 if small else rng.choice([1, 1, 2, 2, 3]))]
    core_big += [(f"XL1I_{i}", s[0], "", du(s[0])) for i in range(4)]
    core_big += [(f"XL1D_{i}", s[0], "nfw=3", chip.defs[s[0]].usage_fn(nfw=3)) for i in range(4)]
    container("CORE_BIG", core_big)
    core_lit = [(f"X{u}", units[u], "", du(units[u])) for u in lit_units]
    core_lit += [(f"XL1_{i}", s[0], "", du(s[0])) for i in range(2)]
    container("CORE_LIT", core_lit)
    l2 = s[-2] if not small else s[-1]
    ncores = 4 if small else 8
    cluster = [(f"XCB{i}", "CORE_BIG", "", du("CORE_BIG")) for i in range(ncores)]
    cluster += [(f"XCL{i}", "CORE_LIT", "", du("CORE_LIT")) for i in range(ncores)]
    cluster += [(f"X{u}", units[u], "", du(units[u])) for u in clu_units]
    cluster += [(f"XL2_{i}", l2, "wsa=500n" if i % 4 == 0 else "", chip.defs[l2].usage_fn(wsa=500 if i % 4 == 0 else 400))
                for i in range(8 if not small else 2)]
    container("CLUSTER", cluster)

    # the top: clusters, last-level cache, SoC units, IO ring, analog
    llc = s[-1]
    top = [(f"XCLUSTER{i}", "CLUSTER", "", du("CLUSTER")) for i in range(args.clusters)]
    top += [(f"X{u}", units[u], "", du(units[u])) for u in top_units]
    base = Counter()
    for _, _, _, x in top:
        base += x
    pads = 200 if not small else 8
    pad_u = Counter({m: pads * n for m, n in chip.defs["IO_PAD"].usage.items()})
    analog_u, analog_text = gen_analog()
    fixed_total = sum(base.values()) + sum(pad_u.values()) + sum(analog_u.values()) + 2 * 4
    llc_n = 0
    if args.mos:
        per = sum(du(llc).values())
        llc_n = max(0, (args.mos - fixed_total) // per - (0 if small else 0))
        # leave room for the fillers
        while llc_n > 0 and args.mos - fixed_total - llc_n * per < 0:
            llc_n -= 1
    top += [(f"XLLC{i}", llc, "", du(llc)) for i in range(llc_n)]
    chip_u = container("CHIP_TOP", top)

    # the top level
    o.add("", "* the chip, its IO ring and analog blocks")
    o.add("XCHIP clk rst_n VDD VSS CHIP_TOP")
    total = Counter(chip_u)
    for i in range(pads):
        o.add(f"XPAD{i} pad_core<{i}> pad<{i}> pad_oe<{i}> VDD VDDIO VSS IO_PAD")
    total += pad_u
    o.add("XPLL0 clk_ref clk VDD VSS PLL_TOP", "XBGR0 vbg VDD VSS BGR")
    total += analog_u
    for i in range(4):
        o.add(f"XDCAPB{i} VDD VSS DCAP_BIG")
        total += chip.defs["DCAP_BIG"].usage
    if args.mos:
        rest = args.mos - sum(total.values())
        assert rest >= 0, rest
        for k in range(6, -1, -1):
            n, rest = divmod(rest, 10**k)
            for i in range(n):
                o.add(f"XFILL{k}_{i} VDD VSS FILLD{k}")
                total += Counter({m: c * 10**k for m, c in chip.defs["FILLD0"].usage.items()})
    o.add(
        "",
        "VDD VDD 0 'supply'",
        "VDDIO VDDIO 0 'vio'",
        "VCLK clk_ref 0 pulse(0 'supply' 0 20p 20p 0.98n 2n)",
        "VRST rst_n 0 pwl(0 0 5n 0 5.1n 'supply')",
        ".tran 10p 100n",
        ".op",
        ".probe tran v(clk) v(XCHIP.XCLUSTER0.XCB0.Xifu.clk)",
        ".meas tran tclk trig v(clk) val='supply/2' rise=10 targ v(clk) val='supply/2' rise=11",
        "",
        ".alter slow corner",
        ".del lib 'models.l' TT",
        ".lib 'models.l' SS",
        ".temp 125",
        "",
        ".alter fast corner",
        ".del lib 'models.l' SS",
        ".lib 'models.l' FF",
        ".temp -40",
        ".end",
    )

    files = {
        "models.l": models,
        "stdcell.sp": stdcell_text,
        "analog.inc": analog_text,
        "chip.sp": o.text(),
    }
    for name, text in files.items():
        with open(os.path.join(args.outdir, name), "w") as f:
            f.write(text)
    def write_expected(path, usage):
        with open(os.path.join(args.outdir, path), "w") as f:
            f.write(f"total\t{sum(usage.values())}\n")
            for m, n in sorted(usage.items(), key=lambda kv: (-kv[1], kv[0])):
                f.write(f"{m}\t{n}\n")

    write_expected("expected.tsv", total)
    # some subckts on their own (with their default parameters)
    for d in ["CORE_BIG", "CORE_LIT", "CLUSTER", "CHIP_TOP"] + srams:
        write_expected(f"expected_{d}.tsv", du(d))
    for name, text in files.items():
        print(f"{name}: {text.count(chr(10))} lines, {len(text.encode())} bytes")
    chip_text = files["chip.sp"]
    # (a bin `family.N`: not the exact-named model `nch_svt` local to a subckt)
    lib = sum(n for m, n in total.items() if "." in m and m.split(".")[0] in FAMILIES)
    print(f"MOSFETs: {sum(total.values())}, of which {lib} use a bin of models.l")
    print(f"subckts in chip.sp: {chip_text.count(chr(10) + '.subckt ')}")
    for d in ["CORE_BIG", "CORE_LIT", "CLUSTER", "CHIP_TOP"] + srams:
        print(f"  {d}: {sum(du(d).values())} MOSFETs")
    cells = sum(n for m, n in total.items() if m.endswith("_sr.1") or m.endswith("_sr.2"))
    print(f"  SRAM bitcell MOSFETs: {cells} ({100 * cells / sum(total.values()):.1f}%)")


def gen_analog():
    """analog.inc: a PLL and a bandgap; exact-named models of their own, the
    RF macro model of models.l, a model local to a subckt"""
    o = Out()
    o.add(
        "* analog cells, with models of their own",
        ".model nch_lp nmos level=54 version=4.7 vth0=0.45 toxe=1.8e-9",
        ".model pch_lp pmos level=54 version=4.7 vth0=-0.47 toxe=1.8e-9",
        "",
        ".subckt VCO_STAGE in out VDD VSS",
        "* a model local to this subckt: it hides the bins nch_svt.N of the library",
        ".model nch_svt nmos level=54 version=4.7 vth0=0.35",
        "MN0 out in VSS VSS nch_svt l=60n w=2u nf=4",
        "MP0 out in VDD VDD pch_svt l=60n w=3u nf=4",
        "MN1 out in VSS VSS nch_lp l=200n w=1u",
        ".ends VCO_STAGE",
        "",
        ".subckt PLL_TOP ref clk VDD VSS",
    )
    u = Counter()
    for i in range(9):
        o.add(f"XVCO{i} v{i} v{(i + 1) % 9} VDD VSS VCO_STAGE")
        u["nch_svt"] += 1
        u[choose("pch_svt", 60, 3000, 4)] += 1
        u["nch_lp"] += 1
    rf = [(100, 8000, 4, None), (100, 1000, 1, None), (300, 1200, 2, None), (118, 6000, 2, 4), (119, 6000, 2, 4)]
    for i, (l, w, nf, dl) in enumerate(rf):
        extra = f" dl_rf={dl}n" if dl else ""
        o.add(f"XLNA{i} lo{i} rf{i} VSS VSS nch_rf lr={l}n wr={sci(w, 'u')} nr={nf}{extra}")
        u[choose_rf(l, w, dl or 2, nf)] += 1
    o.add("XCP0 up dn vctl VDD VSS CHGPUMP", ".ends PLL_TOP", "", ".subckt CHGPUMP up dn out VDD VSS")
    for i in range(6):
        o.add(f"MN{i} out up n{i} VSS nch_lp l=500n w=2u")
        o.add(f"MP{i} out dn p{i} VDD pch_lp l=500n w=4u")
        u["nch_lp"] += 1
        u["pch_lp"] += 1
    o.add(".ends CHGPUMP", "", ".subckt BGR vbg VDD VSS")
    for i in range(8):
        o.add(f"MN{i} n{i} n{i} VSS VSS nch_na l=2u w=4u m=8")
        o.add(f"MP{i} n{i} pb VDD VDD pch_18 l=1u w=5u m=4")
        u["nch_na"] += 1
        u[choose("pch_18", 1000, 5000)] += 1
    o.add("R1 n0 vbg 120k", "Q1 VSS VSS n1 pnp_bjt area=8", ".ends BGR")
    return u, o.text()


if __name__ == "__main__":
    main()
