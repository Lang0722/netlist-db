//! Values of HSPICE expressions and parameters.
//!
//! Expressions use HSPICE's syntax: numbers with scale factors (`2.5u`),
//! parameters, `+ - * /`, `**` and `^` (power), comparisons, `&&`, `||`, `!`,
//! `cond ? a : b`, built-in functions (`sqrt`, `exp`, `log`, `min`, `max`,
//! `pow`, `pwr`, ...) and functions defined with `.param f(x)='...'`.
//! Statistical functions (`agauss`, `gauss`, `aunif`, `unif`, `limit`) give
//! their nominal value, as in a nominal (not Monte Carlo) simulation.
//!
//! [`Scope`] holds the parameters of one level of the hierarchy, and finds a
//! parameter defined at several levels with HSPICE's rules (measured on
//! HSPICE P-2019.06 with `scripts/hspice_kit`):
//! - At one level, a value passed on the instance line hides the level's
//!   `.param` statements, which hide its `.subckt` defaults.
//! - `.option parhier=global` (the default) takes the level closest to the
//!   top that binds the name, also over a value passed below it;
//!   `parhier=local` takes the level closest to the reader.
//! - The text that wins is evaluated at the level that holds it. So the text
//!   of a value passed is evaluated in the new instance. The values passed
//!   are computed first, in the order of the line.
//! - A text that reads its own name reads that name at the level above
//!   (`w='w*k'`: `w` from above, `k` as usual).
//! - A value passed whose text leads back to it through other names
//!   (`a='c*k'` with `.param c='a*3'`) is evaluated, as a whole, at the level
//!   above. Any other loop is an error. (HSPICE runs a loop of two or more
//!   top-level `.param` lines, and it reads 0 where the loop closes. It stops at
//!   the other loops.)
//! - A chain of parameters can have any length, in any order of the file (decks
//!   `c_*`, `r_*`, `params50k`).
//! - HSPICE reads a name at most [`HSPICE_READ_REACH`] levels above the reader.
//!   This looks like a limit of HSPICE and not a rule, so the value here does not
//!   follow it. A read past it gives a [`FarRead`] (see [`Scope::far_reads`]).
//! - The same order chooses among functions defined with `.param f(x)=...`.
//!   The names and the calls in a function's body are resolved at the level
//!   that holds the function, not where it is called.
//! - A level's `.if` blocks are evaluated for each instance, in the order of the
//!   file. A condition sees the level's plain `.param` statements (also later
//!   ones), the values passed, and the active branches of earlier blocks. The
//!   statements of the active branch take their place in the file, so a later
//!   plain `.param` wins over a branch, and a branch over an earlier one. A value
//!   passed hides them all. (See [`Scope::activate`].)
//!
//! Comparisons and truth, as measured (decks `cmp_*`, `eq_edge`, `tr_*`, `xo_*`):
//! - Two values within a relative 1e-13 are equal, for all six comparisons.
//!   There is no absolute margin: `1e-300==0` is false.
//! - A value is true from an absolute 1e-28. `!x` is true only for 0.
//! - The condition of an `.if` line differs, also in the parameters it reads:
//!   `<` and `>` compare exactly, and any value that is not 0 is true.
//! - `x/0` is `x*1e28`.
//!
//! Functions and operators at their limits, as measured (deck lists `e*`, `x*`):
//! - The argument of `exp` stops at 80 at both ends. `sinh` and `cosh` are computed
//!   from that `exp`.
//! - `log`, `log10` and `db` read `|x|` from 1e-28 on, and give the negative of the
//!   result for `x<0`.
//! - `asin` and `acos` out of range give 0. `pow(0,y)` and `pwr(0,y)` give 0.
//! - `a--b` and `a++b` are an operator and the sign of `b`. Three signs stop.
//!
//! ```
//! use netlist_db::eval::{ParHier, Scope};
//! let mut top = Scope::new(ParHier::Global);
//! top.set("wn", Ok(1e-6));
//! assert_eq!(top.eval_str("2*wn + 100n"), Ok(2.1e-6));
//! ```

use crate::{
    AST, CondBlock,
    ast::{KeyValue, ParamFunc, Value},
    parser::is_raw_byte,
};
use core::fmt;
use netlist_expr::near;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    f64::consts::PI,
    rc::Rc,
    sync::Arc,
};

mod expr;
use expr::{Expr, Op, Parser, builtin, power};

/// `name` in lowercase, with no copy when it already is
#[inline]
pub(crate) fn lower(name: &str) -> Cow<'_, str> {
    if name.bytes().any(|c| c.is_ascii_uppercase()) {
        Cow::Owned(name.to_ascii_lowercase())
    } else {
        Cow::Borrowed(name)
    }
}

