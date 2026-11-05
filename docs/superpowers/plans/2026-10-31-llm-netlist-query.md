# netlist-db LLM Query Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the local netlist-db fork into a fast HSPICE (+DSPF) query engine with a Python library and an MCP server, so an LLM can ask about subcircuits, counts, resolved parameters and connectivity without reading the netlist.

**Architecture:** Fix and extend the existing `nom` parser (crate `netlist-db`); add a dependency-free HSPICE expression crate (`netlist-expr`); add a query crate (`netlist-query`) that lowers the parser's builder output (which keeps file/line spans) into an owned, interned model and answers all queries on demand without expanding the hierarchy; expose it through PyO3 (`pybind/`, returns JSON strings wrapped into dicts) and a thin Python MCP server (`mcp_server/`). HSPICE P-2019.06 on the Parallels VM is the test oracle; its outputs are stored in the repo.

**Tech Stack:** Rust 2024 (cargo 1.99), nom 8, tokio, serde/serde_json, PyO3 0.27 + maturin (abi3-py310), Python 3.13, `mcp` (FastMCP), pytest.

**Spec:** `docs/superpowers/specs/2026-10-31-llm-netlist-query-design.md`

## Global Constraints

- Work only in `~/Project/netlist-db` on branch `feature/llm-query`. Never push, never touch `upstream`.
- **No commits** unless the user asks (user rule). Each task ends by staging its files with `git add`; the staged state is "reviewed and accepted".
- Keep MIT. Do not copy code from OpenVAF, KLayout or spice-lsp (GPL/AGPL); ideas only.
- HSPICE P-2019.06-SP1-1 behavior is ground truth. Default `.option parhier` is **GLOBAL**.
- HSPICE runs only through `prlctl exec "Windows 11" …` (never SSH); one `prlctl exec` per deck; bound every run with `timeout 180`.
- Names are case-insensitive (ASCII lowercase for lookup); output keeps the original spelling for display, paths are output lowercase.
- Never return a guessed number: a value is either a number (possibly with caveats) or `unresolved` with a reason.
- Every list in an answer is paged: default `limit` 50, max 500, cursor = decimal offset string.
- Model-card (`.model`) parameters are text only.
- Follow the repo's existing rustfmt style and module layout.
- Long commands: `timeout 180`; on macOS check memory with `/usr/bin/time -l`.

## HSPICE findings that override task text

`golden/NOTES.md` is the record. The ones that change tasks below (already folded into the task text):
- `**` and `^` are power, LEFT-associative, tighter than unary minus: `2**3**2` = 64, `-2**2+10` = 6. `1e-3k` = 1e-3 (scale after an exponent is ignored). `ME` = mega, `MI` = mil. `pow(x,y)` truncates `y`. `sqrt`/`log`/`log10` of a negative keep the sign.
- Line 1 of the top file is always the title, even a dot command.
- Duplicate top-level `.subckt` names: HSPICE aborts.
- `X` lookup is along the instance path (dynamic), not the text nesting.
- `.op` prints R/m under an `m=` instance: `expected.json` entries with `"m"` mean the element's own value is `value * m`.
- `.alter` changes accumulate across runs. `agauss` etc. directly in an element expression make HSPICE abort.
- Ground: `0`, `gnd`, `gnd!`, `ground`, any letter case. `.option sim_dspf="<scope> <file>"`, scope = subckt or instance name.

## Review Focus

1. **Instance names containing `.`** (e.g. `X0.AAA` in `tests/top.sp`) inside paths — expect longest-match path splitting, not a "not found" error. Pinned in Task 7 (`path_with_dotted_instance_name`).
2. **Duplicate `.subckt` names** (today the built `IndexMap` silently keeps the last) — HSPICE aborts on them (golden `dup_subckt`). Expect both kept in the model, an Error diagnostic `duplicate_subckt` saying HSPICE rejects the deck, the first definition used, and a caveat on answers that depend on it. Pinned in Task 6 (`duplicate_subckt_diagnosed`).
3. **`.include` inside `.subckt`** (`tests/top.sp`) — expect the included elements to belong to that subcircuit. Pinned in Task 6 (`include_inside_subckt_belongs_to_def`).
4. **Supply nets with huge fan-out** — expect `total` plus a first page, never the full list. Pinned in Task 9 (`net_info_pages_high_fanout`).
5. **CRLF line endings, tabs, mixed-case keywords (`.SUBCKT`/`.Ends`), comments between `+` lines, no trailing newline** — expect identical parse to the clean form. Pinned in Task 4 (`messy_whitespace_equivalent`).

---

## File Structure

| Path | Responsibility |
|---|---|
| `Cargo.toml` | workspace members: `pybind`, `macros`, `expr`, `query` |
| `src/parser/mod.rs`, `src/parser/utils.rs`, `src/parser/instance.rs` | parser fixes (Tasks 1, 4, 5) |
| `src/ast.rs`, `src/instance.rs`, `src/lib.rs`, `src/span.rs` | new AST fields; export `Span`, `ParseOptions` |
| `expr/src/{lib.rs,number.rs,parse.rs,eval.rs,builtins.rs}` | crate `netlist-expr`: HSPICE numbers, Pratt parser, evaluator |
| `golden/cases/<case>/{deck.sp,…aux,hspice.lis,expected.json}` | HSPICE oracle data |
| `golden/tools/{run_hspice.sh,lis2json.py}`, `golden/NOTES.md` | regenerate oracle; record HSPICE findings |
| `query/src/lib.rs` | `Netlist` public API |
| `query/src/{model.rs,lower.rs,intern.rs,api.rs}` | owned model, lowering from builders, interning, output types + paging |
| `query/src/{hier.rs,paths.rs}` | X→definition linking, counts, path parsing |
| `query/src/params.rs` | `parhier` resolution with chains |
| `query/src/nets.rs` | connectivity |
| `query/src/dspf.rs` | DSPF scanner + logical nets + parasitic summary |
| `query/tests/{load.rs,hier.rs,golden.rs,nets.rs,dspf.rs,perf.rs}` | integration tests |
| `pybind/src/lib.rs`, `pybind/netlist_db/{__init__.py,__init__.pyi}`, `pybind/tests/test_api.py` | Python library |
| `mcp_server/{pyproject.toml,netlist_mcp/server.py,tests/test_server.py}` | MCP server |

## Task 0: Scaffold (done inline by the coordinator before Task 1)

- [ ] Add `expr` and `query` to workspace members; create `expr/Cargo.toml` (`name = "netlist-expr"`, no dependencies) with empty `src/lib.rs`; create `query/Cargo.toml` (`name = "netlist-query"`; deps: `netlist-db = { path = ".." }`, `netlist-expr = { path = "../expr" }`, `serde` (derive), `serde_json`, `tokio` (workspace)) with empty `src/lib.rs`; add `.venv/` to `.gitignore`; create `golden/cases/` and `golden/tools/`.
- [ ] Run `cargo test --workspace` → the existing 10 tests pass. Stage.

### Task 1: Statement-level error recovery

**Files:** Modify `src/parser/mod.rs:33-66,110-113`, `src/parser/utils.rs:642-749`. Test: `src/parser/mod.rs` (`#[cfg(test)]`, tokio test like the existing ones).

**Interfaces:** Produces: a failed statement becomes one `ParseError` in `LocalAST.errors` (position = start of the failed statement) and parsing resumes at the next logical line; `_include` never replaces a whole file's AST because of a statement error (only IO / missing section / circular errors still do).

- [ ] **Step 1: Failing tests** `recover_bad_mosfet_keeps_neighbors` — file `R1 a b 1k\nM1 d g s b x y z w nch W=1u\nR2 c d 2k\n.end\n` (8 tokens, still invalid after Task 4) → built AST has instances `R1`,`R2`, exactly one error at line 2. `recover_bad_dot_line` — `.param f(x=\nR2 a b 1\n` → `R2` present, one error at line 1. `recover_inside_subckt` — error inside a `.subckt` body keeps the rest of the body and the `.ends`.
- [ ] **Step 2:** `cargo test -p netlist-db recover_` → FAIL (instances missing).
- [ ] **Step 3: Implement** in `local_ast`: wrap each statement dispatch; on `nom::Err::Error`/`Failure`, push `ParseErrorInner::Nom(kind).record(stmt_start)` and advance input to the first `\n` not followed by optional spaces + `+` (skip continuation lines). Keep `ast()`'s `?` only for real I/O.
- [ ] **Step 4:** `cargo test --workspace` → all pass (old 10 + new 3).
- [ ] **Step 5:** `git add src/parser`.

### Task 2: `netlist-expr` crate

**Files:** Create `expr/src/{lib.rs,number.rs,parse.rs,eval.rs,builtins.rs}`; tests in each module.

**Interfaces — Produces (exact):**
```rust
pub fn scan_number(s: &str) -> Option<(f64, usize)>; // HSPICE literal at start of s; usize = bytes consumed incl. ignored trailing letters
pub enum Expr { Num(f64), Ident(String), Call { name: String, args: Vec<Expr> },
  Unary { op: UnOp, e: Box<Expr> }, Binary { op: BinOp, l: Box<Expr>, r: Box<Expr> },
  Ternary { c: Box<Expr>, t: Box<Expr>, f: Box<Expr> } }  // Ident/Call names lowercase
pub enum UnOp { Neg, Plus, Not }
pub enum BinOp { Pow, Mul, Div, Add, Sub, Lt, Le, Gt, Ge, Eq, Ne, And, Or }
pub struct ExprError { pub msg: String, pub offset: usize }
pub fn parse(src: &str) -> Result<Expr, ExprError>; // strips one outer pair of '…' or {…}
pub fn free_names(e: &Expr) -> Vec<String>;          // identifiers, excluding called fn names
pub enum Caveat { StatisticalNominal(String), Temper }
pub enum EvalError { Undefined(String), BadCall(String), Domain(String), Unresolved(String) }
pub trait Env { fn var(&mut self, name: &str) -> Result<(f64, Vec<Caveat>), EvalError>;
  fn user_fn(&mut self, name: &str, args: &[f64]) -> Option<Result<(f64, Vec<Caveat>), EvalError>>; }
pub fn eval(e: &Expr, env: &mut dyn Env) -> Result<(f64, Vec<Caveat>), EvalError>;
```

