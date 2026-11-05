#!/usr/bin/env python3
"""Hand-designed decks whose answers are known in advance, for every query
mode of model_select: the listing, --summary, --subckt NAME (listing and
counts), and library mode. The answers are computed here by formulas (from
the instance path, or as closed-form counts) or by the scoping rule written
out directly, not by another implementation.

usage: torture.py [--big] [--ngspice] [--reference] [--keep DIR]

--big        also the large decks (hundreds of millions of MOSFETs, counts
             only)
--ngspice    also compare the decks ngspice reads as HSPICE does with
             ngspice (parhier=local; values off bin edges)
--reference  also compare the small decks with pref.py

Cases:
 chain      24 levels, each passing w='w*1.05' l='l+1n' to the next: L, W at
            each depth by formula, L reaching bin edges through accumulated
            sums; under parhier=global the top-level .param and the first
            value passed win all the way down
 tree       a binary tree passing p='2*p' and p='2*p+1': each leaf's p is its
            path read in binary; W='(1+p mod 7)*100n'; every context distinct
 lattice    three instances per level, passing a='a', a='a', a='a+1' and an
            unused id: counts are binomial sums (581 M MOSFETs with --big);
            under parhier=global the first value passed wins
 matrix     a name defined (top .param, .subckt default, .param inside) or
            passed at each of 3 levels, in every combination, read at the
            bottom: the value by the rule of each parhier
 bins       overlapping and gapped bins, a model and bins of the same name,
            case, dotted names (n.ch.1), non-numeric suffixes (nm.mc), fingers,
            edges through float noise, .option scale, local bins that hold L
            and W before global ones
 errors     a name defined in some contexts only, parameters defined by
            each other, division by zero, an unknown function
 library    no top-level instance: each subckt with its defaults; a nested
            subckt hiding a top-level one, case, a subckt defined after use
"""
import collections
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
from fractions import Fraction as F

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
MS = os.environ.get("MODEL_SELECT", os.path.join(REPO, "target/release/examples/model_select"))

results = collections.Counter()
failures = []


# ---------------------------------------------------------------------------
# bins
# ---------------------------------------------------------------------------


class Family:
    """A binned family on a grid of L and W edges (strings, in meters)"""

    def __init__(self, name, kind, ledges, wedges, extra=""):
        self.name, self.kind, self.extra = name, kind, extra
        self.ledges, self.wedges = ledges, wedges

    def models(self):
        out, n = [], 1
        for i in range(len(self.ledges) - 1):
            for j in range(len(self.wedges) - 1):
                out.append(
                    f".model {self.name}.{n} {self.kind} level=54 lmin={self.ledges[i]} "
                    f"lmax={self.ledges[i + 1]} wmin={self.wedges[j]} wmax={self.wedges[j + 1]}{self.extra}"
                )
                n += 1
        return out

    def bin(self, l, w):
        """The bin of exact L and W: LMIN <= L < LMAX, WMIN <= W < WMAX"""
        le = [F(x) for x in self.ledges]
        we = [F(x) for x in self.wedges]
        nw = len(we) - 1
        for i in range(len(le) - 1):
            for j in range(nw):
                if le[i] <= l < le[i + 1] and we[j] <= w < we[j + 1]:
                    return f"{self.name}.{1 + i * nw + j}"
        return "error"


# ---------------------------------------------------------------------------
# running model_select
# ---------------------------------------------------------------------------


def run_ms(args):
    r = subprocess.run([MS] + args, capture_output=True, text=True, timeout=3600)
    return r.stdout


def listing(deck, subckt=None):
    """path -> (L, W, NF, model or 'error'); for a library or --subckt, the
    paths keep their `SUBCKT:` prefix"""
    out = run_ms(([] if subckt is None else ["--subckt", subckt]) + [deck])
    rows = {}
    for line in out.splitlines()[1:]:
        f = line.split("\t")
        model = "error" if f[-1].startswith("error") else f[-1]
        num = lambda x: float(x) if x else None  # noqa: E731
        rows[f[0]] = (num(f[2]), num(f[3]), num(f[4]), model)
    return rows


