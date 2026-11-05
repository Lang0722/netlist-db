* hier_unused_caller_reach: an unused subckt calls t, whose X line resolves only under outer
.subckt t a b
xn a b only
.ends
.subckt outer a b
.subckt only a b
ro a b 5
.ends
xt a b t
.ends
.subckt spare a b
xs a b t
.ends
v1 1 0 1
x1 1 0 outer
.op
.end
