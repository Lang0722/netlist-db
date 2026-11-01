* hier_unused_cycle_unres: a call cycle and an undefined subckt name, both in unused definitions
.subckt loop a b
rl a b 1
xl a b loop
.ends
.subckt orphan a b
xq a b nosuch
.ends
.subckt used a b
ru a b 2
.ends
v1 1 0 1
x1 1 0 used
.op
.end
