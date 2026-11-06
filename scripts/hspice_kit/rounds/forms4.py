#!/usr/bin/env python3
"""Round 4: tolerance of comparisons, more short keywords, depth, FinFET levels. usage: forms4.py OUTDIR"""
import json, os, shutil, sys
D = {}
N = ".model n nmos level=54\n"
TAIL = "Vd d 0 1\nVg g 0 1\n.option list nomod post=0\n.end\n"
OP = "Vd d 0 1\nVg g 0 1\n.option list nomod post=0 ingold=2 numdgt=10 measdgt=10\n.op\n.dc vd 1 1 1\n.meas dc l0 find lv1(m1) at=1\n.end\n"
M1 = "M1 d g 0 0 n l=1u w=1u\n"; M2 = "M2 d g 0 0 n l=2u w=2u\n"; M3 = "M3 d g 0 0 n l=3u w=3u\n"
def deck(name, body, files=None, tail=TAIL):
    assert name not in D, name
    D[name] = (f"* {name}\n" + body + tail, files or {})

deltas = [0, 1e-16, 2.3e-16, 1e-15, 1e-14, 1e-13, 2e-13, 3e-13, 5e-13, 7e-13, 1e-12, 2e-12, 1e-11, 1e-9, 1e-6, 1e-3]
bases = [1.0, 1e-6, 1e3, 1e-12, -1.0, 1e-30]
ops = ["==", "!=", "<", "<=", ">", ">="]
meta = {"deltas": deltas, "bases": bases, "ops": ops}
t, f = N, N
for bi, base in enumerate(bases):
    for di, d in enumerate(deltas):
        t += f".param a{bi}{di:02d}={base!r} b{bi}{di:02d}={base*(1+d)!r}\n"
        f += f".param a{bi}{di:02d}={base!r} b{bi}{di:02d}={base*(1+d)!r}\n"
        for oi, op in enumerate(ops):
            for side, (x, y) in enumerate(((f"a{bi}{di:02d}", f"b{bi}{di:02d}"), (f"b{bi}{di:02d}", f"a{bi}{di:02d}"))):
                e = f"M{oi}{side}{bi}{di:02d}"
                t += f"{e} d g 0 0 n l='({x}{op}{y})?1u:2u' w=1u\n"
                f += f".if ({x}{op}{y})\n{e} d g 0 0 n l=1u w=1u\n.else\n{e} d g 0 0 n l=2u w=1u\n.endif\n"
deck("cmp_ternary", t)
deck("cmp_if", f)
vals = ["1e-300", "1e-100", "1e-50", "1e-30", "1e-25", "1e-20", "1e-15", "-1e-30", "0"]
meta["vals"] = vals
b = N
for i, x in enumerate(vals):
    b += f".param t{i}={x}\n.if (t{i})\nMi{i} d g 0 0 n l=1u w=1u\n.else\nMi{i} d g 0 0 n l=2u w=1u\n.endif\nMt{i} d g 0 0 n l='(t{i})?1u:2u' w=1u\nMn{i} d g 0 0 n l='(!t{i})?1u:2u' w=1u\nMa{i} d g 0 0 n l='(t{i}&&1)?1u:2u' w=1u\nMo{i} d g 0 0 n l='(t{i}||0)?1u:2u' w=1u\n"
deck("cmp_truth", b)
fv = ["min(1,1.0000000000001)", "max(1,1.0000000000001)", "int(0.9999999999999)", "floor(0.9999999999999)", "ceil(1.0000000000001)", "nint(0.49999999999999)", "sgn(1e-30)"]
meta["fv"] = fv
b = N
for i, x in enumerate(fv):
    b += f".param v{i}='{x}'\nMf{i} d g 0 0 n l='v{i}*1u' w=1u\n"
deck("cmp_funcs", b)

# ---- short keywords that change what a first run holds
for w in ("alt", "alte", "alter", "alters", "alterx", "al"):
    deck(f"ab_alter_{w}", N + M1 + ".op\n" + f".{w}\n" + M2)
for w in ("mac", "macr", "macro", "macros", "ma"):
    deck(f"ab_macro_{w}", N + f".{w} s d g\n" + M1 + ".eom\nX1 d g s\n")
TEMPL = "M1 d g 0 0 n l='(1+temper/100)*1u' w=1u\n"
for w in ("te", "tem", "temp", "tempe", "temper", "tempera", "temperat", "temperatu", "temperatur", "temperature", "temperatures", "tempx"):
    deck(f"ab_temp_{w}", N + f".{w} 75\n" + TEMPL, tail=OP)
for w in ("pr", "pro", "prot", "prote", "protec", "protect", "protects", "protx"):
    deck(f"ab_prot_{w}", N + f".{w}\n" + M1 + ".unprot\n" + M2)
for w in ("un", "unp", "unpr", "unpro", "unprot", "unprote", "unprotec", "unprotect", "unprotects"):
    deck(f"ab_unprot_{w}", N + ".prot\n" + M1 + f".{w}\n" + M2)
for w in ("da", "dat", "data", "datas", "datax"):
    deck(f"ab_data_{w}", N + f".{w} dd\n+ a b\n+ 1 2\n.enddata\n" + M1)
