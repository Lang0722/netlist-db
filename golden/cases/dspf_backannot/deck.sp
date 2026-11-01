* dspf_backannot: pre-layout inverter back-annotated with .option sim_dspf (scope = subckt inv)
.option sim_dspf="inv inv.dspf"
.option sim_dspf_rail=on
.model nch nmos level=1
.model pch pmos level=1
.subckt inv A Y VDD VSS
MN1 Y A VSS VSS nch W=1u L=0.1u
MP1 Y A VDD VDD pch W=2u L=0.1u
.ends
xi in out vdd 0 inv
vdd vdd 0 1
vin vi 0 0.5
rin vi in 1k
rload out 0 1meg
.op
.end
