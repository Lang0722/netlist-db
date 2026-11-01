* nested_def_reach: a top-level def calls names that exist only nested in outer
.subckt t a b
xn a b only
.ends
.subckt u a b
.subckt cell a b
rc a b 3
.ends
xd a b deep
.ends
.subckt outer a b c
.subckt only a b
ro a b 5
.ends
.subckt cell a b
rc a b 2
.ends
.subckt deep a b
xc a b cell
.ends
xt a c t
xu b c u
.ends
v1 1 0 1
v2 2 0 2
x1 1 2 0 outer
.op
.end
