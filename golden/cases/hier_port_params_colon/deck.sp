* hier_port_forms: X lines whose node count equals the port count in several written forms
.subckt cell a b params: w=1
rc a b w
.ends
.subckt cell2 a b w=1
rc a b w
.ends
.subckt cell3 a
+ b c
rc a b 1
rd b c 1
.ends
.macro mac a b
rm a b 4
.eom
v1 1 0 1
x1 1 t1 cell w=2
rt1 t1 0 1
x2 1 t2 cell2 w=3 m=2
rt2 t2 0 1
x3 1 t3 cell2 $ inline comment
rt3 t3 0 1
x4 1
+ t4 0 cell3
rt4 t4 0 1
x5 1 t5 mac
rt5 t5 0 1
.op
.end
