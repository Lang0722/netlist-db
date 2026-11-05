* func_args_cmtline: a comment line and a $ comment inside the argument list of a .param function
.param g(x,
* a comment line
+ y)='x+y'
.param h(x, $ c
+ y)='x*y*1k'
v1 1 0 1
r1 1 0 'g(1k,2k)'
r2 1 0 'h(2,3)'
.op
.end
