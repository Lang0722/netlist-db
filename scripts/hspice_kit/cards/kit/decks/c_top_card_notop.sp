* c_top_card_notop: a top-level card that reads a name only the MOSFETs' subckts define (parhier=global)
.option parhier=global
.model mt nmos level=54 vth0='0.2+dv'
+ k1=0 k2=0 k3=0 dvt0=0 dvt0w=0 eta0=0 etab=0 dvtp0=0 lpe0=0 lpeb=0 lint=0 wint=0 xl=0 xw=0
.subckt c5 d g s dv=0.05
M1 d g s s mt l=1u w=1u
.ends c5
.subckt c7 d g s dv=0.07
M1 d g s s mt l=1u w=1u
.ends c7
X1 d g 0 c5
X2 d g 0 c7
.model ctl nmos level=54 vth0=0.3
+ k1=0 k2=0 k3=0 dvt0=0 dvt0w=0 eta0=0 etab=0 dvtp0=0 lpe0=0 lpeb=0 lint=0 wint=0 xl=0 xw=0
mctl d g 0 0 ctl l=1u w=1u
Vd d 0 1
Vg g 0 1
.option post=0 ingold=2 numdgt=10 measdgt=10
.op
.dc Vd 1 1 1
.meas dc ctl find lv9(mctl) at=1
.meas dc p0 find lv9(x1.m1) at=1
.meas dc p1 find lv9(x2.m1) at=1
.end
