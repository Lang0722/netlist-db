* unquoted_space: an unquoted .param expression with spaces
.param a=2k
.param b = a * 2
.param b2=a * 2 c2=5k
.param c=1k
v1 1 0 1
rb 1 0 b
rb2 1 0 b2
rc 1 0 c
rc2 1 0 c2
.op
.end
