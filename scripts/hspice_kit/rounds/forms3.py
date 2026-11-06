#!/usr/bin/env python3
"""Round 3: equality tolerance, order of `.param` and `.if`, option forms, abbreviations, depth. usage: forms3.py OUTDIR"""
import os, shutil, sys
D = {}
N = ".model n nmos level=54\n"
TAIL = "Vd d 0 1\nVg g 0 1\n.option list nomod post=0\n.end\n"
OP = "Vd d 0 1\nVg g 0 1\n.option list nomod post=0 ingold=2 numdgt=10 measdgt=10\n.op\n.dc vd 1 1 1\n.meas dc l0 find lv1(m1) at=1\n.end\n"
M1 = "M1 d g 0 0 n l=1u w=1u\n"; M2 = "M2 d g 0 0 n l=2u w=2u\n"; M3 = "M3 d g 0 0 n l=3u w=3u\n"
def deck(name, body, files=None, tail=TAIL, title=True):
    assert name not in D, name
    D[name] = ((f"* {name}\n" if title else "") + body + tail, files or {})

# ---- comparisons near equality: one deck, one MOSFET per question. L=1u: true, L=2u: false
deltas = [0, 1e-16, 2.3e-16, 1e-15, 3e-15, 1e-14, 1e-13, 1e-12, 1e-11, 1e-10, 1e-9, 1e-8, 1e-7, 1e-6, 1e-5, 1e-4, 1e-3]
bases = {"one": 1.0, "micro": 1e-6, "kilo": 1e3, "pico": 1e-12}
ops = {"eq": "==", "ne": "!=", "lt": "<", "le": "<=", "gt": ">", "ge": ">="}
body = N
for bname, base in bases.items():
    for i, d in enumerate(deltas):
        body += f".param a_{bname}_{i}={base!r} b_{bname}_{i}={base*(1+d)!r}\n"
        for oname, op in ops.items():
            body += f"M{oname}_{bname}_{i} d g 0 0 n l='(a_{bname}_{i}{op}b_{bname}_{i})?1u:2u' w=1u\n"
            if op in ("<", "<=", ">", ">="):
                body += f"Mr{oname}_{bname}_{i} d g 0 0 n l='(b_{bname}_{i}{op}a_{bname}_{i})?1u:2u' w=1u\n"
deck("cmp_ternary", body)
body = N
for bname, base in bases.items():
    for i, d in enumerate(deltas):
        body += f".param a_{bname}_{i}={base!r} b_{bname}_{i}={base*(1+d)!r}\n"
        for oname, op in ops.items():
            body += (f".if (a_{bname}_{i}{op}b_{bname}_{i})\nM{oname}_{bname}_{i} d g 0 0 n l=1u w=1u\n.else\n"
                     f"M{oname}_{bname}_{i} d g 0 0 n l=2u w=1u\n.endif\n")
deck("cmp_if", body)
body = N
for i, (x, y) in enumerate([("0.1*3", "0.3"), ("1/3*3", "1"), ("0.1+0.2", "0.3"), ("sqrt(2)*sqrt(2)", "2"), ("1u+1n", "1.001u"), ("1e-300", "0"), ("1e-30", "0"), ("1e-20", "0"), ("1e-15", "0"), ("1e-12", "0"), ("1e-6", "0")]):
    body += f".if (({x})=={y})\nMi{i} d g 0 0 n l=1u w=1u\n.else\nMi{i} d g 0 0 n l=2u w=1u\n.endif\nMt{i} d g 0 0 n l='(({x})=={y})?1u:2u' w=1u\n"
deck("cmp_exprs", body)
body = N
for i, x in enumerate(["1e-300", "1e-100", "1e-50", "1e-30", "1e-25", "1e-20", "1e-15", "-1e-30", "0"]):
    body += f".if ({x})\nMi{i} d g 0 0 n l=1u w=1u\n.else\nMi{i} d g 0 0 n l=2u w=1u\n.endif\nMt{i} d g 0 0 n l='({x})?1u:2u' w=1u\nMn{i} d g 0 0 n l='(!{x})?1u:2u' w=1u\nMa{i} d g 0 0 n l='({x}&&1)?1u:2u' w=1u\nMo{i} d g 0 0 n l='({x}||0)?1u:2u' w=1u\n"
deck("cmp_truth", body)