/// Why a value could not be computed
#[derive(Debug, Clone, PartialEq)]
pub enum EvalError {
    /// A parameter that is not defined
    Undefined(String),
    /// A function that is not defined
    UnknownFunction(String),
    /// A function called with the wrong number of arguments
    Arguments {
        function: String,
        expected: &'static str,
        found: usize,
    },
    /// Not a valid expression
    Syntax { expr: String, message: String },
    /// A parameter whose value depends on itself
    Recursive(String),
    /// A value only known during a simulation: `v(node)`, `i(elem)`, `time`, ...
    NotStatic(String),
    /// A result that is not a finite number (a division by zero, ...)
    NotFinite(String),
    /// A parameter out of reach of the stack: a long chain of others leads to it.
    /// The number is the height of its level: how many levels are above it.
    /// [`Scope::settle`] resolves this, so it stays inside this module.
    TooDeep(String, usize),
    /// An evaluation that computes more than [`MAX_CALLS`] different calls of
    /// functions: calls that each call the next ones with new values take
    /// exponential time. The name is the function of the last call.
    TooManyCalls(String),
}

/// How many parameters one evaluation can follow in a chain (`a='b'`, `b='c'`,
/// ...) before it starts again from the far name, and how deeply an expression
/// may nest
pub const MAX_DEPTH: usize = 100;

/// How much stack (in bytes) one evaluation uses before it starts again from the
/// far name, also across levels. A chain that crosses levels is longer than
/// [`MAX_DEPTH`] counts.
const STACK_BUDGET: usize = 512 * 1024;

/// How many different calls of functions defined with `.param f(x)` one evaluation
/// can compute
const MAX_CALLS: usize = if cfg!(test) { 1 << 12 } else { 1 << 22 };

/// In an expression, a value is true from this absolute size
const TRUE_FROM: f64 = 1e-28;

/// What a division by zero multiplies by
const DIVIDE_BY_ZERO: f64 = 1e28;

/// A logarithm reads a smaller size as this one
const LOG_FROM: f64 = 1e-28;
/// `exp`, `sinh` and `cosh` read their argument up to this size, as measured
const EXP_LIMIT: f64 = 80.0;

/// The results of calls of functions: see `Scope::calls`
type Calls = HashMap<(usize, Vec<u64>), Result<f64, EvalError>>;

/// Where the stack is now
fn stack_address() -> usize {
    let probe = 0u8;
    core::ptr::from_ref(&probe) as usize
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undefined(name) => write!(f, "undefined parameter `{name}`"),
            Self::UnknownFunction(name) => write!(f, "unknown function `{name}`"),
            Self::Arguments {
                function,
                expected,
                found,
            } => write!(
                f,
                "`{function}` takes {expected} argument(s), but {found} were given"
            ),
            Self::Syntax { expr, message } => write!(f, "{message} in `{expr}`"),
            Self::Recursive(name) => write!(f, "parameter `{name}` depends on itself"),
            Self::NotStatic(what) => write!(f, "`{what}` is only known during a simulation"),
            Self::NotFinite(expr) => write!(f, "`{expr}` is not a finite number"),
            Self::TooDeep(name, _) => write!(
                f,
                "parameter `{name}` depends on a chain of more than {MAX_DEPTH} others"
            ),
            Self::TooManyCalls(name) => write!(
                f,
                "too many different calls of functions (more than {MAX_CALLS}), the last \
                 one of `{name}`"
            ),
        }
    }
}

impl std::error::Error for EvalError {}

/// Which level is used for a parameter bound at several levels of the
/// hierarchy (`.option parhier`). A level binds a name with a value passed on
/// its instance line (`X1 a b inv w=2u`), a `.param` statement or a `.subckt`
/// default value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParHier {
    /// The level closest to the top: e.g. a top-level `.param w` wins over a
    /// `.subckt` default and over a value passed on an instance line
    /// (HSPICE's default)
    #[default]
    Global,
    /// The level closest to the element
    Local,
}

/// How a parameter is bound at a level (see [`Scope::binding`]): the value it
/// has there (the bits of a number, or the error that computing it gave), if
/// this level or one above binds it.
pub(crate) type Binding = Option<Result<u64, String>>;

#[derive(Debug, Clone)]
enum Def {
    Num(f64),
    Expr(String),
    Err(EvalError),
}

impl From<Result<f64, EvalError>> for Def {
    fn from(value: Result<f64, EvalError>) -> Self {
        match value {
            Ok(n) => Def::Num(n),
            Err(e) => Def::Err(e),
        }
    }
}

impl From<&Value<'_>> for Def {
    fn from(value: &Value<'_>) -> Self {
        match value {
            Value::Num(n) => Def::Num(*n),
            Value::Expr(e) => Def::Expr(e.to_string()),
        }
    }
}

/// How many levels above the reader HSPICE finds a parameter or a function, in both
/// `parhier` modes (decks `*_read_*`, `*_mid_*`, `*_fun_*`, `g_top_*`, `g_self_*`)
pub const HSPICE_READ_REACH: usize = 31;

/// A read of a definition past [`HSPICE_READ_REACH`]. HSPICE does not use that
/// definition: it stops, or under `parhier=global` it takes a nearer value with no
/// message. So the value here can differ from the value that HSPICE simulates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FarRead {
    /// the parameter or function, in lowercase
    pub name: String,
    /// how many levels the definition is above the reader, at most
    pub levels: usize,
}

impl fmt::Display for FarRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is defined {} levels above a line that reads it. HSPICE reads at most \
             {HSPICE_READ_REACH} levels above: it stops, or it takes a nearer value with no \
             message. So HSPICE can use a different value here",
            self.name, self.levels
        )
    }
}

