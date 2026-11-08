* c_nested_local: a card in outer, read by a MOSFET in inner, which defines dv too (parhier=local)
.option parhier=local
.subckt outer d g s dv=0.03
.model mo nmos level=54 vth0='0.2+dv'
+ k1=0 k2=0 k3=0 dvt0=0 dvt0w=0 eta0=0 etab=0 dvtp0=0 lpe0=0 lpeb=0 lint=0 wint=0 xl=0 xw=0
.subckt inner d g s dv=0.05
M1 d g s s mo l=1u w=1u
.ends inner
X1 d g s inner
.ends outer
X1 d g 0 outer
.model ctl nmos level=54 vth0=0.3
+ k1=0 k2=0 k3=0 dvt0=0 dvt0w=0 eta0=0 etab=0 dvtp0=0 lpe0=0 lpeb=0 lint=0 wint=0 xl=0 xw=0
mctl d g 0 0 ctl l=1u w=1u
Vd d 0 1
Vg g 0 1
.option post=0 ingold=2 numdgt=10 measdgt=10
.op
.dc Vd 1 1 1
.meas dc ctl find lv9(mctl) at=1
.meas dc p0 find lv9(x1.x1.m1) at=1
.end
