* pe_scope_local: where the text of a winning definition is evaluated (parhier=local)
.option parhier=local
* (1) a top-level .param text names b; the instance that uses it defines b again
.param b=1
.param a='b*2'
.subckt s x y b=5
r1 x y a
.ends
x1 1 0 s
* (2) a body .param text names c; a child instance that uses it defines c again
.subckt mid x y c=1
.param ee='c*3'
r2 x y ee
xl x y leaf
.ends
.subckt leaf x y c=7
r3 x y ee
.ends
x2 2 0 mid
* (3) a header default text names g; a child instance that uses it defines g again
.subckt mid2 x y g=1 h='g*4'
xl x y leaf2
.ends
.subckt leaf2 x y g=9
r4 x y h
.ends
x3 3 0 mid2
* (4) an X-line override text names q; a child of the new instance uses the result and defines q again
.param q=1
.subckt mid3 x y
xl x y leaf3
.ends
.subckt leaf3 x y q=6
r5 x y p
.ends
x4 4 0 mid3 p='q*2'
v1 1 0 1
v2 2 0 1
v3 3 0 1
v4 4 0 1
.op
.end