- [ ] **Step 1: Failing tests** `scan_suffixes`: `1m`→1e-3, `1M`→1e-3, `1meg`/`1MEG`/`1x`→1e6, `1mil`→25.4e-6, `2.5k`→2500, `10pF`→1e-11 consumed 4, `1u`→1e-6, `1n`,`1p`,`1f`→1e-15, `1a`→1e-18, `1t`→1e12, `1g`→1e9, `1e-3k`→1.0, `1V`→1.0 consumed 2, `abc`→None. `pratt_precedence`: `2+3*4`=14, `2**3**2`=512 (right-assoc), `-2**2`=-4, `(1+2)*3`=9, `1<2 && 3>2`=1, `0||0`=0, `1?2:3`=2, `2*wn` with `wn=1u` = 2e-6, names case-insensitive (`WN` = `wn`). `builtins`: `sqrt(4)`=2, `pow(2,3)`=8, `pwr(-2,2)`=-4 (HSPICE sign-preserving), `log(e)`=1 (natural), `log10(100)`=2, `exp(0)`=1, `abs(-1)`=1, `min(1,2)`=1, `max(1,2)`=2, `int(2.7)`=2, `sgn(-3)`=-1, `sign(3,-1)`=-3, `agauss(1k,0.1k,3)`=1000 with `StatisticalNominal("agauss")` (same for `gauss`, `aunif`, `unif`), `temper` → calls `env.var("temper")`. `user_fn_call`: `f(2)` routes to `Env::user_fn`. `parse_errors`: `2+` → `ExprError` with offset 2.
- [ ] **Step 2:** `cargo test -p netlist-expr` → FAIL.
- [ ] **Step 3: Implement.** `scan_number`: digits/`.`/exponent, then the longest scale prefix among `MEG`,`MIL`,`T`,`G`,`X`,`K`,`M`,`U`,`N`,`P`,`F`,`A` (case-insensitive, `MEG`/`MIL` before `M`), then skip trailing ASCII letters. Pratt binding powers (low→high): `?:` 1 (right), `||` 2, `&&` 3, `== !=` 4, `< <= > >=` 5, `+ -` 6, `* /` 7, unary `- + !` 8, `**` 9 (right). `^` is parsed as `Pow` **only if** golden case `ops` (Task 3) shows HSPICE treats it as power; otherwise a parse error — check `golden/NOTES.md` before finishing. Any builtin the golden case `funcs` shows HSPICE rejects is removed.
- [ ] **Step 4:** `cargo test -p netlist-expr` → PASS.
- [ ] **Step 5:** `git add expr`.

### Task 3: HSPICE golden corpus

**Files:** Create `golden/tools/run_hspice.sh`, `golden/tools/lis2json.py`, `golden/NOTES.md`, `golden/cases/<case>/…`.

**Interfaces — Produces:** for each case dir: `deck.sp` (+ aux files), `hspice.lis` (exact HSPICE output), `expected.json`:
```json
{"case": "parhier_local", "resistors": [{"path": "x1.r1", "value": 3.0}],
 "totals": {"resistors": 1, "capacitors": 0, "mosfets": 0, "subckt_instances": 1},
 "extra": {}}
```
`path` = lowercase HSPICE instance path + element name, without the `N:` element index (top-level elements: just the name). `extra` holds case-specific facts written by hand from the `.lis` (e.g. `"ground_aliases": ["0","gnd"]`, `"marker_counts": {"inv": 6}`).

- [ ] **Step 1:** `run_hspice.sh <case_dir>`: base64 each file of the case dir into `C:\tmp\ndbgold\<case>\` with one `prlctl exec "Windows 11" powershell -NoProfile -Command …` call, run `hspice.exe -i deck.sp -o hspice` in the same call, then fetch `hspice.lis` with `prlctl exec … cmd /c type …` into the case dir. Bound with `timeout 180`. Verify on the existing VM deck (`whowins`, expect `r value 3.0000`).
- [ ] **Step 2:** `lis2json.py <lis> <case>` parses the `.op` resistor blocks (`subckt` row, `element` row, `r value` row; several columns per block; HSPICE engineering suffixes in values) and the `# resistors = N` totals. Test it on a deck with ≥7 resistors so column blocks wrap.
- [ ] **Step 3: Cases** (resistor values carry the quantity under test; one rule per case): `parhier_local`, `parhier_global`, `parhier_default` (whowins deck, results 3/4/4); `nested_global`, `nested_local` (Infineon examples 6 and 7, both settings); `inherit` (undefined name in a nested subckt: parent instance scope vs global); `dup_param` (same `.param` twice: which wins); `dup_subckt`; `units` (`1m 1M 1meg 1x 1mil 2.5k 10pF 1V 1a 1t 1g 1e-3k`); `ops` (precedence incl. `**`, unary minus, `^`, comparisons, `&&`/`||`, ternary — split into several decks if HSPICE rejects one form, record rejections in `NOTES.md`); `funcs` (each builtin of Task 2 + a two-arg `.param f(a,b)=…`); `stat_nominal` (`agauss` etc.); `temper` (`.temp 50` and default); `lib_corner` (+ `models.lib` with `tt`/`ff` sections) ; `include_in_subckt`; `alter` (first-run values; record later runs in `extra`); `title_line` (line 1 = `R1 a b 1` → HSPICE ignores it); `case_mix`; `hier_counts` (3 levels, several call sites, an unused subckt, `m=2` on an `X` line, a marker resistor `rmk_<def>` in each def → `extra.marker_counts`); `nested_def` (`.subckt` defined inside `.subckt`: which definition an `X` resolves to); `ground_alias` (`0`, `gnd`, `gnd!`, `ground` — which are ground, via branch currents); `global_net` (`.global vdd` used inside a subckt without a port); `dspf_included` (hand-written DSPF like spec §6.1a as the netlist) and `dspf_backannot` (`.option sim_dspf=…` on a pre-layout deck; record in `NOTES.md` whether P-2019.06 accepts it and the exact option spelling).
- [ ] **Step 4:** Run all cases; every case has `hspice.lis` with "job concluded" and an `expected.json`. `NOTES.md` lists each HSPICE finding in one line (e.g. "`^` = power: yes/no", "`inherit`: parent scope wins", "ground aliases: …").
- [ ] **Step 5:** `git add golden`.

### Task 3a: Follow-ups from the review of Tasks 1–3

Three parts. **3a-P** and **3a-G** touch disjoint files and run in parallel. **3a-E** needs the new golden cases of 3a-G.

**3a-P — parser** (Files: `src/parser/*.rs`, `src/err.rs`, `src/ast.rs`). Tests in `src/parser/mod.rs` `recover_tests`:
- `ends_name_chars`: `.ends` reads its name with `name`, not `key` → `.ends inv-x1`, `pm#1`, `cell.a`, `bus[0]` give no error.
- `binned_model_name`: `.model nch.1 nmos level=54` parses (model name via `name`).
- `skip_subckt_eof_fallback`: a failed `.subckt` header with no matching `.ends` falls back to the one-line skip → `R0 a b 1\n.subckt\nR1 a b 1\nR9 a b 9\n` keeps R1 and R9 at top level, one error. A failed nested header does not consume the parent's `.ends`.
- `failed_block_one_error`: a failed `.data … .enddata` and a failed `.lib <sec> … .endl` each give exactly one error, and their bodies do not leak.
- `clear_error_text`: new `ParseErrorInner` variants `StrayEnds`, `MissingEnds { subckt }`, `TrailingText` print plain English (e.g. "`.ends` without an open `.subckt`").
- `include_quotes_stripped` (`.include "f.sp"` and `'f.sp'`), `bom_skipped` (UTF-8 BOM at file start), `ic_only_segment_kept` (`LocalAST::is_empty` counts `init_condition` and `nodeset`).
- Perf: `statement_end` returns the position after the blank region so `local_ast` does not scan it twice. Release parse of the 1.9M-line deck (scratch generator) is within 5 % of the staged version.

**3a-G — golden** (Files: `golden/**`):
- `run_hspice.sh` exits non-zero when the run call fails, and refuses a fetched listing that is older than the run.
- `lis2json.py --selftest` catches each case's error and prints a FAIL line; error tests assert the offending line text; a repeated circuit number or a gap in 0..N is an error.
- Rejected cases (`extra.rejected`): totals are `null`, not 0.
- New cases: `units2` (`1min 1hr 1day 1yr 1z 20db 1meter 1ms 1mv 1mohm`), `ops2` (`2**-3**2`, `2^-3^2`, `0**0`, `0^0`, `0**-1`, `0**0.5`; split into several decks when HSPICE aborts and record each abort). Add one NOTES.md line per finding.

**3a-E — expr** (Files: `expr/**`; after 3a-G):
- Builtins `nint floor ceil sin cos tan asin acos atan sinh cosh tanh db` (golden `funcs_extra`: `nint(2.5)`=3, `floor(2.7)`=2, `ceil(2.2)`=3, `db(10)`=20) with tests from the golden values.
- `scan_number` follows `units2`; zero base and unary minus after `**`/`^` follow `ops2` (a `Domain` error where HSPICE aborts).
- `Ident` values pass through the finite check; `golden_ops_values` uses the deck text `1?2:0?4:5` = 2 and all four `ops_caret` values; `parse` strips an outer `'…'`/`{…}` pair only when the pair encloses the whole expression (`'1'*'2'` = 2); `free_names` deduplicates in linear time.

### Task 4: Parser element and lexical fixes

**Files:** Modify `src/parser/utils.rs` (`unit`, `float_unit`, `value`, `option`), `src/parser/instance.rs`, `src/instance.rs`, `Cargo.toml` (root package: add `netlist-expr = { path = "expr" }`). Test: `src/parser/instance.rs`, `src/parser/utils.rs`.

**Interfaces:** Consumes `netlist_expr::scan_number`. Produces: `float_unit` delegates to `scan_number` (one unit table in the repo); `MOSFETBuilder` gains `extra_nodes: Vec<Span>` (nodes after the 4th, before the model); `ResistorBuilder`/`CapacitorBuilder`/`InductorBuilder` gain `params: Vec<KeyValueBuilder>`, and the value may also come from a keyword (`R1 a b r='2*x'`, `C1 a b c=1p`, `L1 a b l=1n`) — the value field becomes `Option<ValueBuilder>`; `.option` values accept double-quoted strings (`sim_dspf="a.dspf"`) as `Expr` spans without the quotes.

