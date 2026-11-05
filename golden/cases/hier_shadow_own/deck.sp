* hier_shadow_own: a name nested in P's own def and in an ancestor's def
.subckt cell a b
rc a b 1
.ends
.subckt outer a b c
.subckt cell a b
rc a b 2
.ends
xm a c mid
xd b c deep
xn a c mid2
.ends
.subckt mid a b
.subckt cell a b
rc a b 3
.ends
xc a b cell
.ends
.subckt mid2 a b
.subckt cell a b
rc a b 4
.ends
xd a b deep
.ends
.subckt deep a b
xc a b cell
.ends
v1 1 0 1
v2 2 0 2
x1 1 2 0 outer
v3 3 0 3
x2 3 0 mid
.op
.end
