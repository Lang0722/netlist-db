* ovr_scope_child_global: X-line override text names a name that only the new instance defines (default parhier)
.subckt cc a b w=2 l=1
r1 a b l
.ends
x1 1 0 cc l='w*3'
.subckt cb a b l=1
.param v=2
r2 a b l
.ends
x2 2 0 cb l='v*3'
v1 1 0 1
v2 2 0 1
.op
.end
