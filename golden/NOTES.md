# HSPICE golden corpus: findings

Labels such as `Task 8`, `6a-P` and `8b-G` name the steps of the build plan of
the query tool. The plan is no longer in the tree. To read it, run
`git show 214b481:docs/superpowers/plans/2026-10-31-llm-netlist-query.md`.

Oracle: HSPICE P-2019.06-SP1-1 (win64) on the Parallels VM "Windows 11",
runs from 2026-10-31 to 2026-11-01. Each `cases/<case>/` holds `deck.sp`
(+ aux files), the exact HSPICE listing `hspice.lis` (CR stripped), and
`expected.json`.

Regenerate one case (about 31 s; the license checkout takes 30 s, so run
cases in parallel, e.g. `ls -d cases/*/ | xargs -P 10 -n 1 tools/run_hspice.sh`):

```
golden/tools/run_hspice.sh golden/cases/<case>
python3 golden/tools/lis2json.py golden/cases/<case>/hspice.lis <case> -o golden/cases/<case>/expected.json
python3 golden/tools/lis2json.py --selftest
```

`-o` keeps the hand-written `extra` keys. The keys `alter_runs`,
`after_end_runs`, `rejected`, `merged_listing`, `dspf_summary` and
`unnamed_circuit_resistors` are recomputed from the listing.
`unnamed_circuit_resistors` holds the resistors of a circuit that the
directory names `.` (a later instance with the full name of an earlier one,
`hier_dotted_collision`): they have no path. `after_end_runs` holds the runs of a deck after
the first `.end` (`after_end_second_deck`). The listing of such a job looks the
same as an `.alter` run, so `lis2json` counts the `.alter` lines of the first
deck in `deck.sp` to tell them apart.
`lis2json` also reads the deck files next to the listing, to restore element
names that the listing shortens (see `long_names`). The hand-written key
`skip_value_check` lists the resistor paths whose listed value is not the deck
value (temperature-adjusted, `res_tc_positional`), or is a value that the
netlist alone does not give (an output variable, `rcl_output_var`; a name
defined in a `.prot` region, `prot_param`). Value tests skip these paths.
`--selftest` checks that each of them is in `resistors`.

`run_hspice.sh` exits non-zero and keeps the old `hspice.lis` when the run
call fails (exit 3: prlctl error, timeout, failed upload) or when the fetched
listing is not complete or is older than the run (exit 1: its `Start time:` is
before the VM time printed just before HSPICE started). A deck that HSPICE
aborts is a result (exit 0). `tools/test_run_hspice.sh` tests these checks
offline with a stub `prlctl`. `lis2json.py --selftest` prints one `FAIL` line
per problem and checks every case even when one case raises an error.

## Findings (one line each)

