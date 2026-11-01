* hier_dotted_collision_xline_order: two level-2 instances with one full name; in the first parent it is the second X line
.subckt cella a b
rc a b 2
.ends
.subckt cellb a b
rc a b 3
.ends
.subckt filler a b
rf a b 7
.ends
.subckt mid a b
xb a b cella
.ends
.subckt wrap a b
xp a b filler
xa.xb a b cellb
.ends
v1 1 0 1
x0 1 t1 wrap
rt1 t1 0 1
x0.xa 1 t2 mid
rt2 t2 0 1
.op
.dc v1 1 1 1
.print dc i(x0.xa.xb.rc) i(x0.xp.rf) i(rt1) i(rt2)
.end
