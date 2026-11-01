* node_dotted_two_splits_b: v(x1.n.1) is node 1 of x1.n (2 V) or node n.1 of x1 (3 V); x1.n first
.subckt cella a b
r1 a 1 1k
r2 1 b 1k
.ends
.subckt cellb a b
r1 a n.1 1k
r2 n.1 b 3k
.ends
v1 in 0 4
x1.n in 0 cella
x1 in 0 cellb
.tran 1n 2n
.meas tran vx find v(x1.n.1) at=1n
.end
