* ovr_body_global: an X-line override, a body .param and a header default of one name in one instance (default parhier)
* (1) an X-line override and a body .param of the new instance
.subckt cb a b
.param w=2
r1 a b w
.ends
x1 1 0 cb w=7
* (2) a header default and a body .param of one name
.subckt cd a b w=1
.param w=5
r2 a b w
.ends
x2 2 0 cd
* (3) all three: an X-line override, a body .param and a header default
x3 3 0 cd w=8
* (4) override text names k: the parent's body defines k=6, the new instance's header k=1
.subckt cq a b k=1 w=1
r4 a b w
.ends
.subckt mq a b
.param k=6
xi a b cq w='k'
.ends
x4 4 0 mq
* (5) a body .param that names a header default overridden on the X line; a child uses the body .param
.subckt ce a b u=1
.param z='u*3'
r5 a b z
xc a b ce2
.ends
.subckt ce2 a b
r6 a b z
.ends
x5 5 0 ce u=4
* (6) one level down: the parent's override and the child's body .param of one name
.subckt cin a b
.param t=2
r7 a b t
.ends
.subckt cmid a b
xi a b cin t=9
.ends
x6 6 0 cmid
v1 1 0 1
v2 2 0 1
v3 3 0 1
v4 4 0 1
v5 5 0 1
v6 6 0 1
.op
.end
