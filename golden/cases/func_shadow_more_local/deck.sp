* func_shadow_more_local: where a .param function with the name of a built-in function is used (parhier=local)
.option parhier=local
.param min(a,b)='a*b+10'
* (1) the top-level min, called in a subckt
.subckt cm a b
rm a b 'min(2,3)*1k'
.ends
x1 11 0 cm
* (2) cs defines sgn: a call in cs, in its child ks, at the top level, and in a top-level instance of ks that HSPICE creates before x2.xk
.subckt cs a b
.param sgn(x)='x*7'
rs a b 'sgn(1)*1k'
xk a b ks
.ends
.subckt ks a b
rk a b 'sgn(1)*1k'
.ends
x2 12 0 cs
r3 13 0 'sgn(1)*1k'
x4 14 0 ks
* (3) ks2 is like ks but has no top-level instance; wk (no sgn) holds another instance of it, after x7
.subckt cs2 a b
.param sgn(x)='x*7'
xk a b ks2
.ends
.subckt ks2 a b
rk2 a b 'sgn(1)*1k'
.ends
.subckt wk a b
xk a b ks2
.ends
x7 17 0 cs2
x8 18 0 wk
* (4) a top-level abs, called in a top-level .param and in a .subckt default
.param abs(x)='x*4'
.param pa='abs(1)*1k'
r5 15 0 pa
.subckt cd a b v='abs(2)*1k'
rd a b v
.ends
x6 16 0 cd
v11 11 0 1
v12 12 0 1
v13 13 0 1
v14 14 0 1
v15 15 0 1
v16 16 0 1
v17 17 0 1
v18 18 0 1
.op
.end
