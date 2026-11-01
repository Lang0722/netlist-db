* global_net: .global vdd used inside a subckt without a port (vss is not global)
.global vdd
vsup vdd 0 2
v2 vss 0 1
rvss vss 0 1k
.subckt load a
rl vdd a 1k
rg a 0 1k
rv vss a 1k
.ends
x1 n1 load
.op
.end
