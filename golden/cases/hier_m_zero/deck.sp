* hier_m_zero: an X line with m=0 and a resistor with m=0
.subckt cell a b
rc a b 1k
.ends
v1 1 0 1
x1 1 0 cell m=0
v2 2 0 2
x2 2 0 cell
r3 2 0 2k m=0
r4 2 0 4k
.op
.end
