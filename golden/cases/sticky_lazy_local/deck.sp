* sticky_lazy_local: texts that an earlier instance does not evaluate, and function bodies (parhier=local)
.option parhier=local
.param f(x)='x*2'
* (1) leafp calls f in a body .param; the X line of its first instance (under w1) overrides it, the later one (under cf1, f = x*3) does not
.subckt leafp a b
.param p='f(1)*1k'
rp a b p
.ends
.subckt w1 a b
xl a b leafp p=5k
.ends
.subckt cf1 a b
.param f(x)='x*3'
xl a b leafp
.ends
x0 10 0 w1
x1 11 0 cf1
* (2) leafq calls f only in the false branch of a ternary; its first instance (under w2, s=1) does not take it, the later one (under cf2, s=0) does
.subckt leafq a b s=1
rq a b 's>0 ? 5k : f(1)*1k'
.ends
.subckt w2 a b
xl a b leafq s=1
.ends
.subckt cf2 a b
.param f(x)='x*3'
xl a b leafq s=0
.ends
x2 12 0 w2
x3 13 0 cf2
* (3) ch defines h, whose body calls f, and calls h in an element line; ch's first instance is under w3 (no f), the later one under cf3 (f = x*3)
.subckt ch a b
.param h(x)='f(x)*1k'
rh a b 'h(1)'
.ends
.subckt w3 a b
xc a b ch
.ends
.subckt cf3 a b
.param f(x)='x*3'
xc a b ch
.ends
x4 14 0 w3
x5 15 0 cf3
* (4) ck defines hk, whose body calls f, and calls hk only in a body .param pk; the X line of ck's first instance (under w4) overrides pk
.subckt ck a b
.param hk(x)='f(x)*1k'
.param pk='hk(1)'
rk a b pk
.ends
.subckt w4 a b
xc a b ck pk=5k
.ends
.subckt cf4 a b
.param f(x)='x*3'
xc a b ck
.ends
x6 16 0 w4
x7 17 0 cf4
* (5) leafu has a body .param u that calls f, used only in the false branch of a ternary; its first instance (under w5, s=1) does not use u
.subckt leafu a b s=1
.param u='f(1)*1k'
ru a b 's>0 ? 5k : u'
.ends
.subckt w5 a b
xl a b leafu s=1
.ends
.subckt cf5 a b
.param f(x)='x*3'
xl a b leafu s=0
.ends
x8 18 0 w5
x9 19 0 cf5
* (6) a function argument with the name of a parameter of the calling scope
.param y=10
.param g2(y)='y*3'
.subckt cy a b y=7
ry a b 'g2(1)*1k'
.ends
x10 20 0 cy
r11 21 0 'g2(1)*1k'
.subckt cz a b z=7
.param g3(z)='z*5'
rz a b 'g3(1)*1k'
.ends
x12 22 0 cz z=9
v10 10 0 1
v11 11 0 1
v12 12 0 1
v13 13 0 1
v14 14 0 1
v15 15 0 1
v16 16 0 1
v17 17 0 1
v18 18 0 1
v19 19 0 1
v20 20 0 1
v21 21 0 1
v22 22 0 1
.op
.end
