#!/usr/bin/env python3
"""Seeded random expressions, one per line. usage: fuzz.py SEED COUNT [kind]"""
import random, sys
ATOMS = ['a', 'b', 'c', 'd', 'e1', 'u1', '1', '2', '3', '0.5', '1e-3', '2u', '1k', '1meg', '1.5', '0', '10', '1n', '2.5m', '.5', '1.', '3e2']
UN = ['abs', 'sqrt', 'exp', 'log', 'log10', 'sin', 'cos', 'tan', 'atan', 'sinh', 'cosh', 'tanh', 'int', 'nint', 'floor', 'ceil', 'sgn', 'db', 'asin', 'acos']
BI = ['pow', 'pwr', 'sign', 'min', 'max', 'atan2', 'hypot']
OPS = ['+', '-', '*', '/', '**', '^']
CMP = ['==', '!=', '<', '>', '<=', '>=', '&&', '||']

def atom(r):
    x = r.choice(ATOMS)
    if r.random() < 0.15:
        x = '-' + x
    return x

def expr(r, depth, cmp_ok):
    k = r.random()
    if depth <= 0 or k < 0.15:
        return atom(r)
    if k < 0.50:
        # a flat chain: precedence and associativity
        n = r.randint(2, 4)
        words = [expr(r, depth - 2, False) if r.random() < 0.3 else atom(r) for _ in range(n)]
        out = words[0]
        for w in words[1:]:
            sp = r.choice(['', '', ' '])
            out += f"{sp}{r.choice(OPS)}{sp}{w}"
        return out
    if k < 0.65:
        return f"{r.choice(UN)}({expr(r, depth - 1, False)})"
    if k < 0.75:
        return f"{r.choice(BI)}({expr(r, depth - 1, False)},{expr(r, depth - 1, False)})"
    if k < 0.85:
        return f"({expr(r, depth - 1, cmp_ok)})"
    if k < 0.90:
        return "-(" + expr(r, depth - 1, False) + ")"
    if cmp_ok:
        kind = r.random()
        if kind < 0.45:
            return f"{expr(r, depth - 1, False)}{r.choice(CMP)}{expr(r, depth - 1, False)}"
        if kind < 0.75:
            return f"{expr(r, depth - 1, True)}?{expr(r, depth - 1, False)}:{expr(r, depth - 1, False)}"
        return f"!{r.choice([atom(r), '(' + expr(r, depth - 1, True) + ')'])}"
    return atom(r)

def main():
    seed, count = int(sys.argv[1]), int(sys.argv[2])
    r = random.Random(seed)
    seen = set()
    while len(seen) < count:
        e = expr(r, r.randint(1, 4), r.random() < 0.5)
        if e in seen or len(e) > 90 or not any(c in e for c in '+-*/^(<>=!&|?'):
            continue
        seen.add(e)
        print(e)

main()
