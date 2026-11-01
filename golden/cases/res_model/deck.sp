* res_model: resistor with a model name (no scaling parameters) + positional value or keyword value
.model rmod r
v1 1 0 1
v2 2 0 1
R1 1 0 rmod 2k
R2 2 0 rmod r=3k
.op
.end
