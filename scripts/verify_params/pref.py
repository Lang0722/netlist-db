#!/usr/bin/env python3
"""Reference model selector: flattens an HSPICE deck (the subset that
pgen.py writes) instance by instance, resolving every parameter, and chooses
each MOSFET's model. Written separately from netlist-db.

usage: pref.py DECK.sp [--subckt NAME] [--parhier local|global]

Output (TSV, like `model_select`): path, L, W, NF, model or `error: ...`

Rules (HSPICE, as measured with scripts/hspice_kit):
- A level (the top, or a subckt instance) binds parameters: the .subckt
  line's defaults, then its .param statements (a later definition of a name
  replaces an earlier one), then the values passed on its instance line.
- A name is resolved among the levels that bind it, on the instance path:
  with parhier=local, the closest to the reader; with parhier=global, the
  closest to the top, also over a value passed below it. The text that wins
  is evaluated at the level that holds it: the text of a value passed in the
  new instance. The values passed are computed first, in the order of the
  line. A text that reads its own name reads that name at the level above. A
  value passed whose text leads back to it through other names is evaluated,
  as a whole, at the level above. Any other loop is an error.
- Functions (.param f(x)=...): the same order chooses the definition; the
  names and the calls in its body are resolved at the level that holds it.
- A model: at each level on the instance path, from the transistor's own up,
  the model of the transistor's name, else the first bin base.suffix that
  holds L and W (W/NF, unless WNFLAG=0 on the instance line or, without one,
  .option wnflag=0); a bin holds them if LMIN <= L < LMAX and WMIN <= W < WMAX,
  a value at most 2e-15 (relative) below a limit counting as on it. The limits
  are evaluated at the level that defines the model. A bin needs its four
  limits. A name with a dot is not named on an instance line; a name with two
  dots is of no use.
- Subckts are looked up along the instance path too.

Not followed (as in netlist-db): the first MOSFET HSPICE makes that uses a
name evaluates the limits in its own instance (see src/models.rs).
"""
import math
import os
import re
import sys

# --------------------------------------------------------------------------
# statements
# --------------------------------------------------------------------------


def statements(text):
    out = []
    for raw in text.splitlines():
        line = raw.rstrip()
        # a `$` comment, outside quotes
        q = False
        for i, ch in enumerate(line):
            if ch == "'":
                q = not q
            elif ch == "$" and not q:
                line = line[:i]
                break
        t = line.strip()
        if not t or t.startswith("*"):
            continue
        if t.startswith("+") and out:
            out[-1] += " " + t[1:]
        else:
            out.append(t)
    return out


def tokens(stmt):
    """Words, a quoted expression being one word; `k = v` joined as `k=v`"""
    toks = re.findall(r"[^\s=']+='[^']*'|'[^']*'|[^\s=]+=|=|[^\s]+", stmt)
    out = []
    for t in toks:
        if t == "=" and out:
            out[-1] += "="
        elif out and out[-1].endswith("=") and "=" not in out[-1][:-1] and not t.startswith("="):
            out[-1] += t
        else:
            out.append(t)
    return out


def kv(tok):
    k, v = tok.split("=", 1)
    if v.startswith("'") and v.endswith("'"):
        v = v[1:-1]
    return k.lower(), v


# --------------------------------------------------------------------------
# expressions
# --------------------------------------------------------------------------

# (by the letters that start the suffix: the first that matches)
SCALE = [("min", 60.0), ("me", 1e6), ("mi", 25.4e-6), ("hr", 3600.0), ("day", 86400.0), ("yr", 31536000.0),
         ("t", 1e12), ("g", 1e9), ("x", 1e6), ("k", 1e3), ("m", 1e-3), ("u", 1e-6), ("n", 1e-9), ("p", 1e-12),
         ("f", 1e-15), ("a", 1e-18), ("z", 1e-21), ("y", 1e-24)]

TOKEN = re.compile(r"\s*(?:(\d+\.?\d*(?:[eE][+-]?\d+)?|\.\d+(?:[eE][+-]?\d+)?)([a-zA-Z]*)"
                   r"|([a-zA-Z_][\w]*)|(\*\*|==|!=|>=|<=|&&|\|\||[-+*/^()?:,<>!]))")


