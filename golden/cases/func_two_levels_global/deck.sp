* func_two_levels_global: functions in two instance levels, a header default text, a function after its use (default parhier)
* (1) a function in two instance levels above the call, none at the top level
.subckt fo a b
.param fn(x)='x*5'
xm a b fm
.ends
.subckt fm a b
.param fn(x)='x*7'
xl a b fl
r6 a b 'fn(1)*1k'
.ends
.subckt fl a b
r5 a b 'fn(1)*1k'
.ends
x5 5 0 fo
* (2) a header default text that names a top-level .param
.param w=4
.subckt ce a b w=2 l='w*3'
r4 a b l
.ends
x4 4 0 ce
* (3) a function defined after its use
r7 7 0 'g(1)*1k'
.param g(x)='x*5'
v4 4 0 1
v5 5 0 1
v7 7 0 1
.op
.end
