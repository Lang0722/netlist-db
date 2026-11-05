* dup_subckt: two .subckt definitions with the same name
.subckt inv a b
r1 a b 1
.ends
.subckt inv a b
r1 a b 2
.ends
x1 1 0 inv
v1 1 0 1
.op
.end
