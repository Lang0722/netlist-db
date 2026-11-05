# netlist-db: HSPICE comparison kit

This kit checks netlist-db's model selection against HSPICE: the same
decks, run by HSPICE on a machine that has it, compared afterwards with
netlist-db's answers (made when the kit was, in `expected/`).

The VM needs only HSPICE and bash. Python and netlist-db are needed only
where the results are compared (the netlist-db session).

## Where things stand

Branch `main` of the fork (`VERSION` gives the commit the answers come from).

What netlist-db does now:
- parses HSPICE decks robustly (checked on 3,315 public decks), with `.lib`
  corner sections loaded fast (one read per library file);
- chooses each MOSFET's model the way HSPICE's model selector does, by the
  rules that this kit measured (`src/eval.rs` and `src/models.rs` state them);
- query modes (`examples/model_select`): one line per MOSFET (streamed, any
  size), `--summary` (how many MOSFETs use each model, without flattening:
  a 30 MB deck holding 300 million MOSFETs takes less than 1 second),
  `--subckt NAME` (one subckt with its defaults), library mode.

What is verified (scripts/verify_params, scripts/gen_big_netlist.py):
- HSPICE P-2019.06-SP1-1 on this kit (`make_kit.py OUTDIR --random 30`):
  L and W equal on all 8,672 MOSFETs (5,297 of them under `parhier=local`), and
  the bin on all but the 5 of the first known gap below; every deck HSPICE stops
  on, netlist-db reports an error for, but for `q_limits_same_scope`;
- a 200,000-line, 30 MB deck with 300,000,000 MOSFETs: every count equals
  the generator's own (also with parhier=global, and per subckt);
- 14 million MOSFETs of the same kind, one by one: equal to the generator's
  counts;
- random decks (parameters passed through 3-5 levels, nested subckts,
  functions, WNFLAG, both parhier): all equal to `pref.py`, in every query
  mode. `pref.py` is written from the same rules, so this checks the code, not
  the rules: only this kit does that;
- hand-designed decks with answers known by formula (24-level chains,
  2^20-leaf trees, a 581-million-MOSFET lattice, every combination of where
  a parameter is defined or passed): all equal;