def counts(deck, subckt=None):
    out = run_ms(["--summary"] + ([] if subckt is None else ["--subckt", subckt]) + [deck])
    c = collections.Counter()
    for line in out.splitlines()[1:]:
        f = line.split("\t")
        name = f[0]
        if subckt is not None or (":" in name and not name.startswith("error")):
            name = name.split(":", 1)[1] if ":" in name and not name.startswith("error") else name
        if name == "total" or name.endswith(":total"):
            continue
        c["error" if name.startswith("error") else name] += int(f[1])
    return c


def close(a, b):
    if a is None or b is None:
        return a is None and b is None
    return abs(float(a) - float(b)) <= 1e-9 * max(abs(float(a)), abs(float(b)))


def check_listing(label, got, expected):
    """`expected`: path -> (L, W, NF, model); L, W, NF may be None (not
    checked) for errors"""
    results[label + ": MOSFETs"] += len(expected)
    bad = []
    for p in sorted(set(got) | set(expected)):
        if p not in got or p not in expected:
            bad.append(f"{p}: {'missing' if p not in got else 'unexpected'}")
            continue
        g, e = got[p], expected[p]
        if g[3] != e[3]:
            bad.append(f"{p}: model {g[3]}, expected {e[3]} (L={g[0]} W={g[1]} NF={g[2]})")
        elif e[3] != "error":
            for k, name in ((0, "L"), (1, "W"), (2, "NF")):
                if e[k] is not None and not close(g[k], e[k]):
                    bad.append(f"{p}: {name}={g[k]}, expected {float(e[k])}")
    if bad:
        results[label + ": FAILED"] += 1
        failures.append(f"{label}: {len(bad)} difference(s): " + "; ".join(bad[:6]))
    return not bad


def check_counts(label, got, expected):
    expected = collections.Counter({k: v for k, v in expected.items() if v})
    results[label + ": MOSFETs counted"] += sum(expected.values())
    if got != expected:
        results[label + ": FAILED"] += 1
        diff = {k: (got.get(k, 0), expected.get(k, 0)) for k in set(got) | set(expected) if got.get(k, 0) != expected.get(k, 0)}
        failures.append(f"{label}: counts (got, expected) {dict(list(diff.items())[:6])}")
        return False
    return True


def tally(expected_listing):
    return collections.Counter(v[3] for v in expected_listing.values())


def strip(rows, prefix):
    return {p.split(":", 1)[1] if p.startswith(prefix + ":") else p: v for p, v in rows.items()}


# ---------------------------------------------------------------------------
# cases
# ---------------------------------------------------------------------------

NB = Family("nb", "nmos", ["1e-10", "35e-9", "40e-9", "45e-9", "50e-9", "60e-9", "2e-6"],
            ["1e-10", "0.4e-6", "0.6e-6", "0.8e-6", "2e-6", "20e-6"])
FB = Family("fb", "nmos", ["1e-10", "15e-9", "20e-9", "25e-9", "30e-9", "100e-9", "1e-6"], ["1e-10", "1"])


def chain(mode, depth=24, l0="30e-9", lk0="10", step="1"):
    """Each level passes w='w*1.05' l='l+{step}n' to the next, from w=0.3u,
    l=l0; M1's L is lk*{step} nm: lk is the level's default (local) or the
    top .param (global). With parhier=global, the top-level `w=5u` and the
    outermost `l` win at every level."""
    lines = [f"* chain of {depth} levels", f".option parhier={mode}", ".param w=5u lk=99"]
    lines += NB.models() + FB.models()
    for k in range(depth):
        # (names of one width: ngspice can lose `c1` among `c10`...`c19`)
        lines.append(f".subckt c{k:02d} d g s w=1u l=1u lk={F(lk0) + k:g}")
        lines.append("M0 d g s s nb w=w l=l")
        lines.append(f"M1 d g s s fb w=1u l='lk*{step}n'")
        if k:
            lines.append(f"X0 d g s c{k - 1:02d} w='w*1.05' l='l+{step}n'")
        lines.append(f".ends c{k:02d}")
    lines.append(f"X0 d g 0 c{depth - 1:02d} w=0.3u l={l0}")
    lines.append(".end")

    def expect(levels, w0, l0, k_top):
        exp = {}
        for j in range(levels):
            path = ".".join(["X0"] * j + ["M"]) if j else "M"
            w = w0 * F(21, 20) ** j if mode == "local" else F("5e-6")
            l = l0 + j * F(step) * F("1e-9") if mode == "local" else l0
            exp[path + "0"] = (l, w, 1, NB.bin(l, w))
            lk = 99 if mode == "global" else F(lk0) + (k_top - j)
            l1 = lk * F(step) * F("1e-9")
            exp[path + "1"] = (l1, F("1e-6"), 1, FB.bin(l1, F("1e-6")))
        return exp

    top = {("X0." + p if p.startswith(("X", "M")) else p): v
           for p, v in expect(depth, F("0.3e-6"), F(l0), depth - 1).items()}
    # an instance of c10 with its defaults
    sub = expect(11, F("1e-6"), F("1e-6"), 10)
    return "\n".join(lines) + "\n", top, {"c10": sub}


