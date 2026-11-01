* func_shadow_sub: a .param function in a subckt with the name of a built-in function
.subckt cs a b
.param sgn(x)='x*7'
rs a b 'sgn(1)*1k'
.ends
v1 1 0 1
x1 1 0 cs
r2 2 0 3k
v2 2 0 1
.op
.end
