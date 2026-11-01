* rcl_name_first_cl: a parameter name in the C or L value place, then numbers
.option list
.temp 125
.param cval=2p lval=2n
v1 1 0 1
c1 1 0 cval 0.004
c2 2 0 cval 0.004 1e-5
c3 1 0 'cval' 0.004
l1 1 11 lval 0.004
l4 1 14 lval 0.004 1e-5
l5 1 15 'lval' 0.004
r11 11 0 1k
r14 14 0 1k
r15 15 0 1k
r2 2 0 1k
.op
.end
