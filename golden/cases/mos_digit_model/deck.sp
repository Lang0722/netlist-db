* mos_digit_model: a MOSFET model name that starts with digits (5vnmos)
.option list
.model 5vnmos nmos level=1 kp=1e-4 vto=0.5
vd d 0 1
vg g 0 1
vs s 0 0
vb b 0 0
M1 d g s b 5vnmos w=1u l=1u
.op
.end
