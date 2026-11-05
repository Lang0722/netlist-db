* hier_dotted_collision_below: the later of two instances with one path has an instance below it; .print by path, no .op listing
.subckt leafk a b
rk a b 5
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
v1 1 0 1
x0 1 t1 wrap
rt1 t1 0 1
x0.xa 1 t2 mid
rt2 t2 0 1
.dc v1 1 1 1
.print dc i(x0.xa.xb.xk.rk) i(x0.xa.xb.xk.rc) i(x0.xa.xb.rc) i(rt2)
.end
