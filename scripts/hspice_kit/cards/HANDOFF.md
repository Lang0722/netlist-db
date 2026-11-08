# Message for the HSPICE verify agent

You run this on the machine with HSPICE (the reference so far was HSPICE
P-2019.06-SP1-1 on the Windows VM, `C:\synopsys\Hspice_P-2019.06-SP1-1\WIN64\hspice.exe`).
Nothing here needs Rust or netlist-db. The comparison needs only Python 3,
and it can run on another machine.

## Why

netlist-db has a new query, `resolve_model` (`query/src/params/cards.rs`).
It gives the `.model` card that an element uses (for a MOSFET, its bin) and
the values of the card's parameters. In a PDK in the macro-model style, a
wrapper subckt (`nch_mac`) holds binned cards whose `vth0`, `u0`, … read the
instance's L, W and NF and the corner's `.param` values. `resolve_model`
evaluates each card parameter **where the card is defined**: a top-level card
with the top-level parameters, a card inside a subckt with the parameters of
that subckt's instance on the element's path.

HSPICE was measured to do so for the limits of a bin only (`lmin`, `lmax`,
…; decks `q_limits_*` of `scripts/hspice_kit/make_kit.py`), with one known
gap: the first MOSFET that looks at a bin evaluates its limits in its own
instance. For `vth0` and the other parameters it was never measured. The
answers of `resolve_model` say so in a caveat. Your run settles it.

## What to run

The kit is `scripts/hspice_kit/cards/kit/` of the repository (branch
`main`): 20 decks in `decks/` (and `cards_pdk.lib`, which two decks load),
netlist-db's answers in `expected.json`, and two run scripts. Read
`scripts/hspice_kit/cards/README.md` for how each deck tells HSPICE's value.

1. Get the repository at `main` (or copy `scripts/hspice_kit/cards/` to the
   machine). Do not change the decks or `expected.json`.
2. Run HSPICE on every deck, **from inside `decks/`** (the corner decks load
   `cards_pdk.lib` by a relative path). One of:
   - Windows: `$env:HSPICE = "C:\synopsys\Hspice_P-2019.06-SP1-1\WIN64\hspice.exe"`, then
     `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\hspice_kit\cards\kit\run_cards.ps1`
   - bash: `HSPICE=/path/to/hspice scripts/hspice_kit/cards/kit/run_cards.sh`
   - by hand, deck by deck: `cd decks` and `hspice -i NAME.sp -o ../results/NAME`.
     If you drive the VM from its host with `prlctl exec`, use one call per
     deck: loops inside one PowerShell call broke the quoting before.

   Each deck takes a second or so. The results go to `kit/results/`:
   `NAME.lis`, `NAME.ms0` (the measures), `NAME.stdout`, and
   `hspice_version.txt`.
3. Compare (Python 3, no packages):

   ```
   python3 scripts/hspice_kit/cards/compare_cards.py scripts/hspice_kit/cards/kit scripts/hspice_kit/cards/kit/results
   ```

   Run it again with `--verbose` for each deck that does not agree: it adds
   HSPICE's warnings and the values of its model listing.

## What to expect

Each probe is one card parameter of one element. The comparison prints:

- `agree`: HSPICE's value is netlist-db's (5 mV for a `vth0`, 2% for an
  `rsh` or an `is`).
- `DIFFER`: it is not. The line names the rules that HSPICE's value fits
  ("once for the subckt, in its first instance", "the MOSFET's own instance",
  "the top-level dv", …). That is the finding.
- `probed`: a question that netlist-db does not answer itself. `c_dup_key`
  asks which `vth0` HSPICE uses when a card sets it twice (the first, or the
  last).
- A probe for which netlist-db has no value (`c_top_card_notop`: a top-level
  card that reads a name only the subckts define) agrees when HSPICE stops on
  the deck. If HSPICE runs it, it evaluates the card in the MOSFET's
  instance: that is the known gap, and worth reporting.
- `card DIFFER`: the `.op` table names another card (bin) than netlist-db.

The decks most likely to tell something new: `c_mac_global` (is a card in a
subckt evaluated for each instance?), `c_top_card_local` (the known gap of
the bin limits, for `vth0`), `c_stat_direct` (does HSPICE accept `agauss`
directly in a card?), `c_dup_key`, `c_temper`, and `c_r_dynamic` (is the card
of an R line looked up along the instance path?).

Before you trust a deck, check that HSPICE ran it to the end: `ctl` (the
control) must be in its `.ms0`. If a deck stops on something that is not the
question (an older HSPICE that rejects a card parameter, a path), keep its
`.lis` as it is and say so. You may run a corrected copy (`NAME_fix.sp`)
next to it, and report both.

## What to bring back

- The full output of `compare_cards.py` (and of `--verbose` for each deck
  that differs or stops).
- `hspice_version.txt`.
- The results themselves: `hspice_card_results.zip` (or `.tar.gz`), or
  commit `kit/results/` to a new branch (for example `hspice/card-results`)
  and push it. First remove the licence lines (`lic:`) from each `.lis`: the
  project does not store them.

Do not change netlist-db's code. If you want to write the findings down, add
them to `golden/NOTES.md`, section "Comparison rounds with HSPICE as the
reference", one line per finding with the deck name, in the style of that
file. The netlist-db session then changes the query and its caveats to
match.
