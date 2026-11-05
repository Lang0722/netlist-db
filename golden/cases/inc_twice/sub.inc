* sub.inc: one .subckt and one .param; deck.sp includes this file twice
.subckt cell a b
rc a b 1k
.ends
.param pi=1k
