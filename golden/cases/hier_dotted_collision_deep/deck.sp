* hier_dotted_collision_deep: the longer first name leads to a deeper instance than the shorter one
.subckt leafa a b
rl a b 2
.ends
.subckt leafb a b
rl a b 3
.ends
.subckt mid2 a b
xc a b leafa
.ends
.subckt mid a b
xb a b mid2
.ends
.subckt wrap a b
xa.xb.xc a b leafb
.ends
v1 1 0 1
x0.xa 1 t2 mid
rt2 t2 0 1
x0 1 t1 wrap
rt1 t1 0 1
.op
.end
