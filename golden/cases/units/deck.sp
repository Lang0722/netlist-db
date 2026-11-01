* units: HSPICE number scale factors (tiny values are scaled up through a .param)
v1 1 0 1
r_m 1 0 1m
r_mu 1 0 1M
r_meg 1 0 1meg
r_megu 1 0 1MEG
r_x 1 0 1x
r_mil 1 0 1mil
r_k 1 0 2.5k
r_v 1 0 1V
r_t 1 0 1t
r_g 1 0 1g
r_ek 1 0 1e-3k
r_me 1 0 1me
r_mi 1 0 1mi
.param p_pf=10pF p_a=1a p_u=1u p_n=1n p_p=1p p_f=1f p_mu=1M
r_pf 1 0 'p_pf*1e12'
r_a 1 0 'p_a*1e18'
r_u 1 0 'p_u*1e6'
r_n 1 0 'p_n*1e9'
r_p 1 0 'p_p*1e12'
r_f 1 0 'p_f*1e15'
r_pm 1 0 'p_mu*1000'
.op
.end
