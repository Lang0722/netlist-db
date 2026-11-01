* cl_tc_positional: bare numbers after a C or L value at .temp 125 (tc1, tc2?)
.option list
.temp 125
.model cmod c
.model lmod l level=3
v1 1 0 1
C1 1 0 cmod 1p 0.004
C2 1 0 1p 0.004
C3 1 0 cmod 1p 0.004 1e-5
C4 1 0 1p 0.004 1e-5
C5 1 0 cmod 1p tc1=0.004
C6 1 0 1p tc1=0.004 tc2=1e-5
C7 1 0 1p
L1 1 11 1n lmod 0.004
L2 1 12 1n 0.004
L3 1 13 1n lmod 0.004 1e-5
L4 1 14 1n 0.004 1e-5
L5 1 15 1n lmod tc1=0.004
L6 1 16 1n tc1=0.004 tc2=1e-5
L7 1 17 1n
r11 11 0 1k
r12 12 0 1k
r13 13 0 1k
r14 14 0 1k
r15 15 0 1k
r16 16 0 1k
r17 17 0 1k
.op
.end
