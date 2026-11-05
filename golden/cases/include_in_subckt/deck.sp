* include_in_subckt: .include inside a .subckt body
.subckt holder a b pv=3
.include 'inc.sp'
r0 a b 1
.ends
v1 1 0 1
x1 1 0 holder
x2 1 0 holder pv=5
.op
.end
