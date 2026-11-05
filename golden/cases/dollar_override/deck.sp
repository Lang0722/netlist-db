* dollar_override: a $ inside a quoted X-line override
.param rval=2k
.subckt cell a b w=1
rc a b w
.ends
v1 1 0 1
x1 1 0 cell w='rval $ c'
r2 1 0 3k
.op
.end
