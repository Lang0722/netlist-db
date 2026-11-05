* func_first_inst_local: one definition that calls a .param function, in several instance contexts (parhier=local)
.option parhier=local
.param f(x)='x*2'
* (1) leaf calls f; its only instance is inside cf, which defines f
.subckt leaf a b
rl a b 'f(1)*1k'
.ends
.subckt cf a b
.param f(x)='x*3'
xl a b leaf
.ends
x1 1 0 cf
* (2) leaf3 calls g, inside ca (g = x*3) and inside cb (g = x*5); the ca instance comes first
.subckt leaf3 a b
r3 a b 'g(1)*1k'
.ends
.subckt ca a b
.param g(x)='x*3'
xl a b leaf3
.ends
.subckt cb a b
.param g(x)='x*5'
xl a b leaf3
.ends
x2 2 0 ca
x4 4 0 cb
* (3) leaf4 calls h, inside cc (h = x*3) and inside cd (h = x*5); the cd instance comes first
.subckt leaf4 a b
r4 a b 'h(1)*1k'
.ends
.subckt cc a b
.param h(x)='x*3'
xl a b leaf4
.ends
.subckt cd a b
.param h(x)='x*5'
xl a b leaf4
.ends
x6 6 0 cd
x8 8 0 cc
v1 1 0 1
v2 2 0 1
v4 4 0 1
v6 6 0 1
v8 8 0 1
.op
.end
