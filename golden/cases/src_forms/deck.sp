* src_forms: forms of the V/I source line (DC values from the .op node voltages)
.option list
V1 1 0 DC 1.2
V2 2 0 DC 1 PULSE(0 1 0 1n 1n 5n 10n)
V3 3 0 1.2 AC 1 0
V4 4 0 PULSE 0 1 0 1n 1n 5n 10n
V5 5 0 DC=2
I1 0 6 DC 1m
V7 7 0 PULSE 0.3 1 0 1n 1n 5n 10n
r1 1 0 1k
r2 2 0 1k
r3 3 0 1k
r4 4 0 1k
r5 5 0 1k
r6 6 0 1k
r7 7 0 1k
.op
.end
