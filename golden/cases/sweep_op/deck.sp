* sweep_op: the .op values of parameters that a .dc or a .data sweep changes
.param pv=2k
.param pd=3k
.param pq='pv*2'
v1 1 0 1
r1 1 0 pv
r2 1 0 pd
r3 1 0 pq
.data d1 pd
+ 1k
+ 4k
.enddata
.dc pv 1k 3k 1k
.tran 1n 2n sweep data=d1
.print dc i(r1) i(r3)
.print tran i(r2)
.op
.end
