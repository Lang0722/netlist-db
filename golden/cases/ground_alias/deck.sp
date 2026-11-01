* ground_alias: which node names HSPICE treats as ground (see branch currents)
v1 1 0 1
r0 1 0 1
r1 1 gnd 2
r2 1 gnd! 4
r3 1 ground 8
r4 1 GND 16
.subckt sg a
rs a gnd 32
.ends
xg 1 sg
.op
.end
