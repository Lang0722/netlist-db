* hier_dotted_collision: a top-level instance name with a dot equals the path of an instance one level down
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
.ends
v1 1 0 1
x0 1 t2 wrap
rt2 t2 0 1
x0.xq 1 t1 cell3
rt1 t1 0 1
.op
.end