# ---- order of `.param` and `.if`
deck("ord_plain_if_plain", N + ".param q=1\n.if (q>1)\n" + M1 + ".else\n" + M2 + ".endif\n.param q=2\n")
deck("ord_if_plain_plain", N + ".if (q>1)\n" + M1 + ".else\n" + M2 + ".endif\n.param q=1\n.param q=2\n")
deck("ord_plain_branch_if", N + ".param q=1\n.if (1)\n.param q=2\n.endif\n.if (q>1)\n" + M1 + ".else\n" + M2 + ".endif\n")
deck("ord_branch_plain_if", N + ".if (1)\n.param q=2\n.endif\n.param q=1\n.if (q>1)\n" + M1 + ".else\n" + M2 + ".endif\n")
deck("ord_branch_if_plain", N + ".if (1)\n.param q=2\n.endif\n.if (q>1)\n" + M1 + ".else\n" + M2 + ".endif\n.param q=1\n")
deck("ord_branch_reads_later", N + ".if (1)\n.param r='q*2'\n.endif\n.param q=1u\nM1 d g 0 0 n l=r w=1u\n")
deck("ord_branch_chain", N + ".if (1)\n.param q=1\n.if (q==1)\n.param q=2\n.endif\n.if (q==2)\n.param q=3\n.endif\n.endif\nM1 d g 0 0 n l='q*1u' w=1u\n")
deck("ord_nested_plain_after", N + ".if (1)\n.if (1)\n.param q=2u\n.endif\n.param q=1u\n.endif\nM1 d g 0 0 n l=q w=1u\n")
deck("ord_nested_plain_before", N + ".if (1)\n.param q=1u\n.if (1)\n.param q=2u\n.endif\n.endif\nM1 d g 0 0 n l=q w=1u\n")
deck("ord_after_block_outer", N + ".if (1)\n.param q=1u\n.endif\n.if (1)\n.if (1)\n.param q=2u\n.endif\n.endif\n.param z=1\nM1 d g 0 0 n l=q w=1u\n")
deck("ord_sub_passed_cond", N + ".subckt s d g k=1\n.if (k==1)\n.param k=2\n.endif\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s\nX2 d g s k=5\nX3 d g s k=1\n")
deck("ord_sub_plain_branch", N + ".subckt s d g\n.param k=1\n.if (k==1)\n.param k=2\n.endif\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s\nX2 d g s k=5\n")
deck("ord_sub_branch_plain", N + ".subckt s d g\n.if (1)\n.param k=2\n.endif\n.param k=1\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s\n")
deck("ord_default_branch_x", N + ".subckt leaf d g v=1\nM1 d g 0 0 n l='v*1u' w=1u\n.ends\n.subckt s d g k=1\n.if (k==1)\n.param j=2\n.else\n.param j=3\n.endif\nX1 d g leaf v=j\n.ends\nX1 d g s\nX2 d g s k=7\n")
deck("ord_global_branch_top", ".option parhier=global\n.if (1)\n.param k=9\n.endif\n" + N + ".subckt s d g k=2\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s\n")
deck("ord_local_branch_top", ".option parhier=local\n.if (1)\n.param k=9\n.endif\n" + N + ".subckt s d g k=2\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s\n")
deck("ord_local_branch_in_sub_hides", ".option parhier=local\n.param k=9\n" + N + ".subckt s d g\n.if (1)\n.param k=2\n.endif\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s\n")
deck("ord_local_false_branch_in_sub", ".option parhier=local\n.param k=9\n" + N + ".subckt s d g\n.if (0)\n.param k=2\n.endif\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s\n")
deck("ord_memo_shape", ".option parhier=local\n" + N + ".subckt leaf d g v=1\n.if (v>1)\nM1 d g 0 0 n l=1u w=1u\n.else\nM2 d g 0 0 n l=2u w=2u\n.endif\n.ends\n.subckt mid d g v=1\nX1 d g leaf v=v\nX2 d g leaf\n.ends\nX1 d g mid\nX2 d g mid v=2\nX3 d g mid\nX4 d g mid v=2\n")
deck("ord_lib_corner", ".param corner=2\n.if (corner==1)\n.lib 'ord_c.l' tt\n.else\n.lib 'ord_c.l' ss\n.endif\nM1 d g 0 0 q l='lc' w=1u\n", {"ord_c.l": ".lib tt\n.param lc=1u\n.model q nmos level=54\n.endl tt\n.lib ss\n.param lc=2u\n.model q pmos level=54\n.endl ss\n"})
deck("ord_lib_section_two_if", ".param corner=2\n.lib 'ord_d.l' top\nM1 d g 0 0 q l='lc' w=1u\n", {"ord_d.l": ".lib top\n.if (corner==1)\n.lib 'ord_d.l' tt\n.else\n.lib 'ord_d.l' ss\n.endif\n.endl top\n.lib tt\n.param lc=1u\n.model q nmos level=54\n.endl tt\n.lib ss\n.param lc=2u\n.model q pmos level=54\n.endl ss\n"})
deck("ord_macro_if", N + ".macro s d g w=1u\n.if (w>2u)\nM1 d g 0 0 n l=1u w=w\n.else\nM2 d g 0 0 n l=2u w=w\n.endif\n.eom\nX1 d g s w=1u\nX2 d g s w=3u\n")
deck("ord_two_top_blocks_x", N + ".subckt a d g\n" + M1 + ".ends\n.subckt b d g\n" + M2 + ".ends\n.param t=2\n.if (t==1)\nXs d g a\n.elseif (t==2)\nXs d g b\n.else\nXs d g a\n.endif\n")

