* rcl_quoted_dollar_line: a $ inside a quoted value on one line, in a .param and in an R value
.param rval=2k
.param q='rval*2 $ c'
v1 1 0 1
r1 1 0 'rval*3 $ c'
r2 1 0 q
r3 1 0 3k
.op
.end
