* elem_m_zero_unused: m=0 on lines of a subckt that has no instances
.subckt leaf a b
rl a b 1k
.ends
.subckt spare a b
rs a b 2k m=0
xu a b leaf m=0
.ends
v1 1 0 1
x1 1 0 leaf
.op
.end
