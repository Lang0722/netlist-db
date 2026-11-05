* rcl_quoted_sticky_local: r= after a quoted value; a top-level f that sticks, called from a body .param, a header default and an X-line override (parhier=local)
.option parhier=local
.param rval=2k
* (1) a quoted expression in the R value place, then r=
r10 10 0 'rval' r=5k
r11 11 0 'rval*1' r=5k
* (2) a parameter name, then tc1=, then r=
r12 12 0 rval tc1=0.004 r=5k
* (3) control: a number, then r= (elem_dup_kw: 5k)
r13 13 0 2k r=5k
.param f(x)='x*2'
* (4) leafp calls f in a body .param; its instance under w is created first and finds only the top-level f
.subckt w a b
xl a b leafp
.ends
.subckt cf a b
.param f(x)='x*3'
xl a b leafp
.ends
.subckt leafp a b
.param v='f(1)*1k'
rp a b v
.ends
x0 20 0 w
x1 21 0 cf
* (5) leafh calls f in a header default
.subckt w2 a b
xl a b leafh
.ends
.subckt cf2 a b
.param f(x)='x*3'
xl a b leafh
.ends
.subckt leafh a b v='f(1)*1k'
rh a b v
.ends
x2 22 0 w2
x3 23 0 cf2
* (6) the X lines that make leafo call f in an override
.subckt w3 a b
xl a b leafo v='f(1)*1k'
.ends
.subckt cf3 a b
.param f(x)='x*3'
xl a b leafo v='f(1)*1k'
.ends
.subckt leafo a b v=1
ro a b v
.ends
x4 24 0 w3
x5 25 0 cf3
v10 10 0 1
v11 11 0 1
v12 12 0 1
v13 13 0 1
v20 20 0 1
v21 21 0 1
v22 22 0 1
v23 23 0 1
v24 24 0 1
v25 25 0 1
.op
.end
