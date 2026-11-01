* quoted_spaces_param: a line with only spaces and a tab inside a quoted .param value that goes on on a + line
.param a=2k
.param q='a
   	
+ *2'
v1 1 0 1
r1 1 0 q
rc 1 0 1k
.op
.end
