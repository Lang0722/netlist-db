* builtin_body_global_rev: ch defines h, whose body calls sgn; ch's first instance is under cs3 (sgn = x*7), the later one under w3 (no sgn) (default parhier)
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
x1 11 0 cs3
x2 12 0 w3
v11 11 0 1
v12 12 0 1
.op
.end
