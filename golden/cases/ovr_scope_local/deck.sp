* p8_ovr_scope_local: override scope, same-level order, .param redefinition order (parhier=local)
.option parhier=local
.param w=4
* (1) override text names w, which the child also defines (default w=2)
.subckt cc a b w=2 l=1
r1 a b l
.ends
x1 1 0 cc l='w*3'
* (2) the same X line also overrides w
x2 2 0 cc w=5 l='w*3'
* (3) body .param vs header default of one name, no override
.subckt cd a b v=1
.param v=2
r3 a b v
.ends
x3 3 0 cd
* (4) a header default that uses another header name, which the X line overrides
.subckt ce a b w=2 l='w*3'
r4 a b l
.ends
x4 4 0 ce w=5
* (5) a top-level .param that uses a name defined again after it
.param a=1
.param b='a*2'
.param a=5
r5 5 0 b
* (6) the same inside a subckt body
.subckt cf a b
.param c=1
.param e='c*2'
.param c=5
r6 a b e
.ends
x6 6 0 cf
v1 1 0 1
v2 2 0 1
v3 3 0 1
v4 4 0 1
v5 5 0 1
v6 6 0 1
.op
.end
