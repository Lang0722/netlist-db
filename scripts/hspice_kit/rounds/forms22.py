#!/usr/bin/env python3
"""Round 22: when does a 4th word that names a model win? usage: forms22.py OUTDIR"""
import os, shutil, sys
D = {}
TAIL = "Vd d 0 1\nVg g 0 1\nVs s 0 0\n.option list nomod post=0\n.end\n"
def deck(name, body):
    assert name not in D, name
    D[name] = f"* {name}\n" + body + TAIL
PN = ".model p pmos level=54\n.model n nmos level=54\n"
AB = ".model a nmos level=54\n.model b nmos level=54\n"
# the common hand-written form: the same word as bulk and model
deck("c_nn", PN + "M1 d g 0 n n l=1u w=1u\n")
deck("c_pp", PN + "M1 d g s p p l=1u w=1u\n")
deck("c_nnn", PN + "M1 d g n n n l=1u w=1u\n")
deck("c_nn_two", PN + "M1 d g 0 n n l=1u w=1u\nM2 d g s n n l=2u w=2u\n")
deck("c_pn_mix", PN + "M1 d g 0 n n l=1u w=1u\nM2 d g s p p l=2u w=2u\n")
deck("c_sub_nn", PN + ".subckt inv in out vdd vss\nMp out in vdd vdd p l=1u w=2u\nMn out in vss vss n l=1u w=1u\n.ends\nX1 d g s 0 inv\n")
deck("c_sub_node_names", PN + ".subckt inv in out p n\nMp out in p p p l=1u w=2u\nMn out in n n n l=1u w=1u\n.ends\nX1 d g s 0 inv\n")
deck("c_pos", PN + "M1 d g 0 n n 1u 2u\nM2 d g s n n 3u 4u\n")
# a line that names `a` by its 5th word, and the line under test
T = "M1 d g 0 a b l=2u w=2u\n"
U5 = "Mu d g s 0 a l=1u w=1u\n"
deck("u_none", AB + T)
deck("u_5th", AB + U5 + T)
deck("u_5th_after", AB + T + U5)
deck("u_4th", AB + "Mu d g s a l=1u w=1u\n" + T)
deck("u_4th_after", AB + T + "Mu d g s a l=1u w=1u\n")
deck("u_4th_plain_b_used", AB + "Mu d g s 0 b l=1u w=1u\n" + T)
deck("u_b_then_a", AB + "Mu d g s 0 b l=1u w=1u\nMv d g s 0 a l=1u w=1u\n" + T)
deck("u_uninst", AB + ".subckt never d g s\n" + U5 + ".ends\n" + T)
deck("u_uninst_in", AB + ".subckt never d g s\n" + U5 + ".ends\n.subckt s d g\n" + T + ".ends\nX1 d g s\n")
deck("u_false_if", AB + ".param r=0\n.if (r>1)\n" + U5 + ".endif\n" + T)
deck("u_true_if", AB + ".param r=2\n.if (r>1)\n" + U5 + ".endif\n" + T)
deck("u_prot", AB + ".prot\n" + U5 + ".unprot\n" + T)
deck("u_self_two", AB + T + T.replace("M1", "M2"))
deck("u_swap", AB + T + "M2 d g 0 b a l=2u w=2u\n")
deck("u_x_word", AB + ".subckt a d g s\nR1 d g 1\n.ends\n" + T)
deck("u_d_line", ".model a d\n.model b nmos level=54\nD1 d g a\n" + T)
deck("u_a_is_d", ".model a d\n.model b nmos level=54\n" + T)
deck("u_a_diode_used_nmos", AB + "D1 d g a\n" + T)
# the line under test: other shapes, `a` is used
deck("t_same", AB + U5 + "M1 d g 0 a a l=2u w=2u\n")
deck("t_same_alone", AB + "M1 d g 0 a a l=2u w=2u\n")
deck("t_three", AB + U5 + "M1 d g a a a l=2u w=2u\n")
deck("t_three_alone", AB + "M1 d g a a a l=2u w=2u\n")
deck("t_pos", AB + U5 + "M1 d g 0 a b 2u 2u\n")
deck("t_no_values", AB + U5 + "M1 d g 0 a b\n")
deck("t_third", AB + U5 + "M1 d g a 0 b l=2u w=2u\n")
deck("t_second", AB + U5 + "M1 d a 0 0 b l=2u w=2u\n")
deck("t_first", AB + U5 + "M1 a g 0 0 b l=2u w=2u\n")
deck("t_param_b", AB + U5 + ".param b=3u\nM1 d g 0 a b w=2u\n")
deck("t_param_b_alone", AB + ".param b=3u\nM1 d g 0 a b w=2u\n")
deck("t_used_b_too", AB + "Mu d g s 0 a l=1u w=1u\nMv d g s 0 b l=1u w=1u\n" + T)
deck("t_sixth", AB + ".model c nmos level=54\n" + U5 + "M1 d g 0 a c b l=2u w=2u\n")
deck("t_bins", ".model a.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n.model b.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n" + U5 + T)
deck("t_bins_no_fit", ".model a.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n.model b.1 nmos level=54 lmin=1e-9 lmax=1 wmin=1e-9 wmax=1\n" + U5 + "M1 d g 0 a b l=2 w=2\n")
out = sys.argv[1]
if os.path.exists(out):
    shutil.rmtree(out)
os.makedirs(os.path.join(out, "decks"))
for name, text in D.items():
    open(os.path.join(out, "decks", name + ".sp"), "w").write(text)
print(len(D), "decks")