def number(digits, suffix):
    v = float(digits)
    s = suffix.lower()
    # (an exponent makes the suffix void)
    if "e" in digits.lower():
        return v
    if s.startswith("db"):
        return 10.0 ** (v / 20)
    for name, f in SCALE:
        if s.startswith(name):
            return v * f
    return v


class Expr:
    def __init__(self, text):
        self.toks = []
        pos = 0
        text = text.strip()
        while pos < len(text):
            m = TOKEN.match(text, pos)
            if not m or m.end() == pos:
                raise ValueError(f"bad expression {text!r} at {pos}")
            pos = m.end()
            if m.group(1) is not None:
                self.toks.append(("num", number(m.group(1), m.group(2))))
            elif m.group(3) is not None:
                self.toks.append(("name", m.group(3).lower()))
            else:
                self.toks.append(("op", m.group(4)))
        self.i = 0

    def peek(self):
        return self.toks[self.i] if self.i < len(self.toks) else ("end", None)

    def take(self, op=None):
        t = self.peek()
        if op is not None and t != ("op", op):
            raise ValueError(f"expected {op}, got {t}")
        self.i += 1
        return t

    def parse(self):
        e = self.cond()
        if self.peek()[0] != "end":
            raise ValueError(f"trailing {self.peek()}")
        return e

    def cond(self):
        c = self.orr()
        if self.peek() == ("op", "?"):
            self.take()
            a = self.cond()
            self.take(":")
            b = self.cond()
            return ("?", c, a, b)
        return c

    def binary(self, sub, ops):
        left = sub()
        while self.peek()[0] == "op" and self.peek()[1] in ops:
            op = self.take()[1]
            left = (op, left, sub())
        return left

    def orr(self):
        return self.binary(self.andd, ("||",))

    def andd(self):
        return self.binary(self.eq, ("&&",))

    def eq(self):
        return self.binary(self.rel, ("==", "!="))

    def rel(self):
        return self.binary(self.add, ("<", ">", "<=", ">="))

    def add(self):
        return self.binary(self.mul, ("+", "-"))

    def mul(self):
        return self.binary(self.unary, ("*", "/"))

    def unary(self):
        if self.peek() in (("op", "-"), ("op", "+"), ("op", "!")):
            op = self.take()[1]
            return ("u" + op, self.unary())
        return self.power()

    def power(self):
        # (to the left: 2**3**2 is 64)
        base = self.atom()
        while self.peek() in (("op", "**"), ("op", "^")):
            self.take()
            base = ("**", base, self.exponent())
        return base

    def exponent(self):
        """A sign right after a power takes the next operand only"""
        if self.peek() in (("op", "-"), ("op", "+"), ("op", "!")):
            op = self.take()[1]
            return ("u" + op, self.exponent())
        return self.atom()

    def atom(self):
        t = self.take()
        if t[0] == "num":
            return ("num", t[1])
        if t == ("op", "("):
            e = self.cond()
            self.take(")")
            return e
        if t[0] == "name":
            if self.peek() == ("op", "("):
                self.take()
                args = []
                if self.peek() != ("op", ")"):
                    args.append(self.cond())
                    while self.peek() == ("op", ","):
                        self.take()
                        args.append(self.cond())
                self.take(")")
                return ("call", t[1], args)
            return ("var", t[1])
        raise ValueError(f"unexpected {t}")


def power(x, y):
    """A zero base: 0; a negative base: the integer part of the exponent"""
    if x == 0:
        return 0.0
    return x ** math.trunc(y) if x < 0 else x ** y


FUNCS = {
    "sqrt": lambda x: math.copysign(math.sqrt(abs(x)), x), "abs": abs, "exp": math.exp,
    "log": lambda x: math.copysign(1, x) * math.log(abs(x)), "ln": lambda x: math.copysign(1, x) * math.log(abs(x)),
    "log10": lambda x: math.copysign(1, x) * math.log10(abs(x)), "floor": math.floor, "ceil": math.ceil,
    "max": max, "min": min,
    # HSPICE: x to the integer part of y
    "pow": lambda x, y: x ** math.trunc(y),
}


