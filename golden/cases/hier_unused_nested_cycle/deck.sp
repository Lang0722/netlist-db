* hier_unused_nested_cycle: a used subckt nests an unused definition that calls itself
.subckt outer a b
.subckt loop a b
rl a b 1
xl a b loop
.ends
ro a b 5
.ends
v1 1 0 1
x1 1 0 outer
.op
.end
