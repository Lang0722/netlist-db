* mos_digit_model_ref: an M line names 5vnmos, and no .model card has that name
.option list
.model nch nmos level=1 kp=1e-4 vto=0.5
vd d 0 1
vg g 0 1
vs s 0 0
vb b 0 0
M1 d g s b 5vnmos w=1u l=1u
.op
.end
