* hier_m_negative: an X line with a negative m
.subckt cell a b
rc a b 1k
.ends
v1 1 0 1
x1 1 0 cell m=-1
v2 2 0 2
x2 2 0 cell
.op
.end
