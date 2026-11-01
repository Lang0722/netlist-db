* long_names: HSPICE shortens element names and instance paths in the .op listing
.subckt xlongcellname a b
rverylongelementname_abc a b 7
rshort a b 9
.ends
.subckt wrap a b
xlonginstname_abcdef a b xlongcellname
.ends
.subckt c a b
rlongname_cell a b 1
.ends
v1 1 0 1
xtop_instance_name_very_long_123456789 1 0 wrap
rverylongtoplevel 1 0 11
v2 11 0 1
x1 11 12 c
x2 12 13 c
x3 13 14 c
x4 14 15 c
x5 15 16 c
x6 16 17 c
x7 17 18 c
x8 18 19 c
x9 19 20 c
x10 20 0 c
.op
.end