NW = Family("nw", "nmos", ["1e-10", "1"], ["1e-10", "250e-9", "450e-9", "650e-9", "1e-6", "2e-6"])


def tree(depth):
    lines = [f"* binary tree of {depth} levels", ".option parhier=local"] + NW.models()
    lines += [".subckt b0 d g s p=0", "M0 d g s s nw w='(1+p-7*floor(p/7))*100n' l=1u",
              "M1 d g s s nw w=1.1u l=1u", ".ends b0"]
    for k in range(1, depth + 1):
        lines += [f".subckt b{k} d g s p=0", f"X0 d g s b{k - 1} p='2*p'", f"X1 d g s b{k - 1} p='2*p+1'", f".ends b{k}"]
    lines += [f"X0 d g 0 b{depth} p=0", ".end"]
    return "\n".join(lines) + "\n"


def tree_expected(depth, listing_too):
    w = lambda r: (1 + r) * F("100e-9")  # noqa: E731
    exp = {}
    if listing_too:
        for p in range(2 ** depth):
            bits = format(p, f"0{depth}b") if depth else ""
            path = ".".join(["X0"] + [f"X{b}" for b in bits])
            exp[path + ".M0"] = (F(1, 10**6), w(p % 7), 1, NW.bin(F(1, 10**6), w(p % 7)))
            exp[path + ".M1"] = (F(1, 10**6), F("1.1e-6"), 1, NW.bin(F(1, 10**6), F("1.1e-6")))
    c = collections.Counter()
    n = 2 ** depth
    for r in range(7):
        c[NW.bin(F(1, 10**6), w(r))] += (n - 1 - r) // 7 + 1 if n - 1 - r >= 0 else 0
    c[NW.bin(F(1, 10**6), F("1.1e-6"))] += n
    return exp, c


NL = Family("nl", "nmos", ["1e-10", "1"], ["1e-10", "125e-9", "275e-9", "425e-9", "625e-9", "2e-6"])


def lattice(depth, mode="local"):
    lines = [f"* ternary lattice of {depth} levels", f".option parhier={mode}"] + NL.models()
    lines += [".subckt t0 d g s a=0 id=0", "M0 d g s s nl w='(1+a)*50n' l=1u", ".ends t0"]
    for k in range(1, depth + 1):
        lines += [f".subckt t{k} d g s a=0 id=0", "M0 d g s s nl w='(1+a)*50n' l=1u",
                  f"X0 d g s t{k - 1} a='a' id=0", f"X1 d g s t{k - 1} a='a' id=1",
                  f"X2 d g s t{k - 1} a='a+1' id=2", f".ends t{k}"]
    lines += [f"X0 d g 0 t{depth} a=0", ".end"]
    return "\n".join(lines) + "\n"


def lattice_expected(depth, listing_too, mode="local"):
    """With parhier=global, the first `a=0` wins at every level"""
    w = lambda a: (1 + a) * F("50e-9")  # noqa: E731
    step = 1 if mode == "local" else 0
    exp = {}
    if listing_too:
        def walk(path, j, a):
            exp[path + ".M0"] = (F(1, 10**6), w(a), 1, NL.bin(F(1, 10**6), w(a)))
            if j < depth:
                for d in range(3):
                    walk(f"{path}.X{d}", j + 1, a + step * (d == 2))
        walk("X0", 0, 0)
    c = collections.Counter()
    for j in range(depth + 1):
        for i in range(j + 1):
            c[NL.bin(F(1, 10**6), w(i * step))] += math.comb(j, i) * 2 ** (j - i)
    return exp, c


