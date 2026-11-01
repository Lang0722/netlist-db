* ind_name_model: a parameter name in the L value place, then a model name
.option list
.temp 125
.param lval=2n
.model lmod l level=3
v1 1 0 1
l2 1 12 lval lmod
l3 1 13 lval lmod 0.004
l6 1 16 1n lmod
r12 12 0 1k
r13 13 0 1k
r16 16 0 1k
.op
.end