/// A function defined with `.param f(a, b) = 'expr'`
#[derive(Debug, Clone)]
struct Func {
    args: Vec<String>,
    body: Def,
}

/// The definitions of a level, by lowercase name
type Defs = HashMap<Rc<str>, Def>;
/// The functions of a level, by lowercase name
type Funcs = HashMap<String, Func>;
/// What the instances of a definition share
type LevelMaps = (Rc<Defs>, Rc<Funcs>);

/// The parameters of one level of the hierarchy: the top level, or one
/// subckt instance (its `.subckt` defaults, its local `.param` statements and
/// the values passed on the instance line).
#[derive(Debug)]
pub struct Scope<'p> {
    parent: Option<&'p Scope<'p>>,
    parhier: ParHier,
    /// `.param` statements and `.subckt` defaults. The instances of a definition
    /// share these (see [`Scope::define_shared`]).
    defs: Rc<Defs>,
    /// values passed on the instance line: they hide `defs`
    passed: Defs,
    funcs: Rc<Funcs>,
    /// what [`Scope::define_shared`] keeps for each definition (kept by the top
    /// level for every level below it)
    shared: RefCell<HashMap<usize, LevelMaps>>,
    cache: RefCell<HashMap<Rc<str>, Result<f64, EvalError>>>,
    /// parameters being evaluated, to find the ones that depend on themselves
    active: RefCell<Vec<Rc<str>>>,
    /// parsed expressions (kept by the top level for every level below it)
    parsed: RefCell<HashMap<String, Arc<Expr>>>,
    /// a condition of an `.if` line is being evaluated: see the module's
    /// documentation
    in_condition: Cell<bool>,
    /// what `cache` holds, for a condition
    condition_cache: RefCell<HashMap<Rc<str>, Result<f64, EvalError>>>,
    /// the reads past [`HSPICE_READ_REACH`] (kept by the top level for every level
    /// below it)
    far_reads: RefCell<Vec<FarRead>>,
    /// where the stack was when the outermost evaluation started; 0: no evaluation
    /// is under way (kept by the top level for every level below it)
    stack_base: Cell<usize>,
    /// the results of the calls of functions in the outermost evaluation, by
    /// function (address) and values of the arguments (kept by the top level).
    /// The values of the parameters do not change in an evaluation, so that a call
    /// with the same values gives the same result. HSPICE reads calls that fan out
    /// to 2^26 calls (decks `fn_fan_*`).
    calls: RefCell<Calls>,
}

impl Scope<'static> {
    /// An empty top level
    pub fn new(parhier: ParHier) -> Self {
        Self::empty(None, parhier)
    }
}

