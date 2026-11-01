* parhier_dup: two .option parhier lines, local first, then global
.option parhier=local
.param w=4
.subckt cc a b w=2
r1 a b w
.ends
x1 1 0 cc
.option parhier=global
v1 1 0 1
.op
.end
