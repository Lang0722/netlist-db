* parhier_default: whowins with no parhier option (expect x1.r1 = 4)
.subckt parvspar a b whowins=1.0
.param whowins=5.0
r1 a b whowins
.ends
.param whowins=4.0
v1 1 0 1.0
x1 1 0 parvspar whowins=3.0
.op
.end
