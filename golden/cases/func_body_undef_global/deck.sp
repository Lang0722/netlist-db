* func_body_undef_global: a top-level .param function that calls a function defined only in the subckt that calls it (default parhier; the name hq is not an output keyword)
.param g(x)='hq(x)*1k'
.subckt ch a b
.param hq(x)='x*5'
r1 a b 'g(1)'
.ends
x1 1 0 ch
v1 1 0 1
.op
.end
