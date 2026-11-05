* func_nested_local: a .param function that calls another function, a free name in a subckt function, an unquoted X-line override (parhier=local)
.option parhier=local
.param f(x)='x*2'
.param g(x)='f(x)*1k'
* (1) the top-level g calls f; the subckt that calls g defines its own f (x*3)
.subckt cf a b
.param f(x)='x*3'
r1 a b 'g(1)'
.ends
x1 1 0 cf
* (2) ch defines f (x*5) and h, which calls f; h is called in a child kh that defines f (x*7)
.subckt ch a b
.param f(x)='x*5'
.param h(x)='f(x)*1k'
xk a b kh
.ends
.subckt kh a b
.param f(x)='x*7'
r2 a b 'h(1)'
.ends
x2 2 0 ch
* (3) ck defines kk=2 and hk, which uses the free name kk; hk is called in a child that defines kk=9
.subckt ck a b
.param kk=2
.param hk(x)='x*kk*1k'
xk a b ckk
.ends
.subckt ckk a b
.param kk=9
r3 a b 'hk(1)'
.ends
x3 3 0 ck
* (4) an unquoted name as an X-line override value: the new instance has a default k=1, the top level k=5
.param k=5
.subckt cq a b k=1 w=1
r4 a b 'w*1k'
.ends
x4 4 0 cq w=k
v1 1 0 1
v2 2 0 1
v3 3 0 1
v4 4 0 1
.op
.end
