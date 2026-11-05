* probe_rcl_vnode: node voltages and a branch current in quoted R and C values (no R= / C=)
v1 out 0 1
r1 out 0 'v(out)*1k+1k'
r2 out 0 'i(v1)*1k+1k'
c1 out 0 '1p*v(out)+1p'
.op
.end
