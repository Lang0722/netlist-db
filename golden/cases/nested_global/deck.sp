* nested_global: nested overrides and defaults under .option parhier=global
.option parhier=global
* example 6: override on a top-level X line vs override on an inner X line
.subckt in6 a b p=1
r6 a b p
.ends
.subckt out6 a b p=2
xi a b in6 p=5
.ends
x6 1 0 out6 p=3
* example 7: subckt default at a higher level vs an inner X-line override
.subckt in7 a b q=1
r7 a b q
.ends
.subckt out7 a b q=2
xi a b in7 q=5
.ends
x7 1 0 out7
* example 8: local .param at a higher level vs an inner X-line override
.subckt in8 a b s=1
r8 a b s
.ends
.subckt out8 a b
.param s=6
xi a b in8 s=7
.ends
x8 1 0 out8
* example 9: three levels, override on the top X line only
.subckt in9 a b t=1
r9 a b t
.ends
.subckt mid9 a b t=2
xi a b in9 t=8
.ends
.subckt out9 a b t=3
xm a b mid9
.ends
x9 1 0 out9 t=4
* example 10: inner override text uses a name that is also a top-level .param
.param w=9
.subckt in10 a b u=1
r10 a b u
.ends
.subckt out10 a b w=2
xi a b in10 u='w*5'
.ends
x10 1 0 out10 w=3
v1 1 0 1
.op
.end
