#!/usr/bin/env python3
"""Random HSPICE decks that stress parameter passing into MOSFETs.

usage: pgen.py SEED OUT.sp [--parhier local|global] [--ngspice]

Every deck has:
- global .param definitions of a pool of names (some of them only in
  `--parhier global` decks are left undefined at the top, defined by subckts)
- subckts in levels (a subckt instantiates subckts of lower levels): default
  parameters (literals, or expressions of globals and of its own earlier
  defaults), .param statements inside, MOSFETs whose L, W and NF are
  expressions, and instances of lower subckts that pass values (expressions
  of the parameters visible where the instance line is)
- global binned models with numeric limits (nch, pch), global binned models
  whose limits read a global parameter (gch: `lb`, which subckts shadow),
  per-finger binning (fch, WNFLAG=1, not in ngspice decks: ngspice ignores
  WNFLAG), and models local to a subckt whose limits read its parameters
- MOSFETs at the top level, and instances of the top subckts

--ngspice: only what ngspice reads as HSPICE does (no WNFLAG, no pow, every
name defined at the top, so that ngspice resolves each name)
"""
import random
import re
import sys

POOL = ["k", "a", "b", "c", "wn", "wp", "lx", "nfv", "s", "lb"]
# the dimensionless names: positive values
DIMLESS = ["k", "a", "b", "c", "s"]


def num(rng, lo, hi):
    return f"{rng.uniform(lo, hi):.3g}"


