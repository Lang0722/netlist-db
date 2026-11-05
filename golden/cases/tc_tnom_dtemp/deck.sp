* tc_tnom_dtemp: TC1/TC2 with .option tnom and dtemp=
.option list tnom=50
.temp 125
.param a=2k
v1 1 0 1
r1 1 0 1k tc1=0.004
r2 1 0 1k tc1=0.004 dtemp=20
r3 1 0 1k 0.004 1e-5
r4 1 0 a '0.004
+ *1' r=5k
r5 1 0 1k tc1=0.004 scale=2
r6 1 0 1k tc1=0.004 dtemp=-75
rn 1 0 1k
.op
.end