def evaluate(tree, level, local=None):
    """`level`: the level that reads; `local`: the arguments of the function
    being evaluated"""
    op = tree[0]
    if op == "num":
        return tree[1]
    if op == "var":
        if local and tree[1] in local:
            return local[tree[1]]
        return level.get(tree[1])
    if op == "call":
        args = [evaluate(a, level, local) for a in tree[2]]
        found = level.func(tree[1])
        if found is not None:
            holder, (names, body) = found
            return evaluate(tree_of(body), holder, dict(zip(names, args)))
        return float(FUNCS[tree[1]](*args))
    if op == "?":
        return evaluate(tree[2] if evaluate(tree[1], level, local) != 0 else tree[3], level, local)
    if op == "u-":
        return -evaluate(tree[1], level, local)
    if op == "u+":
        return evaluate(tree[1], level, local)
    if op == "u!":
        return float(evaluate(tree[1], level, local) == 0)
    a = evaluate(tree[1], level, local)
    b = evaluate(tree[2], level, local)
    return {
        "+": lambda: a + b, "-": lambda: a - b, "*": lambda: a * b, "/": lambda: a / b,
        "**": lambda: power(a, b), "<": lambda: float(a < b), ">": lambda: float(a > b),
        "<=": lambda: float(a <= b), ">=": lambda: float(a >= b), "==": lambda: float(a == b),
        "!=": lambda: float(a != b), "&&": lambda: float(a != 0 and b != 0),
        "||": lambda: float(a != 0 or b != 0),
    }[op]()


PARSED = {}


def tree(text):
    if text not in PARSED:
        PARSED[text] = Expr(text).parse()
    return PARSED[text]


tree_of = tree


# --------------------------------------------------------------------------
# the netlist
# --------------------------------------------------------------------------


class Model:
    def __init__(self, name, params):
        self.name = name
        self.params = params  # lowercase key -> text (last one wins)


class Def:
    """The top level, or a subckt definition"""

    def __init__(self, name):
        self.name = name
        self.defaults = []  # (name, text) of the .subckt line
        self.params = []  # (name, text) of .param statements
        self.funcs = {}  # name -> (arguments, body) of .param f(a, b)='...'
        self.models = []
        self.elements = []  # ('M', name, model, {k: text}) or ('X', name, sub, [(k, text)])
        self.subckts = {}
        self._index = None

    def index(self):
        """The models without a dot by name (the last of a name), and the
        bins `base.suffix` of each base name, in netlist order"""
        if self._index is None:
            exact, by_base = {}, {}
            for m in self.models:
                base, dot, suffix = m.name.partition(".")
                if not dot:
                    exact[m.name] = m
                elif base and suffix and "." not in suffix:
                    by_base.setdefault(base, []).append(m)
            self._index = (exact, by_base)
        return self._index


def section_text(path, section):
    """The lines of `.lib section` ... `.endl` of the file `path` (the last
    such section)"""
    found, cur = None, None
    for line in open(path, encoding="utf-8", errors="replace").read().splitlines():
        t = line.strip().split()
        if cur is None:
            if len(t) >= 2 and t[0].lower() == ".lib" and t[1].lower() == section.lower():
                cur = []
        elif t and t[0].lower().startswith(".endl"):
            found, cur = cur, None
        else:
            cur.append(line)
    if found is None:
        raise ValueError(f"no section {section} in {path}")
    return "\n".join(found)


def expand(path, section=None, top_file=False):
    """The statements of a file (or of one of its `.lib` sections), with
    those of the files it includes and the sections it loads; a top-level
    file ends at `.alter`"""
    base = os.path.dirname(os.path.abspath(path))
    text = section_text(path, section) if section else open(path, encoding="utf-8", errors="replace").read()
    skipping = False
    for st in statements(text):
        toks = tokens(st)
        head = toks[0].lower()
        if skipping:
            if head.startswith(".endl"):
                skipping = False
            continue
        if head in (".inc", ".include"):
            yield from expand(os.path.join(base, toks[1].strip("'\"")))
        elif head == ".lib":
            if len(toks) >= 3:
                yield from expand(os.path.join(base, toks[1].strip("'\"")), toks[2])
            else:
                # a section definition, not loaded here
                skipping = True
        elif head == ".alter" and top_file:
            return
        else:
            yield st


