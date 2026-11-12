# netlist-db

[![build](https://github.com/zao111222333/netlist-db/actions/workflows/build.yml/badge.svg)](https://github.com/zao111222333/netlist-db/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![netlist-db](https://shields.io/crates/v/netlist-db.svg?style=flat-square&label=crates.io)](https://crates.io/crates/netlist-db)
[![Docs](https://docs.rs/netlist-db/badge.svg)](https://docs.rs/netlist-db)
[![codecov](https://codecov.io/github/zao111222333/netlist-db/graph/badge.svg)](https://codecov.io/github/zao111222333/netlist-db)

Concurrent/Parallel SPICE (HSPICE) parser.

## Features
+ Concurrent/Parallel parse multi SPICE files (`.inc` and `.lib`) command
+ Use span to store string, avoid small allocations
+ Units system
+ Circular definition detection
+ HSPICE expressions and parameters, evaluated through the subckt hierarchy (`eval`)
+ HSPICE's model selector: the model bin (`nch.1`, `nch.2`, ...) each MOSFET uses (`models`)

## Model selection
For a binned model library, HSPICE chooses for each MOSFET the first bin
`name.suffix`, in netlist order, whose ranges hold its length and width
(`LMIN <= L < LMAX`, `WMIN <= W/NF < WMAX`). `AST::mosfet_models` gives every MOSFET of the circuit
with its instance path, its L and W (with `.param`, subckt and instance
parameters resolved) and the model chosen for it, or why none could be:

``` rust
let (ast, _) = files.build(parsed);
for m in ast.mosfet_models() {
    match &m.selection {
        Ok(s) => println!("{} {} L={:e} W={:e} -> {}", m.path, m.mname, s.l, s.w, s.model.name),
        Err(e) => println!("{} {}: {e}", m.path, m.mname),
    }
}
```

`AST::mosfet_models_of` does the same for one subckt (e.g. a library cell),
and `AST::select_model` for a given model name, L and W.

These rules were measured on HSPICE P-2019.06 (`scripts/hspice_kit`).

Parameters: a subckt instance has its `.subckt` defaults, its `.param`
statements (which replace a default of the same name) and the values passed
on its line (used over both, and computed in the new instance). A name is
resolved from the closest level that has it with `.option parhier=local`, or
from the level closest to the top with the default `parhier=global`, also
over a value passed below it. A text that reads its own name reads that name
at the level above; a value passed whose text leads back to it through other
names is computed, as a whole, at the level above.

The last `.option parhier` of the deck decides, wherever it is, also in an
included file or a `.lib` section, but not inside a subckt. A netlist that a
`parhier=local` testbench includes has no such line: give `model_select` one
more file that holds it (`model_select netlist.sp local.sp`). It notes on
stderr when it finds none.

Models: at each level of the MOSFET's instance path, from its own up to the
top, the model of that name, else the first bin that holds L and W; a bin that
does not hold them hides nothing. A model or subckt defined in the subckt of
an instance above is seen, so one line can use different models on different
paths. A bin must set its four limits. The ranges of a bin are evaluated where
the model is defined. `W/NF` is compared unless `WNFLAG=0` on the instance or,
without one, `.option wnflag=0`; a `WNFLAG` in a model is ignored. A value at
most 2e-15 (relative) below a limit counts as on it. L and W may also be the
first two values after the model name.

Known gaps (`scripts/hspice_kit/README.md`): HSPICE evaluates the ranges of a
bin in the instance of the first MOSFET that looks at it, which matters only
if a level below the model binds, differently, a name those ranges read (so
for at most one MOSFET per bin, and for none if the ranges are numbers); and
the creation-order rules of `.param` functions.

A hierarchical netlist can hold far more transistors than fit in a list (a
30 MB netlist can flatten to hundreds of millions). `AST::for_each_mosfet_model`
gives them one by one without keeping them, and `AST::model_usage` counts
how many use each model (and how many have no model, with an example path)
without visiting each one: a subckt instance is evaluated once for the
parameters its contents read, and its counts are reused for every instance
that sees the same values.

``` rust
let usage = ast.model_usage();
for (model, n) in &usage.models {
    println!("{} {n}", model.name);
}
for (e, n, example) in &usage.errors {
    println!("{e}: {n} MOSFETs, e.g. {example}");
}
```

From the command line (`--summary` for the counts, `--subckt NAME` for one
subckt):

``` shell
cargo run --release --example model_select -- deck.sp
cargo run --release --example model_select -- --summary deck.sp
```

Rows go to stdout. Errors and warnings go to stderr, with their file and line.
The exit status is 1 when the netlist has an error or a MOSFET has no model. The
rows are then those that could be read, so a count can be too low.

## Example
Download the [releases/latest/examples.zip](https://github.com/zao111222333/netlist-db/releases/latest/download/examples.zip), then
``` shell
cd examples_x86_64-unknown-linux-musl
./parser_multi tests/top.sp
```

Or you can compile & run this example by

``` shell
cargo run --example parser_multi --release -- tests/top.sp
```
<details>
<summary>The output should be</summary>

`tests/top.sp` has errors on purpose: an include file that does not exist,
a circular include and a `.lib` section that does not exist. So the exit
status is 1. Here the `INFO` line for each file that is loaded is left out.
The times change from run to run.

``` text
2026-11-07T10:06:26.057Z ERROR [netlist_db::err] 
File "tests/top.sp", line 5
.inc 'inc/inc0.sp'
                  <-
Error: No such file or directory (os error 2)

2026-11-07T10:06:26.057Z ERROR [netlist_db::err] 
File "tests/inc/../cycle_ref1.sp", line 2
.inc 'cycle_ref0.sp'
                    <-
CircularDefinition: Detect circular definition in File "tests/inc/../cycle_ref0.sp", line 2
   File "tests/top.sp", line 6
     ↓
   File "tests/inc/inc2.sp", line 2
     ↓
   File "tests/inc/../lib.sp", line 3, section pre_layout
     ↓
 * File "tests/inc/../cycle_ref0.sp", line 2
     ↓
   File "tests/inc/../cycle_ref1.sp", line 2
     ↓
 * File "tests/inc/../cycle_ref0.sp", line 2

2026-11-07T10:06:26.057Z WARN  [netlist_db::err] 
File "tests/inc/../lib.sp", line 7
.params flag=1
       <-
Warning: Unknown command `params`, kept as-is

2026-11-07T10:06:26.057Z ERROR [netlist_db::err] 
File "tests/top.sp", line 8
.lib 'lib.sp' tt
                <-
Error: Can NOT find section `tt` in file "tests/lib.sp"

======= AST ===========

.SUBCKT DEMO A1 A2 var1=1.0000000e0 var2=2.0000000e0
.OPTION 
+ gmindc=1.0000000e-14 scale=9.0000000e-1
.PARAM 
+ prelayout=1.0000000e0 flag_cc=1.0000000e0
X0.CCC net8 net23 VSS VPW NHVT11LL_CKT W=1.3500000e-7 L=4.0000000e-8
.params flag=1
.ENDS DEMO
.SUBCKT UNITS A
.PARAM 
+ tera=1.0000000e12 giga=1.0000000e9 mega=1.0000000e6 kilo=1.0000000e3
+ mil=2.5400000e-5 micro=1.0000000e-6 nano=1.0000000e-9 pico=1.0000000e-12
+ femto=1.0000000e-15 atto=1.0000000e-18 db0=1.0000000e0 db20=1.0000000e1
+ db40=1.0000000e2 min=6.0000000e1 hr=3.6000000e3 day=8.6400000e4
+ yr=3.1536000e7
.ENDS UNITS
X0.BBB net8 net23 VSS VPW NHVT11LL_CKT W=1.3500000e-7 L=4.0000000e-8
X0.AAA net8 net23 VSS VPW NHVT11LL_CKT W=1.3500000e-7 L=4.0000000e-8
======= stats =========
parse: 3.149916ms
build: 132.542µs
print: 106.25µs
=======================
```
</details>

## LLM query tool

A real netlist is too large for the context of an LLM. The crate
`netlist-query` (`query/`) parses an HSPICE deck once and answers small
questions about it, in JSON, in pages:

- Which subcircuits the deck defines, and how many instances each one has
  in the expanded hierarchy (`list_subckts`, `subckt_info`, `instances_of`).
- What is inside a subcircuit: ports, defaults, `.param` lines, devices by
  type, the subcircuits it calls (`subckt_info`), and its original text,
  from `.subckt` to `.ends` (`subckt_text`).
- The value of a parameter or of an element parameter, through the
  hierarchy, with the chain of definitions that gave it (`resolve_param`,
  `resolve_element`).
- The `.model` card that an element uses and the values of the card's
  parameters (`resolve_model`). For a MOSFET the card is the bin that HSPICE
  chooses from its L, W and NF, as `model_select` chooses it. The parameters
  are evaluated where the card is defined: a card inside a PDK wrapper
  subckt (`nch_mac`) gets the parameters of that instance, and of the corner
  that the `.lib` line (or `lib_override`) picks.
- Which pins are on a net, and where the net goes through ports
  (`net_info`, `trace_net`).
- DSPF parasitic files: included as netlist text, or given with
  `.option sim_dspf`. Parasitic R and C are not devices. A DSPF net is one
  logical net, with its parasitic sums.

The same queries are available in Rust (`netlist_query::Netlist`), in Python
(`pybind/`, module `netlist_db`) and as an MCP server (`mcp_server/`). The
expected behavior comes from HSPICE P-2019.06 runs: `golden/cases` holds the
decks and listings, and `golden/NOTES.md` records each finding.

### Install

```sh
python3 -m venv .venv
.venv/bin/pip install maturin pytest "mcp>=2,<3"
(cd pybind && ../.venv/bin/maturin develop --release)
.venv/bin/pip install -e mcp_server
```

### Python

```python
import netlist_db

# a golden deck: `x1` calls `parvspar`; `whowins` is 4 at the top level,
# 3 on the X line, 1 and 5 in the subckt
net = netlist_db.load("golden/cases/parhier_global/deck.sp")  # OSError if unreadable
print(net.summary()["expanded_devices"])        # {'resistor': 1, 'vsource': 1}
for row in net.list_subckts(sort="copies", limit=5)["items"]:
    print(row["name"], row["copies"])           # parvspar 1
print(net.resolve_param("x1", "whowins")["value"])  # 4.0 (parhier=global)
pins = net.net_info("x1", "a")       # {"total": ..., "connections": {"items": [...], "next_cursor": ...}}
err = net.subckt_info("nosuch")      # {"error": {"kind": "not_found", "message": ..., "suggestions": [...]}}

# a MOSFET in a PDK wrapper subckt: its bin, and the values of the card
card = netlist_db.load("top.sp").resolve_model("xamp.x1.m0", names=["vth0", "tox"])
card["card"]["name"], card["card"]["scope"]    # ('nch.1', 'xamp.x1')
card["params"]["items"][0]["value"]            # vth0, with this instance's L and the corner's shift
```

A query that fails gives a `dict` with the key `error`, so use
`answer.get("value")` when a name can be missing.

### MCP

```sh
claude mcp add netlist-db -- /path/to/netlist-db/.venv/bin/python -m netlist_mcp
```

Other clients start the same command on stdio. `load_netlist` returns a
handle, and the other tools take it first. See `mcp_server/README.md`.

### How to read the answers

- An instance path is the lowercase instance names, joined by `.`
  (`x1.xinv.m3`). `""` is the top level. HSPICE finds an `X` line's subckt
  along the instance path, so one `X` line can call different nested
  definitions in different instances.
- `net_info` and `trace_net` also take an HSPICE full node name, such as
  `x1.xm1.n1` from a listing or a waveform, as the net, with the scope
  path `""`. When the scope has no net with that name, the name is read as
  an instance path, a `.`, and a net of that instance. A net name can hold
  a `.`, so each `.` is tried, the last one first: HSPICE finds `x1.n.1`
  as the net `n.1` of `x1` (golden `node_dotted_in_subckt`).
- Parameters follow `.option parhier`. The default is `global`: a top-level
  `.param` wins over an `X`-line override or a subckt default. With
  `parhier=local`, the definition nearest the element wins.
- `caveats` says what makes an answer less certain: an `X` line with no
  visible definition, a deck that HSPICE rejects, a value from an `.alter`
  block (answers describe the first run), a total that is a lower bound.
  Read them.
- `diagnostics` lists the problems of the load. An `error` diagnostic is
  usually a form that HSPICE rejects.

### Limits

- `resolve_model` gives the values written on a model card. It does not
  compute the values that the simulator derives from them: the `l…`, `w…`
  and `p…` terms of a binned model with Leff and Weff, the temperature, and
  the defaults of the parameters that the card does not set. A card's
  parameters are evaluated where the card is defined. HSPICE was measured
  to do so for the limits of a bin (with one known gap,
  `scripts/hspice_kit/README.md`) and for the other card parameters (the
  kit `scripts/hspice_kit/cards`).
- An element letter that the parser does not read (for example `E`, `G`,
  `B`) is counted by its letter, and its nets are marked
  `connectivity_incomplete`. So are the nets of a definition with an
  element line that a load error skipped (for example `G1 … POLY(2) …`).
- Verilog-A modules (`.hdl`) and encrypted regions (`.prot`) are not read.
  In a deck with a `.prot` region, the load checks of undefined names are
  off, and a name that only the region defines has no value. An `X` line
  that calls a subckt with no visible definition gets a Warning, not an
  Error, because the region can define it (golden `prot_subckt`).
- HSPICE joins the nets that have one full name into one node: a
  top-level net written `x1.n` is the net `n` of instance `x1` (golden
  `node_dotted_collision`, `node_dotted_two_splits_a`). The tool keeps
  them apart. `net_info` and `trace_net` give a caveat that names the
  other net when the full name of their net has another reading.
- The DSPF scanner reads lines: a `*|` record or a parasitic value on a `+`
  line is not read (the sum of the values is then `null`, with a caveat).
  An R or C line after a `*|NET` block that touches no node of the block is
  a device (Warning `dspf_device_in_block`).
- Some decks that HSPICE rejects load with no Error. `golden/NOTES.md`,
  "Known differences", lists them with the reason for each. A query that
  reaches such a line usually gives the reason.

### Speed and memory

`query/tests/perf.rs` (run it with `timeout 180 cargo test --release -p
netlist-query --test perf -- --ignored --test-threads=1 --nocapture`) loads
a deck of 1,000 subckts with 102 devices each, called 10 times each (10,000
instances, 1.02M expanded devices, 114,000 lines). The decks come from
`query/benches/gen_big.py`. On an Apple M1 Pro (release build):

| Measure | Result | Gate |
|---|---|---|
| Load | 0.08 s | < 1 s |
| `list_subckts` (by copies) | 0.2 ms | < 10 ms |
| `resolve_element("x5_3.m7")` | 0.5–0.6 ms | < 10 ms |
| `net_info("x5_3", "n4")` | 0.25 ms | < 10 ms |
| Peak memory, base deck | 84 MB | |
| Peak memory, one more level (10x the instances, about 10% more lines) | 97–99 MB (1.16–1.18x) | < 1.3x |
| Peak memory, two more levels (1.11M instances) vs the same lines with the groups unused (10,000 instances) | 110 MB vs 112 MB (0.99x) | < 1.1x |

The memory follows the input lines, not the expanded hierarchy: with the
same lines, 1.11M expanded instances need the same memory as 10,000. A
flat deck of 1M resistors loads in 0.5 s with 405 MB. A generated 33 MB
DSPF (100,000 nets, 400,000 parasitic R, 400,000 parasitic C, 100,000
MOSFETs) loads in 0.84 s. The same text with no `*|DSPF` line loads in
0.53 s.

## Where the code is

| To find | Look in |
|---|---|
| The types of parsed statements: values, `key=value`, `.model`, `.subckt`, `.data` | `src/ast.rs` |
| Element lines as parsed: R, C, L, sources, M, Q, D, X, E/F/G/H, K | `src/instance.rs` |
| Spans, file ids and file storage | `src/span.rs` |
| Parse errors and how they are printed | `src/err.rs` |
| The text of a file before the parse: bytes, comments, joined lines | `src/parser/source.rs` |
| The parallel parse of the included files (`ParseManager`) | `src/parser/manager.rs` |
| Include lines: search, read, limits (`IncludeLimits`) | `src/parser/include.rs` |
| Words of a line: separators, numbers, names, values | `src/parser/tokens.rs` |
| Statements: dot commands, `.subckt` headers | `src/parser/statement.rs` |
| Blocks that a failed statement skips | `src/parser/blocks.rs` |
| Element lines (M, X, R, C, sources) | `src/parser/instance.rs` |
| `.ic` and `.nodeset` node voltages | `src/parser/cmds.rs` |
| Readers of HSPICE output files (`.mc0`, `.mt0`, `.sw0`) | `src/parser/data_mc.rs`, `src/parser/data_meas.rs`, `src/parser/data_sweep.rs` |
| The walk from a parse to a netlist (`Files::build`) | `src/netlist_build.rs` |
| Printing a netlist back as text | `src/display.rs` |
| The walk down the subckt hierarchy (`Subckt::append`) | `src/hierarchy.rs` |
| The selector's parameters and scopes | `src/eval.rs` |
| The selector's expression parser and built-in functions | `src/eval/expr.rs` |
| Model selection for each MOSFET | `src/models.rs` |
| Bin rules: limits, L and W of a line | `src/models/bins.rs` |
| Model counts through the hierarchy (`--summary`) | `src/models/summary.rs` |
| Query tool: the output types and paging | `query/src/api.rs` |
| Query tool: the owned model that the queries read | `query/src/model.rs` |
| Query tool: names in any letter case (`Sym`) | `query/src/intern.rs` |
| Query tool: from a parse to its data | `query/src/lower.rs`, `query/src/lower/` |
| Query tool: instances and their counts | `query/src/hier.rs`, `query/src/hier/` |
| Query tool: instance paths (`x1.x2.m3`) | `query/src/paths.rs` |
| Query tool: nets (`net_info`, `trace_net`) | `query/src/nets.rs` |
| Query tool: DSPF parasitic files | `query/src/dspf.rs` |
| Query tool: load checks for forms that HSPICE rejects | `query/src/rejects.rs` |
| Query tool: parameters | `query/src/params.rs`, `query/src/params/` |
| Query tool: the card of an element and its values (`resolve_model`) | `query/src/params/cards.rs` |
| Query tool: expressions | `expr/src/` |

Some files keep their tests in a `tests.rs` in the folder named after the
file: the tests of `src/models.rs` are in `src/models/tests.rs`. The others
have an inline `mod tests` at the end of the file (`src/span.rs`,
`query/src/paths.rs`). The parser tests that parse whole decks are in
`src/parser/*_tests.rs`, a file for each topic. `src/tests/` holds the
tests of the whole crate, a file for each topic. `query/tests/` holds the
tests of the query tool that load decks through its public API.

## Dependencies

The crates that the workspace builds with are in `vendor/`, at the versions
of `Cargo.lock`, and `.cargo/config.toml` tells cargo to read them from
there: a build needs no network. After a change of a dependency in a
`Cargo.toml`, write the folder again, and commit it with `Cargo.lock`:

``` bash
cargo vendor
```

`cargo update` finds a newer version only when it reads crates.io and not
`vendor/`:

``` bash
mv .cargo/config.toml .cargo/config.off
cargo update
mv .cargo/config.off .cargo/config.toml
cargo vendor
```

The Python packages are not vendored: `maturin` builds the binding, and
`mcp` is the one dependency of `mcp_server`.

## Regression
``` bash
cargo install cargo-regression # install for first time
cargo build --examples --release
cargo regression ./tests/
```

## TODO
+ Support commands
  + tran
  + meas
  + ...