impl<'p> Scope<'p> {
    /// An empty level below this one (a subckt instance)
    pub fn child(&'p self) -> Scope<'p> {
        Scope::empty(Some(self), self.parhier)
    }

    fn empty(parent: Option<&'p Scope<'p>>, parhier: ParHier) -> Self {
        Scope {
            parent,
            parhier,
            defs: Rc::default(),
            passed: Defs::default(),
            funcs: Rc::default(),
            shared: RefCell::default(),
            cache: RefCell::default(),
            active: RefCell::default(),
            parsed: RefCell::default(),
            in_condition: Cell::new(false),
            condition_cache: RefCell::default(),
            far_reads: RefCell::default(),
            stack_base: Cell::new(0),
            calls: RefCell::default(),
        }
    }

    fn clear_values(&mut self) {
        self.cache.get_mut().clear();
        self.condition_cache.get_mut().clear();
    }

    pub fn parhier(&self) -> ParHier {
        self.parhier
    }

    /// Define a parameter at this level, from an assignment of a `.param` or
    /// `.subckt` line: `name=value`, or a function `name(a, b)='expr'`. A
    /// later definition of the same name replaces an earlier one.
    pub fn define(&mut self, kv: &KeyValue<'_>) {
        let body = Def::from(&kv.v);
        if let Some((name, args)) = function_key(kv.k) {
            Rc::make_mut(&mut self.funcs).insert(name, Func { args, body });
        } else {
            Rc::make_mut(&mut self.defs).insert(Rc::from(&*lower(kv.k)), body);
        }
        self.clear_values();
    }

    /// Define a function, as `.param f(x)='...'` does
    pub fn define_func(&mut self, f: &ParamFunc<'_>) {
        let args = f.args.iter().map(|a| a.to_ascii_lowercase()).collect();
        let body = Def::from(&Value::Expr(f.body));
        Rc::make_mut(&mut self.funcs).insert(f.name.to_ascii_lowercase(), Func { args, body });
        self.clear_values();
    }

    /// Define a parameter with a value (or the error that computing it gave),
    /// as a `.param` statement of this level would
    pub fn set(&mut self, name: &str, value: Result<f64, EvalError>) {
        Rc::make_mut(&mut self.defs).insert(Rc::from(&*lower(name)), value.into());
        self.clear_values();
    }

    /// Pass a value to this level, as the line of a subckt instance does
    /// (`X1 a b inv w='2*p'`). It is evaluated at this level (see the module's
    /// documentation).
    pub fn pass(&mut self, kv: &KeyValue<'_>) {
        self.passed.insert(Rc::from(&*lower(kv.k)), (&kv.v).into());
        self.clear_values();
    }

    /// Defines `params` and `funcs` at a new level, as [`Scope::define`] and
    /// [`Scope::define_func`] do. `id` names the definition that holds them. All its
    /// instances share one copy, so that an instance costs little. A later
    /// definition at one level makes a copy for that level.
    pub(crate) fn define_shared<'k, 'v: 'k>(
        &mut self,
        id: usize,
        params: impl Iterator<Item = &'k KeyValue<'v>>,
        funcs: &[ParamFunc<'_>],
    ) {
        let root = self.root();
        let known = root.shared.borrow().get(&id).cloned();
        let (defs, funcs) = known.unwrap_or_else(|| {
            let mut level = Scope::new(root.parhier);
            for kv in params {
                level.define(kv);
            }
            for f in funcs {
                level.define_func(f);
            }
            let maps = (level.defs, level.funcs);
            root.shared.borrow_mut().insert(id, maps.clone());
            maps
        });
        self.defs = defs;
        self.funcs = funcs;
    }

    /// Defines the plain `.param` statements of a level, from a list and its
    /// function definitions
    fn define_all(&mut self, params: &[KeyValue<'_>], funcs: &[ParamFunc<'_>]) {
        for kv in params {
            self.define(kv);
        }
        for f in funcs {
            self.define_func(f);
        }
    }

    /// Finds the active branch of each `.if` block of `ast`, whose plain
    /// statements this level has defined, and defines what the branches set.
    /// The branches are added to `active` in netlist order. A condition that
    /// cannot be computed leaves its block with no active branch: HSPICE stops
    /// there, and `active.errors` holds it.
    pub(crate) fn activate<'a, 's>(&mut self, ast: &'a AST<'s>, active: &mut Active<'a, 's>) {
        for block in &ast.cond {
            for branch in &block.branches {
                let holds = match branch.cond {
                    Some(text) => self.condition(text),
                    None => Ok(true),
                };
                match holds {
                    Ok(false) => continue,
                    Ok(true) => {
                        let inner = &branch.ast;
                        active.branches.push(inner);
                        self.define_all(&inner.param, &inner.param_func);
                        self.activate(inner, active);
                        let sets = !inner.param.is_empty()
                            || !inner.param_func.is_empty()
                            || !inner.cond.is_empty();
                        if sets {
                            // (the plain statements after the block win)
                            self.define_all(
                                &ast.param[block.param_at..],
                                &ast.param_func[block.func_at..],
                            );
                        }
                    }
                    Err(error) => active.errors.push(CondError {
                        block,
                        cond: branch.cond.unwrap_or_default(),
                        error,
                    }),
                }
                break;
            }
        }
    }

    /// The truth of the condition of an `.if` or `.elseif` line. Any value that
    /// is not 0 is true. HSPICE stops at a condition that reads `temper` or
    /// `time`, also through a parameter (decks `tq_cond_temper`, `tc_*`).
    fn condition(&self, text: &str) -> Result<bool, EvalError> {
        let tree = self.parse(text)?;
        for level in self.levels() {
            level.in_condition.set(true);
        }
        let v = self.outermost(|| self.eval_tree(&tree, &[], 0));
        for level in self.levels() {
            level.in_condition.set(false);
        }
        let v = v?;
        if v.is_nan() {
            return Err(EvalError::NotFinite(text.to_owned()));
        }
        Ok(v != 0.0)
    }

    /// Whether this level (not one above) binds `key`, a lowercase name
    fn binds(&self, key: &str) -> bool {
        self.passed.contains_key(key) || self.defs.contains_key(key)
    }

    /// Whether `name` is defined at this level (not above)
    pub fn defines(&self, name: &str) -> bool {
        self.binds(&lower(name))
    }

    /// The value of the parameter `name`
    pub fn get(&self, name: &str) -> Result<f64, EvalError> {
        let key = lower(name);
        let key: &str = &key;
        match self.find_at(key) {
            Some((levels, scope)) => {
                self.note_read(key, levels);
                match scope.value_here(key) {
                    // a text that reads its own name: the name at the level above
                    Err(EvalError::Recursive(looped))
                        if looped == key
                            && self.active.borrow().last().is_some_and(|k| **k == *key) =>
                    {
                        match self.parent.and_then(|p| p.find_at(key)) {
                            Some((levels, above)) => {
                                self.note_read(key, levels + 1);
                                above.value_here(key)
                            }
                            None => Err(EvalError::Recursive(looped)),
                        }
                    }
                    result => result,
                }
            }
            None => match key {
                "pi" => Ok(PI),
                // simulation variables: they read 0 in a value, and a condition
                // cannot read them, as measured (decks `p_time_in_expr`, `sv_hertz*`)
                "time" | "hertz" if !self.in_condition.get() => Ok(0.0),
                "time" | "hertz" => Err(EvalError::NotStatic(key.to_owned())),
                _ => Err(EvalError::Undefined(name.to_owned())),
            },
        }
    }

    /// The value of `v`
    pub fn eval(&self, v: &Value<'_>) -> Result<f64, EvalError> {
        match v {
            Value::Num(n) => Ok(*n),
            Value::Expr(e) => self.eval_str(e),
        }
    }

    /// The value of the expression `expr`
    pub fn eval_str(&self, expr: &str) -> Result<f64, EvalError> {
        let tree = self.parse(expr)?;
        let v = self.outermost(|| self.eval_tree(&tree, &[], 0))?;
        if v.is_finite() {
            Ok(v)
        } else {
            Err(EvalError::NotFinite(expr.to_owned()))
        }
    }

    /// This level, then the levels above it
    fn levels(&self) -> impl Iterator<Item = &Scope<'p>> {
        let mut level = Some(self);
        core::iter::from_fn(move || {
            let s = level?;
            level = s.parent;
            Some(s)
        })
    }

