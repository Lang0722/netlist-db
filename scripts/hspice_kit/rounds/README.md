# The comparison rounds with HSPICE

These scripts wrote and compared the decks of the section "Comparison rounds with
HSPICE as the reference" in `golden/NOTES.md`. They are work tools, not a test suite.

## Setup

1. Set `NDB_ROUNDS` to a work folder with free disk.
2. Link this folder there: `ln -s "$PWD" "$NDB_ROUNDS/hard"`.
3. Copy `vmrun.ps1`, `vmback.ps1` and `vmone.ps1` to `tmp/xfer/` of the clone. The
   folder `tmp/` is ignored by git.
4. Check the path `\\Mac\Home\Project\netlist-db-hspice-test` in `vm.sh` and in the
   `.ps1` files. It is how the VM sees the clone.
5. Build the binary: `CARGO_TARGET_DIR=$NDB_ROUNDS/target cargo build --release
   --example model_select`.
6. For `exprs.py`, also copy `zz_eval.rs` to `examples/` and build that example.

`vm.sh` reaches the VM through `prlctl` only. It removes the licence lines (`lic:`)
from each listing that it brings back. Do not store those lines.

## One round

```
python3 $NDB_ROUNDS/hard/forms45.py $NDB_ROUNDS/f45
$NDB_ROUNDS/hard/vm.sh push f45 $NDB_ROUNDS/f45
$NDB_ROUNDS/hard/vm.sh run f45 4 150
$NDB_ROUNDS/hard/vm.sh back f45 $NDB_ROUNDS/f45
python3 $NDB_ROUNDS/hard/rows.py $NDB_ROUNDS/f45
```

## The files

- `forms*.py`: the rounds of line forms. Each writes `OUTDIR/decks`.
- `pdk.py`: seeded decks in the style of a PDK library.
- `fuzz.py`, `exprs.py`, `e*.txt`: random and measured expressions, read from `.ms0`.
- `cmp_list.py`: compares the HSPICE table with `model_select`, row by row.
- `rows.py`, `show.py`, `cmp_read.py`: short views of the results.
- `dd.py`, `reduce*.py`: reduce a deck through HSPICE.
- `all.sh`: runs `cmp_list.py` on all rounds.
- `mk.py`: prepares a deck of `gen_big_netlist.py` for HSPICE.
- `bust.py`: a deck where each leaf MOSFET has its own L, so no cache can merge leaves.
- `mods.py`: large decks of one shape each. `flat.py`: flat decks.
- `per_sub.py`: per-subckt counts of a deck of `gen_big_netlist.py`.
- `vm.sh`, `vmrun.ps1`, `vmback.ps1`, `vmone.ps1`: run decks on the VM.
