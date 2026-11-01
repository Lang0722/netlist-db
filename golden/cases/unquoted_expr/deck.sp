* unquoted_expr: expressions without quotes and without spaces
.param a=2k
.param b=a*2
.param c=a+1k
.param d=-a
.param e=2*a+1k
.param g=max(a,3k)
.param h=a/2
v1 1 0 1
rb 1 0 b
rc 1 0 c
rd 1 0 '-d'
re 1 0 e
rg 1 0 g
rh 1 0 h
.subckt cell n1 n2 w=1k l=w*3
rl n1 n2 l
rw n1 n2 w
.ends
x1 1 0 cell w=a*2
x2 1 0 cell
.op
.end
