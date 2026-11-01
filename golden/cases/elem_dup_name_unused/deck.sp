* elem_dup_name_unused: two resistor lines with one name inside a subckt that no instance path reaches
.subckt cell a b
rc a b 2
.ends
.subckt spare a b
rs a b 2
rs a b 3
.ends
v1 1 0 1
x1 1 0 cell
.op
.end
