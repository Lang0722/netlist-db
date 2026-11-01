* ovr_self_global: an X-line override whose text names the overridden name (default parhier, no top-level w)
.subckt cc a b w=2
r1 a b w
.ends
x1 1 0 cc w='w*3'
v1 1 0 1
.op
.end
