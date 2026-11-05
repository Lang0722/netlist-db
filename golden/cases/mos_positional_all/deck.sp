* mos_positional_all: 8 bare numbers after the model name (l w ad as pd ps nrd nrs?)
* nja: js=1 jsw=0 -> cdsat = AD, cssat = AS; njp: js=0 jsw=1 -> cdsat = PD, cssat = PS
* rsh=1 -> rd eff = NRD, rs eff = NRS
.option list
.model nja nmos level=1 kp=1e-4 vto=0.5 js=1 jsw=0 rsh=1
.model njp nmos level=1 kp=1e-4 vto=0.5 js=0 jsw=1 rsh=1
vd d 0 1
vg g 0 1
vs s 0 0
vb b 0 0
M1 d g s b nja 2u 1u 3p 4p 5u 6u 7 8
M2 d g s b nja l=2u w=1u ad=3p as=4p pd=5u ps=6u nrd=7 nrs=8
M3 d g s b njp 2u 1u 3p 4p 5u 6u 7 8
M4 d g s b njp l=2u w=1u ad=3p as=4p pd=5u ps=6u nrd=7 nrs=8
.op
.end
