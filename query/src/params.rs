//! Parameter resolution (plan Task 8): the value of a parameter as seen from
//! an instance path, under `.option parhier`, with the chain of the
//! definitions that gave it.
//!
//! The scopes of a path are its levels: level 0 is the top level, level `j`
//! the instance at the `j`-th place of the path (with `top=`, the
//! definition and the definitions it is written inside follow the top
//! level). At level `j` a name can have three definitions, tried in this
//! order: the override on the `X` line that makes the instance (`I`), a
//! `.param` in the body of its definition (`L`) and a default on its
//! `.subckt` line (`D`), all evaluated at level `j`; in the text of `I`, the
//! overridden name itself is looked up at level `j-1` (golden
//! `ovr_scope_local`, `ovr_self_local`). At the top level only `.param`
//! lines count. GLOBAL (HSPICE's default) tries the levels from the top
//! level down, LOCAL from the instance up; the first definition found wins
//! (golden/NOTES.md: `parhier_*`, `nested_global`, `nested_local`,
//! `inherit`, `inherit_local`). `.param` functions are looked up in the
//! GLOBAL order; under LOCAL the nearest definition up the path (the scope
//! of the call first), then the top level, except that a text keeps the
//! top-level function once an instance created before this one used it
//! for the same text (see [`Resolver::find_func`]). A `.param` function
//! with the name of a built-in function hides it where it is visible. In
//! one scope the
//! last definition of a name wins, also one after the use (golden
//! `dup_param`, `func_redef`); a file or `.lib` section read twice counts
//! at each place it is read (`inc_once_param_only`, `inc_twice_param_only`,
//! `lib_*_param_only`), which the model keeps in document order. The names
//! in the winning text are resolved at the level it is evaluated in, not
//! where the name is used (golden `pe_scope_local`).
//!
//! Everything is computed on demand for one path, and memoized within one
//! call only.

use crate::{
    Netlist,
    api::{ChainStep, DEFAULT_LIMIT, ElementParams, MAX_LIMIT, QueryError, Resolved, StepKind},
    hier::suggest,
    intern::Sym,
    lower::{QuoteFault, after_eq, arg_words, next_token, one_line, pwl_values, quote_fault},
    model::{
        DefId, ElemIdx, ElemKind, Element, FuncDef, LocIdx, ModelDef, ParamDef, Parhier, ROOT, Root,
    },
    paths::{Walk, Want, walk},
};
use netlist_expr::{Caveat, Env, EvalError, Expr};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    cmp::Ordering,
    collections::{HashMap, HashSet},
    rc::Rc,
};

/// Name references nested deeper than this (a name whose text uses a name
/// whose text uses …) are unresolved, so that no deck can overflow the
/// stack.
const MAX_DEPTH: usize = 256;

/// The largest sum of the heights of the expressions being evaluated: each
/// level of an expression is a frame of the evaluator.
const MAX_HEIGHT: usize = 2048;

/// The words of a reason after which HSPICE stops (see
/// [`Netlist::rcl_check`]).
const REJECTS: &str = "rejects the deck";

/// The statistical functions of `netlist_expr`.
pub(crate) const STATISTICAL: [&str; 4] = ["agauss", "gauss", "aunif", "unif"];

/// The transient functions of a V/I line.
const SOURCE_FUNCS: [&str; 7] = ["pulse", "sin", "exp", "pwl", "sffm", "am", "pat"];

/// The element parameters that `.option scale` multiplies.
const GEOMETRIC: [&str; 8] = ["w", "l", "ad", "as", "pd", "ps", "area", "pj"];

/// The stack of the thread that answers one query. Names are resolved
/// recursively, up to [`MAX_DEPTH`] levels of a few KiB each (about 12 KiB
/// in a debug build), whatever the stack of the caller: a thread that is
/// not the main thread has 512 KiB on macOS (a Python thread, for one).
/// Only the pages that are used are committed.
const STACK: usize = 64 << 20;

/// The most expressions that one call evaluates (each name definition,
/// each function call with new arguments, each word of a source). A deck
/// whose functions call each other with ever new arguments
/// (`fi(x)='f{i-1}(2*x)+f{i-1}(2*x+1)'`) needs work that doubles with each
/// level; this bound keeps one call to about 0.3 s and 100 MB.
const MAX_EVALS: usize = 100_000;

const TOO_DEEP: &str = "parameter references are nested too deeply (more than 256 levels, or very large expressions): not resolved";

const TOO_MUCH: &str = "the value needs more than 100000 evaluations (functions called with very many different arguments, or very many names): not resolved";

const UNDEFINED: &str = "HSPICE rejects a deck with an expression that names a parameter not defined where the expression is evaluated (golden `pe_scope_global`: \"Undefined parameter or function definition\")";

/// The reason for a name that no scope of the path defines.
fn undefined(net: &Netlist, name: &str) -> String {
    if net.protected {
        format!(
            "`{name}` is not defined on this path, or a `.prot` region defines it: the loader does not read `.prot` regions (diagnostic `protected_region`), and HSPICE uses the definitions inside them (golden `prot_param`)"
        )
    } else {
        format!("`{name}` is not defined on this path: {UNDEFINED}")
    }
}

/// The reason of a call of a function that no scope of the call defines.
fn not_visible(name: &str) -> String {
    format!(
        "no `.param` function `{name}` is visible where this text is evaluated: HSPICE rejects this deck or reads the name as an output variable (golden `func_body_undef_global`: \"Undefined function definition\"; `func_nested_global`: `h(...)` is read as an output variable)"
    )
}

const OUT_OF_RANGE: &str =
    "the value is out of the range of a 64-bit float (an overflow): not resolved";

/// The longest expression text quoted in a reason, in characters.
const QUOTE_MAX: usize = 120;

/// The most caveats of one answer (of one [`Resolved`], and of the whole
/// [`ElementParams`]); a last caveat counts the others.
const MAX_CAVEATS: usize = DEFAULT_LIMIT;

/// Keeps the first `MAX_CAVEATS - 1` caveats of `c` and a count of the
/// others, when `c` has more than [`MAX_CAVEATS`].
fn cap_caveats(c: &mut Vec<String>) {
    if c.len() > MAX_CAVEATS {
        let more = c.len() - (MAX_CAVEATS - 1);
        c.truncate(MAX_CAVEATS - 1);
        c.push(format!(
            "{more} more caveats are not listed (an answer lists at most {MAX_CAVEATS})"
        ));
    }
}

const BRACES: &str = "HSPICE P-2019.06 reads `{` and `}` as `[` and `]` and rejects the deck (golden `param_braces`, `brace_quoted_param`): no value";

/// The reason for a text with a `$` inside quotes (see
/// `crate::lower::one_line`, which keeps it there).
const DOLLAR: &str = "the text has a `$` inside quotes: HSPICE P-2019.06 does not read it as the start of a comment there, and it rejects the deck (golden `rcl_quoted_dollar`: \"Undefined parameter or function definition\"; `rcl_quoted_dollar_line`: \"Illegal character in statement\"): no value";

/// The reason for a text with a line that HSPICE does not join to it (see
/// `crate::lower::one_line`, which keeps it on a line of its own): a line
/// inside quotes that is not a `+` line, or a `*` comment line inside open
/// parentheses with no quotes.
const COMMENT_LINE: &str = "the text has a line that HSPICE P-2019.06 does not join to it, and it rejects the deck: a line inside quotes that is not a `+` line, which is a `*` or `$` comment line, a blank line or a line with only spaces (golden `quoted_cmtline`, `quoted_dollarline_elem`, `quoted_blank_line`, `quoted_spaces_elem`: \"string misses dexter quotation mark\"), or a `*` comment line inside open parentheses with no quotes (golden `paren_cmtline_call`: \"Illegal character in statement\"): no value";

/// One level of the scopes of a path.
#[derive(Debug, Clone)]
struct Scope {
    def: DefId,
    /// The `X` line that makes the instance: the definition that holds
    /// it and its index there. `None` for the top level and for `top=`.
    x: Option<(DefId, ElemIdx)>,
    /// The instance path, lowercase (`""` for the top level).
    path: String,
}

/// The scopes of the walked path `w`: the top level, then each instance of
/// the path; with `binding`, also the instance that the last `X` line
/// makes. With `top=` set to a definition, it comes after the top level,
/// and a nested one after the definitions it is written inside, outermost
/// first, as if instantiated in an instance of each (the rule of the
/// hierarchy analysis for its `X` lines).
fn scopes(net: &Netlist, w: &Walk, binding: bool) -> Vec<Scope> {
    // `top=` and the definitions it is written inside have no instance
    // path: `(outer>deep)`
    let top = |def| Scope {
        def,
        x: None,
        path: if def == ROOT {
            String::new()
        } else {
            format!("({})", net.def_display(def))
        },
    };
    let mut out = Vec::with_capacity(w.defs.len() + 2);
    let mut outer = vec![w.defs[0]];
    while let Some(p) = net.defs[outer[outer.len() - 1].idx()].parent
        && p != ROOT
    {
        outer.push(p);
    }
    if w.defs[0] != ROOT {
        out.push(top(ROOT));
    }
    out.extend(outer.into_iter().rev().map(top));
    let mut path = String::new();
    let extra = (binding && w.last_binding.is_some()) as usize;
    let last = w.last_binding;
    for (k, &(holder, i)) in w.elems.iter().enumerate().take(w.defs.len() - 1 + extra) {
        if !path.is_empty() {
            path.push('.');
        }
        let name = net.syms.name(net.defs[holder.idx()].elements[i].name);
        path.push_str(&name.to_ascii_lowercase());
        let def = w.defs.get(k + 1).copied().or(last).expect("a binding");
        out.push(Scope {
            def,
            x: Some((holder, i)),
            path: path.clone(),
        });
    }
    out
}

/// A definition that took part in a value: one for each winning
/// definition and level of evaluation, each function call and `temper`.
/// The nodes of one call form a graph with no cycle; a chain is a walk of
/// it ([`Resolver::answer`]), so a name used many times costs one node.
#[derive(Debug)]
struct Node {
    step: ChainStep,
    /// The caveats of this definition's own text.
    caveats: Vec<String>,
    /// The nodes that its text uses, in order.
    deps: Vec<u32>,
    /// Neither this node nor a node it depends on has a caveat.
    quiet: bool,
}

/// A value and the node that gave it (`None`: no definition, or a cycle).
#[derive(Debug, Clone)]
struct Val {
    value: Result<f64, String>,
    node: Option<u32>,
}

/// What one evaluation collects: the nodes of the names and functions that
/// its text uses, and the caveats of the text itself.
#[derive(Debug, Default)]
struct Frame {
    deps: Vec<u32>,
    caveats: Vec<String>,
    /// The notes of [`Resolver::find_func`] (by id) already in `caveats`.
    notes: HashSet<u32>,
}

/// The definition of a name that wins at some level.
#[derive(Clone, Copy)]
struct Cand<'n> {
    kind: StepKind,
    /// The level whose scope holds the definition.
    level: usize,
    p: &'n ParamDef,
    /// The level the text is evaluated at.
    eval_level: usize,
}

/// The kind of the text being evaluated at a level. It gives the scope of
/// an override's own name, and the instances that evaluate the same text
/// (see [`Resolver::sticky`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Text {
    /// A text that every instance of the definition of the level evaluates:
    /// an element line, the `m` of an `X` line, a word of a source, a
    /// top-level text.
    Body,
    /// The body `.param` or the `.subckt` default of this name: an
    /// instance evaluates it only when the `X` line that makes the instance
    /// does not override the name (golden `func_sticky_ovr_local`).
    Default(Sym),
    /// An override of this name on the `X` line that makes the level. It
    /// is evaluated in the scope of the new instance, but the name itself
    /// is looked up one level up, in the parent's scope (golden
    /// `ovr_scope_local`, `ovr_self_local`). The instances that this line
    /// makes evaluate it.
    Override(Sym),
    /// The body of a `.param` function, at the level of its definition.
    /// Its calls take the nearest definition, with no creation-order rule
    /// (golden `sticky_lazy_local`: x5.xc.rh, x7.xc.rk).
    Func,
}

/// The instances that evaluate a text, for [`Resolver::sticky`]: the walk
/// of an override does not depend on the overridden name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Unit {
    Body,
    Default(Sym),
    Override,
}

/// The most `X` lines that [`Resolver::sticky`] reads in one call, over
/// all its walks; after that, the check gives [`Sticky::Unknown`].
const MAX_STICKY: usize = 4_000_000;

const TOO_LARGE: &str = "the walks of the hierarchy in this call read more than 4000000 `X` lines";

/// See [`Resolver::sticky`].
#[derive(Debug, Clone)]
enum Sticky {
    No,
    /// Yes: the first such instance.
    Top(Rc<First>),
    /// Not known, for this reason.
    Unknown(&'static str),
}

/// An instance that a creation-order walk found.
#[derive(Debug)]
struct First {
    /// Its instance path.
    path: String,
    /// The `X` lines that make it and its ancestors, from the root (the
    /// definition that holds each, its index there).
    lines: Vec<(DefId, ElemIdx)>,
}

impl First {
    /// The `X` line that makes the instance.
    fn line(&self) -> (DefId, ElemIdx) {
        *self.lines.last().expect("an instance below the root")
    }
}

/// The definitions other than the top level that define a function in the
/// first run, in order, by function name: each set has a small id, so that
/// a memo key holds the set at O(1) cost, and functions defined in the
/// same subckts share one walk. Set 0 is empty.
struct Definers {
    by_name: HashMap<Sym, u32>,
    sets: Vec<Rc<[DefId]>>,
}

/// What a call of a function name uses (see [`Resolver::find_func`]).
#[derive(Debug, Clone, Copy)]
enum Found<'n> {
    /// The `.param` function defined at this level.
    User(usize, &'n FuncDef),
    /// The built-in function of the name.
    Builtin,
    /// No function: the reason (a note of [`Resolver::notes`]).
    Nothing(u32),
}

/// The functions of one definition with one name: the index of the last
/// first-run one, and those of `.alter` blocks.
#[derive(Debug, Default)]
struct FuncEntry {
    last: Option<u32>,
    alters: Vec<u32>,
}

/// The resolution of the names of one path, for one call.
struct Resolver<'n> {
    net: &'n Netlist,
    scopes: Vec<Scope>,
    nodes: Vec<Node>,
    /// By (level, name).
    memo: HashMap<(usize, Sym), Val>,
    /// By (definition, level of evaluation).
    by_def: HashMap<(*const ParamDef, usize), Val>,
    /// By (level of the definition, function, the bits of the arguments).
    by_call: HashMap<(usize, Sym, Vec<u64>), Val>,
    /// The definitions being evaluated (definition, level, name),
    /// outermost first: one met again is a cycle.
    stack: Vec<(*const ParamDef, usize, Sym)>,
    /// The functions being called (level of the definition, name).
    calls: Vec<(usize, Sym)>,
    /// The frames of the evaluations in progress.
    frames: Vec<Frame>,
    /// The sum of the heights of the expressions being evaluated.
    height: usize,
    /// The expressions evaluated so far (at most [`MAX_EVALS`]).
    evals: usize,
    /// How many times a limit ([`TOO_DEEP`], [`TOO_MUCH`]) stopped an
    /// evaluation. A value computed while this grew depends on where the
    /// limit was met, so it is not memoized.
    limited: usize,
    temp: Option<Val>,
    /// The `Sym` of `m` (not a subckt parameter on an `X` line).
    m: Option<Sym>,
    /// The lookups in long lists, for this call (see
    /// [`Resolver::last_of`]).
    scans: RefCell<HashMap<List, Scan>>,
    /// The `.param` lines of `.alter` blocks by name, built at the first
    /// use in this call.
    alter_params: Option<HashMap<Sym, Vec<(u32, LocIdx)>>>,
    /// By (level, the instances that evaluate the text, the set of
    /// definitions of the function, the instances after the one of the
    /// level): see [`Resolver::sticky`] and [`Resolver::sticky_later`].
    sticky: RefCell<HashMap<(usize, Unit, u32, bool), Sticky>>,
    /// The `X` lines that [`Resolver::sticky`] read in this call.
    sticky_visits: Cell<usize>,
    /// The definitions of each function name, built at the first use in
    /// this call.
    definers: RefCell<Option<Definers>>,
    /// By (level, function name, text): see [`Resolver::find_func`].
    found: HashMap<(usize, Sym, Text), (Found<'n>, Option<u32>)>,
    /// The caveats and reasons of [`Resolver::find_func`], by id.
    notes: Vec<String>,
    /// The functions of the definitions with long lists of functions, by
    /// name, built at the first lookup in this call.
    func_index: HashMap<DefId, HashMap<Sym, FuncEntry>>,
    /// A count of the steps of this call (the items of lists read, memo
    /// lookups, evaluations, `X` lines walked): it grows linearly with the
    /// size of the deck and of the expressions that the call reads.
    work: Cell<u64>,
    /// The number of walks of [`Resolver::sticky`] in this call, and the
    /// walk that last reached each node of the hierarchy graph (built at
    /// the first walk).
    reached: RefCell<(u32, Vec<u32>)>,
}

/// The lookups in one long list in one call: a count of linear scans, then
/// an index (the last first-run item of each name).
enum Scan {
    Count(u32),
    Index(HashMap<Sym, u32>),
}

/// A list of definitions of names that [`Resolver::last_of`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum List {
    /// The `.param` lines of a definition.
    Params(DefId),
    /// The defaults on the `.subckt` line of a definition.
    Defaults(DefId),
    /// The parameters of an `X` line (the definition that holds it, its
    /// index there).
    Overrides(DefId, ElemIdx),
}