    /// The level whose binding of `key` is used
    fn find(&self, key: &str) -> Option<&Scope<'p>> {
        self.find_at(key).map(|(_, scope)| scope)
    }

    /// The level whose binding of `key` is used, and how many levels it is above
    /// this one
    fn find_at(&self, key: &str) -> Option<(usize, &Scope<'p>)> {
        let mut levels = self.levels().enumerate().filter(|(_, s)| s.binds(key));
        match self.parhier {
            ParHier::Local => levels.next(),
            ParHier::Global => levels.last(),
        }
    }

    /// Keeps a read `levels` above this level if it is past [`HSPICE_READ_REACH`]
    fn note_read(&self, name: &str, levels: usize) {
        if levels <= HSPICE_READ_REACH {
            return;
        }
        let mut far = self.root().far_reads.borrow_mut();
        match far.iter_mut().find(|f| f.name == name) {
            Some(f) => f.levels = f.levels.max(levels),
            None => far.push(FarRead {
                name: name.to_owned(),
                levels,
            }),
        }
    }

    /// The reads past [`HSPICE_READ_REACH`] so far, by name
    pub fn far_reads(&self) -> Vec<FarRead> {
        let mut far = self.root().far_reads.borrow().clone();
        far.sort_by(|a, b| a.name.cmp(&b.name));
        far
    }

    /// How the parameter `name` is bound, seen from this level: what decides
    /// its value here, and in the levels below that do not bind it
    /// themselves (with `parhier=global`, in all those below). Two levels
    /// with the same bindings for the parameters that an expression reads
    /// give it the same value.
    pub(crate) fn binding(&self, name: &str) -> Binding {
        let key = lower(name);
        self.find(&key).map(|s| {
            s.value_here(&key)
                .map(f64::to_bits)
                .map_err(|e| e.to_string())
        })
    }

    /// The value of a definition, evaluated at this level
    fn value_of(&self, def: &Def) -> Result<f64, EvalError> {
        match def {
            Def::Num(n) => Ok(*n),
            Def::Expr(e) => self.eval_str(e),
            Def::Err(e) => Err(e.clone()),
        }
    }

    /// The value of `key`, which this level binds
    fn value_here(&self, key: &str) -> Result<f64, EvalError> {
        self.outermost(|| self.follow(key))
    }

    /// The result of `compute`. If no evaluation is under way, this one is the
    /// outermost: it starts the measures of the stack and of the calls, and it
    /// resolves a chain that is too long for the stack (see [`Scope::settle`]).
    fn outermost(&self, compute: impl Fn() -> Result<f64, EvalError>) -> Result<f64, EvalError> {
        let root = self.root();
        if root.stack_base.get() != 0 {
            return compute();
        }
        root.stack_base.set(stack_address());
        root.calls.borrow_mut().clear();
        let v = loop {
            match compute() {
                Err(EvalError::TooDeep(far, height)) => {
                    if let Err(looped) = self.settle(far, height) {
                        break Err(looped);
                    }
                }
                v => break v,
            }
        };
        root.stack_base.set(0);
        v
    }

    /// Computes the parameter `far` at the level of height `height`, and the ones
    /// it leads to. The first value comes with an empty stack, and the cache keeps
    /// it. So the next evaluation of `far` gets further. HSPICE reads a chain of
    /// any length (decks `c_*`, `r_*`). The error: a name that comes again in a
    /// chain too long for the stack, or a loop.
    fn settle(&self, far: String, height: usize) -> Result<(), EvalError> {
        let mut pending = vec![(far, height)];
        while let Some((name, height)) = pending.last() {
            let level = self
                .levels()
                .nth(self.height() - height)
                .expect("an evaluation reads the levels above it");
            match level.follow(name) {
                Err(EvalError::TooDeep(next, at)) => {
                    // (a name comes again only in a loop)
                    if pending.iter().any(|(n, h)| *n == next && *h == at) {
                        return Err(EvalError::Recursive(next));
                    }
                    pending.push((next, at));
                }
                // (the cache does not keep a loop, so this name would come again)
                Err(EvalError::Recursive(looped)) => return Err(EvalError::Recursive(looped)),
                _ => {
                    pending.pop();
                }
            }
        }
        Ok(())
    }

    /// How many levels are above this one
    fn height(&self) -> usize {
        self.levels().count() - 1
    }

    /// How much stack the outermost evaluation has used
    fn stack_used(&self) -> usize {
        stack_address().abs_diff(self.root().stack_base.get())
    }

    /// The value of `key`, with the stack as it is, or [`EvalError::TooDeep`]
    fn follow(&self, key: &str) -> Result<f64, EvalError> {
        if key == "temper" && self.in_condition.get() {
            return Err(EvalError::NotStatic("temper".to_owned()));
        }
        let cache = if self.in_condition.get() {
            &self.condition_cache
        } else {
            &self.cache
        };
        if let Some(v) = cache.borrow().get(key) {
            return v.clone();
        }
        if self.active.borrow().iter().any(|k| **k == *key) {
            return Err(EvalError::Recursive(key.to_owned()));
        }
        if self.active.borrow().len() >= MAX_DEPTH || self.stack_used() > STACK_BUDGET {
            return Err(EvalError::TooDeep(key.to_owned(), self.height()));
        }
        let passed = self.passed.get_key_value(key);
        let (name, def) = passed
            .or_else(|| self.defs.get_key_value(key))
            .expect("this level binds the name");
        self.active.borrow_mut().push(name.clone());
        let mut v = self.value_of(def);
        self.active.borrow_mut().pop();
        // a value passed whose text leads back to it: as a whole, above
        if matches!(&v, Err(EvalError::Recursive(looped)) if looped == key)
            && let (Some((_, def)), Some(parent)) = (passed, self.parent)
        {
            v = parent.value_of(def).map_err(|e| match e {
                EvalError::TooDeep(..) => e,
                _ => EvalError::Recursive(key.to_owned()),
            });
        }
        // (a cycle depends on the name where the evaluation started: read
        // later, this name may have a value. A chain gets further next time.)
        if !matches!(v, Err(EvalError::Recursive(_) | EvalError::TooDeep(..))) {
            cache.borrow_mut().insert(name.clone(), v.clone());
        }
        v
    }

    /// The top level
    fn root(&self) -> &Scope<'p> {
        let mut s = self;
        while let Some(parent) = s.parent {
            s = parent;
        }
        s
    }

    /// `expr`, parsed (once: the parsed expressions are kept)
    fn parse(&self, expr: &str) -> Result<Arc<Expr>, EvalError> {
        let root = self.root();
        if let Some(tree) = root.parsed.borrow().get(expr) {
            return Ok(tree.clone());
        }
        let tree = Arc::new(Parser::new(expr).parse()?);
        root.parsed
            .borrow_mut()
            .insert(expr.to_owned(), tree.clone());
        Ok(tree)
    }

    /// The function `name` that is used here, and the level that holds it
    fn func(&self, name: &str) -> Option<(&Scope<'p>, &Func)> {
        let mut levels = self
            .levels()
            .enumerate()
            .filter_map(|(at, s)| s.funcs.get(name).map(|f| (at, s, f)));
        let (at, holder, f) = match self.parhier {
            ParHier::Local => levels.next(),
            ParHier::Global => levels.last(),
        }?;
        self.note_read(name, at);
        Some((holder, f))
    }

    /// `locals`: the arguments of the user function being evaluated
    fn eval_tree(
        &self,
        e: &Expr,
        locals: &[(String, f64)],
        depth: usize,
    ) -> Result<f64, EvalError> {
        let eval = |e: &Expr| self.eval_tree(e, locals, depth);
        let exact = self.in_condition.get();
        let truth = |v: f64| {
            if exact {
                v != 0.0
            } else {
                v.abs() >= TRUE_FROM
            }
        };
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        Ok(match e {
            Expr::Num(n) => *n,
            Expr::Var(name) => match locals.iter().find(|(k, _)| k == name) {
                Some((_, v)) => *v,
                None => self.get(name)?,
            },
            Expr::Neg(a) => -eval(a)?,
            Expr::Not(a) => flag(eval(a)? == 0.0),
            Expr::Cond(c, a, b) => {
                if truth(eval(c)?) {
                    eval(a)?
                } else {
                    eval(b)?
                }
            }
            Expr::Chain(first, rest) => {
                let mut x = eval(first)?;
                for (op, next) in rest {
                    x = match op {
                        Op::And => flag(truth(x) && truth(eval(next)?)),
                        Op::Or => flag(truth(x) || truth(eval(next)?)),
                        op => {
                            let y = eval(next)?;
                            match op {
                                Op::Add => x + y,
                                Op::Sub => x - y,
                                Op::Mul => x * y,
                                Op::Div if y == 0.0 => x * DIVIDE_BY_ZERO,
                                Op::Div => x / y,
                                Op::Pow => power(x, y),
                                Op::Lt => flag(x < y && (exact || !near(x, y))),
                                Op::Le => flag(x < y || near(x, y)),
                                Op::Gt => flag(x > y && (exact || !near(x, y))),
                                Op::Ge => flag(x > y || near(x, y)),
                                Op::Eq => flag(near(x, y)),
                                Op::Ne => flag(!near(x, y)),
                                Op::And | Op::Or => unreachable!(),
                            }
                        }
                    };
                }
                x
            }
            Expr::Call(name, args) => {
                if is_simulation_function(name) {
                    return Err(EvalError::NotStatic(format!("{name}(...)")));
                }
                // statistical functions: the nominal value (first argument).
                // A `.param` function of the same name hides it, as measured in
                // HSPICE (golden `stat_user_func`; `l='unif(1u,3u)'` with
                // `.param unif(a,b)='(a+b)/2'` gives L=2u).
                if matches!(
                    name.as_str(),
                    "agauss" | "gauss" | "aunif" | "unif" | "limit"
                ) && self.func(name).is_none()
                {
                    return match args.first() {
                        Some(nominal) => eval(nominal),
                        None => Err(EvalError::Arguments {
                            function: name.clone(),
                            expected: "2 or more",
                            found: 0,
                        }),
                    };
                }
                if let Some((holder, f)) = self.func(name) {
                    if f.args.len() != args.len() {
                        return Err(EvalError::Arguments {
                            function: name.clone(),
                            expected: "as many",
                            found: args.len(),
                        });
                    }
                    // (twice the budget: a chain of parameters stops first, at
                    // a name that can be settled)
                    if depth > 64 || self.stack_used() > 2 * STACK_BUDGET {
                        return Err(EvalError::Recursive(name.clone()));
                    }
                    let values = args.iter().map(eval).collect::<Result<Vec<_>, _>>()?;
                    let key = (
                        core::ptr::from_ref(f) as usize,
                        values.iter().map(|v| v.to_bits()).collect(),
                    );
                    let root = self.root();
                    if let Some(v) = root.calls.borrow().get(&key) {
                        return v.clone();
                    }
                    if root.calls.borrow().len() >= MAX_CALLS {
                        return Err(EvalError::TooManyCalls(name.clone()));
                    }
                    let locals: Vec<_> = f.args.iter().cloned().zip(values).collect();
                    let v = match &f.body {
                        Def::Num(n) => Ok(*n),
                        Def::Expr(body) => {
                            // (its names and calls: where it is defined)
                            let tree = self.parse(body)?;
                            holder.eval_tree(&tree, &locals, depth + 1)
                        }
                        Def::Err(e) => Err(e.clone()),
                    };
                    // (a loop and the depth depend on the calls above this one)
                    if !matches!(v, Err(EvalError::Recursive(_) | EvalError::TooDeep(..))) {
                        root.calls.borrow_mut().insert(key, v.clone());
                    }
                    return v;
                }
                let values = args.iter().map(eval).collect::<Result<Vec<_>, _>>()?;
                builtin(name, &values)
                    .unwrap_or_else(|| Err(EvalError::UnknownFunction(name.clone())))?
            }
        })
    }
}

