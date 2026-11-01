* hier_dup_instance: two X lines with one instance name, at the top level and inside a subckt
.subckt cell a b
rc a b 2
.ends
.subckt cell3 a b
rc a b 3
.ends
.subckt wrap a b
xq a n1 cell
rq n1 b 1
xq a n2 cell3
rq2 n2 b 1
.ends
v1 1 0 1
x1 1 t1 wrap
rt1 t1 0 1
x1 1 t2 cell3
rt2 t2 0 1
.op
.end
