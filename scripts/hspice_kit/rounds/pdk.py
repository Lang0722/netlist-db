#!/usr/bin/env python3
"""Seeded random PDK-style decks. usage: pdk.py OUTDIR FIRST_SEED COUNT

Each deck has a corner library with nested `.lib` calls, binned models (plain,
FinFET, local to a wrapper), wrapper subckts with `.if`, a hierarchy with passed
values and defaults, `.param` chains and functions, and mixed line forms: `+`
lines, comments, quotes, short commands, letter case, CRLF. Nothing here knows
what a result should be: HSPICE is the reference.
"""
import os, random, shutil, sys

TAIL = ".option list nomod post=0\n.end\n"
CMD_PARAM = [".param", ".PARAM", ".Param", ".para", ".parameters", ".pa"]
CMD_MODEL = [".model", ".MODEL", ".Model", ".mod", ".mo"]
CMD_SUBCKT = [".subckt", ".SUBCKT", ".subc", ".su"]
CMD_ENDS = [".ends", ".ENDS", ".eom"]
CMD_OPT = [".option", ".options", ".OPTION", ".opt"]
CMD_INC = [".inc", ".include", ".INCLUDE", ".incl"]


def number(r, x):
    """`x` (a size in metres) written in one of the forms of a number"""
    forms = [
        lambda: f"{x:.6g}",
        lambda: f"{x:.6e}",
        lambda: f"{x * 1e6:.6f}u",
        lambda: f"{x * 1e6:.6f}U",
        lambda: f"{x * 1e9:.4f}n",
        lambda: f"{x * 1e9:.4f}N",
        lambda: f"{x * 1e3:.9f}m",
        lambda: f"{x * 1e12:.2f}p",
    ]
    return r.choice(forms)()


