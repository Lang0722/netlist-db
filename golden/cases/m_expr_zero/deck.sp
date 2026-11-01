* m_expr_zero: m given by an expression that is 0, on an X line
.param mz=0
.subckt t a b
r2 a b 1k
.ends
.subckt s a b
xq a b t m=mz
.ends
x1 1 0 s
v1 1 0 1
.op
.end