class Gen:
    def __init__(self, seed, parhier, ngspice):
        self.rng = random.Random(seed)
        self.parhier = parhier
        self.ngspice = ngspice
        self.lines = []

    def dimless(self, names, depth=0):
        """A positive dimensionless expression of `names` (dimensionless
        names), in about [0.2, 10]"""
        rng = self.rng
        r = rng.random()
        if depth > 2 or r < 0.3:
            return rng.choice(names) if names and rng.random() < 0.7 else num(rng, 0.5, 3)
        e1 = self.dimless(names, depth + 1)
        e2 = self.dimless(names, depth + 1)
        if not self.ngspice and rng.random() < 0.15:
            # functions defined with .param f(x)=...
            return rng.choice([f"fscale({e1})", f"fmix({e1},{e2})"])
        return rng.choice([
            f"({e1}+{e2})",
            f"({e1}*{e2})",
            f"({e1}/{rng.choice(['2', '1.5', '3'])})",
            f"max({e1},{e2})",
            f"min({e1},{e2})",
            f"sqrt({e1})",
            f"(abs({e1}-{e2})+0.2)",
            f"({e1}>{e2} ? {e1} : {e2}*0.5)",
            f"(({e1})**2/4+0.3)",
        ])

    def length(self, names, dim):
        """An expression of a length: `lx` or `wn`/`wp` (lengths), scaled"""
        rng = self.rng
        d = [n for n in names if n in DIMLESS]
        r = rng.random()
        base = {"l": "40n", "w": "400n"}[dim]
        lens = [n for n in names if n in (["lx"] if dim == "l" else ["wn", "wp"])]
        if lens and r < 0.35:
            return f"'{rng.choice(lens)}*{self.dimless(d)}'" if rng.random() < 0.6 else rng.choice(lens)
        if r < 0.45:
            # a literal, sometimes on a bin edge
            if dim == "l":
                return rng.choice(["30n", "60n", "120n", "0.25u", "500n", "45n", "90n"])
            return rng.choice(["150n", "400n", "1u", "2.5u", "0.6u", "3u"])
        return f"'{self.dimless(d)}*{base}'"

    def value(self, name, names, own=True):
        """A value for the parameter `name`; `own`: its definition (which
        does not read the name it defines), else a value passed"""
        rng = self.rng
        if own:
            names = [n for n in names if n != name]
        d = [n for n in names if n in DIMLESS]
        if name == "nfv":
            return rng.choice(["1", "2", "3", "4", "'max(1,nfv-1)'" if "nfv" in names else "2"])
        if name == "lx":
            return rng.choice([f"{rng.choice([30, 45, 60, 90, 130, 200])}n", f"'{self.dimless(d)}*40n'"])
        if name in ("wn", "wp"):
            return rng.choice([f"{rng.choice([0.2, 0.3, 0.5, 0.8, 1.2, 2])}u", f"'{self.dimless(d)}*300n'"])
        if name == "lb":
            return rng.choice(["150n", "250n", "120n", "'k*100n'" if "k" in names else "200n"])
        if rng.random() < 0.5:
            return num(rng, 0.5, 3)
        return f"'{self.dimless(d)}'"

    def generate(self):
        rng = self.rng
        L = self.lines
        L.append(f"* random parameter deck (parhier={self.parhier})")
        L.append(f".option parhier={self.parhier}")
        if not self.ngspice and rng.random() < 0.3:
            L.append(".option wnflag=1")
        # global parameters: all of them in ngspice decks; else some
        if self.ngspice:
            globs = list(POOL)
        else:
            globs = [n for n in POOL if n == "lb" or rng.random() < 0.7]
        defined = []
        if not self.ngspice:
            L.append(".param fscale(x)='x*1.5+0.1' fmix(a,b)='max(a,b)*0.8+min(a,b)*0.2'")
        for n in globs:
            L.append(f".param {n}={self.value(n, defined)}")
            defined.append(n)
        self.globs = globs
        # global models
        L.append(".model nch.1 nmos level=54 version=4.7 lmin=1e-10 lmax=6e-8 wmin=1e-10 wmax=4e-7")
        L.append(".model nch.2 nmos level=54 version=4.7 lmin=1e-10 lmax=6e-8 wmin=4e-7 wmax=1")
        L.append(".model nch.3 nmos level=54 version=4.7 lmin=6e-8 lmax=2.5e-7 wmin=1e-10 wmax=1e-6")
        L.append(".model nch.4 nmos level=54 version=4.7 lmin=6e-8 lmax=2.5e-7 wmin=1e-6 wmax=1")
        L.append(".model nch.5 nmos level=54 version=4.7 lmin=2.5e-7 lmax=1 wmin=1e-10 wmax=1")
        L.append(".model pch.1 pmos level=54 version=4.7 lmin=1e-10 lmax=1.2e-7 wmin=1e-10 wmax=1")
        L.append(".model pch.2 pmos level=54 version=4.7 lmin=1.2e-7 lmax=1 wmin=1e-10 wmax=1.5e-6")
        L.append(".model pch.3 pmos level=54 version=4.7 lmin=1.2e-7 lmax=1 wmin=1.5e-6 wmax=1")
        # limits read the global `lb`: evaluated at the top level
        L.append(".model gch.1 nmos level=54 version=4.7 lmin=1e-10 lmax='lb' wmin=1e-10 wmax=1")
        L.append(".model gch.2 nmos level=54 version=4.7 lmin='lb' lmax=1 wmin=1e-10 wmax=1")
        if not self.ngspice:
            L.append(".model fch.1 nmos level=54 version=4.7 lmin=1e-10 lmax=1 wmin=1e-10 wmax=5e-7 wnflag=1")
            L.append(".model fch.2 nmos level=54 version=4.7 lmin=1e-10 lmax=1 wmin=5e-7 wmax=1 wnflag=1")
        # subckts by level
        levels = rng.randint(3, 5)
        self.subckts = []  # (name, level, defaults)
        for lev in range(levels):
            for i in range(rng.randint(2, 4)):
                self.subckt(f"c{lev}_{i}", lev)
        # the top level
        top = [s for s in self.subckts if s[1] >= levels - 2]
        for i in range(rng.randint(3, 6)):
            self.instance(f"X{i}", rng.choice(top), self.globs)
        for i in range(rng.randint(1, 3)):
            self.mosfet(f"M{i}", self.globs, None)
        L.append("V1 d 0 1")
        L.append("V2 g 0 1")
        L.append("V3 s 0 0")
        L.append(".end")
        return "\n".join(L) + "\n"

    def mosfet(self, name, names, local_model):
        rng = self.rng
        fams = ["nch", "pch", "gch"] + ([] if self.ngspice else ["fch"])
        model = local_model if local_model and rng.random() < 0.5 else rng.choice(fams)
        l = self.length(names, "l")
        w = self.length(names, "w")
        parts = [f"l={l}", f"w={w}"]
        if rng.random() < 0.5:
            nf = rng.choice(["nfv", "2", "'max(1,nfv-1)'", "4"]) if "nfv" in names else rng.choice(["2", "3"])
            parts.append(f"nf={nf}")
        if rng.random() < 0.3:
            parts.append("m=2")
        if rng.random() < 0.2:
            parts.append(f"wnflag={rng.choice([0, 1])}")
        rng.shuffle(parts)
        self.lines.append(f"{name} d g s s {model} {' '.join(parts)}")

    def instance(self, name, sub, names):
        rng = self.rng
        sname, _, defaults, inside = sub
        passed = [n for n in defaults if rng.random() < 0.5]
        # ngspice computes a passed value with the child's own parameters
        # hiding those of the instance's level (HSPICE: at the instance's
        # level): its decks pass no expression of a name the child defines
        here = names
        if self.ngspice:
            names = [n for n in names if n not in defaults and n not in inside]
        vals = []
        for n in passed:
            if rng.random() < 0.5 and n in here:
                # the same name, read where the instance is
                vals.append(f"{n}={n}" if rng.random() < 0.5 else f"{n}='{n}*{num(rng, 0.6, 1.6)}'")
            else:
                vals.append(f"{n}={self.value(n, names, own=False)}")
        nodes = "d g s" if name[0] in "Xx" and rng.random() < 0.8 else "d g 0"
        self.lines.append(f"{name} {nodes} {sname} {' '.join(vals)}".rstrip())

    def nested(self, name, outer_visible, outer_model):
        """A subckt defined inside the current one: its own defaults (some
        hiding the enclosing subckt's names), MOSFETs that may use the
        enclosing subckt's local model"""
        rng = self.rng
        L = self.lines
        defaults = rng.sample(POOL, rng.randint(0, 3))
        outer = [n for n in outer_visible if n not in defaults]
        dvals = [f"{n}={self.value(n, outer + defaults[:i])}" for i, n in enumerate(defaults)]
        L.append(f".subckt {name} d g s {' '.join(dvals)}".rstrip())
        visible = outer + defaults
        for i in range(rng.randint(1, 3)):
            self.mosfet(f"M{i}", visible, outer_model)
        L.append(f".ends {name}")
        return (name, -1, defaults, [])

    def subckt(self, name, lev):
        rng = self.rng
        L = self.lines
        # defaults: names of the pool; in decks that do not define every name
        # at the top, the names a subckt reads are global or its own
        defaults = rng.sample(POOL, rng.randint(0, 4))
        # .param inside: names that are not defaults here
        params = rng.sample([p for p in POOL if p not in defaults], rng.randint(0, 2))
        # each definition reads the names already complete: those of the
        # levels above that this one does not redefine, and the definitions
        # before it here (so that no order of evaluation matters)
        outer = [n for n in self.globs if n not in defaults and n not in params]
        dvals = []
        for i, n in enumerate(defaults):
            dvals.append(f"{n}={self.value(n, outer + defaults[:i])}")
        L.append(f".subckt {name} d g s {' '.join(dvals)}".rstrip())
        for i, n in enumerate(params):
            L.append(f".param {n}={self.value(n, outer + defaults + params[:i])}")
        visible = outer + defaults + params
        local_model = None
        if rng.random() < 0.3:
            local_model = f"lm{lev}{name[-1]}"
            d = [n for n in visible if n in DIMLESS]
            e = self.dimless(d)
            L.append(f".model {local_model}.1 nmos level=54 version=4.7 lmin=1e-10 lmax='({e})*50n' wmin=1e-10 wmax=1")
            L.append(f".model {local_model}.2 nmos level=54 version=4.7 lmin='({e})*50n' lmax=1 wmin=1e-10 wmax=1")
        nested = None
        if lev >= 1 and rng.random() < 0.25:
            nested = self.nested(f"n{name}", visible, local_model)
        for i in range(rng.randint(1 if lev == 0 else 0, 3)):
            self.mosfet(f"M{i}", visible, local_model)
        if nested:
            for i in range(rng.randint(1, 2)):
                self.instance(f"XN{i}", nested, visible)
        lower = [s for s in self.subckts if s[1] < lev]
        if lower:
            for i in range(rng.randint(1, 3)):
                self.instance(f"X{i}", rng.choice(lower), visible)
        L.append(f".ends {name}")
        self.subckts.append((name, lev, defaults, params))


def main():
    seed = int(sys.argv[1])
    out = sys.argv[2]
    parhier = "local"
    if "--parhier" in sys.argv:
        parhier = sys.argv[sys.argv.index("--parhier") + 1]
    ngspice = "--ngspice" in sys.argv
    text = Gen(seed, parhier, ngspice).generate()
    with open(out, "w") as f:
        f.write(text)


if __name__ == "__main__":
    main()
