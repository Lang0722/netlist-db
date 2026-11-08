* c_r_local: the same, parhier=local
.option parhier=local
.param rs=50
.model rtop r rsh='rs*2'
.subckt rcell a b rs=10
.model rloc r rsh='rs*3'
R1 a b rloc l=10u w=1u
.ends rcell
X1 a 0 rcell
X2 a 0 rcell rs=20
R0 a 0 rtop l=10u w=1u
.model rc r rsh=100
rctl a 0 rc l=10u w=1u
Va a 0 1
.option post=0 ingold=2 numdgt=10 measdgt=10
.op
.dc Va 1 1 1
.meas dc ctl find i(rctl) at=1
.meas dc p0 find i(x1.r1) at=1
.meas dc p1 find i(x2.r1) at=1
.meas dc p2 find i(r0) at=1
.end
