* elem_dup_name_sub: two resistor lines with one name inside a used subckt
.subckt cell a b
rc a b 2
rc a b 3
.ends
v1 1 0 1
x1 1 0 cell
.op
.end
