* c_corner_ff: the same library, corner ff (dvg=-0.05)
.lib 'cards_pdk.lib' ff
X1 d g 0 0 nch_mac l=50n
X2 d g 0 0 nch_mac l=500n w=2u nf=2
.model ctl nmos level=54 vth0=0.3
+ k1=0 k2=0 k3=0 dvt0=0 dvt0w=0 eta0=0 etab=0 dvtp0=0 lpe0=0 lpeb=0 lint=0 wint=0 xl=0 xw=0
mctl d g 0 0 ctl l=1u w=1u
Vd d 0 1
Vg g 0 1
.option post=0 ingold=2 numdgt=10 measdgt=10
.op
.dc Vd 1 1 1
.meas dc ctl find lv9(mctl) at=1
.meas dc p0 find lv9(x1.m0) at=1
.meas dc p1 find lv9(x2.m0) at=1
.end
