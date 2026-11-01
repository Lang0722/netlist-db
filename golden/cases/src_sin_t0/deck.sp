* src_sin_t0: SIN with a phase or a delay and no DC value (which value does .op use?)
.option list
V1 1 0 SIN(0.2 1 1meg 0 0 90)
V2 2 0 SIN(0.2 1 1meg 1n)
r1 1 0 1k
r2 2 0 1k
.op
.end
