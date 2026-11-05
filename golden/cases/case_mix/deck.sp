* case_mix: mixed-case keywords, names and parameters
.OPTION PARHIER=GLOBAL
.PARAM Pq=2
.SUBCKT Foo A B Wv=1
R1 A B 'WV*pQ'
.Ends Foo
.Subckt BAR a b
Xf A b fOO wV=3
.ENDS
V1 1 0 1
X1 1 0 FOO WV=5
xB 1 0 bar
Rtop 1 0 PQ
.OP
.END
