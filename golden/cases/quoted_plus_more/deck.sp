* quoted_plus_more: quoted texts that go on on + lines, in other places
.param rval=2k
.param q='rval
+ *2' $ it's q
.param f(x)='x*
+ 3k'
v1 1 0 1
r1 1 0 q
r2 1 0 'f(1)'
r3 1 0 'f(
+ 2)'
r4 1 0 r='rval
+ *5'
.subckt cell a b w='1k
+ *3' $ it's w
rc a b w
.ends
x1 1 0 cell
x2 1 0 cell w='rval
+ *4' $ it's
.op
.end