- [ ] **Step 1: Failing tests** `units_via_expr` (`R1 a b 1m` → `Num(1e-3)`; `10pF` → `Num(1e-11)`); `mos_extra_nodes` (`M1 d g s b e1 nch W=1u` → 4 nodes + `extra_nodes=[e1]`, model `nch`); `mos_three_nodes` unchanged; `rcl_keyword_and_params` (`R1 a b r='2*x' tc1=1e-3 m=2` → value Expr `2*x`, params `tc1`,`m`); `option_quoted_string`; `dollar_annotations` (`M1 d g s b nch W=1u $X=1.2 $Y=3.4` → params only `W`); `messy_whitespace_equivalent` (CRLF, tabs, `.SUBCKT`/`.Ends`, `* comment` between `+` lines, no trailing newline → same instances/params as the clean deck).
- [ ] **Step 2:** `cargo test -p netlist-db` → new tests FAIL.
- [ ] **Step 3: Implement.** Keep `Display` (`src/_impl_display.rs`) compiling for the changed builders.
- [ ] **Step 4:** `cargo test --workspace` → PASS.
- [ ] **Step 5:** `git add Cargo.toml src`.

### Task 5: Parser statements and parse options

**Files:** Modify `src/parser/mod.rs`, `src/parser/utils.rs`, `src/ast.rs`, `src/lib.rs`, `src/span.rs`. Test: `src/parser/mod.rs`.

**Interfaces — Produces:**
```rust
pub struct ParseOptions { pub title_line: TitleLine, pub lib_override: Vec<LibOverride> }
pub struct LibOverride { pub file: PathBuf, pub from: String, pub to: String } // (changed in Task 5a)
pub enum TitleLine { Auto, Yes, No }
pub async fn parse_top_multi_with<I: Iterator<Item = FileId>>(file_ids: I, opts: &ParseOptions) -> (Parsed, Files);
pub use span::{Span, FileId, ParsedId};          // Span exported from the crate root
// LocalAST new fields:
pub param_func: Vec<ParamFuncBuilder>,  // ParamFuncBuilder { name: Span, args: Vec<Span>, body: Span }
pub global: Vec<Span>,
pub protected: Vec<(u32, u32)>,         // first/last line of a .prot/.protect … .unprot/.unprotect region (content skipped)
// Segment new variant:
Segment::Alter { index: usize, pos: Option<Pos> } // marker; statements after it belong to that .alter run
```
Also: `TitleLine::Auto` skips line 1 of a **top** file (HSPICE does so even for a dot command, golden `title_dot`) unless line 1 starts, case-insensitively after spaces, with `.subckt`, `.macro`, `.lib`, `.inc`, `.include` or `.model` (a library loaded directly); when line 1 has content, record an Info diagnostic `title_line` that says whether it was skipped or kept. Included files never skip. `lib_override` replaces the section of a `.lib 'file' sec` whose file name (last path component, case-insensitive) matches. Known analysis/output commands (`.tran .op .dc .ac .temp .print .probe .meas .measure .options .option .end .ic .nodeset .data .hdl .save .tf .noise .sens .four`) are not reported as errors; unknown ones keep today's error. Built `AST` gains `param_func`, `global`, `protected`, `alter_runs` (count).

- [ ] **Step 1: Failing tests** `param_function_parsed` (`.param f(a,b)='a*b' g=2` → one func + one param); `global_parsed`; `prot_region_skipped` (content between `.prot` and `.unprot` not parsed, region lines recorded, statements after it parsed); `alter_segments` (`.param x=1` / `.alter` / `.param x=2` → Alter marker between the two); `title_auto` (line 1 `R1 a b 1` skipped; line 1 `.option parhier=local` skipped; line 1 `.subckt …` kept; line 1 `* t` is a comment either way); `lib_override_selects_section` (deck `.lib 'models.lib' tt` with override `models.lib → ff` loads ff); `analysis_cmds_not_errors`.
- [ ] **Step 2:** FAIL. **Step 3:** implement; `parse_top_multi` keeps its signature and calls `parse_top_multi_with` with defaults (`TitleLine::No`, no override) so `examples/parser_multi.rs` and the regression TOMLs behave as before.
- [ ] **Step 4:** `cargo test --workspace` → PASS; `cargo build --release --example parser_multi` OK.
- [ ] **Step 5:** `git add src`.

### Task 5a: Follow-ups from the review of Tasks 3a–5

**5a-G — golden probes first** (Files: `golden/**`). One case per question, one NOTES.md line per finding:
- `elem_dup_kw`: `R1 1 0 r=1k r=2k` and `R2 2 0 1k r=3k` — which value wins.
- `res_model`: `.model rmod r` (no scaling parameters), `R1 1 0 rmod 2k`, `R2 2 0 rmod r=3k` — model + positional value and model + keyword.
- `mos_positional`: `.model nch nmos level=1`, `M1 d g s b nch 2u 1u` — whether HSPICE reads the trailing numbers as L and W (use `.option list` or any listing that prints MOSFET L/W; if no listing shows them, record that).
- `title_plus`: line 1 is a title, line 2 is a `+` line — does HSPICE treat line 2 as part of the title or as an error.
- `macro_eom`: `.macro inv a b … .eom` instantiated by an `X` line.

**5a-P — parser** (Files: `src/**`; after 5a-G). Tests in the module the code lives in:
- `mos_positional_lw`: the model is the last bare token that is not a number; bare numeric tokens after it are positional parameters stored in `params` under `l w ad as pd ps nrd nrs` in that order (follow `mos_positional` if it says otherwise). `M1 d g s b nch 2u 1u` → nodes `d g s b`, model `nch`, `l=2u`, `w=1u`, no extra nodes. `M1 1 2 3 4 nch` (numeric node names) still works.
- `rcl_model_and_value`: two bare tokens after the nodes are model + value (new field `model: Option<Span>` on R/C/L builders): `R1 a b rmod 2k`. One bare identifier stays in `value` (the query layer decides model vs parameter). Duplicate keywords follow `elem_dup_kw`.
- `source_function_not_dc`: `V1 a 0 PULSE(0,1,0,1n,1n,5n,10n)`, `SIN(0 1 1meg)`, `EXP(…)`, `PWL(…)` with commas, `SFFM(…)`, `AM(…)`, `PAT(…)` become a new variant `Func { name: Span, text: Span }`, never `Value`. Keep the existing space-separated `PWL` parse.
- `lib_override_triples`: `ParseOptions.lib_override: Vec<LibOverride { file, from, to }>`; only calls that match file name (last component, any case) AND section change; drop the same-file exception of Task 5. `Parsed` records which overrides matched so Task 6 can warn about unused ones. Deck calling `tt` and `res_tt` with override `(models.lib, tt, ff)` → `ff` and `res_tt` both load.
- `endl_in_comment`: a `.lib` section ends only at a line whose first token is `.endl` (not inside a comment).
- `deep_nesting_no_crash`: `.subckt` nesting deeper than 256 levels gives one `NestingTooDeep` error and no stack overflow; a deck of 5,000 nested headers without `.ends` finishes.
- `macro_eom_synonyms` (if `macro_eom` confirms): `.macro`/`.eom` = `.subckt`/`.ends`.
- `title_auto_prot`: `.prot`/`.protect` join the Auto library-keyword list; `+` lines right after a skipped title line follow `title_plus`.
- `clear_element_errors`: a MOSFET node-count error and an R/C/L line with no value print plain English (no `MapRes`/`Verify`).
- `cargo test --workspace` passes; release parse time of the 1M-line deck within 5 % of the staged version.

### Task 6: Query model, lowering, load, summary, diagnostics

**Files:** Create `query/src/{lib.rs,model.rs,lower.rs,intern.rs,api.rs}`, `query/tests/load.rs`.

**Interfaces — Produces (exact public API; later tasks add methods to `Netlist`):**
```rust
pub struct LoadOptions { pub path: PathBuf, pub top: Option<String>, pub lib_override: Vec<LibOverride>, pub title_line: TitleLine }
pub struct Netlist { /* owned, Send + Sync */ }
pub struct Loc { pub file: String, pub line: u32 }
pub struct Page { pub limit: usize, pub cursor: Option<String> }        // Default: 50, None
pub struct Paged<T> { pub items: Vec<T>, pub total: usize, pub next_cursor: Option<String> }
pub enum QueryError { NotFound { what: String, name: String, suggestions: Vec<String> }, BadArgument(String) }
pub enum LoadError { Io(String), NoTopLevel(String) }
impl Netlist {
  pub fn load(opts: &LoadOptions) -> Result<Netlist, LoadError>;
  pub fn summary(&self) -> Summary;  // files, definitions, root ("top-level"|"subckt:<name>"|"none"), top_level_instances, expanded device totals by type (filled in Task 7), parhier ("global"|"local"), diagnostics_count, alter_runs
  pub fn diagnostics(&self, level: Option<Level>, page: &Page) -> Paged<Diagnostic>; // Level: Error|Warning|Info; Diagnostic { level, code, message, loc }
}
```
Diagnostics lowering also covers (from the Task 5 review): Info `title_line` from `ASTBuilder.title`; one Info `analysis_commands` per command name with its count (`.meas` ×120), not one per line; one Warning `protected_region` per `.prot` region ("lines a–b of <file>: not parsed; definitions, elements and counts inside are missing"); Warning `lib_override_unused`; renumber `.alter` blocks in document order across all files (the parser counts per file); plain-English text for every parser error kind. All output types derive `serde::Serialize`; `api.rs` has `fn page<T>(items: impl ExactSizeIterator<Item=T>, page: &Page) -> Paged<T>`. Model (internal, used by later tasks): `Def { name, display, ports, defaults: Vec<ParamDef>, params: Vec<ParamDef>, funcs: Vec<FuncDef>, elements: Vec<Element>, nested: Vec<DefId>, parent: Option<DefId>, loc }`, `Element { name, kind: ElemKind, pins: Vec<(PinRole, Sym)>, model: Option<Sym>, callee: Option<Sym>, params: Vec<ParamDef>, loc, after_alter: Option<usize> }`, `ParamDef { name: Sym, text: String, num: Option<f64>, loc }`. The root (top-level statements) is `DefId(0)`. Lowering walks `Parsed`/`Files` builders (not the built `AST`, which loses locations), follows `Segment::Include` into the included file's builders, records `.option parhier`, and turns every `ParseError` into an `Error` diagnostic with file and line.