/// A condition of an `.if` or `.elseif` line that could not be computed
#[derive(Debug)]
pub(crate) struct CondError<'a, 's> {
    pub(crate) block: &'a CondBlock<'s>,
    pub(crate) cond: &'a str,
    pub(crate) error: EvalError,
}

/// What the `.if` blocks of one level give, for one instance of it
#[derive(Debug, Default)]
pub(crate) struct Active<'a, 's> {
    /// the active branches, in netlist order: a branch before those nested in it
    pub(crate) branches: Vec<&'a AST<'s>>,
    pub(crate) errors: Vec<CondError<'a, 's>>,
}

/// The items of a list of `ast`, and of its active branches `active`, in
/// netlist order. `items` chooses the list, `at` the place of a block in it.
pub(crate) fn in_order<'a, 's, T>(
    ast: &'a AST<'s>,
    active: &[&'a AST<'s>],
    items: &impl Fn(&'a AST<'s>) -> &'a [T],
    at: &impl Fn(&CondBlock<'s>) -> usize,
) -> Vec<&'a T> {
    fn walk<'a, 's, T>(
        ast: &'a AST<'s>,
        active: &[&'a AST<'s>],
        next: &mut usize,
        items: &impl Fn(&'a AST<'s>) -> &'a [T],
        at: &impl Fn(&CondBlock<'s>) -> usize,
        out: &mut Vec<&'a T>,
    ) {
        let list = items(ast);
        let mut from = 0;
        for block in &ast.cond {
            let to = at(block);
            out.extend(&list[from..to]);
            from = to;
            for branch in &block.branches {
                // (`active` is in the order of this walk)
                if active
                    .get(*next)
                    .is_some_and(|a| core::ptr::eq(*a, &branch.ast))
                {
                    *next += 1;
                    walk(&branch.ast, active, next, items, at, out);
                }
            }
        }
        out.extend(&list[from..]);
    }
    let mut out = Vec::new();
    walk(ast, active, &mut 0, items, at, &mut out);
    out
}

/// Whether `cmd`, a dot-command as written, is `.temp`
fn is_temp(cmd: &str) -> bool {
    crate::parser::canonical_cmd(&cmd.to_ascii_lowercase()) == "temp"
}

/// The first value of the text after a `.temp`: `75`, `=75`, `'50+25'`, `t`.
/// As measured on HSPICE P-2019.06-SP1-1, a comma also ends a value (`50,75`,
/// `50, 75`, `50,`), and a `$` right after a number starts a comment
/// (`50$room`). The query tool reads `.temp` with this function too.
pub fn first_temp(raw: &str) -> &str {
    let text = raw.trim_start().trim_start_matches('=').trim_start();
    match text.chars().next() {
        Some(q @ ('\'' | '"')) => {
            let close = text[1..].find(q).map_or(text.len(), |n| n + 2);
            &text[..close]
        }
        _ => {
            // (a `$` inside a name is a part of the name)
            let number = text.starts_with(|c: char| c.is_ascii_digit() || ".+-".contains(c));
            let end = |c: char| c.is_whitespace() || c == ',' || (number && c == '$');
            text.split(end).next().unwrap_or_default()
        }
    }
}

impl<'s> AST<'s> {
    /// The parameters of the top level: its `.param` statements and the active
    /// branches of its `.if` blocks, with the `.option parhier` rule. `temper`
    /// is the first `.temp` value, else the last `.option tnom` number, else 25.
    pub fn scope(&self) -> Scope<'static> {
        self.top_level().0
    }

    /// The parameters of the top level, and what its `.if` blocks give
    pub(crate) fn top_level(&self) -> (Scope<'static>, Active<'_, 's>) {
        let mut scope = Scope::new(ParHier::Global);
        scope.set("temper", Ok(25.0));
        scope.define_all(&self.param, &self.param_func);
        // (the top level has nothing above it: its conditions do not depend on
        // `parhier`)
        let mut active = Active::default();
        scope.activate(self, &mut active);
        let options = self.top_options(&active.branches);
        let last = |name: &str| {
            options
                .iter()
                .rev()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_ref())
        };
        // (`.option parhier` alone means `local`, as measured: deck `oph_flag`)
        let local = match last("parhier") {
            Some(None) => true,
            Some(Some(Value::Expr(e))) => e.eq_ignore_ascii_case("local"),
            _ => false,
        };
        scope.parhier = if local {
            ParHier::Local
        } else {
            ParHier::Global
        };
        let commands = in_order(self, &active.branches, &|a| &a.unknown, &|b| b.unknown_at);
        let temp = commands.iter().find(|u| is_temp(u.cmd));
        let tnom = options
            .iter()
            .rev()
            .find(|(k, v)| k.eq_ignore_ascii_case("tnom") && matches!(v, Some(Value::Num(_))));
        let temper = match (temp, tnom) {
            (Some(u), _) => scope.eval_str(first_temp(u.raw)).ok(),
            (None, Some((_, Some(Value::Num(t))))) => Some(*t),
            _ => None,
        };
        if let Some(temper) = temper {
            scope.set("temper", Ok(temper));
        }
        (scope, active)
    }

    /// The `.option` items of the top level and its active branches `active`,
    /// in netlist order
    pub(crate) fn top_options<'a>(
        &'a self,
        active: &[&'a AST<'s>],
    ) -> Vec<&'a (&'s str, Option<Value<'s>>)> {
        in_order(self, active, &|a| &a.option, &|b| b.option_at)
    }

    /// The value of the `.option` named `name` (the last one, if it is set
    /// several times)
    pub fn option_value(&self, name: &str) -> Option<&Value<'s>> {
        self.option
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.as_ref())
    }
}