- `parhier`: `local` gives 3, `global` gives 4, no option gives 4, so the default is GLOBAL (`parhier_local`, `parhier_global`, `parhier_default`).
- `nested_global`: the override on the top `X` line wins at every level below (r6=3, r9=4). A higher-level header default beats an inner `X` override (r7=2). A higher-level local `.param` beats an inner `X` override (r8=6). Override text `u='w*5'` uses the top-level `.param w=9` (r10=45).
- `nested_local`: the innermost definition wins: inner `X` override (r6=5, r7=5, r8=7, r9=8). Override text `u='w*5'` gives 15 with the parent instance's `w=3` (r10=15). `in10` defines no `w`, so this case cannot tell the parent's scope from the new instance's scope: `ovr_scope_local` shows that HSPICE uses the new instance's scope.
- `inherit`: a name the child does not define comes from the parent instance scope (override `kk=3`, else parent default `jj=6`). Under the default (global), the top-level `.param k=4` beats the parent's `k=3`. Under `parhier=local` (`inherit_local`), the parent's `k=3` wins.
- `dup_param`: the LAST definition wins, with no warning: on separate lines (a=2), on one line (c=3), inside a subckt (b=6), and also a definition after the use (`.param d=8`, `rd 1 0 d`, later `.param d=7` gives 7).
- `dup_subckt`: REJECTED. HSPICE aborts with `**error** (deck.sp:5) duplicated subckt definition 'inv'`. No definition "wins".
- `nested_def`: a `.subckt` inside a `.subckt` is allowed, also with the same name as a top-level subckt (no duplicate error). An `X` line inside `outer` uses `outer`'s nested `cell` (x1.xc.rc=2). An `X` line inside `deep`, which is nested in `outer` and instantiated inside `outer`, also uses `outer`'s nested `cell` (x1.xd.xc.rc=2). A top-level `X` uses the top-level `cell` (x2.rc=1). This case cannot tell lookup by text nesting (lexical) from lookup by instance path (dynamic): both rules give these values. `nested_def_dynamic` and `nested_def_reach` can.
- `nested_def_dynamic`: HSPICE finds the definition of an `X` line along the INSTANCE PATH (dynamic lookup), not along the text nesting. The top-level `deep2` holds `xc a b cell`. Under the top-level instance `x2`, it uses the top-level `cell` (x2.xc.rc=1). Under `x1` (an `outer` instance), it uses `outer`'s nested `cell` (x1.xd.xc.rc=2), also two levels down through the top-level `mid` (x1.xm.xd.xc.rc=2). So one `X` line can bind to different definitions on different instance paths. The directory prints `cell` for both definitions.
- `nested_def_reach`: a top-level def can call a name that has no top-level definition, when its instance is inside an instance that nests that name (`t` calls `only`, nested in `outer`: x1.xt.xn.ro=5). The nearest instance ancestor wins over the text parent: `deep` is written inside `outer`, but an instance of the top-level `u` instantiates it. `u` and `outer` both nest a `cell`. `deep`'s `xc` uses `u`'s `cell` (x1.xu.xd.xc.rc=3), not `outer`'s (2).
- `X` lookup rule (from `nested_def`, `nested_def_dynamic`, `nested_def_reach`, `hier_shadow_own`): for an `X` line in the definition of instance P, look in the nested definitions of P's definition, then in those of the definition of P's parent instance, and so on up the instance path, then at the top level.
- `hier_shadow_own`: a name nested in the definition of instance P and in the definition of an instance above P: an `X` line in P's definition uses the one in P's own definition (`mid` nests `cell` with rc=3, `outer` nests `cell` with rc=2: x1.xm.xc.rc=3, and x2.xc.rc=3). The nearest instance ancestor wins over a farther one: `deep` under `mid2` under `outer` uses `mid2`'s `cell` (x1.xn.xd.xc.rc=4), and `deep` right under `outer` uses `outer`'s (x1.xd.xc.rc=2).
- `hier_same_name_nested`: an `X` line can call the name of a definition on its own instance path when a nested definition of that name is visible, with no error: `cell`'s `xc` calls `cell` and binds to `cell>cell` (x1.xc.rin=2), and `wrap` under `top2` calls `top2` and binds to `top2>top2` (x2.xw.xt.rt2=4). 4 resistors, 5 instances.
- `hier_reentry`: REJECTED. HSPICE aborts when a definition is instantiated inside an instance of itself on one instance path, also when the inner instance binds its `X` lines to other definitions, so that the expansion would end. `aa` calls `bb`, `bb` (which nests a `bb`) calls `aa`, and the inner `aa`'s `xb` binds to `bb>bb`, a leaf: `**error**  subcircuit aa               is defined recursively` (no line number). With `hier_same_name_nested`, the rule is "one definition twice on one instance path", not "one name twice" (an inference from these probes).
- `hier_reentry_unscoped`: REJECTED in the same way when the definition that comes back has no nested definitions: `a` -> `s` -> `a` -> `s>s` gives `**error**  subcircuit a                is defined recursively`.
- `hier_top_nested_only`: REJECTED. A top-level `X` line that calls a name defined only nested in `outer` aborts: `**error** (deck.sp:11) Definition of model/subckt "only" is not found for the element "x2".`
- `hier_unres_in_subckt`: REJECTED. An `X` line in a subckt with no visible definition on one of the subckt's instance paths aborts, also when it resolves on another path: `xn` in `t` resolves under `x1.xt` (`outer>only`) but not under `x2`, and HSPICE gives `**error** (deck.sp:4) Definition of model/subckt "only" is not found for the element "x2.xn".`
- `hier_unused_cycle_unres`: HSPICE runs a deck with an unused definition that calls itself (`loop`) and an unused definition that calls an undefined name (`orphan` calls `nosuch`), with no warning and no error (1 resistor, 1 instance). So HSPICE checks `X` lines only on instance paths.
- `hier_unused_caller_reach`: an unused definition (`spare`) that calls `t`, whose `X` line resolves only under an `outer` instance, does not make HSPICE abort. The deck runs (x1.xt.xn.ro=5, 3 instances).
- `hier_unused_nested_unres`, `hier_unused_nested_cycle`: a used `outer` nests a definition that no line calls: `helper`, which calls an undefined name (`xq a b nosuch`), or `loop`, which calls itself. HSPICE runs both decks with no warning (x1.ro=5, 1 resistor, 1 instance). So HSPICE does not check the `X` lines of an unused nested definition either.
- `hier_dup_nested`: REJECTED. Two `.subckt cell` inside one `outer` abort at the second: `**error** (deck.sp:6) duplicated subckt definition 'cell'`, as two top-level definitions do (`dup_subckt`).
- `hier_m_zero`, `hier_m_negative`: REJECTED. An `X` line with `m=0` or `m=-1` aborts: `**error** (deck.sp:6) Multiplication factor(M) for the element x1.x1 cannot be < or = zero. Enter a valid M value.` In `hier_m_zero`, HSPICE reports only this line, not the top-level `r3 2 0 2k m=0` of the same deck (line 9).
- `elem_m_zero`, `elem_m_negative`: REJECTED. A resistor with `m=0`, `m=-1` or `m=-0.5`, at the top level and inside a used subckt, aborts with one error for each element, the top-level one first: `**error** (deck.sp:8) Multiplication factor(M) for the element r3 cannot be < or = zero` and `**error** (deck.sp:3) ... for the element x1.rc ...`. For `m=-1`, HSPICE also warns `r3 defined in subckt 0 resistance is negative`.
- `elem_m_zero_unused`: HSPICE runs a deck whose unused subckt holds `rs a b 2k m=0` and `xu a b leaf m=0`, with no warning (x1.rl=1k, 1 resistor, 1 instance). So HSPICE checks `m` only on instance paths.
- `hier_empty_kinds`: the `Circuit Statistics` count each element of each expanded instance once, for every element type: an empty subckt, or one with only a `.param`, adds instances and no elements, and the `R`, `L`, `K`, `C`, `D`, `E`, `G`, `V`, `I`, `F`, `H` elements of `kinds` count once in each of its 2 instances (one has `m=2`): 4 inductors, 2 mutual inductors, 3 voltage sources (with the top-level `v1`), 25 resistors.
- `hier_dotted_names`: an instance name can hold a `.`, so an instance path is not split at each `.`. The top level has `x0.aaa` (cell), `x0` (wrap) and `x0.xm` (cell3); `wrap` holds `xq` (cell) and `xm.n` (cell3). HSPICE runs the deck and lists `x0.aaa`, `x0`, `x0.xm`, `x0.xq` and `x0.xm.n` (cell3, x0.xm.n.rc=3, x0.xm.rc=3; 10 resistors, 5 instances). So `x0.xm.n.rc` is `x0`, then `xm.n`, then `rc`, though the top level has an instance `x0.xm`.
- `hier_dotted_collision`: two instances get one full name: the top-level `x0.xq` (cell3) and wrap's `xq` (cell) under `x0`. HSPICE runs the deck and gives `x0.xq` to the top-level instance, which it creates first (level 1): x0.xq.rc=3. It lists wrap's `xq` as circuit `.`; its `rc` (2) has no path (6 resistors, 3 instances). `lis2json` puts the resistors of a circuit `.` in `extra.unnamed_circuit_resistors`, not in `resistors`.
- `hier_dotted_collision_deep`: the directory lists `x0.xa` (mid) before `x0` (wrap), but wrap's `xa.xb.xc` (leafb, level 2) gets `x0.xa.xb.xc` before mid2's `xc` under `x0.xa.xb` (leafa, level 3, circuit `.`): x0.xa.xb.xc.rl=3. So the instance that HSPICE creates first gets the name, not the longer name at the first level where two readings differ.
- `hier_dotted_collision_order`, `hier_dotted_collision_order_rev`: two level-2 instances get `x0.xa.xb`: wrap's `xa.xb` (cellb) under `x0`, and mid's `xb` (cella) under `x0.xa`. The one whose parent comes first gets it: with the `x0` line first, x0.xa.xb.rc=3 (cellb); with the `x0.xa` line first (`_rev`), x0.xa.xb.rc=2 (cella). The other is circuit `.`. With `hier_dotted_collision_deep`: HSPICE creates the instances breadth-first, level by level, in a level by the order of the parents, and in a parent by `X`-line order.
- `hier_dotted_collision_below`: the instances below the later of two instances with one full name still get full names from that name. mid's `xb` (cella) loses `x0.xa.xb` to wrap's `xa.xb` (cellb), but its child `xk` (leafk) is `x0.xa.xb.xk`: `.print dc i(x0.xa.xb.xk.rk)` gives 117.6471m (leafk's `rk`), `i(x0.xa.xb.rc)` gives 250.0000m (cellb's `rc`, not cella's), and `i(x0.xa.xb.xk.rc)` gives `**warning** (deck.sp:24) Could not find branch element .rc ;branch output ignored`. The directory prints the names of the circuits from the later instance on shifted by one: circuit 4 (cella) as `x0.xa.xb.xk.`, circuit 5 (leafk) as `.`. So with such a collision, the names that the directory and the `.op` listing print can belong to another circuit (an inference for the `.op` listing, which labels elements by circuit number); this case has no `.op` listing.
- `elem_dotted_name_collision`, `elem_dotted_name_collision_rev`: two resistors have the full path `x0.ra.rb`: the device `rb` (2 ohm) of the top-level instance `x0.ra` (cell) and the device `ra.rb` (3 ohm) of `x0` (wrap). In both line orders (wrap is circuit 2 in the first case and circuit 1 in `_rev`), `.print dc i(x0.ra.rb)` gives 333.3333m, the current of cell's `rb` in series with `rt1` (1 ohm); wrap's `ra.rb` would give 250m. So HSPICE finds an element by splitting its path at the last `.`: the circuit with the full name `x0.ra`, then its `rb`, whichever circuit was created first. The `.op` listing labels them `1:rb`, `2:ra.rb` (`1:ra.rb`, `2:rb` in `_rev`).
- `hier_dotted_collision_hidden`: as `hier_dotted_collision_order` (wrap's `xa.xb`, cellb, gets `x0.xa.xb`; mid's `xb`, cella, is circuit `.`), but only the later instance holds an `rz`: `.print dc i(x0.xa.xb.rz)` gives `**warning** (deck.sp:21) Could not find branch element x0.xa.xb.rz ;branch output ignored`, and `i(x0.xa.xb.rc)` gives 250.0000m (cellb's `rc`). So a device that only the later instance with a full name holds has no path.
- `hier_dotted_collision_shift`: as `hier_dotted_collision_below`, with one more top-level instance `x9` (other: `xp` to leafp, `ro`) after the colliding ones. The directory lists circuit 5 (cella, the later `x0.xa.xb`) as `x9.xp.`, circuit 6 (leafp) as `x0.xa.xb.xk.` and circuit 7 (leafk) as `.`: the names of circuits 5 to 7 are shifted by one. `.print` finds the elements by their own paths, but the headers show shifted names: `i(x0.xa.xb.xk.rk)` = 117.6471m (leafk's `rk`, header `.rk`), `i(x0.xa.xb.rc)` = 250.0000m (cellb's `rc`), `i(x9.xp.rp)` = 115.7895m (leafp's `rp`, header `x0.xa.xb.xk.rp`), `i(x9.ro)` = 73.6842m. `lis2json` maps the shifted names back and lists the circuit in `extra.collided_circuits`.
- `hier_dotted_collision_xline_order`: two level-2 instances get `x0.xa.xb`: wrap's second `X` line `xa.xb` (cellb; its first line is `xp`, filler) and mid's first line `xb` (cella) under `x0.xa`. With the `x0` line first, `i(x0.xa.xb.rc)` gives 225.8065m, the current of cellb's `rc` (3 ohm) beside `xp`'s `rf` (7 ohm), and cella's `rc` is circuit 5 (`.`). So the order is the parent first, then the `X`-line order in it: a later line of an earlier parent comes before the first line of a later parent.
- `node_dotted_collision` (bug-hunt probe): a top-level node written `x1.n` and the node `n` of instance `x1` (cell) have one full name, and HSPICE joins them into one node. The `.tran` node table lists only `0:in` and `1:n` (5.0000, the voltage of `v2` on the top-level `x1.n`), and `.meas tran vx find v(x1.n)` gives 5. So the cell's `r1`, `r2` and the top-level `r3` are on one node.
- `node_dotted_in_subckt` (bug-hunt probe): `v(x1.n.1)` finds the node `n.1` of instance `x1` (1 V, the divider of 2 V), and the deck has no instance `x1.n`. So HSPICE does not split a node's full name only at its last `.`, as it does for an element path (`elem_dotted_name_collision`).
- `node_dotted_two_splits_a`, `node_dotted_two_splits_b` (bug-hunt probes): the node `1` of instance `x1.n` (cella) and the node `n.1` of instance `x1` (cellb) have one full name `x1.n.1`. In both `X`-line orders `v(x1.n.1)` gives 2.4 V, and the node table has one node (`2:1` = 2.4000). Apart, the nodes would be at 2 V and 3 V. Joined, 4 V feeds the node through two 1k resistors and the node goes to ground through 1k and 3k: 8/(1+1+1+1/3) = 2.4. So HSPICE joins the nodes that have one full name, whichever instances they come from.
- `prot_subckt` (bug-hunt probe): an `X` line that calls a subckt defined inside `.prot` / `.unprot` runs with no error (job concluded). The listing does not show the protected `r1`; its statistics count 2 resistors. So an `X` line into a protected definition is not a rejected deck: the loader, which does not read the region, gives the Warning `unresolved_subckt` and says that a `.prot` region can define the subckt. It counts 1 resistor, and the summary note says that the totals are a lower bound (`golden_totals_all_cases` checks the note).
- `hier_dup_instance`: REJECTED. Two top-level `X` lines named `x1` abort: `**error**  above line attempts to redefine x1`, then `**error** (deck.sp:17) difficulty in reading input` (line 17 is the second `x1`). HSPICE reports only this pair, not the two `xq` lines of `wrap` in the same deck.
- `hier_dup_instance_sub`: REJECTED. Two `X` lines named `xq` in a used subckt abort: `**error**  above line attempts to redefine xq` (no line number).
- `elem_dup_name`: REJECTED. Two top-level resistors named `r1` abort: `**error** attempts to redefine r1 at line 0 and line 1` (these numbers are not file lines).
- `elem_dup_name_sub`: REJECTED. Two resistors named `rc` in a used subckt abort: `**error**  above line attempts to redefine rc`, then `**error** (deck.sp:4) difficulty in reading input` (line 4 is the second `rc`).
- `hier_dup_instance_unused`, `elem_dup_name_unused`: HSPICE runs a deck whose unused subckt holds two `X` lines named `xq`, or two resistors named `rs`, with no warning (x1.rc=2, 1 resistor, 1 instance). So HSPICE checks element names only on instance paths, as it checks `m` (`elem_m_zero_unused`).
- `hier_port_more`, `hier_port_fewer`: REJECTED. An `X` line whose node count is not the port count of the subckt it calls aborts, with more nodes (the top-level `x1` with 3 nodes, `cell` with 2 ports) and with fewer (`xc` with 2 nodes in the used `wrap`, `cell` with 3 ports): `**error** (deck.sp:6) Number of nodes mismatch between instance "x1" and subcircuit "cell". Subcircuit definition has 2 node(s) whereas subckt instance has 3 node(s).`, and the same at deck.sp:7 for `xc`.
- `hier_port_unused`: HSPICE runs a deck whose unused subckt holds an `X` line with 3 nodes and one with 1 node that call `cell` (2 ports), with no warning (x1.rc=2, 2 resistors, 1 instance). So HSPICE checks the node count only on instance paths, as it checks `m` (`elem_m_zero_unused`).
- `hier_port_dynamic`: REJECTED. One `X` line binds to two definitions on two instance paths: `t`'s `xn` calls `only`, which `outer` nests with 2 ports and `outer2` nests with 3 ports. Under `x1` (outer) the line matches; under `x2` (outer2) it does not, and HSPICE aborts: `**error** (deck.sp:3) Number of nodes mismatch between instance "xn" and subcircuit "only". Subcircuit definition has 3 node(s) whereas subckt instance has 2 node(s).` So HSPICE checks the node count for each binding of the line.
- `hier_port_forms`: HSPICE runs a deck whose `X` lines match the port count in other written forms: `params:` in a `.subckt` header is read as a port (`cell a b params: w=1` has 3 ports, and `x1 1 t1 0 cell w=2` gives x1.rc=2), a header default with no `params:` is not a port (`cell2 a b w=1`, 2 ports), an `X` line with an `m=` and with a `$` comment, a header and an `X` line that go on on a `+` line (`cell3`), and `.macro` (x5.rm=4). 11 resistors, 5 instances, x2.rc=1.5 (`w=3 m=2`).
- `hier_port_params_colon`: REJECTED. The same deck with `x1 1 t1 cell w=2` (2 nodes): `**error** (deck.sp:17) Number of nodes mismatch between instance "x1" and subcircuit "cell". Subcircuit definition has 3 node(s) whereas subckt instance has 2 node(s).` So `params:` counts as a port in P-2019.06.
- `hier_model_callee`: REJECTED. An `X` line that calls the name of a model card (`.model rmod r`, no `.hdl` line) aborts: `**error** (deck.sp:4) Definition of model/subckt "rmod" is not found for the element "x1".`
- `long_names`: the `.op` listing shortens names. The `element` row keeps 8 characters of the element name (`rverylongtoplevel` gives `0:rverylon`; also with a 2-digit circuit number: `11:rlongnam`, and with a 3-digit one in `long_defs_mult`: `104:rverylon`). The `subckt` row keeps 10 characters of the instance path (`xtop_instance_name_very_long_123456789.xlonginstname_abcdef` gives `xtop_insta`; `x1.xm.xd.xc` in `nested_def_dynamic` gives `x1.xm.xd.x`). The circuit name directory prints full paths. A path longer than its column is alone on its line, and the definition and multiplier follow on the next line.
- `long_names`: `lis2json` takes the full instance path from the directory. It restores each 8-character element name from the deck files of the case: the one element name that starts with it and belongs to the instance's definition (`0:rverylon` gives `rverylongtoplevel`, `12:rverylon` in `xlongcellname` gives `rverylongelementname_abc`). When no deck name or more than one deck name fits, it stops with an error. `expected.json` always holds full names.
- `long_defs_mult`: the directory prints the multiplier with an engineering suffix: `m=1000` gives `1.00k`, `m=0.5` gives `500.00m`, `m=1meg` gives `1.00x`, `m=2k` gives `2.00k`. A definition name that is too long for its column (32 characters) stays on the line, and the multiplier goes alone onto the next line. When the path is also too long, the entry takes 3 lines: number and path, then the definition, then the multiplier. The `.op` listing prints R/m for these instances too (1k with `m=1000` gives 1.0000, with `m=0.5` 2.0000k, with `m=1meg` 1.0000m). `lis2json` reads all these forms and stops with an error on any other line inside the directory.
- `units`: `1m`=1e-3, `1M`=1e-3, `1meg`=`1MEG`=`1x`=1e6, `1mil`=25.4e-6, `2.5k`=2500, `10pF`=1e-11, `1V`=1, `1a`=1e-18, `1u`=1e-6, `1n`=1e-9, `1p`=1e-12, `1f`=1e-15, `1t`=1e12, `1g`=1e9.
- `units`: `1me`=1e6 and `1mi`=25.4e-6. So `ME` is mega and `MI` is mil, as prefixes. Plain `M` is milli.
- `units`: `1e-3k` = 0.001. HSPICE ignores a scale suffix after an exponent and warns: `nonstandard number 1e-3k is interpreted as 0.001`.
- `units2`: time units: `1min`=60, `1hr`=3600, `1day`=86400, `1yr`=31536000 (365 days). `min` is read before the `mi` (mil) prefix: `1min`=60, but `1mi`=`1mil`=25.4e-6.
- `units2`: `db` converts decibels to a ratio, 10^(x/20): `20db`=10, `6db`=1.9953.
- `units2`: `z` is zepto: `1z`=1e-21 (through a `.param`).
- `units2`: letters after a scale prefix are ignored, with no warning: `1meter`=1e6 (`ME` = mega), `1ms`=`1mv`=`1mohm`=1e-3.
- `units2`: a resistor of 1e-21 ohm directly across the voltage source is dropped from the `.op` listing (`Recovered resistors ... to avoid V loop`), but `# resistors` still counts it. Seen in a probe deck, so `units2` reads `1z` through a `.param`.
- `ops`: `**` is LEFT-associative: `2**3**2` = 64 (not 512).
- `ops`: `-2**2+10` = 6, so unary minus binds looser than `**`. `2*3**2` = 18. `2*-3+10` = 4. `+3` = 3. `8/2/2` = 2. `10-3-2` = 5.
- `ops`: `**` takes fractional and negative-base operands: `2**0.5` = 1.4142, `(-2)**3` = -8.
- `ops`: comparisons give 1/0: `<`, `<=`, `>`, `>=`, `==`, `!=` all work. `&&` binds tighter than `||` (`1||0&&0` = 1).
- `ops`: the ternary works and is right-associative (`1?2:0?4:5` = 2). It binds looser than comparison (`1+1>1?5:6` = 5).
- `ops_caret`: `^` = power (yes), the same as `**`: `2^3`=8, left-associative `2^3^2`=64, `-2^2+10`=6, `2*3^2`=18.
- `ops_not`: unary `!` works: `!0`=1, `!3`=0.
- `ops2`: a unary minus right after `**` or `^` takes the next operand only, and the power stays left-associative: `2**-3**2` = `2^-3^2` = (2**-3)**2 = 0.015625.
- `ops2`: a zero base gives 0 for every exponent, with no error and no warning: `0**0` = `0^0` = 0 (not 1), `0**-1` = 0 (not infinity), `0**0.5` = 0. No `ops2` expression aborts, so `ops2` is one deck.
- `funcs`: all accepted: `sqrt pow pwr log log10 exp abs min max int sgn sign` and a two-argument `.param f(a,b)='a*b+1'` (f(2,3)=7).
- `funcs`: `pow(x,y)` TRUNCATES y to an integer: `pow(2,0.5)`=1, `pow(2,2.7)`=4, `pow(-2,3)`=-8.
- `funcs`: `pwr(x,y)` = sign(x)*|x|^y: `pwr(-2,2)`=-4, `pwr(-2,0.5)`=-1.4142.
- `funcs`: `log` is natural (`log(100)`=4.6052), `log10(100)`=2, `int` truncates (`int(-2.7)`=-2), `sgn(-3)`=-1, `sign(3,-1)`=-3.
- `funcs_neg`: negative arguments keep the sign: `sqrt(-4)`=-2, `log(-100)`=-4.6052, `log10(-100)`=-2 (no error).
- `funcs_extra`: also accepted: `nint(2.5)`=3, `floor(2.7)`=2, `ceil(2.2)`=3, `sin cos tan asin acos atan sinh cosh tanh`, `db(10)`=20.
- `stat_nominal`: in `.param`, `agauss(1k,0.1k,3)`, `gauss(2k,0.1,3)`, `aunif(3k,0.1k)` and `unif(4k,0.1)` give their nominal values (1k, 2k, 3k, 4k) in `.op`.
- `stat_direct`: REJECTED. `agauss` used directly in an element expression aborts: `"agauss" function cannot be defined in expressions directly`.
- `f_limit` (run on 2026-11-07, not in `golden/cases`): a statistical function in quotes in a `.param` value stops too. `.param x='limit(1,0.1)'` aborts with `**error** (deck.sp:3) "limit" function cannot be defined in expressions directly. Please pre-define a parameter for it , then use the parameter in expressions.` The bare form `.param x=limit(1,0.1)` gives 1 (`f_limit_bare`), as in `stat_nominal`. The model selector reads 1 for both forms and gives no message. Only `limit` was run in quotes.
- `temper`: with `.temp 50`, `temper` = 50 in an element expression and in a `.param` (150). With no `.temp` (`temper_default`), it is 25 (125).
- `lib_corner`: `.lib 'models.lib' tt` gives tt values (rcorner=1k). `lib_corner_ff` gives ff (2k). A `.lib 'models.lib' base` call inside a section works (rbase=100).
- `include_in_subckt`: lines from `.include` inside a `.subckt` belong to that subckt (x1.ri, x2.ri). They see the instance parameters (x1.rj=6, x2.rj=10).
- `alter`: the first run is the deck without the `.alter` blocks. `.alter` changes ACCUMULATE: run 3 keeps run 2's `p=3k` (r1=3k) and adds `r2=5k`.
- `title_line`: line 1 of the top file is always the title. An element there (`R1 1 0 1`) is ignored (1 resistor in the statistics).
- `title_dot`: line 1 is ignored even when it is a dot command. `.option parhier=local` on line 1 has no effect (x1.r1=4, not 3). HSPICE prints it as the title.
- `title_plus`: REJECTED. A `+` line right after line 1 aborts HSPICE: `**error** (deck.sp:2) unable to open file "r1 1 0 1"` (the text of line 1). The `+` line does not join the title, and HSPICE does not ignore it.
- `title_plus_words`, `title_plus_comment`: REJECTED. In all three probes (line 1 = element, plain words, `*` comment), HSPICE aborts the same way.
- `case_mix`: keywords, subckt names, instance names and parameter names are case-insensitive (`.SUBCKT Foo`/`.Ends Foo`, `fOO`, `WV`/`wV`/`Wv`, `PQ`/`pQ`/`Pq`).
- `hier_counts`: the circuit name directory lists every expanded instance once. An `m=2` instance and its children are listed once with multiplier 2.00 (instance counts per definition: blk 2, mid 5, leaf 17, unused 0).
- `hier_counts`: `# resistors` counts each expanded element once, not weighted by `m` (48). The `.op` listing prints the effective R/m for elements under an `m=2` instance (`x1.xm2.rmk_mid` = 1000 for a 2k element). `expected.json` marks these entries with `"m": 2.0`, and the element's own value is `value * m`.
- Merging: when identical subckt instances (same definition, same parameters) connect to the same nodes, HSPICE merges them in the `.op` listing. It prints one column with the parallel value (e.g. `x1.rmk_blk` 1.5k for two 3k), while `# resistors` still counts all of them. All golden decks give each instance its own nodes, so no listing is merged (`lis2json` adds `extra.merged_listing` if one is).
- `ground_alias`: `0`, `gnd`, `gnd!` and `ground` are all ground, in any letter case (`GND`), also inside a subckt.
- `global_net`: `.global vdd` connects `vdd` inside a subckt that has no `vdd` port (x1.rl carries 1 mA). A name that is not global (`vss`) is a local node of the instance (x1.rv carries 0).
- Singular supply: a voltage source whose node has no other element gets an automatic 1 MEG resistor ("singular supplies were terminated"), and `# resistors` counts it. No golden deck has one now.
- `node_angle_names`: HSPICE accepts `<` and `>` in node names (`net<3>`, `a<0>`, `b>c`) and in `.subckt` ports (`d<1>`), as Virtuoso writes bus bits. The parser takes both as name characters (Task 9).
- `dspf_included`: HSPICE reads a DSPF as the netlist itself. It treats `*|` lines as comments and accepts `:` in node names (`A:1`, `MN1:G`). `# resistors` = 6 = 5 parasitic + `rload`, `# capacitors` = 3, `# mosfets` = 2.
- `dspf_backannot`: P-2019.06 accepts `.option sim_dspf="<scope> <file>"`. The scope can be a subckt name (`inv`) or an instance name (`xi`). Both match nets `XI/A`, `XI/Y`. `@inv` is rejected (`Wrong instance or subckt name "@INV"`). Without a scope, the DSPF nets are matched at the top level (`no match for net`).
- `dspf_backannot`: HSPICE ignores the DSPF's `.SUBCKT` line (warning `1st ".SUBCKT" statement is ignored`). It warns that `*|BUSBIT []` is not recognized.
- `dspf_backannot`: in a DC-only deck, every net is in the "power rail block". Nets expand only with `.option sim_dspf_rail=on`; without it they are skipped as `skipping power rail net`. With it: 2 nets expanded, 5 R and 3 C written, 5 new nets.
- `dspf_backannot`: the `sim_dspf` flow writes a different listing ("Advanced Analog Library"). It has no `.op` element listing and no circuit name directory. `expected.json` has `resistors: []`, totals from its `Netlist Statistics` (pre-layout: 2 resistors, 2 MOSFETs), `subckt_instances: null` and `extra.dspf_summary`.
- `elem_dup_kw`: the LAST `r=` wins, with no warning (`R1 1 0 r=1k r=2k` gives 2k). A keyword value after a positional value wins (`R2 2 0 1k r=3k` gives 3k).
- `elem_dup_kw_order`: REJECTED. A bare resistor value right after the `r=` value is a syntax error (`R3 3 0 r=3k 1k`: `syntax error at or before "1k"`). This is the only probed form with the bare value after `r=`. It gives no value, so it cannot tell "the keyword wins" from "the last value wins". The only accepted probed form (`R2 2 0 1k r=3k` in `elem_dup_kw`) has the keyword last, so both rules give 3k there.
- `res_model`: HSPICE accepts a model name and a value together. With a model that has no scaling parameters (`.model rmod r`), the value is the resistance: `R1 1 0 rmod 2k` gives 2k, `R2 2 0 rmod r=3k` gives 3k.
- `mos_positional`: the two numbers after the model name are L, then W: `M1 d g s b nch 2u 1u` gives L=2u, W=1u, the same as `l=2u w=1u`. `.option list` prints them (`l drawn`, `w drawn` in the element summary). The `.op` listing does not print L or W, but its `beta` (kp*W/L = 50u) agrees.
- `mos_positional_all`: 8 numbers after the model name are `l w ad as pd ps nrd nrs`, in that order (each equals the keyword form). `.option list` shows them: `cdsat`/`cssat` = AD/AS when `js=1`, PD/PS when `jsw=1`, and `rd eff`/`rs eff` = NRD/NRS when `rsh=1`.
- `macro_eom`: `.macro`/`.eom` work the same as `.subckt`/`.ends`. An `X` line instantiates the definition, the header default and the `X` override work (x1.r1=3, x2.r1=4 with `rv=4`), and the directory prints the definition name `inv`.
- `ind_model_nolevel`: REJECTED. An `L` element that calls `.model lmod l` (no `level`) aborts: `L-model called by 0:l1 is not for L_element, its level must be 3`. An inductor model card needs `level=3`.
- `ind_value_model`, `ind_model_value`: with `.model lmod l level=3`, HSPICE accepts both orders and reads the same inductance. `L1 1 2 1n lmod` and `L1 1 2 lmod 1n` both give 1n (`.op` `l value`, `.option list` `ind eff`), node2 `2` and model `lmod`.
- `cap_model_value`: `C1 1 2 cmod 1p` (model, then value) gives 1p (`.option list` `cap eff`). The `.op` listing prints no capacitors.
- `cap_value_model`: `C1 1 2 1p cmod` (value, then model) does not abort, but HSPICE does not read `1p` as the value. `.option list` shows node2 `1p`, model `cmod` and `cap eff poly`, the `.op` node list has a node `1p`, and HSPICE warns `Capacitance value for element c1 is 0.000`. So for C, only "model, then value" gives the value. The listing does not show what HSPICE does with the token `2` of this line.
- `res_value_model`: REJECTED. `R1 1 0 2k rmod` (value, then model) aborts: `**error** (deck.sp:5) Element in r1 has three nodes defined,  element must have two nodes`. So HSPICE reads `2k` as a third node. For R, only "model, then value" is accepted (`res_model`).
- `res_tc_positional`: the bare numbers after a resistor value are TC1, then TC2. At `.temp 125` (tnom = 25, so dT = 100), `R1 1 0 1k 0.004` gives 1.4k, the same as `1k tc1=0.004`, and `R2 2 0 1k 0.004 1e-5` gives 1.5k, the same as `1k tc1=0.004 tc2=1e-5`. The `.op` `r value` is the value at the simulation temperature, R*(1 + TC1*dT + TC2*dT^2). `extra.skip_value_check` lists r1 to r4.
- `mos_digit_model`, `mos_digit_model_33`: REJECTED. A model name that starts with a digit aborts at the `.model` line: `Model name must start with a letter, syntax error at or before "5vnmos"` (also `"33nmos"`). So HSPICE accepts no `.model` card with such a name.
- `mos_digit_model_ref`: REJECTED. An `M` line with `5vnmos` in the model place (and no `.model 5vnmos` card) aborts with a syntax error at or before `w`, the token after `5vnmos`: `**error** (deck.sp:8) syntax error at or before "w" "1u" "l" "=" "1u"`, then the `M` syntax reference. It is not a "model not found" error. The listing does not show whether HSPICE reads `5vnmos` as a number or as a node.
- `mos_suffix_letters`: in the positional L place, HSPICE ignores the letters after a scale suffix, with no warning: `M1 d g s b nch 2umos 1u` gives L=2u, W=1u (`.option list` `l drawn` 2.0000u, `w drawn` 1.0000u; `.op` `beta` 50.0000u, the same as the `l=2u w=1u` control). This agrees with `units2` (`1mohm` = 1e-3 as a resistor value). Only `2umos` in the L place was probed.
- `mos_kw_and_positional`: in `M1 d g s b nch 2u 1u l=3u`, the keyword after the positional L wins: L=3u, W=1u (`.option list` `l drawn`, `w drawn`; `.op` `beta` 33.3333u, the same as the `l=3u w=1u` control). HSPICE warns `Duplicate parameter l is set. The last one overrides previous ones.` So the last value wins, as in `elem_dup_kw`.
- `diode_area`: the bare numbers after the diode model name are AREA, then PJ. At 0.6 V with `.model dmod d is=1e-14 jsw=1e-8`, `D1 1 0 dmod 2` carries 2 times the current of `D2 2 0 dmod` (277.5999u, 138.7999u), the same as `area=2`. `D3 3 0 dmod 2 1e-6` carries 3 times (416.3998u = AREA*IS + PJ*JSW), the same as `area=2 pj=1e-6`.
- `bjt_area`: the bare number after the BJT model name is AREA. `Q1 c b e qmod 2` gives ic 136.0873u, 2 times `Q2 c2 b e qmod` (68.0436u) and the same as `area=2`. HSPICE reads the 4th node token `qmod` as the model (model row `qmod`), not as a substrate node.
- `inc_twice`: REJECTED. HSPICE reads a file again at each `.include` of it, so a `.subckt` in a file included twice aborts: `**error** (sub.inc:2) duplicated subckt definition 'cell'`.
- `inc_twice_params`: REJECTED. A `.model` in a file included twice aborts: `**error** (params.inc:4) duplicated model definition 'rmod'`. HSPICE gives line 4. The `.model` is on line 3, the last line of the file.
- `inc_twice_param_only`: a file with only a `.param` can be included twice (no error, no warning). The second copy is read and is the last definition: `.include` (pi=1k), then `.param pi=5k`, then the same `.include` gives pi=1k. This needs `inc_once_param_only`: it shows that HSPICE reads the file in place, at the `.include`, so pi=1k comes from the second `.include`.
- `inc_once_param_only`: HSPICE reads an `.include` file in place, at the `.include`. `.include` (pi=1k), then `.param pi=5k` gives pi=5k (the later `.param` wins).
- `lib_twice`: REJECTED. HSPICE loads a `.lib` section again at each call, so the same `.lib 'm.lib' tt` call twice aborts when the section has a `.subckt`: `**error** (m.lib:3) duplicated subckt definition 'cell'`. The section's `.model` comes after the `.subckt`, so this case does not show the error for the `.model`.
- `lib_twice_param_only`: a section with only a `.param` can be called twice (no error, no warning). The second call is read and is the last definition (pl=1k, not the 5k between the two calls). So HSPICE does not ignore a repeated section call. This needs `lib_once_param_only`: it shows that HSPICE reads the section in place, at the call, so pl=1k comes from the second call.
- `lib_once_param_only`: HSPICE reads a `.lib` section in place, at the call. `.lib 'm.lib' tt` (pl=1k), then `.param pl=5k` gives pl=5k (the later `.param` wins).
- `after_end`: the lines after `.end` do not join the first deck, and HSPICE gives no warning. An element line and a `.param` after `.end` change nothing (r1=1k, not the 2k of the later `.param`; `# resistors` = 1), and the listing has one job only. These lines have no analysis command, so this probe cannot show whether HSPICE starts a second job for lines after `.end`. A complete second deck after `.end` runs as a second job (`after_end_second_deck`).
- `src_forms`: HSPICE accepts all these source forms (`.option list` and `.op` node voltages). `DC 1.2` gives 1.2. `DC 1 PULSE(0 1 0 1n 1n 5n 10n)` gives DC 1 and a pulse. `1.2 AC 1 0` gives DC 1.2, AC magnitude 1, AC phase 0. The phase 0 is also the default (`src_ac_default_phase`), so this form does not show that HSPICE reads the token after the AC magnitude as the phase (`src_ac_phase` shows it). `PULSE 0 1 0 1n 1n 5n 10n` without parentheses gives the same pulse as with parentheses. `DC=2` gives 2. `I1 0 6 DC 1m` drives 1 mA into node 6 (1 V across 1k). Every form here has the DC value first (or no DC value). `src_order_ac_first` and `src_order_fn_first` probe two other orders.
- `src_forms`: the `.op` value of a source with a function is its DC value when the line has one (V2: 1, not 0). A PULSE source with no DC value gives its initial value, the first PULSE argument (`PULSE 0 ...` gives 0, `PULSE 0.3 ...` gives 0.3). HSPICE says so for V4: `**info** (deck.sp:6) DC voltage reset to initial transient source value in source 0:v4. new dc=0.0000D+00`. It prints this info line for V4 only, not for V7 (0.3). This case probes PULSE only. `src_fn_nodc` probes PWL, SIN, EXP, SFFM and AM.
- `src_fn_nodc`: with no DC value, each of the six probed sources gives, in `.op`, the value of its function at t=0. This value is not always the first argument. `PWL(0 0.4 1n 1)` gives 0.4 (the first argument is the time 0). `PWL(1n 0.6 2n 1)` gives 0.6 (`.option list` adds the point (0, 0.6)). `SIN(0.2 1 1meg)` gives 0.2, `EXP(0.5 1 0 1n 1n 5n)` gives 0.5, `SFFM(0.1 1 1meg 2 1k)` gives 0.1. `AM(1 0.5 1k 1meg 0)` gives 0 (the first argument is 1). HSPICE prints the info line `DC voltage reset to initial transient source value` for the first source only (`0:v1. new dc=0.4000D+00`). In this case, the `new dc` of the info line and the `.option list` `dc volt` equal the `.op` value. They do not always: see `src_sin_t0`. `R` and `TD=` after the closing parenthesis of a PWL are probed in `pwl_repeat`, `pwl_repeat_td` and `pwl_td`. Not tested: `R` and `TD=` inside the parentheses.
- `src_ac_phase`: HSPICE reads the token after the AC magnitude as the AC phase. `V1 1 0 1.2 AC 1 45` gives DC 1.2, AC magnitude 1, phase 45. `V2 2 0 DC 0.5 AC 2 90` gives DC 0.5, AC magnitude 2, phase 90 (`.option list` `ac mag`, `ac phase`; `.op` node voltages 1.2 and 0.5).
- `src_order_ac_first`: AC before DC is accepted. `V1 1 0 AC 1 45 DC 0.7` gives DC 0.7, AC magnitude 1, phase 45 (`.option list`; `.op` node 1 = 0.7).
- `src_order_fn_first`: REJECTED. A transient function before `DC` aborts. `V1 1 0 PULSE(0 1 0 1n 1n 5n 10n) DC 0.7` gives `**warning** Too many parameters in power supply, ignored.`, then `**error** (deck.sp:3) Undefined parameter or function definition "dc" for v1`. So HSPICE reads the `dc` after the function as a parameter name, not as the DC keyword. `src_fn_dc_more` and `src_fn_dc_more_noparen` show the same abort after `SIN(...)` and after `PULSE` without parentheses. Not tested: AC before a function, a function before AC, a bare DC value after AC or after a function, and `AC=`.
- `param_braces`: REJECTED. `{...}` in an element value is a syntax error: `R1 1 0 {c}` gives `syntax error at or before "{c}"`.
- `param_braces_param`: REJECTED. `{...}` in a `.param` value is not an expression: `.param c={3*a}` aborts with `Undefined parameter or function definition "[3" for c` and the same for `"a]"`. So HSPICE P-2019.06 does not accept `{...}` as expression quotes.
- `title_plus_gap`: REJECTED in the same way as `title_plus`. A `*` comment line between line 1 and the `+` line does not prevent the abort: `**error** (deck.sp:3) unable to open file "title_plus_gap probe deck"` (the text of line 1).
- `nonutf8_comment`: a Latin-1 byte (0xE9, not valid UTF-8) in a `*` comment is accepted, with no warning (r1=2k). The listing does not show the comment.
- `brace_quoted_param`, `brace_quoted_elem`, `brace_quoted_fn`: REJECTED. `{...}` inside quotes aborts in all three places, as `{...}` without quotes does. HSPICE reads `{a}` inside the quotes as the name `[a]`. `.param a=2 p='{a}*2'` gives `**error** (deck.sp:2) Undefined parameter or function definition "[a]" for p`. `R1 1 0 '{a}'` (with `.param a=2k`) gives the same error for `r1`, and `V1 1 0 PWL(0 '{a}' 1n 1)` (with `.param a=0.5`) gives it for `v1`. Only `{a}` with a parameter name inside was probed.
- `brace_fn_unquoted`: REJECTED. `{...}` without quotes in a source-function argument is a syntax error, as in an element value (`param_braces`). `V1 1 0 PWL(0 {a} 1n 1)` (with `.param a=0.5`) gives `**error** (deck.sp:4) syntax error at or before "{a}" "1n" "1" ")"`, then the `V`/`I` syntax reference. Only PWL was probed.
- `pwl_repeat`, `pwl_repeat_td`, `pwl_td`: HSPICE accepts `R`, `R=0 TD=1n` and `TD=1n` after the closing parenthesis of `PWL(0 0.3 1n 1)`. `.option list` prints `repeat delay.` 0 for `R` and for `R=0`, and `delay time...` 1.0000n for `TD=1n`. The PWL points stay (0, 0.3) and (1n, 1). The `.op` value is 0.3 in all three cases (node 1; the info line says `new dc=0.3000D+00`). With `R` and with `R=0`, HSPICE warns `has discontinuity at the junction of repeated pwl source . A rise/fall time of 1e-15 second is added`. With `TD=1n`, the `.op` value 0.3 is the first PWL value. A waveform that holds its first value until the delay ends also gives 0.3 at t=0 (an inference), so these cases cannot tell the rule "first value" from the rule "value at t=0".
- `cl_tc_positional`: the bare numbers after a C or L value are TC1, then TC2, with or without a model name (`.option list` `tc1`, `tc2`, the same as the keyword forms `tc1=`, `tc2=`). `C1 1 0 cmod 1p 0.004` and `C2 1 0 1p 0.004` give tc1 4.0000m. `C3 1 0 cmod 1p 0.004 1e-5` and `C4 1 0 1p 0.004 1e-5` give tc1 4.0000m and tc2 10.0000u. For L, the model name comes after the value: `L1 1 11 1n lmod 0.004` (model `lmod`) and `L2 1 12 1n 0.004` give tc1 4.0000m, and `L3`, `L4` with `0.004 1e-5` add tc2 10.0000u. At `.temp 125` (dT = 100), the `.option list` temperature-adjusted values are 1.4p and 1.4n with TC1 only, and 1.5p and 1.5n with TC1 and TC2, the same as the keyword forms. The `.op` `l value` prints the same 1.4n and 1.5n (1.0000n for `L7` without TC). The element summary prints the deck value (`cap eff` 1.0000p, `ind eff` 1.0000n).
- `src_ac_default_phase`: an AC magnitude with no phase gives phase 0. `V1 1 0 DC 1 AC 1` gives DC 1, AC magnitude 1, AC phase 0 (`.option list`; `.op` node 1 = 1).
- `src_sin_t0`: with no DC value, the `.op` value of a SIN source includes its phase. `SIN(0.2 1 1meg 0 0 90)` gives 1.2 in `.op` (node 1 and the source `volts`). This is 0.2 + 1*sin(90 deg), the value of the SIN formula at t=0 (an inference: the listing shows only 1.2). For this source, `.option list` prints `dc volt` 200.0000m and the info line says `new dc=0.2000D+00`, so neither gives the `.op` value. `SIN(0.2 1 1meg 1n)` (delay 1n) gives 0.2. `.option list` reads the 4th, 5th and 6th arguments as delay, theta and phase. HSPICE prints the info line for the first source only.
- `src_fn_dc_more`, `src_fn_dc_more_noparen`: REJECTED, as `src_order_fn_first`. `V1 1 0 SIN(0 1 1meg) DC 0.7` aborts with `**error** (deck.sp:3) Undefined parameter or function definition "dc" for v1`, and no warning. `V1 1 0 PULSE 0 1 0 1n 1n 5n 10n DC 0.7` (no parentheses) gives `**warning** Too many parameters in power supply, ignored.`, then the same error.
- `after_end_second_deck`: a complete deck after `.end` (a title line, a source, a resistor, `.op`, `.end`) runs as a second job in the same listing. Each job has its own `job concluded` and its own job statistics. The second job prints the line after `.end` as its title. The first job has only the first deck's elements (r1 = 1k, `# resistors` = 1), and the second job only the second deck's (r2 = 2k, `# elements` = 2). `lis2json` puts the second job's resistors in `extra.after_end_runs`, not in `alter_runs`.
- `end_in_include`: HSPICE ignores a `.end` in an included file, with a warning, and reads on: `**warning** (inc.sp:2)'.end' can not exist in include/lib files, ignored`. The R line after that `.end` in the included file (r2 = 2k) and the R line after the `.include` in the top file (r3 = 3k) are both loaded (`# resistors` = 3). The warning names "include/lib files", but only an `.include` file was probed.
- `pwl_repeat_noparen`, `pwl_repeat_key_bare`, `pwl_ac_r` (close-out probes): HSPICE accepts each form, and `R` is the repeat flag in each. `V1 1 0 PWL 0 0.3 1n 1 R` (no parentheses): `.option list` prints `repeat delay.` 0 and the two PWL points (0, 0.3) and (1n, 1), so `R` is not a fifth PWL argument. `V1 1 0 PWL(0 0.3 1n 1) R=1n R`: `repeat delay.` 0, the repeat time of the bare `R`. Here 1n is the last time point, and `R=1n` alone at the last time point drops the repeat (`pwl_repeat_key_only`), so this case alone cannot tell "the last one wins" from "`R=1n` is dropped". `pwl_repeat_key_bare_mid` tells them apart: the last one wins. `V1 1 0 PWL(0 0.3 1n 1) AC 1 R`: `ac mag` 1.0000, `ac phase` 0. and `repeat delay.` 0, so `R` there is the repeat flag, not the AC phase. In all three, the `.op` value is 0.3 (node 1), with the same info line and the same warning as `pwl_repeat`.
- `brace_quoted_temp`, `brace_temp` (close-out probes): REJECTED. `{...}` in a `.temp` value aborts, as in a `.param` value and an element value. `.temp '{a}'` (with `.param a=50`) gives `**error** (deck.sp:3) Undefined parameter or function definition "[a]" for temper@_1`, and `.temp {a}` gives `**error** (deck.sp:3) syntax error at or before "{a}"`. Among the other analysis and output commands, only `.meas`, `.print` and `.probe` were probed (`brace_meas`, `brace_print`, `brace_probe`, `brace_probe_quoted`).
- `pwl_repeat_key_only`, `pwl_repeat_key_mid`, `pwl_repeat_key_bare_mid` (close-out probes): `V1 1 0 PWL(0 0.3 1n 1) R=1n` (`R=` at the last time point) prints no `repeat delay.` line, and HSPICE warns `**warning** (deck.sp:3) delay greater than piece-wise in v1  1.00000D-09`: it drops the repeat. `V1 1 0 PWL(0 0.3 1n 1 2n 0.5) R=1n` (before the last time point 2n) prints `repeat delay.` 1.0000n. `V1 1 0 PWL(0 0.3 1n 1 2n 0.5) R=1n R` prints `repeat delay.` 0, so the bare `R` after a valid `R=1n` wins: the last one wins, as for element keywords (`elem_dup_kw`). The PWL points stay as written, and the `.op` value is 0.3 (node 1) in all three.
- `pwl_r_param_last` (close-out probe): `.param r=2` / `V1 1 0 PWL 0 0.3 1n r` (no parentheses, a parameter named `r` as the last word): `.option list` prints `repeat delay.` 0 and the PWL points (0, 0.3) and (1n, 0.3). So `r` is the repeat flag, not the value 2 of the parameter, and the PWL keeps 3 values. HSPICE gives the last time point a value, with `**warning** Element 0:v1 is missing terminal voltage 0.30000 used by HSPICE.` This case cannot tell whether that value is the previous value or the first value (both are 0.3). The `.op` value is 0.3.
- `pulse_r_noparen` (close-out probe): REJECTED. `V1 1 0 PULSE 0 1 0 1n R` (no parentheses): HSPICE reads `R` as a fifth PULSE argument, the parameter name `r`, and aborts because no `.param r` exists: `**error** (deck.sp:3) Undefined parameter or function definition "r" for v1. Please enter a defined name.` So `R` is the repeat flag only after a PWL.
- `end_quoted_arg`, `end_kv_arg` (close-out probes): HSPICE ends the first deck at `.end 'x'` and at `.end x=1`, with no warning, as at `.end`: it does not read the text after the keyword. A complete deck after that line runs as a second job (r2 = 2k), and the first job has r1 only (`# resistors` = 1), as in `after_end_second_deck`. A first run of these probes had the line `r2 1 0 2k` right after the `.end` line: HSPICE read it as the title of a second deck, which aborted with `**error**  circuit 0 has no nodes`, and the first job had r1 only.
- `brace_probe`, `brace_print`, `brace_probe_quoted`, `brace_meas` (close-out probes): HSPICE ignores a `.probe` or `.print` output with `{...}`, with a warning, and runs the deck: `.probe v({a})` gives `**warning** (deck.sp:6) .probe is ignored because of syntax error at or before "{a}"`, `.print v({a})` the same for `.print`, and `.probe v('{a}')` gives `**warning** (deck.sp:6) Unable to find referenced node [a]; Output variable ignored. Specify a valid node.` (HSPICE reads `{a}` inside the quotes as the node `[a]`). A `.meas` with `{...}` is REJECTED: `.meas tran m1 max v({a})` gives `**error** (deck.sp:6) syntax error at or before "{a}" ")"`.
- `ovr_scope_local` (Task 8 review probe, `parhier=local`, `.param w=4`, `.subckt cc a b w=2 l=1`): HSPICE evaluates the text of an `X`-line override in the scope of the instance that the line makes, not in the parent's scope. `x1 1 0 cc l='w*3'` gives x1.r1=6 (the new instance's default `w=2`, not the top-level 4), and `x2 2 0 cc w=5 l='w*3'` gives x2.r1=15 (`w=5` from the same `X` line). The same deck: a body `.param` beats a header default of the same name (x3.r3=2), a header default can use another header name that the `X` line overrides (`l='w*3'` with `w=5` on the line: x4.r4=15), and a `.param` that uses a name defined again after it takes the last definition, at the top level (`.param a=1`, `.param b='a*2'`, `.param a=5`: r5=10) and in a subckt body (x6.r6=10), as in `dup_param`.
- `ovr_scope_child_global`: the same under the default parhier (GLOBAL), with no top-level definition of the name: `x1 1 0 cc l='w*3'` gives x1.r1=6 (the new instance's default `w=2`), and an override text that names a body `.param` of the new instance (`x2 2 0 cb l='v*3'`, `.param v=2` in `cb`) gives x2.r2=6.
- `ovr_scope_more_local` (`parhier=local`, `.param k=2 v=9`): an override text that names a body `.param` of the new instance uses it (`x1 1 0 cb l='v*3'`, `.param v=2` in `cb`: x1.r1=6, not 27). The `m` of an `X` line is evaluated in the scope of the line, not in the new instance: `x2 2 0 cm m='k'` (`cm` has the default `k=1`) gives m=2 (the directory prints 2.00, and `.op` prints R/m = 500 for the 1k `r2`).
- `ovr_self_local` (`parhier=local`, `.param w=4`, `.subckt cc a b w=2`): an override text that names the overridden name itself uses the parent's scope for that name: `x1 1 0 cc w='w*3'` gives x1.r1=12 (4*3, not the new instance's default 2*3).
- `ovr_self_global`: REJECTED. The same line under the default parhier with no definition of `w` above the instance aborts: `**error** (deck.sp:5) cannot redefine parameter to itself=x1.w`. So HSPICE looks up the overridden name only above the new instance, and the new instance's own default does not count.
- `ovr_undeclared` (`parhier=local`): an `X`-line override of a name that the `.subckt` line does not declare is accepted with no warning, and it is that instance's parameter: it beats the top-level `.param q=2` (`x1 1 0 cu q=5`: x1.r1=5), and it beats a body `.param w=2` of the definition (`x2 2 0 cb w=7`: x2.r2=7).
- `func_scope` (default parhier, GLOBAL): `.param` functions follow the GLOBAL order of parameters. A subckt `cf` that defines its own `.param f(x)='x*3'` still uses the top-level `f(x)='x*2'` (x1.r1=2k), and so does the top-level `leaf` under it (x1.xl.rl=2k). A function defined only in a subckt (`g` in `cg`) is visible in it (x2.r2=5k) and in the instances below it (x2.xl.rg=5k).
- `func_scope_local` (the same deck with `parhier=local`): the subckt's own `f` wins in the subckt (x1.r1=3k), but the top-level `leaf` under it uses the top-level `f`, not its parent's (x1.xl.rl=2k). `g` is visible as under GLOBAL (x2.r2=5k, x2.xl.rg=5k). This case alone looks like "the call's own scope, then as under GLOBAL", but the later probes show another rule: under LOCAL a call takes the nearest definition up the instance path (`func_levels_local`, `func_first_inst_local`), and x1.xl.rl=2k comes from the top-level instance `xt` of `leaf`, which HSPICE creates before x1.xl and which finds no `f` above it (`func_sticky_local`).
- `func_free_local` (`parhier=local`): a name in the body of a top-level function that is not an argument is resolved at the top level, where the function is defined, not where it is called: `.param k=2`, `.param h(x)='x*k'`, and a call in a subckt whose default is `k=7` gives x1.r1=2k.
- `rcl_param_tc` (`.temp 125`, `.param rval=2k`): a parameter name in the R value place, then bare numbers, reads as value, TC1, TC2: `r1 1 0 rval 0.004` gives 2.8k (2k*(1+0.004*100)), and `r3 3 0 rval 0.004 1e-5` gives 3k. `r2 2 0 rval` gives 2k. `extra.skip_value_check` lists r1 and r3 (temperature-adjusted `.op` values).
- `src_delay_scale`: with no DC value, a source with a delay gives in `.op` the value of its function at t=0. `AM(1 0.5 1k 1meg 1n)` gives 0 (`.option list` `dc volt` 0). `SIN(0.2 1 1meg 1n 0 90)` gives 1.2 (offset + amplitude*sin(phase), also with a delay), `SIN(0.2 1 1meg 0 1e6 30)` gives 0.7 (theta does not change t=0), `EXP(0.5 1 2n ...)` gives 0.5 and `PULSE(0.3 1 2n ...)` gives 0.3. The R element `scale=` multiplies the value: `r6 6 0 1k scale=2` gives `.op` `r value` 2.0000k and `.option list` `res eff` 2.0000k, `scale` 2.0000.
- `elem_scale_cl`: the element `scale=` multiplies C and L values too: `c1 1 0 1p scale=3` gives `.option list` `cap eff` 3.0000p (`scale` 3.0000), and `l1 1 2 1n scale=3` gives `ind eff` 3.0000n and `.op` `l value` 3.0000n (the inductor summary prints `nt` 3.0000 and no `scale` row).
- `sffm_phase`: HSPICE P-2019.06 shows 5 `SFFM` arguments, and a 6th and 7th do not change the `.op` value. `SFFM(0.1 1 1meg 2 1k 90 0)` and `SFFM(0.1 1 1meg 2 1k 0 90)` run with no warning; `.option list` prints only offset, amplitude, carrier freq, modn index and signal freq, and both give 0.1 in `.op` (the offset).
- `m_expr_zero`: REJECTED. An `X` line whose `m` is an expression with the value 0 (`xq a b t m=mz`, `.param mz=0`) aborts, as `m=0` does (`hier_m_zero`): `**error** (deck.sp:7) Multiplication factor(M) for the element x1.xq.xq cannot be < or = zero.` (HSPICE prints the name `x1.xq.xq`).
- `res_name_model`: REJECTED. A parameter name in the R value place, then a model name (`r1 1 0 rval rmod`, `.model rmod r`), aborts as `res_value_model` does: `**error** (deck.sp:4) Element in r1 has three nodes defined,  element must have two nodes`.
- `cap_name_model`: `c1 1 0 cval cmod` (`.model cmod c`, `.param cval=1p`) does not abort, but HSPICE does not read `cval` as the value, as in `cap_value_model`: `.option list` shows node2 `cval`, model `cmod` and `cap eff poly`, the `.op` node list has a node `cval`, and HSPICE warns `Capacitance value for element c1 is 0.000`.
- `ovr_self_combo` (second Task 8 review probe, `parhier=local`, `.param w=4`, `.subckt cc a b w=2 l=1`): an override that names the overridden name, and a use of that name on the same `X` line: `x1 1 0 cc w='w*2' l='w*3'` gives x1.r2=8 (`w` from the top level, 4*2) and x1.r1=24 (`l` uses the line's `w`=8). One level down, where the parent instance has `w=3`: `xc a b cc w='w*2'` gives x2.xc.r2=6 and x2.xc.r1=1. The override beats a body `.param w=2` of the new instance (`x3 3 0 cb w='w*3'`: x3.r3=12), and a header default `l='w*3'` uses the overridden `w` (`x4 4 0 ce w='w*2'`: x4.r4=24). Functions: with no top-level `fn`, `fo` (`fn(x)='x*5'`) and its child `fm` (`fn(x)='x*7'`) define `fn`; a call in `fm` and a call in `fl` under `fm` both use `fm`'s `fn` (x5.xm.r6=7k, x5.xm.xl.r5=7k): the nearer definition wins.
- `func_two_levels_global` (default parhier, the `fo`/`fm`/`fl` functions of `ovr_self_combo`): the outer `fo`'s `fn` wins, also over the `fn` of the call's own scope (x5.xm.r6=5k, x5.xm.xl.r5=5k). A function defined after its use works (`r7 7 0 'g(1)*1k'`, then `.param g(x)='x*5'`: r7=5k). A header default `l='w*3'` with a top-level `.param w=4` and a default `w=2` gives x4.r4=12.
- `func_redef`: a top-level function defined twice, used between the two definitions: the last definition wins (r1=3k), with no warning, as for `.param` values (`dup_param`).
- `parhier_dup`: `.option parhier=local`, then later `.option parhier=global`: the last option wins (x1.r1=4).
- `pe_scope_local` (`parhier=local`): the text of a winning `.param`, header default or `X` override is evaluated in the scope where it is defined, not where it is used. A top-level `.param a='b*2'` with `.param b=1`, used in an instance with `b=5`, gives x1.r1=2 (not 10). A body `.param ee='c*3'` in `mid` (`c=1`), used in a child with `c=7`, gives x2.xl.r3=3 (not 21). A header default `h='g*4'` gives x3.xl.r4=4 (not 36). An `X` override `p='q*2'` gives x4.xl.r5=2 (not 12).
- `pe_scope_global`: REJECTED. A top-level `.param a='b*2'` where only a subckt defines `b` aborts: `**error** (deck.sp:3) Undefined parameter or function definition "b" for a. Please enter a defined name.`
- `param_undef_unused` (Task 13a probe): REJECTED. An unused top-level `.param a='b*2'`, where nothing defines `b`, aborts: `**error** (deck.sp:2) Undefined parameter or function definition "b" for a. Please enter a defined name.` So HSPICE evaluates every top-level `.param`, used or not.
- `func_three_local` (`parhier=local`, a top-level `f(x)='x*2'`): the nearest instance definition beats the top-level one and a farther instance. `fo` (`x*5`), its child `fm` (`x*7`), and `fl` under `fm`: x5.xm.xl.r5=7k, x5.xm.r6=7k. With no top-level `g` and `g` in three levels (`x*3`, `x*5`, `x*7`), a call one level below the third gives 7k (x7.xb.xc.xd.r7). A call in an `X`-line override uses the new instance's function: `x3 3 0 cf w='f(1)*1k'` with `cf`'s `f(x)='x*3'` gives x3.rw=3k.
- `func_levels_local` (`parhier=local`, a top-level `f(x)='x*2'`): an instance definition two levels above the call beats the top-level one. `f` in level 1 (`x*5`), call in level 3: x5.xm.xl.r5=5k. `f` in level 2 (`x*7`), call in level 4: x7.xb.xc.xd.r7=7k. `g` only in level 1, call in level 3: x3.xm.xl.r3=3k.
- `func_first_inst_local`, `func_first_inst_global`: one definition that calls a function, in two instances under parents with different functions (no top-level definition), gets the parent's function in each instance, in both modes: `leaf3` under `ca` (`x*3`) gives x2.xl.r3=3k and under `cb` (`x*5`) x4.xl.r3=5k; `leaf4` under `cd` (`x*5`, created first) gives x6.xl.r4=5k and under `cc` (`x*3`) x8.xl.r4=3k. Under LOCAL with a top-level `f(x)='x*2'`, `leaf` with its only instance under `cf` (`x*3`) gives x1.xl.rl=3k: the parent's `f` beats the top-level one.
- `func_sticky_local` (`parhier=local`, a top-level `f(x)='x*2'`): `leafc` calls `f`. Its instance under `w` (no `f`) is created first and gets the top-level `f` (x0.xl.rc=2k), and its instance under `cf` (`x*3`) then also gets the top-level `f` (x1.xl.rc=2k, where `func_first_inst_local` gives 3k). For `leafd`, the instance under `cf2` (`x*3`) is created first and gets 3k (x3.xl.rd), and the instance under `w2` gets the top-level `f` (x4.xl.rd=2k). So (an inference from these probes and `func_scope_local`): under LOCAL a call takes the nearest definition of the function up its instance path, but when an instance of the same definition that HSPICE creates before it (breadth-first, `hier_dotted_collision*`) found no definition above it and took the top-level one, it takes the top-level one too. `func_sticky_ovr_local` shows that the unit of this rule is the text that the instances evaluate, not the definition.
- `rcl_tc_kw_after` (`.temp 125`, `.param rval=2k`, no `.model rval`): `r4 4 0 rval 0.004 1e-5 r=5k` gives `.option list` `res eff` 2.0000k, `tc1` 4.0000m, `tc2` 10.0000u, and `.op` 3.0000k (2k*1.5). So HSPICE uses the name `rval` as the value and not the `r=5k` after it, and the numbers after the name are TC1 and TC2. The control `r5 5 0 5k tc1=0.004 tc2=1e-5` gives 7.5k. `extra.skip_value_check` lists r4 and r5 (temperature-adjusted `.op` values).
- `rcl_kw_after_more` (`.temp 125`, `.param rval=2k`): an `r=` after a parameter name in the value place is not used, and an `r=` after a number is (`elem_dup_kw`). `r6 6 0 rval r=5k` gives 2k (`res eff` 2.0000k). `r8 8 0 rval 0.004 r=5k` gives `res eff` 2.0000k, `tc1` 4.0000m (`.op` 2.8k). `r7 7 0 1k 0.004 1e-5 r=5k` gives `res eff` 5.0000k, `tc1` 4.0000m, `tc2` 10.0000u (`.op` 7.5k), and `r9 9 0 1k 0.004 r=5k` `res eff` 5.0000k, `tc1` 4.0000m (`.op` 7k). `extra.skip_value_check` lists r7, r8 and r9 (temperature-adjusted `.op` values).
- `rcl_quoted_sticky_local` (third Task 8 review probe, `parhier=local`, `.param rval=2k`, no `.temp`): a quoted expression in the R value place, then `r=`: HSPICE uses the quoted expression and not the `r=`, with no warning (`r10 10 0 'rval' r=5k` and `r11 11 0 'rval*1' r=5k` give 2k). A name, then `tc1=` and `r=`, gives the name's value (`r12 12 0 rval tc1=0.004 r=5k`: 2k), and a number, then `r=`, gives the `r=` value (`r13 13 0 2k r=5k`: 5k, as in `elem_dup_kw`). With a top-level `f(x)='x*2'`, a call in a body `.param` (`leafp`) and in a `.subckt` default (`leafh`) uses the top-level `f` in the later instance under a parent that defines `f(x)='x*3'`, as in `func_sticky_local` (x1.xl.rp=2k, x3.xl.rh=2k). A call in an `X`-line override: `xl a b leafo v='f(1)*1k'` in `w3` (no `f`, created first) gives x4.xl.ro=2k, and the same text on the `X` line in `cf3` (`f(x)='x*3'`) gives x5.xl.ro=3k. So two `X` lines with the same override text count separately.
- `rcl_quoted_more` (`.option list`, no `.temp`): a quoted number also beats a later `r=` (`r14 14 0 '2k' r=5k`: 2k), and a `tc1=` after a quoted expression is read (`r15 15 0 'rval*1' tc1=0.004 r=5k`: `res eff` 2.0000k, `tc1` 4.0000m). The same for C and L, with no warning: `c1 31 0 'cval' c=5p` and `c2 32 0 'cval*1' c=5p` give `cap eff` 2.0000p (`.param cval=2p`), and `l1 33 34 'lval' l=5n` and `l2 35 36 'lval*1' l=5n` give `ind eff` 2.0000n and `.op` `l value` 2.0000n (`.param lval=2n`).
- `func_sticky_ovr_local` (`parhier=local`, a top-level `f(x)='x*2'`): the rule of `func_sticky_local` applies to the text that an instance evaluates. (1) `leafd` calls `f` in a `.subckt` default `v='f(1)*1k'`. Its first instance (under `w4`, no `f`) has `v=5k` on its `X` line (x0.xl.rd=5k), and its later instance (under `cf4`, `f(x)='x*3'`) uses the default with `cf4`'s `f`: x1.xl.rd=3k, not 2k. (2) One `X` line `xl a b leafo v='f(1)*1k'` in `mo`, with `mo` instantiated under `w5` (no `f`, first) and under `cf5` (`f(x)='x*3'`): x2.xm.xl.ro=2k and x3.xm.xl.ro=2k. (3) `leafb` calls `f` in an element line, and its instances are under `cf6`, then `w6`, then `cf6` again: x4.xl.rb=3k, x5.xl.rb=2k, x6.xl.rb=2k. So any earlier instance that used the top-level `f` counts, not only the first instance. An inference that fits all these probes: HSPICE keeps the top-level function for an expression once it evaluated that expression in an instance with no definition above it, and it does not evaluate a `.subckt` default that the `X` line overrides.
- `func_body_undef_global`: REJECTED. A top-level `.param g(x)='hq(x)*1k'`, called in `ch`, which defines `hq(x)='x*5'`, aborts: `**error** (deck.sp:2) Undefined function definition "hq". Please enter a defined name.` So the body of a top-level function does not see the functions of the calling subckt, as for parameter names (`pe_scope_global`). A first probe used the name `h` (`func_nested_global`): HSPICE read `h(...)` as an output variable and did not abort.
- `func_nested_local` (`parhier=local`): a function called in a function body is resolved where the outer function is defined. The top-level `g(x)='f(x)*1k'`, called in `cf` (which defines `f(x)='x*3'`), gives x1.r1=2k (the top-level `f`). `ch`'s `h(x)='f(x)*1k'`, called in the child `kh` (which defines `f(x)='x*7'`), gives x2.xk.r2=5k (`ch`'s `f(x)='x*5'`). A free name in the body of a function defined in a subckt also comes from where the function is defined: `ck`'s `hk(x)='x*kk*1k'` with `kk=2`, called in a child with `kk=9`, gives x3.xk.r3=2k. An unquoted override `x4 4 0 cq w=k` uses the new instance's default `k=1`, not the top-level `k=5` (x4.r4=1k).
- `func_sticky_more_local` (`parhier=local`): the order of the rule of `func_sticky_local` is breadth-first. `leafc`'s instance under `w` > `w2` is first in the text but at level 3, so the one under `cf` (level 2) keeps `cf`'s `f` (x1.xl.rc=3k, x0.xm.xl.rc=2k). The rule is per function name: `leafg` under `cfg` uses the top-level `f` (x3.xl.rf=2k) and `cfg`'s `g` (x3.xl.rg=7k), because its earlier instance under `wg` found `wg`'s `g` (x2.xl.rg=5k). A child of a later instance that uses the top-level `f` is not affected: x5.xl.xk.rk=3k (`cfs`'s `f`).
- `param_sticky_local` (`parhier=local`): parameter names do not follow the rule of `func_sticky_local`. The later parent's definition wins for a body `.param` (x1.xl.rp=3k), a header default (x3.xl.rq=3k) and a name in an expression (x5.xl.rs=3k), although an earlier instance of the same definition found only the top-level definition (2k).
- `ovr_body_global` (default parhier): at one level, the `X`-line override beats the body `.param`, which beats the header default (x1.r1=7, x2.r2=5, x3.r2=8; also x6.xi.r7=9, where the override is on the `X` line in `cmid`). An override text `w='k'` gives the parent's body `.param k=6`, not the new instance's default `k=1` (x4.xi.r4=6: under GLOBAL the outer definition wins). A body `.param z='u*3'` with the default `u` overridden to 4 gives 12, and a child inherits it (x5.r5=12, x5.xc.r6=12).
- `func_shadow_top` (fourth Task 8 review probe, default parhier): a top-level `.param max(a,b)='a+b'` hides the built-in `max`, with no warning: `r1 1 0 'max(1,2)*1k'` gives 3k (the built-in gives 2k).
- `func_shadow_sub` (default parhier): a `.param sgn(x)='x*7'` in `cs` hides the built-in `sgn` in `cs`, with no warning: x1.rs=7k (the built-in gives 1k).
- `func_shadow_scope` (default parhier): a `.param` function with the name of a built-in function is used where it is visible, and the built-in elsewhere. The top-level `min(a,b)='a*b+10'`, called in `cm`: x1.rm=16k. `cs` defines `sgn(x)='x*7'`: x2.rs=7k, and the top-level r3=1k and the top-level instance x4 of `ks` (x4.rk=1k) use the built-in. x2.xk, the instance of `ks` inside `cs`, sees `cs`'s `sgn` but gives x2.xk.rk=1k (the built-in). The listing shows only this value. An inference: x4, which HSPICE creates first (breadth-first), evaluates the same text and finds no `.param` `sgn`, so a rule like that of `func_sticky_local` holds also under GLOBAL, for the choice of a `.param` function or the built-in. A top-level `abs(x)='x*4'` in a top-level `.param` and in a `.subckt` default: r5=4k, x6.rd=8k.
- `func_shadow_more`: REJECTED. The deck of `func_shadow_scope`, and: `cs2` defines `sgn` and holds an instance of `ks2`, which calls `sgn`; `wk` (no `sgn`) holds another instance of `ks2`, created after it (x7, then x8). HSPICE aborts: `**error** (deck.sp:26) Undefined parameter or function definition "sgn" for x8.xk.rk2. Please enter a defined name.` The listing shows only this error. An inference: x7.xk, created first, uses `cs2`'s `sgn`, and x8.xk then finds no `.param` `sgn` and does not use the built-in. So the instance created first decides the kind of function (`.param` or built-in) for the later instances that evaluate the text, in both directions (`func_shadow_scope`: the built-in).
- `func_shadow_more_local`: REJECTED. `func_shadow_more` with `.option parhier=local`: the same error for x8.xk.rk2 (deck.sp:27).
- `sticky_lazy_local` (fourth Task 8 review probe, `parhier=local`, a top-level `f(x)='x*2'`): (1) a body `.param p='f(1)*1k'` that the `X` line of the first instance overrides (x0.xl.rp=5k) does not count for the rule of `func_sticky_local`: the later instance under `cf1` (`f(x)='x*3'`) gives x1.xl.rp=3k. (2) a call that the first instance does not evaluate, in the untaken branch of a ternary (x2.xl.rq=5k), counts: the later instance under `cf2` gives x3.xl.rq=2k (the top-level `f`). (5) a body `.param u='f(1)*1k'` that the first instance does not use counts too: x8.xl.ru=5k, x9.xl.ru=2k. (3) a call in the body of a function defined in a subckt has no creation-order rule: `ch` defines `h(x)='f(x)*1k'` and calls it; its first instance is under `w3` (no `f`: x4.xc.rh=2k), and the later one under `cf3` (`f(x)='x*3'`) gives x5.xc.rh=3k, the nearest `f`. (4) the same through a body `.param pk='hk(1)'` that the `X` line of the first instance overrides: x6.xc.rk=5k, x7.xc.rk=3k. (6) a function argument hides a parameter of the same name: `g2(y)='y*3'` with a top-level `y=10`, called at the top level and in `cy` (`y=7`): r11=3k, x10.ry=3k; `cz`'s `g3(z)='z*5'`, in an instance with `z=9`: x12.rz=5k.
- `func_global_more` (fourth Task 8 review probe, default parhier): names in function bodies and calls in overrides under GLOBAL. (1) A free name `kk` in the body of a function of `ck`, which `ck` (`kk=2`) and the top level (`kk=9`) define: x1.rk=9k (the top-level `kk`: the GLOBAL order from the level of the function's definition). (2) An argument hides a top-level parameter of its name: r2=3k (`g2(y)='y*3'`, `y=10`), x3.ry=5k. (4) A call in an `X`-line override, where the holder `cp` (`fo(x)='x*3'`) and the new instance `cc` (`fo(x)='x*5'`) define `fo` and the top level does not: x4.xc.rc=3k (the outer one). (5) `ch` defines `hn(x)='fn(x)*1k'`; its parent `cq` (`fn(x)='x*3'`) and its child `kh` (`fn(x)='x*7'`) define `fn`, and `kh` calls `hn`: x5.xh.xk.rh=3k (`cq`'s `fn`: the body is resolved at `ch`, where the child's `fn` is not visible). (6) A free name `qq` in the body of `cm`'s `hq`, which `cm`'s parent `cr` defines (`qq=4`) and `cm`'s child `km` defines as a default (`qq=8`): x6.xm.xk.rq=4k.
- `func_nested_global` (third Task 8 review probe, default parhier): the first form of `func_body_undef_global`, with the name `h`: a top-level `.param g(x)='h(x)*1k'`, called in `ch`, which defines `h(x)='x*5'`. HSPICE does not abort. The listing says `ac output variable type=h      not allowed for transient or dc`, lists x1.r1 (deck.sp:5) under "Summary of recovered resistors to avoid V loop", and `.op` gives `v1` (1 V on x1.r1 alone) a current of -100.0000k. So HSPICE read `h(...)` as an output variable and gave x1.r1 a value of about 0 (an inference from these lines; the listing prints no value for x1.r1). The listing shows no resistor (`extra.merged_listing`).
- `rcl_quoted_plus` (fifth Task 8 review probe, `.option list`, `.param rval=2k cval=2p lval=2n`): a quoted R, C or L value that goes on on a `+` line is one expression: `r1 1 0 'rval` / `+ *3'` gives 6k, `r4 1 0 'rval` / `+ *5' r=9k` gives 10k (the quoted value, not `r=`), `r5 1 0 'rval+` / `+ 1k'` gives 3k, `r6 1 0 'rval*` / `+ 3' tc1=0.004` gives 6k (no `.temp`). A quoted value on a `+` line after the nodes (`r3 1 0` / `+ 'rval*4'`) gives 8k, and an `r=` on the `+` line after a quoted value (`r2 1 0 'rval*2'` / `+ r=5k`) is not used (4k). C and L: `c1 2 0 'cval` / `+ *3'` gives `.option list` `cap eff` 6.0000p, and `l1 3 4 'lval` / `+ *3'` gives `ind eff` 6.0000n and `.op` `l value` 6.0000n.
- `rcl_quoted_dollar`: REJECTED. A `$` inside a quoted R value that goes on on a `+` line is not the start of a comment: `.param rval=2k`, `r1 1 0 'rval $ c` / `+ *2'` aborts with `**error** (deck.sp:4) Undefined parameter or function definition "rval $ c" for r1. Please enter a defined name.`
- `rcl_quoted_dollar_line`: REJECTED. A `$` inside a quoted `.param` value on one line: `.param q='rval*2 $ c'` aborts with `**error** (deck.sp:3) Illegal character in statement in column    8 : rval*2 $ c` and `**error** (deck.sp:3)  Number is improperly formed in column  10 : rval*2 $ c`. The deck also has `r1 1 0 'rval*3 $ c'` (line 5) and `r2 1 0 q`; HSPICE reports only line 3, so the listing does not show what it does with `r1` alone (`dollar_elem_line` shows it).
- `dollar_elem_line`, `dollar_func_body`, `dollar_override`: REJECTED. A `$` inside quotes in three more places aborts. A quoted R value on one line, `r1 1 0 'rval*3 $ c'`: `**error** (deck.sp:4) Illegal character in statement in column    8 : rval*3 $ c` (and `Number is improperly formed`). The body of a `.param` function, `.param f(x)='x*2 $ c'` (called in `r1 1 0 'f(1)*1k'`): `**error** (deck.sp:2) Illegal character in statement in column    5 : x*2 $ c`. An `X`-line override, `x1 1 0 cell w='rval $ c'`: `**error** (deck.sp:7) Undefined parameter or function definition "rval $ c" for x1.w.`
- `builtin_body_global` (fifth Task 8 review probe, default parhier): built-in function names in function bodies, defaults and overrides. (1) `ch` defines `h(x)='sgn(x)*1k'` and calls it; its first instance is under `w3` (no `sgn`: x1.xc.rh=1k, the built-in), the later one under `cs3` (`sgn(x)='x*7'`: x2.xc.rh=7k): in a function body the nearest `.param` function wins, with no creation-order rule. (2) The body of a top-level `g(x)='sgn(x)*1k'`, called in `cs4` (which defines `sgn`), uses the built-in (x3.r3=1k): the body is resolved at the top level. (3) A top-level `abs(x)='x*4'` defined after the top-level function `g2` that calls it: r4=4k. (4) A call of `max` before the top-level `.param max(a,b)='a+b'`: r5=3k (the last definition, also after the use). (5) A `.subckt` default `v='sgn(1)*1k'` and an `X`-line override `v='sgn(2)*1k'` in `cd`, which defines `sgn(x)='x*6'`: x6.rd=6k, x7.rd=12k (the new instance's `sgn`).
- `builtin_body_global_rev`: the order of `builtin_body_global` (1) reversed: the instance under `cs3` is created first (x1.xc.rh=7k), the one under `w3` later (x2.xc.rh=1k, the built-in).
- `builtin_local` (`parhier=local`, a top-level `sgn(x)='x*2'`): (1) `ks` calls `sgn`; its first instance is under `w1` (no `sgn`: x1.xk.rk=2k, the top-level `sgn`), the later one under `cs1` (`sgn(x)='x*7'`) gives x2.xk.rk=2k too (the rule of `func_sticky_local`). (2) The other order: x3.xk.rk2=7k (`cs2`'s `sgn`, first), x4.xk.rk2=2k. (3) With no top-level `abs`, the top-level instance x5 of `ka` uses the built-in (x5.ra=1k) and the later x6.xk, under `ca` (`abs(x)='x*4'`), uses the built-in too (x6.xk.ra=1k), as in `func_shadow_scope`. (4) `ch4` defines `h4(x)='min(x,5)*1k'`; under `w4` (no `min`) x7.xc.rh=1k (the built-in), under `cm4` (`min(a,b)='a+b'`, created later) x8.xc.rh=6k: no creation-order rule in a function body.
- `step_param`, `step_plain`, `step_lin`: HSPICE P-2019.06 has no `.step` command. `.step param pv 1k 3k 1k`, `.step pv 1k 3k 1k` and `.step lin pv 1k 3k 1k` each give `**warning** (deck.sp:6) unknown command ".step" is ignored`, and the deck runs once with the deck value (`.param pv=2k`: r1=2k).
- `quoted_cmtline` (sixth Task 8 review probe): REJECTED. A `*` comment line inside a quoted R value that goes on on a `+` line: `.param a=2k`, `r7 1 0 'a` / `* a comment line` / `+ *5'` aborts with `**error** (deck.sp:4) string misses dexter quotation mark, error is  before or at "'a"` (line 4 is the `r7` line). So HSPICE does not go on with the quoted text on the `+` line after a comment line. Outside quotes it does (`plus_after_cmtline`).
- `elem_unquoted_cmtline`: REJECTED in the same way when the comment line inside the quotes holds a `'` (`* it's a comment line`): `**error** (deck.sp:5) string misses dexter quotation mark, error is  before or at "'a"`.
- `plus_after_cmtline`: outside quotes, a `+` line after a `*` comment line goes on with the statement: `.param p=1k` / `* a comment line` / `+ p=4k` gives rp=4k, `r2 1 0 1k` / `* a comment line` / `+ r=5k` gives r2=5k, and `x1 1 0 cell` / `* a comment line` / `+ w=3k` gives x1.rc=3k.
- `unquoted_space`: REJECTED. HSPICE does not read the text from a ` *` to the line end in an unquoted `.param` value: `.param b2=a * 2 c2=5k` defines no `c2`, and `rc2 1 0 c2` aborts with `**error** (deck.sp:10) Undefined parameter or function definition "c2" for rc2. Please enter a defined name.`
- `unquoted_space_val`: the deck of `unquoted_space` with no use of `c2`: `.param b = a * 2` gives rb=2k and `.param b2=a * 2 c2=5k` gives rb2=2k (`.param a=2k`), so ` * 2` is not part of the value.
- `unquoted_expr`, `elem_unquoted`: an expression with no quotes and no spaces is read whole: `.param b=a*2` (rb=4k), `c=a+1k` (3k), `d=-a` (`rd 1 0 '-d'`: 2k), `e=2*a+1k` (5k), `g=max(a,3k)` (3k), `h=a/2` (1k), a header default `l=w*3` (x2.rl=3k), the same with the `X`-line override `w=a*2` (x1.rw=4k, x1.rl=12k), and the R value `re 1 0 a*2` (4k).
- `quoted_plus_more`: a quoted text that goes on on a `+` line is one expression also in a `.param` value (`q='rval` / `+ *2'`: r1=4k), a function body (`f(x)='x*` / `+ 3k'`: r2=3k), a function call (`'f(` / `+ 2)'`: r3=6k), an `r=` value (`r4 1 0 r='rval` / `+ *5'`: 10k), a `.subckt` default (`w='1k` / `+ *3'`: x1.rc=3k) and an `X`-line override (`w='rval` / `+ *4'`: x2.rc=8k), also with a `$` comment after the closing quote.
- `dollar_quote_comment`: a `'` in a `$` comment does not start a quoted text: `.param rval=2k $ don't change` (r1=2k), `r3 1 0 'rval*3' $ it's 'tripled'` (6k), `r5 1 0 rval $ it's` / `+ tc1=0` (2k), `x2 1 0 cell $ it's` / `+ w=3k` (x2.rc=6k). Function names are case-insensitive: `.param F2(X)='x*2'` called as `f2(1)` and `F2(2)` (r8=2k, r9=4k), `.param Max(a,b)='a+b'` called as `MAX(1,2)` (r10=3k, not the built-in 2k), `SQRT(4)` (r11=2k), and `.param SGN(Y)='y*7'` in `cs` called as `sgn(1)` and `Sgn(2)` (x3.rs=7k, x3.rt=14k).
- `func_args_plus`: the argument list of a `.param` function can go on on a `+` line: `.param g(x,` / `+ y)='x+y'` and `r1 1 0 'g(1k,2k)'` give r1=3k, with no warning.
- `rcl_name_first_cl` (`.option list`, `.temp 125`, `.param cval=2p lval=2n`, no `.model` cards): a parameter name in the C or L value place, then numbers, reads as value, TC1, TC2, as for R (`rcl_param_tc`). `c1 1 0 cval 0.004` gives `cap eff` 2.0000p and `tc1` 4.0000m, and `c2 2 0 cval 0.004 1e-5` adds `tc2` 10.0000u (temperature-adjusted 2.8p and 3.0p). `l1 1 11 lval 0.004` gives `ind eff` 2.0000n and `tc1` 4.0000m (`.op` `l value` 2.8n), and `l4 1 14 lval 0.004 1e-5` adds `tc2` 10.0000u (`.op` 3.0n). A quoted name, then a number, is the same (`c3 1 0 'cval' 0.004`, `l5 1 15 'lval' 0.004`: `tc1` 4.0000m).
- `ind_name_model` (`.option list`, `.temp 125`, `.param lval=2n`, `.model lmod l level=3`): a parameter name in the L value place, then a model name, is the value, then the model, as a number is (`ind_value_model`): `l2 1 12 lval lmod` gives `ind eff` 2.0000n and model `lmod`, and `l3 1 13 lval lmod 0.004` adds `tc1` 4.0000m (`.op` 2.8n). The control `l6 1 16 1n lmod` gives 1.0000n.
- `quoted_blank_line` (seventh Task 8 review probe): REJECTED. A blank line inside a quoted R value that goes on on a `+` line: `.param a=2k`, `r7 1 0 'a` / (blank line) / `+ *5'` aborts with `**error** (deck.sp:4) string misses dexter quotation mark, error is  before or at "'a"` (line 4 is the `r7` line), as a `*` comment line there does (`quoted_cmtline`).
- `sweep_op` (seventh Task 8 review probe): the `.op` block gives the deck value of a parameter that a `.dc` or a `.data` sweep changes. The deck has `.param pv=2k`, `.param pd=3k`, `.param pq='pv*2'`, `.dc pv 1k 3k 1k`, `.tran 1n 2n sweep data=d1` (`.data d1 pd` with the rows 1k and 4k) and `.op`. The `.op` block gives r1=2k (`pv`), r2=3k (`pd`) and r3=4k (`pq`), not the first sweep value (1k for `pv` and for `pd`). The listing prints the `.dc` results first: at pv = 1k, 2k, 3k the r3 current is 500.0000u, 250.0000u, 166.6667u, so `pq` follows `pv`. Then it prints the `.op` block, then the operating point of the `.tran` for each `.data` row (r2=1k, then r2=4k). No warning.
- `tc_tnom_dtemp` (seventh Task 8 review probe, `.option list tnom=50`, `.temp 125`, `.param a=2k`): TC1 and TC2 use TNOM from `.option tnom`, and the element `dtemp=` adds to the temperature of the element. The listing gives tnom= 50.000 and temp= 125.000 (dT = 75). `.option list` `res eff`, then `.op` `r value`: `r1 1 0 1k tc1=0.004` gives 1.0000k and 1.3000k (1k*(1+0.004*75)). `r2 1 0 1k tc1=0.004 dtemp=20` gives 1.0000k and 1.3800k (dT = 95; the element summary prints `temp` 70.0000, that is 50 + 20). `r3 1 0 1k 0.004 1e-5` gives 1.0000k and 1.3563k (1k*(1+0.3+1e-5*75^2)). `r4 1 0 a '0.004` / `+ *1' r=5k` gives 2.0000k, `tc1` 4.0000m, and 2.6000k: the name `a` is the value and `r=5k` is not used (as in `rcl_tc_kw_after`), and the quoted TC1 that goes on on a `+` line is read. `r5 1 0 1k tc1=0.004 scale=2` gives 2.0000k (`scale` 2.0000) and 2.6000k (2k*1.3): the TC applies to the value times the scale. `r6 1 0 1k tc1=0.004 dtemp=-75` gives 1.0000k and 1.0000k (the element is at TNOM; the summary prints `temp` -25.0000). `rn 1 0 1k` gives 1.0000k. The `temperature-adjusted values` table lists r1 to r5 only, with the `.op` values. `extra.skip_value_check` lists r1 to r5: their `.op` value includes the TC.
- `func_args_cmtline` (seventh Task 8 review probe): a `*` comment line inside the argument list of a `.param` function is passed over: `.param g(x,` / `* a comment line` / `+ y)='x+y'` and `r1 1 0 'g(1k,2k)'` give r1=3k. A `$` comment after the comma is passed over too: `.param h(x, $ c` / `+ y)='x*y*1k'` and `r2 1 0 'h(2,3)'` give r2=6k. No warning.
- `plus_after_blank` (seventh Task 8 review probe): outside quotes, a `+` line after a blank line goes on with the statement, as after a `*` comment line (`plus_after_cmtline`): `.param p=1k` / (blank line) / `+ p=4k` gives rp=4k, `r2 1 0 1k` / (blank line) / `+ r=5k` gives r2=5k, and `x1 1 0 cell` / (blank line) / `+ w=3k` gives x1.rc=3k. A `$` comment line there is passed over too: `r3 1 0 1k` / `$ a dollar comment line` / `+ r=6k` gives r3=6k. No warning.
- `body_self_ref_global` (seventh Task 8 review probe, default parhier): a body `.param` whose text names its own name. With `.param w=4` at the top level and `.subckt cb a b w=2` holding `.param w='w*3'` and `r1 a b w`, HSPICE runs the deck with no warning and no error, and the top-level `w=4` wins: x1.r1=4, and x2.r1=4 for `x2 1 0 cb w=7`. The listing does not show whether HSPICE evaluates the text `w*3` (an inference: under GLOBAL the top-level definition wins, so HSPICE does not need it).
- `quoted_blank_param` (8b-G probe): REJECTED. A blank line inside a quoted `.param` value that goes on on a `+` line: `.param a=2k`, `.param q='a` / (blank line) / `+ *2'` aborts with `**error** (deck.sp:3) string misses dexter quotation mark, error is  before or at "'a"` (line 3 is the `.param q` line), as in an R value (`quoted_blank_line`).
- `quoted_spaces_param` (8b-G probe): REJECTED. A line with only spaces and a tab (3 spaces, then a tab) inside a quoted `.param` value: `.param q='a` / (spaces and a tab) / `+ *2'` aborts with `**error** (deck.sp:3) string misses dexter quotation mark, error is  before or at "'a"`.
- `quoted_spaces_elem` (8b-G probe): REJECTED. The same line inside a quoted R value: `r1 1 0 'a` / (3 spaces, then a tab) / `+ *5'` aborts with `**error** (deck.sp:4) string misses dexter quotation mark, error is  before or at "'a"` (line 4 is the `r1` line).
- `quoted_dollarline_param` (8b-G probe): REJECTED. A `$` comment line inside a quoted `.param` value: `.param q='a` / `$ a dollar comment line` / `+ *2'` aborts with `**error** (deck.sp:3) string misses dexter quotation mark, error is  before or at "'a"`. This is the error of a `*` comment line inside quotes (`quoted_cmtline`), not an error of a `$` inside a quoted text (`rcl_quoted_dollar`: `Undefined parameter or function definition "rval $ c"`; `rcl_quoted_dollar_line`: `Illegal character in statement`).
- `quoted_dollarline_elem` (8b-G probe): REJECTED. The same `$` comment line inside a quoted R value: `r1 1 0 'a` / `$ a dollar comment line` / `+ *5'` aborts with `**error** (deck.sp:4) string misses dexter quotation mark, error is  before or at "'a"`.
- `quoted_plus_empty_param` (8b-G probe): a `+` line with no text inside a quoted `.param` value is passed over: `.param a=2k`, `.param q='a` / `+` / `+ *2'` and `r1 1 0 q` give r1=4k, with no warning.
- `quoted_plus_empty_elem` (8b-G probe): a `+` line with no text inside a quoted R value is passed over: `.param a=2k`, `r1 1 0 'a` / `+` / `+ *5'` gives r1=10k, with no warning.
- `quote_tail_param` (8b-G probe, the deck of the review `oracle-t8h/tool_only/u.sp` with a new title line): the text after the closing quote of a `.param` value is not part of the value: `.param a=2k`, `.param b='a' * 2` and `r1 1 0 b` give r1=2k (not 4k), with no warning. The deck uses only `b`, so it does not show whether HSPICE reads anything from ` * 2` (`unquoted_space` shows that HSPICE does not read the text from a ` *` to the line end of an unquoted `.param` value).
- `quote_tail_elem` (8b-G probe, `.option list`, `.param a=2k`): the text after the closing quote of an R value is not used: `r1 1 0 'a' *2` gives `.option list` `res eff` 2.0000k, `tc1` 0. and `tc2` 0., and `.op` r1=2k, with no warning. So `*2` does not multiply the value, and it is not TC1.
- `quoted_plus_pwl` (8b-G probe, `.option list`): a quoted PWL argument that goes on on a `+` line is one expression. `v1 1 0 PWL(0 '0.3` / `+ *1' 1n 1)` gives the PWL points (0, 300.0000m) and (1n, 1.0000), `dc volt` 300.0000m and `.op` node 1 = 300.0000m. `v2 2 0 PWL(0 '0.3` / `+ *2' 1n 1)` gives (0, 600.0000m) and (1n, 1.0000), and node 2 = 600.0000m. HSPICE prints the info line `DC voltage reset to initial transient source value` for v1 only, and no warning.
- `quoted_plus_tc` (8b-G probe, `.option list`, `.param rval=2k`, no `.temp`): a quoted positional TC2 that starts on a `+` line and goes on on the next `+` line is read. `R1 1 0 rval '0.004'` / `+ '1e-5` / `+ *1'` gives `res eff` 2.0000k, `tc1` 4.0000m and `tc2` 10.0000u, and `R2` with `+ *2'` on the last line gives `tc2` 20.0000u. `.op` gives 2.0000k for both. No warning.
- `kw_value_next_line` (8b-G probe): a `key=` at the end of a line takes its value from the next `+` line: `r1 1 0 r=` / `+ 5k` gives r1=5k, with no warning.
- `paren_plus_call` (8b-G probe, the control of `paren_blank_call` and `paren_cmtline_call`): an unquoted function call in a `.param` value can go on on a `+` line: `.param p=max(1k,` / `+ 3k)` gives rp=3k, with no warning. This deck cannot tell whether HSPICE drops the `+` of the `+` line or reads it as a unary plus (`max(1k,+3k)` is 3k too).
- `paren_blank_call` (8b-G probe): a blank line inside the open parentheses of an unquoted function call in a `.param` value is passed over: `.param p=max(1k,` / (blank line) / `+ 3k)` gives rp=3k, with no warning.
- `paren_cmtline_call` (8b-G probe): REJECTED. A `*` comment line inside the open parentheses of an unquoted function call in a `.param` value is not passed over: `.param p=max(1k,` / `* a comment line` / `+ 3k)` aborts with `**error** (deck.sp:2) Illegal character in statement in column    8 : max(1k,*acommentline+3k)`. The error text holds the comment line without its spaces, and the `+` of the next line. A `*` comment line inside the argument list of a `.param` function is passed over (`func_args_cmtline`), and so is one inside PWL parentheses (`paren_cmtline_pwl`).
- `paren_blank_args` (8b-G probe): a blank line inside the argument list of a `.param` function is passed over: `.param g(x,` / (blank line) / `+ y)='x+y'` and `r1 1 0 'g(1k,2k)'` give r1=3k, with no warning, as with no line between (`func_args_plus`) and with a `*` comment line (`func_args_cmtline`).
- `paren_plus_pwl` (8b-G probe, `.option list`, the control of `paren_blank_pwl` and `paren_cmtline_pwl`): PWL values in parentheses can go on on a `+` line: `v1 1 0 PWL(0 0.3` / `+ 1n 1)` gives the PWL points (0, 300.0000m) and (1n, 1.0000), and `.op` node 1 = 300.0000m.
- `paren_blank_pwl` (8b-G probe, `.option list`): a blank line inside PWL parentheses is passed over: `v1 1 0 PWL(0 0.3` / (blank line) / `+ 1n 1)` gives the PWL points (0, 300.0000m) and (1n, 1.0000), and `.op` node 1 = 300.0000m, with no warning.
- `paren_cmtline_pwl` (8b-G probe, `.option list`): a `*` comment line inside PWL parentheses is passed over: `v1 1 0 PWL(0 0.3` / `* a comment line` / `+ 1n 1)` gives the PWL points (0, 300.0000m) and (1n, 1.0000), and `.op` node 1 = 300.0000m, with no warning.
- `prot_param` (Task 13a review probe): a `.param rval=1k` and a `.model rmod r` inside `.prot` / `.unprot` are used outside it: `.param a='rval*2'`, `r1 1 0 rval` and `r2 1 0 a` give r1=1k and r2=2k, with no warning, and `r3 1 0 rmod` is counted (3 resistors) but has no `.op` column. So HSPICE uses the definitions inside a `.prot` region. The loader does not read the region (Warning `protected_region`): the checks of undefined names are off, and a name of the region gets the reason "not defined on this path, or a `.prot` region defines it". `extra.skip_value_check` lists r1 and r2 (names that only the region defines).
- `rcl_output_var` (Task 13a review probe): output variables in quoted R and C values run: `r1 out 0 'v(out)*1k+1k'` gives 2.0000k (with `v1 out 0 1`), `r2 out 0 'i(v1)*1k+1k'` gives 998.4985, and `c1 out 0 '1p*v(out)+1p'` is counted, with no warning. The netlist alone gives no value: resolution names the call (`v`, `i`) as a function that is not visible, and the load check of undefined names does not read the arguments of such a call. `extra.skip_value_check` lists r1 and r2.
- `param_output_var` (Task 13a review probe): an unused top-level `.param p='v(out)*2'` runs, with `**warning** (deck.sp:2) Parameter p is defined as an expression containing output signals, which may cause incorrect result.` So `out` in `v(out)` is not a parameter name (compare `param_undef_unused`).
- `func_body_unused` (Task 13a review probe): an unused top-level `.param f(x)='x*kk'`, where nothing defines `kk`, runs with no warning. So HSPICE does not check a function body before a call, and the load check of top-level `.param` texts does not read function bodies (compare `param_undef_unused`, a value).
- `param_redefined_text` (Task 13a review probe): `.param a='b*2'`, where nothing defines `b`, then `.param a=5k`, then `r1 1 0 a`, gives r1=5k with no warning. So HSPICE does not evaluate a top-level text that a later definition replaces (`dup_param`: the last one wins), and the load check reads only the last one.
- `stat_user_func` (Task 13a review probe): with `.param unif(a,b)='(a+b)/2'`, `r1 1 0 'unif(1k,3k)'` gives r1=2k with no warning. So a `.param` function hides the statistical function of its name (as in `func_shadow_top`), and the check `statistical_in_element` does not report it.
- `stat_in_mos` (Task 13a review probe): REJECTED. `agauss` directly in a MOSFET value (`m1 1 1 0 0 nch w='1u*agauss(1,0.1,3)' l=1u`) aborts as in an R value (`stat_direct`): `**error** (deck.sp:3) "agauss" function cannot be defined in expressions directly.` The check `statistical_in_element` reads every element line (probed for R and M).
- `ind_level_twice` (Task 13a review probe): REJECTED. `.model lmod l level=3 level=1` with `L1 1 2 1n lmod` aborts: `**error**  L-model called by 0:l1 is not for L_element, its level must be 3`. So the last `level` key of a card wins.
- `func_zero_args` (Task 13a review probe): REJECTED. A `.param` function with no arguments, not called (`.param f0()='2'`), aborts: `**error** (deck.sp:2) syntax error at or before ")" "=" "'2'"`. The loader gives the Error `func_no_args`. (The first probe of this form also had `v(out)*i(v1)` in the body and aborted in the same way.)

## Rejected decks

These decks abort in HSPICE (`***** job aborted`). They are kept as evidence:
`expected.json` has `resistors: []`, every total null (the statistics of an
aborted read-in count nothing reliable) and `extra.rejected` with the error
text. Value tests skip them by name.

- `dup_subckt`: REJECTED (duplicated subckt definition).
- `stat_direct`: REJECTED (`agauss` directly in an element expression).
- `title_plus`, `title_plus_words`, `title_plus_comment`: REJECTED (a `+` line right after line 1: `unable to open file "<line 1>"`).
- `elem_dup_kw_order`: REJECTED (a bare resistor value right after `r=`: syntax error).
- `ind_model_nolevel`: REJECTED (an `l` model card without `level=3`).
- `res_value_model`: REJECTED (a resistor value, then a model name: the value is read as a third node).
- `mos_digit_model`, `mos_digit_model_33`: REJECTED (a model name that starts with a digit).
- `mos_digit_model_ref`: REJECTED (`5vnmos` in the model place of an `M` line: syntax error).
- `inc_twice`, `inc_twice_params`: REJECTED (a `.subckt` or a `.model` in a file that is included twice).
- `lib_twice`: REJECTED (a `.subckt` in a `.lib` section that is called twice).
- `src_order_fn_first`: REJECTED (a transient function before `DC` on a `V` line).
- `param_braces`, `param_braces_param`: REJECTED (`{...}` in an element value or in a `.param` value).
- `title_plus_gap`: REJECTED (a `+` line after line 1 and a `*` comment line).
- `brace_quoted_param`, `brace_quoted_elem`, `brace_quoted_fn`: REJECTED (`{...}` inside quotes: undefined parameter `[a]`).
- `brace_fn_unquoted`: REJECTED (`{...}` in a PWL argument: syntax error).
- `src_fn_dc_more`, `src_fn_dc_more_noparen`: REJECTED (`DC` after `SIN(...)` or after `PULSE` without parentheses).
- `brace_quoted_temp`, `brace_temp`: REJECTED (`{...}` in a `.temp` value, inside quotes or not).
- `pulse_r_noparen`: REJECTED (`R` after `PULSE` values without parentheses is an undefined parameter `r`).
- `brace_meas`: REJECTED (`{...}` in a `.meas` argument: syntax error).
- `hier_reentry`, `hier_reentry_unscoped`: REJECTED (a definition instantiated inside an instance of itself: `subcircuit ... is defined recursively`).
- `hier_top_nested_only`, `hier_unres_in_subckt`: REJECTED (an `X` line with no visible definition on an instance path: `Definition of model/subckt ... is not found`).
- `hier_dup_nested`: REJECTED (two nested definitions with one name in one parent: duplicated subckt definition).
- `hier_m_zero`, `hier_m_negative`, `elem_m_zero`, `elem_m_negative`: REJECTED (`m` <= 0 on an `X` line or a resistor on an instance path: `Multiplication factor(M) ... cannot be < or = zero`).
- `hier_dup_instance`, `hier_dup_instance_sub`, `elem_dup_name`, `elem_dup_name_sub`: REJECTED (two `X` lines or two resistors with one name in one definition on an instance path: `attempts to redefine`).
- `hier_port_more`, `hier_port_fewer`: REJECTED (an `X` line whose node count is not the port count of its subckt: `Number of nodes mismatch`).
- `hier_port_dynamic`: REJECTED (an `X` line whose node count is not the port count of the definition it binds to on one of its instance paths: `Number of nodes mismatch`).
- `hier_port_params_colon`: REJECTED (`params:` in a `.subckt` header is a port, so an `X` line with one node less: `Number of nodes mismatch`).
- `hier_model_callee`: REJECTED (an `X` line that calls the name of an R model card: `Definition of model/subckt "rmod" is not found`).
- `ovr_self_global`: REJECTED (an `X`-line override that names itself, with no definition above the instance: `cannot redefine parameter to itself`).
- `m_expr_zero`: REJECTED (an `X` line whose `m` expression is 0: `Multiplication factor(M) ... cannot be < or = zero`).
- `res_name_model`: REJECTED (a parameter name as the R value, then a model name: the name is read as a third node).
- `pe_scope_global`: REJECTED (a top-level `.param` text that names a parameter that only a subckt defines: `Undefined parameter or function definition`).
- `param_undef_unused`: REJECTED (an unused top-level `.param` text that names a parameter defined nowhere: `Undefined parameter or function definition`).
- `func_body_undef_global`: REJECTED (the body of a top-level `.param` function calls a function that only the calling subckt defines: `Undefined function definition`).
- `func_shadow_more`, `func_shadow_more_local`: REJECTED (an instance finds no `.param` function with the name of a built-in function, after an instance created before it used one for the same text: `Undefined parameter or function definition`).
- `rcl_quoted_dollar`, `rcl_quoted_dollar_line`, `dollar_elem_line`, `dollar_func_body`, `dollar_override`: REJECTED (a `$` inside quotes, in an R value, a `.param` value, a function body or an `X`-line override: `Undefined parameter or function definition`, `Illegal character in statement`).
- `quoted_cmtline`, `elem_unquoted_cmtline`: REJECTED (a `*` comment line inside a quoted R value: `string misses dexter quotation mark`).
- `unquoted_space`: REJECTED (a name that only the text after a ` *` of an unquoted `.param` line defines: `Undefined parameter or function definition`).
- `quoted_blank_line`, `quoted_blank_param`, `quoted_spaces_param`, `quoted_spaces_elem`: REJECTED (a blank line, or a line with only spaces and a tab, inside a quoted R or `.param` value: `string misses dexter quotation mark`).
- `quoted_dollarline_param`, `quoted_dollarline_elem`: REJECTED (a `$` comment line inside a quoted R or `.param` value: `string misses dexter quotation mark`).
- `paren_cmtline_call`: REJECTED (a `*` comment line inside the open parentheses of an unquoted function call in a `.param` value: `Illegal character in statement`).
- `stat_in_mos`: REJECTED (`agauss` directly in a MOSFET `w=` value: `cannot be defined in expressions directly`).
- `ind_level_twice`: REJECTED (an `l` card whose last `level` key is not 3: `is not for L_element, its level must be 3`).
- `func_zero_args`: REJECTED (a `.param` function with an empty argument list: `syntax error`).

## Probes for the merge of the two parser branches

These probes ran on the same HSPICE version on 2026-11-05, in a kit outside this
corpus. The comments "probes on P-2019.06" in the code point to this section.
"Runs" means that the job concluded. "Stops" means that the job aborted. The
values of L and W come from `.meas` of `lv1` and `lv2`.

Values in parentheses:

- Runs: `.param p=(1+2)*1k` (3k), `.param p=(3k)`, `.subckt sub a b w=(3k)`,
  `x1 1 0 sub w=(3k)`, `r1 1 0 r=(3k)`, `r1 1 0 (3k)`, `v1 1 0 (1)`,
  `v1 1 0 dc=(1)`, `.option scale=(1)`, `.model nch nmos level=54 tox=(2n)`,
  `m1 1 1 0 0 nch l=(1u) w=(2u)`, `.param p=max(1k, 3k)`,
  `x1 1 0 sub w=max(1k, 3k)`, `.param f(x)=(x+1k)*2`,
  `.subckt (via) a b` with `.ends (via)` and `x1 1 0 (via)`.
- Stops: `.subckt sub a b w=max(1k, 3k)`, `r1 1 0 max(1k,3k)`,
  `.temp (50)`, `.temp (25+25)`, `.temp 25 (50)`.
- netlist-db reads the next three, although HSPICE stops:
  `.subckt sub a b w=max(1k, 3k)`, `r1 1 0 max(1k,3k)`,
  `m1 d g 0 0 nch l=max(1u,2u) w=1u`. The message for the last two is
  `Undefined parameter or function definition "max"`.

A value in parentheses over `+` lines. HSPICE keeps the `+` as an operator:

- `.param a=(1u` / `+ 2u)` is 3u. `.param a=(1u+` / `+ 2u)` is 3u.
  `.param a=(3u` / `+ -1u)` is 2u. `.param a=max(1u,` / `+ 2u)` is 2u.
- `x1 d g c w=(1u` / `+ 2u)` is 3u. `x1 d g c w=max(1u,` / `+ 2u)` is 2u.
- Stops: `.param a=(1u` / `+ *2)`, `.param a=max(1u` / `+ ,2u)`,
  `.param a=2*max(1u,` / `+ 2u)`, `.param a=max(1u,` / `+` / `+ 2u)`,
  `.subckt c d g w=(1u` / `+ 2u)`, and `l=max(1u,` / `+ 2u)` on a MOSFET line.
- On a MOSFET line, HSPICE reads `l=(1u` / `+ +2u)` and `l=(1u` / `+ 2u)` as 1u.
  netlist-db reads them as 1u too.
- netlist-db gives an error row on the `.param` and `X` forms with no operator
  (`(1u` / `+ 2u)`), and a value on the forms that stop.

Quotes in a group. HSPICE accepts them only inside `agauss`, `gauss`, `aunif`,
`unif`, `limit` and `str`, when the call is the whole value, on `.param`,
`.subckt`, `X` and `PWLFILE=` lines:

- Runs: `.param a=agauss(0,0,'2*pl')`, `.param a=AGAUSS(0,0,'2')`,
  `.param a=str("x")`, `.subckt c d g w=agauss(1u,0,'2') f=str('x')`,
  `X1 d g c w=agauss(1u,0,'2') f=str('x')`.
- Stops: `.param a=max(1,'2')` ("cannot use quotation marks"), `sin('2')`,
  `f('2')`, `(1+'2')`, `agauss(0,0,max(1,'2'))`, `max(1,agauss(0,0,'2'))`,
  `2*agauss(0,0,'2')`, `agauss(0,0,'2')*2`, `.param a='max'(1u,2u)`,
  `M1 ... l=agauss(1u,0,'2')`, `R1 d 0 r=agauss(1k,0,'2')`.

The words after the model name of a MOSFET:

- The values are `l w ad as pd ps nrd nrs`. They can be numbers, bare names or
  quoted expressions in any mix. Nine values and four or more names run.
- `OFF` takes a place and gives no value. `nch OFF 1u 2u` has the default L and
  W=1u. `nch 1u OFF 2u` has L=1u and the default W. `nch 1u 2u OFF` has L=1u, W=2u.
- `nch l=2u OFF w=1u` and `nch l=2u w=1u OFF` run. netlist-db reads both as
  L=2u, W=1u, with no error.

The words before the model name of a MOSFET:

- `m1 d g 0 0 x nch` (5 nodes) and `m1 d g 0 0 x y z nch` (7 nodes) run.
  `m1 d g 0 0 x y z w nch` (8 nodes) stops.
- The model name is the first word after the fourth node that names a model on the
  instance path, else the fourth word if it names a model. With `.model a`, `.model b`
  and `.param b=2u`, `M1 d g 0 0 a b w=1u` uses `a` with L=2u, `M1 d g 0 a b w=1u` uses
  `b` with the default L, and `M1 d g 0 0 x a b w=1u` uses `a` with L=2u. With no such
  word, HSPICE takes the last word as the model and reports that it is not found.
- `M1 d g 0 0 a b l=1u w=1u` stops (`Undefined parameter ... "b"`), although a keyword
  gives L. netlist-db reads it, as it did before the merge.
- A word that names a model only in another subckt also counts: `M1 d g 0 0 e a` at the
  top stops if `e` is a model inside some subckt. netlist-db reads `a` there.
- After four nodes, a word that starts with a digit stops the job: `M1 d g 0 0 5 nch`,
  `M1 d g 0 0 5a nch`, `M1 d g 0 0 1u nch` ("Model name is missing"). A sixth or a
  seventh node can start with a digit. So can the first four nodes.
- HSPICE drops an M line whose four nodes are one node. In a subckt,
  `M1 d d d d nch l=w w=1u` gives `**warning** (deck.sp:9) Both nodes of element x1.m1
  are connected together; Line ignored.`, and the statistics give `# mosfets = 0`
  (deck `p_self_local`, run on 2026-11-07, not in `golden/cases`). With
  `M1 d d 0 0 nch l=w w=1u` it keeps the line (`p_self_local2`). netlist-db lists and
  counts the dropped line. Not probed: which other sets of equal nodes HSPICE drops.

Other forms:

- Commas are delimiters: `.global vdd,vss`, `.ends,c1`, `.ends c1,`, `.ends (c1),`,
  `M1 d,g,0,0,nch,l=1u,w=1u`, `.subckt c d,g w=1u`, `.temp 25,50`, `X1 d,g,c` run.
- `.temp 50,75`, `.temp 50, 75`, `.temp 50,`, `.temp 50$room` and `.temp 50 $room`
  give temper=50 in the first run, with no warning (decks `t_*`, 2026-11-07). So a
  comma ends a value, with or without a space, and a `$` right after the number
  starts a comment. `50,75` and `50, 75` run again at 75. `.temp nosuch` stops
  ("can not find :nosuch").
- `.option post=0,ingold=2`, `R1 d,0,1k` and `V1 q,0,1` run. netlist-db gives an
  error on them, as it did before the merge.
- Stops: stray `.ends` and `.eom`, `.subckt` with no name, `.model 1m nmos`,
  `.model rmod r` with `R1 d 0 1k rmod`, `M1 d g 0 0 nch =1u`, `.data` with no name,
  `e1 2 0 1 0 gain=2`, `v1 1 0 SIN(0 1 1meg) PULSE(0 1 0 1n)`.
- Runs: `.subckt noports` with `x1 noports`, `.temp "50"`, `.temp '50'`,
  `e1 2 0 1 0 2 max=g`, `e1 2 0 vcvs 1 0 2`, `e1 2 0 poly(1) 1 0 0 2`, a `.lib`
  section with `.endlx` (a warning, and the next lines stay in the section).

## Comparison rounds with HSPICE as the reference

These rounds ran on HSPICE P-2019.06-SP1-1 on 2026-11-05 and 2026-11-06.
`scripts/hspice_kit/rounds/` holds the generators, and deck names below come from
them.

Method: a deck with `.option list nomod post=0` and no analysis line makes HSPICE
elaborate the circuit with no solve. The listing still holds the table of MOSFETs
(`**** mosfets`), with the model, `w drawn` and `l drawn` of each row. That gives
one row per MOSFET at low cost. A killed job can still be read: the table is complete
when `**** subcircuit calls` comes after it. For a value with `temper` or under
`.alter`, the table does not show the first run, so `.meas` gives the value.

Verdicts on 5,319 decks. Most decks aim at edge forms, so these counts do not
estimate an error rate on real decks. The baseline is commit f14c43e.

| Verdict | f14c43e | now |
|---|---|---|
| Same L, W and model | 1,209 | 4,022 |
| Both stop | 576 | 541 |
| Different values | 1,151 | 200 |
| HSPICE stops, netlist-db reads | 373 | 408 |
| HSPICE reads, netlist-db error row, also with other rows | 1,881 | 35 |
| HSPICE reads, netlist-db error only | 16 | 0 |
| Not comparable (no or empty listing, `.alter`, `temper`) | 113 | 113 |

### Rules that the code now follows

The tests in `src/tests/` and `src/eval/tests.rs` name the decks.

- A MOSFET line. `(`, `)` and `,` only separate words, also after a key and over `+`
  lines. `w=(2u + 3u)` is 2u, and `w=(2u+3u)` is 5u. On `.param` and `X` lines a group
  with spaces is one expression. A lone `+` takes a place and gives no value (`1u + 2u`
  has no W). In `+2u` it is a sign.
- A word that starts with a number is an expression only with `*`, `/`, `+` or `-`
  right after the number. Otherwise HSPICE reads the number and ignores the rest
  (`2n7002` is 2n, and `2^2` is 2).
- HSPICE assigns the plain numbers first and then the expressions, each in the order
  of the line, and the last one stays: `1u k w=5u` has W=k, and `1u 2u w=5u` has W=5u.
- A comma ends the value of a key. `ic=` keeps its list, and values after it belong
  to it.
- `.if`, `.elseif`, `.else` and `.endif` are evaluated per instance, after the values
  passed.
- FinFET bins (level 72) read `nfin*` and `wsheet*`, not `wmin` and `wmax`.
- `DEFL`, `DEFW`, `SCALE`, `SCALM`: the last plain number wins. A value of 0 or less
  gives the default back.
- Math: see the header of `src/eval.rs`.
- A chain of parameters can have any length. netlist-db stopped at 100 before.

### Differences that stay: HSPICE and netlist-db give different values

- `.if` order. The branch depends on the order of the instances in HSPICE when the
  first condition is one comparison in quotes or extra parentheses (`.if ('ll<4u')`,
  `.if ((ll<4u))`) and the second one is plain. netlist-db gives the logical branch.
  Decks `q_FIE_quoted`, s1218, s3216 and rounds f13 to f16, f20, f23. The form with `&&`
  does not show this.

  ```
  .subckt mac d g s b ll=1u ww=1u
  .if ('ll<4u')
  M0 d g s b n l=ll w=ww
  .elseif (ww>4u)
  M0 d g s b n l='ll*1.5' w=ww
  .else
  M0 d g s b n l='ll*2' w='ww/2'
  .endif
  .ends
  X0 d g s 0 mac ll=5u ww=5u
  X1 d g s 0 mac ll=3u ww=1u
  X2 d g s 0 mac ll=5u ww=1u    $ the logic says .else, HSPICE takes .if
  ```

- `.if` truth. `.if (2**-3^2<0.01)` is true in HSPICE, while `.meas` shows 0.015625.
  `.if (!(log(-8)))` is true in HSPICE, and false here. Decks `a00`, `t08`, `i0058`,
  `i0072`.
- The fourth word. On `M1 d g 0 a b ...` where `a` and `b` both name models, HSPICE
  takes `a` when another active line uses `a` as a model, and it reads `b` as L. Deck
  `t_param_b` gives a different value. Other forms stop in HSPICE only.
- A group right after the model. HSPICE stops on most forms. With one quoted value
  after the group and another active line that uses the model, W is only the first
  number of the quoted text (`(1u*1) '2u+1u'` has W=2u). Deck s2148.
- FinFET under `.option scalm`. After the first use of a bin, HSPICE multiplies its
  `nfin` limits by `scalm`, once per bin. So the choice depends on the order of the
  lines. Decks of round f36, r4.
- Depth under `parhier=global`. HSPICE reads a parameter at most 31 levels above the
  reader, in both `parhier` modes. Past that, a top-level name is undefined and HSPICE
  stops (`g_read_32`, `l_read_32`), and under `global` a nearer value wins with no
  message (`g_top_32`, `g_self_33`). This looks like a limit or a bug of HSPICE, not a
  rule. Decision: netlist-db does not copy it, and reads through any depth. So a value
  can differ from the value that HSPICE simulates, and netlist-db warns at each read
  past 31 levels (`FarRead`: `ModelUsage::far_reads`, the return of
  `for_each_mosfet_model`, the log, and stderr in `model_select`). The distance counts
  from the level that holds the text to the level of the definition that wins. A deep
  deck that reads a near value agrees, with no warning (`l_top_96`).
- `.alter` in an include (`inc_alter_inside`, `al_inside_inc`). Line 1 is always a title
  in HSPICE (`o_title_option`). `.param f=1u` with `.param f(x)=...` (`p_func_same_as_param`).

### Differences that stay: netlist-db gives an error row

- A loop of two or more top-level `.param` lines. HSPICE runs it, with no warning, and
  reads 0 where the loop closes (`.param a='b+1u' b='a+1u'` gives a=1u, b=2u, with the
  lines in the order of the file). A loop in a subckt and `.param a='a+1u'` stop in both.
- `.option macmod=1` or `3`: an M line may name a `.subckt` (`mc_macmod*`).
- `l=0` with `xl>0` (`z_l0_xl`), strings (`x_value_string`, `if2_str`), nesting over 256
  (`if2_nested_300`, `dp_if_1000`, `dp_loc_*`), an overflow (`f_pow_big`, `10**400`),
  an include that splits a statement (`inc_ends_split`, `inc_plus_*`, `inc_subckt_open`).
- A digit-first word with the model at the fourth place (`M1 d g 0 n 1u 2n7002`). Golden
  `mos_digit_model` needs this error.
- `1u 2&&3`, `1u 2?3:4`, `w=2<3`, `w=2?3:4`, `1u 2u;3`, `1u 2u&3`.

### Differences that stay: HSPICE stops and netlist-db reads a value

- Duplicate names (`m_name_dup*`, `x_dup_name`, `model_redefined*`, `subckt_redefined*`,
  `inc_twice*`, `lib_twice`), unread undefined names (`unread_undefined_*`, `w=2u + c`).
- A dot line on line 1 (`o_title_*`), `pi`, `min` and `max` with other than two values,
  two signs in front of a value (`--2`, `!-2`), nested parentheses after a key
  (`l=(1u+(2u))`, `l=(max(1u,2u))`), operators as words (`w=(k / 1)`, `l=(k)*(j)`),
  bare `max(...)` on an M line, `1u k^2`, `w=k^2`.
- A model more than 99 levels above the MOSFET (`d_100`, `rch_100`, `deep200`).
- Level-specific stops (`mdl_lv49_bins`, `ff_73_*`, `z_w0_lv1`).

### Listings of 0 bytes

HSPICE writes an empty `.lis` on some decks, and the cause is unknown. It may be a crash.
Of the 27 decks with an empty listing in the table above, 17 stay empty when they run
alone. The other 10 gave a listing when they ran alone, so their empty listings came
from the job and not from the deck. One deck (`nf_78_sub`) was empty once and normal on
the next run, so one empty listing proves nothing. All the decks that were reduced hold
`.if`. `c5_gl` is a short example: with `.option parhier=local` it runs and L=2u, and
netlist-db gives L=2u in both.

### Value limits

`int`, `ceil`, `floor`, `trunc`, `nint` and `round` convert through 32 bits in HSPICE
(`int(3e9)` is -2147483648). `.meas` prints `*.` from 1e100 on and `0.` below 1e-28, but the
values are still there. The evaluator in `expr/` (query crate) does not follow the math
rules of this section. `src/eval.rs` (model selector) does.

### Large decks

| Deck | HSPICE | Compared with netlist-db |
|---|---|---|
| 11k, 100k | concluded in 31 s and 53 s | L and W equal on 111,176 rows |
| 250k | killed at 175 s, table complete, peak 1.75 GB | equal on 249,992 rows |
| 500k | killed at 170 s, table complete, peak 2.97 GB | equal on 499,992 rows |
| 1M | killed at 175 s, table cut, peak 3.86 GB | equal on 645,936 rows |
| 1M to 300M with no table | killed at the cap of about 4.5 GB | no compare |
| `mods.py`: 2,000 bins, 50k parameter chain, 20k subckts, 100k fan-out, 64 `.if` blocks, global | concluded | equal on 219,000 rows |
| `bust.py`: 10k leaves with their own L, plain and `.if` | concluded | equal on 20,000 rows |

The machine was a VM with 6.4 GB. HSPICE cannot flatten 1M MOSFETs or more under the cap,
so the larger decks were checked against the closed-form counts of their generators.

### Round 47: the forms and limits of the parser repairs (`forms47.py`, P-2019.06-SP1-1)

- A `\` line after a `$` in a name (`ds_name`, `ds_value`): the line goes on, and `ds_word`, `ds_num`: a `$` that starts a comment hides the `\`. netlist-db agrees on all four.
- Short forms of `.unprot`: HSPICE ends a plain `.prot` region at `.unpr` (`pl_unpr`), and its own warning names `.unpr` (`fl_noend`: ".prot and .unpr not matched"). netlist-db ends a `.prot freelib` block at each short form, and a block with no end is an error. The text of these test blocks is not encrypted, so HSPICE stops at it ("Error in encrypted blocks"). netlist-db skips the block.
- A `\` at the end of the last line inside a plain `.prot` region (decks `prot_bs1`, `prot_bs2`, `prot_ctrl`, run on 2026-11-07, not in `golden/cases`; `.prot` / `R2 a 0 2k\` / `.unprot` / `R1 a 0 1k`). With one `\`, the R2 line goes on over `.unprot` and takes in the R1 line, and HSPICE stops: `**error** (deck.sp:9) Undefined parameter or function definition "r1" for r2.` With `\\`, the job concludes with `**warning** .prot and .unpr not matched,  unencrypted information may be enclosed.`, and R1 is simulated (the current is 1.5 mA, from 1k and 2k in parallel) but has no row in the listing. With no `\` (`prot_ctrl`), R1 is listed. The model selector gives a `SyntaxError` on the R2 line with one `\` and no message with `\\`, and in both cases it reads the lines after `.unprot`. The query tool gives the Error `missing_unprot` in both cases and reads no line after `.prot`, so it drops R1. Not probed: how HSPICE joins the lines inside `.prot`.
- Bytes that are not UTF-8, outside comments: this HSPICE on Windows reads each such byte as white space. `.subckt \xe9 d g` defines the subckt `d` (`b_subckt` stops: "duplicated subckt definition 'd'"), and `.param p\xe9=1u p\xe8=2u` sets `p` twice (`b_param`: L=W=2u). This looks like an accident of the character tables, and it can differ on other systems. So netlist-db does not copy it. Each byte value stays one private character in the name, so the names stay apart, and the warning `non_utf8` says that the file has such bytes.
- Lone CR line ends (2026-11-07, `cr_only`, `cr_slash`, `cr_mid`): this HSPICE on Windows reads a lone CR as white space, not as a line end. A file with only CR line ends is one title line: HSPICE simulates nothing and gives no message. In an LF file, a lone CR joins two lines (`cr_mid` stops). netlist-db keeps the logical reading (a lone CR ends a line) and gives a warning (`lone_cr` in the query tool), once for each file. For a `.lib` section, it warns only for a lone CR in the section's own lines.
- Long expressions: `e_sum_1000` and `e_pow_1000` agree. With 30000 terms HSPICE writes an empty listing and no message (`e_sum_30000`, `e_pow_30000`). netlist-db gives the value.
- Functions that fan out: HSPICE gives the value at 26 levels, 2^26 calls (`fn_fan_26`: L=67.1089m). netlist-db keeps the result of each call during one evaluation, so it agrees at each depth measured. Calls that each give new values stop after 2^22 different calls, with an error. This limit is ours.
- Chains of includes: HSPICE reads 1000 files (`i_chain_1000`). netlist-db follows 1100, and the next include is an error, because its walks recurse. On a stack of 2 MB the release build reads `i_chain_1000`.
- Diamonds of includes: `i_dia_10` and `i_dia_16` agree. HSPICE did not end `i_dia_20` in 140 s. netlist-db stops at 1 GB of expanded text plus 16 times the size of the files, with an error. This limit is ours. The deck of 40 levels takes 6 s and 2.6 GB before the stop.

### The output files of a swept `.dc` (run on 2026-11-07)

Decks `sw_dc1`, `sw_dc2`, `sw_dc_alter`, `sw_tran_alter`, `sw_dc_param` and
`sw_dc_data`, with `.option post=2`. They are not in `golden/cases`.

- A single sweep (`.dc v1 0 1 0.5`) writes one table, and one `0.1000000E+31` ends it.
  The independent variable is named `VOLTS` for a source, `TIME` for `.tran`, the
  parameter name for `.dc p 1 2 1` and the data name for `.dc data=d1`. No file holds
  the word `sweep`.
- A second sweep (`.dc v1 0 1 0.5 sweep p 1 2 1`) writes one table for each outer
  value, in one file. Line 1 holds `0001` where a single sweep holds `0000`. Line 3
  holds the number of outer values (0 for a single sweep). The outer name (`p`) comes
  after the signal names on line 5. Each table starts with its outer value and ends
  with `0.1000000E+31`. Each number takes 13 characters, 6 to a line, and the numbers
  go on over the line ends.
- `.alter` writes one file for each run (`.sw0` and `.sw1`, `.tr0` and `.tr1`), each
  in the form of a single sweep.
- `src/parser/data_sweep.rs` reads none of these files. It reads only one table whose
  independent variable is named `sweep`, the form of `tests/sim.sw0`. With `VOLTS`
  changed to `sweep`, it reads `sw_dc1`. For a second sweep it also fails: the outer
  name is not a `v(`, `i(` or `p(` name, and the outer values and the
  `0.1000000E+31` between the tables move the columns. Nothing in the workspace calls
  it outside its own test.

### The card kit (`scripts/hspice_kit/cards`, run on 2026-11-08)

HSPICE P-2019.06-SP1-1 ran the 20 decks of the kit (answers of netlist-db 66c0e11).
The results are in `scripts/hspice_kit/cards/kit/results/`, with no `lic:` lines.
`compare_cards.py` gives 34 probes that agree, 1 that differs and 1 that is only
probed. The `.op` table names netlist-db's card for each MOSFET, and the bins of the
corner decks too.

- A card inside a subckt is evaluated in each instance, with the parameters of that instance, under both `parhier` modes (`c_mac_global`: x1 0.23, x2 0.27; `c_mac_local`: the same values with a top-level `dv`).
- Under `parhier=global` a top-level `.param` wins over the instance in a card, as for an element (`c_mac_global_top`: 0.21 for both instances).
- A top-level card is evaluated with the top-level parameters, also when only MOSFETs in subckts that define the name use it (`c_top_card_local`: 0.21 for both). The gap of the bin limits (the first MOSFET's own instance) does not occur for `vth0`.
- A top-level card that reads a name that only the subckts define: HSPICE stops (`c_top_card_notop`: `Undefined parameter or function definition "dv" for mt`). It does not evaluate the card in the MOSFET's instance. netlist-db gives no value, so the two agree.
- A card in an outer instance is evaluated in that instance, also when the MOSFET's own instance defines the name (`c_nested_local`, `c_nested_global`: 0.23). A card found along the instance path is evaluated where it is defined (`c_dynamic_local`: mid's 0.23).
- A body `.param` of the macro computed from the instance's L, a PDK library with corner sections and binned cards in the wrapper subckt, a `.param` function, `temper` and `agauss` through a `.param` all agree (`c_body_param`, `c_corner_tt`, `c_corner_ff`, `c_func_global`, `c_temper`, `c_stat_param`).
- `agauss` directly in a card: REJECTED (`c_stat_direct`: `"agauss" function cannot be defined in expressions directly`), as in an element expression (`stat_direct`, `stat_in_mos`). netlist-db gives the nominal value 0.25. This is the one difference.
- A card that sets `vth0` twice: HSPICE uses the last one (`c_dup_key`: 0.32 of 0.24 and 0.32). For a bin limit the first one wins (`mdl_dup_keys`).
- R cards: per instance, the top-level value under `global` (`c_r_global`: 150) and the instance's value under `local` (`c_r_local`: 30, 60). An R line finds its card along the instance path (`c_r_dynamic`: mid's card 300 under mid, the top-level card 200 elsewhere).
- A diode card inside a subckt is evaluated in each instance (`c_diode`: `is` 1e-14 and 4e-14).
- The run: `run_cards.ps1` does not parse in Windows PowerShell 5.1 (`$LASTEXITCODE:` reads as a drive-qualified name). Over `ssh`, `hspice.exe` wrote empty files. The decks ran with one `prlctl exec` call per deck, from inside `decks/`, as `golden/tools/run_hspice.sh` does. `hspice -v` printed nothing there, so `hspice_version.txt` holds the header line of the listings.

## Known differences

HSPICE rejects each deck below, and the loader gives no Error for it
(Task 13a). Each line gives the reason. The test
`rejected_decks_have_errors` takes the case names of this section.

- `m_expr_zero`: an `m` expression with the value 0 or less. In general the
  value depends on the instance path, so a load check must evaluate it on
  every path, which is not linear in the input. A linear check exists for
  the forms whose value does not depend on the path (a constant, or names
  that only top-level `.param` lines define under parhier=global, as in
  this case), and it is not done. `resolve_element` on a path below the
  line gives the reason.
- `func_shadow_more`, `func_shadow_more_local`: an instance finds no
  `.param` function with the name of a built-in after an earlier instance
  used one for the same text. The result depends on the creation order of
  the instances, so no linear check finds it. `resolve_element` gives the
  reason.
- `func_body_undef_global`: the body of a top-level `.param` function calls
  a function that only a subckt defines. The cost is not the reason: HSPICE
  reads some such names as output variables and runs the deck
  (`func_nested_global`, with `h`), and the set of those names was not
  probed, so a name check could reject a deck that HSPICE runs.
  `resolve_element` names the call as a function that is not visible.
- `pulse_r_noparen`: `R` after the `PULSE` values without parentheses.
  The loader keeps the arguments of a source function as text and does not
  read them as expressions. A linear check of the PULSE words exists, and
  it is not done. `resolve_element('v1')` gives the DC value 0 (the PULSE
  value at t=0) and does not give the reason.
- Forms with no golden case: an `X` line in a subckt whose override names
  itself (the check needs the instance paths above the line, so only the
  top-level form `ovr_self_global` gets an Error), and an element
  expression that names a parameter that a subckt or an `X` line defines
  somewhere but not on its path. After a load Error, and in a deck with a
  `.prot` region (`prot_param`), the checks of undefined names are off: a
  skipped statement or the region can define the name.

## Choices not probed

Each line below is a form that was not run in HSPICE: the loader makes the
choice that the line states, and a test pins it. Some lines also name
probed forms, with their golden case, to show the rule that the choice
follows; the result of each probe is in Findings. When a probe settles a
form, the form leaves this list.

Probes settled some forms after the list was started. The loader follows
each result, and a test pins it. The 6b-G probes settled `.end` in an
included file (`end_in_include`: HSPICE ignores it with a warning and reads
on; the loader gives the Warning `end_in_include`) and `{...}` inside quotes
(`brace_quoted_param`, `brace_quoted_elem`, `brace_quoted_fn`: HSPICE aborts;
the 6a-P loader kept the expression with no diagnostic, and it now gives the
Error `braces`). The other 6b-G probes (`pwl_repeat`, `cl_tc_positional`,
`src_fn_dc_more`, `after_end_second_deck`) settled forms that had no line
here.

The close-out probes settled these forms. `R` after a PWL
(`pwl_repeat_noparen`, `pwl_repeat_key_bare`, `pwl_repeat_key_bare_mid`,
`pwl_ac_r`, `pwl_r_param_last`) is the repeat flag: the 6a-P loader read it
as a fifth PWL argument and as the AC phase, with no diagnostic. `R` after
PULSE values (`pulse_r_noparen`) is a fifth argument, as the loader reads
it. HSPICE aborts on `{...}` in a `.temp` value (`brace_quoted_temp`,
`brace_temp`) and in a `.meas` (`brace_meas`): the loader kept these lines
with no Error, and it now gives the Error `braces`. HSPICE ignores a
`.print` or `.probe` output with `{...}`, with a warning (`brace_print`,
`brace_probe`, `brace_probe_quoted`): the loader gives the Warning
`braces_output`. A `.end` line with text after the keyword ends the top file
(`end_quoted_arg`, `end_kv_arg`): the loader gave `trailing_text` and read
on. HSPICE warns about an odd number of PWL values (`pwl_r_param_last`) and
drops the repeat for `R=` at the last time point (`pwl_repeat_key_only`):
the loader gives the Warnings `pwl_odd_values` and `pwl_repeat_dropped`.

The Task 7 probes settled these forms. A name nested in P's own definition
and in an ancestor's definition (`hier_shadow_own`): P's own wins, as the
loader chose. An `X` line with no visible definition on an instance path
from the root (`hier_top_nested_only`, `hier_unres_in_subckt`): HSPICE
aborts, and the loader now gives the Error `unresolved_subckt` there (it
gave a Warning). The same line in a definition that no instance path
reaches, and a call cycle there (`hier_unused_cycle_unres`): HSPICE runs the
deck, and the loader now gives a Warning for each (it gave the Error
`subckt_cycle` for the cycle). A definition instantiated inside an instance
of itself when the dynamic lookup would end the expansion (`hier_reentry`,
`hier_reentry_unscoped`): HSPICE aborts, and the loader now gives the Error
`subckt_cycle` (it gave totals and no diagnostic). The Task 7 review probes
settled these forms. Two nested definitions with one name in one parent
(`hier_dup_nested`): HSPICE aborts, as the loader's Error `duplicate_subckt`
says (its message said that this form was not tested). `m` <= 0 on an `X`
line or a resistor (`hier_m_zero`, `hier_m_negative`, `elem_m_zero`,
`elem_m_negative`): HSPICE aborts on an instance path and does not check a
line that no path reaches (`elem_m_zero_unused`): the loader gave totals and
no diagnostic, and it now gives the Error `m_not_positive` on an instance
path and the Warning `m_not_positive` elsewhere. An unused nested definition
with an undefined callee or a call cycle (`hier_unused_nested_unres`,
`hier_unused_nested_cycle`): HSPICE runs the deck, and the loader gives a
Warning, as for an unused top-level definition. The second Task 7 review
probes settled these forms. Two `X` lines or two resistors with one name
in one definition (`hier_dup_instance`, `hier_dup_instance_sub`,
`elem_dup_name`, `elem_dup_name_sub`): HSPICE aborts on an instance path and
does not check a definition that no path reaches (`hier_dup_instance_unused`,
`elem_dup_name_unused`): the loader gave totals and no diagnostic, and it
now gives the Error `duplicate_element` on an instance path and the Warning
`duplicate_element` elsewhere. Instance names with a `.` (`hier_dotted_names`):
the path walk took the longest name at each level and stopped when the rest
of the path was not found below it; it then went back to a shorter name.

The Task 7 close-out probes settled these forms. Two instances with one full
name (`hier_dotted_collision`, `hier_dotted_collision_deep`,
`hier_dotted_collision_order`, `hier_dotted_collision_order_rev`,
`hier_dotted_collision_below`): HSPICE gives the name to the instance it
creates first, breadth-first, and the instances below a later one keep full
names. The walk took the longer name at the first level where two readings
differed, which is wrong for `hier_dotted_collision_deep` and
`hier_dotted_collision_order`, with no caveat; it now searches the instances
in HSPICE's creation order, with a caveat. An `X` line whose node count is
not the port count of its subckt (`hier_port_more`, `hier_port_fewer`):
HSPICE aborts; the loader gave no diagnostic, and it now gives the Error
`port_count_mismatch`. The same line in a subckt that no instance path
reaches (`hier_port_unused`): HSPICE runs the deck, and the loader gives the
Warning `port_count_mismatch`. An `X` line that calls the name of a model
card of a built-in type (`hier_model_callee`): HSPICE aborts; the loader gave
the Warning `unresolved_subckt` (a model of a Verilog-A module), and it now
gives the Error.

The Task 7 review probes settled these forms. Two elements with one full
path (`elem_dotted_name_collision`, `elem_dotted_name_collision_rev`): HSPICE
splits the path at its last `.`; the walk took the first element in creation
order, which is wrong for `elem_dotted_name_collision_rev`, and it now takes
HSPICE's rule. A device that only the later instance with a full name holds
(`hier_dotted_collision_hidden`): not found, as the walk did. The names after
a later instance (`hier_dotted_collision_shift`): `lis2json` stopped with an
error; it now maps them back. The `X`-line order in a level
(`hier_dotted_collision_xline_order`): as the walk did. One `X` line with two
bindings (`hier_port_dynamic`): the node count is checked for each binding,
as the loader did. `params:` in a header (`hier_port_forms`,
`hier_port_params_colon`): a port, as the parser reads it.

The Task 8 review probes settled these forms. The scope of an `X`-line
override text (`ovr_scope_local`, `ovr_scope_child_global`,
`ovr_scope_more_local`, `ovr_self_local`, `ovr_self_global`): HSPICE uses
the new instance's scope, with the overridden name itself looked up above
it, and `m` in the line's scope. Task 8 used the parent's scope, which gave
wrong values with no caveat under LOCAL (12 for x1.r1 and x2.r1 of
`ovr_scope_local`) and no value under GLOBAL (`ovr_scope_child_global`); it
now follows HSPICE. The scope of a `.param` function (`func_scope`,
`func_scope_local`): Task 8 used the nearest definition, which gave 3k for
x1.r1 of `func_scope` with no caveat; it now uses the GLOBAL order under
GLOBAL (the LOCAL order is in the next paragraph). The element `scale=` of R, C and L
(`src_delay_scale`, `elem_scale_cl`): Task 8 gave the value as written; it
now multiplies it. AM with a delay (`src_delay_scale`): 0, where Task 8 gave
no value. These probes agree with Task 8's choices: a name in the body of a
top-level function (`func_free_local`), a parameter name as the R value with
TC numbers after it (`rcl_param_tc`), an override of a name that the
`.subckt` line does not declare (`ovr_undeclared`), and SFFM with 7
arguments (`sffm_phase`: the offset). `m` <= 0 from an expression on an `X`
line (`m_expr_zero`) and a name, then a model name, as the R or C value
(`res_name_model`, `cap_name_model`): HSPICE rejects the deck, or uses a
capacitance of 0; Task 8 gave a value with no caveat, and it now gives a
caveat, or no value with the reason.

The second Task 8 review probes settled these forms. The `.param` function
that a call uses under LOCAL (`ovr_self_combo`, `func_three_local`,
`func_levels_local`, `func_first_inst_local`, `func_sticky_local`): Task 8
took the outer of two instance definitions with no caveat (5k for
x5.xm.xl.r5 of `ovr_self_combo`, where HSPICE gives 7k), and the first fix
tried "the call's scope, then the top level, then up the path", which
`func_levels_local` refutes (2k for x5.xm.xl.r5, where HSPICE gives 5k);
it now takes the nearest definition up the path, then the top level, with
the creation-order rule of `func_sticky_local`. Under GLOBAL the outer one
wins (`func_two_levels_global`), as Task 8 chose. In one scope the last
definition of a function wins (`func_redef`), as Task 8 chose. A parameter
name in the R value place, then `r=` (`rcl_tc_kw_after`,
`rcl_kw_after_more`): HSPICE uses the name and not the `r=`; Task 8 gave
the `r=` value (5k for r6 of `rcl_kw_after_more`, where HSPICE gives 2k)
with a caveat that said that the `r=` wins, and it now gives the name's
value, with TC1 and TC2 from the numbers after the name. These probes agree
with Task 8: the scope of a winning definition's text (`pe_scope_local`),
two `.option parhier` lines (`parhier_dup`, the last wins) and a top-level
`.param` that names a parameter that only a subckt defines
(`pe_scope_global`: HSPICE rejects the deck; Task 8 gives no value, and the
reason now says that HSPICE rejects the deck).

The third Task 8 review probes settled these forms. A quoted expression in
the place of the R, C or L value, then `r=`, `c=` or `l=`
(`rcl_quoted_sticky_local`, `rcl_quoted_more`): HSPICE uses the quoted
expression; Task 8 gave the keyword's value with no caveat (5k for r10 of
`rcl_quoted_sticky_local`, where HSPICE gives 2k), and it now gives the
quoted value. The unit of the rule of `func_sticky_local`
(`func_sticky_ovr_local`, `rcl_quoted_sticky_local`): it is the text that
an instance evaluates, not the definition; Task 8 applied the rule per
definition, which gave 2k for x1.xl.rd of `func_sticky_ovr_local` and for
x5.xl.ro of `rcl_quoted_sticky_local` (HSPICE gives 3k), with a caveat that
said that HSPICE uses the top-level `f`; it now follows the text. Any
earlier instance counts, not only the first one (`func_sticky_ovr_local`),
as Task 8 chose. A function that the body of a top-level function calls and
that only the calling subckt defines (`func_body_undef_global`): HSPICE
rejects the deck; Task 8 gives no value, and the reason now says that
HSPICE rejects the deck. These probes agree with Task 8: a function called
in a function body, and a free name in the body of a function defined in a
subckt (`func_nested_local`; Task 8 gave a caveat that said that this was
not probed, and it now gives none), the breadth-first order and the
function-name unit of the rule of `func_sticky_local`
(`func_sticky_more_local`), parameter names, which do not follow that rule
(`param_sticky_local`), and the order of an override, a body `.param` and a
header default at one level under GLOBAL (`ovr_body_global`).

The Task 8 close-out probes settled these forms. A `.param` function with
the name of a built-in function (`func_shadow_top`, `func_shadow_sub`,
`func_shadow_scope`): HSPICE uses it where it is visible; Task 8 called the
built-in (1k for x1.rs of `func_shadow_sub`, where HSPICE gives 7k) with no
caveat, and it now asks for a `.param` function first. With no top-level
`.param` function of the name, the instance that HSPICE creates first
among those that evaluate the text decides: the built-in after an instance that used the built-in
(x2.xk.rk of `func_shadow_scope`, with a caveat), and a rejected deck after
one that used a `.param` function (`func_shadow_more`,
`func_shadow_more_local`: no value, and the reason says that HSPICE rejects
the deck). A call in a function body (`sticky_lazy_local`): it takes the
nearest definition, with no creation-order rule; Task 8 applied the rule to
the body of the function, which gave 2k for x5.xc.rh and x7.xc.rk (HSPICE
gives 3k) with a caveat, and it now follows HSPICE. These probes agree with
Task 8: a body `.param` that the `X` line of the first instance overrides
does not count for the rule (x1.xl.rp), a text that the first instance does
not use counts (x9.xl.ru, x3.xl.rq), a function argument hides a parameter
of its name (r11, x10.ry, x12.rz), and, under GLOBAL, a free name and a
nested call in a function body and a call in an `X`-line override
(`func_global_more`). The name `h` in the body of a top-level function
(`func_nested_global`): HSPICE reads it as an output variable and does not
abort; the reason of Task 8 said that HSPICE rejects the deck, and it now
says that HSPICE rejects the deck or reads the name as an output variable.

The fifth Task 8 review probes settled these forms. A `$` inside quotes
(`rcl_quoted_dollar`, `rcl_quoted_dollar_line`, and the fixer's probes
`dollar_elem_line`, `dollar_func_body`, `dollar_override`): HSPICE does
not read it as the start of a comment, and it rejects the deck; Task 8
read it as a comment and gave a value with no caveat (4k for r1 of
`rcl_quoted_dollar` and for r2 of `rcl_quoted_dollar_line`), and it now
gives no value, with the reason. `.step` (`step_param`, `step_plain`, `step_lin`): HSPICE
P-2019.06 ignores it with a warning; the Task 8 close-out gave a caveat
that `.step` sweeps the name, and the loader gave the Error
`unknown_command`; it now gives no caveat, and the loader gives the
Warning `unknown_command`. These probes agree with Task 8: a quoted R, C
or L value that goes on on a `+` line (`rcl_quoted_plus`), a built-in name
in a function body (`builtin_body_global`, `builtin_body_global_rev`,
`builtin_local`), and under LOCAL a built-in name that the top level also
defines (`builtin_local`).

The sixth Task 8 review probes settled these forms. A `*` comment line
inside quotes (`quoted_cmtline`, `elem_unquoted_cmtline`): HSPICE rejects
the deck; Task 8 passed over the comment line and gave a value with no
diagnostic (10k for r7 of `quoted_cmtline`), and it now gives no value,
with the reason, and the loader gives the Error `quoted_comment_line`. A
`$` inside quotes now also gives a load diagnostic, the Error
`quoted_dollar` (Task 8 gave only the reason in the answers that evaluate
the text). The argument list of a `.param` function on a `+` line
(`func_args_plus`): HSPICE accepts it; the parser gave the Error
`trailing_text` and skipped the definition, and it now reads it (r1=3k).
These probes agree with Task 8: a parameter name in the C or L value
place, then numbers (`rcl_name_first_cl`) or a model name
(`ind_name_model`), a `+` line after a `*` comment line outside quotes
(`plus_after_cmtline`), a ` *` in an unquoted `.param` value
(`unquoted_space`, `unquoted_space_val`), unquoted expressions
(`unquoted_expr`, `elem_unquoted`), quoted texts that go on on a `+` line
(`quoted_plus_more`), a `'` in a `$` comment and the letter case of
function names (`dollar_quote_comment`).

The 8b-G probes (Task 8 close-out 2) and the seventh Task 8 review probes
settled these forms. A blank line, a line with only spaces and a tab, and a
line with only a `$` comment, inside quotes in a `.param` value or an R
value (`quoted_blank_line`, `quoted_blank_param`, `quoted_spaces_param`,
`quoted_spaces_elem`, `quoted_dollarline_param`, `quoted_dollarline_elem`):
HSPICE rejects the deck with the error of a `*` comment line there (`string
misses dexter quotation mark`). Task 8 passed over a blank line or a line
with only spaces and gave a value with no diagnostic (10k for r7 of
`quoted_blank_line`), and it read a `$` comment line as a `$` inside quotes
(the Error `quoted_dollar`, with the reason of `rcl_quoted_dollar`). It now
treats each line inside quotes that is not a `+` line as it treats a `*`
comment line there: no value, with the reason, and the Error
`quoted_comment_line`. A `+` line with no text inside quotes is passed over
(`quoted_plus_empty_param`, `quoted_plus_empty_elem`), as Task 8 did. A `*`
comment line inside the open parentheses of an unquoted function call in a
`.param` value (`paren_cmtline_call`): HSPICE rejects the deck (`Illegal
character in statement`); Task 8 gave the Error `trailing_text`, because
the parser did not read a call that goes on on a `+` line. The parser now
reads such a call (the next sentences), so the value has no number, with
the reason, and the loader gives the Error `paren_comment_line`. These forms HSPICE accepts, and the tool rejected
them with an Error and skipped the statement: a quoted PWL argument that
goes on on a `+` line (`quoted_plus_pwl`: the Error `bad_source`), a quoted
positional TC2 on `+` lines (`quoted_plus_tc`: the Error `bad_positional`),
a `key=` at the end of a line with its value on the next `+` line
(`kw_value_next_line`: the Error `bad_value`), and an unquoted function call
in a `.param` value that goes on on a `+` line, also after a blank line
(`paren_plus_call`, `paren_blank_call`: the Error `trailing_text`). The
parser now reads each of them, and the answers give HSPICE's values. The
`=` before a value on the next `+` line now also counts as `key=value` in
the reading of R, C and L lines (`after_eq`). These probes agree with the
tool: the text after the closing quote of a `.param` value or an R value
is not used (`quote_tail_param`, `quote_tail_elem`), a quoted TC1 that goes
on on a `+` line (`tc_tnom_dtemp`: r4), outside quotes a blank line or a `$`
comment line before a `+` line (`plus_after_blank`), a blank line or a `*`
comment line in the argument list of a `.param` function and in PWL
parentheses (`paren_blank_args`, `func_args_cmtline`, `paren_blank_pwl`,
`paren_cmtline_pwl`), a `$` comment after a comma in that argument list
(`func_args_cmtline`), the `.op` value of a parameter that a `.dc` or a
`.data` sweep changes (`sweep_op`: the deck value; the sweep caveat said
"the `.op` before the sweep" and "not probed", and it now cites `sweep_op`
and says "the `.op` value"), TNOM from `.option tnom`, the element `dtemp=`
and the element `scale=` together with a TC (`tc_tnom_dtemp`, for R; the TC
caveat now cites it), and a body `.param` whose text names its own name
under GLOBAL with a top-level definition of the name
(`body_self_ref_global`: the top-level value wins).

- `M1 d g s b e 5vnmos`: after 4 nodes, the loader reads model `e` and a positional L of 5, with the letters ignored (as `2umos` in `mos_suffix_letters`). HSPICE rejects a model name that starts with a digit (`mos_digit_model`), so `5vnmos` is not the model of a deck that HSPICE accepts. With 3 nodes (`M1 d g s nch 2umos 1u`), the loader gives the Error `digit_first_model`, because `2umos` can be the model there.
- Two tokens after the R/C/L nodes (for Task 8): `R1 a 0 rval 0.004` is stored as model `rval` and value `0.004`, and `L1 a b lval lmod` as model `lval` and value `lmod`. When no `.model` of the first word is visible, Task 8 reads them as value and TC1 (`r=rval`, `tc1=0.004`, as in `res_tc_positional`), or as value and model (`l=lval`, model `lmod`, as in `ind_value_model`). With a number in the first place, HSPICE reads value and TC1 for R, C and L (`res_tc_positional`; `cl_tc_positional`: `C2 1 0 1p 0.004`, `L2 1 12 1n 0.004`). With a parameter name in the first place, HSPICE reads value and TC1 for R, C and L (`rcl_param_tc`, `rcl_name_first_cl`), value and model for L (`ind_name_model`: `l2 1 12 lval lmod`), rejects a model name second for R (`res_name_model`) and uses a capacitance of 0 for C (`cap_name_model`), as Task 8 reads them.
- Two `.model` cards with one name in one scope, on different lines: the loader gives the Error `duplicate_model`, as for one card read twice (`inc_twice_params`).
- `{...}` in the arguments of a source function (`PULSE({x} 1 0 1n)`), with or without quotes: the loader gives the Error `braces`, as for `{...}` in an element value (`param_braces`). HSPICE aborts on `{a}` in a PWL argument, without quotes (`brace_fn_unquoted`, syntax error) and inside quotes (`brace_quoted_fn`). The other functions were not probed.
- A `}` inside quotes with no `{` (`R1 a b 'a}'`): the loader gives the Error `braces`, as for `{a}` inside quotes. HSPICE reads `}` as `]` (`brace_quoted_param`), but only `{a}` was probed.
- A word that starts with a digit right after an R or C value, with no model name before it (`R1 a b 2k 4mx`, `C1 a b 1p 4mx`): the loader gives the Error `digit_first_model`. After an R or C value, it gives this Error only there. HSPICE reads no model name after an R value (`res_value_model`), and not the order "value, then model" for C (`cap_value_model`), so this is the place of TC1 (`res_tc_positional`, `cl_tc_positional`), where the loader reads a number only with nothing after its scale. In the other places of TC1 and TC2 (TC1 after a model name and a value: `R1 a b rmod 2k 4mx`; TC2: `R1 a b 2k 0.004 4mx`), it ignores the letters after the number, as HSPICE does in the probed number places (`mos_suffix_letters`, `units2`): `4mx` gives 4m. On an L line, a word right after the value with no model name before it is in the place of the model name (`ind_value_model`), and one that starts with a digit gives the same Error (`mos_digit_model`). Before the value, a word that starts with a digit is in the place of the model name, and it gives this Error too (`R1 a b 2n7002 2k`).
- A `+` line with only a comment (`+ $ c`) after the title line, also after `*` comment lines: the loader gives the Error `plus_after_title`, as for other `+` lines there (`title_plus`, `title_plus_gap`).
- A token with a character outside letters, digits, `_`, `.` and `:` before an R/C/L value (`R1 a b rmod-1 2k`): the loader gives the Error `bad_model_name`. Alone (`R1 a b rmod-1`), the token is an expression value.
- A transient function with no space in it right after the AC magnitude (`AC 1 SIN(0,1,1meg)`): the loader reads it as the function, not as the AC phase.
- An include cycle (`a.sp` includes `b.sp`, which includes `a.sp`): the loader does not follow the include that closes the cycle, in document order, and gives the Error `include_cycle` at that include line. It compares the paths in canonical form, so `main.sp` -> `sub/b.sp` -> `../main.sp` is a cycle too.
- A failed block statement (a `.subckt`, `.data` or `.lib <section>` line with an error) whose closing line (`.ends`, `.enddata`, `.endl`) comes only after the `.end` of the top file: the loader reads no line after that `.end`, as for every `.end` of a top file (`after_end`; a `.end` line with text after the keyword too, `end_quoted_arg`), so the block has no closing line. As for a failed block with no closing line at all, the loader skips only the failed line: the body lines up to the `.end` are read in the enclosing scope (`* t` / `.subckt` / `R1 a b 1` / `.end` / `R2 a b 2` / `.ends` / `R3 a b 3` loads R1 at the top level), with only the Error of the failed line. A `.subckt` nested deeper than 256 levels (`nesting_too_deep`) is different: its lines are skipped up to its `.ends`, or up to the `.end` when no `.ends` comes before it. Not probed: HSPICE may report the missing `.ends`.
- A `.end`, `.subckt` or `.ends` line inside a `.prot` region or inside a `.lib <section>` definition of the file: the loader does not read the text there, so the line does not end the top file, and it does not open or close a block for the skip of a failed block statement (`* t` / `.lib tt` / `R1 a b 1` / `.end` / `.endl tt` / `R3 a b 3` loads R3, with no Info `after_end`). A `.lib <section>` definition that does not parse (text after its `.endl`) is read line by line, so a `.end` in it ends the top file.
- A `.temp` line with no value (`.temp`): the loader keeps the line, gives no temperature, and gives the Warning `temp_not_read`. The loader reads the first value of a `.temp` line as the model selector does (`first_temp` in `src/eval.rs`), so `.temp=25` gives 25. `.temp (25)` is a syntax error.
- A `.end` line with text after the keyword in an included file (`.end 'x'`), and `+` lines after a `.end` line: the loader reads `.end` and passes over the rest of its logical line (`+` lines included), as HSPICE does with the text after `.end` in a top file (`end_quoted_arg`, `end_kv_arg`). In an included file, it ignores the line with the Warning `end_in_include`, as for `.end` (`end_in_include`).
- `R` after a PWL, in forms that were not probed. The loader stores a bare `R` as `R=0`, as in the probed forms (`pwl_repeat`, `pwl_repeat_td`, `pwl_repeat_noparen`, `pwl_repeat_key_bare_mid`, `pwl_ac_r`, `pwl_r_param_last`). It also accepts `R` later on the line (`PWL(...) TD=1n R`), once. A second bare `R` (`PWL(...) R R`) and `R` after another function in parentheses (`PULSE(...) R`) give the Error `trailing_text`. After the repeat flag, or with no PWL before it, a word `R` after the AC magnitude is the AC phase (a parameter name). Without parentheses, a parameter named `r` in the middle of the PWL values (`PWL 0 r 1n 1`) ends them, as the last word does (`pwl_r_param_last`), so the values after it give the Error `trailing_text`.
- The points of a PWL, in forms that were not probed: the loader gives the Warning `pwl_odd_values` for every odd number of values (`pwl_r_param_last` probed 3 values), and the Warning `pwl_repeat_dropped` for a repeat time `R` at or after the last time point (`pwl_repeat_key_only` probed `R=` at the last point). It checks `R` only when `R` and the last time point are plain numbers, and it counts the values up to a word `R` or a `key=value` word inside the parentheses. It keeps the values and `R` as written.
- `{...}` in an analysis command other than `.temp` and `.meas` (`.tran 1n '{a}'`, `.dc`, `.ac`, `.save`, ...): the loader gives the Error `braces` and skips the line, as for `.temp` and `.meas` (`brace_temp`, `brace_quoted_temp`, `brace_meas`). Every `{` or `}` outside comments counts, also inside quotes and also where the rest of the line reads as tokens. A `.print` or `.probe` line with `{...}` is kept, with the Warning `braces_output`, as HSPICE ignores the output with a warning (`brace_print`, `brace_probe`, `brace_probe_quoted`); `.print v('{a}')` (quotes in a `.print`) was not probed.
- An `X` line with no visible definition on an instance path from the root, which HSPICE rejects (`hier_top_nested_only`, `hier_unres_in_subckt`): the loader gives one Error `unresolved_subckt` for each such `X` line, with the number of instance paths it affects, and still gives the copies and the expanded totals without those instances, with a note that the totals are a lower bound. HSPICE gives no totals.
- A call cycle on an instance path from the root, which HSPICE rejects (`hier_reentry`, `hier_reentry_unscoped`): the loader gives the Error `subckt_cycle`, also for a cycle whose expansion never ends (a subckt that calls itself with no nested definition to stop it), which was not probed. The copies of the definitions in the cycle and below the first definition that comes back, and the expanded totals, are `null` with a note. A cycle that no instance path from the root goes through makes no instances: its definitions have 0 copies, and the loader gives the Warning `subckt_cycle` (HSPICE runs such a deck: `hier_unused_cycle_unres`).
- `top` set to a nested definition (`top=outer>deep`): its `X` lines look up names as if it were instantiated inside an instance of each definition that it is written inside, nearest first, then at the top level (`xc` in `outer>deep` uses `outer>cell`). HSPICE has no `top` option.
- A definition that no instance path from the root reaches (0 copies), and every definition of a bare netlist (no root): HSPICE does not check its `X` lines (`hier_unused_cycle_unres`, `hier_unused_nested_unres`, `hier_unused_nested_cycle`). The loader checks them in imagined contexts. First, each such definition whose name no `X` line calls is checked as if instantiated at the top level (a nested definition: inside an instance of each definition it is written inside, nearest first, as for `top`), and the definitions below it in those imagined contexts too. Then each such definition that no check reached (in a call cycle with no entry, or called only by a name that binds to another definition) is checked in the same way, in document order (only this step depends on the order of the definitions in the deck). So a definition that only unused definitions call is checked as called from them, not on its own at the top level (`u2` called by the unused `u1`, which nests `only`: `u2`'s `xq p only` binds to `u1>only`, with no Warning). These checks give `x_lines`, `binds_to`, the lines of `instances_of` (with 0 copies) and the Warnings `unresolved_subckt` and `subckt_cycle`, and they add no copies. For a definition that the root reaches, `binds_to`, `instances_of`, `element_info` and its `unresolved_subckt` diagnostics use its instance paths from the root only (`hier_unused_caller_reach`). Two things about it can still come from an imagined context: its `x_lines` counts the `X` lines of unused definitions that bind to it, and a call cycle found in an imagined context gives the Warning `subckt_cycle`, which can name it (the used `a` calls `b`; the unused `spare` nests a `b` that calls `a`, and calls `a`: the Warning names `` `a` (`xb`) -> `spare>b` (`xa`) -> `a` ``).
- `m` <= 0 in forms that were not probed. The loader checks `m` only when its text is a number with an optional sign (`m=0`, `m=-1`, `m='0'`); an expression (`m=mz` with `.param mz=0`, `m='1-1'`) is not evaluated here (parameter resolution, Task 8). It checks the R, C, L, M, Q, D and `X` lines; only R and `X` were probed. It gives the Error `m_not_positive` for each such line on an instance path from the root, also for the lines that HSPICE does not report once it has rejected the deck (the top-level `r3` of `hier_m_zero`), and the Warning `m_not_positive` for a line that no instance path reaches (`elem_m_zero_unused`). When the hierarchy analysis stops (`hierarchy_too_large`), it gives the Error for every such line. The counts still count these lines and the instances of such an `X` line.
- Two element lines with one name in one definition, in forms that were not probed. The loader checks every element type (only `X` lines and resistors were probed). It gives the Error `duplicate_element` for each later line with the name of an earlier line of the same definition on an instance path from the root, also for the lines that HSPICE does not report once it has rejected the deck (the two `xq` lines of `wrap` in `hier_dup_instance`), and the Warning `duplicate_element` in a definition that no instance path reaches (`hier_dup_instance_unused`, `elem_dup_name_unused`). An element read twice (its file included twice, as in `inc_twice`) gets the same diagnostics. When the hierarchy analysis stops (`hierarchy_too_large`), it gives the Error for every such line. Queries use the first line (`element_info` gives a caveat), and the counts count every line.
- Instance paths with a `.` in names, in forms that were not probed. The walk follows HSPICE's creation order (`hier_dotted_collision*`): the first instance created with a full name owns it, and the instances below a later instance keep full names. An element path is split at its last `.` (`elem_dotted_name_collision`): the device of that name in the first instance with the full name before it. A device that only a later instance with that full name holds has no path: `element_info` gives `BadArgument` with the reason (`hier_dotted_collision_hidden`). Not probed: a device name with a `.` (`ra.rb`) when no instance has the full name before the last `.` of the path, or when the first such instance has no element of that name: the loader answers with that device and a caveat that HSPICE may not find it. An `X`-line path that names both an `X` line and a device (`x1.r2` for the top-level `X` line `x1.r2` and the `r2` of `x1`): `element_info` gives the device (the element rule), and a scope path (`resolve_param`) the instance, with a caveat. An `X` line name of more than one line of one definition (`duplicate_element`, which HSPICE rejects): the walk uses the first line, so a path through the second one is not found. The walk keeps one instance for each (place in the path, definition, lookup context) and counts the instances it does not keep in the shared-name caveat; a lookup context keeps only the scopes that are the nearest one for a name they nest. A path with more than 4 × (definitions + element lines) + 4M instances and context entries gives `BadArgument` (only when nested definitions are used in very many combinations, as for `hierarchy_too_large`).
- An `X` line with no visible subckt definition on an instance path, whose callee is the name of a `.model` card that is visible from the line (in its definition, in a definition it is written inside, or at the top level; the nearest one) and whose type is not a built-in HSPICE type (`amp c core csw d l lpnp nmf njf nmos npn opt pjf plot pmf pmos pnp r s sp sw u w`), or that has no visible model card in a deck whose first run has a `.hdl` line: the loader gives the Warning `unresolved_subckt` (the callee can be a Verilog-A module, or a model of one, which the loader does not read), not the Error, and the summary note says that these lines are counted neither as instances nor as devices. A visible model card of a built-in type gives the Error (`hier_model_callee`), also when the deck has a `.hdl` line, and so does a `.hdl` line in an `.alter` block only. Not probed: `hspice.exe`, which `run_hspice.sh` calls, runs no Verilog-A (`*pvaE* Please invoke hspice script instead of binary`, job aborted), and the visibility of a model card in a definition on the instance path that the line is not written inside.
- The node count of an `X` line when the hierarchy analysis stops (`hierarchy_too_large`): the bindings are not known, so the loader does not check it (no `port_count_mismatch`).
- Parameter functions (`.param f(a,b)=...`), in forms that were not probed (Task 8). The probed rule: under GLOBAL a call uses the top-level definition first, then the definitions down the instance path (`func_scope`, `func_two_levels_global`); under LOCAL the nearest definition up the instance path (the scope of the call first), then the top-level one (`ovr_self_combo`, `func_three_local`, `func_levels_local`, `func_first_inst_local`). Under LOCAL, when an instance that HSPICE creates before this one (breadth-first) evaluated the same text, found no definition above it and so took the top-level one, this instance takes the top-level one too (`func_scope_local`, `func_sticky_local`, `func_sticky_more_local`, `func_sticky_ovr_local`, `rcl_quoted_sticky_local`; an inference from these probes). Any such earlier instance counts, not only the first instance (`func_sticky_ovr_local`). The instances that evaluate a text: for an element line, each instance of its definition; for a `.subckt` default or a body `.param` of a name, each instance whose `X` line does not override the name (`func_sticky_ovr_local`, `sticky_lazy_local`: x1.xl.rp); for an `X`-line override, each instance that this `X` line makes. An instance counts also when it does not use the text (`sticky_lazy_local`: x9.xl.ru, and x3.xl.rq for an untaken ternary branch). A call in a function body has no such rule: it takes the nearest definition from the level of the function's definition, under GLOBAL in the GLOBAL order (`sticky_lazy_local`: x5.xc.rh, x7.xc.rk). A `.param` function with the name of a built-in function (`max`, `sgn`, ...) hides it where it is visible, and the built-in is used where none is visible (`func_shadow_top`, `func_shadow_sub`); with no top-level function of the name, the instance that HSPICE creates first among those that evaluate the text decides the kind of function (`.param` or built-in) for the later ones, in both modes: the built-in after an instance that used the built-in (`func_shadow_scope`: x2.xk.rk), and a rejected deck after one that used a `.param` function (`func_shadow_more`, `func_shadow_more_local`). The instance that uses a `.param` function before such a later instance (x7.xk.rk2 of `func_shadow_more`) keeps its value, with a caveat that HSPICE rejects the deck (the listing names only the later instance). A built-in name in a function body takes the nearest `.param` function from the level of the definition, else the built-in, with no creation-order rule (`builtin_body_global`, `builtin_body_global_rev`, `builtin_local`: (4)), and under LOCAL a built-in name that the top level also defines follows the rule of `func_sticky_local`, as other names do (`builtin_local`: (1), (2)). Task 8 finds that instance with a breadth-first walk of the hierarchy graph, with a caveat; when the hierarchy was not analysed (`hierarchy_too_large`) or the walks of one call read more than 4000000 `X` lines, it takes the nearest definition with a caveat. Calls of functions that the same subckts define share one walk. In one scope the last definition wins, as for `.param` values (`dup_param`, `func_redef`). The names in the body of a function that are not arguments, and the functions that it calls, are resolved where the function is defined (`func_free_local`, `func_nested_local`). A function that calls itself has no value. A call inside an `X`-line override uses the scope of the new instance, as the rest of the override text does (`ovr_scope_local`). A function argument hides a parameter of its name (`sticky_lazy_local`: r11, x10.ry, x12.rz; `func_global_more`: r2, x3.ry). A call of a function that no scope of the call defines has no value, and the reason says that HSPICE rejects the deck or reads the name as an output variable (`func_body_undef_global`, `func_nested_global`). A `.param` function with the name of a statistical function hides that function too, on an R line (`stat_user_func`) and on an M line: with `.param unif(a,b)='(a+b)/2'`, `M1 d g 0 0 nch l='unif(1u,3u)' w=1u` gives L=2u with no warning, and so do the same decks with `agauss` and `limit` in place of `unif` (decks `s_unif`, `s_agauss`, `s_limit`, run on 2026-11-07, not in `golden/cases`). The query tool and the model selector give L=2u.
- A `$` inside quotes in the forms that were not probed (Task 8): a `.subckt` default, a `key=value` of an element, a source-function argument (`PWL(0 '0.3 $ c' 1n 1)`), and a quoted text other than an R value that goes on on a `+` line after a `$` in it: the value has no number, and the reason says that HSPICE rejects the deck, as in the probed places (`rcl_quoted_dollar`, `rcl_quoted_dollar_line`, `dollar_elem_line`, `dollar_func_body`, `dollar_override`). The text that answers show keeps the `$` and the text after it. A `$` outside quotes starts a comment. A line with only a `$` comment inside quotes is not a `$` inside a quoted text: it is a line that is not a `+` line (`quoted_dollarline_param`, `quoted_dollarline_elem`; the next lines). The loader gives the Error `quoted_dollar` for each such text that it stores (a `.param` value, a function body, a `.subckt` default, a value or `key=value` of an element line, the arguments of a source function, and on an R, C or L line whose value comes from a later `r=`, `c=` or `l=` the rest of the line after the nodes, so also the quoted value that the keyword replaces), also in a definition that no instance path reaches and in an `.alter` block (not probed: HSPICE may check such a text only when it evaluates it). It does not look at the title line, comment lines, `.prot` regions, `.option`, `.temp`, `.data` and analysis lines, or `.lib` sections that no call reads.
- A line inside quotes that is not a `+` line, in the forms that were not probed (Task 8): a `*` or `$` comment line, a blank line or a line with only spaces, in a function body, a `.subckt` default, an `X`-line override, a `key=value` of an element, a quoted value that a later `r=` replaces, a quoted TC and a source-function argument (`PWL(0 '0.3` / (blank line) / `+ *1' 1n 1)`). HSPICE rejects a deck with one in a quoted `.param` value or R value (`quoted_cmtline`, `elem_unquoted_cmtline`, `quoted_blank_line`, `quoted_blank_param`, `quoted_spaces_param`, `quoted_spaces_elem`, `quoted_dollarline_param`, `quoted_dollarline_elem`), so the value has no number and the reason says that HSPICE rejects the deck, and the loader gives the Error `quoted_comment_line` for the texts of the `$` line above. A `*` comment line with a `'` in it (`elem_unquoted_cmtline`) on an R line: the parser rejects the line instead (the Error `trailing_text`) and skips it. The text that answers show keeps such a line on a line of its own. A `+` line with no text inside quotes is passed over in every place, as in the probed `.param` value and R value (`quoted_plus_empty_param`, `quoted_plus_empty_elem`). Outside quotes, a `*` or `$` comment line and a blank line between `+` lines are passed over (`plus_after_cmtline`, `plus_after_blank`), except a `*` comment line inside open parentheses (the next line).
- Line breaks inside the open parentheses of an unquoted expression, in the forms that were not probed (Task 8). The parser reads a group in parentheses that goes on on a `+` line, after blank lines and comment lines, in every unquoted value (an element value, a `key=value`, an `X`-line override, a `.subckt` default, a function body), as in the probed `.param` value (`paren_plus_call`, `paren_blank_call`); it does not read a space inside the group, as on one line. A `*` comment line there: the value has no number, the reason says that HSPICE rejects the deck, and the loader gives the Error `paren_comment_line` for the texts of the `$` line above, as for the probed `.param` value (`paren_cmtline_call`). The same holds for parentheses inside the argument of a source function (`PWL(0 max(0.3,` / `* c` / `+ 0.4) 1n 1)`); the parentheses of the function itself are not in its argument text, so a `*` comment line there is passed over (`paren_cmtline_pwl`). A `$` comment line and a `$` comment at the end of a line inside the parentheses are passed over, as a blank line is (`paren_blank_call`; a `$` after a comma in the argument list of a `.param` function, `func_args_cmtline`).
- A `key=` at the end of a line with its value on the next `+` line (after blank lines and comment lines), in the places that were not probed (Task 8): every `key=value` that the parser reads (`.param a=` / `+ 5k`, `.option`, an `X`-line override, the parameters of every element), as for the probed `r=` of an R line (`kw_value_next_line`).
- A quoted TC1 or TC2 after an R, C or L value (`R1 a b 1k '0.004'`, also on `+` lines), in the forms that were not probed (Task 8): the parser keeps it as an expression, as for the probed quoted TC2 after a parameter name (`quoted_plus_tc`). An unquoted expression there (`R1 a b 1k a*2`) is still the Error `bad_positional`.
- A statistical function (`agauss`, `gauss`, `aunif`, `unif`) in an `X` line override (`x1 a b cell w='agauss(1,0.1,3)'`): Task 8 reads it as an element expression, so it has no value, as in `stat_direct`. Only a `.param` (`stat_nominal`, the nominal value with a caveat) and an element value (`stat_direct`) were probed.
- The `.model` card of an R, C or L line (Task 8): a card is visible from the definition that holds the line, from each definition that it is written inside, and from the top level, as for the definition of an `X` line (Task 7). Only top-level cards were probed (`res_model`).
- After a name in the R/C/L value place that names no visible `.model` card, Task 8 moves the numbers one place on, as HSPICE does for R, C and L (`rcl_param_tc`: `r3 3 0 rval 0.004 1e-5` reads TC1 0.004 and TC2 1e-5; `rcl_name_first_cl`: `c2 2 0 cval 0.004 1e-5`, `l4 1 14 lval 0.004 1e-5`; `ind_name_model`: `l3 1 13 lval lmod 0.004` reads model `lmod` and TC1 0.004). Not probed: a third number (it has no place, and the caveat says that it is not used), and a keyword `tc1=` or `tc2=` later on the line, which wins over the number in its place (the last value wins, as in `elem_dup_kw`; with `R1 a b rval 0.004 1e-5 tc1=3e-3`, TC1 is 3e-3 and TC2 1e-5). A name before `r=` is the value, and the `r=` is not used, also with numbers between them, which are TC1 and TC2 (probed for R: `rcl_tc_kw_after`, `rcl_kw_after_more`, and with a quoted TC1 that goes on on a `+` line, `tc_tnom_dtemp`: r4). A quoted expression right after the nodes, then `r=`, `c=` or `l=`, is the value in the same way (probed for R, C and L, also with a quoted number: `rcl_quoted_sticky_local`, `rcl_quoted_more`). Task 8 reads the numbers between a quoted value and the keyword as TC1 and TC2, a model name after a quoted value on an L line as the model, and a quoted value after the name of a visible model card (`C1 a b cmod 'cval' c=5p`) as the value, each with a caveat (not probed). Task 8 reads a name before `c=` or `l=` in the same way, with a caveat that says that this was probed for R only, and on an L line a name after the name in the value place is the model (`L1 a b lval lmod 0.004 l=5n`, as in `ind_value_model`). With a model name after a number value (`L1 a b 1n lmod l=5n`), `l=` replaces the number, as in `elem_dup_kw`. A name, then the name of a visible model card, is a value, then a model, which HSPICE rejects for R (`res_name_model`) and reads with a capacitance of 0 for C (`cap_name_model`): the value has no number, and the reason says so. For L it is a value, then a model (`ind_value_model`).
- The TC caveat (Task 8) uses the temperature plus `dtemp=` of the line, and TNOM from `.option tnom` (else 25). With the element's `scale=`, the TC caveat applies the TC to the scaled value. `res_tc_positional`, `cl_tc_positional` and `rcl_param_tc` probed `.temp 125` with no `dtemp` and no `tnom`. `tc_tnom_dtemp` probed `dtemp`, `tnom` and `scale` for R, and HSPICE agrees with each choice: `.option tnom=50` gives TNOM 50, `dtemp=20` adds 20 to the temperature of the element (and `dtemp=-75` at `.temp 125` puts it at TNOM), and `1k tc1=0.004 scale=2` gives 2k*(1+0.004*75). Not probed: `dtemp`, `tnom` and `scale` with a TC on a C or L line.
- Several temperatures (`.temp 25 50`, or two `.temp` lines): `temper` is the first value of the first run, and the answer gives a caveat. Only one `.temp` value (`temper`) and none (`temper_default`) were probed.
- A V or I line with no DC value and no function: `dc` is 0 with a caveat. With no DC value, a function other than PULSE, EXP, SFFM, SIN, PWL and AM (for example PAT) has no `dc` value.
- Parameter references (Task 8): a cycle (`.param a=b b=a`) has no value, and the reason names the cycle. References nested more than 256 levels deep, expressions whose heights add up to more than 2048 along one evaluation, and a value that needs more than 100000 evaluations in one call (functions called with very many different arguments) have no value. Such a failure depends on where the limit was met, so it is not reused within the call (`r1 1 0 p300 tc1=p100` still resolves `tc1`). A chain lists at most 500 steps, with a caveat.
- `.option scale` other than 1 (Task 8): `resolve_element` gives the geometric parameters (`w`, `l`, `ad`, `as`, `pd`, `ps`, `area`, `pj`; not the inductance `l` of an inductor) as written, with a caveat that HSPICE multiplies them by the scale.
- The element `scale=` (Task 8): the R, C and L value is the value as written times `scale=` (`src_delay_scale`, `elem_scale_cl`), with a caveat; with a model card, the card's parameters are not read (not probed together).
- `X`-line overrides, in forms that were not probed (Task 8). The text is evaluated in the scope of the new instance (`ovr_scope_local`, `ovr_scope_child_global`, `ovr_scope_more_local`), and the overridden name itself above it (`ovr_self_local`); with no definition of that name above the instance, the value has no number and the reason says that HSPICE rejects the deck (`ovr_self_global`). Under GLOBAL with a definition above the instance, that definition wins and the override text is not evaluated, as for any override under GLOBAL (`parhier_global`, `nested_global`); whether HSPICE still checks the text there was not probed. `resolve_element` on the `X` line gives the override's own value, with a caveat that names the winning definition and the value that the instance uses (`resolve_param` gives that value). `m` is evaluated in the scope of the line (`ovr_scope_more_local`). An `X` line with no visible definition (`unresolved_subckt`): an override that is not a number has no value, since the scope of the new instance is not known. `m` <= 0 from an expression on an `X` line of the path, or on the element, gives a caveat (`m_expr_zero` probed an `X` line).
- `top` set to a nested definition (`top=outer>deep`): its scopes are the top level, then each definition it is written inside (outermost first), then itself, as if instantiated in an instance of each with no overrides, so the header defaults and `.param` lines of `outer` are visible. A chain step of these scopes has the scope `(outer)` or `(outer>deep)`, not `""`. HSPICE has no `top` option.
- `.alter` blocks (Task 8): answers describe the first run. A caveat names each `.alter` block that defines again a `.param` that the value uses, a `.param` function it calls, the `.temp` (in `temper` and in the TC caveat), `.option tnom` or `.option scale` (in their caveats), `.option parhier` (on every answer of the deck: a later run can take other definitions), the model card of the element, a definition of the path, an `X` line of the path, or the element (`alter` probed `.param` and element changes). Under LOCAL a definition below the top level gets no caveat for a top-level `.param` of an `.alter` block, as it wins in every run.
- DSPF files (Task 10). A file is a DSPF when a line of its first 50 starts with `*|DSPF`; a DSPF in a `.lib` section is not scanned. The scanner reads lines, not statements: a `*|` record or an R/C value on a `+` or `*+` line is not read. A `*|NET` block ends at the next `*|NET`, at `.ENDS`, or at an element line that is not R or C (the instance section); its R and C lines are parasitics, not devices (`dspf_included`: `# resistors` 6 = 5 parasitic + `rload`). The logical net of a block holds the net, the first name of each `*|P`, `*|I` and `*|S` record, and both nodes of each of its resistors; a capacitor's nodes are not mapped (its second node is ground or another net). A block with no parasitic belongs to the definition of the file's first `.SUBCKT`, else to the top level. `.option sim_dspf="<scope> <file>"`: the file is found next to the file of the `.option` line; a scope that is a subckt name gives the parasitics to every instance of it, else the scope is an instance path (`/` read as `.`); no scope is the top level (probed: `inv` and `xi` both match `XI/A`, `@inv` is rejected, `dspf_backannot`). A DSPF net name with the divider (`XI/A`) is matched only as a plain name. The DSPF `*|DIVIDER`, `*|DELIMITER` and `*|BUSBIT` lines are not used: names are never split (spec 6.1a).
- The parameters of a `.model` card (`resolve_model`). Each `key=value` of the card is evaluated where the card is defined, as a text that every instance of that definition evaluates: a top-level card with the top-level parameters, a card inside a subckt with the parameters of that subckt's instance on the element's path (and of the corner that the `.lib` lines load). Probed only for the limits of a bin (decks `q_limits_mac_*`, `q_limits_nested_*`, `q_limits_notop_local` of `scripts/hspice_kit`), with the known gap that the first MOSFET that looks at a bin evaluates its limits in its own instance; an answer gives a caveat for each name that the element's level sees otherwise. The card of a MOSFET is the one that the model selector chooses (`query/tests/selector_agree.rs` checks the choice on decks with bins, `.option scale`/`scalm`/`defl`/`defw`/`wnflag`, level 72, cards inside subckts and errors); the card of an R, C, L line, a diode or a BJT is the nearest card of its name on the instance path, as for a MOSFET (not probed for these elements). A key that a card sets twice gives both items, each with a caveat (a bin limit uses the first, deck `mdl_dup_keys`; an `L` card the last `level`, `ind_level_twice`). A statistical function in a card gives its nominal value, as in a `.param`. `query/tests/cards.rs` pins these choices. The kit `scripts/hspice_kit/cards` (20 decks, with netlist-db's answers and the value of each other rule) asks HSPICE about each of them. It ran on 2026-11-08: see "The card kit" in the section of the comparison rounds.

## Differences from the plan and spec

- Plan Task 2 expects `2**3**2` = 512 (right-associative). HSPICE gives 64 (left-associative).
- Plan Task 2 expects `1e-3k` = 1.0. HSPICE gives 0.001.
- Plan Task 2 tests `pow(2,3)` only. HSPICE `pow` truncates the exponent (`pow(2,0.5)` = 1).
- Plan Task 3a-E gives a `Domain` error only where HSPICE aborts on a zero base or on a unary minus after `**`/`^`. HSPICE aborts on none of the `ops2` forms, so none of them is a `Domain` error: a zero base gives 0 (`0**0` = 0, `0**-1` = 0), and `2**-3**2` = 0.015625.
- Spec 6.2a `title_line: auto` keeps line 1 when it starts with `.`. HSPICE ignores line 1 of the top file in all cases (`title_dot`).
- Review focus 2 expects "HSPICE's winner" for duplicate `.subckt` names. HSPICE has no winner: it aborts (`dup_subckt`).
- Plan Task 7 builds lexical `X`-to-definition lookup ("innermost enclosing definition's nested defs first, then outward, then global") and confirms it with `nested_def`. `nested_def` cannot tell lexical from dynamic lookup. HSPICE uses the instance path (dynamic lookup): `nested_def_dynamic` gives x1.xd.xc.rc=2 (a lexical rule gives 1), and `nested_def_reach` gives x1.xu.xd.xc.rc=3 (a lexical rule gives 2) and resolves `only` from the top-level `t` (a lexical rule finds no definition). Effects on Task 7: (1) the binding of an `X` line can depend on the instance path, so `copies(child) = Σ copies(parent) × calls` over one fixed definition graph is not correct. In `nested_def_dynamic`, the one `xc` line of `deep2` gives 1 copy of the top-level `cell` and 2 copies of `outer`'s nested `cell`. Copies must be counted along instance paths, or per (definition, visible nested definitions). (2) Two nested definitions with the same name are different definitions. (3) An `X` name with no top-level definition is not an error by itself.
- Plan Task 6a-G probes the inductor orders with `.model lmod l`. HSPICE aborts on that model card for an `L` element (`ind_model_nolevel`: its level must be 3). `ind_value_model` and `ind_model_value` use `.model lmod l level=3`.
- Plan Task 6a-G asks one case `mos_digit_model` for `5vnmos` and `33nmos`. HSPICE aborts on each of them at the `.model` line, so each has its own case (`mos_digit_model`, `mos_digit_model_33`). Plan Task 6a-P `digit_first_model` reads `5vnmos` and `33nmos` as names. In the probed forms, HSPICE accepts no deck with such a model name: it rejects the model card, and it rejects an `M` line with `5vnmos` in the model place (`mos_digit_model_ref`, syntax error). 6a-P also reads a bare token as a positional number only with nothing after the scale suffix. Where HSPICE expects a number, it ignores the letters after the number: `1V` = 1 (`units`), `1mohm` = 1e-3 (`units2`), and `2umos` in the positional L place = 2u, with no warning (`mos_suffix_letters`). So for a deck that HSPICE accepts, the 6a-P rule can read a token differently from HSPICE. `5vnmos` and `33nmos` in a number place were not probed.
- Plan Task 6a-P `param_braces` groups `{...}` like quotes in `.param` values and element values. HSPICE P-2019.06 rejects `{...}` in both places (`param_braces`, `param_braces_param`), also inside quotes (`brace_quoted_param`, `brace_quoted_elem`, `brace_quoted_fn`).
- Plan Task 7 close-out keeps only the first instance for each place in the path, because "nothing below a later instance with that full name has a path". HSPICE gives full names below it (`hier_dotted_collision_below`: `i(x0.xa.xb.xk.rk)` finds the `rk` below the later `x0.xa.xb`). So the walk keeps one instance for each (place in the path, definition, lookup context); a later identical one has the same instances below it, all created later.
- Plan Task 7 says that an `X` name with no visible definition on some path is a Warning, not a load error, and that a cycle is an Error. HSPICE rejects a deck with an `X` line that has no visible definition on an instance path from the root (`hier_top_nested_only`, `hier_unres_in_subckt`), so the loader gives an Error there, and a Warning only in a definition that no instance path reaches, where HSPICE runs the deck (`hier_unused_cycle_unres`). A cycle that no instance path goes through is a Warning too. HSPICE also rejects a definition inside an instance of itself when the dynamic lookup would end the expansion (`hier_reentry`, `hier_reentry_unscoped`), so a "cycle" is one definition twice on one instance path, not only an expansion that does not end.
- Plan Task 6a-G asks for one case each for `inc_twice_params` and `lib_twice`. Both abort on a duplicate definition, so `inc_twice_param_only` and `lib_twice_param_only` show the accepted form: a repeated `.include` or `.lib` section call is read again, never ignored. The controls `inc_once_param_only` and `lib_once_param_only` show that HSPICE reads the file or section in place, at the call.
- Plan Task 6a-P `rcl_order` accepts "the R/C/L model and value orders that HSPICE accepts" and rejects the others. HSPICE reads the value in the order "value, then model" only for L (`ind_value_model`). For R it aborts (`res_value_model`: `2k` is a third node). For C it does not abort, but it does not read the value (`cap_value_model`): `C1 1 2 1p cmod` gives node2 `1p` and capacitance 0, with only a warning. So if 6a-P takes this C order as accepted and stores c=1p, it gives a value that HSPICE does not use, with no error.
- Plan Task 6a-P `source_forms` accepts the source forms "in each order that HSPICE accepts". The probes show these orders: DC first (`src_forms`) and AC before DC (`src_order_ac_first`) are accepted, and a function before `DC` is rejected (`src_order_fn_first`, `src_fn_dc_more`, `src_fn_dc_more_noparen`). Other orders were not probed (see `src_order_fn_first`). With no DC value, the `.op` value of each probed function is its value at t=0, not always its first argument (`src_fn_nodc`: PWL gives 0.4 from `PWL(0 0.4 ...)`, AM gives 0; `src_sin_t0`: `SIN(0.2 1 1meg 0 0 90)` gives 1.2, while `.option list` `dc volt` shows 0.2).
- Plan Task 6b-G asks one case `src_fn_dc_more` for `SIN(0 1 1meg) DC 0.7` and `PULSE 0 1 0 1n 1n 5n 10n DC 0.7`. HSPICE aborts on each of them, so each has its own case (`src_fn_dc_more`, `src_fn_dc_more_noparen`).
- Plan Task 6a-P `after_end` says "If HSPICE ignores the lines, the parser stops the top file at `.end`". Lines with no analysis after `.end` change nothing in the first job, and the listing shows no second job (`after_end`). A complete deck after `.end` is not ignored: it runs as a second job (`after_end_second_deck`). Its elements are not part of the first job. The loader loads the first job only: its Info `after_end` counts the lines after `.end` and says that a second deck (a second job in HSPICE) is not loaded.
- Plan Task 8 evaluates the override `I_j` in level j−1 (the parent's scope). HSPICE evaluates it in level j, the scope of the new instance (`ovr_scope_local`: x1.r1=6 with the new instance's default `w=2`, where level j−1 gives 12; `ovr_scope_child_global`, `ovr_scope_more_local`), except for the overridden name itself, which comes from level j−1 (`ovr_self_local`: `w='w*3'` gives 12), and `m`, which is evaluated in the line's scope (`ovr_scope_more_local`).
- Plan Task 8 leaves the scope of `.param` functions open ("top-level ones are visible everywhere; a function defined inside a subckt is visible in that subckt"). Under GLOBAL HSPICE takes the top-level `f` before a subckt's own `f` (`func_scope`), and the outer of two instance definitions (`func_two_levels_global`). Under LOCAL it takes the nearest definition up the instance path, then the top-level one (`func_three_local`, `func_levels_local`), except when an instance created before this one evaluated the same text and took the top-level one (`func_scope_local`, `func_sticky_local`, `func_sticky_ovr_local`). A call in a function body has no such exception (`sticky_lazy_local`). A `.param` function with the name of a built-in function hides it where it is visible (`func_shadow_top`, `func_shadow_sub`, `func_shadow_scope`, `func_shadow_more`).
- Spec 6.2a asks for a caveat on a parameter that a `.dc`, `.data` or `.step` sweep changes. Task 8 did not give it; the Task 8 close-out does. The loader keeps the names that the analysis lines of the first run sweep: the first word of `.dc` and the word after the values of its first sweep (three values: `.dc v1 0 1 0.1 v2 0 1 0.5`; a type and three values: `.dc v1 lin 11 0 1 v2 lin 3 1k 3k`, also `dec` and `oct`; `poi`, the count and that many values; or `start=`, `stop=` and `step=`), the word after `sweep` in `.dc`, `.tran`, `.ac` and `.noise`, and the parameters of a `.data` block that a `data=` names. Not `.step`: HSPICE P-2019.06 has no `.step` command and ignores it with a warning (`step_param`, `step_plain`, `step_lin`). An answer that uses a swept top-level `.param` (also through a chain), the temperature (`temp`: through `temper` or TC1/TC2), or the DC value of a swept top-level source gets the caveat "`name` is swept by .dc at file:line; this is the nominal value". The value is the one of the deck, which the `.op` block gives (`sweep_op`): `.param pv=2k` with `.dc pv 1k 3k 1k` gives r1=2k, `.param pq='pv*2'` gives r3=4k (in the `.dc` results `pq` follows `pv`, so a name swept through a chain is swept too), and `.param pd=3k` with a `.data` sweep of the rows 1k and 4k gives r2=3k. Spec 6.2a calls the nominal value the "sweep start", which is 1k for `pv` and for `pd`; HSPICE's `.op` block does not use it. The listing prints the `.dc` results before the `.op` block. Not probed: a `.dc` sweep of the temperature or of a source.
- Plan Task 8 close-out says that `Element` keeps its staged size with `nodes_end` as a `u32` with a sentinel. A `u32` field makes `Element` 60 bytes, not the staged 56 (the struct has no 4-byte hole for it). So the end of the second node token is kept in a side table of the definition (`Def::rcl_ends`), only for the R, C and L lines that parameter resolution reads again (a model word or a `key=value` on the line), and a test pins `size_of::<Element>()` at 56.
- Plan Task 8 gives `ChainStep { kind, scope, text, value, loc: Loc }` and `ElementParams { path, kind, params }`. `ChainStep.loc` is `Option<Loc>` (a `Builtin` step, such as `temper` with no `.temp` line or the default AC phase, has no line), and `ChainStep` has a `name` (the name that the step defines). `ElementParams` also has `model` (from the reading of the R/C/L tokens), `params_total` (`params` lists at most 500) and `caveats` (on the whole answer: the path, the element type, `.alter` changes).
- Plan Task 8 says "The R/C/L value in `ElementParams` is the element's own value". With the element `scale=`, HSPICE's value is the value as written times the scale (`src_delay_scale`, `elem_scale_cl`), and `resolve_element` gives that product, with a caveat. TC1/TC2 still give the value at TNOM with a caveat.
- Plan Task 10 tests `net_info("", "A")` on `dspf_included`. Net `A` is a port of `inv`, so the test asks in the scope of its instance: `net_info("xi", "A")`. HSPICE counts the parasitic R and C in its statistics; the summary gives them apart (`expanded_parasitics`), and the golden totals test adds them back.
