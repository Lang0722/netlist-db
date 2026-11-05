# netlist-db: HSPICE comparison kit

This kit checks netlist-db's model selection against HSPICE: the same
decks, run by HSPICE on a machine that has it, compared afterwards with
netlist-db's answers (made when the kit was, in `expected/`).

The VM needs only HSPICE and bash. Python and netlist-db are needed only
where the results are compared (the netlist-db session).

## Where things stand

Branch `fix/stability-and-hspice-gaps` of the fork (`VERSION` gives the
commit the answers come from).

What netlist-db does now:
- parses HSPICE decks robustly (checked on 3,315 public decks), with `.lib`
  corner sections loaded fast (one read per library file);
- chooses each MOSFET's model the way HSPICE's model selector does: the
  model of that name, else the bin `name.N` whose LMIN <= L < LMAX and
  WMIN <= W < WMAX, with parameters resolved through the hierarchy
  (`.option parhier`), bin limits evaluated where the model is defined, W
  per finger with WNFLAG;
- query modes (`examples/model_select`): one line per MOSFET (streamed, any
  size), `--summary` (how many MOSFETs use each model, without flattening:
  a 30 MB deck holding 300 million MOSFETs takes about 1 second),
  `--subckt NAME` (one subckt with its defaults), library mode.

What is verified (scripts/verify_params, scripts/gen_big_netlist.py):
- a 200,000-line, 30 MB deck with 300,000,000 MOSFETs: every count equals
  the generator's own (also with parhier=global, and per subckt);
- 1.6 million MOSFETs of the same kind, one by one, against a separate
  reference implementation: all equal;
- 3,600 random decks (parameters passed through 3-5 levels, nested
  subckts, functions, WNFLAG, both parhier): all equal to the reference, in
  every query mode;
- hand-designed decks with answers known by formula (24-level chains,
  2^20-leaf trees, a 581-million-MOSFET lattice, every combination of where
  a parameter is defined or passed): all equal;
- ngspice (as an outside reference, where it reads decks as HSPICE does):
  L, W and NF equal on every MOSFET compared; bins equal except within 1 nm
  of a bin edge (ngspice's own tolerance).

What only HSPICE can tell (and what netlist-db assumes until then):
1. With `parhier=global`, a value passed on an instance line
   (`X1 a b sub p=2`) against a top-level `.param p=1`: netlist-db uses the
   passed value. (probe case 1, case 3)
2. Bins of a multi-finger MOSFET (`W=4u NF=8`) without WNFLAG: netlist-db
   compares the total width (W); ngspice's HSPICE mode assumes W/NF.
   (probe case 8, deck q_nf_default) This one matters for every
   multi-finger transistor.
3. A length within 1 nm below a bin limit: netlist-db takes the lower bin
   (limits exact, within 1e-9 relative). (probe case 10, deck q_edge)

## On the HSPICE machine

```
tar xzf hspice_kit.tar.gz
cd hspice_kit
./run_hspice.sh                      # or: HSPICE=/path/to/hspice ./run_hspice.sh
```

It runs HSPICE on each deck of `decks/` (`hspice -i decks/NAME.sp -o
results/NAME`), one by one: a deck HSPICE stops on does not stop the
others. Then it packs `results/` into **`hspice_results.tar.gz`: bring that
file back**. The decks are small (the largest has a few hundred MOSFETs);
the whole run should take a few minutes.

If your HSPICE needs another command line (`hspice deck.sp > deck.lis`,
another output directory), change the one `"$HSPICE" -i ...` line of
`run_hspice.sh`; what matters is that each deck's `.lis`, `.mt0`/`.ms0`
and `.pa0` end up in `results/` under the deck's name.

Also useful, if you can (optional):
- `bin/model_select` is netlist-db's, statically linked (any x86-64
  Linux): `bin/model_select --summary your_deck.sp` gives the model counts
  of your own netlist; `bin/model_select your_deck.sp > listing.tsv` one
  line per MOSFET. Bring back what you can share (the summary has no
  netlist content but model names and counts).
- `bin/gen_big_netlist.py OUTDIR` (Python 3) writes the 300-million-MOSFET
  test deck again, if you want to time HSPICE's parsing of a deck that size.

## What to expect

- `probe_global`, `probe_local`: the questions above, by `.measure`
  (values of node voltages, and drain currents that tell which bin was
  chosen). The most important results.
- Decks `q_gap`, `q_dotted`, `q_nonnumeric`, `q_localhide`: netlist-db finds
  no model for their MOSFET; HSPICE should stop with an error there (that is
  agreement). If HSPICE runs one of them, it chose a model netlist-db
  does not: the comparison shows which.
- `q_wnflag_instance`, `q_wnflag_option`: `wnflag` on an instance line and
  `.option wnflag` are ngspice's; HSPICE may reject or ignore them. Its
  message is the answer.
- `wnflag=1` in a model card (`probe_*`, `q_wnflag_model`, decks `r_local*`,
  `r_global*`): if HSPICE stops on it as an unknown parameter, delete
  `wnflag=1` from those decks' `.model` lines and run them again (the
  comparison will show the bins that change, as expected).
- The other decks (`chain_*`, `matrix_*`, `lattice4_*`, `tree6`, `bins`,
  `scale`, `library`, `r_*`) should run cleanly. Each MOSFET's L, W and bin
  are read from `.measure` of LV1, LV2, LV9 (and the `.op` table).
- An older HSPICE may not take `version=4.7` in the BSIM4 model cards (the
  probes and `r_*` decks): set it to a version it has (the version does not
  change the choice of bins). Any other message HSPICE stops on is a result
  too: keep its `.lis`.

## Back here

Put `hspice_results.tar.gz` in the netlist-db session (upload it, or
commit it to the branch), then:

```
tar xzf hspice_results.tar.gz
python3 scripts/hspice_kit/compare_hspice.py hspice_kit results
```

It prints the probe's table (netlist-db's answer and HSPICE's, per case
and parhier), then per deck whether HSPICE ran it, and per MOSFET any
difference of L, W or bin. A difference on the three questions above
changes netlist-db's rule to HSPICE's; any other is looked into.

## Contents

| | |
|---|---|
| `decks/` | the decks for HSPICE (`MANIFEST.tsv`: what each checks) |
| `expected/` | netlist-db's answers: per MOSFET (index, path, L, W, NF, model), per probe case |
| `run_hspice.sh` | runs HSPICE on all decks, packs the results |
| `compare_hspice.py` | compares the results with `expected/` (run where netlist-db is) |
| `bin/` | `model_select` (static), `gen_big_netlist.py` |
| `VERSION` | the netlist-db commit of the answers |

How a deck tells HSPICE's choices: every model of the kit's decks (but the
probes) has VTH0 = 0.2 + 0.02*N V (N the number of its bin `name.N`, 0
without one; negative for PMOS), and zero DIBL, short/narrow-channel,
pocket and lateral-doping threshold terms; the bodies are at the sources.
So each MOSFET's LV9 (threshold) gives its bin, LV1 and LV2 (effective
length and width, with LINT, WINT, XL, XW zero) its L and W (W/NF with
several fingers). The decks are made by `scripts/hspice_kit/make_kit.py`.
