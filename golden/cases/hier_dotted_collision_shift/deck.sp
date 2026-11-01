* hier_dotted_collision_shift: circuits created after the later of two instances with one full name
.subckt leafk a b
rk a b 5
.ends
.subckt leafp a b
rp a b 7
.ends
.subckt cella a b
rc a b 2
xk a b leafk
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
.subckt other a b
xp a b leafp
ro a b 11
.ends
v1 1 0 1
x0 1 t1 wrap
rt1 t1 0 1
x0.xa 1 t2 mid
rt2 t2 0 1
x9 1 t3 other
rt3 t3 0 1
.op
.dc v1 1 1 1
.print dc i(x0.xa.xb.xk.rk) i(x0.xa.xb.rc) i(x9.xp.rp) i(x9.ro) i(rt3)
.end
