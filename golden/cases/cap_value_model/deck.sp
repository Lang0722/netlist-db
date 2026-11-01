* cap_value_model: capacitor value, then model name (C1 1 2 1p cmod)
.option list
.model cmod c
v1 1 0 1
C1 1 2 1p cmod
r1 2 0 1k
.op
.end