/// A definition with at least this many `.param` functions gets an index
/// of them by name at the first lookup in a call.
const SHORT_FUNCS: usize = 16;

/// A list with at least this many items (`.param` lines, `.subckt`
/// defaults, `X`-line overrides) gets an index after [`SCANS`] linear
/// scans in one call, so that a call that looks up many names costs time
/// linear in the size of the deck.
const BIG_SCOPE: usize = 256;
const SCANS: u32 = 16;

impl<'n> Resolver<'n> {
    fn new(net: &'n Netlist, scopes: Vec<Scope>) -> Self {
        Resolver {
            net,
            scopes,
            nodes: Vec::new(),
            memo: HashMap::new(),
            by_def: HashMap::new(),
            by_call: HashMap::new(),
            stack: Vec::new(),
            calls: Vec::new(),
            frames: Vec::new(),
            height: 0,
            evals: 0,
            limited: 0,
            temp: None,
            m: net.syms.get("m"),
            scans: RefCell::new(HashMap::new()),
            alter_params: None,
            sticky: RefCell::new(HashMap::new()),
            sticky_visits: Cell::new(0),
            definers: RefCell::new(None),
            found: HashMap::new(),
            notes: Vec::new(),
            func_index: HashMap::new(),
            work: Cell::new(0),
            reached: RefCell::new((0, Vec::new())),
        }
    }

    /// Counts `n` steps of work (see [`Resolver::work`]).
    fn step(&self, n: usize) {
        self.work.set(self.work.get().saturating_add(n as u64));
    }

