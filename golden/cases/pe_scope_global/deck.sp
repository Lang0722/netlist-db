* pe_scope_global: .param texts that name a name only a lower instance defines (default parhier)
* (1) a top-level .param text names b, which only the subckt defines
.param a='b*2'
.subckt s x y b=5
r1 x y a
.ends
x1 1 0 s
* (2) a body .param text names c, which only a child instance defines
.subckt mid x y
.param ee='c*3'
xl x y leaf
.ends
.subckt leaf x y c=7
r3 x y ee
.ends
x2 2 0 mid
v1 1 0 1
v2 2 0 1
.op
.end
