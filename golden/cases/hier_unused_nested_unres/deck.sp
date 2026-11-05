* hier_unused_nested_unres: a used subckt nests an unused definition that calls an undefined name
.subckt outer a b
.subckt helper a b
xq a b nosuch
.ends
ro a b 5
.ends
v1 1 0 1
x1 1 0 outer
.op
.end
