* builtin_local: built-in function names under parhier=local
.option parhier=local
.param sgn(x)='x*2'
* (1) ks calls sgn; its first instance is under w1 (no sgn: the top-level sgn), the later one under cs1 (sgn = x*7)
.subckt ks a b
rk a b 'sgn(1)*1k'
.ends
.subckt w1 a b
xk a b ks
.ends
.subckt cs1 a b
.param sgn(x)='x*7'
xk a b ks
.ends
x1 11 0 w1
x2 12 0 cs1
* (2) the same with the instance under cs2 first
.subckt ks2 a b
rk2 a b 'sgn(1)*1k'
.ends
.subckt cs2 a b
.param sgn(x)='x*7'
xk a b ks2
.ends
.subckt w2 a b
xk a b ks2
.ends
x3 13 0 cs2
x4 14 0 w2
* (3) no top-level abs: the top-level instance x5 of ka (built-in) is created before x6.xk (ca defines abs)
.subckt ka a b
ra a b 'abs(1)*1k'
.ends
.subckt ca a b
.param abs(x)='x*4'
xk a b ka
.ends
x6 16 0 ca
x5 15 0 ka
* (4) ch4 defines h4, whose body calls min; its first instance is under w4 (no min), the later one under cm4 (min = a+b)
.subckt ch4 a b
.param h4(x)='min(x,5)*1k'
rh a b 'h4(1)'
.ends
.subckt w4 a b
xc a b ch4
.ends
.subckt cm4 a b
.param min(a,b)='a+b'
xc a b ch4
.ends
x7 17 0 w4
x8 18 0 cm4
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
