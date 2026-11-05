* func_sticky_more_local: creation order, function names and children for the sticky top-level function (parhier=local)
.option parhier=local
.param f(x)='x*2'
.param g(x)='x*4'
* (1) leafc: the instance that finds only the top-level f is first in the text but deeper (level 3, under w>w2); the one under cf is at level 2
.subckt w a b
xm a b w2
.ends
.subckt w2 a b
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
* (2) leafg calls f and g: its first instance (under wg, g = x*5) finds only the top-level f; the later one is under cfg (f = x*3, g = x*7)
.subckt wg a b
.param g(x)='x*5'
xl a b leafg
.ends
.subckt cfg a b
.param f(x)='x*3'
.param g(x)='x*7'
xl a b leafg
.ends
.subckt leafg a b
rf a b 'f(1)*1k'
rg a b 'g(1)*1k'
.ends
x2 12 0 wg
x3 13 0 cfg
* (3) leafs calls f: its first instance (under ws) finds only the top-level f; under cfs (f = x*3), its child xk binds to the top-level kk, which calls f
.subckt ws a b
xl a b leafs
.subckt kk a b
rn a b 1k
.ends
.ends
.subckt cfs a b
.param f(x)='x*3'
xl a b leafs
.ends
.subckt leafs a b
rs a b 'f(1)*1k'
xk a b kk
.ends
.subckt kk a b
rk a b 'f(1)*1k'
.ends
x4 14 0 ws
x5 15 0 cfs
v10 10 0 1
v11 11 0 1
v12 12 0 1
v13 13 0 1
v14 14 0 1
v15 15 0 1
.op
.end
