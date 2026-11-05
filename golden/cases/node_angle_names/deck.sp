* node_angle_names: node names with < and > (bus bits from Virtuoso)
v1 net<3> 0 1
r1 net<3> a<0> 1k
r2 a<0> 0 1k
r3 a<0> b>c 2k
r4 b>c 0 2k
.subckt cell d<1> q
rc d<1> q 4k
.ends
x1 a<0> 0 cell
.op
.end
