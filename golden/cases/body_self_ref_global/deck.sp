* body_self_ref_global: a body .param whose text names its own name, under the default parhier
.param w=4
.subckt cb a b w=2
.param w='w*3'
r1 a b w
.ends
v1 1 0 1
x1 1 0 cb
x2 1 0 cb w=7
.op
.end
