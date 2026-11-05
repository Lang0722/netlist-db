* dup_param: the same .param name defined twice
.param a=1
.param a=2
.param c=1 c=3
.param d=8
rd 1 0 d
.subckt sd x y
.param b=5
.param b=6
rb x y b
.ends
ra 1 0 a
rc 1 0 c
xd 1 0 sd
v1 1 0 1
.param d=7
.op
.end
