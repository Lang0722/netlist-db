* func_global_more: names in function bodies and calls in overrides under the default parhier (GLOBAL)
.param kk=9
* (1) a free name in the body of a function defined in ck, which ck and the top level both define
.subckt ck a b
.param kk=2
.param hk(x)='x*kk*1k'
rk a b 'hk(1)'
.ends
x1 11 0 ck
* (2) a function argument with the name of a top-level parameter, in a top-level function and in a function of cy
.param y=10
.param g2(y)='y*3'
r2 12 0 'g2(1)*1k'
.subckt cy a b
.param g3(y)='y*5'
ry a b 'g3(1)*1k'
.ends
x3 13 0 cy
* (4) a call in an X-line override, where the holder cp and the new instance cc both define fo (no top-level fo)
.subckt cp a b
.param fo(x)='x*3'
xc a b cc v='fo(1)*1k'
.ends
.subckt cc a b v=1
.param fo(x)='x*5'
rc a b v
.ends
x4 14 0 cp
* (5) ch defines hn, whose body calls fn; ch's parent cq and ch's child kh define fn (no top-level fn); kh calls hn
.subckt cq a b
.param fn(x)='x*3'
xh a b ch
.ends
.subckt ch a b
.param hn(x)='fn(x)*1k'
xk a b kh
.ends
.subckt kh a b
.param fn(x)='x*7'
rh a b 'hn(1)'
.ends
x5 15 0 cq
* (6) a free name in the body of a function of cm, which only cm's child defines as a default and which cm's parent defines
.subckt cr a b
.param qq=4
xm a b cm
.ends
.subckt cm a b
.param hq(x)='x*qq*1k'
xk a b km
.ends
.subckt km a b qq=8
rq a b 'hq(1)'
.ends
x6 16 0 cr
v11 11 0 1
v12 12 0 1
v13 13 0 1
v14 14 0 1
v15 15 0 1
v16 16 0 1
.op
.end
