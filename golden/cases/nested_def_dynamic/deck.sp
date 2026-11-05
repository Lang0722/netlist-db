* nested_def_dynamic: X lookup of a nested .subckt name follows the instance path, not the text
.subckt cell a b
rc a b 1
.ends
.subckt deep2 a b
xc a b cell
.ends
.subckt mid a b
xd a b deep2
.ends
.subckt outer a b c
.subckt cell a b
rc a b 2
.ends
xd a c deep2
xm b c mid
.ends
v1 1 0 1
v2 2 0 2
x1 1 2 0 outer
v3 3 0 3
x2 3 0 deep2
.op
.end