def matrix(mode, top_defined):
    """Every combination of (default, .param inside, passed) at 3 levels;
    passing needs the default (a subckt declares what it is passed)"""
    options = [(0, 0, 0), (0, 1, 0), (1, 0, 0), (1, 1, 0), (1, 0, 1), (1, 1, 1)]
    vals = {1: (2, 3, 4), 2: (5, 6, 7), 3: (8, 9, 10)}
    lines = [f"* parameter matrix ({mode}, top .param {'v=1' if top_defined else 'none'})",
             f".option parhier={mode}", ".model nv nmos level=54"]
    if top_defined:
        lines.append(".param v=1")
    exp = {}
    c = 0
    for o1 in options:
        for o2 in options:
            for o3 in options:
                lv = {1: o1, 2: o2, 3: o3}
                for k in (3, 2, 1):
                    d, b, _ = lv[k]
                    lines.append(f".subckt m{c}_{k} d g s" + (f" v={vals[k][0]}" if d else ""))
                    if b:
                        lines.append(f".param v={vals[k][1]}")
                    if k == 3:
                        lines.append("M0 d g s s nv w='v*1u' l=1u")
                    else:
                        lines.append(f"X0 d g s m{c}_{k + 1}" + (f" v={vals[k + 1][2]}" if lv[k + 1][2] else ""))
                    lines.append(f".ends m{c}_{k}")
                lines.append(f"X{c} d g 0 m{c}_1" + (f" v={vals[1][2]}" if o1[2] else ""))

                def binding(k):
                    """What level k binds v to: passed, else .param, else
                    the default"""
                    d, b, p = lv[k]
                    return vals[k][2] if p else vals[k][1] if b else vals[k][0] if d else None

                # the closest to the bottom with local, to the top with global
                top = [1] if top_defined else []
                bound = [binding(k) for k in (1, 2, 3) if binding(k) is not None]
                order = top + bound if mode == "global" else bound[::-1] + top
                v = order[0] if order else None
                path = f"X{c}.X0.X0.M0"
                exp[path] = (None, None, None, "error") if v is None else (F(1, 10**6), v * F(1, 10**6), 1, "nv")
                c += 1
    lines.append(".end")
    return "\n".join(lines) + "\n", exp


def limits(line):
    """A `.model` line with the limits it does not set, wide (a bin needs its
    four limits)"""
    wide = (("lmin", "1e-10"), ("lmax", "1"), ("wmin", "1e-10"), ("wmax", "1"))
    return line + "".join(f" {k}={v}" for k, v in wide if f"{k}=" not in line.lower())


