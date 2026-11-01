* func_sticky_ovr_local: which instances make a top-level function stick (parhier=local)
.option parhier=local
.param f(x)='x*2'
* (1) leafd calls f in a header default; the X line of its first instance (under w4) overrides it, the later one (under cf4, f = x*3) does not
.subckt w4 a b
xl a b leafd v=5k
.ends
.subckt cf4 a b
.param f(x)='x*3'
xl a b leafd
.ends
.subckt leafd a b v='f(1)*1k'
rd a b v
.ends
x0 10 0 w4
x1 11 0 cf4
* (2) one X line with a call in an override, in two instances of its holder mo: the first under w5 (no f), the later under cf5 (f = x*3)
.subckt mo a b
xl a b leafo v='f(1)*1k'
.ends
.subckt leafo a b v=1
ro a b v
.ends
.subckt w5 a b
xm a b mo
.ends
.subckt cf5 a b
.param f(x)='x*3'
xm a b mo
.ends
x2 12 0 w5
x3 13 0 cf5
* (3) leafb calls f in an element line; its instances are under cf6 (f = x*3), then w6 (no f), then cf6 again
.subckt leafb a b
rb a b 'f(1)*1k'
.ends
.subckt cf6 a b
.param f(x)='x*3'
xl a b leafb
.ends
.subckt w6 a b
xl a b leafb
.ends
x4 14 0 cf6
x5 15 0 w6
x6 16 0 cf6
v10 10 0 1
v11 11 0 1
v12 12 0 1
v13 13 0 1
v14 14 0 1
v15 15 0 1
v16 16 0 1
.op
.end
