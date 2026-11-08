# netlist-db: HSPICE kit of the card values

This kit checks `resolve_model` against HSPICE: where HSPICE evaluates the
parameters of a `.model` card. `resolve_model` evaluates each `key=value` of a
card where the card is defined: a top-level card with the top-level
parameters, a card inside a subckt (a PDK macro model, `nch_mac`) with the
parameters of that subckt's instance and of the corner. HSPICE was measured
to do so for the limits of a bin (`q_limits_*` of `../make_kit.py`), with one
known gap. For the other parameters of a card it was not: this kit asks.

The message for the agent that runs it on the HSPICE machine is
`HANDOFF.md`.

## Contents

| | |
|---|---|
| `kit/decks/` | 20 decks and `cards_pdk.lib` (the library of the corner decks) |
| `kit/expected.json` | each probe: the element, the card parameter, netlist-db's card, scope and value, and the value of each rule that HSPICE could follow |
| `kit/MANIFEST.tsv` | what each deck asks |
| `kit/run_cards.sh`, `kit/run_cards.ps1` | run HSPICE on all decks (bash, or PowerShell on Windows) |
| `kit/VERSION` | the netlist-db commit of the answers |
| `make_cards_kit.py` | writes `kit/` (needs the `resolve_model` example of the query crate) |
| `compare_cards.py` | compares HSPICE's results with `kit/expected.json` (Python 3 only) |

`query/tests/cards_kit.rs` checks that the answers in `kit/expected.json`
are still `resolve_model`'s. After a change of the query, make the kit again:

```
cargo build --release -p netlist-query --example resolve_model
python3 scripts/hspice_kit/cards/make_cards_kit.py scripts/hspice_kit/cards/kit
```

## How a deck tells HSPICE's value

- `vth0` of an NMOS card (level 54): each card zeroes the other threshold
  terms (as in `../make_kit.py`), and the bodies are at the sources, so the
  threshold `LV9` of a MOSFET is its card's VTH0. The control MOSFET `mctl`
  (a card with `vth0=0.3`) gives the offset of LV9 from VTH0, which
  `compare_cards.py` takes off.
- `rsh` of an R card: each R line has `l=10u w=1u`, and the current at 1 V
  gives RSH*L/W. The control `rctl` (`rsh=100`) gives the scale.
- `is` of a diode card: the current at 0.6 V is IS*(exp(V/Vt)-1). The
  control `dctl` (`is=1e-14`) gives the scale.

Each probe lists the value that each rule would give: netlist-db's rule,
"once for the subckt, in its first instance", "the MOSFET's own instance",
and others. The rules of a probe are at least 20 mV (or a factor of 1.5)
apart, so a difference tells which rule HSPICE follows.

## Run

On the HSPICE machine, from the kit (each deck runs from inside `decks/`,
because the corner decks load `cards_pdk.lib` by a relative path):

```
./run_cards.sh                                    # bash; HSPICE=/path/to/hspice ./run_cards.sh
powershell -NoProfile -ExecutionPolicy Bypass -File run_cards.ps1   # Windows; $env:HSPICE first
```

They write `results/` (`NAME.lis`, `NAME.ms0`, `NAME.stdout`,
`hspice_version.txt`) and pack it (`hspice_card_results.tar.gz` or `.zip`).
By hand, for one deck: `cd decks` and `hspice -i NAME.sp -o ../results/NAME`.

## Compare

```
python3 scripts/hspice_kit/cards/compare_cards.py scripts/hspice_kit/cards/kit RESULTS_DIR [--verbose]
```

For each probe it prints `agree`, `DIFFER` (with the rules that HSPICE's
value fits), or `probed` (a question that netlist-db does not answer: which
`vth0` HSPICE uses when a card sets it twice). For a MOSFET it also checks the
card (bin) that the `.op` table names. A probe whose netlist-db value is
missing (a name that no level of the card defines) agrees when HSPICE stops
on the deck. `--verbose` adds HSPICE's warnings and the `vth0`, `rsh` and `is`
values of its model listing. The exit status is 1 when a probe differs.

## The questions

| deck | asks |
|---|---|
| `c_mac_global`, `c_mac_local`, `c_mac_global_top` | a card inside a subckt: evaluated for each instance, with its parameters, under both `parhier` modes, and does a top-level `.param` win under `global`? |
| `c_top_card_local`, `c_top_card_notop` | a top-level card used only by MOSFETs in subckts that define the name too (the known gap of the bin limits: the first MOSFET's own instance?), or only there |
| `c_nested_local`, `c_nested_global`, `c_dynamic_local` | a card in an outer instance (nested, or found along the instance path) read by a MOSFET whose instance defines the name too |
| `c_body_param` | a body `.param` of the macro computed from the instance's L, as in a PDK |
| `c_corner_tt`, `c_corner_ff` | a PDK library: corner sections, a wrapper subckt with binned cards (also the bin, from the `.op` table) |
| `c_func_global`, `c_temper`, `c_stat_param`, `c_stat_direct` | a `.param` function, `temper` at `.temp 50`, `agauss` through a `.param` and directly in a card |
| `c_dup_key` | a card that sets `vth0` twice: the first or the last? |
| `c_r_global`, `c_r_local`, `c_r_dynamic` | R cards (`rsh`): per instance, and the card on the instance path or the one the line's definition sees |
| `c_diode` | a diode card (`is`) per instance |