def bins_case():
    um = F(1, 10**6)
    lines = ["* bins"] + [limits(line) for line in (
        ".model ov.1 nmos lmin=1e-7 lmax=1e-6 wmin=1e-7 wmax=1e-5",
        ".model ov.2 nmos lmin=5e-7 lmax=2e-6 wmin=1e-7 wmax=1e-5",
        ".model gp.1 nmos lmin=1e-7 lmax=2e-7", ".model gp.2 nmos lmin=3e-7 lmax=4e-7",
        ".model op.1 nmos lmax=1e-7", ".model op.2 nmos lmin=1e-7",
        ".model ex.1 nmos lmin=1e-10 lmax=1",
        ".model CS.1 NMOS LMIN=1E-7 LMAX=1E-6", ".model cs.2 nmos lmin=1e-6 lmax=1e-5",
        ".model n.ch.1 nmos lmin=1e-10 lmax=1", ".model n.ch.2 nmos lmin=1 lmax=2",
        ".model nm.1 nmos lmin=1e-7 lmax=1e-6", ".model nm.mc nmos level=54",
        ".model nm.1a nmos level=54",
        ".model mx.1 nmos wnflag=1 lmin=1e-10 lmax=1 wmin=1e-10 wmax=1e-6",
        ".model mx.2 nmos lmin=1e-10 lmax=1 wmin=1e-6 wmax=1e-3",
        ".model ed.1 nmos lmin=1e-10 lmax=3e-7", ".model ed.2 nmos lmin=3e-7 lmax=1",
        ".model loc.1 nmos lmin=1e-10 lmax=1e-6", ".model loc.2 nmos lmin=1e-6 lmax=1",
    )] + [".model ex nmos level=54",
          ".subckt hide d g s",
          limits(".model loc.1 nmos lmin=1e-10 lmax=5e-7"),
          "M0 d g s s loc l=0.3u w=1u",
          "M1 d g s s loc l=0.8u w=1u",
          ".ends hide"]
    devices = [
        ("M1", "ov l=0.7u w=1u", (F("0.7e-6"), um, 1, "ov.1")),       # in both: the first
        ("M2", "ov l=1.5u w=1u", (F("1.5e-6"), um, 1, "ov.2")),
        ("M3", "gp l=0.25u w=1u", (None, None, None, "error")),       # in no bin
        ("M4", "op l=10n w=50u", (F("10e-9"), 50 * um, 1, "op.1")),   # open limits
        ("M5", "op l=5u w=1u", (5 * um, um, 1, "op.2")),
        ("M6", "ex l=1u w=1u", (um, um, 1, "ex")),                    # the model of that name
        ("M7", "cs l=0.5u w=1u", (F("0.5e-6"), um, 1, "CS.1")),       # case
        ("M8", "CS l=2u w=1u", (2 * um, um, 1, "cs.2")),
        ("M9", "n.ch l=0.5 w=1u", (None, None, None, "error")),       # n.ch.1 is a bin of
        ("M10", "n l=0.5 w=1u", (None, None, None, "error")),         # nothing
        ("M11", "nm l=0.5u w=1u", (F("0.5e-6"), um, 1, "nm.1")),
        ("M12", "nm l=5u w=1u", (5 * um, um, 1, "nm.mc")),            # nm.mc is a bin of nm
        ("M13", "mx w=4u l=1u nf=8", (um, 4 * um, 8, "mx.1")),        # the width per finger
        ("M14", "mx w=4u l=1u", (um, 4 * um, 1, "mx.2")),
        ("M15", "ed l='0.1u*3' w=1u", (F("0.3e-6"), um, 1, "ed.2")),  # edges, float noise
        ("M16", "ed l='1u-0.7u' w=1u", (F("0.3e-6"), um, 1, "ed.2")),
        ("M17", "ed l='0.9u/3' w=1u", (F("0.3e-6"), um, 1, "ed.2")),
        ("M18", "ed l=299.999n w=1u", (F("299.999e-9"), um, 1, "ed.1")),
        ("M19", "loc l=0.8u w=1u", (F("0.8e-6"), um, 1, "loc.1")),
    ]
    for name, rest, _ in devices:
        lines.append(f"{name} d g 0 0 {rest}")
    lines += ["X1 d g 0 hide", ".end"]
    exp = {name: e for name, _, e in devices}
    # the local bin, then, for L=0.8u that it does not hold, the global one
    exp["X1.M0"] = (F("0.3e-6"), um, 1, "loc.1")
    exp["X1.M1"] = (F("0.8e-6"), um, 1, "loc.1")
    # .option scale
    scale = ["* scale", ".option scale=1e-6", limits(".model sc.1 nmos lmin=1e-10 lmax=1e-6"),
             limits(".model sc.2 nmos lmin=1e-6 lmax=1"), "M1 d g 0 0 sc l=0.5 w=2", "M2 d g 0 0 sc l=2 w=2", ".end"]
    sexp = {"M1": (F("0.5e-6"), 2 * um, 1, "sc.1"), "M2": (2 * um, 2 * um, 1, "sc.2")}
    return "\n".join(lines) + "\n", exp, "\n".join(scale) + "\n", sexp


def errors_case(mode):
    um = F(1, 10**6)
    lines = ["* errors", f".option parhier={mode}", ".model nv nmos level=54", ".param k=1",
             ".subckt eu d g s", "M0 d g s s nv w='u*1u' l=1u", ".ends eu",
             ".subckt pu d g s u=2", "X0 d g s eu", ".ends pu",
             ".subckt rc d g s a='b' b='a'", "M0 d g s s nv w='a*1u' l=1u", ".ends rc",
             "X0 d g 0 eu", "X1 d g 0 pu", "X2 d g 0 pu u=3", "X3 d g 0 rc", "X4 d g 0 rc a=1",
             "M1 d g 0 0 nv w='1u/(k-k)' l=1u", "M2 d g 0 0 nv w='foo(1)*1u' l=1u", ".end"]
    exp = {"X0.M0": (None, None, None, "error"), "X1.X0.M0": (um, 2 * um, 1, "nv"),
           "X2.X0.M0": (um, 3 * um, 1, "nv"), "X3.M0": (None, None, None, "error"),
           "X4.M0": (um, um, 1, "nv"), "M1": (None, None, None, "error"), "M2": (None, None, None, "error")}
    return "\n".join(lines) + "\n", exp


