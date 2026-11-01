* hier_port_unused: an unused subckt holds X lines whose node counts differ from the port count of the called subckt
.subckt cell a b
rc a b 2
.ends
.subckt spare a b
xm a b n3 cell
xf a cell
.ends
v1 1 0 1
x1 1 t1 cell
rt1 t1 0 1
.op
.end
