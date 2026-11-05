* inc_twice_param_only: the same file (only a .param) included twice
.include 'params.inc'
.param pi=5k
.include 'params.inc'
v1 1 0 1
r1 1 0 pi
.op
.end
