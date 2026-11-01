* res_tc_positional: bare numbers after a resistor value at .temp 125 (tc1, tc2?)
.temp 125
v1 1 0 1
v2 2 0 1
v3 3 0 1
v4 4 0 1
v5 5 0 1
R1 1 0 1k 0.004
R2 2 0 1k 0.004 1e-5
R3 3 0 1k tc1=0.004
R4 4 0 1k tc1=0.004 tc2=1e-5
R5 5 0 1k
.op
.end
