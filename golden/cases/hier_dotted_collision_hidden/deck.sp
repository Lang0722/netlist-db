* hier_dotted_collision_hidden: a device that only the later of two instances with one full name holds
.subckt cella a b
rc a b 2
rz a b 5
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
.dc v1 1 1 1
.print dc i(x0.xa.xb.rz) i(x0.xa.xb.rc) i(x0.xa.xb.rq) i(rt2)
.end