- [ ] **Step 1: Failing tests** `load_counts_tests_top_sp` (`tests/top.sp` loads; definitions include `demo`, `units`; diagnostics include the `inc0.sp` I/O error and the circular include); `diagnostic_has_file_and_line`; `duplicate_subckt_diagnosed` (two top-level `.subckt inv` → both in model, Error diagnostic `duplicate_subckt` with "HSPICE rejects this deck", the first is active; a nested `.subckt cell` with the same name as a top-level `cell` is NOT a duplicate); `include_inside_subckt_belongs_to_def`; `parhier_option_read` (`.option parhier=local` → "local"; none → "global"); `paging_limits` (75 diagnostics → 50 + cursor "50"; second page 25, no cursor).
- [ ] **Step 2:** `cargo test -p netlist-query` → FAIL. **Step 3:** implement (block on tokio with a current-thread runtime + `enable_all`). **Step 4:** PASS.
- [ ] **Step 5:** `git add query`.

### Task 6a: Follow-ups from the review of Tasks 5a and 6

**6a-G — golden probes** (Files: `golden/**`; runs in parallel with 6a-Q). A line that HSPICE rejects aborts the whole deck, so each form that HSPICE may reject gets its own case. Read values from `.op`; where `.op` does not print a value, use `.option list`. One NOTES.md line per finding:
- `ind_value_model` (`L1 1 2 1n lmod`), `ind_model_value` (`L1 1 2 lmod 1n`), with `.model lmod l`; `cap_value_model` / `cap_model_value`, the same for C with `.model cmod c`. Which order HSPICE accepts.
- `res_tc_positional`: `R1 1 0 1k 0.004` and `R2 2 0 1k 0.004 1e-5` at `.temp 125`. Are the bare numbers after the value TC1 and TC2? List the element paths whose listed value is temperature-adjusted in `extra.skip_value_check`, so that Task 8's value check skips them.
- `mos_digit_model`: `.model 5vnmos nmos level=1 kp=1e-4 vto=0.5`, `M1 d g s b 5vnmos w=1u l=1u`, and the same with `33nmos`. Is a model name that starts with digits read as the model?
- `mos_kw_and_positional`: `M1 d g s b nch 2u 1u l=3u`. Which L wins?
- `diode_area`: `D1 1 0 dmod 2` and `D2 2 0 dmod` at the same bias. Is the bare number the area (current × 2)? `D3 3 0 dmod 2 1e-6` (pj). `bjt_area`: `Q1 c b e qmod 2`.
- `inc_twice`: a file with a `.subckt`, `.include`d twice. `inc_twice_params`: a file with only `.param` and `.model`, `.include`d twice. Does HSPICE abort?
- `lib_twice`: the same `.lib 'm.lib' tt` call twice (the section has a `.subckt` and a `.model`). Abort, ignore the second call, or load it twice?
- `after_end`: element and `.param` lines after `.end`. Ignored, or a second simulation?
- `src_forms`: `V1 1 0 DC 1.2`, `V2 2 0 DC 1 PULSE(0 1 0 1n 1n 5n 10n)`, `V3 3 0 1.2 AC 1 0`, `V4 4 0 PULSE 0 1 0 1n 1n 5n 10n` (no parentheses), `V5 5 0 DC=2`, `I1 0 6 DC 1m`, each with a load resistor. The `.op` node voltages give the DC values.
- `param_braces`: `.param a=2 c={3*a}`, `R1 1 0 {c}`.
- `title_plus_gap`: line 1 a title, line 2 a `*` comment, line 3 a `+` line. Abort like `title_plus`, or not?
- `nonutf8_comment`: a Latin-1 byte (0xE9) inside a `*` comment. Accepted or not?

**6a-Q — query** (Files: `query/**`; runs in parallel with 6a-G). Tests in `query/`:
- `interner_single_copy`: each name is stored once, in its first-seen spelling. Lookup is ASCII case-insensitive with a fast non-cryptographic hash. Answers show that spelling for subckt, element, port, parameter and net names; paths stay lowercase. Measure 1M resistors with 2M unique names (load time and peak RSS, before and after).
- `keyword_value_text`: `R1 a b r=1k`, `C1 a 0 c=10pF` and `R2 2 0 1k r=3k` keep the text as written of the winning value.
- `element_params_unique`: in one element, the last value of a name wins (golden `elem_dup_kw`: a keyword after a positional value wins). Only the winner is kept.
- `alter_in_included_subckt`: an `.alter` reached through an `.include` inside a `.subckt` gives an Error `alter_in_subckt`. The marker is ignored, so later statements stay in the first run.
- `include_cycle_guard`: `walk` keeps a stack of the files in progress. A re-entry gives an Error `include_cycle` and no recursion.
- `diagnostics_document_order`: the diagnostics of one file are in line order (statements and `.subckt` bodies interleaved). A file included twice reports its lowering diagnostics once.
- `summary_first_run`: `definitions` counts only first-run definitions. New field `alter_definitions`.
- `find_top_rule`: a plain name picks the active top-level definition, else the one active nested definition (no inactive ancestor). An ambiguity lists distinct qualified names. Task 7 uses the same rule for subckt names.
- `LoadError` derives `Serialize`. Convention for every later query: it returns `Result<_, QueryError>`, and its lists are `Paged<T>` inside.
- Unit tests remove their temp directories.

**6a-P — parser** (Files: `src/**`, `expr/**` if needed, and `query/src/lower.rs` to adapt to changed builder types and for the lowering items below; after 6a-G and 6a-Q). Follow the 6a-G findings. Tests in the module the code lives in:
- `rcl_order`: accept the R/C/L model and value orders that HSPICE accepts. Reject the others with a plain-English error. If `res_tc_positional` confirms, store bare numbers after the value as positional `tc1`, `tc2` (as for MOSFET positional values).
- `digit_first_model`: a bare token is a positional number only if it reads completely as a number with at most one scale suffix and nothing after it (`2u`, `1e-6`, `0.5`). `5vnmos` and `33nmos` are names. Follow `mos_digit_model`.
- `diode_bjt_positional`: keep or remove the D/Q positional names, as `diode_area` and `bjt_area` show.
- `source_forms`: V/I lines accept `DC v`, `DC=v`, a bare first value, `AC mag [phase]`, and one transient function with or without parentheses, in each order that HSPICE accepts. Store the DC value, the AC magnitude and phase, and the function name and arguments. A helper gives the argument text without `+` markers, `$` comments and `*` comment lines. Lowering stores every source function as its arguments only.
- `param_braces`: `{…}` groups like quotes in `.param` values and element values.
- `after_end`: follow `after_end`. If HSPICE ignores the lines, the parser stops the top file at `.end` and records how many lines follow. Lowering gives an Info `after_end`.
- `title_plus_gap`: follow `title_plus_gap`.
- `non_utf8`: a file with invalid UTF-8 is decoded lossily (U+FFFD) and keeps all its statements. The parser records the first bad line. Lowering gives a Warning `non_utf8`.
- `repeated_include`: follow `inc_twice` and `lib_twice`. If HSPICE ignores a repeated `.lib` section call, lowering skips it and gives an Info `lib_section_repeated`. A `duplicate_subckt` message names both include sites, never the same file:line twice.
- Release parse time of the 1M-line deck within 5 % of the staged version.
- **Completion items (added after review round 3):**
  - `ac_then_function`: `V1 a 0 AC 1 SIN(0,1,1meg)`, `V2 b 0 DC 0 AC 1 PULSE(0,1.8,0,1n,1n,5n,10n)` and `V3 c 0 AC=1 PWL(0,0,1n,1)` keep their function. A token whose name before `(` is a source keyword is never the AC phase. A second function after AC gives the "two transient functions" error.
  - `braces` Error also for `{` inside source-function arguments (golden `param_braces`: HSPICE rejects `{…}`).
  - A first R/C/L token with a character outside the name set (`R1 a b rmod-1 2k`) is an Error, as before 6a-P.
  - `comment_only_plus_lines`: any number of comment-only `+` lines (`R1 a b 1k $a` / `+ $b` / `+ $c`) continue the statement. The element is kept. Parse time stays within 5 %.
  - `duplicate_model`: two `.model` cards with one name in one scope give an Error `duplicate_model` (golden `inc_twice_params`: HSPICE aborts).
  - `mos_digit_after_model`: pin `M1 d g s b e 5vnmos` → model `e`, `l` = 5 (HSPICE rejects model names that start with a digit, `mos_digit_model`; it ignores letters after a number, `mos_suffix_letters`).
  - Messages: the error for a value-like token in the model place (`C1 a b 10pF 0.004`, `R1 a b 1kohm r=2k`) names the token and says that R and C take "model, then value". The `after_end` Info uses "it" for one line. `duplicate_subckt` names only the include lines where the two include chains differ.
  - Interner: a built-in name that lowering interns (`r c l dc pwl temp w ad as pd ps nrd nrs area pj`, model types) does not fix the display spelling: the first spelling in the deck replaces it. The hash mixes every byte into the table tag (final avalanche): a test interns 238k names of the form `net_bus?net_bus?net_bus?` (varying at bytes 7, 15, 23) in under 1 s.
  - Deterministic include cycles: one deck gives the same diagnostics and elements in every run (test: 20 loads of the a→b→c→a deck agree). `Segment::Include` records the include line, so the cycle diagnostic has a file and line. One code for include cycles.
  - Record in golden/NOTES.md, section "Choices not probed": `.end` in an included file is ignored; `M1 d g s b e 5vnmos`; the R/C/L two-token reading of Task 8.
