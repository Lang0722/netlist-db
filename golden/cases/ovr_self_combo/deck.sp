* ovr_self_combo: overrides that name the overridden name, functions in two instance levels (parhier=local)
.option parhier=local
.param w=4
.subckt cc a b w=2 l=1
r1 a b l
r2 a b w
.ends
* (1) a self-reference and a use of the same name on one line
x1 1 0 cc w='w*2' l='w*3'
* (2) a self-reference one level down, where the parent instance defines w
.subckt mid a b w=3
xc a b cc w='w*2'
.ends
x2 2 0 mid
* (3) a self-reference where the new instance has a body .param of the name
.subckt cb a b
.param w=2
r3 a b w
.ends
x3 3 0 cb w='w*3'
* (4) a header default that names the overridden name
.subckt ce a b w=2 l='w*3'
r4 a b l
.ends
x4 4 0 ce w='w*2'
* (5) a function in two instance levels above the call, none at the top level
.subckt fo a b
.param fn(x)='x*5'
xm a b fm
.ends
.subckt fm a b
.param fn(x)='x*7'
xl a b fl
r6 a b 'fn(1)*1k'
.ends
.subckt fl a b
r5 a b 'fn(1)*1k'
.ends
x5 5 0 fo
v1 1 0 1
v2 2 0 1
v3 3 0 1
v4 4 0 1
v5 5 0 1
.op
.end
