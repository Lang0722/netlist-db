* p8_func_free_local: a top-level function body names k, which the calling subckt also defines (parhier=local)
.option parhier=local
.param k=2
.param h(x)='x*k'
.subckt ck a b k=7
r1 a b 'h(1)*1k'
.ends
x1 1 0 ck
r0 2 0 'h(1)*1k'
v1 1 0 1
v2 2 0 1
.op
.end