def read(path):
    top = Def(None)
    options = {}
    stack = [top]
    for st in expand(path, top_file=True):
        toks = tokens(st)
        head = toks[0].lower()
        cur = stack[-1]
        if head == ".end":
            break
        if head == ".option" or head == ".options":
            for t in toks[1:]:
                if "=" in t:
                    k, v = kv(t)
                    options[k] = v.lower()
        elif head == ".param":
            for t in toks[1:]:
                k, v = kv(t)
                f = re.fullmatch(r"(\w+)\((.*)\)", k)
                if f:
                    cur.funcs[f.group(1)] = ([a.strip() for a in f.group(2).split(",")], v)
                else:
                    cur.params.append((k, v))
        elif head == ".subckt":
            d = Def(toks[1].lower())
            d.defaults = [kv(t) for t in toks[2:] if "=" in t]
            cur.subckts[d.name] = d
            stack.append(d)
        elif head == ".ends":
            stack.pop()
        elif head == ".model":
            params = {}
            for t in toks[3:]:
                if "=" in t:
                    k, v = kv(t)
                    params[k] = v
            cur.models.append(Model(toks[1].lower(), params))
        elif head[0] == "m":
            params = dict(kv(t) for t in toks[1:] if "=" in t)
            words = [t for t in toks[1:] if "=" not in t]
            # the numbers after the model name: L, then W (a keyword wins)
            model = max(i for i, t in enumerate(words) if not re.match(r"[-+.\d]", t))
            for key, value in zip(("l", "w"), words[model + 1:]):
                params.setdefault(key, value)
            cur.elements.append(("M", toks[0], words[model].lower(), params))
        elif head[0] == "x":
            words = [t for t in toks[1:] if "=" not in t]
            passed = [kv(t) for t in toks[1:] if "=" in t]
            cur.elements.append(("X", toks[0], words[-1].lower(), passed))
    return top, options


class Recursive(Exception):
    pass


class Level:
    """The parameters of one level: the top, or a subckt instance"""

    def __init__(self, parent, d, passed, parhier):
        self.parent = parent
        self.parhier = parhier
        self.defs = {}
        for k, v in d.defaults:
            self.defs[k] = v
        for k, v in d.params:
            self.defs[k] = v
        self.funcs = d.funcs
        self.passed = dict(passed)  # name -> text
        self.cache = {}
        self.active = []
        # the values passed, in the order of the line
        for name in self.passed:
            try:
                self.get(name)
            except Exception:  # noqa: BLE001 - an error matters when it is read
                pass

    def chain(self):
        lv = self
        while lv is not None:
            yield lv
            lv = lv.parent

    def choose(self, levels):
        """Among `levels`, from this level up, the one that parhier takes"""
        if not levels:
            return None
        return levels[0] if self.parhier == "local" else levels[-1]

    def find(self, name):
        return self.choose([lv for lv in self.chain() if name in lv.passed or name in lv.defs])

    def get(self, name):
        lv = self.find(name)
        if lv is not None:
            try:
                return lv.value_here(name)
            except Recursive as e:
                # a text that reads its own name: the name at the level above
                above = self.parent.find(name) if self.parent else None
                if e.args[0] == name and self.active[-1:] == [name] and above is not None:
                    return above.value_here(name)
                raise
        if name == "pi":
            return math.pi
        raise KeyError(f"undefined parameter {name}")

    def func(self, name):
        """(the level that holds the function, the function), or None"""
        lv = self.choose([lv for lv in self.chain() if name in lv.funcs])
        return None if lv is None else (lv, lv.funcs[name])

    def value_here(self, name):
        if name in self.cache:
            v = self.cache[name]
            if isinstance(v, Exception):
                raise v
            return v
        if name in self.active:
            raise Recursive(name)
        self.active.append(name)
        looped = None
        try:
            v = self.eval(self.passed[name] if name in self.passed else self.defs[name])
        except Recursive as e:
            looped = e
        except Exception as e:  # noqa: BLE001 - the error is the value
            v = e
        self.active.pop()
        if looped is not None:
            # a value passed whose text leads back to it: as a whole, above
            if looped.args[0] != name or name not in self.passed or self.parent is None:
                raise looped  # (not kept: read later, this name may have a value)
            try:
                v = self.parent.eval(self.passed[name])
            except Exception:  # noqa: BLE001 - the error is the value
                v = Recursive(name)
                raise v
        self.cache[name] = v
        if isinstance(v, Exception):
            raise v
        return v

    def eval(self, text):
        v = evaluate(tree(text), self)
        if not math.isfinite(v):
            raise ValueError(f"not finite: {text}")
        return v