    /// The last first-run `.param` of `n` in the body of `def`.
    fn local_param(&self, def: DefId, n: Sym) -> Option<&'n ParamDef> {
        let params = &self.net.defs[def.idx()].params;
        self.last_of(List::Params(def), params, n)
    }

    /// The last first-run item named `n` of `list` (the list `key`): a
    /// linear scan, or, for a long list scanned [`SCANS`] times in this
    /// call, an index.
    fn last_of(&self, key: List, list: &'n [ParamDef], n: Sym) -> Option<&'n ParamDef> {
        let scan = || {
            self.step(list.len());
            list.iter().rfind(|p| p.name == n && p.alter.is_none())
        };
        if list.len() < BIG_SCOPE {
            return scan();
        }
        let mut scans = self.scans.borrow_mut();
        let entry = scans.entry(key).or_insert(Scan::Count(0));
        if let Scan::Count(c) = entry {
            *c += 1;
            if *c <= SCANS {
                return scan();
            }
            // later items replace earlier ones: the last one wins
            self.step(list.len());
            let index = list
                .iter()
                .enumerate()
                .filter(|(_, p)| p.alter.is_none())
                .map(|(i, p)| (p.name, u32::try_from(i).expect("fewer than 2^32 items")))
                .collect();
            *entry = Scan::Index(index);
        }
        match entry {
            Scan::Index(index) => index.get(&n).map(|&i| &list[i as usize]),
            Scan::Count(_) => unreachable!("indexed above"),
        }
    }

    /// Adds the node of `step`, whose text's evaluation collected `frame`.
    fn node(&mut self, step: ChainStep, frame: Frame) -> u32 {
        let id = u32::try_from(self.nodes.len()).expect("fewer than 2^32 nodes");
        let quiet =
            frame.caveats.is_empty() && frame.deps.iter().all(|&d| self.nodes[d as usize].quiet);
        self.nodes.push(Node {
            step,
            caveats: frame.caveats,
            deps: frame.deps,
            quiet,
        });
        id
    }

    /// The answer for `name`: its value, the chain (`root`, then the nodes
    /// `frame.deps` use, depth first, each once, at most [`MAX_LIMIT`]
    /// steps) and the caveats (those of `frame`, then those of the nodes).
    fn answer(
        &self,
        name: String,
        value: Result<f64, String>,
        root: Option<ChainStep>,
        frame: &Frame,
    ) -> Resolved {
        let mut chain: Vec<ChainStep> = root.into_iter().collect();
        let mut caveats: Vec<&str> = Vec::new();
        let mut known: HashSet<&str> = HashSet::new();
        for c in &frame.caveats {
            if known.insert(c) {
                caveats.push(c);
            }
        }
        let mut truncated = false;
        let mut seen: HashSet<u32> = HashSet::new();
        let mut todo: Vec<u32> = frame.deps.iter().rev().copied().collect();
        while let Some(id) = todo.pop() {
            if !seen.insert(id) {
                continue;
            }
            let node = &self.nodes[id as usize];
            if chain.len() < MAX_LIMIT {
                chain.push(node.step.clone());
            } else {
                truncated = true;
            }
            for c in &node.caveats {
                if known.insert(c) {
                    caveats.push(c);
                }
            }
            todo.extend(node.deps.iter().rev());
        }
        let mut caveats: Vec<String> = caveats.into_iter().map(str::to_owned).collect();
        if truncated {
            caveats.push(format!(
                "the chain has more than {MAX_LIMIT} steps: only the first {MAX_LIMIT} are listed"
            ));
        }
        let (value, unresolved) = match value {
            Ok(v) if v.is_finite() => (Some(v), None),
            Ok(_) => (None, Some(OUT_OF_RANGE.to_owned())),
            Err(why) => (None, Some(why)),
        };
        Resolved {
            name,
            value,
            unresolved,
            chain,
            caveats,
        }
    }

    /// The definition of `n` at level `j` alone: `I`, then `L`, then `D`;
    /// the last one of each kind.
    fn at_level(&self, j: usize, n: Sym) -> Option<Cand<'n>> {
        let net = self.net;
        let s = &self.scopes[j];
        if let Some((holder, i)) = s.x
            && Some(n) != self.m
        {
            let d = &net.defs[holder.idx()];
            let list = d.element_params(&d.elements[i]);
            if let Some(p) = self.last_of(List::Overrides(holder, i), list, n) {
                // evaluated in the new instance's scope (golden
                // `ovr_scope_local`, `ovr_scope_child_global`)
                return Some(Cand {
                    kind: StepKind::InstanceOverride,
                    level: j,
                    p,
                    eval_level: j,
                });
            }
        }
        let d = &net.defs[s.def.idx()];
        if let Some(p) = self.local_param(s.def, n) {
            let kind = if s.def == ROOT {
                StepKind::GlobalParam
            } else {
                StepKind::LocalParam
            };
            return Some(Cand {
                kind,
                level: j,
                p,
                eval_level: j,
            });
        }
        let defaults = &d.defaults;
        self.last_of(List::Defaults(s.def), defaults, n)
            .map(|p| Cand {
                kind: StepKind::SubcktDefault,
                level: j,
                p,
                eval_level: j,
            })
    }

    /// The definition of `n` that wins at level `k`: GLOBAL tries levels
    /// `0..=k`, LOCAL `k..=0`.
    fn find(&self, k: usize, n: Sym) -> Option<Cand<'n>> {
        match self.net.parhier {
            Parhier::Global => (0..=k).find_map(|j| self.at_level(j, n)),
            Parhier::Local => (0..=k).rev().find_map(|j| self.at_level(j, n)),
        }
    }

    /// The value of `n` at level `k`.
    fn resolve(&mut self, k: usize, n: Sym) -> Val {
        self.step(1);
        if let Some(v) = self.memo.get(&(k, n)) {
            return v.clone();
        }
        let net = self.net;
        let name = net.syms.name(n);
        let Some(c) = self.find(k, n) else {
            let v = Val {
                value: Err(undefined(net, name)),
                node: None,
            };
            self.memo.insert((k, n), v.clone());
            return v;
        };
        let key = (c.p as *const ParamDef, c.eval_level);
        if let Some(v) = self.by_def.get(&key) {
            let v = v.clone();
            self.memo.insert((k, n), v.clone());
            return v;
        }
        if let Some(at) = self
            .stack
            .iter()
            .position(|&(p, l, _)| (p, l) == (key.0, key.1))
        {
            let names: Vec<&str> = self.stack[at..]
                .iter()
                .map(|&(_, _, s)| net.syms.name(s))
                .chain([name])
                .collect();
            // not memoized: each name in the cycle gets this reason
            return Val {
                value: Err(format!(
                    "cycle: {} (each value needs the next one)",
                    names.join(" -> ")
                )),
                node: None,
            };
        }
        self.stack.push((key.0, key.1, n));
        let element = c.kind == StepKind::InstanceOverride;
        let text = match c.kind {
            StepKind::InstanceOverride => Text::Override(n),
            StepKind::LocalParam | StepKind::SubcktDefault => Text::Default(n),
            _ => Text::Body,
        };
        let limited = self.limited;
        let (value, mut frame) = self.eval_param(c.p, c.eval_level, element, text);
        self.stack.pop();
        // under LOCAL a definition below the top level wins in every run
        // over a top-level `.param` of an `.alter` block
        let alters = if net.parhier == Parhier::Local && c.level > 0 {
            Vec::new()
        } else {
            self.altered(n)
        };
        for (a, at) in alters {
            frame.caveats.push(format!(
                "`{name}` is changed by .alter #{a} at {at}: this is the first run's value (golden `alter`: the changes of the `.alter` blocks accumulate over the runs)"
            ));
        }
        // a sweep changes the top-level `.param`
        if c.kind == StepKind::GlobalParam {
            frame.caveats.extend(net.swept_caveats(n));
        }
        let step = ChainStep {
            kind: c.kind,
            scope: self.scopes[c.level].path.clone(),
            name: name.to_owned(),
            text: param_text(net, c.p),
            value: value.as_ref().ok().copied(),
            loc: Some(net.loc(c.p.loc)),
        };
        let v = Val {
            value,
            node: Some(self.node(step, frame)),
        };
        if self.limited == limited {
            self.by_def.insert(key, v.clone());
            self.memo.insert((k, n), v.clone());
        }
        v
    }

    /// The `.alter` blocks that define the parameter `n` again (`.alter`
    /// lines are at the top level): the block number and `file:line`.
    fn altered(&mut self, n: Sym) -> Vec<(u32, String)> {
        let net = self.net;
        if net.alters.is_empty() {
            return Vec::new();
        }
        let by_name = self.alter_params.get_or_insert_with(|| {
            let mut m: HashMap<Sym, Vec<(u32, LocIdx)>> = HashMap::new();
            for p in &net.defs[ROOT.idx()].params {
                if let Some(a) = p.alter {
                    m.entry(p.name).or_default().push((a, p.loc));
                }
            }
            m
        });
        by_name
            .get(&n)
            .into_iter()
            .flatten()
            .map(|&(a, loc)| (a, loc_str(net, loc)))
            .collect()
    }

    /// The value of the parameter `p`, evaluated at level `level`.
    /// `element`: an expression of an element line. `text`: see [`Text`].
    fn eval_param(
        &mut self,
        p: &ParamDef,
        level: usize,
        element: bool,
        text: Text,
    ) -> (Result<f64, String>, Frame) {
        if let Some(x) = p.num {
            // as `netlist_expr` does for a number in an expression
            let v = if x.is_finite() {
                Ok(x)
            } else {
                Err(format!(
                    "the number `{}` is out of range",
                    param_text(self.net, p)
                ))
            };
            return (v, Frame::default());
        }
        match self.net.param_text(p) {
            Some(s) => self.eval_text(&s, level, &[], element, text),
            None => (Err("no value".to_owned()), Frame::default()),
        }
    }

    /// The value of the expression `s` at level `level`, with the
    /// arguments `args` of a function bound (lowercase names). `text`: see
    /// [`Text`].
    fn eval_text(
        &mut self,
        s: &str,
        level: usize,
        args: &[(String, f64)],
        element: bool,
        text: Text,
    ) -> (Result<f64, String>, Frame) {
        let fail = |why: String| (Err(why), Frame::default());
        if s.contains(['{', '}']) {
            return fail(BRACES.to_owned());
        }
        match quote_fault(s) {
            Some(QuoteFault::CommentLine | QuoteFault::ParenCommentLine) => {
                return fail(COMMENT_LINE.to_owned());
            }
            Some(QuoteFault::Dollar) => return fail(DOLLAR.to_owned()),
            None => {}
        }
        self.evals += 1;
        self.step(1);
        if self.evals > MAX_EVALS {
            self.limited += 1;
            return fail(TOO_MUCH.to_owned());
        }
        let e = match netlist_expr::parse(s) {
            Ok(e) => e,
            Err(err) => {
                return fail(format!("cannot read the expression `{}`: {err}", quote(s)));
            }
        };
        // a `.param` function hides a built-in of its name (golden
        // `func_shadow_top`, `stat_user_func`)
        if element && let Some(f) = statistical(&e, &mut |n| self.user_func(level, n, text)) {
            return fail(format!(
                "HSPICE rejects `{f}()` directly in an element expression (golden `stat_direct`, `stat_in_mos`: \"cannot be defined in expressions directly\"): it belongs in a `.param`"
            ));
        }
        // the arguments of a call that is neither a built-in nor a `.param`
        // function are not parameters: `v(out)`, `i(v1)` (golden
        // `rcl_output_var`)
        if let Some(name) = self.unknown_call(&e, level, text) {
            return fail(not_visible(&name));
        }
        let h = height(&e);
        if self.stack.len() + self.calls.len() >= MAX_DEPTH || self.height + h > MAX_HEIGHT {
            self.limited += 1;
            return fail(TOO_DEEP.to_owned());
        }
        self.height += h;
        self.frames.push(Frame::default());
        let r = netlist_expr::eval(
            &e,
            &mut Cx {
                r: self,
                level,
                args,
                text,
            },
        );
        let mut frame = self.frames.pop().expect("pushed above");
        self.height -= h;
        match r {
            Ok((v, cs)) => {
                for c in cs {
                    let text = self.caveat_text(&c);
                    frame.caveats.push(text);
                }
                (Ok(v), frame)
            }
            // `1e308 * 1e308`: the evaluator writes each number in full
            Err(EvalError::Domain(msg)) => (Err(short_numbers(&msg)), frame),
            Err(err) => (Err(err.to_string()), frame),
        }
    }

    /// The caveat of an evaluator note.
    fn caveat_text(&mut self, c: &Caveat) -> String {
        match c {
            Caveat::StatisticalNominal(f) => format!(
                "`{f}` gives its nominal value, as HSPICE's `.op` without Monte Carlo does (golden `stat_nominal`)"
            ),
            Caveat::Temper => {
                let t = self.temperature();
                let source = self.net.temps.iter().find(|p| p.alter.is_none()).map_or(
                    "HSPICE's default: the first run has no `.temp` line".to_owned(),
                    |p| format!("the first `.temp` value, at {}", loc_str(self.net, p.loc)),
                );
                match t.value {
                    Ok(v) => {
                        format!("uses `temper` = {v} ({source}; golden `temper`, `temper_default`)")
                    }
                    Err(_) => "uses `temper`, whose value is not resolved".to_owned(),
                }
            }
        }
    }

    /// The last first-run `.param` function `sym` of definition `def`
    /// (golden `func_redef`): a linear scan of a short list, else an index
    /// of the list built once in this call.
    fn func_of(&mut self, def: DefId, sym: Sym) -> Option<&'n FuncDef> {
        let funcs: &'n [FuncDef] = &self.net.defs[def.idx()].funcs;
        if funcs.len() < SHORT_FUNCS {
            self.step(funcs.len());
            return funcs.iter().rfind(|f| f.name == sym && f.alter.is_none());
        }
        self.step(1);
        let i = self.func_entry(def).get(&sym)?.last?;
        Some(&funcs[i as usize])
    }

    /// The `.param` functions of `def` by name (see [`Resolver::func_index`]).
    fn func_entry(&mut self, def: DefId) -> &HashMap<Sym, FuncEntry> {
        let funcs: &'n [FuncDef] = &self.net.defs[def.idx()].funcs;
        let work = &self.work;
        self.func_index.entry(def).or_insert_with(|| {
            work.set(work.get().saturating_add(funcs.len() as u64));
            let mut index: HashMap<Sym, FuncEntry> = HashMap::new();
            for (i, f) in funcs.iter().enumerate() {
                let i = u32::try_from(i).expect("fewer than 2^32 functions");
                let entry = index.entry(f.name).or_default();
                match f.alter {
                    None => entry.last = Some(i),
                    Some(_) => entry.alters.push(i),
                }
            }
            index
        })
    }

    /// The `.alter` blocks that define the function `sym` of `def` again.
    fn func_alters(&mut self, def: DefId, sym: Sym) -> Vec<&'n FuncDef> {
        let funcs: &'n [FuncDef] = &self.net.defs[def.idx()].funcs;
        if self.net.alters.is_empty() {
            return Vec::new();
        }
        if funcs.len() < SHORT_FUNCS {
            self.step(funcs.len());
            return funcs
                .iter()
                .filter(|g| g.name == sym && g.alter.is_some())
                .collect();
        }
        self.step(1);
        let alters = self
            .func_entry(def)
            .get(&sym)
            .map(|e| e.alters.clone())
            .unwrap_or_default();
        alters.into_iter().map(|i| &funcs[i as usize]).collect()
    }

    /// The note `text`: its id in [`Resolver::notes`].
    fn note(&mut self, text: String) -> u32 {
        let id = u32::try_from(self.notes.len()).expect("fewer than 2^32 notes");
        self.notes.push(text);
        id
    }

    /// What a call of the function `sym` at level `level`, in a text of
    /// kind `text`, uses, and a caveat on the choice (a note). Memoized for
    /// the call by (level, name, text).
    ///
    /// GLOBAL: the top level first, then the instances down the path, as
    /// for a parameter (golden `func_scope`: a subckt's own `f` loses to the
    /// top-level `f`; `func_two_levels_global`: the outer of two instances
    /// wins). LOCAL: the nearest definition up the instance path (the level
    /// of the call first), then the top level (golden `ovr_self_combo`,
    /// `func_three_local`, `func_levels_local`, `func_first_inst_local`). A
    /// function defined only in an instance above is visible below it in
    /// both modes. In one scope the last definition wins (golden
    /// `func_redef`).
    ///
    /// The creation-order rule: under LOCAL, HSPICE keeps the top-level `f`
    /// for a text once an instance that it creates before this one
    /// evaluated the same text and found no `f` above it (golden
    /// `func_scope_local`, where the top-level `xt` of `leaf` comes before
    /// `x1.xl`, `func_sticky_local`, `func_sticky_more_local`,
    /// `func_sticky_ovr_local`; see [`Resolver::sticky`]).
    ///
    /// A name of a built-in function (`max`, `sgn`, ...): a visible
    /// `.param` function of that name hides the built-in, and the built-in
    /// is used where none is visible (golden `func_shadow_top`,
    /// `func_shadow_sub`, `func_shadow_scope`). With no top-level `.param`
    /// function of the name, the instance that HSPICE creates first among
    /// those that evaluate the text decides for the later ones, in both
    /// modes: when it finds no `.param` function above it, the later ones
    /// use the built-in too (golden `func_shadow_scope`: x2.xk.rk = 1k);
    /// when it finds one, a later one that finds none makes HSPICE reject
    /// the deck (golden `func_shadow_more`, `func_shadow_more_local`).
    ///
    /// A call in a function body ([`Text::Func`]) takes the nearest
    /// definition (the GLOBAL order under GLOBAL), or the built-in, with no
    /// creation-order rule (golden `sticky_lazy_local`: x5.xc.rh = 3k,
    /// x7.xc.rk = 3k, where the first instance of the subckt that defines
    /// the function found the top-level `f`).
    /// A `.param` function `name` is visible at `level` for `text`.
    fn user_func(&mut self, level: usize, name: &str, text: Text) -> bool {
        self.net
            .syms
            .get(name)
            .is_some_and(|s| matches!(self.find_func(level, s, text).0, Found::User(..)))
    }

    /// The first call in `e` of a name that is neither a built-in nor a
    /// `.param` function visible at `level`.
    fn unknown_call(&mut self, e: &Expr, level: usize, text: Text) -> Option<String> {
        match e {
            Expr::Num(_) | Expr::Ident(_) => None,
            Expr::Call { name, args } => {
                if !netlist_expr::is_builtin(name) && !self.user_func(level, name, text) {
                    return Some(name.clone());
                }
                args.iter().find_map(|a| self.unknown_call(a, level, text))
            }
            Expr::Unary { e, .. } => self.unknown_call(e, level, text),
            Expr::Binary { l, r, .. } => self
                .unknown_call(l, level, text)
                .or_else(|| self.unknown_call(r, level, text)),
            Expr::Ternary { c, t, f } => self
                .unknown_call(c, level, text)
                .or_else(|| self.unknown_call(t, level, text))
                .or_else(|| self.unknown_call(f, level, text)),
        }
    }

    fn find_func(&mut self, level: usize, sym: Sym, text: Text) -> (Found<'n>, Option<u32>) {
        self.step(1);
        if let Some(&found) = self.found.get(&(level, sym, text)) {
            return found;
        }
        let found = self.find_func_here(level, sym, text);
        self.found.insert((level, sym, text), found);
        found
    }

    /// [`Resolver::find_func`], not memoized.
    fn find_func_here(&mut self, level: usize, sym: Sym, text: Text) -> (Found<'n>, Option<u32>) {
        let net = self.net;
        let name = net.syms.name(sym);
        let builtin = netlist_expr::is_builtin(name);
        let near = match net.parhier {
            Parhier::Global => {
                (0..=level).find_map(|j| self.func_of(self.scopes[j].def, sym).map(|f| (j, f)))
            }
            Parhier::Local => (0..=level)
                .rev()
                .find_map(|j| self.func_of(self.scopes[j].def, sym).map(|f| (j, f))),
        };
        let top = self.func_of(self.scopes[0].def, sym);
        if text == Text::Func || !builtin || top.is_some() {
            let Some((j, f)) = near else {
                if builtin {
                    return (Found::Builtin, None);
                }
                let why = self.note(not_visible(name));
                return (Found::Nothing(why), None);
            };
            if text == Text::Func || net.parhier == Parhier::Global || j == 0 || j == level {
                return (Found::User(j, f), None);
            }
            let Some(top) = top else {
                return (Found::User(j, f), None);
            };
            let golden = "golden `func_scope_local`, `func_sticky_local`, `func_sticky_ovr_local`";
            let near_path = self.scopes[j].path.clone();
            let what = self.what(level, text);
            let set = self.definers(sym);
            return match self.sticky(level, set, text) {
                Sticky::No => (Found::User(j, f), None),
                Sticky::Top(first) => {
                    let why = self.note(format!(
                        "under parhier=local the nearest `{name}` is the one of `{near_path}`, but `{}`, an instance {what} that HSPICE creates before this one (breadth-first), evaluates the same text, finds no `{name}` above it and uses the top-level `{name}`; HSPICE then keeps the top-level `{name}` for this text in the later instances ({golden})",
                        first.path
                    ));
                    (Found::User(0, top), Some(why))
                }
                Sticky::Unknown(why) => {
                    let why = self.note(format!(
                        "under parhier=local HSPICE uses the top-level `{name}` instead of the one of `{near_path}` when an instance {what} that it creates before this one evaluates the same text and finds no `{name}` above it ({golden}); this was not checked: {why}"
                    ));
                    (Found::User(j, f), Some(why))
                }
            };
        }
        self.builtin_name(level, sym, text, near)
    }

    /// [`Resolver::find_func`] for the name `sym` of a built-in function
    /// with no top-level `.param` function, in a text other than a function
    /// body, where `near` is the `.param` function that the instance of
    /// `level` finds (with its level).
    fn builtin_name(
        &mut self,
        level: usize,
        sym: Sym,
        text: Text,
        near: Option<(usize, &'n FuncDef)>,
    ) -> (Found<'n>, Option<u32>) {
        let name = self.net.syms.name(sym);
        let set = self.definers(sym);
        if set == 0 {
            // no `.param` function of the name anywhere
            return (Found::Builtin, None);
        }
        if let Some((j, f)) = near
            && j == level
        {
            // every instance of the definition of the level finds it
            return (Found::User(j, f), None);
        }
        let golden = "golden `func_shadow_scope`, `func_shadow_more`, `func_shadow_more_local`";
        let what = self.what(level, text);
        // the first instance created before this one that evaluates the
        // text, and the first one with no `.param` function above it
        let any = self.sticky(level, 0, text);
        let none = match (&any, near) {
            (Sticky::No, None) => return (Found::Builtin, None),
            (Sticky::Unknown(why), _) => {
                let why = *why;
                let found = near.map_or(Found::Builtin, |(j, f)| Found::User(j, f));
                let used = if near.is_some() {
                    format!("the `.param` function `{name}`")
                } else {
                    format!("the built-in `{name}`")
                };
                let note = self.note(format!(
                    "`{name}` is also a built-in function: HSPICE keeps the kind of function (`.param` or built-in) that the instance {what} that it creates first uses for this text ({golden}); this answer uses {used}, and the order was not checked: {why}"
                ));
                return (found, Some(note));
            }
            _ => self.sticky(level, set, text),
        };
        let first_any = match &any {
            Sticky::Top(f) => Some(Rc::clone(f)),
            _ => None,
        };
        let first_none = match &none {
            Sticky::Top(f) => Some(Rc::clone(f)),
            Sticky::Unknown(why) => {
                let why = *why;
                let found = near.map_or(Found::Builtin, |(j, f)| Found::User(j, f));
                let note = self.note(format!(
                    "`{name}` is also a built-in function: HSPICE keeps the kind of function (`.param` or built-in) that the instance {what} that it creates first uses for this text ({golden}); the order was not checked: {why}"
                ));
                return (found, Some(note));
            }
            Sticky::No => None,
        };
        // the instance created first used the built-in
        let first_builtin = match (&first_any, &first_none) {
            (Some(a), Some(n)) => a.lines == n.lines,
            _ => false,
        };
        match (near, first_any) {
            (Some((j, f)), _) if first_none.is_none() => {
                // the instances up to this one use a `.param` function: a
                // later one that finds none makes HSPICE reject the deck
                // (golden `func_shadow_more`: x8.xk.rk2 after x7.xk.rk2)
                let note = match self.sticky_later(level, set, text) {
                    Sticky::No => None,
                    Sticky::Top(later) => Some(self.note(format!(
                        "HSPICE rejects this deck: the instances {what} that HSPICE creates up to this one use a `.param` function `{name}` for this text, and HSPICE keeps that kind of function for the later instances, but `{}`, created after this one, finds no `.param` function `{name}` above it ({golden}: \"Undefined parameter or function definition\" for the later instance); this answer is for this instance, which uses the `.param` function",
                        later.path
                    ))),
                    Sticky::Unknown(why) => Some(self.note(format!(
                        "`{name}` is also a built-in function: HSPICE rejects the deck when an instance {what} that it creates after this one finds no `.param` function `{name}` above it ({golden}); this was not checked: {why}"
                    ))),
                };
                (Found::User(j, f), note)
            }
            (Some((j, _)), Some(first)) if first_builtin => {
                let near_path = self.scopes[j].path.clone();
                let note = self.note(format!(
                    "`{name}` is a built-in function, and the `.param` function `{name}` of `{near_path}` is visible here, but `{}`, an instance {what} that HSPICE creates first (breadth-first), evaluates the same text, finds no `.param` function `{name}` above it and uses the built-in; HSPICE then keeps the built-in for this text in the later instances (golden `func_shadow_scope`: x2.xk.rk)",
                    first.path
                ));
                (Found::Builtin, Some(note))
            }
            (Some(_), first) => {
                let first = first.expect("an instance with none comes after the first one");
                let none = first_none.expect("checked above");
                let why = self.note(format!(
                    "HSPICE rejects this deck: `{}`, an instance {what} that HSPICE creates first (breadth-first), uses a `.param` function `{name}` for this text, and HSPICE keeps that kind of function for the later instances, but `{}`, created after it and before this one, finds no `.param` function `{name}` above it ({golden}: \"Undefined parameter or function definition\")",
                    first.path, none.path
                ));
                (Found::Nothing(why), None)
            }
            (None, Some(_)) if first_builtin => (Found::Builtin, None),
            (None, first) => {
                let first = first.expect("checked above");
                let why = self.note(format!(
                    "no `.param` function `{name}` is visible here, and `{}`, an instance {what} that HSPICE creates before this one (breadth-first), uses a `.param` function `{name}` for this text: HSPICE keeps that kind of function for the later instances and rejects this deck, though `{name}` is also a built-in function ({golden}: \"Undefined parameter or function definition\")",
                    first.path
                ));
                (Found::Nothing(why), None)
            }
        }
    }

    /// The instances that evaluate a text of kind `text` at level `level`,
    /// for a caveat.
    fn what(&self, level: usize, text: Text) -> String {
        let net = self.net;
        let def = net.def_display(self.scopes[level].def);
        match (text, self.scopes[level].x) {
            (Text::Override(_), Some((holder, i))) => format!(
                "made by the same `X` line (`{}` in `{}`)",
                net.syms.name(net.defs[holder.idx()].elements[i].name),
                net.def_display(holder)
            ),
            (Text::Default(n), _) => format!(
                "of `{def}` whose `X` line does not override `{}`",
                net.syms.name(n)
            ),
            _ => format!("of `{def}`"),
        }
    }

    /// Whether an instance that HSPICE creates before the instance of level
    /// `level` evaluates the same text (of kind `text`) and finds no
    /// definition of the function on its instance path (none of the
    /// definitions of set `set`, see [`Definers`]; any instance for the
    /// empty set 0), so that it uses the top-level one (or the built-in),
    /// with that instance. The instances that evaluate the text: for
    /// [`Text::Body`] each instance of the definition of the level, for
    /// [`Text::Default`] each one whose `X` line does not override the
    /// name, for [`Text::Override`] each instance that the `X` line of the
    /// level makes (golden `func_sticky_ovr_local`: a `.subckt` default
    /// that the `X` line of the first instance overrides does not count,
    /// and two `X` lines with the same override text count separately;
    /// golden `sticky_lazy_local`: x1.xl.rp, a body `.param` that the `X`
    /// line of the first instance overrides does not count either, and
    /// x9.xl.ru, a body `.param` that no element of the earlier instance
    /// uses still counts). HSPICE creates the instances breadth-first:
    /// level by level, in a level by the order of the parents, in a parent
    /// by `X`-line order (golden `hier_dotted_collision*`), so an instance
    /// comes first when it is less deep, or as deep with a smaller list of
    /// `X`-line indices. Any such earlier instance counts, not only the
    /// first one (golden `func_sticky_ovr_local`: cf6, then w6, then cf6);
    /// the walk gives the first one. Memoized for the call by (level, the
    /// instances that evaluate the text, set).
    fn sticky(&self, level: usize, set: u32, text: Text) -> Sticky {
        let unit = match text {
            Text::Override(_) => Unit::Override,
            Text::Default(n) => Unit::Default(n),
            Text::Body | Text::Func => Unit::Body,
        };
        let key = (level, unit, set, false);
        self.step(1);
        if let Some(s) = self.sticky.borrow().get(&key) {
            return s.clone();
        }
        let defs = self.definer_set(set);
        let s = match text {
            // the instances that evaluate a default are some of those of
            // `Text::Body`, in the same order: the first of those is the
            // first of these when its `X` line does not override the name
            Text::Default(n) => match self.sticky(level, set, Text::Body) {
                Sticky::Top(first) if self.overrides(first.line().0, first.line().1, n) => {
                    self.sticky_walk(level, text, &defs, false)
                }
                s => s,
            },
            _ => self.sticky_walk(level, text, &defs, false),
        };
        self.sticky.borrow_mut().insert(key, s.clone());
        s
    }

    /// As [`Resolver::sticky`], but for all the instances that evaluate
    /// the text, in creation order: the first one that finds no definition
    /// of set `set` above it. Asked when no instance before the one of
    /// level `level` is such an instance, so the one it gives comes after
    /// it.
    fn sticky_later(&self, level: usize, set: u32, text: Text) -> Sticky {
        let unit = match text {
            Text::Override(_) => Unit::Override,
            Text::Default(n) => Unit::Default(n),
            Text::Body | Text::Func => Unit::Body,
        };
        let key = (level, unit, set, true);
        self.step(1);
        if let Some(s) = self.sticky.borrow().get(&key) {
            return s.clone();
        }
        let s = self.sticky_walk(level, text, &self.definer_set(set), true);
        self.sticky.borrow_mut().insert(key, s.clone());
        s
    }

    /// Whether `X` line `i` of `holder` overrides the name `n`.
    fn overrides(&self, holder: DefId, i: ElemIdx, n: Sym) -> bool {
        let h = &self.net.defs[holder.idx()];
        let list = h.element_params(&h.elements[i]);
        self.last_of(List::Overrides(holder, i), list, n).is_some()
    }

    /// The id of the set of definitions of the function `sym` (see
    /// [`Definers`]), built for all names at the first use in this call.
    fn definers(&self, sym: Sym) -> u32 {
        self.step(1);
        let mut guard = self.definers.borrow_mut();
        let d = guard.get_or_insert_with(|| {
            let mut m: HashMap<Sym, Vec<DefId>> = HashMap::new();
            for (i, d) in self.net.defs.iter().enumerate().skip(ROOT.idx() + 1) {
                let id = DefId(u32::try_from(i).expect("fewer than 2^32 definitions"));
                self.step(d.funcs.len());
                for f in d.funcs.iter().filter(|f| f.alter.is_none()) {
                    let v = m.entry(f.name).or_default();
                    if v.last() != Some(&id) {
                        v.push(id);
                    }
                }
            }
            let empty: Rc<[DefId]> = Rc::from(Vec::new());
            let mut ids: HashMap<Rc<[DefId]>, u32> = HashMap::from([(Rc::clone(&empty), 0)]);
            let mut sets = vec![empty];
            let mut by_name = HashMap::with_capacity(m.len());
            for (name, defs) in m {
                self.step(defs.len());
                let defs: Rc<[DefId]> = Rc::from(defs);
                let id = *ids.entry(Rc::clone(&defs)).or_insert_with(|| {
                    sets.push(defs);
                    u32::try_from(sets.len() - 1).expect("fewer than 2^32 sets")
                });
                by_name.insert(name, id);
            }
            Definers { by_name, sets }
        });
        d.by_name.get(&sym).copied().unwrap_or(0)
    }

    /// The definitions of set `set` (see [`Definers`]).
    fn definer_set(&self, set: u32) -> Rc<[DefId]> {
        match &*self.definers.borrow() {
            Some(d) => Rc::clone(&d.sets[set as usize]),
            None => Rc::from(Vec::new()),
        }
    }

    /// A breadth-first walk of the hierarchy graph from the root, through
    /// the nodes whose definitions are not in `defs` (the definitions of
    /// the function), each node once (the first instance of a node comes
    /// before the other ones, and so do the instances below it), up to the
    /// instance of `level`, or with `later` to the end. Each `X` line it
    /// reads counts against [`MAX_STICKY`] for the call.
    fn sticky_walk(&self, level: usize, text: Text, defs: &[DefId], later: bool) -> Sticky {
        let net = self.net;
        let hier = &net.hier;
        let defines = |d: DefId| defs.binary_search(&d).is_ok();
        let target = self.scopes[level].def;
        let above = &self.scopes[1..=level];
        let Some(line) = self.scopes[level].x else {
            return Sticky::No;
        };
        // a scope above every instance (`top=`) that defines the function
        // hides the top-level one from all of them
        if defines(target) || above.iter().any(|s| s.x.is_none() && defines(s.def)) {
            return Sticky::No;
        }
        if net.root == Root::None {
            return Sticky::No;
        }
        if hier.too_large || hier.nodes.is_empty() {
            return Sticky::Unknown(
                "the hierarchy was not analysed (diagnostic `hierarchy_too_large`)",
            );
        }
        // the `X`-line indices of the instance of `level` from the root
        let seq: Vec<u32> = above
            .iter()
            .filter_map(|s| s.x.map(|(_, i)| i as u32))
            .collect();
        let count = |n: usize| {
            self.step(n);
            let v = self.sticky_visits.get().saturating_add(n);
            self.sticky_visits.set(v);
            v > MAX_STICKY
        };
        // the instance that `X` line `i` of `holder` makes, of definition
        // `d`, evaluates the text
        let evaluates = |holder: DefId, i: ElemIdx, d: DefId| match text {
            Text::Body | Text::Func => Ok(d == target),
            Text::Override(_) => Ok((holder, i) == line),
            Text::Default(n) => {
                if d != target {
                    return Ok(false);
                }
                let h = &net.defs[holder.idx()];
                let len = h.element_params(&h.elements[i]).len();
                if count(len.min(BIG_SCOPE)) {
                    return Err(());
                }
                Ok(!self.overrides(holder, i, n))
            }
        };
        // (graph node, previous step, `X` line, its `X`-line indices
        // against those of the instance of `level`); the root first
        let mut steps: Vec<(u32, u32, u32, Ordering)> = vec![(0, u32::MAX, 0, Ordering::Equal)];
        let mut level_steps: Vec<u32> = vec![0];
        let mut guard = self.reached.borrow_mut();
        let (walk, reached) = &mut *guard;
        *walk += 1;
        let walk = *walk;
        if reached.len() != hier.nodes.len() {
            if count(hier.nodes.len() / 64) {
                return Sticky::Unknown(TOO_LARGE);
            }
            *reached = vec![0; hier.nodes.len()];
        }
        reached[0] = walk;
        let mut depth = 0;
        while !level_steps.is_empty() && (later || depth < seq.len()) {
            depth += 1;
            let last = !later && depth == seq.len();
            let mut next = Vec::new();
            for &si in &level_steps {
                let (node, _, _, order) = steps[si as usize];
                let n = &hier.nodes[node as usize];
                for e in &hier.edges[n.edges.range()] {
                    let order = if later {
                        Ordering::Equal
                    } else {
                        order.then(e.elem.cmp(&seq[depth - 1]))
                    };
                    // as deep as the instance of `level`: the instances
                    // from here on come after it
                    if last && order != Ordering::Less {
                        return Sticky::No;
                    }
                    if count(1) {
                        return Sticky::Unknown(TOO_LARGE);
                    }
                    // `target` NONE: no visible definition
                    let Some(t) = hier.nodes.get(e.target as usize) else {
                        continue;
                    };
                    if defines(t.def) {
                        continue;
                    }
                    match evaluates(n.def, e.elem as ElemIdx, t.def) {
                        Err(()) => return Sticky::Unknown(TOO_LARGE),
                        Ok(true) => {
                            let mut lines = vec![(n.def, e.elem as ElemIdx)];
                            let mut at = si;
                            while at != 0 {
                                let (_, prev, elem, _) = steps[at as usize];
                                let holder = hier.nodes[steps[prev as usize].0 as usize].def;
                                lines.push((holder, elem as ElemIdx));
                                at = prev;
                            }
                            lines.reverse();
                            let names: Vec<String> = lines
                                .iter()
                                .map(|&(holder, elem)| {
                                    let e = &net.defs[holder.idx()].elements[elem];
                                    net.syms.name(e.name).to_ascii_lowercase()
                                })
                                .collect();
                            return Sticky::Top(Rc::new(First {
                                path: names.join("."),
                                lines,
                            }));
                        }
                        Ok(false) => {}
                    }
                    if !last && reached[e.target as usize] != walk {
                        reached[e.target as usize] = walk;
                        next.push(u32::try_from(steps.len()).expect("fewer than 2^32 steps"));
                        steps.push((e.target, si, e.elem, order));
                    }
                }
            }
            level_steps = next;
        }
        Sticky::No
    }

    /// Calls the function `name` (visible from level `level` in a text of
    /// kind `text`, see [`Resolver::find_func`]) with `args`. `None`: the
    /// built-in function of the name. The names in the body of a `.param`
    /// function, other than its arguments, are resolved where it is
    /// defined, not where it is called, and so are the functions it calls
    /// (golden `func_free_local`, `func_nested_local`, `func_global_more`),
    /// with no creation-order rule ([`Text::Func`]). An argument hides a
    /// parameter of the same name (golden `sticky_lazy_local`: r11, x10.ry,
    /// x12.rz).
    fn call(
        &mut self,
        level: usize,
        name: &str,
        args: &[f64],
        text: Text,
    ) -> Option<Result<(f64, Vec<Caveat>), EvalError>> {
        let net = self.net;
        let Some(sym) = net.syms.get(name) else {
            return (!netlist_expr::is_builtin(name))
                .then(|| Err(EvalError::Unresolved(not_visible(name))));
        };
        let (found, note) = self.find_func(level, sym, text);
        if let Some(id) = note {
            let top = self
                .frames
                .last_mut()
                .expect("a call is inside an evaluation");
            if top.notes.insert(id) {
                top.caveats.push(self.notes[id as usize].clone());
            }
        }
        let (j, f) = match found {
            Found::User(j, f) => (j, f),
            Found::Builtin => return None,
            Found::Nothing(why) => {
                return Some(Err(EvalError::Unresolved(self.notes[why as usize].clone())));
            }
        };
        let shown = net.syms.name(f.name);
        if f.args.len() != args.len() {
            return Some(Err(EvalError::BadCall(format!(
                "`{shown}` takes {} arguments, not {}",
                f.args.len(),
                args.len()
            ))));
        }
        let key = (
            j,
            sym,
            args.iter().map(|a| a.to_bits()).collect::<Vec<u64>>(),
        );
        self.step(1 + args.len());
        let v = match self.by_call.get(&key) {
            Some(v) => v.clone(),
            None if self.calls.contains(&(j, sym)) => {
                return Some(Err(EvalError::Unresolved(format!(
                    "the function `{shown}` calls itself"
                ))));
            }
            None => {
                let arg_names: Vec<String> = f
                    .args
                    .iter()
                    .map(|&a| net.syms.name(a).to_ascii_lowercase())
                    .collect();
                let bound: Vec<(String, f64)> = arg_names
                    .iter()
                    .cloned()
                    .zip(args.iter().copied())
                    .collect();
                let body = net.span_text(f.body).into_owned();
                self.calls.push((j, sym));
                let limited = self.limited;
                let (value, mut frame) = self.eval_text(&body, j, &bound, false, Text::Func);
                self.calls.pop();
                for g in self.func_alters(self.scopes[j].def, sym) {
                    if let Some(a) = g.alter {
                        frame.caveats.push(format!(
                            "the function `{shown}` is defined again by .alter #{a} at {}: this is the first run's value",
                            loc_str(net, g.loc)
                        ));
                    }
                }
                let step = ChainStep {
                    kind: StepKind::ParamFunction,
                    scope: self.scopes[j].path.clone(),
                    name: shown.to_owned(),
                    text: format!("{shown}({}) = {body}", arg_names.join(",")),
                    value: value.as_ref().ok().copied(),
                    loc: Some(net.loc(f.loc)),
                };
                let v = Val {
                    value,
                    node: Some(self.node(step, frame)),
                };
                if self.limited == limited {
                    self.by_call.insert(key, v.clone());
                }
                v
            }
        };
        let top = self
            .frames
            .last_mut()
            .expect("a call is inside an evaluation");
        top.deps.extend(v.node);
        Some(
            v.value
                .map(|x| (x, Vec::new()))
                .map_err(EvalError::Unresolved),
        )
    }

    /// The circuit temperature: the first `.temp` value of the first run,
    /// else 25 (golden `temper`, `temper_default`).
    fn temperature(&mut self) -> Val {
        if let Some(t) = &self.temp {
            return t.clone();
        }
        let net = self.net;
        // a `.temp` value that uses `temper` gets this
        self.temp = Some(Val {
            value: Err("the `.temp` value uses `temper`: no value".to_owned()),
            node: None,
        });
        let temps: Vec<&ParamDef> = net.temps.iter().filter(|p| p.alter.is_none()).collect();
        let builtin = |text: String, value: Option<f64>, loc| ChainStep {
            kind: StepKind::Builtin,
            scope: String::new(),
            name: "temper".to_owned(),
            text,
            value,
            loc,
        };
        // a `.temp` line of an `.alter` block sets the temperature of the
        // later runs; a `.dc temp` sweeps it
        let mut altered = net.temp_alter_caveats();
        for t in ["temp", "temper"] {
            if let Some(sym) = net.syms.get(t) {
                altered.extend(net.swept_caveats(sym));
            }
        }
        let v = match temps.first() {
            None => {
                let step = builtin(
                    "25 (no `.temp` line in the first run: HSPICE's default)".to_owned(),
                    Some(25.0),
                    None,
                );
                let frame = Frame {
                    caveats: altered,
                    ..Frame::default()
                };
                Val {
                    value: Ok(25.0),
                    node: Some(self.node(step, frame)),
                }
            }
            Some(&p) => {
                let (value, mut frame) = self.eval_param(p, 0, false, Text::Body);
                if temps.len() > 1 || more_on_line(net, p) {
                    frame.caveats.push(format!(
                        "the deck sets more than one temperature (`.temp` at {}): HSPICE runs each one; this value uses the first",
                        loc_str(net, p.loc)
                    ));
                }
                frame.caveats.extend(altered);
                let step = builtin(
                    param_text(net, p),
                    value.as_ref().ok().copied(),
                    Some(net.loc(p.loc)),
                );
                Val {
                    value,
                    node: Some(self.node(step, frame)),
                }
            }
        };
        self.temp = Some(v.clone());
        v
    }

    /// TNOM: the last `.option tnom` of the first run, else 25.
    fn tnom(&mut self) -> Result<f64, String> {
        let net = self.net;
        let tnom = net.syms.get("tnom");
        match net
            .options
            .iter()
            .rfind(|p| Some(p.name) == tnom && p.alter.is_none())
        {
            Some(p) => self.eval_param(p, 0, false, Text::Body).0,
            None => Ok(25.0),
        }
    }

    /// The value of the word `w` of a source function at level `k`; the
    /// nodes it uses go to the current frame.
    fn word(&mut self, w: Option<&&str>, k: usize) -> Result<f64, String> {
        let Some(w) = w else {
            return Err("the function has too few arguments".to_owned());
        };
        let (v, frame) = self.eval_text(w, k, &[], true, Text::Body);
        let top = self.frames.last_mut().expect("inside source_t0");
        top.deps.extend(frame.deps);
        top.caveats.extend(frame.caveats);
        v
    }

    /// The value at t=0 of the function `f` of a source at level `k`
    /// (golden `src_fn_nodc`, `src_sin_t0`, `pwl_*`); `td` is a `TD=`
    /// parameter of the line.
    fn source_t0(&mut self, f: &ParamDef, td: Option<&ParamDef>, k: usize) -> Result<f64, String> {
        let net = self.net;
        let fname = net.syms.name(f.name);
        let args = net.param_text(f).unwrap_or_default();
        let words = arg_words(&args);
        match fname.to_ascii_lowercase().as_str() {
            // the first argument: the value before the waveform starts
            "pulse" | "exp" | "sffm" => self.word(words.first(), k),
            "sin" => {
                // vo + va*sin(phase): the SIN formula at t=0, also before
                // the delay (golden `src_sin_t0`)
                let vo = self.word(words.first(), k)?;
                if words.get(5).is_none() {
                    return Ok(vo);
                }
                let phase = self.word(words.get(5), k)?;
                if phase == 0.0 {
                    return Ok(vo);
                }
                let va = self.word(words.get(1), k)?;
                Ok(vo + va * phase.to_radians().sin())
            }
            "pwl" => {
                let values = pwl_values(&args);
                // `TD=` inside the parentheses or on the line
                let td_word = words
                    .iter()
                    .find_map(|w| {
                        w.split_once('=')
                            .filter(|(key, _)| key.trim().eq_ignore_ascii_case("td"))
                    })
                    .map(|(_, v)| v);
                let delay = match (td_word, td) {
                    (Some(w), _) => self.word(Some(&w), k)?,
                    (None, Some(p)) => {
                        let text = param_text(net, p);
                        self.word(Some(&text.as_str()), k)?
                    }
                    (None, None) => 0.0,
                };
                self.pwl_at(&values, -delay, k)
            }
            // sa*(oc + sin(2*pi*fm*(t-td)))*sin(2*pi*fc*(t-td)): 0 at t=0
            // with no delay (golden `src_fn_nodc`), and 0 before a delay
            // (golden `src_delay_scale`)
            "am" => Ok(0.0),
            _ => Err(format!(
                "the value of a {fname} source at t=0 is not resolved"
            )),
        }
    }

    /// The value of the PWL `values` (time, value, …) at time `t`: the
    /// first value before the first time point, the last after the last,
    /// linear in between.
    fn pwl_at(&mut self, values: &[&str], t: f64, k: usize) -> Result<f64, String> {
        let pairs = values.len() / 2;
        if pairs == 0 {
            return Err("the PWL has no point".to_owned());
        }
        let mut t0 = self.word(values.first(), k)?;
        let mut v0 = self.word(values.get(1), k)?;
        if t <= t0 {
            return Ok(v0);
        }
        for i in 1..pairs {
            let t1 = self.word(values.get(2 * i), k)?;
            let v1 = self.word(values.get(2 * i + 1), k)?;
            if t <= t1 {
                return Ok(if t1 == t0 {
                    v1
                } else {
                    v0 + (v1 - v0) * (t - t0) / (t1 - t0)
                });
            }
            (t0, v0) = (t1, v1);
        }
        Ok(v0)
    }

    /// The answer for the element parameter `item`, at level `k` (the
    /// scope of the element; an `X`-line override at `k + 1`, see
    /// [`Text`]), for the element at `path`.
    fn resolve_item(&mut self, item: Item<'n>, k: usize, path: &str) -> Resolved {
        let net = self.net;
        let step = |text: String, value: &Result<f64, String>, loc: Option<LocIdx>| ChainStep {
            kind: StepKind::ElementParam,
            scope: path.to_owned(),
            name: item.name.clone(),
            text,
            value: value.as_ref().ok().copied(),
            loc: loc.map(|l| net.loc(l)),
        };
        let (value, root, mut frame) = match &item.src {
            Src::Param(p) => {
                let (level, text) = match (item.child, item.sym) {
                    (true, Some(s)) => (k + 1, Text::Override(s)),
                    _ => (k, Text::Body),
                };
                let (value, frame) = self.eval_param(p, level, true, text);
                let root = step(param_text(net, p), &value, Some(p.loc));
                (value, Some(root), frame)
            }
            Src::Word(text, loc) => {
                let (value, frame) = self.eval_text(text, k, &[], true, Text::Body);
                let root = step(text.clone(), &value, Some(*loc));
                (value, Some(root), frame)
            }
            Src::Fail(why, p) => {
                let value = Err(why.clone());
                let root = p.map(|p| step(param_text(net, p), &value, Some(p.loc)));
                (value, root, Frame::default())
            }
            Src::Builtin(v, text) => {
                let value = Ok(*v);
                let root = ChainStep {
                    kind: StepKind::Builtin,
                    ..step((*text).to_owned(), &value, None)
                };
                (value, Some(root), Frame::default())
            }
            Src::SourceT0(func, td) => {
                self.frames.push(Frame::default());
                let value = self.source_t0(func, *td, k);
                let mut frame = self.frames.pop().expect("pushed above");
                let fname = net.syms.name(func.name);
                frame.caveats.push(format!(
                    "the line has no DC value: HSPICE's `.op` uses the value of its {fname} at t=0 (golden `src_fn_nodc`, `src_sin_t0`, `src_delay_scale`)"
                ));
                let root = ChainStep {
                    name: fname.to_owned(),
                    ..step(param_text(net, func), &value, Some(func.loc))
                };
                (value, Some(root), frame)
            }
        };
        frame.caveats.extend(item.caveats);
        self.answer(item.name, value, root, &frame)
    }
}

