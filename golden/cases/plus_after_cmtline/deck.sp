* plus_after_cmtline: + lines after a * comment line, in unquoted statements
.param p=1k
* a comment line
+ p=4k
.subckt cell a b w=1k
rc a b w
.ends
v1 1 0 1
r2 1 0 1k
* a comment line
+ r=5k
rp 1 0 p
x1 1 0 cell
* a comment line
+ w=3k
.op
.end
