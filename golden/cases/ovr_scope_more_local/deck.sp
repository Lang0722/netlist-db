* ovr_scope_more_local: X-line override text and m in the scope of the new instance (parhier=local)
.option parhier=local
.param k=2 v=9
.subckt cb a b l=1
.param v=2
r1 a b l
.ends
x1 1 0 cb l='v*3'
.subckt cm a b k=1
r2 a b 1k
.ends
x2 2 0 cm m='k'
v1 1 0 1
v2 2 0 1
.op
.end
