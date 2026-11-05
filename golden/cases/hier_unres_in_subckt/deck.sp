* hier_unres_in_subckt: an X line inside a subckt calls a name with no visible definition on one instance path
.subckt t a b
rt a b 1
xn a b only
.ends
.subckt outer a b
.subckt only a b
ro a b 5
.ends
xt a b t
.ends
v1 1 0 1
x1 1 0 outer
v2 2 0 2
x2 2 0 t
.op
.end
