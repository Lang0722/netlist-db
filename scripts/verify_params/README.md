# Checking parameters and model choice

`run.py FIRST LAST` writes random decks (`pgen.py`) that pass parameters
through subckt hierarchies into MOSFETs, and checks every query mode of
`model_select` (the listing, `--summary`, `--subckt NAME`) against
`pref.py`, a reference written separately from netlist-db that flattens a
deck instance by instance. With [ngspice](https://ngspice.sourceforge.io/)
installed, it also checks the reference against ngspice's own parameter
expansion and model binning (`ngs.py`).

``` shell
cargo build --release --example model_select
python3 scripts/verify_params/run.py 1 100
```

The decks: subckts in 3 to 5 levels; the same names defined at several
levels (top-level `.param`, `.subckt` defaults, `.param` inside subckts,
values passed on instance lines, which may read the new instance's parameters
and lead back to themselves); L, W and NF as expressions; binned models whose
limits are numbers, read a top-level parameter (that subckts redefine), or read
the parameters of the subckt that defines them; a model family and a subckt
that some subckts define again (seen along the instance path); nested subckt
definitions; `.param f(x)=...` functions; `WNFLAG` in models, on instance lines
and as `.option wnflag`; `.option parhier=local` and `global`.

`pref.py` follows the rules measured on HSPICE (`../hspice_kit`). It shares
them with netlist-db, so that they agree says the rules are implemented as
written; only `../hspice_kit` says the rules are HSPICE's. (Its first rules
were not, and thousands of decks agreed on them.)

netlist-db must agree with the reference on every MOSFET (the exit status);
the comparison with ngspice is shown for information. What ngspice does
differently from HSPICE, which the ngspice decks avoid or the comparison
takes into account:
- a value passed whose text leads back to it through other names: HSPICE
  computes it at the level above the new instance, ngspice does not (the
  ngspice decks pass no expression of a name that the subckt defines);
- it compares the total width unless `.option wnflag=1` (the ngspice decks set
  it; HSPICE compares `W/NF` by default);
- at a bin edge, a value within 1e-9 (absolute: 1 nm) of either limit
  matches, both bins then holding it; which one ngspice takes depends on its
  order of models (`pref.py --ngspice-rule` takes the last defined, which is
  not always ngspice's choice);
- `show` cuts device names at 21 characters (`ngs.py` recovers them from
  `listing e`; a cut name that several MOSFETs share is not compared), and
  ngspice mis-expands a MOSFET of a nested subckt that uses the enclosing
  subckt's local model, when parameters follow the model name (it stops);
- `pow(x, y)` uses all of `y` (HSPICE: its integer part);
- it does not implement `.option parhier=global`.

`torture.py` runs decks designed by hand whose answers are known in
advance: computed by a formula from each instance's path (or as closed-form
counts), or by the scoping rule written out directly. A chain of 24 levels
each rescaling what it passes down; a binary tree whose leaves' parameter is
their path read in binary (every context different); a ternary lattice whose
counts are binomial sums (581 million MOSFETs with `--big`); every
combination of where a name is defined or passed over 3 levels, under both
`parhier` rules; overlapping and gapped bins, a model and bins of one name,
dotted and non-numeric names (`n.ch.1`, `nm.mc`), fingers, edges reached
through float noise, `.option scale`, local bins that do not hold; errors in
some contexts only; library mode, nested subckts hiding top-level ones.

``` shell
python3 scripts/verify_params/torture.py --big --reference --ngspice
```

`../parhier_probe.sp` and `../hspice_kit` ask HSPICE itself.