/// The evaluator's view of the names at one level.
struct Cx<'a, 'n> {
    r: &'a mut Resolver<'n>,
    level: usize,
    args: &'a [(String, f64)],
    text: Text,
}

impl Env for Cx<'_, '_> {
    fn var(&mut self, name: &str) -> Result<(f64, Vec<Caveat>), EvalError> {
        if let Some(&(_, v)) = self.args.iter().find(|(a, _)| a == name) {
            return Ok((v, Vec::new()));
        }
        let v = if name == "temper" {
            self.r.temperature()
        } else {
            match self.r.net.syms.get(name) {
                Some(sym) => match self.text {
                    // `x1 a b cell w='w*3'`: `w` from the parent's scope
                    // (golden `ovr_self_local`); with none there, HSPICE
                    // rejects the deck (golden `ovr_self_global`)
                    Text::Override(own) if own == sym => {
                        let up = self.level - 1;
                        if self.r.find(up, sym).is_none() {
                            return Err(EvalError::Unresolved(format!(
                                "the `X`-line override of `{name}` uses `{name}` itself, which HSPICE looks up above the new instance, and no scope above it defines `{name}`: HSPICE rejects the deck (golden `ovr_self_global`: \"cannot redefine parameter to itself\")"
                            )));
                        }
                        self.r.resolve(up, sym)
                    }
                    _ => self.r.resolve(self.level, sym),
                },
                None => {
                    return Err(EvalError::Unresolved(undefined(self.r.net, name)));
                }
            }
        };
        let top = self.r.frames.last_mut().expect("inside an evaluation");
        top.deps.extend(v.node);
        v.value
            .map(|x| (x, Vec::new()))
            .map_err(EvalError::Unresolved)
    }

