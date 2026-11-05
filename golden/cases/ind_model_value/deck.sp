* ind_model_value: inductor model name, then value (L1 1 2 lmod 1n)
.option list
.model lmod l level=3
v1 1 0 1
L1 1 2 lmod 1n
r1 2 0 1k
.op
.end
