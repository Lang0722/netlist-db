* p8_ovr_undeclared: X-line overrides of names the .subckt line does not declare (parhier=local)
.option parhier=local
.param q=2
.subckt cu a b w=1
r1 a b q
.ends
x1 1 0 cu q=5
.subckt cb a b
.param w=2
r2 a b w
.ends
x2 2 0 cb w=7
v1 1 0 1
v2 2 0 1
.op
.end