def library_case():
    um = F(1, 10**6)
    lines = ["* library", ".model nv nmos level=54",
             ".subckt inv d g s w=1u", "M0 d g s s nv w=w l=1u", ".ends inv",
             ".subckt top2 d g s",
             ".subckt inv d g s w=2u", "M0 d g s s nv w='w*3' l=1u", ".ends inv",
             "X0 d g s inv", "X1 d g s INV w=1u", "X2 d g s later", ".ends top2",
             ".subckt later d g s", "M0 d g s s nv w=7u l=1u", ".ends later", ".end"]
    exp = {"inv:M0": (um, um, 1, "nv"),
           "top2:X0.M0": (um, 6 * um, 1, "nv"), "top2:X1.M0": (um, 3 * um, 1, "nv"),
           "top2:X2.M0": (um, 7 * um, 1, "nv"), "later:M0": (um, 7 * um, 1, "nv")}
    return "\n".join(lines) + "\n", exp


# ---------------------------------------------------------------------------
# ngspice and the reference (optional)
# ---------------------------------------------------------------------------


def other(tool, deck, expected, label):
    """The listing of ngspice or the reference against the expected one"""
    script = {"ngspice": "ngs.py", "reference": "pref.py"}[tool]
    r = subprocess.run(["python3", os.path.join(HERE, script), deck], capture_output=True, text=True, timeout=3600)
    rows = {}
    for line in r.stdout.splitlines()[1:]:
        f = line.split("\t")
        if f[0].endswith(".*"):
            continue
        model = "error" if f[-1].startswith("error") or f[-1] == "not compared" else f[-1]
        num = lambda x: float(x) if x not in ("", "None") else None  # noqa: E731
        rows[f[0].lower()] = (num(f[1]), num(f[2]), num(f[3]), model.lower())
    exp = {p.lower(): (v[0], v[1], v[2], v[3].lower()) for p, v in expected.items()}
    if tool == "ngspice":
        # ngspice prints 6 digits
        bad = []
        for p in exp:
            g = rows.get(p)
            if g is None or g[3] != exp[p][3] or any(
                    exp[p][k] is not None and abs(float(g[k]) - float(exp[p][k])) > 1e-5 * abs(float(exp[p][k]))
                    for k in (0, 1)):
                bad.append(f"{p}: {g} vs {exp[p]}")
        results[label + ": MOSFETs"] += len(exp)
        if bad:
            results[label + ": FAILED"] += 1
            failures.append(f"{label}: {len(bad)} difference(s): " + "; ".join(bad[:5]))
        return
    check_listing(label, rows, exp)


# ---------------------------------------------------------------------------


