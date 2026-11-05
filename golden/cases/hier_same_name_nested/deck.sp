* hier_same_name_nested: X lines that call the name of a definition on their own instance path
.subckt cell a b
.subckt cell a b
rin a b 2
.ends
rout a b 1
xc a b cell
.ends
.subckt top2 a b
.subckt top2 a b
rt2 a b 4
.ends
xw a b wrap
.ends
.subckt wrap a b
rw a b 3
xt a b top2
.ends
v1 1 0 1
x1 1 0 cell
v2 2 0 2
x2 2 0 top2
.op
.end
