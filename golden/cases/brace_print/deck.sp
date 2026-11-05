* brace_print: {...} in a .print argument
.param a=1
v1 a 0 1
r1 a 0 1k
.tran 1n 2n
.print v({a})
.end
