* brace_probe: {...} in a .probe argument
.param a=1
v1 a 0 1
r1 a 0 1k
.tran 1n 2n
.probe v({a})
.end
