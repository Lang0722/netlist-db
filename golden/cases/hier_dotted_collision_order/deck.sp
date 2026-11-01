* hier_dotted_collision_order: two level-2 instances get one path; the parent with the shorter name comes first
.subckt cella a b
rc a b 2
.ends
.subckt cellb a b
rc a b 3
.ends
.subckt mid a b
xb a b cella
.ends
.subckt wrap a b
xa.xb a b cellb
.ends
v1 1 0 1
x0 1 t1 wrap
rt1 t1 0 1
x0.xa 1 t2 mid
rt2 t2 0 1
.op
.end
