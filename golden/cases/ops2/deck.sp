* ops2: unary minus after a power operator, and zero to a power
v1 1 0 1
r1 1 0 '2**-3**2'
r2 1 0 '2^-3^2'
* zero base: "10+" keeps the resistor away from 0 ohm
r3 1 0 '10+0**0'
r4 1 0 '10+0^0'
r5 1 0 '10+0**-1'
r6 1 0 '10+0**0.5'
.op
.end
