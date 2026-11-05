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
for at most one MOSFET per bin, and for none if the ranges are numbers); the
creation-order rules of `.param` functions; and `M1 d g 0 a pl pw` (3 nodes,
then names of parameters after the model name), which is reported as an error.

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