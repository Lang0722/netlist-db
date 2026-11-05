* builtin_body_global: built-in function names in function bodies, defaults and overrides (default parhier)
* (1) ch defines h, whose body calls sgn; ch's first instance is under w3 (no sgn), the later one under cs3 (sgn = x*7)
.subckt ch a b
.param h(x)='sgn(x)*1k'
rh a b 'h(1)'
.ends
.subckt w3 a b
xc a b ch
.ends
.subckt cs3 a b
.param sgn(x)='x*7'
xc a b ch
.ends
x1 11 0 w3
x2 12 0 cs3
* (2) a top-level function whose body calls sgn, called in cs4, which defines sgn
.param g(x)='sgn(x)*1k'
.subckt cs4 a b
.param sgn(x)='x*5'
r3 a b 'g(1)'
.ends
x3 13 0 cs4
* (3) a top-level function whose body calls abs, which the top level defines after it
.param g2(x)='abs(x)*1k'
.param abs(x)='x*4'
r4 14 0 'g2(1)'
* (4) a call of max before the top-level .param max
r5 15 0 'max(1,2)*1k'
.param max(a,b)='a+b'
* (5) a .subckt default and an X-line override that call sgn, in a subckt that defines sgn
.subckt cd a b v='sgn(1)*1k'
.param sgn(x)='x*6'
rd a b v
.ends
x6 16 0 cd
x7 17 0 cd v='sgn(2)*1k'
v11 11 0 1
v12 12 0 1
v13 13 0 1
v14 14 0 1
v15 15 0 1
v16 16 0 1
v17 17 0 1
.op
.end