    /// `None`: the built-in function of the name (see
    /// [`Resolver::find_func`]).
    fn user_fn(
        &mut self,
        name: &str,
        args: &[f64],
    ) -> Option<Result<(f64, Vec<Caveat>), EvalError>> {
        self.r.call(self.level, name, args, self.text)
    }
}

/// One parameter of an element, as it is read.
struct Item<'n> {
    name: String,
    sym: Option<Sym>,
    src: Src<'n>,
    caveats: Vec<String>,
    /// An override of an `X` line that binds to a definition: evaluated
    /// in the scope of the new instance (see [`Text`]).
    child: bool,
}

enum Src<'n> {
    /// A parameter of the line.
    Param(&'n ParamDef),
    /// A word of the line read as the value (an R/C/L word that names no
    /// visible `.model`), on line `LocIdx`.
    Word(String, LocIdx),
    /// No value, for this reason (the parameter of the line, if any).
    Fail(String, Option<&'n ParamDef>),
    /// A value that HSPICE gives with no text, described.
    Builtin(f64, &'static str),
    /// The DC value of a source with none: its function at t=0, with a
    /// `TD=` of the line.
    SourceT0(&'n ParamDef, Option<&'n ParamDef>),
}

impl<'n> Item<'n> {
    fn param(net: &Netlist, p: &'n ParamDef) -> Self {
        Item {
            name: net.syms.name(p.name).to_owned(),
            sym: Some(p.name),
            src: Src::Param(p),
            caveats: Vec::new(),
            child: false,
        }
    }

    /// An item named by the built-in name `name` (lowercase).
    fn named(net: &Netlist, name: &str, src: Src<'n>) -> Self {
        Item {
            name: name.to_owned(),
            sym: net.syms.get(name),
            src,
            caveats: Vec::new(),
            child: false,
        }
    }
}

/// Runs `f` on a thread with a stack of [`STACK`] bytes, and returns what
/// it returns (a panic in `f` goes on in the caller).
fn on_big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> Result<T, QueryError> {
    std::thread::scope(|s| {
        let thread = std::thread::Builder::new()
            .name("netlist-params".to_owned())
            .stack_size(STACK)
            .spawn_scoped(s, f)
            .map_err(|e| {
                QueryError::BadArgument(format!(
                    "cannot start the thread that resolves parameters: {e}"
                ))
            })?;
        Ok(thread
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
    })
}

/// The text of `p` for answers.
fn param_text(net: &Netlist, p: &ParamDef) -> String {
    net.param_text(p)
        .map(Cow::into_owned)
        .or_else(|| p.num.map(|n| n.to_string()))
        .unwrap_or_default()
}

fn loc_str(net: &Netlist, loc: LocIdx) -> String {
    let l = net.loc(loc);
    format!("{}:{}", l.file, l.line)
}

/// The height of the tree `e`.
fn height(e: &Expr) -> usize {
    match e {
        Expr::Num(_) | Expr::Ident(_) => 1,
        Expr::Call { args, .. } => 1 + args.iter().map(height).max().unwrap_or(0),
        Expr::Unary { e, .. } => 1 + height(e),
        Expr::Binary { l, r, .. } => 1 + height(l).max(height(r)),
        Expr::Ternary { c, t, f } => 1 + height(c).max(height(t)).max(height(f)),
    }
}

/// The first statistical function that `e` calls.
pub(crate) fn statistical<'e>(e: &'e Expr, user: &mut dyn FnMut(&str) -> bool) -> Option<&'e str> {
    match e {
        Expr::Num(_) | Expr::Ident(_) => None,
        Expr::Call { name, args } => {
            if STATISTICAL.contains(&name.as_str()) && !user(name) {
                return Some(name);
            }
            args.iter().find_map(|a| statistical(a, user))
        }
        Expr::Unary { e, .. } => statistical(e, user),
        Expr::Binary { l, r, .. } => statistical(l, user).or_else(|| statistical(r, user)),
        Expr::Ternary { c, t, f } => statistical(c, user)
            .or_else(|| statistical(t, user))
            .or_else(|| statistical(f, user)),
    }
}

/// `msg` with each word that is a number of more than 12 characters
/// written as [`fmt_num`] does (`1000…0` with 309 digits gives `1e308`).
fn short_numbers(msg: &str) -> String {
    msg.split(' ')
        .map(|w| match w.parse::<f64>() {
            Ok(x) if w.len() > 12 => Cow::Owned(fmt_num(x)),
            _ => Cow::Borrowed(w),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` for a reason: at most [`QUOTE_MAX`] characters, then `…`.
fn quote(text: &str) -> Cow<'_, str> {
    match text.char_indices().nth(QUOTE_MAX) {
        Some((at, _)) => Cow::Owned(format!("{}…", &text[..at])),
        None => Cow::Borrowed(text),
    }
}

/// A number for a caveat: 7 significant digits at most, in plain form from
/// 0.001 to below 1e7 (`2`, `1500`, `0.0015`), else with an exponent
/// (`1.4e-12`, `1e-6`).
fn fmt_num(x: f64) -> String {
    let trim = |s: &str| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            s.to_owned()
        }
    };
    if x == 0.0 {
        return "0".to_owned();
    }
    if !x.is_finite() {
        return x.to_string();
    }
    let s = format!("{x:.6e}");
    let Some((m, e)) = s.split_once('e') else {
        return s;
    };
    match e.parse::<i32>() {
        // 7 significant digits: 6 - e after the point
        Ok(e) if (-3..=6).contains(&e) => {
            let rounded: f64 = s.parse().unwrap_or(x);
            trim(&format!("{rounded:.*}", (6 - e) as usize))
        }
        _ => format!("{}e{e}", trim(m)),
    }
}

/// The `.temp` line of `p` has more text after its first value (a list of
/// temperatures).
fn more_on_line(net: &Netlist, p: &ParamDef) -> bool {
    let Some(t) = p.text else { return false };
    let text = &net.files[t.file as usize].text;
    let rest = &text[t.end as usize..];
    let line = rest.split('\n').next().unwrap_or("");
    let line = line.split('$').next().unwrap_or("");
    !line.trim().trim_start_matches('\'').trim().is_empty()
}

impl Netlist {
    /// The parameter `p` of an element line is written as `key=value` (a
    /// `=` before its text, also before a quote), not as a bare token.
    fn after_eq(&self, p: &ParamDef) -> bool {
        p.text.is_some_and(|t| {
            after_eq(
                self.files[t.file as usize].text.as_bytes(),
                t.start as usize,
            )
        })
    }

    /// The token right after the text of `p` on its statement.
    fn word_after(&self, p: &ParamDef) -> Option<&str> {
        let t = p.text?;
        let text = &self.files[t.file as usize].text;
        let b = text.as_bytes();
        let mut at = t.end as usize;
        if b.get(at) == Some(&b'\'') {
            at += 1;
        }
        let start = next_token(b, at)?;
        let end = b[start..]
            .iter()
            .position(|&c| c.is_ascii_whitespace() || c == b'$')
            .map_or(b.len(), |n| start + n);
        text.get(start..end)
    }

    /// The token of the R, C or L line `e` that starts first at or after
    /// byte `at`, before the first `key=value`: its start and end. A quoted
    /// expression is one token.
    fn token_from(&self, e: &Element, at: usize) -> Option<(usize, usize)> {
        let b = self.files[e.loc.file as usize].text.as_bytes();
        let start = next_token(b, at)?;
        let end = if b[start] == b'\'' {
            b[start + 1..]
                .iter()
                .position(|&c| c == b'\'')
                .map_or(b.len(), |n| start + n + 2)
        } else {
            b[start..]
                .iter()
                .position(|&c| c.is_ascii_whitespace() || matches!(c, b'$' | b'='))
                .map_or(b.len(), |n| start + n)
        };
        // `r=5k`, `r = 5k`: the keywords start
        if end == start
            || b.get(end) == Some(&b'=')
            || next_token(b, end).is_some_and(|n| b[n] == b'=')
        {
            return None;
        }
        Some((start, end))
    }

    /// The tokens of the R, C or L line `e` from byte `at`, before the
    /// first `key=value`, at most 4 (`R1 a b rval 0.004 1e-5 r=5k` gives
    /// `0.004`, `1e-5` after `rval`).
    fn words_from(&self, e: &Element, mut at: usize) -> Vec<&str> {
        let text = &self.files[e.loc.file as usize].text;
        let mut out = Vec::new();
        while out.len() < 4
            && let Some((start, end)) = self.token_from(e, at)
        {
            out.push(&text[start..end]);
            at = end;
        }
        out
    }

    /// The end of the model token of the R, C or L line `e`, whose second
    /// node token ends at byte `nodes_end`, when it is the first token
    /// after the nodes (`R1 a b rval 0.004 r=5k`).
    fn model_end(&self, e: &Element, nodes_end: Option<usize>) -> Option<usize> {
        let m = self.syms.name(e.model?);
        let (start, end) = self.token_from(e, nodes_end?)?;
        let text = &self.files[e.loc.file as usize].text;
        text[start..end].eq_ignore_ascii_case(m).then_some(end)
    }

    /// The token of the R, C or L line `e` that starts first at or after
    /// byte `at` when it is a quoted expression (`R1 a b 'rval' r=5k`): its
    /// text inside the quotes, on one line (without the `+` markers of a
    /// text that goes on on a `+` line, as `param_text` gives it; a `$` and
    /// a `*` comment line inside the quotes are kept, see
    /// `crate::lower::one_line`), and the end of the token.
    fn quoted_at(&self, e: &Element, at: usize) -> Option<(Cow<'_, str>, usize)> {
        let (start, end) = self.token_from(e, at)?;
        let token = &self.files[e.loc.file as usize].text[start..end];
        let inner = token.strip_prefix('\'')?;
        let inner = inner.strip_suffix('\'').unwrap_or(inner);
        Some((one_line(inner, true), end))
    }

    /// The text of `p` when it is one bare name (not quoted, not a
    /// number).
    fn plain_word(&self, p: &ParamDef) -> Option<&str> {
        let t = p.text?;
        if p.num.is_some() {
            return None;
        }
        let text = &self.files[t.file as usize].text;
        if t.start > 0 && text.as_bytes()[t.start as usize - 1] == b'\'' {
            return None;
        }
        let word = text[t.start as usize..t.end as usize].trim();
        matches!(netlist_expr::parse(word), Ok(Expr::Ident(_))).then_some(word)
    }

    /// The `.model` card `m` of the first run that is visible from
    /// definition `def`: in it, in a definition it is written inside, or
    /// at the top level, the nearest one (the rule of the hierarchy
    /// analysis for `X` lines).
    fn visible_model(&self, def: DefId, m: Sym) -> Option<&ModelDef> {
        let mut at = Some(def);
        while let Some(d) = at {
            let found = self.defs[d.idx()]
                .models
                .iter()
                .find(|md| md.name == m && md.alter.is_none());
            if found.is_some() {
                return found;
            }
            at = (d != ROOT).then(|| self.defs[d.idx()].parent.unwrap_or(ROOT));
        }
        None
    }

    /// The caveats of `.alter` blocks that set `.option parhier`, or that
    /// redefine a definition of the path `w`, an `X` line of the path or
    /// the element at its end.
    fn alter_caveats(&self, w: &Walk) -> Vec<String> {
        let mut out = Vec::new();
        if self.alters.is_empty() {
            return out;
        }
        let parhier = self.syms.get("parhier");
        for p in self.options.iter().filter(|p| Some(p.name) == parhier) {
            if let Some(a) = p.alter {
                out.push(format!(
                    "`.option parhier` is set again by .alter #{a} at {}: this answer uses the first run's parhier ({}), and a later run can take other definitions (golden `alter`: the changes of the `.alter` blocks accumulate over the runs)",
                    loc_str(self, p.loc),
                    match self.parhier {
                        Parhier::Local => "local",
                        Parhier::Global => "global",
                    }
                ));
            }
        }
        for &d in w.defs.iter().chain(&w.last_binding) {
            let def = &self.defs[d.idx()];
            let parent = def.parent.unwrap_or(ROOT);
            for &other in &self.defs[parent.idx()].nested {
                let o = &self.defs[other.idx()];
                if let (Some(a), true) = (o.alter, o.name == def.name && d != ROOT) {
                    out.push(format!(
                        "subckt `{}` is defined again by .alter #{a} at {}: this answer describes the first run",
                        self.def_display(d),
                        loc_str(self, o.loc)
                    ));
                }
            }
        }
        for &(holder, i) in &w.elems {
            let e = &self.defs[holder.idx()].elements[i];
            for o in &self.defs[holder.idx()].elements {
                if let (Some(a), true) = (o.alter, o.name == e.name) {
                    out.push(format!(
                        "`{}` is changed by .alter #{a} at {}: this answer describes the first run (golden `alter`: the changes of the `.alter` blocks accumulate over the runs)",
                        self.syms.name(e.name),
                        loc_str(self, o.loc)
                    ));
                }
            }
        }
        out
    }

    /// The scopes of the instance path `scope_path` (`""`: the top level,
    /// also in a bare netlist), and the caveats of the path.
    fn param_scopes(&self, scope_path: &str) -> Result<(Vec<Scope>, Vec<String>), QueryError> {
        let p = scope_path.trim();
        if (p.is_empty() || p == ".") && self.root == Root::None {
            let top = Scope {
                def: ROOT,
                x: None,
                path: String::new(),
            };
            return Ok((vec![top], Vec::new()));
        }
        let w = walk(self, p, Want::Instance)?;
        if let Some(&(holder, i)) = w.elems.last() {
            let e = &self.defs[holder.idx()].elements[i];
            if e.kind != ElemKind::Instance {
                return Err(QueryError::BadArgument(format!(
                    "`{}` is a {}, not a subckt instance: give the path of the instance that holds it (or use resolve_element for its parameters)",
                    w.path,
                    e.kind.name()
                )));
            }
            if w.last_binding.is_none() {
                return Err(QueryError::BadArgument(format!(
                    "`{}` calls subckt `{}`, which has no visible definition on this path (diagnostic `unresolved_subckt`): it has no scope",
                    w.path,
                    e.callee.map_or("", |c| self.syms.name(c))
                )));
            }
        }
        let mut caveats = w.caveats.clone();
        caveats.extend(self.path_caveats(&w));
        caveats.extend(self.alter_caveats(&w));
        Ok((scopes(self, &w, true), caveats))
    }

    /// The value of parameter `name` (any letter case) as seen in the scope
    /// of the instance `scope_path` (`""`: the top level), under
    /// `.option parhier`, with its chain. `temper` is the circuit
    /// temperature. A name with no definition on the path is `NotFound`,
    /// with close names; a value that cannot be computed is `unresolved`
    /// with the reason.
    pub fn resolve_param(&self, scope_path: &str, name: &str) -> Result<Resolved, QueryError> {
        on_big_stack(|| self.resolve_param_here(scope_path, name))?
    }

    fn resolve_param_here(&self, scope_path: &str, name: &str) -> Result<Resolved, QueryError> {
        let wanted = name.trim();
        if wanted.is_empty() {
            return Err(QueryError::BadArgument(
                "the parameter name is empty".to_owned(),
            ));
        }
        let (scopes, mut path_caveats) = self.param_scopes(scope_path)?;
        let mut r = Resolver::new(self, scopes);
        let k = r.scopes.len() - 1;
        path_caveats.extend(r.path_m_caveats(k));
        let (shown, v) = if wanted.eq_ignore_ascii_case("temper") {
            ("temper".to_owned(), r.temperature())
        } else {
            let sym = self
                .syms
                .get(wanted)
                .filter(|&s| r.find(k, s).is_some())
                .ok_or_else(|| QueryError::NotFound {
                    what: "parameter".to_owned(),
                    name: wanted.to_owned(),
                    suggestions: r.visible_names(k, wanted),
                })?;
            (self.syms.name(sym).to_owned(), r.resolve(k, sym))
        };
        let frame = Frame {
            deps: v.node.into_iter().collect(),
            caveats: path_caveats,
            ..Frame::default()
        };
        let mut res = r.answer(shown, v.value, None, &frame);
        cap_caveats(&mut res.caveats);
        Ok(res)
    }

    /// The parameters of the element at `path` (a device or an `X` line),
    /// each resolved in the scope of the definition that holds it (the
    /// overrides of an `X` line in the scope of the instance it makes,
    /// golden `ovr_scope_local`): the R/C/L value as `r`/`c`/`l` (the
    /// element's own value, times its `scale=`, golden `src_delay_scale`,
    /// `elem_scale_cl`; the `.op` listing prints R/m under an instance with
    /// `m`), the `dc`, function, `ac` and `acphase` of a source, and every
    /// `key=value`. An `X`-line override that a definition above the new
    /// instance beats (GLOBAL, golden `nested_global`) keeps its own value,
    /// with a caveat that gives the value the instance uses.
    ///
    /// R/C/L: a word after the nodes is a model only when a `.model` card
    /// of that name is visible (golden `res_model`). With no such card,
    /// `R1 a 0 rval 0.004` reads as the value `rval` and TC1 0.004 (golden
    /// `res_tc_positional`, `rcl_param_tc`), and `L1 a b lval lmod` as the
    /// value `lval` and the model `lmod` (golden `ind_value_model`), each
    /// with a caveat; an `r=` after such a name is not used (golden
    /// `rcl_kw_after_more`). TC1/TC2 at a temperature other than TNOM give
    /// a caveat with the temperature-adjusted value that `.op` prints.
    ///
    /// Sources: with no DC value, `dc` is the value of the function at
    /// t=0 (golden `src_fn_nodc`, `src_sin_t0`, `src_delay_scale`, `pwl_*`:
    /// PULSE, EXP, SFFM their first argument, SIN offset + amplitude ×
    /// sin(phase), PWL its value at t=0, AM 0), with a caveat; other
    /// functions are unresolved. With no DC value and no function it is 0.
    /// An AC magnitude with no phase has the phase 0
    /// (`src_ac_default_phase`).
    pub fn resolve_element(&self, path: &str) -> Result<ElementParams, QueryError> {
        on_big_stack(|| self.resolve_element_here(path).map(|(ep, _)| ep))?
    }

    /// [`Netlist::resolve_element`], and the work of the call (see
    /// [`Resolver::work`]).
    #[cfg(test)]
    pub(crate) fn resolve_element_work(
        &self,
        path: &str,
    ) -> Result<(ElementParams, u64), QueryError> {
        on_big_stack(|| self.resolve_element_here(path))?
    }

    fn resolve_element_here(&self, path: &str) -> Result<(ElementParams, u64), QueryError> {
        let w = walk(self, path, Want::Element)?;
        let Some(&(holder, i)) = w.elems.last() else {
            return Err(QueryError::BadArgument(
                "the path is empty: it names the root, not an element".to_owned(),
            ));
        };
        let d = &self.defs[holder.idx()];
        let e = &d.elements[i];
        let mut caveats = w.caveats.clone();
        match e.kind {
            ElemKind::Other(_) => caveats.push(
                "element type not parsed: its node list and its parameters are not trusted"
                    .to_owned(),
            ),
            ElemKind::Instance if w.last_binding.is_none() => caveats.push(format!(
                "no definition of subckt `{}` is visible on this path (diagnostic `unresolved_subckt`)",
                e.callee.map_or("", |c| self.syms.name(c))
            )),
            _ => {}
        }
        caveats.extend(self.path_caveats(&w));
        caveats.extend(self.alter_caveats(&w));
        // an `X` line's overrides are evaluated in the scope of the
        // instance it makes (one level below the line)
        let binding = e.kind == ElemKind::Instance && w.last_binding.is_some();
        let mut r = Resolver::new(self, scopes(self, &w, binding));
        let k = r.scopes.len() - 1 - usize::from(binding);
        caveats.extend(r.path_m_caveats(k));
        let params = d.element_params(e);
        let (items, model, more) = match e.kind {
            ElemKind::Resistor | ElemKind::Capacitor | ElemKind::Inductor => {
                self.rcl_items(holder, i)
            }
            ElemKind::Vsource | ElemKind::Isource => (self.source_items(params), None, Vec::new()),
            ElemKind::Instance => (self.x_items(params, binding), e.model, Vec::new()),
            ElemKind::Other(_) => {
                let items = params
                    .iter()
                    .map(|p| Item {
                        src: Src::Fail(
                            "element type not parsed: its parameters are not resolved".to_owned(),
                            Some(p),
                        ),
                        ..Item::param(self, p)
                    })
                    .collect();
                (items, None, Vec::new())
            }
            _ => (
                params.iter().map(|p| Item::param(self, p)).collect(),
                e.model,
                Vec::new(),
            ),
        };
        caveats.extend(more);
        let total = items.len();
        let syms: Vec<Option<Sym>> = items.iter().map(|it| it.sym).collect();
        let mut out: Vec<Resolved> = items
            .into_iter()
            .take(MAX_LIMIT)
            .map(|it| {
                // an override that a definition above the new instance
                // beats (GLOBAL)
                let shadow = match (&it.src, it.sym) {
                    (Src::Param(p), Some(s)) if binding && Some(s) != r.m => {
                        r.shadowed(k + 1, s, p)
                    }
                    _ => Vec::new(),
                };
                let mut res = r.resolve_item(it, k, &w.path);
                // the override text and the winning value can share a
                // caveat (an `.alter` or a sweep of a top-level `.param`)
                let known: HashSet<String> = res.caveats.iter().cloned().collect();
                res.caveats
                    .extend(shadow.into_iter().filter(|c| !known.contains(c)));
                res
            })
            .collect();
        self.value_caveats(&mut r, e.kind, &syms, &mut out);
        // `.dc v1 0 1 0.1`: the DC value of a top-level source is swept
        if matches!(e.kind, ElemKind::Vsource | ElemKind::Isource)
            && holder == ROOT
            && !self.swept.is_empty()
            && let Some(dc) = out.iter_mut().find(|p| p.name == "dc")
        {
            dc.caveats.extend(self.swept_caveats(e.name));
        }
        for p in &mut out {
            // `1e300 scale=1e10`, a PWL of huge values
            if p.value.is_some_and(|v| !v.is_finite()) {
                p.value = None;
                p.unresolved = Some(OUT_OF_RANGE.to_owned());
            }
        }
        for p in &mut out {
            cap_caveats(&mut p.caveats);
        }
        cap_caveats(&mut caveats);
        let ep = ElementParams {
            path: w.path,
            kind: e.kind.name().to_owned(),
            model: model.map(|m| self.syms.name(m).to_owned()),
            params: out,
            params_total: total,
            caveats,
        };
        Ok((ep, r.work.get()))
    }

    /// The caveats that depend on the values of an element's parameters
    /// (`syms[i]` is the name of `out[i]`): TC1/TC2 at another temperature,
    /// `m` <= 0, `.option scale`.
    fn value_caveats(
        &self,
        r: &mut Resolver<'_>,
        kind: ElemKind,
        syms: &[Option<Sym>],
        out: &mut [Resolved],
    ) {
        let get = |name: &str| self.syms.get(name);
        let n = out.len();
        let at = |s: Option<Sym>| {
            s.and_then(|s| syms.iter().position(|&t| t == Some(s)))
                .filter(|&i| i < n)
        };
        let key = match kind {
            ElemKind::Resistor => Some("r"),
            ElemKind::Capacitor => Some("c"),
            ElemKind::Inductor => Some("l"),
            _ => None,
        };
        if let Some(key) = key
            && let Some(vi) = at(get(key))
        {
            // the element's `scale=` multiplies the value: `.option list`
            // `res eff`, `cap eff`, `ind eff` and the `.op` values (golden
            // `src_delay_scale`, `elem_scale_cl`)
            if let Some(si) = at(get("scale"))
                && out[vi].value.is_some()
            {
                match out[si].value {
                    Some(s) if s != 1.0 => {
                        let x = out[vi].value.expect("checked");
                        out[vi].value = Some(x * s);
                        out[vi].caveats.push(format!(
                            "the line's `scale={s}` multiplies the value as written ({}): HSPICE's effective value is {} (golden `src_delay_scale`, `elem_scale_cl`)",
                            fmt_num(x),
                            fmt_num(x * s)
                        ));
                        let more: Vec<ChainStep> = out[si].chain.clone();
                        let room = MAX_LIMIT.saturating_sub(out[vi].chain.len());
                        out[vi].chain.extend(more.into_iter().take(room));
                    }
                    Some(_) => {}
                    None => {
                        let why = out[si].unresolved.clone().unwrap_or_default();
                        out[vi].value = None;
                        out[vi].unresolved = Some(format!(
                            "HSPICE multiplies the value by the line's `scale=`, which has no value: {why}"
                        ));
                    }
                }
            }
            let tc = |name: &str| at(get(name)).map(|i| &out[i]);
            let (tc1, tc2, dtemp) = (tc("tc1"), tc("tc2"), tc("dtemp"));
            let set = |t: Option<&Resolved>| t.is_some_and(|t| t.value != Some(0.0));
            if set(tc1) || set(tc2) {
                let temp = r.temperature().value;
                let tnom = r.tnom();
                // `dtemp=` on the line: the element is that much warmer
                let (dtemp, shown) = match dtemp.map(|d| d.value) {
                    None => (Some(0.0), String::new()),
                    Some(Some(d)) => (Some(d), format!(" + DTEMP {d}")),
                    Some(None) => (None, " + DTEMP (not resolved)".to_owned()),
                };
                if let (Ok(t), Ok(tn)) = (temp, tnom)
                    && dtemp.is_none_or(|d| t + d != tn)
                {
                    let coef = |t: Option<&Resolved>| t.map_or(Some(0.0), |t| t.value);
                    let adjusted = match (out[vi].value, coef(tc1), coef(tc2), dtemp) {
                        (Some(x), Some(a), Some(b), Some(d)) => {
                            let dt = t + d - tn;
                            format!(" = {}", fmt_num(x * (1.0 + a * dt + b * dt * dt)))
                        }
                        _ => String::new(),
                    };
                    out[vi].caveats.push(format!(
                        "TC1/TC2 are set and the temperature is {t}{shown} (TNOM {tn}): HSPICE's `.op` uses {key}*(1 + TC1*dT + TC2*dT^2){adjusted} at that temperature; this value is the one at TNOM (golden `res_tc_positional`, `cl_tc_positional`, `rcl_param_tc`; TNOM from `.option tnom`, `dtemp=` and `scale=`: golden `tc_tnom_dtemp`)"
                    ));
                    let later = self
                        .temp_alter_caveats()
                        .into_iter()
                        .chain(self.option_alter_caveats("tnom"));
                    out[vi].caveats.extend(later);
                }
                // a sweep of the temperature changes the value, also from TNOM
                let swept = ["temp", "temper"]
                    .iter()
                    .filter_map(|t| self.syms.get(t))
                    .flat_map(|t| self.swept_caveats(t));
                out[vi].caveats.extend(swept);
            }
        }
        if !matches!(
            kind,
            ElemKind::Vsource | ElemKind::Isource | ElemKind::Other(_)
        ) && let Some(i) = at(get("m"))
            && out[i].value.is_some_and(|m| m <= 0.0)
        {
            out[i].caveats.push(
                "HSPICE rejects a multiplier of zero or less on an instance path (golden `elem_m_zero`, `hier_m_zero`, `m_expr_zero`; diagnostic `m_not_positive`)"
                    .to_owned(),
            );
        }
        let scale = get("scale").and_then(|s| {
            self.options
                .iter()
                .rfind(|p| p.name == s && p.alter.is_none())
        });
        if let Some(p) = scale
            && p.num != Some(1.0)
            && !matches!(
                kind,
                ElemKind::Vsource | ElemKind::Isource | ElemKind::Instance | ElemKind::Other(_)
            )
        {
            for name in GEOMETRIC {
                // an inductor's `l` is its inductance, not a length
                if kind == ElemKind::Inductor && name == "l" {
                    continue;
                }
                if let Some(i) = at(get(name)) {
                    out[i].caveats.push(format!(
                        "`.option scale` is set ({}): HSPICE multiplies this geometric parameter by it; the value here is as written",
                        loc_str(self, p.loc)
                    ));
                    out[i].caveats.extend(self.option_alter_caveats("scale"));
                }
            }
        }
    }

    /// The caveats of the analyses of the first run that sweep the name
    /// `n` (a parameter, a source, `temp`): the answer gives the value of
    /// the deck, the nominal one, which is the `.op` value (golden
    /// `sweep_op`; spec 6.2a).
    fn swept_caveats(&self, n: Sym) -> Vec<String> {
        // `swept` is sorted by name
        let at = self.swept.partition_point(|s| s.name < n);
        self.swept[at..]
            .iter()
            .take_while(|s| s.name == n)
            .map(|s| {
                let via = s.data.map_or(String::new(), |(d, at)| {
                    format!(
                        " (a parameter of the `.data` block `{}` at {})",
                        self.syms.name(d),
                        loc_str(self, at)
                    )
                });
                format!(
                    "`{}` is swept by .{} at {}{via}; this is the nominal value, the one of the deck, which is the `.op` value (golden `sweep_op`; spec 6.2a)",
                    self.syms.name(n),
                    s.cmd,
                    loc_str(self, s.loc)
                )
            })
            .collect()
    }

    /// The caveats of the `.temp` lines of `.alter` blocks: they set the
    /// temperature of the later runs.
    fn temp_alter_caveats(&self) -> Vec<String> {
        self.temps
            .iter()
            .filter_map(|p| {
                p.alter.map(|a| {
                    format!(
                        "the temperature is set again by .alter #{a} at {}: this is the first run's temperature",
                        loc_str(self, p.loc)
                    )
                })
            })
            .collect()
    }

    /// The caveats of the `.option <name>` items of `.alter` blocks.
    fn option_alter_caveats(&self, name: &str) -> Vec<String> {
        let Some(sym) = self.syms.get(name) else {
            return Vec::new();
        };
        self.options
            .iter()
            .filter(|p| p.name == sym)
            .filter_map(|p| {
                p.alter.map(|a| {
                    format!(
                        "`.option {name}` is set again by .alter #{a} at {}: this is the first run's value",
                        loc_str(self, p.loc)
                    )
                })
            })
            .collect()
    }

    /// The parameters of an `X` line. An override is evaluated in the scope
    /// of the instance that the line makes (`binding`: the line binds to a
    /// definition on this path), with the overridden name itself from the
    /// line's scope (see [`Text`]); `m` in the line's scope (golden
    /// `ovr_scope_more_local`). With no binding, the scope of an override
    /// is not known: only a number has a value.
    fn x_items<'n>(&'n self, params: &'n [ParamDef], binding: bool) -> Vec<Item<'n>> {
        let m = self.syms.get("m");
        params
            .iter()
            .map(|p| {
                let item = Item::param(self, p);
                if Some(p.name) == m || p.num.is_some() {
                    item
                } else if binding {
                    Item {
                        child: true,
                        ..item
                    }
                } else {
                    Item {
                        src: Src::Fail(
                            "the subckt that the line calls has no visible definition on this path, and HSPICE evaluates an override in the scope of the new instance: not resolved".to_owned(),
                            Some(p),
                        ),
                        ..item
                    }
                }
            })
            .collect()
    }

    /// The parameters of a V/I line: `dc` first (from the function at t=0,
    /// or 0, when the line has none), then the others in order, with
    /// `acphase` 0 after an `ac` with no phase.
    fn source_items<'n>(&'n self, params: &'n [ParamDef]) -> Vec<Item<'n>> {
        let sym = |n: &str| self.syms.get(n);
        let (dc, ac, acphase, td) = (sym("dc"), sym("ac"), sym("acphase"), sym("td"));
        let is_func = |p: &ParamDef| {
            let n = self.syms.name(p.name);
            SOURCE_FUNCS.iter().any(|f| f.eq_ignore_ascii_case(n)) && !self.after_eq(p)
        };
        let func = params.iter().find(|p| is_func(p));
        let mut items = Vec::with_capacity(params.len() + 2);
        match (params.iter().find(|p| Some(p.name) == dc), func) {
            (Some(p), _) => items.push(Item::param(self, p)),
            (None, Some(f)) => {
                let td = params.iter().find(|p| Some(p.name) == td);
                items.push(Item::named(self, "dc", Src::SourceT0(f, td)));
            }
            (None, None) => items.push(Item {
                caveats: vec![
                    "the line has no DC value and no function: its DC value is 0, as in SPICE (not probed)"
                        .to_owned(),
                ],
                ..Item::named(self, "dc", Src::Builtin(0.0, "0 (no DC value on the line)"))
            }),
        }
        let has_phase = params.iter().any(|p| Some(p.name) == acphase);
        let has_dc = params.iter().any(|p| Some(p.name) == dc);
        for p in params {
            if Some(p.name) == dc {
                continue;
            }
            if is_func(p) {
                let fname = self.syms.name(p.name);
                let why = if has_dc {
                    format!(
                        "a {fname} waveform has no single value: `.op` uses the DC value of the line (golden `src_forms`)"
                    )
                } else {
                    format!(
                        "a {fname} waveform has no single value: with no DC value on the line, `dc` is its value at t=0"
                    )
                };
                items.push(Item {
                    src: Src::Fail(why, Some(p)),
                    ..Item::param(self, p)
                });
                continue;
            }
            items.push(Item::param(self, p));
            if Some(p.name) == ac && !has_phase {
                // golden `src_ac_default_phase`
                items.push(Item::named(
                    self,
                    "acphase",
                    Src::Builtin(0.0, "0 (no AC phase on the line: the default)"),
                ));
            }
        }
        // the parts of the source have the plan's names, whatever the
        // deck's spelling (`DC 1`)
        for it in &mut items {
            for (name, sym) in [("dc", dc), ("ac", ac), ("acphase", acphase)] {
                if it.sym.is_some() && it.sym == sym {
                    name.clone_into(&mut it.name);
                }
            }
        }
        items
    }

    /// For R, C or L line `i` of `holder`, as [`Netlist::resolve_element`]
    /// reads it: the reason HSPICE rejects the line (`R1 1 0 rval rmod`,
    /// golden `res_name_model`), and its visible model card. The reading
    /// does not depend on the instance path (the load checks of
    /// `crate::rejects`).
    pub(crate) fn rcl_check(
        &self,
        holder: DefId,
        i: ElemIdx,
    ) -> (Option<String>, Option<&ModelDef>) {
        let (items, model, _) = self.rcl_items(holder, i);
        let why = items.into_iter().find_map(|it| match it.src {
            Src::Fail(why, _) if why.contains(REJECTS) => Some(why),
            _ => None,
        });
        (why, model.and_then(|m| self.visible_model(holder, m)))
    }

    /// The parameters of an R, C or L line, read as HSPICE reads the tokens
    /// after the nodes (see [`Netlist::resolve_element`]), the model, and
    /// caveats on the element. The value is named `r`, `c` or `l`.
    fn rcl_items<'n>(
        &'n self,
        holder: DefId,
        i: ElemIdx,
    ) -> (Vec<Item<'n>>, Option<Sym>, Vec<String>) {
        let d = &self.defs[holder.idx()];
        let e = &d.elements[i];
        let nodes_end = d.nodes_end(i);
        let params = d.element_params(e);
        let key = match e.kind {
            ElemKind::Resistor => "r",
            ElemKind::Capacitor => "c",
            _ => "l",
        };
        let ksym = self.syms.get(key);
        let mut items: Vec<Item<'n>> = params.iter().map(|p| Item::param(self, p)).collect();
        let vi = params.iter().position(|p| Some(p.name) == ksym);
        let mut model = e.model;
        let mut caveats = Vec::new();
        let card = |m: Sym| self.visible_model(holder, m);
        let model_caveat = |md: &ModelDef| {
            let mut c = format!(
                "the element has the model `{}` (`.model` at {}): model-card parameters are not resolved, and some (a scale, a TC) can change the effective value",
                self.syms.name(md.name),
                loc_str(self, md.loc)
            );
            for o in self.defs[ROOT.idx()]
                .models
                .iter()
                .filter(|o| o.name == md.name)
            {
                if let Some(a) = o.alter {
                    c.push_str(&format!(
                        ". `.alter #{a}` at {} defines the model again (this is the first run)",
                        loc_str(self, o.loc)
                    ));
                }
            }
            c
        };
        let no_card = |m: &str| format!("no `.model {m}` card is visible from the line");
        // `R1 a b 'rval' r=5k`: a quoted expression in the place of the
        // value, which the `key=` value replaced in `params`: right after
        // the nodes, or after the name of a visible model card that comes
        // first (`R1 a b rmod 'rval' r=5k`); an L line can also have its
        // model after it
        let model_end = self.model_end(e, nodes_end);
        let quoted =
            vi.filter(|&vi| self.after_eq(&params[vi]))
                .and_then(|_| match (e.model, model_end) {
                    // a first name with no visible card is the value (golden
                    // `rcl_kw_after_more`)
                    (Some(m), Some(at)) => card(m).and_then(|_| self.quoted_at(e, at)),
                    (Some(_), None) if e.kind != ElemKind::Inductor => None,
                    _ => self.quoted_at(e, nodes_end?),
                });
        if let (Some(vi), Some((q, q_end))) = (vi, quoted) {
            let mut rest = self.words_from(e, q_end);
            let mut more = Vec::new();
            if let Some(m) = e.model {
                let mname = self.syms.name(m);
                if model_end.is_some() {
                    more.push(format!(
                        "`{mname}` before it is the model, as in `R1 1 0 rmod 2k` (golden `res_model`); a quoted value after a model name, then the `{key}=`, was not probed"
                    ));
                } else {
                    // `L1 a b 'lval' lmod l=5n`: the value, then the model
                    // (golden `ind_value_model`)
                    if rest.first().is_some_and(|w| w.eq_ignore_ascii_case(mname)) {
                        rest.remove(0);
                    }
                    more.push(format!(
                        "`{mname}` after it is the model, as in `L1 1 2 1n lmod` (golden `ind_value_model`); with a quoted value this was not probed"
                    ));
                }
                caveats.push(card(m).map_or_else(|| no_card(mname), model_caveat));
            }
            let mut why = vec![self.before_key_why(&params[vi], &q, &rest, e.kind, true)];
            why.extend(more);
            items = self.name_before_key(params, vi, &q, &rest, why);
            for it in &mut items {
                if it.sym.is_some() && it.sym == ksym {
                    key.clone_into(&mut it.name);
                }
            }
            return (items, model, caveats);
        }
        match (e.model, vi) {
            (Some(m), Some(vi)) if card(m).is_some() => {
                let md = card(m).expect("checked");
                items[vi].caveats.push(model_caveat(md));
            }
            (Some(m), None) => {
                caveats.push(no_card(self.syms.name(m)));
                items.insert(
                    0,
                    Item::named(
                        self,
                        key,
                        Src::Fail("the line has no value".to_owned(), None),
                    ),
                );
            }
            (Some(m), Some(vi)) => {
                let v = &params[vi];
                let mname = self.syms.name(m);
                let word = self.plain_word(v);
                let word_card = word.and_then(|w| self.syms.get(w)).and_then(card);
                // `R1 1 0 rval rmod`, `C1 1 0 cval cmod`: a value, then a
                // model name
                let name_model = |w: &str| {
                    if e.kind == ElemKind::Resistor {
                        format!(
                            "`{mname}` is followed by the model name `{w}`: HSPICE reads `{mname}` as a third node and {REJECTS} (golden `res_value_model`, `res_name_model`)"
                        )
                    } else {
                        format!(
                            "`{mname}` is followed by the model name `{w}`: HSPICE does not read `{mname}` as the value; it uses a capacitance of 0 and warns (golden `cap_value_model`, `cap_name_model`)"
                        )
                    }
                };
                if self.after_eq(v) && model_end.is_none() {
                    // `L1 a b 1n lmod l=5n`: the model comes after a number
                    // value, which `l=` replaces (golden `elem_dup_kw`)
                    caveats.push(no_card(mname));
                } else if self.after_eq(v) {
                    // `r6 6 0 rval r=5k`: HSPICE uses the name as the value
                    // and not the `r=` after it (golden `rcl_kw_after_more`,
                    // `rcl_tc_kw_after`)
                    let rest = model_end.map_or_else(Vec::new, |at| self.words_from(e, at));
                    let first = rest
                        .first()
                        .copied()
                        .filter(|w| matches!(netlist_expr::parse(w), Ok(Expr::Ident(_))));
                    let first_card = first.and_then(|w| self.syms.get(w)).and_then(card);
                    model = None;
                    match (first, first_card) {
                        (Some(w), Some(md)) if e.kind != ElemKind::Inductor => {
                            model = Some(md.name);
                            items[vi] = Item {
                                src: Src::Fail(name_model(w), Some(v)),
                                ..Item::param(self, v)
                            };
                        }
                        (Some(w), md) if e.kind == ElemKind::Inductor => {
                            // `L1 a b lval lmod l=5n`: the value, then the
                            // model (golden `ind_value_model`)
                            model = md.map(|md| md.name);
                            let more = format!(
                                "`{w}` after it is the model, as in `L1 1 2 1n lmod` (golden `ind_value_model`)"
                            );
                            let tcs = &rest[1..];
                            let why = self.before_key_why(v, mname, tcs, e.kind, false);
                            items = self.name_before_key(params, vi, mname, tcs, vec![why, more]);
                            match md {
                                Some(md) => caveats.push(model_caveat(md)),
                                None => caveats.push(no_card(w)),
                            }
                        }
                        _ => {
                            let why = self.before_key_why(v, mname, &rest, e.kind, false);
                            items = self.name_before_key(params, vi, mname, &rest, vec![why]);
                        }
                    }
                } else if self
                    .word_after(v)
                    .is_some_and(|w| w.eq_ignore_ascii_case(mname))
                {
                    // `L1 a b 1n lmod`: the value, then the model
                    caveats.push(no_card(mname));
                } else if e.kind != ElemKind::Inductor
                    && let Some(md) = word_card
                {
                    let w = word.expect("a card has a word");
                    model = Some(md.name);
                    items[vi] = Item {
                        src: Src::Fail(name_model(w), Some(v)),
                        ..Item::param(self, v)
                    };
                } else if e.kind == ElemKind::Inductor
                    && let Some(word) = word
                {
                    // `L1 a b lval lmod`: the value, then the model (golden
                    // `ind_value_model`)
                    items[vi] = Item {
                        src: Src::Word(mname.to_owned(), v.loc),
                        caveats: vec![format!(
                            "`{mname}` names no visible `.model` card, so it is read as the value and `{word}` as the model, as in `L1 1 2 1n lmod` (golden `ind_value_model`; with a parameter name as the value, golden `ind_name_model`)"
                        )],
                        ..Item::param(self, v)
                    };
                    model = self.syms.get(word);
                    match model.and_then(card) {
                        Some(md) => items[vi].caveats.push(model_caveat(md)),
                        None => caveats.push(no_card(word)),
                    }
                } else {
                    model = None;
                    items = self.shifted_tc(params, vi, mname);
                }
            }
            (None, Some(vi)) => {
                let v = &params[vi];
                let md = (!self.after_eq(v))
                    .then(|| self.plain_word(v))
                    .flatten()
                    .and_then(|w| self.syms.get(w))
                    .and_then(card);
                if let Some(md) = md {
                    // `R1 a b rmod`: one bare name that names a visible
                    // model is the model (golden `res_model`)
                    model = Some(md.name);
                    let name = self.syms.name(md.name);
                    items[vi].src = Src::Fail(
                        format!(
                            "`{name}` is a model (`.model {name} {}` at {}): the value comes from the model card, whose parameters are not resolved",
                            self.syms.name(md.kind),
                            loc_str(self, md.loc)
                        ),
                        Some(v),
                    );
                }
            }
            (None, None) => items.insert(
                0,
                Item::named(
                    self,
                    key,
                    Src::Fail("the line has no value".to_owned(), None),
                ),
            ),
        }
        // the value has the plan's name, whatever the deck's spelling of
        // `r=` or of a `.param R`
        for it in &mut items {
            if it.sym.is_some() && it.sym == ksym {
                key.clone_into(&mut it.name);
            }
        }
        (items, model, caveats)
    }

    /// The caveat on the value `value` (a name that names no visible
    /// `.model` card, or with `quoted`, the text of a quoted expression)
    /// right after the nodes of an R, C or L line, before the `key=` value
    /// `v`, with the words `tcs` between them (see
    /// [`Netlist::name_before_key`]).
    fn before_key_why(
        &self,
        v: &ParamDef,
        value: &str,
        tcs: &[&str],
        kind: ElemKind,
        quoted: bool,
    ) -> String {
        let kv = format!("{}={}", self.syms.name(v.name), param_text(self, v));
        let mut why = if quoted {
            format!(
                "the quoted expression `'{value}'` in the place of the value is the value: HSPICE uses it and not the `{kv}` after it, on R, C and L lines (golden `rcl_quoted_sticky_local`, `rcl_quoted_more`); a number there gives the `{kv}` value (golden `elem_dup_kw`)"
            )
        } else {
            format!(
                "`{value}` names no visible `.model` card, so it is read as the value: HSPICE uses it and not the `{kv}` after it (golden `rcl_kw_after_more`: `r6 6 0 rval r=5k` gives 2k; a number before `r=` gives the `r=` value, golden `elem_dup_kw`)"
            )
        };
        if !tcs.is_empty() {
            why.push_str(if quoted {
                ". The numbers after it are TC1 and TC2, as after a name (golden `rcl_tc_kw_after`; not probed after a quoted expression)"
            } else {
                ". The numbers after the name are TC1 and TC2 (golden `rcl_tc_kw_after`)"
            });
        }
        if let Some(w) = tcs.get(2) {
            why.push_str(&format!(
                ". The third number after the value (`{w}`) has no place and is not used"
            ));
        }
        if kind != ElemKind::Resistor && !quoted {
            // golden `rcl_name_first_cl`, `ind_name_model`: C and L with a
            // name, then TC1 and TC2, and no keyword
            why.push_str(&format!(
                ". A name before the `{}=` was probed for R only",
                self.syms.name(v.name)
            ));
        }
        why
    }

    /// The items of `R1 a b rval 0.004 1e-5 r=5k`, where `rval` names no
    /// visible `.model` card and the `key=` value `params[vi]` follows:
    /// HSPICE uses the name as the value and not the `key=` value (golden
    /// `rcl_kw_after_more`: `r6 6 0 rval r=5k` gives 2k, and `r8 8 0 rval
    /// 0.004 r=5k` 2k with TC1 0.004), and the words `tcs` after the name
    /// are TC1, TC2 (golden `rcl_tc_kw_after`). The same for a quoted
    /// expression `R1 a b 'rval' r=5k` (golden `rcl_quoted_sticky_local`,
    /// `rcl_quoted_more`). A keyword `tc1=` or `tc2=` wins over the word in
    /// its place, as in [`Netlist::shifted_tc`]. `value`: the text of the
    /// value; `caveats`: the caveats on it.
    fn name_before_key<'n>(
        &'n self,
        params: &'n [ParamDef],
        vi: usize,
        value: &str,
        tcs: &[&str],
        mut caveats: Vec<String>,
    ) -> Vec<Item<'n>> {
        let (tc1, tc2) = (self.syms.get("tc1"), self.syms.get("tc2"));
        let is_tc = |p: &ParamDef, s: Option<Sym>| s.is_some() && Some(p.name) == s;
        let kw = |s: Option<Sym>| params.iter().rfind(|p| is_tc(p, s) && self.after_eq(p));
        let v = &params[vi];
        let mut out: Vec<Item<'n>> = Vec::with_capacity(params.len() + 2);
        for (idx, p) in params.iter().enumerate() {
            if idx == vi {
                out.push(Item {
                    src: Src::Word(value.to_owned(), v.loc),
                    caveats: std::mem::take(&mut caveats),
                    ..Item::param(self, v)
                });
                for (s, name, w) in [(tc1, "tc1", tcs.first()), (tc2, "tc2", tcs.get(1))] {
                    match (kw(s), w) {
                        (Some(k), _) => out.push(Item::param(self, k)),
                        (None, Some(w)) => {
                            // a quoted word can go on on a `+` line
                            let w = one_line(w, false).into_owned();
                            out.push(Item::named(self, name, Src::Word(w, v.loc)));
                        }
                        (None, None) => {}
                    }
                }
            } else if !is_tc(p, tc1) && !is_tc(p, tc2) {
                out.push(Item::param(self, p));
            }
        }
        out
    }

    /// The items of `R1 a 0 rval 0.004 1e-5`, where `rval` names no visible
    /// `.model` card (stored as model `rval`, value `0.004`, `tc1` `1e-5`):
    /// the value `rval`, then TC1 and TC2 from the numbers after it (golden
    /// `res_tc_positional`, `rcl_param_tc`). A third number has no place.
    /// A keyword `tc1=` or `tc2=` wins over the number in its place (the
    /// last value wins, golden `elem_dup_kw`; the lowering keeps only the
    /// keyword when it replaced the stored `tc1`, so the number after the
    /// value is read again from the line).
    fn shifted_tc<'n>(&'n self, params: &'n [ParamDef], vi: usize, mname: &str) -> Vec<Item<'n>> {
        let (tc1, tc2) = (self.syms.get("tc1"), self.syms.get("tc2"));
        let v = &params[vi];
        let positional = |p: &ParamDef| !self.after_eq(p);
        let is_tc = |p: &ParamDef, s: Option<Sym>| s.is_some() && Some(p.name) == s;
        let kw = |s: Option<Sym>| params.iter().rfind(|p| is_tc(p, s) && !positional(p));
        let (kw1, kw2) = (kw(tc1), kw(tc2));
        // the number right after the value: the stored positional `tc1`,
        // else the token after the value on the line
        let second = params.iter().find(|p| is_tc(p, tc1) && positional(p));
        let second_word = match second {
            Some(_) => None,
            None => self
                .word_after(v)
                .filter(|w| {
                    let unsigned = w.strip_prefix(['-', '+']).unwrap_or(w);
                    matches!(netlist_expr::parse(unsigned), Ok(Expr::Num(_)))
                })
                .map(str::to_owned),
        };
        let third = params.iter().find(|p| is_tc(p, tc2) && positional(p));
        let vtext = param_text(self, v);
        let mut why = format!(
            "`{mname}` names no visible `.model` card, so it is read as the value and `{vtext}` as TC1, as in `R1 1 0 1k 0.004` (golden `res_tc_positional`, `cl_tc_positional`; with a parameter name as the value, golden `rcl_param_tc`, `rcl_name_first_cl`)"
        );
        if let Some(p) = third {
            why.push_str(&format!(
                ". The third number after the value (`{}`) has no place and is not used",
                param_text(self, p)
            ));
        }
        let mut out: Vec<Item<'n>> = Vec::with_capacity(params.len() + 1);
        for (idx, p) in params.iter().enumerate() {
            if idx == vi {
                out.push(Item {
                    src: Src::Word(mname.to_owned(), v.loc),
                    caveats: vec![why.clone()],
                    ..Item::param(self, v)
                });
                out.push(match kw1 {
                    Some(k) => Item::param(self, k),
                    None => Item::named(self, "tc1", Src::Param(v)),
                });
                match (kw2, second, &second_word) {
                    (Some(k), ..) => out.push(Item::param(self, k)),
                    (None, Some(p), _) => out.push(Item::named(self, "tc2", Src::Param(p))),
                    (None, None, Some(w)) => {
                        out.push(Item::named(self, "tc2", Src::Word(w.clone(), v.loc)));
                    }
                    (None, None, None) => {}
                }
            } else if !is_tc(p, tc1) && !is_tc(p, tc2) {
                out.push(Item::param(self, p));
            }
        }
        out
    }
}

