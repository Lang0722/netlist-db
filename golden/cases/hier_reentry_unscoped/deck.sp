* hier_reentry_unscoped: a subckt with no nested definitions re-entered on one instance path
.subckt a p q
ra p q 1
xs p q s
.ends
.subckt s p q
.subckt s p q
rn p q 3
.ends
rs p q 2
xa p q a
.ends
v1 1 0 1
x1 1 0 a
.op
.end
