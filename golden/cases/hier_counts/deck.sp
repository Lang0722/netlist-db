* hier_counts: 3 levels, several call sites, an unused subckt, m=2 on an X line
* Every instance has its own port nodes: HSPICE merges identical subckt
* instances that sit in parallel, and its .op listing then hides them.
.model nch nmos level=1
.subckt leaf a b
rmk_leaf a b 1k
mn a a b b nch w=1u l=1u
.ends
.subckt mid a b
rmk_mid a b 2k
cm a b 1p
xl1 a n1 leaf
xl2 a n2 leaf
xl3 a n3 leaf
rs1 n1 b 11
rs2 n2 b 12
rs3 n3 b 13
.ends
.subckt blk a b
rmk_blk a b 3k
xm1 a n1 mid
xm2 a n2 mid m=2
xl0 a n3 leaf
rs1 n1 b 21
rs2 n2 b 22
rs3 n3 b 23
.ends
.subckt unused a b
rmk_unused a b 4k
.ends
v1 1 0 1
x1 1 t1 blk
x2 1 t2 blk
x3 1 t3 mid
rt1 t1 0 31
rt2 t2 0 32
rt3 t3 0 33
.op
.end