# ---- option forms, with bins that tell the value
B8 = ".model b.1 nmos level=54 lmin=1u lmax=8u wmin=1u wmax=8u\n.model b.2 nmos level=54 lmin=8u lmax=1e-3 wmin=8u wmax=1e-3\n"
deck("opd_plain", ".option defl=7u defw=7u\n" + B8 + "M1 d g 0 0 b\n")
deck("opd_quoted", ".option defl='7u' defw='7u'\n" + B8 + "M1 d g 0 0 b\n")
deck("opd_named", ".param dv=7u\n.option defl=dv defw=dv\n" + B8 + "M1 d g 0 0 b\n")
deck("opd_expr", ".option defl='3.5u*2' defw='3.5u*2'\n" + B8 + "M1 d g 0 0 b\n")
deck("opd_none", B8 + "M1 d g 0 0 b\n")
deck("opd_twice", ".option defl=7u defw=7u\n.option defl=9u defw=9u\n" + B8 + "M1 d g 0 0 b\n")
deck("opd_after_m", B8 + "M1 d g 0 0 b\n.option defl=7u defw=7u\n")
deck("opd_sub", ".subckt s d g\n.option defl=7u defw=7u\n.ends\n" + B8 + "M1 d g 0 0 b\n")
deck("opd_branch", ".if (1)\n.option defl=7u defw=7u\n.endif\n" + B8 + "M1 d g 0 0 b\n")
deck("opd_false_branch", ".if (0)\n.option defl=7u defw=7u\n.endif\n" + B8 + "M1 d g 0 0 b\n")
deck("opd_scale_applies", ".option defl=7 defw=7 scale=1u\n" + B8 + "M1 d g 0 0 b\n")
BS = ".model b.1 nmos level=54 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n.model b.2 nmos level=54 lmin=1u lmax=1e-3 wmin=1e-8 wmax=1e-3\n"
deck("ops_plain", ".option scale=1e-6\n" + BS + "M1 d g 0 0 b l=0.5 w=2\n")
deck("ops_unit", ".option scale=1u\n" + BS + "M1 d g 0 0 b l=0.5 w=2\n")
deck("ops_twice", ".option scale=1e-6\n.option scale=4e-6\n" + BS + "M1 d g 0 0 b l=0.5 w=2\n")
deck("ops_sub", ".subckt s d g\n.option scale=1e-6\n.ends\n" + BS + "M1 d g 0 0 b l=0.5 w=2\n")
deck("ops_sub_used", ".subckt s d g\n.option scale=1e-6\n.ends\nX1 d g s\n" + BS + "M1 d g 0 0 b l=0.5 w=2\n")
deck("ops_after_m", BS + "M1 d g 0 0 b l=0.5 w=2\n.option scale=1e-6\n")
deck("ops_upper", ".OPTION SCALE=1E-6\n" + BS + "M1 d g 0 0 b l=0.5 w=2\n")
deck("oph_twice_lg", ".option parhier=local\n.option parhier=global\n.param k=1u\n" + N + ".subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n")
deck("oph_twice_gl", ".option parhier=global\n.option parhier=local\n.param k=1u\n" + N + ".subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n")
deck("oph_quoted", ".option parhier='local'\n.param k=1u\n" + N + ".subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n")
deck("oph_upper", ".OPTION PARHIER=LOCAL\n.param k=1u\n" + N + ".subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n")
deck("oph_sub", ".subckt z d g\n.option parhier=local\n.ends\n.param k=1u\n" + N + ".subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n")
deck("oph_after", ".param k=1u\n" + N + ".subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n.option parhier=local\n")
deck("oph_junk", ".option parhier=foo\n.param k=1u\n" + N + ".subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n")
deck("oph_flag", ".option parhier\n.param k=1u\n" + N + ".subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n")
AW = ".model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6\n.model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3\n"
deck("opw_plain0", ".option wnflag=0\n" + AW + "M1 d g 0 0 a w=4u l=1u nf=8\n")
deck("opw_plain1", ".option wnflag=1\n" + AW + "M1 d g 0 0 a w=4u l=1u nf=8\n")
deck("opw_flag", ".option wnflag\n" + AW + "M1 d g 0 0 a w=4u l=1u nf=8\n")
deck("opw_none", AW + "M1 d g 0 0 a w=4u l=1u nf=8\n")
deck("opw_quoted1", ".option wnflag='1'\n" + AW + "M1 d g 0 0 a w=4u l=1u nf=8\n")
deck("opw_quoted_expr", ".option wnflag='1-1'\n" + AW + "M1 d g 0 0 a w=4u l=1u nf=8\n")
deck("opw_inst_named", ".param wf=0\n" + AW + "M1 d g 0 0 a w=4u l=1u nf=8 wnflag=wf\n")
deck("opw_inst_quoted", ".param wf=0\n" + AW + "M1 d g 0 0 a w=4u l=1u nf=8 wnflag='wf'\n")
deck("opt_wl_then_zero", ".option wl\n.option wl=0\n" + N + "M1 d g 0 0 n 1u 2u\n")
deck("opt_wl_zero_then", ".option wl=0\n.option wl\n" + N + "M1 d g 0 0 n 1u 2u\n")
deck("opt_wl_num2", ".option wl=2\n" + N + "M1 d g 0 0 n 1u 2u\n")
deck("opt_wl_named", ".param q=1\n.option wl=q\n" + N + "M1 d g 0 0 n 1u 2u\n")
deck("opt_wl_branch", ".if (1)\n.option wl\n.endif\n" + N + "M1 d g 0 0 n 1u 2u\n")
deck("opt_wl_false_branch", ".if (0)\n.option wl\n.endif\n" + N + "M1 d g 0 0 n 1u 2u\n")
deck("opt_wl_key_value", ".option wl\n" + N + "M1 d g 0 0 n 1u w=2u\n")
deck("opt_wl_three", ".option wl\n" + N + "M1 d g 0 0 n 1u 2u 3p\n")
TEMPL = "M1 d g 0 0 n l='(1+temper/100)*1u' w=1u\n"
deck("tq_two_lines", N + ".temp 50\n.temp 75\n" + TEMPL, tail=OP)
deck("tq_word", N + ".temperature 75\n" + TEMPL, tail=OP)
deck("tq_eq", N + ".temp=75\n" + TEMPL, tail=OP)
deck("tq_tnom_quoted", ".option tnom='50'\n" + N + TEMPL, tail=OP)
deck("tq_tnom_named", ".param tt=50\n.option tnom=tt\n" + N + TEMPL, tail=OP)
deck("tq_tnom_twice", ".option tnom=50\n.option tnom=75\n" + N + TEMPL, tail=OP)
deck("tq_branch", N + ".if (1)\n.temp 75\n.endif\n" + TEMPL, tail=OP)
deck("tq_false_branch", N + ".if (0)\n.temp 75\n.endif\n" + TEMPL, tail=OP)
deck("tq_cond_temper", N + ".temp 75\n.if (temper>50)\nM1 d g 0 0 n l=1u w=1u\n.else\nM1 d g 0 0 n l=2u w=1u\n.endif\n", tail=OP)
deck("tq_sub_temp", N + ".subckt s d g\n.temp 75\n.ends\nX1 d g s\n" + TEMPL, tail=OP)
deck("tq_expr", N + ".param t=75\n.temp t\n" + TEMPL, tail=OP)
deck("tq_expr_quoted", N + ".temp '50+25'\n" + TEMPL, tail=OP)

