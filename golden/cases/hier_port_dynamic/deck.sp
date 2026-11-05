* hier_port_dynamic: one X line binds to nested definitions with different port counts on two instance paths
.subckt t a b
xn a b only
.ends
.subckt outer a b
.subckt only a b
ro a b 5
.ends
xt a b t
.ends
.subckt outer2 a b
.subckt only a b c
ro a c 6
rq c b 1
.ends
xt a b t
.ends
v1 1 0 1
x1 1 t1 outer
rt1 t1 0 1
x2 1 t2 outer2
rt2 t2 0 1
.op
.end
