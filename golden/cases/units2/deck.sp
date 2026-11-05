* units2: more number suffixes (time units, db, z, letters after m)
v1 1 0 1
r_min 1 0 1min
r_hr 1 0 1hr
r_day 1 0 1day
r_yr 1 0 1yr
r_db 1 0 20db
r_db6 1 0 6db
r_meter 1 0 1meter
r_ms 1 0 1ms
r_mv 1 0 1mv
r_mohm 1 0 1mohm
* 1z directly (1e-21 ohm) would be dropped from the .op listing
* ("Recovered resistors ... to avoid V loop"), so it goes through a .param.
.param p_z=1z
r_z 1 0 'p_z*1e21'
.op
.end
