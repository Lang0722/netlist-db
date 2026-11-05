* probe_param_redefined: a top-level .param whose first text names b (defined nowhere), then defined again with a number
.param a='b*2'
.param a=5k
v1 1 0 1
r1 1 0 a
.op
.end
