* hier_port_more: a top-level X line with more nodes than the subckt has ports
.subckt cell a b
rc a b 2
.ends
v1 1 0 1
x1 1 t1 t3 cell
rt1 t1 0 1
rt3 t3 0 1
.op
.end
