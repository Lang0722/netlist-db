* hier_dup_nested: two nested definitions with one name in one parent
.subckt outer a b
.subckt cell a b
rc a b 2
.ends
.subckt cell a b
rc a b 3
.ends
xc a b cell
.ends
v1 1 0 1
x1 1 0 outer
.op
.end
