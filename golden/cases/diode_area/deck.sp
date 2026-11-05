* diode_area: bare numbers after the diode model name (area, then pj?)
.option list
.model dmod d is=1e-14 jsw=1e-8
v1 1 0 0.6
v2 2 0 0.6
v3 3 0 0.6
v4 4 0 0.6
v5 5 0 0.6
D1 1 0 dmod 2
D2 2 0 dmod
D3 3 0 dmod 2 1e-6
D4 4 0 dmod area=2
D5 5 0 dmod area=2 pj=1e-6
.op
.end
