* macro_eom: .macro/.eom used as .subckt/.ends
.macro inv a b rv=3
r1 a b rv
.eom
v1 1 0 1
v2 2 0 1
x1 1 0 inv
x2 2 0 inv rv=4
.op
.end
