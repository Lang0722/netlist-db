* prot_subckt: an X line calls a subckt that is defined in a .prot region
.prot
.subckt secret a b
r1 a b 1k
.ends
.unprot
v1 1 0 1
x1 1 0 secret
r2 1 0 2k
.op
.end
