# netlist-db as an LLM query tool for HSPICE netlists — design

Date: 2026-10-31. Status: draft for review.

## 1. Intent

Real HSPICE netlists are often far too large for an LLM's context window.
The LLM therefore cannot read the file. This project turns the local fork of
netlist-db (`~/Project/netlist-db`, upstream
`https://github.com/zao111222333/netlist-db`) into a tool that reads the
netlist for the LLM and answers small, structured questions about it. The LLM
receives only the answers, never the whole file.

The user named three capabilities, all required in this project:

1. **Inventory and counts.** Which subcircuits the netlist defines, how many
   there are, and how many times each one is instantiated.
2. **Parameter resolution through the hierarchy.** For a device at an instance
   path, the effective parameter values, and the chain of definitions that
   produced each value, under HSPICE's `.option parhier` rules.
3. **Net connectivity.** What connects to a net inside a subcircuit, and how a
   net continues through ports into child instances and up into parents.

Also required by the user: a look inside one subcircuit (its ports, defaults,
devices, child instances and local parameters).

The LLM reaches the tool as a **Python library** or as an **MCP server**
(user's words: "python tools or mcp"). Both must exist.

The input is a **standard HSPICE-format netlist**. Sometimes it also comes
with an **SPF (DSPF) parasitic file** (user's words: "the standard HSpice
format. And sometimes it might also come with the SPF file"). See §6.1a.

## 2. Background facts the design rests on

These were established in the conversation that produced this document.

### 2.1 Why netlist-db

Benchmarks on 2026-10-31 (synthetic HSPICE-style decks, M-series Mac):

| Deck | netlist-db parse+build | KLayout read+flatten | analog-design-cli parse |
|---|---|---|---|
| 1.02M devices after expansion, 3.9 MB | 0.11 s, 112 MB RSS | 1.07 s, 892 MB RSS | 4.62 s, 301 MB RSS |

netlist-db is fastest and smallest, MIT-licensed, and written for HSPICE. It is
an AST builder only: no subcircuit linking, no counts, no expression
evaluation, no parameter scoping, no net graph. Upstream describes it as
"under building", and the last upstream commit is 2025-11-29 (`3d4ba3b`).

### 2.2 How netlist-db parses (current state)

- Hand-written `nom` parser combinators read characters directly. There is no
  separate lexer. Text is kept as spans into the file buffer. Numbers become
  `f64` with `fast-float2`.
- Each `.include` / `.lib` file is parsed in its own tokio task. One file is
  parsed on one thread.
- `Files::build` (`src/lib.rs:261`) joins all files in source order into one
  AST (`src/ast.rs`).
- Known device letters: R C L V I M Q D X. Other letters become
  `InstanceCtx::Unknown`.
- `pybind/` is an empty PyO3 stub: the module defines no functions.

### 2.3 Defects found by testing (must be fixed — see §6.1)

| Input | Current behavior |
|---|---|
| A MOSFET line with 5 nodes + model (`M1 d g s b x nch`) | The **whole file's AST is discarded**. Only one log line reports it. |
| `.param f(x)=x*2` (HSPICE parameter function) | The **whole file's AST is discarded**. |
| `R1 a b 1m`, `1M` | Kept as expression text `'1m'`. `M`/`m` (milli) is missing from the unit table (`src/parser/utils.rs:83`). |
| `R5 a b 10pF` | Kept as expression text. HSPICE ignores letters after a valid scale factor. |
| `.global vdd` | Reported as an unknown command (kept as raw tokens). |

The "whole file discarded" cases come from `_include` replacing the file AST
with the error (`src/parser/mod.rs:110-113`) when any statement parser fails.

### 2.4 HSPICE parameter scoping (verified with HSPICE on the VM)

Test deck: subcircuit default `whowins=1.0`, local `.param whowins=5.0` inside
the subcircuit, global `.param whowins=4.0`, instance `X1 … whowins=3.0`; a
resistor `r1 a b whowins` inside the subcircuit.

| Setting | HSPICE result for `x1.r1` |
|---|---|
| `.option parhier=local` | 3.0 (innermost wins: instance > local `.param` > subckt default; globals only if no local definition) |
| `.option parhier=global` | 4.0 (outermost definition wins) |
| no `parhier` option | 4.0 — **default is GLOBAL** in HSPICE P-2019.06-SP1-1 |

Secondary sources (Infineon AKB 2022 parameter quiz) add: under GLOBAL, an
override on a top-level `X` line wins at every level below it, and a
subcircuit default at a higher level wins over an `X`-line override made
inside that subcircuit for a deeper level. Forum reports suggest `parhier`
may not govern model-card parameters the same way.

### 2.5 HSPICE reference environment

- HSPICE P-2019.06-SP1-1 (win64) on the Parallels VM "Windows 11",
  `C:\synopsys\Hspice_P-2019.06-SP1-1\WIN64\hspice.exe`.
- A license server on the VM itself.
- **Access is through `prlctl exec "Windows 11" …`, not SSH** (user decision;
  SSH key signing through 1Password currently fails). Proven method: send the
  deck as base64 through `powershell -NoProfile -Command`, write it with
  `[IO.File]::WriteAllBytes`, run `hspice.exe -i x.sp -o x`, read `x.lis`.
  Use **one `prlctl exec` call per deck**: PowerShell loops broke the quoting.
- In the `.lis` file, `.op` lists each element by subcircuit path and name
  with its final value, for example `subckt x1` / `element 1:r1` /
  `r value 3.0000`. This is the ground truth for resolved values and for the
  set of expanded instances.

## 3. Goals

1. A Rust query core in the fork that loads an HSPICE netlist (with
   `.include` / `.lib` sections) once and answers the queries in §5 from
   memory.
2. Correct HSPICE semantics for scoping, units, expressions and `parhier`,
   checked against real HSPICE output.
3. A Python library (PyO3 + maturin, in `pybind/`) that exposes the same
   queries on a loaded netlist object.
4. An MCP server (Python) that exposes the same queries as tools and keeps
   loaded netlists in memory between calls.
5. Robust loading: one bad statement never removes other statements. Every
   problem appears in a structured diagnostics list.
6. Scales to post-layout size (assume millions of devices, many files)
   without expanding the hierarchy in memory.

## 4. Non-goals

- Simulation, operating points, or any electrical analysis.
- Evaluating **model-card** parameters (`.model` bodies). They are reported as
  text. (User guess accepted as a first-version limit; see §2.4.)
- Writing or editing netlists.
- Spectre, ngspice, CDL-specific or DSPF-specific syntax beyond what HSPICE
  accepts.
- Encrypted (`.prot`/`.unprot`) content: report that a protected region
  exists; do not try to read it.
- Pushing to upstream or opening upstream PRs. The fork stays local.

## 5. Anti-goals (failure even if it "works")

- **Silent wrong values.** If a value cannot be resolved with confidence, the
  answer says "unresolved" and why. It never returns a guess as a number.
  (KLayout returned W=L=0.0 silently for global parameters inside subcircuits;
  this is the failure to avoid.)
- **Context flooding.** No answer may be unbounded. analog-design-cli wrote
  98 MB of JSON for a 100k-device deck. Every list is limited and paged.
- **Re-parsing per query.** analog-design-cli re-parsed the file on each CLI
  call. Load once, query many times.
- **Losing a file to one bad line** (current netlist-db behavior, §2.3).

## 6. The design

### 6.1 Parser fixes in netlist-db

- Statement-level error recovery: when a statement parser fails, record a
  diagnostic with file and line, skip to the next logical statement (next line
  that is not a `+` continuation), and continue. Never replace a file's AST
  with an error.
- Units: add `M`/`m` = 1e-3 (keep `MEG`/`ME`/`X` = 1e6, `MIL`/`MI` = 25.4e-6);
  accept and ignore trailing letters after a valid scale factor (`10pF`).
  Check the full HSPICE suffix rules against HSPICE (§7).
- MOSFET: accept the node counts HSPICE accepts (4 nodes + model is standard;
  extra nodes for SOI-style models). Do not reject a line for its node count.
- `.param` function definitions (`.param f(x)=…`): parse and keep them.
- `.global`: parse into the AST (needed for connectivity).
- Unknown dot-commands: keep as today (raw tokens + diagnostic), but at
  "info" level for known-harmless analysis commands (`.tran`, `.op`, `.temp`,
  `.alter`, …) so they do not look like errors.

### 6.1a SPF / DSPF parasitic files

A DSPF file is SPICE-syntax text with structured `*|` comment lines, for
example:

```
*|DSPF 1.0
*|DIVIDER /
*|DELIMITER :
.SUBCKT inv A Y VDD VSS
*|NET A 1.2e-15
*|P (A I 0 0 0)
*|I (M1:G M1 G I 1e-16 0 0)
*|S (A:1 0 0)
R1 A A:1 10
C1 A:1 0 1e-16
M1 Y M1:G VSS VSS nch W=1u L=0.1u
.ENDS
```

Requirements (assistant calls — the user gave no sample yet):

- **Two ways it can arrive**, both supported:
  1. Included as the netlist itself, or through `.include`: the DSPF holds
     the extracted devices and the parasitic R/C. It loads like any HSPICE
     file.
  2. Referenced for back-annotation of a pre-layout netlist (HSPICE
     `.option sim_dspf`, and related options). The pre-layout netlist gives
     the logical structure; the DSPF is parsed only to attach per-net
     parasitic data. Confirm which options HSPICE P-2019.06 accepts with a
     golden deck.
- **Parse the `*|` header and net blocks** (`*|DSPF`, `*|DIVIDER`,
  `*|DELIMITER`, `*|BUSBIT`, `*|GROUND_NET`, `*|NET`, `*|P`, `*|I`, `*|S`)
  instead of dropping them as comments. Names must accept the DSPF divider,
  delimiter and bus characters (`/`, `:`, `[]`, `<>`).
- **Logical nets.** A DSPF splits one logical net into many sub-nodes
  (`A:1`, `A:2`) and instance-pin nodes (`M1:G`). Connectivity answers are
  given at the **logical-net** level by default, using the `*|NET` blocks to
  map every sub-node and instance-pin node to its logical net. Do not map by
  splitting names at the delimiter: `M1:G` belongs to the net that lists it
  in a `*|I` line, not to a net called `M1`.
- **Parasitic summary per logical net**: total capacitance (from `*|NET`),
  count and sum of parasitic R and C elements, pin count. An option gives the
  physical sub-node view for one net on request, paged.
- **Size.** DSPF files are often much larger than the netlist. Parasitic
  R/C elements are stored compactly and are never listed in full by default.
- Parasitic R/C elements inside a DSPF are **not** counted as devices in
  device totals. They are reported separately.

### 6.2 Query core (new Rust module in the fork)

**Definitions and hierarchy.**
- Each `.subckt` becomes a definition: name, ports, header defaults, local
  `.param`s, elements, child calls, source location (file, line). The
  top-level statements form an implicit root definition.
- HSPICE names are case-insensitive. Keep the original spelling for display.
- Link each `X` line to its definition. Unresolved names are diagnostics, not
  errors. Nested `.subckt` definitions are allowed. HSPICE finds the
  definition along the **instance path** (golden `nested_def_dynamic`,
  `nested_def_reach`): first the nested definitions of the current
  instance's definition, then those of each ancestor instance's definition,
  then the top level. So one `X` line can bind to different definitions on
  different paths. Two nested definitions with the same name are different
  definitions. A duplicate top-level `.subckt` name makes HSPICE abort
  (golden `dup_subckt`): report an Error diagnostic, use the first
  definition, and add a caveat to answers that depend on it.
- Detect call cycles and report them.

**Counts without expansion.**
- `x_lines(def)`: number of `X` lines that call `def`.
- `copies(def)`: number of instances after full expansion, computed over the
  call graph (a DAG): `copies(root) = 1`;
  `copies(child) = Σ over parents copies(parent) × calls(parent → child)`.
  When nested definitions exist, a graph node is (definition, lookup
  context), because the binding of an `X` line depends on the path. With no
  nested definitions this is the plain definition graph.
- Expanded device totals: `Σ copies(def) × devices_in(def)`.
- Report the `m` multiplier separately from copies (copies count instances;
  `m` is a parameter).
- Definitions with zero copies are listed as unused.
- Counts can exceed 2^32; overflow must be detected, not wrapped.

**Parameter resolution.**
- An expression parser for HSPICE expressions: a small Pratt parser
  (binding-power table), with HSPICE units (`M` = milli), HSPICE built-in
  functions, user `.param` functions, and case-insensitive names. Do not copy
  OpenVAF's parser (GPL-3.0, Verilog-A semantics: `M` = mega, `^` = XOR); use
  it only as a model of the Pratt method.
- Resolution is **per instance path** (for example `x1.x2.m3`), computed on
  demand. It is never precomputed for all instances.
- Scope chain along the path, ordered by `.option parhier` (default GLOBAL,
  §2.4). `.lib` corner selection and `.include` order apply as HSPICE does.
- Every resolved value carries its chain: each step names the definition kind
  (instance override / local `.param` / subckt default / global `.param` /
  `.param` function), the scope (path), the source location, and the raw text.
- Cycles and undefined names give "unresolved" with the reason (anti-goal 1).

**Connectivity.**
- Per definition: net → list of (element or child instance, pin). Pin roles
  are named for known devices (MOSFET D/G/S/B, BJT C/B/E, diode A/K, two-pin
  elements 1/2) and by port name for `X` instances.
- Across the hierarchy: map a net down through an instance's port list into
  the child, and up from a port to the parent's net, on demand by path.
- `.global` nets and ground (`0`, and HSPICE's ground aliases — check against
  HSPICE) connect across all levels.
- High-fanout nets (supplies) return counts plus a paged list.
- **Element letters netlist-db does not parse** (E F G H B J K T W U S and
  any other): user decision (b). They count as devices, typed by letter.
  Their node list is not trusted, so every net they appear to touch is
  marked `connectivity_incomplete: true` with the element name, in
  `net_info` and `trace_net` answers. Their parameters are not resolved
  ("unresolved: element type not parsed").

### 6.2a Roots, title line, corners, `.alter`, statistical values

These were settled after the builder check. All are assistant calls (§9).

- **Input kinds.** Both a full HSPICE deck (title line, top-level `X` lines,
  analysis commands) and a bare netlist (only `.subckt` definitions, as in
  `tests/asap7_extracted.sp`) must load.
- **Title line.** HSPICE ignores the first line of the **top-level** file
  only, even when it is a dot command (golden `title_line`, `title_dot`).
  `load` has an option `title_line` with values `auto` (default), `yes` and
  `no`. `auto` treats line 1 as a title unless it starts with `.subckt`,
  `.macro`, `.lib`, `.inc`, `.include` or `.model`: such a file is a library
  loaded directly, not a deck. When line 1 has content, an Info diagnostic
  says which choice was made. Included files never have a title line.
- **Root.** If the top-level scope has instances, it is the root, and paths
  start from there (`x1.x2.m3`). If it has none, `load` reports
  `root: none`, all `copies` are reported as `null` with the reason "no
  top-level instances", and the caller may pass `top=<subckt>`. With `top`:
  that subcircuit is the root, paths start at its contents (`x2.m3`), its
  header defaults are its parameter values, and its ports are free nets.
  `x_lines` counts work without a root.
- **Corners.** The deck's own `.lib` selections apply. A different corner is
  a new `load` with an override argument `lib_override=[(file, from_section, to_section), …]`. Only `.lib` calls to that file AND that section change, so a deck that calls `tt` and `res_tt` of one library switches each one explicitly. An override that matches no call gives a Warning.
  Parsing is fast enough (§2.1) that this does not break "load once, query
  many".
- **`.alter`.** Answers describe the **first run** only. Each parameter,
  `.lib` or element that an `.alter` block changes gets a caveat in its
  answer ("changed by .alter #2 at file:line"). Diagnostics list all
  `.alter` blocks.
- **Statistical and sweep values.** `agauss`, `gauss`, `aunif`, `unif` and
  similar functions return their **nominal** value, because HSPICE `.op`
  without Monte Carlo uses nominal values, so the golden decks check this.
  Each such value carries a caveat (`statistical: nominal`). Parameters
  swept by `.dc` / `.data` / `.step`-like sweeps and expressions that use
  `temper` also return the nominal value (sweep start or `.temp` / default
  temperature) with a caveat. A value is "unresolved" only when no nominal
  value exists.
- **Result shape.** Every resolved value has `value`, `chain`, and
  `caveats` (possibly empty), or `unresolved` with a `reason`.

### 6.3 Query API (the same in Rust, Python and MCP)

First draft. Names may change. All answers are compact JSON-like structures
with a `limit` and a continuation token for any list.

| Query | Returns |
|---|---|
| `load(path, top?, lib_override?, title_line?)` | handle + summary: files, definitions, root, top-level instances, expanded device total, `parhier` mode, diagnostics count (§6.2a) |
| `diagnostics(level?)` | paged list with file and line |
| `list_subckts(sort?, filter?)` | per definition: name, ports count, devices count, `x_lines`, `copies`, location |
| `subckt_info(name)` | ports, header defaults, local params, devices by type with model names, child calls by definition |
| `instances_of(name)` | the `X` lines that call it: parent definition, instance name, location, overrides |
| `element_info(path)` | one device or instance by path: type, pins → nets, raw params |
| `resolve_param(path, name)` / `resolve_element(path)` | value(s) + chain, or "unresolved" + reason |
| `net_info(path, net)` | connections in that scope, with pin roles; global/ground flag |
| `trace_net(path, net, direction, depth)` | where the net goes through ports, down or up, bounded by depth |

### 6.4 Layers

1. Rust core in the fork (parser fixes + query module).
2. Python package from `pybind/` (PyO3 + maturin, abi3-py310 as already
   configured). A loaded netlist is a Python object that keeps the Rust data
   alive.
3. MCP server in Python on top of layer 2. It holds loaded netlists by handle
   between calls and only translates calls. No logic is duplicated in Python.

## 7. Verification

- Golden decks: each tests one rule (parhier local/global/default, instance
  overrides, nested defaults, `.lib` corners, `.include`, unit suffixes
  including `1m`/`1M`/`1MEG`/`10pF`, `.param` functions, built-in functions,
  nested `.subckt` lookup, `.global`, ground aliases, `m` factor).
- In each deck, the parameter under test sets a **resistor value**, so the
  HSPICE `.op` listing gives the true value per instance path.
- HSPICE runs on the VM through `prlctl exec` (§2.5). The deck and its `.lis`
  output are stored together in the repository; tests compare against the
  stored output and run without HSPICE.
- The `.op` element listing also gives the full set of expanded instances,
  which checks `copies`.
- Robustness: the existing malformed-line cases (§2.3) become regression
  tests.
- DSPF decks: a small hand-written DSPF (included form) and a back-annotated
  pre-layout deck, run in HSPICE. Check the logical-net mapping, the per-net
  parasitic sums, and that HSPICE's element listing matches the tool's
  device and parasitic counts. When the user supplies a real DSPF, add it
  (or a reduced copy) as a regression input.
- Performance gate: the 1.02M-device synthetic deck must still load in well
  under 1 s, and a single query on it must answer in milliseconds. Memory must
  not grow with the expanded instance count.

## 8. Constraints

- Work only in the local fork. Never push to upstream or open upstream PRs.
- Keep the MIT license of the fork. Do not copy GPL/AGPL code (OpenVAF,
  KLayout, spice-lsp); they may be read for ideas only.
- HSPICE P-2019.06-SP1-1 behavior is the ground truth.
- Rust edition 2024 (as in the fork); local toolchain is cargo 1.99.
- Long-running commands (HSPICE, benchmarks) are bounded with a timeout.

## 9. Decisions

Made by the user:
- Base: netlist-db, forked locally.
- Dialect: HSPICE.
- Interfaces: Python library and MCP server.
- Scope: inventory/counts, subckt contents, parameter resolution with
  `parhier`, and net connectivity — all in this project.
- HSPICE reference runs through `prlctl exec`, not SSH.
- Input: standard HSPICE format, sometimes with an SPF (DSPF) file.
- Unparsed element letters: option (b) — count them, flag their nets as
  connectivity incomplete.

Made by the assistant (open to change):
- Rust core + PyO3 binding + thin Python MCP server, with no logic in Python.
- On-demand, per-path resolution and DAG-based counts instead of expansion.
- Resistor-value decks + stored `.lis` files as the test oracle.
- Model-card parameters are text-only in this version.
- Answer size limits and paging on every list.
- Bare netlists load, with an optional `top`; title-line handling `auto`
  (§6.2a).
- Corners change by a new `load` with `lib_override`; `.alter` answers
  describe the first run, with caveats (§6.2a).
- Statistical and sweep values return nominal numbers with caveats, not
  "unresolved" (§6.2a).
- DSPF: both the included form and the back-annotation form; logical-net
  view by default via `*|NET` blocks; parasitics summarized per net and
  excluded from device totals (§6.1a).

## 10. Left to the builder

- Module layout inside the fork, data structure choices, string interning.
- Exact JSON field names and the paging token format.
- Whether the MCP server uses the official Python `mcp` SDK (FastMCP) or a
  similar library.
- The exact HSPICE built-in function list (take it from the HSPICE manual and
  confirm each one with a golden deck).
- How to bring in upstream changes later (merge or rebase), if any appear.
