# netlist-db

[![build](https://github.com/zao111222333/netlist-db/actions/workflows/build.yml/badge.svg)](https://github.com/zao111222333/netlist-db/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![netlist-db](https://shields.io/crates/v/netlist-db.svg?style=flat-square&label=crates.io)](https://crates.io/crates/netlist-db)
[![Docs](https://docs.rs/netlist-db/badge.svg)](https://docs.rs/netlist-db)
[![codecov](https://codecov.io/github/zao111222333/netlist-db/graph/badge.svg)](https://codecov.io/github/zao111222333/netlist-db)

Concurrent/Parallel SPICE (HSPICE) parser, under building.

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
./parser tests/top.sp
```

Or you can compile & run this example by

``` shell
cargo run --example parser --release -- tests/top.sp
```
<details>
<summary>The output should be</summary>
<?xml version="1.0" encoding="UTF-8" ?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd">
<!-- This file was created with the aha Ansi HTML Adapter. https://github.com/theZiz/aha -->
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
<meta http-equiv="Content-Type" content="application/xml+xhtml; charset=UTF-8"/>
</head>
<body>
<pre style="color:white; background-color:black">
cargo run --example parser --release -- tests/top.sp | aha --black > output.html
2025-01-27T01:40:43.135Z DEBUG [netlist_db::file] load File &quot;tests/inc/inc0.sp&quot;
2025-01-27T01:40:43.135Z DEBUG [netlist_db::file] load File &quot;tests/inc/inc1.sp&quot;
2025-01-27T01:40:43.135Z DEBUG [netlist_db::file] load File &quot;tests/lib.sp&quot;, section tt
2025-01-27T01:40:43.135Z DEBUG [netlist_db::file] load File &quot;tests/units.sp&quot;
2025-01-27T01:40:43.135Z DEBUG [netlist_db::file] load File &quot;tests/inc/inc2.sp&quot;
2025-01-27T01:40:43.135Z DEBUG [netlist_db::file] load File &quot;tests/inc/../lib.sp&quot;, section pre_layout
2025-01-27T01:40:43.136Z DEBUG [netlist_db::file] load File &quot;tests/inc/../cycle_ref0.sp&quot;
2025-01-27T01:40:43.136Z DEBUG [netlist_db::file] load File &quot;tests/inc/../cycle_ref1.sp&quot;
2025-01-27T01:40:43.137Z ERROR [netlist_db::lexer] 
File <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">&quot;tests/top.sp&quot;</span>, line <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">5</span>
.inc 'inc/inc0.sp'
                  <span style="font-weight:bold;"></span><span style="font-weight:bold;filter: contrast(70%) brightness(190%);color:red;">&lt;-</span>
<span style="font-weight:bold;"></span><span style="font-weight:bold;filter: contrast(70%) brightness(190%);color:fuchsia;">Error</span>: <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">No such file or directory (os error 2)</span>

2025-01-27T01:40:43.137Z ERROR [netlist_db::lexer] 
File <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">&quot;tests/inc/../cycle_ref1.sp&quot;</span>, line <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">2</span>
.inc 'cycle_ref0.sp'
                    <span style="font-weight:bold;"></span><span style="font-weight:bold;filter: contrast(70%) brightness(190%);color:red;">&lt;-</span>
<span style="font-weight:bold;"></span><span style="font-weight:bold;filter: contrast(70%) brightness(190%);color:fuchsia;">CircularDefinition</span>: <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">Detect circular definition in File &quot;tests/inc/../cycle_ref0.sp&quot;, line 2</span>
   File &quot;tests/top.sp&quot;, line 6
     ↓
   File &quot;tests/inc/inc2.sp&quot;, line 2
     ↓
   File &quot;tests/inc/../lib.sp&quot;, line 3, section pre_layout
     ↓
<span style="font-weight:bold;"></span><span style="font-weight:bold;filter: contrast(70%) brightness(190%);color:red;"> * File &quot;tests/inc/../cycle_ref0.sp&quot;, line 2</span>
     ↓
   File &quot;tests/inc/../cycle_ref1.sp&quot;, line 2
     ↓
<span style="font-weight:bold;"></span><span style="font-weight:bold;filter: contrast(70%) brightness(190%);color:red;"> * File &quot;tests/inc/../cycle_ref0.sp&quot;, line 2</span>

2025-01-27T01:40:43.137Z ERROR [netlist_db::lexer] 
File <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">&quot;tests/inc/../lib.sp&quot;</span>, line <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">7</span>
.params flag=1
       <span style="font-weight:bold;"></span><span style="font-weight:bold;filter: contrast(70%) brightness(190%);color:red;">&lt;-</span>
<span style="font-weight:bold;"></span><span style="font-weight:bold;filter: contrast(70%) brightness(190%);color:fuchsia;">SyntaxError</span>: <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">Unknwon command `params`</span>

2025-01-27T01:40:43.137Z ERROR [netlist_db::lexer] 
File <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">&quot;tests/top.sp&quot;</span>, line <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">8</span>
.lib 'lib.sp' tt
                <span style="font-weight:bold;"></span><span style="font-weight:bold;filter: contrast(70%) brightness(190%);color:red;">&lt;-</span>
<span style="font-weight:bold;"></span><span style="font-weight:bold;filter: contrast(70%) brightness(190%);color:fuchsia;">Error</span>: <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">Can NOT find section `tt` in file &quot;tests/lib.sp&quot;</span>

2025-01-27T01:40:43.137Z ERROR [netlist_db::lexer] 
File <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">&quot;tests/units.sp&quot;</span>, line <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">7</span>
+ micr?o=1u
<span style="font-weight:bold;"></span><span style="font-weight:bold;filter: contrast(70%) brightness(190%);color:red;">&lt;-</span>
<span style="font-weight:bold;"></span><span style="font-weight:bold;filter: contrast(70%) brightness(190%);color:fuchsia;">ParserError</span>: <span style="filter: contrast(70%) brightness(190%);color:fuchsia;">TakeWhile1</span>

======= AST ===========

.subckt DEMO A1 A2 var1=1 var2=2.option 
+ gmindc=0.00000000000001 scale=0.9
.param 
+ prelayout=1 flag_cc=1
X0.CCC net8 net23 VSS VPW NHVT11LL_CKT W=0.000000135 L=0.00000004
.params flag=1
.ends DEMO
.subckt UNITS A
.ends UNITS
X0.BBB net8 net23 VSS VPW NHVT11LL_CKT W=0.000000135 L=0.00000004
X0.AAA net8 net23 VSS VPW NHVT11LL_CKT W=0.000000135 L=0.00000004
======= ERR ===========
true
======= stats =========
parse: 2.200291ms
build: 40.833µs
print: 11.334µs
=======================
</pre>
</body>
</html>
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

- Model-card parameters are text only: the tool does not compute model
  values.
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
| `net_info("x5_3", "n4")` | 0.02 ms | < 10 ms |
| Peak memory, base deck | 79 MB | |
| Peak memory, one more level (10x the instances, about 10% more lines) | 91–93 MB (1.15–1.18x) | < 1.3x |
| Peak memory, two more levels (1.11M instances) vs the same lines with the groups unused (10,000 instances) | 104–106 MB vs 104–105 MB (1.00–1.01x) | < 1.1x |

The memory follows the input lines, not the expanded hierarchy: with the
same lines, 1.11M expanded instances need the same memory as 10,000. A
flat deck of 1M resistors loads in 0.5 s with 405 MB. A generated 33 MB
DSPF (100,000 nets, 400,000 parasitic R, 400,000 parasitic C, 100,000
MOSFETs) loads in 0.84 s. The same text with no `*|DSPF` line loads in
0.53 s.

## Regression
``` bash
cargo install cargo-regression # install for first time
cargo build --examples --release
cargo regression ./tests/
```

## TODO
+ Fully expression system
+ Support commands
  + tran
  + meas
  + ...