- ngspice (as an outside reference, where it reads decks as HSPICE does):
  L, W and NF equal on every MOSFET compared; bins equal except within 1 nm
  of a bin edge (ngspice's own tolerance).

What HSPICE answered (each is a deck of this kit or a probe case):
1. `parhier=global`: the level closest to the top wins, also over a value
   passed below it (cases 1, 3). `parhier=local`: the closest to the reader.
2. A value passed is computed in the new instance, not where its line is
   (case 4, `q_override_*`). A text that reads its own name reads that name
   above (`q_cycle_*_self*`). A value passed whose text leads back to it through
   other names is computed, as a whole, above (`q_cycle_pass_*`, `q_cycle_text_*`);
   the values passed are computed in the order of the line (`q_cycle_swap*`).
3. The names and calls in the body of a `.param` function are resolved where
   the function is defined (`q_func_*`).
4. Bins compare `W/NF` by default (case 8, `q_nf_default`). `WNFLAG=0` on the
   instance, else `.option wnflag=0`, turns that off; a `WNFLAG` in a model is
   ignored (`q_wnflag*`).
5. The edge tolerance is relative, about 2e-15 (`q_edge_tol_*`).
6. Models and subckts are looked up along the instance path (`q_model_dynamic`,
   `q_subckt_dynamic`), level by level: a bin that does not hold L and W hides
   nothing (`q_localbin_out_globalexact`, `q_localhide`). Any `name.suffix` is a
   bin of `name`, but `a.b.1` is a bin of nothing, and a name with a dot is not
   named on an instance line (`q_nonnumeric`, `q_two_dots_*`, `q_bin_direct*`).
   A bin looked at must set its four limits (`q_incomplete_*`, `q_mc_*`).
7. L and W may be the first two values after the model name: numbers, names of
   parameters or quoted expressions (`q_positional*`).
8. `**` and `^` associate to the left, `0**y` is 0, a negative base takes the
   integer part of `y`, and `sqrt` and `log` of a negative number give the
   negative of the result for its absolute value (`q_expr*`). Scale factors go by
   the first letters (`1meter` is mega, `1minute` is 60); `z` is 1e-21, `y` is
   1e-24; an exponent makes the factor void (`1e-3k` is 0.001) (`q_units*`).
9. The last `.option parhier` of the deck decides, wherever it is and in any form
   (`.options`, capital letters, spaces around `=`, among other options, on a `+`
   line, quoted), but not one inside a subckt (`q_parhier_*`). One in an `.inc`
   file or a `.lib` section counts too (run by hand, once).
10. A MOSFET line has at most 7 nodes (`q_nodes_*`). The model name is the first word
    after the fourth node that names a model, else the fourth word if it names one
    (`q_two_model_words*`, `q_node_model_name`). After four nodes, a word that
    starts with a digit stops HSPICE (`q_digit_word_*`). Any number of values can
    follow the model name (`q_positional_numbers_9`, `q_positional_names_*`). `OFF`
    takes a place and gives no value (`q_off_*`).
11. Quotes in a group stop HSPICE (`q_group_quotes`, `q_quote_in_group`), but not in
    `agauss`, `gauss`, `aunif`, `unif`, `limit` and `str`, when the call is the whole
    value (`q_stat_quotes*`, `q_str_quotes`). A line inside quotes must be a `+` line
    (`q_quoted_*`). `.temp` takes no group (`q_temp_*`). A value in parentheses does
    not go on on a `+` line of a MOSFET line (`q_mos_group_lines`).

`golden/NOTES.md` ("Probes for the merge of the two parser branches") lists more
forms, also those on which netlist-db is known to differ.

Known gaps, reported by `compare_hspice.py` but not failing it:
- HSPICE evaluates the limits of each bin in the instance of the first MOSFET
  that looks at the bin (breadth first), and at the level of the model for all
  the others (`q_limits_s1_s2`, `s2_s1`, `s1_s3`, `bfs`, `nested_local`,
  `same_scope`, case 7 under local). netlist-db always does the latter: its
  choice for a transistor does not depend on the order of the walk. This
  matters only if a level below the model binds a name those limits read, with
  another value: with `parhier=local`, or a name no level above binds. So it
  reaches at most one MOSFET per bin, and none for a bin whose limits are numbers
  (`grep -iE '(lmin|lmax|wmin|wmax) *= *[^0-9.+ -]' FILES` lists the others). On
  such a deck HSPICE can also stop.
- A value in parentheses over a `+` line of a MOSFET line (`q_mos_group_lines`):
  HSPICE reads a part of it. netlist-db reports an error, so it gives no wrong
  value.

Known gaps not in this kit:
- a definition instantiated inside itself: HSPICE stops. netlist-db skips that
  line and reports nothing, so the MOSFETs below it are not counted;
- `.param` functions with the same name at several levels: HSPICE keeps, in
  some cases, mainly under `parhier=local`, the function that an expression found
  in the first instance that evaluated it;
- `m` on an X line: HSPICE computes it at the level of the line, netlist-db in
  the new instance. This can change a result only if an expression reads `m`;
- `20db`: HSPICE gives 9.99999907, netlist-db 10.
- `OFF` after a `key=value` on a MOSFET line (`M1 d g 0 0 nch l=2u w=1u OFF`):
  HSPICE runs it. netlist-db reports an error and does not count that MOSFET;
- `M1 d g 0 0 nch l=max(1u,2u) w=1u`: HSPICE stops, netlist-db reads L=2u.

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
  chosen).
- Some `q_*` decks make HSPICE stop (`q_gap`, `q_dotted`, `q_cycle_plain_*`,
  ...): netlist-db reports an error for their MOSFET, and that is agreement. If
  HSPICE runs one of them, it chose a model netlist-db does not: the comparison
  shows which.
- For other `q_*` decks (`q_stray_ends`, `q_two_functions`, ...), netlist-db
  reports an error in the text of the deck (`expected/NAME.text_error`). HSPICE
  should stop on them. If HSPICE runs one, the comparison says so.
- The other decks (`chain_*`, `matrix_*`, `lattice4_*`, `tree6`, `bins`,
  `scale`, `library`, `r_*`) should run cleanly. Each MOSFET's L, W and bin
  are read from `.measure` of LV1, LV2, LV9 (and the `.op` table; LV9 is not
  used below an L of 10 nm, where the threshold is no more VTH0 alone).
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
difference of L, W or bin. Any difference that is not a known gap is a bug of
netlist-db, or a new rule to measure.

## Contents

| | |
|---|---|
| `decks/` | the decks for HSPICE (`MANIFEST.tsv`: what each checks) |
| `expected/` | netlist-db's answers: per MOSFET (index, path, L, W, NF, model), per probe case, and `NAME.text_error` if netlist-db reports an error in the text of a deck |
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
