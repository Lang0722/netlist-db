#!/usr/bin/env python3
"""Round 5: thresholds and rules that the fixes need. usage: forms5.py OUTDIR"""
import json, os, shutil, sys
D = {}
N = ".model n nmos level=54\n"
TAIL = "Vd d 0 1\nVg g 0 1\n.option list nomod post=0\n.end\n"
OP = "Vd d 0 1\nVg g 0 1\n.option list nomod post=0 ingold=2 numdgt=10 measdgt=10\n.op\n.dc vd 1 1 1\n.meas dc l0 find lv1(m1) at=1\n.end\n"
M1 = "M1 d g 0 0 n l=1u w=1u\n"; M2 = "M2 d g 0 0 n l=2u w=2u\n"; M3 = "M3 d g 0 0 n l=3u w=3u\n"
def deck(name, body, files=None, tail=TAIL):
    assert name not in D, name
    D[name] = (f"* {name}\n" + body + tail, files or {})
meta = {}
# ---- where `==` ends: relative deltas from 1.0e-13 to 2.0e-13
deltas = [1.0e-13 + k * 0.1e-13 for k in range(11)]
bases = [1.0, 1.5, 3.0, 7.0, 1e-6, 1e3, 1e-12]
meta["deltas"], meta["bases"] = deltas, bases
b = N
for bi, base in enumerate(bases):
    for di, d in enumerate(deltas):
        x, y = base, base * (1 + d)
        b += f".param a{bi}{di:02d}={x!r} b{bi}{di:02d}={y!r}\nMe{bi}{di:02d} d g 0 0 n l='(a{bi}{di:02d}==b{bi}{di:02d})?1u:2u' w=1u\n"
deck("eq_edge", b)
# ---- where the truth of a value ends
vals = ["1e-31", "1e-30", "2e-30", "5e-30", "1e-29", "2e-29", "5e-29", "1e-28", "1e-27", "1e-26", "1e-25", "1e-24"]
meta["vals"] = vals
b = N
for i, x in enumerate(vals):
    b += f".param t{i}={x}\nMt{i} d g 0 0 n l='(t{i})?1u:2u' w=1u\nMa{i} d g 0 0 n l='(t{i}&&1)?1u:2u' w=1u\nMo{i} d g 0 0 n l='(t{i}||0)?1u:2u' w=1u\nMn{i} d g 0 0 n l='(!t{i})?1u:2u' w=1u\nMs{i} d g 0 0 n l='(1&&t{i})?1u:2u' w=1u\n"
deck("tr_ternary", b)
b = N
for i, x in enumerate(vals):
    for k, cond in (("a", f"t{i}&&1"), ("o", f"t{i}||0"), ("n", f"!t{i}"), ("c", f"t{i}?1:0"), ("p", f"(t{i})")):
        b += f".param t{k}{i}={x}\n" .replace(f"t{k}{i}", f"t{i}", 1) if k == "a" else ""
        b += f".if ({cond})\nM{k}{i} d g 0 0 n l=1u w=1u\n.else\nM{k}{i} d g 0 0 n l=2u w=1u\n.endif\n"
deck("tr_if", b)
# ---- exact order in calls and groups of a condition
two = ".param a=1 b='1+1e-15'\n.param f(x,y)='x<y'\n"
for name, cond in (("group", "(a<b)"), ("and", "(a<b)&&1"), ("call", "max(a<b,0)"), ("fn", "f(a,b)"), ("not", "!(a<b)"), ("tern", "(a<b)?1:0"), ("plain", "a<b"),
                   ("le_plain", "b<=a"), ("eq_plain", "a==b"), ("add", "(a<b)+0")):
    deck(f"xo_{name}", N + two + f".if ({cond})\n{M1}.else\n{M2}.endif\n")
