* elem_m_negative: a negative m on a resistor, at the top level and inside a subckt
.subckt cell a b
rc a b 1k m=-0.5
rd a b 3k
.ends
v1 1 0 1
x1 1 0 cell
r3 1 0 2k m=-1
r4 1 0 4k
.op
.end
