* param_sticky_local: one definition that uses a .param name, with an earlier instance that finds the name only at the top level (parhier=local)
.option parhier=local
* (1) a body .param in the later parent
.param p=2k
.subckt w a b
xl a b leafp
.ends
.subckt cp a b
.param p=3k
xl a b leafp
.ends
.subckt leafp a b
rp a b p
.ends
x0 10 0 w
x1 11 0 cp
* (2) a header default in the later parent
.param q=2k
.subckt wq a b
xl a b leafq
.ends
.subckt cq a b q=3k
xl a b leafq
.ends
.subckt leafq a b
rq a b q
.ends
x2 12 0 wq
x3 13 0 cq
* (3) an expression that names the parameter
.param s=2k
.subckt ws a b
xl a b leafs
.ends
.subckt cs a b
.param s=3k
xl a b leafs
.ends
.subckt leafs a b
rs a b 's*1'
.ends
x4 14 0 ws
x5 15 0 cs
v10 10 0 1
v11 11 0 1
v12 12 0 1
v13 13 0 1
v14 14 0 1
v15 15 0 1
.op
.end
