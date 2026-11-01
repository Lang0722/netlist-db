* lib_twice_param_only: the same .lib section (only a .param) called twice
.lib 'm.lib' tt
.param pl=5k
.lib 'm.lib' tt
v1 1 0 1
r1 1 0 pl
.op
.end
