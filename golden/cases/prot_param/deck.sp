* probe_prot_param: a .param and a .model inside a .prot region, used outside it
.prot
.param rval=1k
.model rmod r
.unprot
.param a='rval*2'
v1 1 0 1
r1 1 0 rval
r2 1 0 a
r3 1 0 rmod
.op
.end
