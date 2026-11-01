* brace_meas: {...} in a .meas argument
.param a=1
v1 a 0 1
r1 a 0 1k
.tran 1n 2n
.meas tran m1 max v({a})
.end
