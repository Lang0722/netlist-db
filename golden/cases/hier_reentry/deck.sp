* hier_reentry: a definition re-entered on one instance path; dynamic binding ends the recursion
.subckt aa p q
.subckt zz p q
rz p q 9
.ends
ra p q 1
xb p q bb
.ends
.subckt bb p q
.subckt bb p q
rn p q 3
.ends
rb p q 2
xa p q aa
.ends
v1 1 0 1
x1 1 0 aa
.op
.end