- **Close-out (added after review round 5).** Five review rounds each found one new parser edge. Most of them were forms that no golden case settled. So the close-out first gets HSPICE ground truth for the open forms, then fixes the parser to match.
  - **6b-G probes** (Files: `golden/**`, before the fixes). Same rules as 6a-G. A form that HSPICE may reject gets its own case.
    - `brace_quoted_param` (`.param a=2 p='{a}*2'`), `brace_quoted_elem` (`R1 1 0 '{a}'` with `.param a=2k`), `brace_quoted_fn` (`V1 1 0 PWL(0 '{a}' 1n 1)` with `.param a=0.5`). Is a `{` inside quotes rejected like an unquoted `{` (`param_braces_param` shows that HSPICE reads `{`/`}` as `[`/`]`)?
    - `pwl_repeat`: `PWL(0 0.3 1n 1) R`, `PWL(0 0.3 1n 1) R=0 TD=1n`, `PWL(0 0.3 1n 1) TD=1n`. Accepted? What is the `.op` value?
    - `cl_tc_positional`: `C1 1 0 cmod 1p 0.004` and `L1 1 0 1n lmod 0.004` (and the forms without a model). Read the values with `.option list`.
    - `src_ac_default_phase`: `V1 1 0 DC 1 AC 1`. The AC phase is 0?
    - `src_sin_t0`: `SIN(0.2 1 1meg 0 0 90)` (phase 90 gives 1.2 at t=0 if the rule is "value at t=0") and `SIN(0.2 1 1meg 1n)` (delay).
    - `src_fn_dc_more`: `SIN(0 1 1meg) DC 0.7` and `PULSE 0 1 0 1n 1n 5n 10n DC 0.7` (no parentheses). Rejected like `src_order_fn_first`?
    - `after_end_second_deck`: `.end`, then a complete second deck (title, source, R, `.op`, `.end`). One job or two?
    - `end_in_include`: an included file with `.end` between two R lines. Is the second R loaded?
  - **Parser and lowering fixes** (Files as in 6a-P, plus `golden/NOTES.md`). A test for each:
    - `braces`: follow the `brace_quoted_*` cases in all three places (element values, `.param` values, source-function arguments). If HSPICE rejects them, a quoted `{` is a `braces` Error, as an unquoted one is. The value is unresolved, never a number.
    - `pwl_repeat`, `cl_tc_positional`, `src_fn_dc_more`, `after_end_second_deck`, `end_in_include`: follow the probes. Replace the matching "Choices not probed" lines with the probe results.
    - Block-error recovery stops at a `.end` line of the top file. In `* t` / `.subckt` / `R1 a b 1` / `.end` / `R2 a b 2` / `.ends` / `R3 a b 3`, R3 is not loaded and the `after_end` Info appears.
    - The lowering helper `statement_rest` passes over `$` comment lines between continuation lines, as the parser does (`R6 a b 1k` / `$ c` / `+ m={x}` gives `braces`).
    - Include cycles compare canonical paths. `main.sp` → `sub/b.sp` → `../main.sp` gives `include_cycle`.
    - The `digit_first_model` message and its NOTES line state the real rule: the error fires only for a word right after a value with no model name before it.
    - Display spelling of the R/C/L keyword comes from its first spelling on the line (`R1 a b R=1k r=2k` shows `R`).
    - `comment_only_plus_at_end_of_file` also runs with CRLF on every line.

### Task 7: Hierarchy, counts, subcircuit queries, paths

**Files:** Create `query/src/{hier.rs,paths.rs}`, `query/tests/hier.rs`; modify `query/src/lib.rs`.

**Interfaces — Produces:**
```rust
impl Netlist {
  pub fn list_subckts(&self, filter: Option<&str>, sort: SubcktSort, page: &Page) -> Result<Paged<SubcktRow>, QueryError>;
  // SubcktSort: Name | Copies | Devices ; SubcktRow { name, ports, devices, x_lines, copies: Option<u64>, copies_note: Option<String>, loc }
  pub fn subckt_info(&self, name: &str, page: &Page) -> Result<SubcktInfo, QueryError>;
  // ports, defaults, local params, devices_by_type: [{type, count, models:[..]}], children: [{subckt, x_lines}], elements: Paged<ElementBrief>
  pub fn instances_of(&self, name: &str, page: &Page) -> Result<Paged<InstanceRef>, QueryError>;
  // InstanceRef { parent, instance, loc, overrides: [{name,text}], m: Option<String> }
  pub fn element_info(&self, path: &str) -> Result<ElementInfo, QueryError>;
  // ElementInfo { path, kind, def (owning subckt), pins: [{role, net}], model, callee, params: [{name,text}], loc }
}
pub(crate) fn split_path(net: &Netlist, path: &str) -> Result<Vec<(DefId, ElemIdx)>, QueryError>; // longest-match on names with dots; "" or "." = root
```
`X`→definition lookup is DYNAMIC (golden `nested_def`, `nested_def_dynamic`, `nested_def_reach`): for an `X` line in the definition of instance P, look in the nested definitions of P's definition, then of P's parent instance's definition, and so on up the instance path, then the top level. Count over a DAG whose nodes are (definition, context), where context = the ancestor definitions on the instance path that have nested definitions (memoized; with no nested definitions anywhere, context is always empty and this is the plain definition DAG). Nested definitions are separate definitions with qualified display names joined by `>` (e.g. `outer>cell`); `subckt_info`/`instances_of` take a name by the Task 6a `find_top_rule`: a plain name picks the active top-level definition, else the one active nested definition; an ambiguity is `NotFound` with the distinct qualified candidates as suggestions; `outer>cell` names a nested definition. Lists come from the first run (`.alter` blocks excluded). Names in answers use the first-seen spelling (Task 6a); paths are lowercase. An `X` name with no visible definition on some path is a Warning diagnostic for that path, not a load error. Cycles → `Error` diagnostic and `copies: None` with note. `copies(root)=1`, `copies(child) = Σ copies(parent) × calls`, `u64` checked arithmetic (overflow → `None` + note "overflow"). No root (bare netlist, no `top`) → `copies: None`, note "no top-level instances". `top=<name>` makes that definition the root. Summary's expanded device totals = `Σ copies × local devices` by type (parasitic R/C from DSPF excluded later in Task 10). `m` is reported, never multiplied into copies.

- [ ] **Step 1: Failing tests** `golden_hier_counts` (`golden/cases/hier_counts`: `copies` per def == `extra.marker_counts`; summary resistor total == `totals.resistors`; unused subckt has copies 0); `bare_netlist_no_root` (`tests/asap7_extracted.sp`: all copies `None`, then `top=` its first subckt → numbers); `golden_nested_def_dynamic` (golden `nested_def_dynamic`: top-level `cell` copies 1, `outer>cell` copies 2; `element_info("x1.xd.xc.rc")` has def `outer>cell`; `nested_def_reach`: `x1.xu.xd.xc.rc` belongs to `u>cell`, and `only` resolves under `x1.xt`); `cycle_reported`; `path_with_dotted_instance_name` (`tests/top.sp`: `element_info("x0.aaa")` finds `X0.AAA`); `instances_of_lists_overrides`; `unknown_name_suggests` (`subckt_info("invv")` → NotFound with suggestion `inv`).
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** `cargo test -p netlist-query` PASS.
- [ ] **Step 5:** `git add query`.

**Task 7 close-out (added after review round 4).** The oracle reviewer found that HSPICE names instances in creation order. Instance names can hold a `.`, so two instances can get the same full path. HSPICE creates the instances breadth-first: level by level, then by parent order, then by `X`-line order. The instance created first keeps the path. HSPICE lists a later one as circuit `.`, so nothing below it has a path. The probes are `hier_dotted_collision`, `hier_dotted_collision_deep`, `hier_dotted_collision_order` and `hier_dotted_collision_order_rev`. The backtracking `walk()` used "the longer name wins" and is exponential on some decks. Replace it:
- `walk()` is a breadth-first search in that creation order. A node is one instance whose full name is `path[..pos]`. At each node, the `X` lines of its definition are tried in line order. A line is a candidate when its name is a prefix of the rest of the path that ends at a `.` or at the end. Only the FIRST node for each `pos` is kept: a later node at the same `pos` has the same full name, so HSPICE lists it as circuit `.`. So the search has at most (number of `.` in the path + 1) nodes, each with at most that many hash lookups. There is no backtracking and no memo keyed on scopes. Each node keeps its (definition, context) for the dynamic binding.
- An element path is a node's full name, a `.`, and the name of a non-`X` element of that node's definition. The first node in creation order that gives such an element wins. If another node also gives one, add a caveat.
- Tests: `golden_hier_dotted_collision` (all four decks as golden cases. `lis2json` skips or marks circuit `.`, with a selftest). `walk_is_polynomial`: the two exponential repros from the review (scoped definitions on the path, with a typo at the end of the path) at 60 levels answer in < 50 ms. Also the Fibonacci(60) test.
- `port_count_mismatch`: an `X` line whose node count differs from the port count of its callee gives an Error on an instance path (oracle `hier_port_more`, `hier_port_fewer`: HSPICE rejects both). In a definition that no instance path reaches, it gives a Warning (not probed). The totals get a note.
- Verilog-A exemption: an unresolved `X` callee gives a Warning only if the first run has an `.hdl` line, or if a `.model` card with that name is visible from the `X` line and its type is not a built-in HSPICE type. Otherwise it is an Error (oracle `hier_model_callee`, a rejected golden case).
- Short messages: a `duplicate_element` message stays below about 100 bytes, as `unresolved_subckt` does. `instances_of` gives a caveat on an `X` line whose name is duplicated.
- No list in an answer is unbounded. Parameter, default, function and override lists are capped at `MAX_LIMIT` with a total, as ports, pins and children are.
- The `Summary.expanded_note` doc lists every note. The Verilog-A note says that these lines are counted neither as instances nor as devices.

### Task 8: Parameter resolution with `parhier`

**Files:** Create `query/src/params.rs`, `query/tests/golden.rs`; modify `query/src/lib.rs`.

