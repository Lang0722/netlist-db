* alter: first run plus two .alter runs
.param p=1k
v1 1 0 1
r1 1 0 p
r2 1 0 2k
.op
.alter
.param p=3k
.alter
r2 1 0 5k
.end
