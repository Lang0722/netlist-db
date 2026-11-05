* src_fn_nodc: transient functions with no DC value (which value does .op use?)
.option list
V1 1 0 PWL(0 0.4 1n 1)
V2 2 0 PWL(1n 0.6 2n 1)
V3 3 0 SIN(0.2 1 1meg)
V4 4 0 EXP(0.5 1 0 1n 1n 5n)
V5 5 0 SFFM(0.1 1 1meg 2 1k)
V6 6 0 AM(1 0.5 1k 1meg 0)
r1 1 0 1k
r2 2 0 1k
r3 3 0 1k
r4 4 0 1k
r5 5 0 1k
r6 6 0 1k
.op
.end
