* elem_dotted_name_collision: a device name with a dot gives the full name of a device in a top-level instance with a dot
.subckt cell a b
rb a b 2
.ends
.subckt wrap a b
ra.rb a b 3
.ends
v1 1 0 1
x0.ra 1 t1 cell
rt1 t1 0 1
x0 1 t2 wrap
rt2 t2 0 1
.op
.dc v1 1 1 1
.print dc i(x0.ra.rb) i(rt1) i(rt2)
.end
