* inc_twice_params: the same file (only .param and .model) included twice
.include 'params.inc'
.param pi=5k
.include 'params.inc'
v1 1 0 1
v2 2 0 1
r1 1 0 pi
r2 2 0 rmod 3k
.op
.end
