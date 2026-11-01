* hier_top_nested_only: a top-level X line calls a name that is defined only nested in outer
.subckt outer a b
.subckt only a b
ro a b 5
.ends
xo a b only
.ends
v1 1 0 1
x1 1 0 outer
v2 2 0 2
x2 2 0 only
.op
.end
