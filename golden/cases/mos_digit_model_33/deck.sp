* mos_digit_model_33: a MOSFET model name that starts with digits (33nmos)
.option list
.model 33nmos nmos level=1 kp=1e-4 vto=0.5
vd d 0 1
vg g 0 1
vs s 0 0
vb b 0 0
M1 d g s b 33nmos w=1u l=1u
.op
.end
