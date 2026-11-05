* lib_twice: the same .lib section call twice
.lib 'm.lib' tt
.param pl=5k
.lib 'm.lib' tt
v1 1 0 1
v2 2 0 1
v3 3 0 1
x1 1 0 cell
r1 2 0 pl
r2 3 0 rmod 2k
.op
.end
