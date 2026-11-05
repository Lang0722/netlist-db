* hier_port_fewer: an X line in a used subckt with fewer nodes than the called subckt has ports
.subckt cell a b c
rc a b 2
rd b c 3
.ends
.subckt wrap a b
xc a b cell
.ends
v1 1 0 1
x1 1 t1 wrap
rt1 t1 0 1
.op
.end
