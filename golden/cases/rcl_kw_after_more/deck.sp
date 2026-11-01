* rcl_kw_after_more: r= after a parameter name or a number in the R value place, with and without TC numbers
.option list
.temp 125
.param rval=2k
r6 6 0 rval r=5k
r7 7 0 1k 0.004 1e-5 r=5k
r8 8 0 rval 0.004 r=5k
r9 9 0 1k 0.004 r=5k
v6 6 0 1
v7 7 0 1
v8 8 0 1
v9 9 0 1
.op
.end