for w in ("endd", "endda", "enddat", "enddata", "enddatas"):
    deck(f"ab_enddata_{w}", N + f".data dd\n+ a b\n+ 1 2\n.{w}\n" + M1)
for w in ("endl", "endlx", "endli", "endlib"):
    deck(f"ab_endl_{w}", f".lib 'ab_el{w}.l' a\nM1 d g 0 0 q l=1u w=1u\n", {f"ab_el{w}.l": f".lib a\n.model q nmos level=54\n.{w} a\n"})
for w in ("in", "inc"):
    deck(f"ab_in_{w}", f".{w} 'ab_i2.inc'\nM1 d g 0 0 q l=1u w=1u\n", {"ab_i2.inc": ".model q nmos level=54\n"})
for w in ("pa", "su", "mo", "op", "li", "li_x"):
    deck(f"ab_short_{w}", N + M1 + f".{w} x\n")

# ---- depth: where the model search of a deep MOSFET ends
def chain(n, local=False):
    s = N if not local else ""
    s += ".subckt s0 d g\n" + (N if local else "") + M1 + ".ends\n"
    for k in range(1, n):
        s += f".subckt s{k} d g\nX1 d g s{k-1}\n.ends\n"
    return s + f"X1 d g s{n-1}\n"
for n in (12, 15, 20, 25, 30, 35, 40, 45, 50, 60, 70, 80, 90):
    deck(f"dp_top_{n}", chain(n))
for n in (20, 40, 60, 100, 256, 257, 300, 1000, 3000):
    deck(f"dp_loc_{n}", chain(n, True))

# ---- FinFET levels: which values choose the bin
for lv in (72, 73):
    deck(f"ff_{lv}_lw", f".model f.1 nmos level={lv} lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n.model f.2 nmos level={lv} lmin=1u lmax=1e-3 wmin=1e-8 wmax=1e-3\nM1 d g 0 0 f l=500n\n")
    deck(f"ff_{lv}_nfin_bins", f".model f.1 nmos level={lv} lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\n.model f.2 nmos level={lv} lmin=1e-8 lmax=1u nfinmin=4 nfinmax=9\nM1 d g 0 0 f l=500n nfin=6\n")
    deck(f"ff_{lv}_exact", f".model f nmos level={lv}\nM1 d g 0 0 f l=500n nfin=6\n")
deck("lv_49_bins_plain", ".model f.1 nmos level=49 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3 version=3.2\n.model f.2 nmos level=49 lmin=1u lmax=1e-3 wmin=1e-8 wmax=1e-3 version=3.2\nM1 d g 0 0 f l=500n w=1u\nM2 d g 0 0 f l=2u w=1u\n")
deck("lv_49_bins_lmin", ".model f.1 nmos level=49 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3 version=3.2\nM1 d g 0 0 f l=500n w=1u\n")
deck("lv_53_bins", ".model f.1 nmos level=53 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n.model f.2 nmos level=53 lmin=1u lmax=1e-3 wmin=1e-8 wmax=1e-3\nM1 d g 0 0 f l=500n w=1u\nM2 d g 0 0 f l=2u w=1u\n")
deck("lv_1_exact", ".model f nmos level=1\nM1 d g 0 0 f l=500n w=1u\n")
deck("lv_none_exact", ".model f nmos\nM1 d g 0 0 f l=500n w=1u\n")
deck("lv_mix_bins", ".model f.1 nmos level=54 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n.model f.2 nmos level=49 lmin=1u lmax=1e-3 wmin=1e-8 wmax=1e-3 version=3.2\nM1 d g 0 0 f l=500n w=1u\nM2 d g 0 0 f l=2u w=1u\n")
deck("lv_pmos_on_n", ".model f.1 pmos level=54 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n.model f.2 nmos level=54 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\nM1 d g 0 0 f l=500n w=1u\n")

out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
json.dump(meta, open(os.path.join(out, "meta.json"), "w"))
for name, (text, files) in D.items():
    with open(os.path.join(out, "decks", name + ".sp"), "w") as f:
        f.write(text)
    for rel, content in files.items():
        p = os.path.join(out, "decks", rel)
        with open(p, "w") as f:
            f.write(content)
sub = os.path.join(out, "decks", "sub")
os.makedirs(sub, exist_ok=True)
open(os.path.join(sub, "cwd_probe.sp_"), "w").write("* cwd probe\n" + N + ".inc 'cw1.inc'\n.inc 'cw2.inc'\n.inc 'cw3.inc'\nM1 d g 0 0 n l=k1 w=k2\nM2 d g 0 0 n l=k3 w=1u\n" + TAIL)
open(os.path.join(sub, "cw1.inc"), "w").write(".param k1=1u\n")
open(os.path.join(out, "decks", "cw1.inc"), "w").write(".param k1=2u\n")
open(os.path.join(sub, "cw2.inc"), "w").write(".param k2=3u\n")
open(os.path.join(out, "decks", "cw3.inc"), "w").write(".param k3=4u\n")
print(len(D), "decks")
