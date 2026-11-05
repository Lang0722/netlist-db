* unquoted_space_val: an unquoted .param expression with spaces (no use of c2)
.param a=2k
.param b = a * 2
.param b2=a * 2 c2=5k
.param c=1k
v1 1 0 1
rb 1 0 b
rb2 1 0 b2
rc 1 0 c
.op
.end