**Interfaces — Produces:**
```rust
impl Netlist {
  pub fn resolve_param(&self, scope_path: &str, name: &str) -> Result<Resolved, QueryError>;
  pub fn resolve_element(&self, path: &str) -> Result<ElementParams, QueryError>; // params by name: "r"/"c"/"l" for the R/C/L value, plus every key=value
}
pub struct Resolved { pub name: String, pub value: Option<f64>, pub unresolved: Option<String>, pub chain: Vec<ChainStep>, pub caveats: Vec<String> }
pub struct ChainStep { pub kind: StepKind, pub scope: String, pub text: String, pub value: Option<f64>, pub loc: Loc }
pub enum StepKind { InstanceOverride, LocalParam, SubcktDefault, GlobalParam, ParamFunction, ElementParam, Builtin }
pub struct ElementParams { pub path: String, pub kind: String, pub params: Vec<Resolved> }
```
Algorithm (per scope chain along the path, levels 0 = root … k = owning def): candidates at level j are, in order, `I_j` (override on the `X` line that instantiates level j, evaluated in level j−1), `L_j` (local `.param`, evaluated in level j), `D_j` (header default, evaluated in level j); level 0 = top-level `.param`. **GLOBAL**: scan j = 0…k, first hit wins. **LOCAL**: scan j = k…0, first hit wins. Then apply what golden `inherit`, `dup_param`, `nested_global`, `nested_local` show where they differ. Names inside the winning text are resolved recursively at the scope the text lives in; a cycle → `unresolved: "cycle: a -> b -> a"`. Statistical builtins in a `.param` and `temper` → value + caveat (`temper` = first `.temp` value, else 25). A statistical builtin directly in an element expression → `unresolved: "HSPICE rejects agauss() directly in an element expression"` (golden `stat_direct`; name the function used). A parameter or element changed by an `.alter` block → caveat `changed by .alter #n at file:line` (changes accumulate over runs: golden `alter`). An answer that depends on a duplicate `.subckt` → caveat (Task 6). `.option scale` present → caveat on W/L-like params. Unparsed element letters → `unresolved: "element type not parsed"`. Model-card params are not resolved. One bare name after the R/C/L nodes (stored as `r`/`c`/`l` text by Task 6) is a model when a `.model` of that name is visible (golden `res_model`), else a parameter reference. Element params are unique by name after Task 6a (last value wins, golden `elem_dup_kw`, `mos_kw_and_positional`). Two tokens after the R/C/L nodes are stored as model + value (`R1 a 0 rval 0.004` → model `rval`, r `0.004`; `L1 a b lval lmod` → model `lval`, l `lmod`): when no `.model` of the first word is visible, read them as value + TC1 (`r=rval`, `tc1=0.004`, golden `res_tc_positional`) or value + model (`l=lval`, model `lmod`, golden `ind_value_model`). Answers show parameter texts through `Netlist::param_text` (it removes `+` markers and comments). A V/I source with no DC value: `dc` is the value of its function at t=0 (golden `src_fn_nodc`: PULSE/SIN/EXP/SFFM first argument, PWL the value at t=0, AM 0), with a caveat; other functions → unresolved with a reason. The R/C/L value in `ElementParams` is the element's own value; the `.op` listing divides by `m`, which the golden test accounts for.