impl Resolver<'_> {
    /// The caveat of the override `p` of `n` on the `X` line that makes
    /// level `level`, when another definition wins there: under GLOBAL a
    /// definition above the new instance beats the override (golden
    /// `parhier_global`, `nested_global`), so the instance does not use
    /// the override's value. The caveats of the value that the instance
    /// uses follow (a sweep or an `.alter` change of the winning top-level
    /// `.param`).
    fn shadowed(&mut self, level: usize, n: Sym, p: &ParamDef) -> Vec<String> {
        let Some(c) = self.find(level, n) else {
            return Vec::new();
        };
        if std::ptr::eq(c.p, p) {
            return Vec::new();
        }
        let net = self.net;
        let name = net.syms.name(n);
        let what = match c.kind {
            StepKind::GlobalParam => "the top-level `.param`".to_owned(),
            StepKind::LocalParam => format!("the body `.param` of `{}`", self.scopes[c.level].path),
            StepKind::SubcktDefault => {
                format!("the `.subckt` default of `{}`", self.scopes[c.level].path)
            }
            _ => format!("the `X`-line override of `{}`", self.scopes[c.level].path),
        };
        let val = self.resolve(level, n);
        let used = match &val.value {
            Ok(v) => format!("the instance uses {name} = {}", fmt_num(*v)),
            Err(why) => format!("the instance's `{name}` has no value ({why})"),
        };
        let mut out = vec![format!(
            "HSPICE does not use this override: under parhier=global {what} `{name}={}` at {} wins over it, so {used} (golden `parhier_global`, `nested_global`; resolve_param on `{}` gives the chain)",
            param_text(net, c.p),
            loc_str(net, c.p.loc),
            self.scopes[level].path
        )];
        out.extend(self.node_caveats(val.node));
        out
    }

    /// The caveats of the node `id` and of the nodes it depends on, as
    /// [`Resolver::answer`] lists them, without the chain; the walk passes
    /// over the nodes with no caveat below them.
    fn node_caveats(&self, id: Option<u32>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut known: HashSet<&str> = HashSet::new();
        let mut seen: HashSet<u32> = HashSet::new();
        let mut todo: Vec<u32> = id.into_iter().collect();
        while let Some(id) = todo.pop() {
            if self.nodes[id as usize].quiet || !seen.insert(id) {
                continue;
            }
            let node = &self.nodes[id as usize];
            for c in &node.caveats {
                if known.insert(c) {
                    out.push(c.clone());
                }
            }
            todo.extend(node.deps.iter().rev());
        }
        out
    }

    /// The caveats of the `X` lines that make levels `1..=upto` and whose
    /// `m` is an expression with a value of zero or less: HSPICE rejects
    /// the deck (golden `m_expr_zero`). `m` is evaluated in the scope of
    /// the line, not in the new instance (golden `ovr_scope_more_local`).
    /// A number as written is checked by `path_caveats`.
    fn path_m_caveats(&mut self, upto: usize) -> Vec<String> {
        let net = self.net;
        let mut out = Vec::new();
        let Some(m) = self.m else { return out };
        for j in 1..=upto {
            let Some((holder, i)) = self.scopes[j].x else {
                continue;
            };
            let d = &net.defs[holder.idx()];
            let e = &d.elements[i];
            let Some(p) = d.element_params(e).iter().rfind(|p| p.name == m) else {
                continue;
            };
            if p.num.is_some() || net.m_not_positive(d, e, m).is_some() {
                continue;
            }
            if let (Ok(v), _) = self.eval_param(p, j - 1, true, Text::Body)
                && v <= 0.0
            {
                out.push(format!(
                    "`{}` has `m={}` = {v}: HSPICE rejects a multiplier of zero or less on an instance path (golden `m_expr_zero`, `hier_m_zero`)",
                    self.scopes[j].path,
                    param_text(net, p)
                ));
            }
        }
        out
    }

    /// The parameter names defined on the path up to level `k` that are
    /// close to `wanted`.
    fn visible_names(&self, k: usize, wanted: &str) -> Vec<String> {
        let net = self.net;
        let mut names: Vec<&str> = vec!["temper"];
        for s in &self.scopes[..=k] {
            if let Some((holder, i)) = s.x {
                let d = &net.defs[holder.idx()];
                names.extend(
                    d.element_params(&d.elements[i])
                        .iter()
                        .filter(|p| Some(p.name) != self.m)
                        .map(|p| net.syms.name(p.name)),
                );
            }
            let d = &net.defs[s.def.idx()];
            names.extend(
                d.params
                    .iter()
                    .filter(|p| p.alter.is_none())
                    .map(|p| net.syms.name(p.name)),
            );
            names.extend(d.defaults.iter().map(|p| net.syms.name(p.name)));
        }
        suggest(wanted, names.into_iter().map(|n| (n, n.to_owned())))
    }
}

