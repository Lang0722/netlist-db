* node_dotted_collision: a top-level node named x1.n and the node n of instance x1
.subckt cell a b
r1 a n 1k
r2 n b 1k
.ends
v1 in 0 2
x1 in 0 cell
v2 x1.n 0 5
r3 x1.n 0 1k
.tran 1n 2n
.meas tran vx find v(x1.n) at=1n
.end
