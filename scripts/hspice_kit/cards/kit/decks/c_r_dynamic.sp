* c_r_dynamic: an R card: the one on the instance path (mid's), or the one that the line's definition sees (the top level)?
.model rm r rsh=200
.subckt leaf a b
R1 a b rm l=10u w=1u
.ends leaf
.subckt mid a b
.model rm r rsh=300
X1 a b leaf
.ends mid
X1 a 0 mid
X2 a 0 leaf
.model rc r rsh=100
rctl a 0 rc l=10u w=1u
Va a 0 1
.option post=0 ingold=2 numdgt=10 measdgt=10
.op
.dc Va 1 1 1
.meas dc ctl find i(rctl) at=1
.meas dc p0 find i(x1.x1.r1) at=1
.meas dc p1 find i(x2.r1) at=1
.end
