* func_three_local: .param functions at the top level and in instance levels above the call (parhier=local)
.option parhier=local
.param f(x)='x*2'
* (1) a top-level f, and an f in two instance levels above the call
.subckt fo a b
.param f(x)='x*5'
xm a b fm
.ends
.subckt fm a b
.param f(x)='x*7'
xl a b fl
r6 a b 'f(1)*1k'
.ends
.subckt fl a b
r5 a b 'f(1)*1k'
.ends
x5 5 0 fo
* (2) no top-level g, a g in three instance levels above the call
.subckt ga a b
.param g(x)='x*3'
xb a b gb
.ends
.subckt gb a b
.param g(x)='x*5'
xc a b gc
.ends
.subckt gc a b
.param g(x)='x*7'
xd a b gd
.ends
.subckt gd a b
r7 a b 'g(1)*1k'
.ends
x7 7 0 ga
* (3) a call in an X-line override, where the new instance defines f
.subckt cf a b w=1
.param f(x)='x*3'
rw a b w
.ends
x3 3 0 cf w='f(1)*1k'
v3 3 0 1
v5 5 0 1
v7 7 0 1
.op
.end
