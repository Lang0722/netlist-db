* func_sticky_local: one definition that calls f, with instances that find f at the top level or in the parent (parhier=local)
.option parhier=local
.param f(x)='x*2'
* (1) leafc: the instance that finds the top-level f (under w) is created before the one under cf
.subckt w a b
xl a b leafc
.ends
.subckt cf a b
.param f(x)='x*3'
xl a b leafc
.ends
.subckt leafc a b
rc a b 'f(1)*1k'
.ends
x0 10 0 w
x1 11 0 cf
* (2) leafd: the instance that finds the parent's f (under cf2) is created before the one under w2
.subckt w2 a b
xl a b leafd
.ends
.subckt cf2 a b
.param f(x)='x*3'
xl a b leafd
.ends
.subckt leafd a b
rd a b 'f(1)*1k'
.ends
x3 13 0 cf2
x4 14 0 w2
v10 10 0 1
v11 11 0 1
v13 13 0 1
v14 14 0 1
.op
.end