NGSPICE_RULE = "--ngspice-rule" in sys.argv


def within(x, lo, hi):
    if NGSPICE_RULE:
        # ngspice (inpgmod.c in_range): within 1e-9 (absolute) of either
        # limit, or strictly inside
        eq = lambda a, b: b is not None and abs(a - b) < 1e-9  # noqa: E731
        return eq(x, lo) or eq(x, hi) or ((lo is None or lo < x) and (hi is None or x < hi))
    tol = lambda v: abs(v) * 2e-15  # noqa: E731
    return x >= lo - tol(lo) and x < hi - tol(hi)


LIMITS = {}


OPTIONS = {}


def choose(mname, l, w, nf, scopes, inst_wnflag=None):
    """`scopes`: (definition, level) on the instance path, the transistor's
    own first"""
    name = mname.lower()
    seen = 0
    # the instance's WNFLAG, else .option wnflag, else the width per finger
    flag = float(OPTIONS.get("wnflag", "1")) != 0 if inst_wnflag is None else inst_wnflag != 0
    weff = w / nf if flag and nf > 0 else w
    for d, lv in scopes:
        exact, by_base = d.index()
        if name in exact:
            return exact[name].name
        bins = by_base.get(name, [])
        if any(k not in m.params for m in bins for k in ("lmin", "lmax", "wmin", "wmax")):
            return f"error: a bin of {name} without its four limits"
        # ngspice tries the models last defined first
        for m in (reversed(bins) if NGSPICE_RULE else bins):
            # (the level is kept with its limits: its id stays its own)
            key = (id(m), id(lv))
            if key not in LIMITS:
                lim = {k: lv.eval(m.params[k]) for k in ("lmin", "lmax", "wmin", "wmax")}
                LIMITS[key] = (lv, lim)
            lim = LIMITS[key][1]
            if within(l, lim["lmin"], lim["lmax"]) and within(weff, lim["wmin"], lim["wmax"]):
                return m.name
        seen += len(bins)
    return f"error: no bin of {name}" if seen else f"error: no model {name}"


def walk(d, lv, scopes, path, out):
    """`scopes`: (definition, level) on the instance path, `d`'s own first"""
    for el in d.elements:
        if el[0] == "M":
            _, name, mname, params = el
            p = path + [name]
            try:
                l = lv.eval(params["l"]) if "l" in params else 1e-4
                w = lv.eval(params["w"]) if "w" in params else 1e-4
                nf = lv.eval(params["nf"]) if "nf" in params else 1.0
                wnflag = lv.eval(params["wnflag"]) if "wnflag" in params else None
                model = choose(mname, l, w, nf, scopes, wnflag)
                out.append((".".join(p), l, w, nf, model))
            except Exception as e:  # noqa: BLE001
                out.append((".".join(p), None, None, None, f"error: {e}"))
        else:
            _, name, sub, passed = el
            # the definition: the nearest on the instance path
            sd = next((dd.subckts[sub] for dd, _ in scopes if sub in dd.subckts), None)
            if sd is None:
                continue
            child = Level(lv, sd, passed, lv.parhier)
            walk(sd, child, [(sd, child)] + scopes, path + [name], out)


def main():
    path = sys.argv[1]
    top, options = read(path)
    OPTIONS.update(options)
    parhier = options.get("parhier", "global")
    if "--parhier" in sys.argv:
        parhier = sys.argv[sys.argv.index("--parhier") + 1]
    root = Level(None, top, {}, parhier)
    out = []
    if "--subckt" in sys.argv:
        name = sys.argv[sys.argv.index("--subckt") + 1].lower()
        sd = top.subckts[name]
        child = Level(root, sd, {}, parhier)
        walk(sd, child, [(sd, child), (top, root)], [], out)
    else:
        walk(top, root, [(top, root)], [], out)
    print("path\tL\tW\tNF\tmodel")
    for p, l, w, nf, m in out:
        print(f"{p}\t{l!r}\t{w!r}\t{nf!r}\t{m}")


if __name__ == "__main__":
    main()