/// The parameters that the expression `expr` reads, lowercase, each once.
/// `None` if it calls one of `functions`, functions defined with `.param
/// f(x)=...` (a function's body reads the parameters of the level that holds
/// it, which depends on the levels above). An expression that is not valid
/// reads nothing: it is an error wherever it is.
pub(crate) fn expr_names(expr: &str, functions: &HashSet<String>) -> Option<Vec<String>> {
    fn walk(e: &Expr, functions: &HashSet<String>, out: &mut Vec<String>) -> Option<()> {
        match e {
            Expr::Num(_) => {}
            Expr::Var(name) => {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
            Expr::Neg(a) | Expr::Not(a) => walk(a, functions, out)?,
            Expr::Chain(first, rest) => {
                walk(first, functions, out)?;
                for (_, next) in rest {
                    walk(next, functions, out)?;
                }
            }
            Expr::Cond(c, a, b) => {
                walk(c, functions, out)?;
                walk(a, functions, out)?;
                walk(b, functions, out)?;
            }
            Expr::Call(name, args) => {
                if functions.contains(name) {
                    return None;
                }
                for a in args {
                    walk(a, functions, out)?;
                }
            }
        }
        Some(())
    }
    let mut out = Vec::new();
    if let Ok(tree) = Parser::new(expr).parse() {
        walk(&tree, functions, &mut out)?;
    }
    Some(out)
}

/// The name of the function that the key of a `.param` assignment defines
/// (`f(a, b)`: `f`, lowercase), if it defines one
pub(crate) fn function_name(key: &str) -> Option<String> {
    function_key(key).map(|(name, _)| name)
}

/// `name(a, b)`: the function name and its arguments, lowercase
fn function_key(key: &str) -> Option<(String, Vec<String>)> {
    let (name, rest) = key.split_once('(')?;
    let args = rest.strip_suffix(')')?;
    let args = args
        .split(',')
        .map(|a| a.trim().to_ascii_lowercase())
        .filter(|a| !a.is_empty())
        .collect();
    Some((name.trim().to_ascii_lowercase(), args))
}

/// Functions of node voltages, currents and other simulation results
fn is_simulation_function(name: &str) -> bool {
    matches!(
        name,
        "v" | "i"
            | "vr"
            | "vi"
            | "vm"
            | "vp"
            | "vdb"
            | "ir"
            | "ii"
            | "im"
            | "ip"
            | "idb"
            | "par"
            | "str"
            | "lv"
            | "lx"
            | "deriv"
            | "integ"
            | "tdelay"
    ) || ((name.starts_with("lv") || name.starts_with("lx"))
        && name[2..].bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests;
