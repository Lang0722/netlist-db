* func_scope_local: .param functions defined inside subckts (parhier=local)
.option parhier=local
.param f(x)='x*2'
.subckt leaf a b
rl a b 'f(1)*1k'
.ends
.subckt cf a b
.param f(x)='x*3'
r1 a b 'f(1)*1k'
xl a b leaf
.ends
.subckt leafg a b
rg a b 'g(1)*1k'
.ends
.subckt cg a b
.param g(x)='x*5'
r2 a b 'g(1)*1k'
xl a b leafg
.ends
x1 1 0 cf
x2 2 0 cg
xt 3 0 leaf
r0 4 0 'f(1)*1k'
v1 1 0 1
v2 2 0 1
v3 3 0 1
v4 4 0 1
.op
.end
