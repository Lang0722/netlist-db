//! Parameter resolution: the value of a parameter as seen from
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
    api::{
        ChainStep, DEFAULT_LIMIT, ElementParams, MAX_LIMIT, NameText, QueryError, Resolved,
        StepKind,
    },
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

mod resolver;
use resolver::{List, Scan};

mod queries;

mod cards;

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
pub(crate) const STATISTICAL: [&str; 5] = ["agauss", "gauss", "aunif", "unif", "limit"];

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
    /// The definitions being evaluated (definition, level, name, an `X`-line
    /// override), outermost first: one met again is a cycle.
    stack: Vec<(*const ParamDef, usize, Sym, bool)>,
    /// A cycle was found inside the evaluation in progress. A value that
    /// failed after a cycle depends on where the evaluation started, so it is
    /// not memoized.
    cycled: bool,
    /// The `X`-line override of the last cycle found that comes first on its
    /// line (definition, level). [`Resolver::resolve`] evaluates its text one
    /// level up.
    closed: Option<(*const ParamDef, usize)>,
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
    // a comma at the end gives no second value (deck `t_comma_end`)
    let line = line.trim().trim_start_matches('\'').trim();
    !line.trim_start_matches(',').trim().is_empty()
}

#[cfg(test)]
mod tests;