def main():
    big = "--big" in sys.argv
    ngs = "--ngspice" in sys.argv and shutil.which("ngspice")
    ref = "--reference" in sys.argv
    keep = sys.argv[sys.argv.index("--keep") + 1] if "--keep" in sys.argv else None
    work = keep or tempfile.mkdtemp(prefix="torture_")
    os.makedirs(work, exist_ok=True)
    if not os.path.exists(MS):
        sys.exit(f"no {MS}: cargo build --release --example model_select (or set MODEL_SELECT)")

    def write(name, text, drive=True):
        # (the nodes the top-level instances use are driven, for ngspice)
        if drive:
            text = re.sub(r"(?m)^\.end\s*$", "Vd d 0 1\nVg g 0 1\n.end", text)
        path = os.path.join(work, name)
        with open(path, "w") as f:
            f.write(text)
        return path

    for mode in ("local", "global"):
        # chain
        text, exp, subs = chain(mode)
        d = write(f"chain_{mode}.sp", text)
        check_listing(f"chain {mode} listing", listing(d), exp)
        check_counts(f"chain {mode} --summary", counts(d), tally(exp))
        check_listing(f"chain {mode} --subckt c10", strip(listing(d, "c10"), "c10"), subs["c10"])
        check_counts(f"chain {mode} --subckt c10 --summary", counts(d, "c10"), tally(subs["c10"]))
        if ref:
            other("reference", d, exp, f"chain {mode}: reference")
        if ngs and mode == "local":
            # 2.5 nm from every edge (ngspice's 1 nm rule), and 18 levels
            # (ngspice loses subckts in some deeper chains)
            text2, exp2, _ = chain(mode, depth=18, l0="32.5e-9", lk0="2.5", step="5")
            other("ngspice", write("chain_ngspice.sp", text2), exp2, "chain: ngspice")
        # parameter matrix
        for top in (True, False):
            text, exp = matrix(mode, top)
            d = write(f"matrix_{mode}_{int(top)}.sp", text)
            check_listing(f"matrix {mode} top={int(top)} listing", listing(d), exp)
            check_counts(f"matrix {mode} top={int(top)} --summary", counts(d), tally(exp))
            if ref:
                other("reference", d, exp, f"matrix {mode} top={int(top)}: reference")
            if ngs and mode == "local" and top:
                other("ngspice", d, exp, "matrix: ngspice")
        # errors
        text, exp = errors_case(mode)
        d = write(f"errors_{mode}.sp", text)
        check_listing(f"errors {mode} listing", listing(d), exp)
        check_counts(f"errors {mode} --summary", counts(d), tally(exp))
        if ref:
            other("reference", d, exp, f"errors {mode}: reference")
    # tree and lattice: listing on small ones, counts on all
    for depth in (6, 12):
        d = write(f"tree{depth}.sp", tree(depth))
        exp, c = tree_expected(depth, True)
        check_listing(f"tree {depth} listing", listing(d), exp)
        check_counts(f"tree {depth} --summary", counts(d), c)
        if ref:
            other("reference", d, exp, f"tree {depth}: reference")
        if ngs and depth == 6:
            other("ngspice", d, exp, "tree 6: ngspice")
    for depth in (4, 8):
        for mode in ("local", "global"):
            d = write(f"lattice{depth}_{mode}.sp", lattice(depth, mode))
            exp, c = lattice_expected(depth, True, mode)
            check_listing(f"lattice {depth} {mode} listing", listing(d), exp)
            check_counts(f"lattice {depth} {mode} --summary", counts(d), c)
            if ref and mode == "local":
                other("reference", d, exp, f"lattice {depth}: reference")
        if ngs and depth == 4:
            exp, _ = lattice_expected(depth, True)
            other("ngspice", d.replace("_global", "_local"), exp, "lattice 4: ngspice")
    if big:
        for depth in (20,):
            d = write(f"tree{depth}.sp", tree(depth))
            _, c = tree_expected(depth, False)
            check_counts(f"tree {depth} --summary ({2 * 2 ** depth} MOSFETs)", counts(d), c)
        for depth in (18,):
            d = write(f"lattice{depth}.sp", lattice(depth))
            _, c = lattice_expected(depth, False)
            check_counts(f"lattice {depth} --summary ({(3 ** (depth + 1) - 1) // 2} MOSFETs)", counts(d), c)
    # bins
    text, exp, stext, sexp = bins_case()
    d = write("bins.sp", text)
    check_listing("bins listing", listing(d), exp)
    check_counts("bins --summary", counts(d), tally(exp))
    d = write("scale.sp", stext)
    check_listing("scale listing", listing(d), sexp)
    # library mode
    text, exp = library_case()
    d = write("library.sp", text, drive=False)
    check_listing("library listing", listing(d), exp)
    for name in ("inv", "top2", "later"):
        sub = {p.split(":", 1)[1]: v for p, v in exp.items() if p.startswith(name + ":")}
        check_listing(f"library --subckt {name}", strip(listing(d, name), name), sub)
        check_counts(f"library --subckt {name} --summary", counts(d, name), tally(sub))

    for k in sorted(results):
        print(f"{k:62s} {results[k]}")
    print(f"{len(failures)} failure(s)")
    for f in failures:
        print("  " + f[:1500])
    if not keep:
        shutil.rmtree(work)
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
