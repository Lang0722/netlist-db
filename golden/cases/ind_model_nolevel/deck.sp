* ind_model_nolevel: an L element that calls a .model of type l without level=3
.option list
.model lmod l
v1 1 0 1
L1 1 2 1n lmod
r1 2 0 1k
.op
.end