# ---- abbreviations
for w in ("opt", "opti", "optio", "optn", "o", "op_x", "optionx"):
    deck(f"ab_option_{w}", f".{w} parhier=local\n.param k=1u\n" + N + ".subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n")
for w in ("par", "para", "parame", "paramet", "paramete", "parameter", "parameters", "paramx", "params"):
    deck(f"ab_param_{w}", N + f".{w} k=2u\nM1 d g 0 0 n l=k w=1u\n")
for w in ("inc", "incl", "inclu", "includ", "include", "includes", "includex"):
    deck(f"ab_inc_{w}", f".{w} 'ab_i.inc'\nM1 d g 0 0 q l=1u w=1u\n", {"ab_i.inc": ".model q nmos level=54\n"})
for w in ("sub", "subc", "subck", "subckt", "subckts", "subcktx"):
    deck(f"ab_subckt_{w}", N + f".{w} s d g\n" + M1 + ".ends\nX1 d g s\n")
for w in ("end", "ends", "endsx", "eom", "endx"):
    deck(f"ab_ends_{w}", N + ".subckt s d g\n" + M1 + f".{w}\nX1 d g s\n")
for w in ("mod", "mode", "model", "models", "modelx"):
    deck(f"ab_model_{w}", f".{w} q nmos level=54\nM1 d g 0 0 q l=1u w=1u\n")
