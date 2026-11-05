* quoted_plus_tc: a quoted positional TC2 that goes on on a + line
.option list
.param rval=2k
v1 1 0 1
R1 1 0 rval '0.004'
+ '1e-5
+ *1'
R2 1 0 rval '0.004'
+ '1e-5
+ *2'
.op
.end
