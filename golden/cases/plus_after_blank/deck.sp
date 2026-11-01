* plus_after_blank: + lines after a blank line or a $ line, outside quotes
.param p=1k

+ p=4k
.subckt cell a b w=1k
rc a b w
.ends
v1 1 0 1
r2 1 0 1k

+ r=5k
rp 1 0 p
x1 1 0 cell

+ w=3k
r3 1 0 1k
$ a dollar comment line
+ r=6k
.op
.end