class Deck:
    def __init__(self, seed, prefix):
        self.r = random.Random(seed)
        self.seed = seed
        self.prefix = prefix
        self.files = {}
        self.top = []  # lines of the main deck
        self.globals = {}  # top-level parameters: name -> size in metres
        self.ratios = {}  # top-level parameters that are plain ratios near 1
        self.scale = 1.0
        self.scalm = 1.0
        self.parhier = "local"

    def name(self, stem):
        return f"{self.prefix}{self.seed}_{stem}"

    # ---- values
    def size(self, x, names=()):
        """A value for a size `x` in metres, in the line's own units, as a text that
        can follow `key=`: a number, an expression or a name"""
        r = self.r
        x = x / self.scale
        kind = r.random()
        if kind < 0.45:
            return number(r, x)
        if kind < 0.60:
            return f"'{number(r, x)}'"
        if kind < 0.72 and self.ratios:
            k = r.choice(sorted(self.ratios))
            return f"'{number(r, x)}*{k}'"
        if kind < 0.80:
            a = r.uniform(0.2, 0.8)
            return f"'{number(r, x * a)}+{number(r, x * (1 - a))}'"
        if kind < 0.87:
            q = r.choice(["'", '"'])
            return f"{q}max({number(r, x)},{number(r, x / 2)}){q}"
        if kind < 0.93 and names:
            k = r.choice(names)
            return f"'{number(r, x)}*{k}*{r.uniform(0.5, 2):.3f}'"
        if kind < 0.95:
            return f"({number(r, x)}*1)"
        if kind < 0.97:
            return f"{number(r, x)}*1"
        return f"'abs({number(r, x)})*sqrt(4)/2'"

    def wrap(self, words):
        """A statement from `words`, over `+` lines at random, with comments"""
        r = self.r
        out = [words[0]]
        for w in words[1:]:
            k = r.random()
            if k < 0.08:
                out.append("\n+ " + w)
            elif k < 0.10:
                out.append("\n$ a comment\n+ " + w)
            elif k < 0.12:
                out.append("\n* a comment\n+ " + w)
            elif k < 0.13:
                out.append("\n\n+ " + w)
            else:
                out.append(" " + w)
        tail = r.random()
        if tail < 0.06:
            out.append(" $ note l=9u w=9u")
        elif tail < 0.08:
            out.append(" // note l=9u")
        return "".join(out)

    def param_line(self, items):
        r = self.r
        cmd = r.choice(CMD_PARAM)
        sep = r.choice([" ", " ", " ", ", ", ","])
        if sep == "," and any(v[0] not in "'\"" for _, v in items):
            sep = " "
        return cmd + " " + sep.join(f"{k}={v}" for k, v in items)

    # ---- the library
    def edges(self, low, high, n):
        """`n` bins between `low` and `high`"""
        r = self.r
        cuts = sorted(r.uniform(0, 1) for _ in range(n - 1))
        pts = [low] + [low * (high / low) ** c for c in cuts] + [high]
        return pts

    def limit(self, x):
        """A limit of a bin, in the units of the model cards. `.option scalm`
        scales these."""
        r = self.r
        x = x / self.scalm
        k = r.random()
        if k < 0.75:
            return f"{x:.6e}"
        if k < 0.9:
            return f"'{x:.6e}'"
        return f"'{x * 2:.6e}/2'"

    def bins(self, base, kind, level, nl, nw, extra):
        r = self.r
        lp = self.edges(1e-9, 1.0, nl)
        wp = self.edges(1e-9, 1.0, nw)
        cards = []
        k = 1
        order = [(i, j) for i in range(nl) for j in range(nw)]
        if r.random() < 0.3:
            r.shuffle(order)
        for i, j in order:
            words = [f"{r.choice(CMD_MODEL)} {base}.{k} {kind}", f"level={level}"]
            words += [f"lmin={self.limit(lp[i])}", f"lmax={self.limit(lp[i + 1])}"]
            words += [f"wmin={self.limit(wp[j])}", f"wmax={self.limit(wp[j + 1])}"]
            words += extra
            if r.random() < 0.2:
                words.insert(1, "(")
                words.append(")")
            cards.append(self.wrap(words))
            k += 1
        return cards

    def finfet_bins(self, base, kind, nl, nfins):
        r = self.r
        lp = self.edges(1e-9, 1.0, nl)
        fins = sorted(r.sample(range(2, 30), nfins - 1))
        fp = [0.5] + fins + [1000]
        cards = []
        k = 1
        for i in range(nl):
            for j in range(nfins):
                words = [f"{r.choice(CMD_MODEL)} {base}.{k} {kind}", "level=72"]
                words += [f"lmin={self.limit(lp[i])}", f"lmax={self.limit(lp[i + 1])}"]
                words += [f"nfinmin={fp[j]}", f"nfinmax={fp[j + 1]}"]
                cards.append(self.wrap(words))
                k += 1
        return cards

    def library(self):
        """A corner library file, and the call of one corner. Returns the model names"""
        r = self.r
        name = self.name("pdk.l")
        corners = ["tt", "ss", "ff", "sf", "fs"]
        corner = r.choice(corners)
        nch = r.choice(["nch", "NCH", "nch_lvt", "n1"])
        pch = r.choice(["pch", "PCH", "pch_hvt", "p1"])
        xl = r.choice(["", "", "xl='dlo'", "xl=dlo", "xl='dlo*1'"])
        xw = r.choice(["", "", "xw='dwo'", "xw=dwo"])
        extra = [x for x in (xl, xw) if x]
        lines = ["* the library " + name]
        for c in corners:
            dlo = {"tt": 0, "ss": 2e-9, "ff": -2e-9, "sf": 1e-9, "fs": -1e-9}[c]
            lines.append(f".lib {c}")
            lines.append(self.param_line([("dlo", f"{dlo:g}"), ("dwo", f"{dlo * 2:g}")]))
            lines.append(f".lib '{name}' core")
            if r.random() < 0.3:
                lines.append(f".lib '{name}' extra")
            lines.append(f".endl {c}")
        lines.append(".lib core")
        lines += self.bins(nch, "nmos", 54, r.randint(1, 4), r.randint(1, 3), extra)
        lines += self.bins(pch, "pmos", 54, r.randint(1, 3), r.randint(1, 3), extra)
        lines.append(f"{r.choice(CMD_MODEL)} plain nmos level=54 {' '.join(extra)}")
        # (with scalm, HSPICE changes the nfin limits of a bin after its first use)
        fin = self.scalm == 1.0 and r.random() < 0.4
        if fin:
            lines += self.finfet_bins("fin", "nmos", r.randint(1, 3), r.randint(1, 3))
        lines.append(".endl core")
        lines.append(".lib extra")
        lines.append(self.param_line([("corner_k", "1")]))
        lines.append(".endl extra")
        self.files[name] = "\n".join(lines) + "\n"
        call = r.choice([f".lib '{name}' {corner}", f'.lib "{name}" {corner}', f".LIB '{name}' {corner.upper()}", f".lib {name} {corner}"])
        self.top.append(call)
        return [nch, pch, "plain"] + (["fin"] if fin else [])

    # ---- MOSFET lines
    def mos(self, name, nodes, model, l, w, names=(), more=()):
        """A MOSFET line, with `l` and `w` given as sizes in metres"""
        r = self.r
        if model == "fin" and r.random() < 0.7:
            more = list(more) + [f"nfin={r.randint(1, 30)}"]
        ltext, wtext = self.size(l, names), self.size(w, names)
        first = ltext[1:-1] if ltext.startswith("(") else ltext
        kind = r.random()
        if r.random() < 0.15:
            # a later key against a value at a place (rounds 25 to 27)
            other = number(r, l / self.scale * 3)
            other_w = number(r, w / self.scale * 3)
            words = r.choice([
                [first, wtext, f"w={other_w}"],
                [first, wtext, f"l={other}"],
                [first, f"w={wtext}", f"w={other_w}"],
                [f"w={wtext}", f"w={other_w}", f"l={ltext}"],
                [f"l={other}", f"w={wtext}", f"l={ltext}"],
                [f"l={ltext}", wtext, f"w={other_w}"],
                [f"w={other_w}", wtext, f"l={ltext}"],
            ]) + list(more)
            return self.wrap(f"{name} {' '.join(nodes)} {model}".split() + words)
        if kind < 0.60:
            pairs = [f"{r.choice(['l', 'L'])}={ltext}", f"{r.choice(['w', 'W'])}={wtext}"]
            if r.random() < 0.3:
                pairs.reverse()
            pairs += list(more)
            words = pairs
        elif kind < 0.72:
            words = [first, wtext] + list(more)
        elif kind < 0.80:
            words = [first, f"w={wtext}"] + list(more)
        elif kind < 0.86:
            # a value at the place of W after a key
            words = [f"l={ltext}", wtext] + list(more)
        elif kind < 0.90:
            words = [f"l={ltext}", f"w={number(r, w / self.scale * 3)}", f"w={wtext}"] + list(more)
        elif kind < 0.94:
            words = [f"w={number(r, w / self.scale * 3)}", wtext, f"l={ltext}"] + list(more)
        elif kind < 0.97:
            words = [f"l={ltext}", f"w={wtext}", "OFF"] + list(more)
        else:
            words = [f"l={ltext}", "off", f"w={wtext}"] + list(more)
        head = f"{name} {' '.join(nodes)} {model}"
        return self.wrap(head.split() + words)

    def sizes(self):
        r = self.r
        return (10 ** r.uniform(-7.5, -5), 10 ** r.uniform(-6.7, -4.5))

    # ---- subckts
    def wrapper(self, name, model, level_if):
        """A wrapper around a binned model: `.subckt name d g s b l= w= ...`"""
        r = self.r
        l0, w0 = self.sizes()
        out = []
        ports = "d g s b"
        keys = [("ll", self.size(l0)), ("ww", self.size(w0))]
        if r.random() < 0.5:
            keys.append(("nn", "1"))
        out.append(self.wrap(f"{r.choice(CMD_SUBCKT)} {name} {ports}".split() + [f"{k}={v}" for k, v in keys]))
        has_nn = any(k == "nn" for k, _ in keys)
        d = r.uniform(-1e-9, 1e-9) / self.scale
        out.append(self.param_line([("dl", f"{d:.4e}"), ("le", "'ll+dl'")]))
        nf = ["nf=nn"] if has_nn else []
        if level_if and r.random() < 0.7:
            cut = 10 ** r.uniform(-7.5, -5) / self.scale
            out.append(r.choice([f".if (ll<{cut:.4e})", f".IF ('ll<{cut:.4e}')", f".if(ll<{cut:.4e})"]))
            out.append(self.wrap(f"M0 d g s b {model} l=le w=ww".split() + nf))
            if r.random() < 0.3:
                out.append(f".elseif (ww>{cut:.4e})")
                out.append(self.wrap(f"M0 d g s b {model} l='le*1.5' w=ww".split() + nf))
            out.append(".else")
            out.append(self.wrap(f"M0 d g s b {model} l='le*2' w='ww/2'".split() + nf))
            out.append(".endif")
        else:
            out.append(self.wrap(f"M0 d g s b {model} l=le w=ww".split() + nf))
            if r.random() < 0.3:
                out.append(self.wrap(f"M1 d g s b {model} 'le*2' 'ww*2'".split() + nf))
        out.append(r.choice(CMD_ENDS) + r.choice(["", f" {name}"]))
        return out, has_nn

    def local_model_wrapper(self, name):
        """A subckt with its own bins"""
        r = self.r
        l0, w0 = self.sizes()
        out = [self.wrap(f"{r.choice(CMD_SUBCKT)} {name} d g s b ll={self.size(l0)} ww={self.size(w0)}".split())]
        out += self.bins("lm", "nmos", 54, r.randint(1, 3), r.randint(1, 2), [])
        out.append(f"M0 d g s b lm l='ll' w='ww'")
        out.append(r.choice(CMD_ENDS))
        return out

    def blocks(self, wrappers, models):
        """A hierarchy of 2 to 4 levels"""
        r = self.r
        depth = r.randint(1, 3)
        made = []
        lines = []
        for level in range(depth):
            name = self.name(f"blk{level}").lower()
            shared = r.choice(["k", "ka", "scale_l"])
            keys = {shared: f"{r.uniform(0.5, 2):.3f}"}
            head = f"{r.choice(CMD_SUBCKT)} {name} a b c {shared}={keys[shared]}"
            lines.append(self.wrap(head.split()))
            names = [shared]
            if r.random() < 0.6:
                v = f"'{shared}*{r.uniform(0.5, 2):.3f}'"
                lines.append(self.param_line([("kl", v)]))
                names.append("kl")
            for i in range(r.randint(1, 4)):
                l, w = self.sizes()
                nodes = r.sample(["a", "b", "c", "0", "q1", "q2"], 4)
                pick = r.random()
                if pick < 0.5 and wrappers:
                    wn, has_nn = r.choice(wrappers)
                    args = [f"ll='{number(r, l / self.scale)}*{r.choice(names)}'", f"ww={self.size(w, names)}"]
                    if has_nn and r.random() < 0.5:
                        args.append(f"nn={r.randint(1, 4)}")
                    if r.random() < 0.15:
                        args = args[:1]
                    lines.append(self.wrap(f"X{i} {' '.join(nodes)} {wn}".split() + args))
                else:
                    model = r.choice(models)
                    lines.append(self.mos(f"M{i}", nodes, model, l, w, names))
            for j, (sub, subkey) in enumerate(made[-2:]):
                for t in range(r.randint(1, 3)):
                    nodes = r.sample(["a", "b", "c", "q1"], 3)
                    args = []
                    if r.random() < 0.7:
                        args.append(f"{subkey}='{r.choice(names)}*{r.uniform(0.5, 2):.3f}'")
                    lines.append(self.wrap(f"Xs{j}{t} {' '.join(nodes)} {sub}".split() + args))
            lines.append(r.choice(CMD_ENDS))
            made.append((name, shared))
        return lines, made

    # ---- the deck
    def build(self):
        r = self.r
        self.scale = r.choice([1.0, 1.0, 1.0, 1e-6, 2.0, 0.5])
        self.scalm = r.choice([1.0, 1.0, 1.0, 1e-6, 2.0])
        self.parhier = r.choice(["local", "local", "local", "global"])
        self.top.append(f"* deck {self.seed}: scale {self.scale} scalm {self.scalm} parhier {self.parhier}")
        opts = []
        if self.scale != 1.0:
            opts.append(f"scale={self.scale:g}")
        if self.scalm != 1.0:
            opts.append(f"scalm={self.scalm:g}")
        if self.parhier != "local" or r.random() < 0.2:
            opts.append(f"parhier={self.parhier}")
        if r.random() < 0.2:
            opts.append("defl=1u defw=2u")
        if r.random() < 0.15:
            opts.append(r.choice(["wnflag=1", "wnflag=0", "wnflag"]))
        if opts:
            self.top.append(f"{r.choice(CMD_OPT)} {' '.join(opts)}")
        # top-level parameters
        for k in ["k", "ka", "scale_l", "gl", "ratio_a", "ratio_b"]:
            if r.random() < 0.5:
                v = r.uniform(0.5, 2)
                self.ratios[k] = v
                self.top.append(self.param_line([(k, f"{v:.4f}")]))
        if r.random() < 0.3:
            self.top.append(".param ff(x,y)='x*y+1e-9'")
            self.top.append(f".param via_f='ff(1e-6,0.5)'")
        models = self.library()
        models = [m for m in models]
        wrappers = []
        wrap_lines = []
        for i in range(r.randint(0, 3)):
            name = f"{r.choice(['nch', 'pch', 'dev'])}_mac{i}"
            lines, has_nn = self.wrapper(name, r.choice(models), True)
            wrap_lines += lines
            wrappers.append((name, has_nn))
        if r.random() < 0.4:
            name = "local_dev"
            wrap_lines += self.local_model_wrapper(name)
            wrappers.append((name, False))
        block_lines, made = self.blocks(wrappers, models)
        # the wrappers go to an include file, or not
        placed = wrap_lines + block_lines
        how = r.random()
        if how < 0.4:
            inc = self.name("cells.inc")
            self.files[inc] = "\n".join(placed) + "\n"
            self.top.append(f"{r.choice(CMD_INC)} '{inc}'")
        elif how < 0.6:
            sub = self.name("d") + "/" + self.name("cells.inc")
            inner = self.name("inner.inc")
            self.files[sub] = "\n".join(wrap_lines) + "\n.inc '" + inner + "'\n"
            self.files[self.name("d") + "/" + inner] = "\n".join(block_lines) + "\n"
            self.top.append(f"{r.choice(CMD_INC)} \"{sub}\"")
        else:
            self.top += placed
        # calls at the top
        names = sorted(self.ratios)
        for i in range(r.randint(1, 4)):
            sub, key = r.choice(made)
            args = []
            if r.random() < 0.6:
                args.append(f"{key}={r.uniform(0.5, 2):.3f}")
            self.top.append(self.wrap(f"X{i} d g s {sub}".split() + args))
        for i in range(r.randint(0, 3)):
            l, w = self.sizes()
            self.top.append(self.mos(f"Mt{i}", ["d", "g", "s", "0"], r.choice(models), l, w, names))
        # a block of `.if` at the top
        if r.random() < 0.4:
            c = r.choice(names) if names else "1"
            l, w = self.sizes()
            self.top.append(f".if ({c}>1)")
            self.top.append(self.param_line([("branch_l", number(r, l / self.scale))]))
            self.top.append(".else")
            self.top.append(self.param_line([("branch_l", number(r, l * 2 / self.scale))]))
            self.top.append(".endif")
            self.top.append(self.mos("Mb", ["d", "g", "s", "0"], r.choice(models), l, w, names))
            self.top[-1] = self.top[-1].replace(" 0 ", " 0 ", 1)
        self.top.append("Vd d 0 1")
        self.top.append("Vg g 0 1")
        self.top.append("Vs s 0 0")
        text = "\n".join(self.top) + "\n" + TAIL
        if r.random() < 0.15:
            text = text.replace("\n", "\r\n")
        return text


def main():
    out, first, count = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    prefix = sys.argv[4] if len(sys.argv) > 4 else "s"
    if os.path.exists(out):
        shutil.rmtree(out)
    os.makedirs(os.path.join(out, "decks"))
    for seed in range(first, first + count):
        d = Deck(seed, prefix)
        text = d.build()
        with open(os.path.join(out, "decks", f"{prefix}{seed}.sp"), "w", newline="") as f:
            f.write(text)
        for rel, content in d.files.items():
            path = os.path.join(out, "decks", rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", newline="") as f:
                f.write(content)
    print(count, "decks")


if __name__ == "__main__":
    main()
