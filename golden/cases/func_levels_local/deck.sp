* func_levels_local: a top-level .param function and an instance function not on the parent level (parhier=local)
.option parhier=local
.param f(x)='x*2'
* (1) f at the top level and in level 1, the call in level 3
.subckt fo a b
.param f(x)='x*5'
xm a b fm
.ends
.subckt fm a b
xl a b fl
.ends
.subckt fl a b
r5 a b 'f(1)*1k'
.ends
x5 5 0 fo
* (2) f at the top level and in level 2, the call in level 4
.subckt ga a b
xb a b gb
.ends
.subckt gb a b
.param f(x)='x*7'
xc a b gc
.ends
.subckt gc a b
xd a b gd
.ends
.subckt gd a b
r7 a b 'f(1)*1k'
.ends
x7 7 0 ga
* (3) g in level 1 only, the call in level 3
.subckt ho a b
.param g(x)='x*3'
xm a b hm
.ends
.subckt hm a b
xl a b hl
.ends
.subckt hl a b
r3 a b 'g(1)*1k'
.ends
x3 3 0 ho
v3 3 0 1
v5 5 0 1
v7 7 0 1
.op
.end
