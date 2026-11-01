* src_order_fn_first: a transient function before DC on a V line
.option list
V1 1 0 PULSE(0 1 0 1n 1n 5n 10n) DC 0.7
r1 1 0 1k
.op
.end
