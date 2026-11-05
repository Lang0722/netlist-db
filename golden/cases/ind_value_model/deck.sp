* ind_value_model: inductor value, then model name (L1 1 2 1n lmod)
.option list
.model lmod l level=3
v1 1 0 1
L1 1 2 1n lmod
r1 2 0 1k
.op
.end