for w in ("lib", "libr", "library"):
    deck(f"ab_lib_{w}", f".{w} 'ab_l.l' a\nM1 d g 0 0 q l=1u w=1u\n", {"ab_l.l": ".lib a\n.model q nmos level=54\n.endl a\n"})
for w in ("glob", "globa", "global", "globals"):
    deck(f"ab_global_{w}", N + f".{w} d g\n.subckt s\n" + M1 + ".ends\nX1 s\n")
for w in ("el", "els", "else", "elsex"):
    deck(f"ab_else_{w}", N + ".if (0)\n" + M1 + f".{w}\n" + M2 + ".endif\n")
for w in ("endi", "endif", "endifx"):
    deck(f"ab_endif_{w}", N + ".if (1)\n" + M1 + f".{w}\n")
for w in ("elsei", "elseif", "elseifx"):
    deck(f"ab_elseif_{w}", N + ".if (0)\n" + M1 + f".{w} (1)\n" + M2 + ".else\n" + M3 + ".endif\n")
for w in ("i", "if", "ifx"):
    deck(f"ab_if_{w}", N + f".{w} (0)\n" + M1 + ".else\n" + M2 + ".endif\n")

# ---- depth
def chain(n):
    s = N + ".subckt s0 d g\n" + M1 + ".ends\n"
    for k in range(1, n):
        s += f".subckt s{k} d g\nX1 d g s{k-1}\n.ends\n"
    return s + f"X1 d g s{n-1}\n"
for n in (10, 100, 255, 256, 257, 300, 1000, 3000):
    deck(f"dp_x_{n}", chain(n))
def nested(n):
    s = N
    for k in range(n):
        s += f".subckt s{k} d g\n"
    s += M1
    s += ".ends\n" * n
    return s + "X1 d g s0\n"
def nested_inst(n):
    s = N
    for k in range(n):
        s += f".subckt s{k} d g\n"
    s += M1
    for k in range(n - 1, 0, -1):
        s += f".ends\nX1 d g s{k}\n"
    return s + ".ends\nX1 d g s0\n"
for n in (10, 100, 255, 256, 257, 300, 1000):
    deck(f"dp_nest_{n}", nested_inst(n))
for n in (1000, 5000):
    deck(f"dp_if_{n}", N + ".if (1)\n" * n + M1 + ".endif\n" * n)

out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, (text, files) in D.items():
    with open(os.path.join(out, "decks", name + ".sp"), "w") as f:
        f.write(text)
    for rel, content in files.items():
        p = os.path.join(out, "decks", rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(content)
# the cwd probe: run by hand with `-i sub\cwd_probe.sp`
sub = os.path.join(out, "decks", "sub")
os.makedirs(sub, exist_ok=True)
open(os.path.join(sub, "cwd_probe.sp_"), "w").write("* cwd probe\n" + N + ".inc 'cw1.inc'\n.inc 'cw2.inc'\n.inc 'cw3.inc'\nM1 d g 0 0 n l=k1 w=k2\nM2 d g 0 0 n l=k3 w=1u\n" + TAIL)
open(os.path.join(sub, "cw1.inc"), "w").write(".param k1=1u\n")
open(os.path.join(out, "decks", "cw1.inc"), "w").write(".param k1=2u\n")
open(os.path.join(sub, "cw2.inc"), "w").write(".param k2=3u\n")
open(os.path.join(out, "decks", "cw3.inc"), "w").write(".param k3=4u\n")
print(len(D), "decks")
