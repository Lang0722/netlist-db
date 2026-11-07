"""Write the decks of the speed and memory gate.

    python3 gen_big.py OUT.sp           1,000 cells x 102 devices; each cell
                                        called 10 times at the top level:
                                        10,000 instances, 1.02M devices.
    python3 gen_big.py OUT.sp --levels N
                                        the same cells, with N more levels of
                                        groups: the top calls a group 10
                                        times, each group calls the group
                                        below it (at the bottom, the cell) 10
                                        times. N=2: 1,110,000 instances, 100x
                                        the cell instances, about 20% more
                                        input lines.
    python3 gen_big.py OUT.sp --levels N --unused
                                        the same lines, but the top calls the
                                        cells, so the groups are unused: the
                                        control deck of the memory gate.
"""

import sys

CELLS = 1000
PER_CELL = 34  # x (M, R, C) = 102 devices
CALLS = 10


def main() -> None:
    out = sys.argv[1]
    levels = int(sys.argv[3]) if sys.argv[2:3] == ["--levels"] else 0
    unused = "--unused" in sys.argv[2:]
    with open(out, "w") as f:
        w = f.write
        w("* big deck\n.param l=0.1u wn=1u\n.model nch nmos level=1\n.model pch pmos level=1\n")
        for k in range(CELLS):
            w(f".subckt cell{k} a y vdd vss wn=1u\n")
            for j in range(PER_CELL):
                w(f"M{j} n{j} a vss vss nch W=wn L=l\n")
                w(f"R{j} n{j} y {j + 1}k\n")
                w(f"C{j} n{j} vss {j + 1}p\n")
            w(f".ends cell{k}\n")
        # group level g calls level g-1; level 0 is the cell
        for g in range(1, levels + 1):
            below = f"g{g - 1}_" if g > 1 else "cell"
            for k in range(CELLS):
                w(f".subckt g{g}_{k} a y vdd vss wn=1u\n")
                for u in range(CALLS):
                    w(f"Xc{u} a y{u} vdd vss {below}{k} wn=wn\n")
                w(f".ends g{g}_{k}\n")
        callee = f"g{levels}_" if levels and not unused else "cell"
        for k in range(CELLS):
            for t in range(CALLS):
                w(f"X{k}_{t} a{k} y{t} vdd 0 {callee}{k} wn=2u\n")
        w(".op\n.end\n")


if __name__ == "__main__":
    main()
