* hier_dup_instance_unused: two X lines with one name inside a subckt that no instance path reaches
.subckt cell a b
rc a b 2
.ends
.subckt spare a b
xq a b cell
xq a b cell
.ends
v1 1 0 1
x1 1 0 cell
.op
.end
