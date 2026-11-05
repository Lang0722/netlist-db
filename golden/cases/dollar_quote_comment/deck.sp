* dollar_quote_comment: $ comments that hold quotes, and function names in other letter cases
.param rval=2k $ don't change
.param q='rval*2' $ it's doubled
.param z = 3k $ a 'quoted' word
.param F2(X)='x*2' $ f2's body
.param Max(a,b)='a+b'
v1 1 0 1
r1 1 0 rval $ the 'r' value
r2 1 0 q
r3 1 0 'rval*3' $ it's 'tripled'
r4 1 0 'rval
+ *4' $ quote's here
r5 1 0 rval $ it's
+ tc1=0
r6 1 0 z
r8 1 0 'f2(1)*1k'
r9 1 0 'F2(2)*1k' $ 'F2'
r10 1 0 'MAX(1,2)*1k'
r11 1 0 'SQRT(4)*1k'
.subckt cell a b w=1k $ width's default
rc a b 'w*2' $ it's
.ends
x1 1 0 cell w='rval' $ it's
x2 1 0 cell $ it's
+ w=3k
.subckt cs a b
.param SGN(Y)='y*7'
rs a b 'sgn(1)*1k'
rt a b 'Sgn(2)*1k'
.ends
x3 1 0 CS
.op
.end
