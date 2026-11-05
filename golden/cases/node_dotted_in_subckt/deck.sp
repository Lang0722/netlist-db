* node_dotted_in_subckt: v(x1.n.1) names the node n.1 of instance x1 (there is no instance x1.n)
.subckt cell a b
r1 a n.1 1k
r2 n.1 b 1k
.ends
v1 in 0 2
x1 in 0 cell
.tran 1n 2n
.meas tran vx find v(x1.n.1) at=1n
.end