#[cfg(test)]
mod tests {
    use crate::{LoadOptions, Netlist};

    /// Loads `text` as the deck of test `name` (in a temporary directory,
    /// removed after the load).
    fn load_text(name: &str, text: &str) -> Netlist {
        let dir = std::env::temp_dir().join(format!(
            "netlist-query-params-{}-{name}",
            std::process::id()
        ));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let main = dir.join("main.sp");
        std::fs::write(&main, text).unwrap();
        let net = Netlist::load(&LoadOptions::new(main));
        _ = std::fs::remove_dir_all(&dir);
        net.unwrap()
    }

    /// The sum of `terms`, as a balanced tree of `+` (so that the
    /// expression stays shallow).
    fn sum(terms: &[String]) -> String {
        match terms {
            [] => "0".to_owned(),
            [one] => one.clone(),
            _ => {
                let (a, b) = terms.split_at(terms.len() / 2);
                format!("({}+{})", sum(a), sum(b))
            }
        }
    }

    /// The value and the work of `resolve_element(path)` on `text`.
    fn work(name: &str, text: &str, path: &str) -> (f64, u64) {
        let net = load_text(name, text);
        let (ep, work) = net.resolve_element_work(path).unwrap();
        let r = ep.params.iter().find(|p| p.name == "r").unwrap();
        (r.value.unwrap_or_else(|| panic!("{name}: {r:?}")), work)
    }

