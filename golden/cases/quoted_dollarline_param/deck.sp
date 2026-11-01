* quoted_dollarline_param: a $ comment line inside a quoted .param value that goes on on a + line
.param a=2k
.param q='a
$ a dollar comment line
+ *2'
v1 1 0 1
r1 1 0 q
rc 1 0 1k
.op
.end
