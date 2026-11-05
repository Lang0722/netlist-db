* bjt_area: a bare number after the BJT model name (area?)
.option list
.model qmod npn is=1e-16 bf=100
vb b 0 0.7
ve e 0 0
vc c 0 1
vc2 c2 0 1
vc3 c3 0 1
Q1 c b e qmod 2
Q2 c2 b e qmod
Q3 c3 b e qmod area=2
.op
.end