- [ ] **Step 1: Failing tests** `golden_all_resistors` — for every case dir with `expected.json`: load `deck.sp` (`title_line: Auto`), for each `resistors[]` entry `resolve_element(path)` has param `r` equal to `value * m` (`m` = the entry's `"m"`, default 1; `.op` prints R/m) with `|v - expected| <= 2e-4 * max(|expected|, 1e-30)`; cases with `extra.rejected` are skipped automatically, and so are the paths in `extra.skip_value_check` (temperature-adjusted values); `dspf_backannot` (no `.op` listing) has no resistor entries. If the data contradicts the within-level order `I > L > D`, follow the data and record it in `golden/NOTES.md`. `chain_shows_winner` (parhier_global: chain ends with `GlobalParam` value 4; parhier_local: `InstanceOverride` 3). `cycle_unresolved`. `stat_caveat`. `alter_caveat`.
- [ ] **Step 2:** FAIL. **Step 3:** implement (memoize per `(scope path, name)` within one call only; no global precompute). **Step 4:** PASS.
- [ ] **Step 5:** `git add query`.

**Task 8 close-out (added after review round 4).** The coordinator accepts the edits outside the Task 8 file list (`api.rs`, `paths.rs`, `lower.rs`, `model.rs`). Probe decks from the reviews are in the scratchpad: `oracle-t8f/`, `oracle-t8d/`, and the cost decks in `rv8/d/`. Each probe deck that settles a rule becomes a golden case with one NOTES line. A test for each item:
- **Cost.** One `resolve_element` call costs at most O(n log n) in the number of `.param` functions, calls and definitions. Index the functions of a scope by name once per call. Memoize `find_func` per (level, name, text). Give each set of defining subckts a small id, so that the creation-order memo key is O(1) to build and to hash. Prefer a deterministic work counter to wall-clock time in the test. The test fails on quadratic cost (the `funcs_*`, `adv_*` and `advmem_*` shapes at two sizes).
- **A user function with a built-in name** (`max`, `min`, `sgn`, `abs`, …): where a `.param` function of that name is visible, HSPICE uses it, with no warning. Where none is visible, the built-in is used. The creation-order rule applies to both (the probes `func_shadow_top`, `func_shadow_sub`, `func_shadow_scope`, and the rejected `func_shadow_more` and `func_shadow_more_local`). netlist-expr must ask the environment for a user function before it tries a built-in.
- **A function body has no creation-order rule.** A call inside a function body takes the nearest definition (`sticky_lazy_local`: x5.xc.rh = 3k, x7.xc.rk = 3k). Correct the NOTES line and the doc of `find_func`. Also store `sticky_lazy_local` and `func_global_more` as golden cases. The other results of these two probes agree with the tool and settle lines in "Choices not probed".
- **Sweep caveat (spec §6.2a).** The loader keeps the parameter names that a first-run `.dc`, `.data` or `.step` sweeps. An answer whose value depends on a swept name gets the caveat "swept by .dc at file:line; this is the nominal value".
- A quoted R/C/L value that continues on a `+` line is normalized with `source_args`, as `param_text` is (`r3 1 0 'rval` / `+ *3'` gives rval*3).
- `Element` keeps its staged size: `nodes_end` is a `u32` with a sentinel value, not an `Option<u32>`. Add a test that checks `size_of::<Element>()`.
- The reason for a function that is not visible hedges: "HSPICE rejects this deck or reads the name as an output variable". Store the `h` probe (`oracle-t8d/func_nested_global`) as a golden case, or reword the `func_body_undef_global` line so that it claims only what a stored listing shows.

**Task 8 close-out 2 (added after review round 7).** Rounds 5 to 7 each found one new form of a line break inside an open quote: a `$` comment, a `*` comment line, and then a blank line. So the work is in two steps. First, 8b-G gets HSPICE ground truth for the whole set of these forms. Then one fix round applies all of it.

- **8b-G probes** (golden only, one deck for each form that HSPICE can reject):
  - Store the reviewer decks in the scratchpad `oracle-t8i/` as golden cases: `quoted_blank_line` (REJECTED), `sweep_op`, `tc_tnom_dtemp`, `func_args_cmtline`, `plus_after_blank` and `body_self_ref_global`.
  - Inside an open quote: a line with only spaces and a tab, a `$` comment line, and a `+` line with no text. Probe each in a `.param` value and in an R value.
  - Text after a closing quote: `.param b='a' * 2` (the deck `oracle-t8h/tool_only/u.sp`) and `r1 1 0 'a' *2`.
  - A quoted text that continues on a `+` line in a source argument (`v1 1 0 PWL(0 '0.3` / `+ *1' 1n 1)`) and in a positional TC (`R1 1 0 rval '0.004'` / `+ '1e-5` / `+ *1'`).
  - A `key=` at the end of a line, with its value on the next `+` line (`r1 1 0 r=` / `+ 5k`).
  - Inside open parentheses with no quote: a blank line and a `*` comment line, in a `.param` function call (`max(1k,` …), in a `.param` function argument list, and in PWL parentheses.
- **Fix round:**
  - A blank or spaces-only line inside an open quote gets the same result as a `*` comment line there: no value, the HSPICE reason, and the load Error `quoted_comment_line`. Outside quotes, a blank line before a `+` line continues the statement (`plus_after_blank`).
  - Apply every 8b-G result. Where HSPICE accepts a form that the tool rejects or reads differently, fix the tool. Where HSPICE rejects a form that the tool accepts, give a load Error if the check is cheap. If not, record the form in NOTES "Known differences".
  - The coordinator accepts the `param_item` change in `src/parser/utils.rs` (`loss_sep` around the arguments of a `.param` function). Add a parser unit test for it.
  - The sweep caveat cites `sweep_op` and does not say "not probed". It says "the .op value", not "the .op before the sweep", because the listing prints the `.dc` results first.
  - The TC caveat and the NOTES TC line cite `tc_tnom_dtemp` for `dtemp`, `tnom` and `scale`.
- **Review rubric change.** Rubric rule 2 covers decks that HSPICE accepts. When HSPICE rejects a deck and the tool answers it with no diagnostic, the finding is minor. Task 13a collects these findings.

### Task 7a: Follow-ups from the Task 7 review

**Files:** `query/src/{paths.rs,hier.rs}`, `golden/**`, `golden/tools/lis2json.py`. Runs after the Task 8 close-out. Probe decks are in the scratchpad `oracle-task7f/`. Each probe that settles a rule becomes a golden case with one NOTES line. A test for each item:
- **Element paths split at the last `.`.** HSPICE splits an element path at its last `.` into a circuit name and an element name. It takes the circuit with that full name that was created first, then the element in it (`elem_dotted_name_collision`, `elem_dotted_name_collision_rev`, `hier_dotted_collision_hidden`). Replace "the first element in creation order" with this rule. Correct the caveat text and the NOTES citations (the hidden-device rule cites `hier_dotted_collision_hidden`, not `_below`).
- **lis2json** stops with an error when a circuit row in the directory does not match an `X` line of its parent's definition. So it cannot write the shifted names that `hier_dotted_collision_shift` shows. Add `hier_dotted_collision_shift` as a golden case with the shift recorded, or as a rejected-by-lis2json case, with a selftest.
- **Walk contexts** are (scope, parent-context id) pairs, not copied lists. A context changes only when a definition on the path is hot. So the "too many readings" bound fires only on decks that also get `hierarchy_too_large`. Test: the reviewer chains (n = 12…18) answer, and a 12000-level chain answers in linear time and memory.
- **port_count_mismatch** is also checked when the analysis stops, for `X` lines whose binding does not depend on the context. Store `hier_port_dynamic` and `hier_port_forms` as golden cases.
- **Verilog-A.** A visible `.model` card of a built-in type is never a Verilog-A module, even with an `.hdl` line (Error, as `hier_model_callee`). The built-in types include `s`. The Verilog-A message is below about 100 bytes.
- The caveat list of `element_info` is capped (one caveat per kind, with a count). The shared-name caveat counts the skipped duplicate instances too.
- `candidates()` uses a per-definition name index, not a scan of every element.

*Done (coordinator, inline):* a context is a short canonical list, not a (scope, parent) pair: it keeps only the scopes that are the nearest one for a name they nest, so contexts that bind every name the same way share one id. The 12000-level chain and the reviewer chains answer in linear time. The walk takes `Want::Element` (element rule) or `Want::Instance` (scopes). Two staged test expectations changed: `golden_duplicate_element` (one capped caveat) and `walk_contexts_are_pairs` (n pairs reach `p{n}`). Release on gen_big.py: load 0.07 s, `element_info` 0.001 ms; 1M-element flat deck: first query 45 ms (builds the index), then 0.001 ms.

### Task 9: Connectivity

**Files:** Create `query/src/nets.rs`, `query/tests/nets.rs`; modify `query/src/lib.rs`.

**Interfaces — Produces:**
```rust
impl Netlist {
  pub fn net_info(&self, scope_path: &str, net: &str, page: &Page) -> Result<NetInfo, QueryError>;
  // NetInfo { scope, net, is_port: bool, is_global: bool, is_ground: bool, connectivity_incomplete: bool, incomplete_due_to: Vec<String>, total: usize, connections: Paged<Conn> }
  // Conn { element, kind, pin_role, callee: Option<String> }
  pub fn trace_net(&self, scope_path: &str, net: &str, dir: TraceDir, depth: u32, page: &Page) -> Result<NetTrace, QueryError>;
  // TraceDir: Down | Up | Both ; NetTrace { hops: Paged<Hop> }  Hop { from_scope, from_net, via_instance, to_scope, to_net, direction }
}
```
Pin roles: MOSFET `d g s b` (+ `e1…` for extra nodes), BJT `c b e s`, diode `p n`, R/C/L/V/I `1 2`, `X` → the callee's port names. Ground = `0`, `gnd`, `gnd!`, `ground`, any letter case (golden `ground_alias`), at every level; `.global` names connect across all levels (golden `global_net`). Per-definition net index built lazily on first query of that definition and cached (`OnceLock` per def). Unparsed element letters → their nets get `connectivity_incomplete` with the element name.

- [ ] **Step 1: Failing tests** `net_info_inverter` (pins d of M1/M2 on `out`); `trace_down_through_ports` (top net `n1` → `xb1.a` → `xb1.x1.in`); `trace_up_from_port`; `global_and_ground` (from golden cases); `unknown_letter_incomplete` (`E1 out 0 in 0 2` → `connectivity_incomplete`); `net_info_pages_high_fanout` (net with 1,000 connections → `total` 1000, 50 items, cursor).
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** PASS.
- [ ] **Step 5:** `git add query`.

*Done (coordinator, inline):* `query/src/nets.rs`. A scope is an instance path (`""` = the root, or the top level of a bare netlist). A device path or an `X` line with no visible definition is `BadArgument`. Port names of an `X` pin come from its binding on the path, so one `X` line can show other port names in other instances (test `port_names_follow_the_binding`). An unresolved `X` line or a node count that is not the port count lists its pins as `n1, n2, …` with a capped caveat, and a trace stops there. A trace is breadth-first over (instance, net), each once, with at most 100 000 steps (`truncated` + caveat). It does not follow ground or `.global` nets through ports, because they connect by name. `NetInfo` also has `def` and `caveats`. `NetTrace` has `scope`, `net`, `truncated`, `connectivity_incomplete`, `incomplete_due_to` and `caveats`, and `Hop` has `depth` and `connectivity_incomplete`. The net index stores 8 bytes per pin. A definition with at least 1 pin for each 4 names in the deck uses an array by name id (4 bytes per name), and the others use a map. HSPICE probe `node_angle_names`: `<` and `>` are name characters, so the parser now takes them (bus bits such as `net<3>` were a `syntax_error` before). Release: 1M-resistor flat deck, `net_info` on ground (1M pins) 14–16 ms the first time (builds the index), then 1.05 ms. The index adds about 20 MB to 405 MB RSS (about 10 bytes per pin). gen_big.py: `net_info` top `vdd` 0.23 ms, then 0.013 ms. `trace_net` from it reaches 10 000 hops in 9 ms.

### Task 10: DSPF

**Files:** Create `query/src/dspf.rs`, `query/tests/dspf.rs`; modify `query/src/{lower.rs,nets.rs,lib.rs}`.

**Interfaces — Produces:** load detects a DSPF file by a `*|DSPF` line in its first 50 lines (included or top) and, for `.option sim_dspf="<scope> <file>"` (golden `dspf_backannot`: scope = a subckt name or an instance name, applied to every matching instance; no scope = top level), scans that file too and ignores its `.SUBCKT` line. When the DSPF is the netlist itself (`dspf_included`), HSPICE treats `*|` lines as comments and accepts `:` in node names. `NetInfo` gains `parasitics: Option<Parasitics>` (`total_cap: Option<f64>, r_count, r_sum, c_count, c_sum, pins`) and `net_info` gains `view: NetView` (`Logical` default | `Physical`). In the logical view every DSPF sub-node (`*|S`) and instance-pin node (`*|I`) maps to the logical net of its enclosing `*|NET` block; parasitic R/C inside DSPF net blocks are excluded from device totals and reported only in `parasitics`. Divider/delimiter/busbit come from the header (defaults `/`, `:`, `[]`).

- [ ] **Step 1: Failing tests** `dspf_logical_net` (golden `dspf_included`: `net_info("", "A")` logical view lists `M1` pin `g` although M1's line uses `M1:G`); `dspf_parasitic_sums` (r/c counts and sums match the deck; `total_cap` from `*|NET A 1.2e-15`); `dspf_not_in_device_totals` (summary resistor total == golden `totals.resistors` minus parasitic count per `extra`); `dspf_backannot` (golden `dspf_backannot`: pre-layout nets gain `parasitics`); `dspf_physical_view_pages`.
- [ ] **Step 2:** FAIL. **Step 3:** implement as a line scanner over `Files.inner` text (no second parser). **Step 4:** PASS.
- [ ] **Step 5:** `git add query`.

*Done (coordinator, inline):* `query/src/dspf.rs`. The scanner reads the `*|NET` blocks; the parser still reads the included DSPF as netlist text, and an element flag (`Element.parasitic`, in padding: still 56 bytes) keeps the block R and C out of `devices`, `expanded_devices`, `devices_by_type` and `elements`. New API: `NetView` (`net_info` takes it as its 3rd argument), `Conn.node` (physical view), `NetInfo.parasitics: Option<Parasitics>` (with `file`; no `nodes` field), `SubcktInfo.parasitics`, `Summary.expanded_parasitics` and `Summary.dspf: Vec<DspfFile>`. Diagnostics (Warning): `dspf_file_missing`, `dspf_scope_unknown`, `dspf_net_unmatched`. `trace_net` follows logical nets. The plan's `net_info("", "A")` asks in scope `xi` (see NOTES, differences). Found while measuring: an internal DSPF net is often named only on its `*|NET` line, so the loader adds each block name to the names (test `dspf_net_named_only_in_its_block`). Release, a generated DSPF of 42 MB (100k nets, 400k R, 400k C, 100k MOSFETs): load 0.80 s, where the same text without the `*|DSPF` line loads in 0.54 s (the scan is about 0.21 s); 451 MB RSS against 405 MB; `net_info` on a net 12 ms the first time (builds the net index), then 0.001 ms, in both views.

### Task 11: Python library

**Files:** Modify `pybind/Cargo.toml` (dep `netlist-query`, `serde_json`), `pybind/src/lib.rs`, `pybind/netlist_db/__init__.py`, `pybind/netlist_db/__init__.pyi`; create `pybind/tests/test_api.py`.

**Interfaces — Produces (Python):**
```python
netlist_db.load(path, top=None, lib_override=None, title_line="auto") -> Netlist  # lib_override: list of (file, from_section, to_section)
class Netlist:
    def summary(self) -> dict; def diagnostics(self, level=None, limit=50, cursor=None) -> dict
    def list_subckts(self, filter=None, sort="name", limit=50, cursor=None) -> dict
    def subckt_info(self, name, limit=50, cursor=None) -> dict; def instances_of(self, name, limit=50, cursor=None) -> dict
    def element_info(self, path) -> dict; def resolve_param(self, scope_path, name) -> dict; def resolve_element(self, path) -> dict
    def net_info(self, scope_path, net, view="logical", limit=50, cursor=None) -> dict
    def trace_net(self, scope_path, net, direction="down", depth=3, limit=50, cursor=None) -> dict
```
Rust `#[pyclass(frozen)]` holds `netlist_query::Netlist`; methods return JSON strings; `__init__.py` wraps them with `json.loads`. Query errors come back as `{"error": {"kind": ..., "message": ..., "suggestions": [...]}}`, never as exceptions; only load I/O failures raise `OSError`. Drop the stale `polars` stubs. Venv: `python3 -m venv .venv && .venv/bin/pip install maturin pytest mcp`; build with `cd pybind && ../.venv/bin/maturin develop --release`.

- [ ] **Step 1: Failing tests** (`.venv/bin/pytest pybind/tests -q`): `test_load_and_summary` (golden `hier_counts`); `test_counts_match_rust` (copies equal `extra.marker_counts`); `test_resolve_param_chain` (parhier_global → 4.0, chain last kind `GlobalParam`); `test_error_is_dict`; `test_net_info_paging`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** PASS.
- [ ] **Step 5:** `git add pybind`.

*Done (coordinator, inline):* the `pybind` crate now depends on `netlist-query` only (not on `netlist-db` with its `py` feature), and every query releases the GIL (`py.detach`). Bad enum strings (`sort`, `view`, `direction`, `level`), a negative `limit` or `depth`, and a bad cursor come back as `{"error": {"kind": "bad_argument", …}}`; `load` raises `OSError` (cannot read) or `ValueError` (bad `title_line`, unknown `top`). `Level` and `TraceDir` gained `FromStr`, and `TraceDir` serializes in lowercase. Tests: the 5 planned, plus `test_dspf_and_views` and `test_lib_override` (7 pass). `.venv/` is in `.gitignore`.

### Task 12: MCP server

**Files:** Create `mcp_server/pyproject.toml`, `mcp_server/netlist_mcp/{__init__.py,server.py}`, `mcp_server/tests/test_server.py`, `mcp_server/README.md`.

**Interfaces — Produces:** FastMCP server `netlist-db` with tools `load_netlist(path, top=None, lib_override=None, title_line="auto") -> {"handle": str, "summary": dict}`, `unload_netlist(handle)`, and one tool per Python method in Task 11, each taking `handle` first. Handles are `"n1"`, `"n2"`, … held in a process-wide dict; loading the same resolved path + options again returns the existing handle. Tool docstrings tell the LLM: paths are lowercase, `""` is the top level, lists are paged with `cursor`. Entry point `python -m netlist_mcp` (stdio). No query logic in Python.

- [ ] **Step 1: Failing tests** (call tools in-process via `FastMCP.call_tool`): `test_load_then_list`, `test_handle_reuse`, `test_unknown_handle_error`, `test_resolve_tool_matches_library`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** `.venv/bin/pytest mcp_server/tests -q` PASS; `timeout 20 .venv/bin/python -m netlist_mcp` starts and waits on stdio.
- [ ] **Step 5:** `git add mcp_server`.

*Done (coordinator, inline):* `mcp` 2.x (installed: 2.3.0) renamed FastMCP to `MCPServer` (`mcp.server.mcpserver`), so the server uses `MCPServer`; `pyproject.toml` pins `mcp>=2,<3`. Tools return compact JSON text (a `dict` return becomes indented JSON, which costs the LLM tokens) and never raise (an exception reaches the client as `UnexpectedToolError`): errors are `{"error": …}` with kinds `unknown_handle`, `load_failed`, `bad_argument` and the library's kinds. `load_netlist` also takes `reload` and returns `reused`; `python -m netlist_mcp` needs `netlist_mcp/__main__.py`. Tests: the 4 planned plus `test_tools_have_docs` (5 pass). A stdio client (`mcp.client.stdio`) started `python -m netlist_mcp`, listed 12 tools and called `load_netlist` and `net_info`.

### Task 13: Performance gate and documentation

**Files:** Create `query/tests/perf.rs` (`#[ignore]`, run explicitly), `query/benches/gen_big.py`; modify `README.md` (new section "LLM query tool").

- [ ] **Step 1:** `gen_big.py` writes the 1,000-cell × 102-device × 10-instance deck (1.02M expanded) and a variant with 10× the expanded instances, made by one more level of hierarchy (not by 10× more `X` lines, which adds input lines and so memory; Task 6 measurement). `perf.rs`: release load < 1.0 s; `list_subckts`, `resolve_element("x5_3.m7")`, `net_info("x5_3","n4")` each < 10 ms; peak RSS of the 10×-instance variant < 1.3 × the base (memory must not scale with expanded count).
- [ ] **Step 2:** `timeout 180 cargo test --release -p netlist-query --test perf -- --ignored` with `/usr/bin/time -l` → PASS; record numbers in README.
- [ ] **Step 3:** README: install, Python example, MCP config snippet, the parhier default, caveat meanings, limits (model cards text-only, unparsed letters).
- [ ] **Step 4:** Full suite: `cargo test --workspace`, `.venv/bin/pytest pybind/tests mcp_server/tests -q` → all PASS.
- [ ] **Step 5:** `git add query README.md`.

*Done (coordinator, inline):* `query/benches/gen_big.py` (`--deep`: one more level, a group that calls the cell 10 times: 110,000 instances, 10.2M devices, about 10% more lines) and `query/tests/perf.rs` (`perf_gate`, and `perf_child`, which the gate starts as a child process per deck to read its peak RSS with `getrusage`; `libc` is a dev-dependency). The planned `resolve_element("x5_3.m7")` check expects W = 1u, not the `X` line's 2u: under the default `parhier=global` the top-level `.param wn=1u` wins. Apple M1 Pro, release: load 0.07 s, `list_subckts` 0.2 ms, `resolve_element` 0.6–0.7 ms, `net_info` 0.02–0.03 ms, peak RSS 79 MB, deep variant 91–93 MB (1.16–1.18x). README section "LLM query tool" (its Python example was run).

### Task 13a: Diagnostics for decks that HSPICE rejects

The reviews of Task 8 and later record, as minor findings, the decks that HSPICE rejects but the tool answers with no diagnostic. The coordinator copies each finding here, with its deck and the HSPICE error. For each one: store the deck as a rejected golden case, then give a load Error if the check is linear in the input size. If it is not, record the form in NOTES "Known differences". Test: `rejected_decks_have_errors` checks that every rejected golden case gives at least one load Error, except the cases that NOTES lists under "Known differences".

*Done (coordinator, inline):* 11 rejected cases loaded with no Error; parameter resolution already gave the reason for most of them, at query time. New module `query/src/rejects.rs` (called from the hierarchy analysis, one pass over the lines, Errors on instance paths only): `undefined_param` (a top-level `.param` or function body that names a name with no top-level definition: `pe_scope_global`, and the new probe `param_undef_unused` showed that HSPICE rejects it also when unused; a simple R/C/L value that names a name defined nowhere: `unquoted_space`), `override_self` (top-level `X` line only: `ovr_self_global`), `statistical_in_element` (`stat_direct`), `name_then_model` (`res_name_model`, through the resolver's own `rcl_items`, only for an R value that is a model name; the parser's `value_then_model` stays apart), `inductor_model_level` (`ind_model_nolevel`; `ModelDef.level` is new). The name checks count `X`-line override names as definitions (else `pe_scope_local` got a false Error) and are off after a load Error (else a skipped `.param` gave a cascade in `brace_quoted_param`). Known differences (NOTES, new section): `m_expr_zero`, `func_shadow_more`, `func_shadow_more_local`, `func_body_undef_global`, `pulse_r_noparen`. Tests `query/tests/rejects.rs` (4: `rejected_decks_have_errors`, `load_checks_name_their_form`, `load_checks_quiet_on_accepted_decks`, `load_checks_scope`); the `unquoted_space` assertion of `comment_line_in_quotes` now expects its Error. Perf gate load 0.07 → 0.08 s.

### Whole-branch review and fix pass

*Done (coordinator, inline):* three reviewers in parallel (Python/MCP and perf; DSPF and connectivity; Task 13a and Task 7a), then one fix pass.

- Python/MCP (12 items): the server stops at start when it finds the PyPI `netlist_db` (no `load`); `""` means "not given"; `load_netlist` and every tool return kind `internal` on an exception; paths use `abspath`, not `realpath` (a symlinked deck reads its includes beside the link); a NUL in a path gives `bad_argument`; handle keys ignore letter case. pybind: `catch_unwind` in every query (an `internal` error) and in `load` (`RuntimeError`); a negative `trace_net` depth gives `bad_argument`; Python ints are clamped to i64; `py.typed`. Perf gate: also a two-level deck (1.11M instances) against an unused control with the same lines (< 1.1x); the plan's 10x gate stays.
- DSPF and connectivity: the scanner picks the parasitics of each block (a union-find over the block's resistors that touch no other net; a capacitor with a node of the block; a `* Instance Section` line ends a block). Other R and C lines are devices (Warning `dspf_device_in_block`). `Parasitics.r_sum`/`c_sum` are `None` when a value was not read, and `Parasitics.caveats` says so (also the `sim_dspf_rail` caveat). A block named for ground holds ground, and `trace_net` up from it no longer panics. A node that only parasitics touch gets a caveat, not NotFound. NotFound suggestions leave out the asked name. The up-trace stops at an `X` line with the wrong node count. A port that is also `.global` or ground gets a caveat. Element lines that a load error skipped make `net_info` and `trace_net` incomplete (`incomplete_due_to` names them).
- Task 13a: the name checks are off with a `.prot` region (C1). The arguments of a call that is neither a built-in nor a `.param` function are not names (I1). The function-body check is removed (I2, probe `func_body_unused`). The top-level override check and the L check are linear (I3: 2.25 s to 0.07 s; I4: 1.69 s to 0.07 s; 100k failing L lines: 1.72 s to 0.09 s). Only the last `.param` definition is checked (M1). A `.param` function hides `unif` (M2, also in the resolver). The statistical check stays on every element (probe `stat_in_mos`) and names the key (M3). The last `level` key wins (probe `ind_level_twice`, M4). NOTES "Known differences" wording (M5), `expanded_note` doc (M6), lazy `place` (M7). M8 (`hierarchy_too_large`) is not changed: it follows the m/port policy. Resolver: a call of an unknown name (`v(out)`) gives the "not visible" reason before its arguments are read, and in a deck with `.prot` an undefined name gets a `.prot` reason, not "HSPICE rejects".
- 9 new golden cases from the review probes: `prot_param` and `rcl_output_var` (`skip_value_check` r1, r2: skipped 16 to 20), `param_output_var`, `func_body_unused`, `param_redefined_text`, `stat_user_func`, and REJECTED `stat_in_mos`, `ind_level_twice`, `func_zero_args` (new load Error `func_no_args`).
- Deviation from Task 13a: `m_expr_zero` (its path-independent form) and `pulse_r_noparen` have linear checks that are not done (NOTES "Known differences").
- Measured (release): perf gate load 0.083 s, `list_subckts` 0.22 ms, `resolve_element` 0.59 ms, `net_info` 0.02 ms, RSS 1.16x (10x instances) and 1.00x (111x against the control). A generated 33 MB DSPF (100k nets, 400k R, 400k C, 100k MOSFETs) loads in 0.84 s (0.74 s before the fix pass; the same text with no `*|DSPF` line 0.53 s).

## Execution order

Task 0 → {Task 1 ∥ Task 2 ∥ Task 3} → {3a-P ∥ (3a-G → 3a-E)} → Task 4 → Task 5 → 5a-G → 5a-P → Task 6 → {6a-G ∥ 6a-Q} → 6a-P → 6b-G → 6a-P close-out → Task 7 → Task 8 → Task 8 close-out → 8b-G → Task 8 close-out 2 → Task 7a → Task 9 → Task 10 → Task 11 → Task 12 → Task 13 → Task 13a → whole-branch review.
Task 2 must read `golden/NOTES.md` (Task 3) for `^` and rejected builtins before finishing; if Task 3 is not done yet, Task 2 finishes the rest and leaves those two items to a follow-up fix after Task 3.
