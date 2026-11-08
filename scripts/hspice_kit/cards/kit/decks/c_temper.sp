* c_temper: a card that reads temper, at .temp 50 (the temperature terms zeroed)
.temp 50
.model mtp nmos level=54 vth0='0.2+temper*2m'
+ k1=0 k2=0 k3=0 dvt0=0 dvt0w=0 eta0=0 etab=0 dvtp0=0 lpe0=0 lpeb=0 lint=0 wint=0 xl=0 xw=0 kt1=0 kt1l=0 kt2=0
M1 d g 0 0 mtp l=1u w=1u
.model ctl nmos level=54 vth0=0.3
+ k1=0 k2=0 k3=0 dvt0=0 dvt0w=0 eta0=0 etab=0 dvtp0=0 lpe0=0 lpeb=0 lint=0 wint=0 xl=0 xw=0 kt1=0 kt1l=0 kt2=0
mctl d g 0 0 ctl l=1u w=1u
Vd d 0 1
Vg g 0 1
.option post=0 ingold=2 numdgt=10 measdgt=10
.op
.dc Vd 1 1 1
.meas dc ctl find lv9(mctl) at=1
.meas dc p0 find lv9(m1) at=1
.end
