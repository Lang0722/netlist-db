* p8_src_scale: source .op values with a delay, and the R element SCALE
.option list
v1 1 0 SIN(0.2 1 1meg 1n 0 90)
v2 2 0 AM(1 0.5 1k 1meg 1n)
v3 3 0 EXP(0.5 1 2n 1n 5n 5n)
v4 4 0 PULSE(0.3 1 2n 1n 1n 5n 10n)
v5 5 0 SIN(0.2 1 1meg 0 1e6 30)
rl1 1 0 1k
rl2 2 0 1k
rl3 3 0 1k
rl4 4 0 1k
rl5 5 0 1k
r6 6 0 1k scale=2
v6 6 0 1
.op
.end