    /// `n` top-level functions, each called once in one element (the
    /// review's `funcs_*`).
    fn funcs(n: usize) -> String {
        let mut s = String::from("* t\nv1 1 0 1\n");
        for i in 0..n {
            s.push_str(&format!(".param f{i}(x)='x*2'\n"));
        }
        let calls: Vec<String> = (0..n).map(|i| format!("f{i}(1)")).collect();
        s.push_str(&format!("r1 1 0 '{}'\n.end\n", sum(&calls)));
        s
    }

    /// `n` top-level functions, and `f0` called `n` times with different
    /// arguments (the review's `funcs_same_*`).
    fn funcs_same(n: usize) -> String {
        let mut s = String::from("* t\nv1 1 0 1\n");
        for i in 0..n {
            s.push_str(&format!(".param f{i}(x)='x*2'\n"));
        }
        let calls: Vec<String> = (0..n).map(|i| format!("f0({i})")).collect();
        s.push_str(&format!("r1 1 0 '{}'\n.end\n", sum(&calls)));
        s
    }

    /// Under LOCAL, a top-level `f`, `d` unused subckts that define `f`,
    /// and `cf` (which defines `f`) holding `leaf`, whose element calls
    /// `f(1)` `c` times (the review's `adv_*`), or whose `c` body `.param`
    /// lines each call `f` (the review's `advmem_*`, `body`).
    fn adv(d: usize, c: usize, body: bool) -> String {
        let mut s = String::from("* t\n.option parhier=local\n.param f(x)='x*2'\nv1 1 0 1\n");
        for i in 0..d {
            s.push_str(&format!(".subckt u{i} a b\n.param f(x)='x*5'\n.ends\n"));
        }
        s.push_str(".subckt leaf a b\n");
        let terms: Vec<String> = if body {
            for i in 0..c {
                s.push_str(&format!(".param p{i}='f({i})'\n"));
            }
            (0..c).map(|i| format!("p{i}")).collect()
        } else {
            vec!["f(1)".to_owned(); c]
        };
        s.push_str(&format!(
            "r1 a b '{}'\n.ends\n.subckt cf a b\n.param f(x)='x*3'\nxl a b leaf\n.ends\nx1 1 0 cf\n.end\n",
            sum(&terms)
        ));
        s
    }

    /// One `resolve_element` call costs work linear in the number of
    /// `.param` functions, calls and definitions (plan Task 8 close-out):
    /// doubling the deck at most about doubles the work, where a scan of
    /// all functions at each call, or a copy of the list of the subckts
    /// that define a function at each call, would make it 4 times as much.
    #[test]
    fn work_is_linear() {
        for (name, shape) in shapes() {
            let mut works = Vec::new();
            for n in [1000, 2000] {
                let (text, path, want) = shape(n);
                let (v, w) = work(&format!("{name}_{n}"), &text, path);
                assert_eq!(v, want, "{name} {n}");
                works.push(w);
            }
            let ratio = works[1] as f64 / works[0] as f64;
            assert!(ratio < 2.5, "{name}: work {works:?} (ratio {ratio})");
        }
    }

    type Shape = fn(usize) -> (String, &'static str, f64);

    /// The decks of the cost tests: a name, and the deck of size `n` with
    /// the path to resolve and its value.
    fn shapes() -> [(&'static str, Shape); 4] {
        [
            ("funcs", |n| (funcs(n), "r1", 2.0 * n as f64)),
            ("funcs_same", |n| {
                let want = (0..n).map(|i| 2.0 * i as f64).sum();
                (funcs_same(n), "r1", want)
            }),
            ("adv", |n| (adv(n, n, false), "x1.xl.r1", 3.0 * n as f64)),
            ("advmem", |n| {
                let want = (0..n).map(|i| 3.0 * i as f64).sum();
                (adv(n, n, true), "x1.xl.r1", want)
            }),
        ]
    }

    /// The wall-clock companion of `work_is_linear`, for the costs that the
    /// work counter does not count (building, hashing and copying memo
    /// keys): 4 times the deck takes at most about 4 times as long, where
    /// a memo key that holds the list of the subckts that define a function
    /// would make it 16 times as long.
    #[test]
    #[ignore = "timing: run with --release -- --ignored"]
    fn time_is_linear() {
        for (name, shape) in shapes() {
            let mut times = Vec::new();
            for n in [4000, 16000] {
                let (text, path, want) = shape(n);
                let net = load_text(&format!("time_{name}_{n}"), &text);
                let mut best = f64::MAX;
                for _ in 0..3 {
                    let t = std::time::Instant::now();
                    let ep = net.resolve_element(path).unwrap();
                    best = best.min(t.elapsed().as_secs_f64());
                    let r = ep.params.iter().find(|p| p.name == "r").unwrap();
                    assert_eq!(r.value, Some(want), "{name} {n}");
                }
                times.push(best);
            }
            let ratio = times[1] / times[0];
            assert!(ratio < 8.0, "{name}: times {times:?} s (ratio {ratio})");
        }
    }
}
