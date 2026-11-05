* inherit_local: names a nested subckt does not define, .option parhier=local
.option parhier=local
.param k=4
.subckt child a b
r1 a b kk
r2 a b k
r3 a b jj
.ends
.subckt parent a b kk=2 k=2 jj=6
xc a b child
.ends
x1 1 0 parent kk=3 k=3
v1 1 0 1
.op
.end
