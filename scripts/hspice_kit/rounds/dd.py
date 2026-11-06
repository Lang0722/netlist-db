#!/usr/bin/env python3
"""Reduces a deck through HSPICE: removes statements while the listing still matches
a pattern. usage: dd.py DECK.sp REGEX OUTDIR"""
import os, re, shutil, subprocess, sys
S = os.environ['NDB_ROUNDS']

def section(text, name):
    lines = text.splitlines()
    out, on = [], False
    for x in lines:
        t = x.strip().lower()
        if not on and re.fullmatch(rf"\.lib\s+{re.escape(name.lower())}", t):
            on = True
            continue
        if on and t.startswith(".endl"):
            break
        if on:
            out.append(x)
    return out

def flatten(path, lines=None):
    folder = os.path.dirname(path)
    if lines is None:
        lines = open(path, errors="replace", newline="").read().replace("\r\n", "\n").split("\n")
    out = []
    for x in lines:
        m = re.match(r"(?i)\s*\.inc[a-z]*\s+['\"]?([^'\"\s]+)", x)
        l = re.match(r"(?i)\s*\.lib\s+['\"]?([^'\"\s]+)['\"]?\s+(\S+)", x)
        if m:
            p = os.path.join(folder, m.group(1))
            out += flatten(p)
        elif l:
            p = os.path.join(folder, l.group(1))
            out += flatten(p, section(open(p, errors="replace").read(), l.group(2)))
        else:
            out.append(x)
    return out

def statements(lines):
    out, cur = [], []
    for x in lines:
        s = x.strip()
        if cur and (s.startswith("+") or s.startswith("*") or s.startswith("$") or s == ""):
            cur.append(x)
        else:
            if cur:
                out.append(cur)
            cur = [x]
    if cur:
        out.append(cur)
    return out

def run(kit, variants, regex):
    """name -> True if the listing of this variant matches"""
    root = os.path.join(S, kit)
    if os.path.exists(root):
        shutil.rmtree(root)
    os.makedirs(root + "/decks")
    for name, st in variants.items():
        with open(f"{root}/decks/{name}.sp", "w") as f:
            f.write("* dd\n" + "\n".join(x for s in st for x in s) + "\n")
    q = lambda *a: subprocess.run(a, capture_output=True, text=True)
    q(S + "/hard/vm.sh", "push", kit, root)
    q(S + "/hard/vm.sh", "run", kit, "4", "120")
    q(S + "/hard/vm.sh", "back", kit, root)
    res = {}
    for name in variants:
        p = f"{root}/results/{name}.lis"
        res[name] = os.path.exists(p) and bool(re.search(regex, open(p, errors="replace").read()))
    return res

def main():
    global KEEP
    deck, regex, kit = sys.argv[1], sys.argv[2], sys.argv[3]
    KEEP = sys.argv[4] if len(sys.argv) > 4 else None
    state = os.path.join(S, kit + ".state")
    if os.path.exists(state):
        st = statements(open(state).read().split("\n"))
        print(len(st), "statements from the state file", flush=True)
    else:
        lines = flatten(deck)
        # the title stays out: `run` writes one
        st = statements(lines[1:])
        print(len(st), "statements after the flattening", flush=True)
        r = run(kit, {"flat": st}, regex)
        print("flat deck matches:", r["flat"], flush=True)
        if not r["flat"]:
            return
    width = max(1, len(st) // 8)
    while True:
        variants = {}
        for k in range(0, len(st), width):
            v = st[:k] + st[k + width:]
            if KEEP and not re.search(KEEP, "\n".join(x for s in v for x in s)):
                continue
            variants[f"c{k}"] = v
        if len(variants) > 80:
            variants = dict(list(variants.items())[:80])
        res = run(kit, variants, regex)
        keep = [int(n[1:]) for n, ok in res.items() if ok]
        print(f"width {width}: {len(variants)} tried, {len(keep)} keep the pattern; {len(st)} statements", flush=True)
        if keep:
            # remove all that kept it, if together they still do; else the first half
            def without(ks):
                gone = set()
                for k in ks:
                    gone.update(range(k, k + width))
                return [s for i, s in enumerate(st) if i not in gone]
            cand = keep
            while cand:
                v = without(cand)
                if KEEP and not re.search(KEEP, "\n".join(x for s in v for x in s)):
                    cand = cand[: len(cand) // 2]
                    continue
                r = run(kit, {"t": v}, regex)
                if r["t"]:
                    st = v
                    open(state, "w").write("\n".join(x for s in st for x in s))
                    break
                cand = cand[: len(cand) // 2]
        elif width == 1:
            break
        if not keep:
            width = max(1, width // 2)
    out = os.path.join(S, kit, "reduced.sp")
    text = "* reduced\n" + "\n".join(x for s in st for x in s) + "\n"
    open(out, "w").write(text)
    print(text)

main()
