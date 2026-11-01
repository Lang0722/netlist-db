* rcl_quoted_more: a quoted value in the R, C or L value place, then the value keyword (no .temp)
.option list
.param rval=2k cval=2p lval=2n
* (1) a quoted number, then r=
r14 14 0 '2k' r=5k
* (2) a quoted expression, then tc1= and r=
r15 15 0 'rval*1' tc1=0.004 r=5k
* (3) a quoted expression, then c=
c1 31 0 'cval' c=5p
c2 32 0 'cval*1' c=5p
* (4) a quoted expression, then l=
l1 33 34 'lval' l=5n
l2 35 36 'lval*1' l=5n
r33 34 0 1k
r35 36 0 1k
v14 14 0 1
v15 15 0 1
v31 31 0 1
v32 32 0 1
v33 33 0 1
v35 35 0 1
.op
.end
