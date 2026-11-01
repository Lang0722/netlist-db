* lib_once_param_only: one .lib call (only a .param), then a .param of the same name
.lib 'm.lib' tt
.param pl=5k
v1 1 0 1
r1 1 0 pl
.op
.end