deck("xo_param_body", N + ".param a=1 b='1+1e-15' c='a<b'\n" + f".if (c)\n{M1}.else\n{M2}.endif\n")
# ---- temper and time in a condition
deck("tc_indirect", N + ".temp 75\n.param t='temper'\n" + f".if (t>50)\n{M1}.else\n{M2}.endif\n", tail=OP)
deck("tc_in_body", N + ".temp 75\n.param t='temper*1'\n" + f".if (t)\n{M1}.else\n{M2}.endif\n", tail=OP)
deck("tc_time", N + f".if (time>0)\n{M1}.else\n{M2}.endif\n")
deck("tc_temper_no_temp", N + f".if (temper>50)\n{M1}.else\n{M2}.endif\n")
# ---- option forms
BS = ".model b.1 nmos level=54 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n.model b.2 nmos level=54 lmin=1u lmax=1e-3 wmin=1e-8 wmax=1e-3\n"
deck("os_num_then_quoted", ".option scale=1e-6\n.option scale='4e-6'\n" + BS + "M1 d g 0 0 b l=0.5 w=2\n")
deck("os_quoted_then_num", ".option scale='4e-6'\n.option scale=1e-6\n" + BS + "M1 d g 0 0 b l=0.5 w=2\n")
deck("os_num_then_name", ".param q=4e-6\n.option scale=1e-6\n.option scale=q\n" + BS + "M1 d g 0 0 b l=0.5 w=2\n")
deck("wl_unused_sub", ".subckt z d g\n.option wl\n.ends\n" + N + "M1 d g 0 0 n 1u 2u\n")
deck("wl_inc", ".inc 'wl.inc'\n" + N + "M1 d g 0 0 n 1u 2u\n", {"wl.inc": ".option wl\n"})
deck("wl_lib_unused", ".lib 'wl.l' a\n" + N + "M1 d g 0 0 n 1u 2u\n", {"wl.l": ".lib a\n.param q=1\n.endl a\n.lib b\n.option wl\n.endl b\n"})
deck("wl_after_end", N + "M1 d g 0 0 n 1u 2u\n" + TAIL + ".option wl\n", tail="")
deck("wl_after_alter", N + "M1 d g 0 0 n 1u 2u\n.op\n.alter\n.option wl\n")
# ---- depth of a top model
def chain(n):
    s = N + ".subckt s0 d g\n" + M1 + ".ends\n"
    for k in range(1, n):
        s += f".subckt s{k} d g\nX1 d g s{k-1}\n.ends\n"
    return s + f"X1 d g s{n-1}\n"
for n in (91, 92, 93, 94, 95, 96, 97, 98, 99):
    deck(f"dq_top_{n}", chain(n))
# ---- data rows with short commands
deck("dat_rows_da", N + ".da dd\n a b\n 1 2\n.enddata\n" + M1)
deck("dat_rows_dat", N + ".dat dd\n a b\n 1 2\n.enddata\n" + M1)
deck("dat_rows_endd", N + ".data dd\n a b\n 1 2\n.endd\n" + M1)
deck("dat_rows_endda", N + ".data dd\n a b\n 1 2\n.endda\n" + M1)
# ---- a name twice on one line
deck("kk_m", N + "M1 d g 0 0 n l=1u l=2u w=1u\n")
deck("kk_x", N + ".subckt s d g k=1\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s k=2 k=3\n")
deck("kk_subckt", N + ".subckt s d g k=1 k=2\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s\n")
deck("kk_param", N + ".param k=1u k=2u\nM1 d g 0 0 n l=k w=1u\n")
deck("kk_m_case", N + "M1 d g 0 0 n l=1u L=2u w=1u\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
json.dump(meta, open(os.path.join(out, "meta.json"), "w"))
for name, (text, files) in D.items():
    with open(os.path.join(out, "decks", name + ".sp"), "w") as f:
        f.write(text)
    for rel, content in files.items():
        with open(os.path.join(out, "decks", rel), "w") as f:
            f.write(content)
print(len(D), "decks")
