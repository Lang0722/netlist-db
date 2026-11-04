* parhier_probe.sp: how HSPICE resolves subckt parameters and chooses
* bins, for comparison with netlist-db (scripts/parhier_probe.py).
*
* Cases 1-6: an instance of a subckt holding a resistor R fed 1 A, whose
* value is the parameter being resolved: HSPICE's `.measure` gives it as the
* voltage of node `n` (the .mt0 file); and a MOSFET whose width is the same
* value in um, which netlist-db's model_select prints.
* Cases 7-11: a MOSFET whose bin is the question. In each binned model, bin
* .1 has VTH0=0.25 (on at VGS=0.5) and bin .2 VTH0=0.75 (off): HSPICE's
* `.measure` of the drain current tells which bin it chose.
*
* Run it once as is (parhier=global, HSPICE's default) and once with the
* option line below set to local.
.option parhier=global
.option post=0 ingold=2
.param p=1 lb=2e-7
.model nch.1 nmos level=54 version=4.7 vth0=0.25 lmin=1e-8 lmax='lb' wmin=1e-8 wmax=1e-3
.model nch.2 nmos level=54 version=4.7 vth0=0.75 lmin='lb' lmax=1e-3 wmin=1e-8 wmax=1e-3
* bins split at W=1u, without and with WNFLAG
.model nfa.1 nmos level=54 version=4.7 vth0=0.25 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6
.model nfa.2 nmos level=54 version=4.7 vth0=0.75 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3
.model nfb.1 nmos level=54 version=4.7 vth0=0.25 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6 wnflag=1
.model nfb.2 nmos level=54 version=4.7 vth0=0.75 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3 wnflag=1
* bins split at L=1u
.model edg.1 nmos level=54 version=4.7 vth0=0.25 lmin=1e-8 lmax=1e-6 wmin=1e-8 wmax=1e-3
.model edg.2 nmos level=54 version=4.7 vth0=0.75 lmin=1e-6 lmax=1e-3 wmin=1e-8 wmax=1e-3

* the value of `v`, read where the case is
.subckt show d v=0
I1 0 n 1
R1 n 0 'v'
M1 d d 0 0 nch w='v*1u' l=1u
.ends show

* 1. a top-level .param, a .subckt default, and a value passed:
*    global: 1 (top-level wins) or 2 (passed wins)? local: 2
.subckt c1 d p=3
X1 d show v=p
.ends c1
* 2. the same, nothing passed: global 1, local 3
.subckt c2 d p=3
X1 d show v=p
.ends c2
* 3. a value passed two levels up, read below: global 1 or 5? local 5
.subckt c3leaf d
X1 d show v=p
.ends c3leaf
.subckt c3 d p=4
X1 d c3leaf
.ends c3
* 4. an expression passed on an instance line, computed where the line is
*    (the parent's k=7, q=8): 15; not where it goes (the child's default
*    k=100 and .param q=200): 300
.subckt c4child d s=0 k=100
.param q=200
X1 d show v='s'
.ends c4child
.subckt c4 d k=7 q=8
X1 d c4child s='k+q'
.ends c4
* 5. a .param in the subckt and a value passed for the same name:
*    passed (2), .param (5) or default (3)?
.subckt c5 d p=3
.param p=5
X1 d show v=p
.ends c5
* 6. a default read by another default, one passed: 2*6 = 12
.subckt c6 d a=1 b='a*2'
X1 d show v=b
.ends c6
* 7. the limits of a global model read the global `lb` (2e-7), not the
*    subckt's: L=5e-7 is in nch.2 (with lb=1e-6 it would be nch.1)
.subckt c7 d g lb=1e-6
M1 d g 0 0 nch w=1u l=5e-7
.ends c7
* 8. NF=8 fingers of W=4u (0.5u each), no WNFLAG: by the total width nfa.2,
*    by the width per finger nfa.1
M8 d8 g 0 0 nfa w=4u l=1u nf=8
* 9. the same, with WNFLAG=1 in the model: nfb.1
M9 d9 g 0 0 nfb w=4u l=1u nf=8
* 10. L 0.5 nm below LMAX=1u: edg.1 if the limits are exact; edg.2 if a
*     value within 1 nm of a limit counts as on it (ngspice)
M10 d10 g 0 0 edg w=1u l=0.9995u
* 11. L on the edge: edg.2
M11 d11 g 0 0 edg w=1u l=1u

Xc1 d c1 p=2
Xc2 d c2
Xc3 d c3 p=5
Xc4 d c4
Xc5 d c5 p=2
Xc6 d c6 a=6
Xc7 d7 g c7
Vd d 0 1
Vg g 0 0.5
V7 d7 0 1
V8 d8 0 1
V9 d9 0 1
V10 d10 0 1
V11 d11 0 1

.op
.tran 1n 2n
.meas tran case1 find v(xc1.x1.n) at=1n
.meas tran case2 find v(xc2.x1.n) at=1n
.meas tran case3 find v(xc3.x1.x1.n) at=1n
.meas tran case4 find v(xc4.x1.x1.n) at=1n
.meas tran case5 find v(xc5.x1.n) at=1n
.meas tran case6 find v(xc6.x1.n) at=1n
.meas tran case7 find i(v7) at=1n
.meas tran case8 find i(v8) at=1n
.meas tran case9 find i(v9) at=1n
.meas tran case10 find i(v10) at=1n
.meas tran case11 find i(v11) at=1n
.end
