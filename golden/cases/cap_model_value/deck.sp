* cap_model_value: capacitor model name, then value (C1 1 2 cmod 1p)
.option list
.model cmod c
v1 1 0 1
C1 1 2 cmod 1p
r1 2 0 1k
.op
.end
