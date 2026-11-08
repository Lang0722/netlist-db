* c_diode: a diode card inside a subckt, evaluated for each instance?
.subckt dcell p n k=1
.model dm d is='1e-14*k' n=1
D1 p n dm
.ends dcell
X1 p 0 dcell
X2 p 0 dcell k=4
.model dcc d is=1e-14 n=1
dctl p 0 dcc
Vp p 0 0.6
.option post=0 ingold=2 numdgt=10 measdgt=10
.op
.dc Vp 0.6 0.6 0.6
.meas dc ctl find i(dctl) at=0.6
.meas dc p0 find i(x1.d1) at=0.6
.meas dc p1 find i(x2.d1) at=0.6
.end
