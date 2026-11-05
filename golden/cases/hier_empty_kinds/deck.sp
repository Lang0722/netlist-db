* hier_empty_kinds: instances of empty subckts, and devices of several types inside subckts
.model dm d is=1e-14
.subckt empty a b
.ends
.subckt onlyprm a b
.param k=1
.ends
.subckt onlyx a b
xe a b empty
.ends
.subckt kinds a b
rk a b 1k
lk a n1 1n
rl n1 b 1k
lk2 n7 0 1n
rn7 n7 0 1k
kk lk lk2 0.5
ck a b 1p
dk a n6 dm
rd n6 b 1k
ek n2 0 a b 1
re n2 0 1k
gk 0 n3 a b 1m
rg n3 0 1k
vk n4 0 0.5
rv n4 0 1k
ik 0 n5 1m
ri n5 0 1k
fk 0 n8 vk 2
rf n8 0 1k
hk n9 0 vk 100
rh n9 0 1k
.ends
v1 1 0 1
x1 1 t1 empty
r1 t1 0 1k
x2 1 t2 onlyprm
r2 t2 0 1k
x3 1 t3 onlyx
r3 t3 0 1k
x4 1 t4 kinds
r4 t4 0 1k
x5 1 t5 kinds m=2
r5 t5 0 1k
.op
.end
