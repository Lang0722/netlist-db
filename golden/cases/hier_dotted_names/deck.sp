* hier_dotted_names: instance names with dots; one starts with the name of another top-level instance
.subckt cell a b
rc a b 2
.ends
.subckt cell3 a b
rc a b 3
.ends
.subckt wrap a b
rw a b 4
xq a n1 cell
rq n1 b 1
xm.n a n2 cell3
rm n2 b 1
.ends
v1 1 0 1
x0.aaa 1 t1 cell
rt1 t1 0 1
x0 1 t2 wrap
rt2 t2 0 1
x0.xm 1 t3 cell3
rt3 t3 0 1
.op
.end
