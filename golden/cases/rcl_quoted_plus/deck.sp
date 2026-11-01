* rcl_quoted_plus: quoted R, C and L values that go on on a + line
.option list
.param rval=2k cval=2p lval=2n
v1 1 0 1
r1 1 0 'rval
+ *3'
r2 1 0 'rval*2'
+ r=5k
r3 1 0
+ 'rval*4'
r4 1 0 'rval
+ *5' r=9k
r5 1 0 'rval+
+ 1k'
r6 1 0 'rval*
+ 3' tc1=0.004
c1 2 0 'cval
+ *3'
v2 2 0 1
l1 3 4 'lval
+ *3'
v3 3 0 1
r7 4 0 1k
.op
.end
