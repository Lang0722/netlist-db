* ovr_self_local: an X-line override whose text names the overridden name (parhier=local)
.option parhier=local
.param w=4
.subckt cc a b w=2
r1 a b w
.ends
x1 1 0 cc w='w*3'
v1 1 0 1
.op
.end
