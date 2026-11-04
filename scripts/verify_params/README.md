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
values passed on instance lines, which may read the instance's parameters);
L, W and NF as expressions; binned models whose limits are numbers, read a
top-level parameter (that subckts redefine), or read the parameters of the
subckt that defines them; nested subckt definitions; `.param f(x)=...`
functions; `WNFLAG` in models, on instance lines and as `.option wnflag`;
`.option parhier=local` and `global`.

netlist-db must agree with the reference on every MOSFET (the exit status);
the comparison with ngspice is shown for information. What ngspice does
differently from HSPICE, which the ngspice decks avoid or the comparison
takes into account:
- ngspice computes a value passed on an instance line with the parameters
  of the subckt it instantiates hiding those of the instance's level (HSPICE
  computes it where the instance line is);
- at a bin edge, a value within 1e-9 (absolute: 1 nm) of either limit
  matches, both bins then holding it; which one ngspice takes depends on its
  order of models (`pref.py --ngspice-rule` takes the last defined, which is
  not always ngspice's choice);
- `show` cuts device names at 21 characters (`ngs.py` recovers them from
  `listing e`; a cut name that several MOSFETs share is not compared), and
  ngspice mis-expands a MOSFET of a nested subckt that uses the enclosing
  subckt's local model, when parameters follow the model name (it stops);
- it ignores the model parameter `WNFLAG`, and `pow(x, y)` uses all of `y`
  (HSPICE: its integer part);
- it does not implement `.option parhier=global`.

`../parhier_probe.sp` asks HSPICE itself the questions that ngspice cannot
answer (`../parhier_probe.py` compares its results with netlist-db's).
