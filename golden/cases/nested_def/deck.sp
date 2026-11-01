* nested_def: a .subckt defined inside a .subckt; which definition an X line uses
.subckt cell a b
rc a b 1
.ends
.subckt outer a b
.subckt cell a b
rc a b 2
.ends
.subckt deep a b
xc a b cell
.ends
xc a b cell
xd a b deep
.ends
v1 1 0 1
x1 1 0 outer
x2 1 0 cell
.op
.end
