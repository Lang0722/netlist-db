* rcl_tc_kw_after: a parameter name in the R value place, two numbers, then r= (no .model rval)
.option list
.temp 125
.param rval=2k
r4 4 0 rval 0.004 1e-5 r=5k
r5 5 0 5k tc1=0.004 tc2=1e-5
v4 4 0 1
v5 5 0 1
.op
.end
