* mos_suffix_letters: letters after the scale suffix of a positional L (2umos)
.option list
.model nch nmos level=1 kp=1e-4 vto=0.5
vd d 0 1
vd2 d2 0 1
vg g 0 1
vs s 0 0
vb b 0 0
M1 d g s b nch 2umos 1u
M2 d2 g s b nch l=2u w=1u
.op
.end
