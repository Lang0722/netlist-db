* inc_twice: the same file (a .subckt and a .param) included twice
.include 'sub.inc'
.param pi=5k
.include 'sub.inc'
v1 1 0 1
v2 2 0 1
x1 1 0 cell
r1 2 0 pi
.op
.end
