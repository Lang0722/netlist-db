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
//!   above. Any other loop is an error.
//! - The same order chooses among functions defined with `.param f(x)=...`.
//!   The names and the calls in a function's body are resolved at the level
//!   that holds the function, not where it is called.
//!
//! ```
//! use netlist_db::eval::{ParHier, Scope};
//! let mut top = Scope::new(ParHier::Global);
//! top.set("wn", Ok(1e-6));
//! assert_eq!(top.eval_str("2*wn + 100n"), Ok(2.1e-6));
//! ```

use crate::{
    AST,
    ast::{KeyValue, Value},
};
use core::fmt;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    f64::consts::PI,
    sync::Arc,
};

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
    /// A parameter defined through a chain of more than [`MAX_DEPTH`] others
    TooDeep(String),
}

/// How many parameters a value may depend on in a chain (`a='b'`, `b='c'`,
/// ...), and how deeply an expression may nest
pub const MAX_DEPTH: usize = 100;

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
            Self::TooDeep(name) => write!(
                f,
                "parameter `{name}` depends on a chain of more than {MAX_DEPTH} others"
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

/// A function defined with `.param f(a, b) = 'expr'`
#[derive(Debug, Clone)]
struct Func {
    args: Vec<String>,
    body: Def,
}

/// The parameters of one level of the hierarchy: the top level, or one
/// subckt instance (its `.subckt` defaults, its local `.param` statements and
/// the values passed on the instance line).
#[derive(Debug)]
pub struct Scope<'p> {
    parent: Option<&'p Scope<'p>>,
    parhier: ParHier,
    /// `.param` statements and `.subckt` defaults
    defs: HashMap<String, Def>,
    /// values passed on the instance line: they hide `defs`
    passed: HashMap<String, Def>,
    funcs: HashMap<String, Func>,
    cache: RefCell<HashMap<String, Result<f64, EvalError>>>,
    /// parameters being evaluated, to find the ones that depend on themselves
    active: RefCell<Vec<String>>,
    /// parsed expressions (kept by the top level for every level below it)
    parsed: RefCell<HashMap<String, Arc<Expr>>>,
}

impl Scope<'static> {
    /// An empty top level
    pub fn new(parhier: ParHier) -> Self {
        Self {
            parent: None,
            parhier,
            defs: HashMap::new(),
            passed: HashMap::new(),
            funcs: HashMap::new(),
            cache: RefCell::new(HashMap::new()),
            active: RefCell::new(Vec::new()),
            parsed: RefCell::new(HashMap::new()),
        }
    }
}

impl<'p> Scope<'p> {
    /// An empty level below this one (a subckt instance)
    pub fn child(&'p self) -> Scope<'p> {
        Scope {
            parent: Some(self),
            parhier: self.parhier,
            defs: HashMap::new(),
            passed: HashMap::new(),
            funcs: HashMap::new(),
            cache: RefCell::new(HashMap::new()),
            active: RefCell::new(Vec::new()),
            parsed: RefCell::new(HashMap::new()),
        }
    }

    pub fn parhier(&self) -> ParHier {
        self.parhier
    }

    /// Define a parameter at this level, from an assignment of a `.param` or
    /// `.subckt` line: `name=value`, or a function `name(a, b)='expr'`. A
    /// later definition of the same name replaces an earlier one.
    pub fn define(&mut self, kv: &KeyValue<'_>) {
        let body = Def::from(&kv.v);
        if let Some((name, args)) = function_key(&kv.k) {
            self.funcs.insert(name, Func { args, body });
        } else {
            self.defs.insert(kv.k.to_ascii_lowercase(), body);
        }
        self.cache.get_mut().clear();
    }

    /// Define a parameter with a value (or the error that computing it gave),
    /// as a `.param` statement of this level would
    pub fn set(&mut self, name: &str, value: Result<f64, EvalError>) {
        self.defs.insert(name.to_ascii_lowercase(), value.into());
        self.cache.get_mut().clear();
    }

    /// Pass a value to this level, as the line of a subckt instance does
    /// (`X1 a b inv w='2*p'`). It is evaluated at this level (see the module's
    /// documentation).
    pub fn pass(&mut self, kv: &KeyValue<'_>) {
        self.passed
            .insert(kv.k.to_ascii_lowercase(), (&kv.v).into());
        self.cache.get_mut().clear();
    }

    /// Whether this level (not one above) binds `key`, a lowercase name
    fn binds(&self, key: &str) -> bool {
        self.passed.contains_key(key) || self.defs.contains_key(key)
    }

    /// Whether `name` is defined at this level (not above)
    pub fn defines(&self, name: &str) -> bool {
        self.binds(&name.to_ascii_lowercase())
    }

    /// The value of the parameter `name`
    pub fn get(&self, name: &str) -> Result<f64, EvalError> {
        let key = name.to_ascii_lowercase();
        match self.find(&key) {
            Some(scope) => match scope.value_here(&key) {
                // a text that reads its own name: the name at the level above
                Err(EvalError::Recursive(looped))
                    if looped == key && self.active.borrow().last() == Some(&key) =>
                {
                    match self.parent.and_then(|p| p.find(&key)) {
                        Some(above) => above.value_here(&key),
                        None => Err(EvalError::Recursive(looped)),
                    }
                }
                result => result,
            },
            None => match key.as_str() {
                "pi" => Ok(PI),
                // simulation variables
                "time" | "hertz" => Err(EvalError::NotStatic(key)),
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
        let v = self.eval_tree(&tree, &[], 0)?;
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
        let mut levels = self.levels().filter(|s| s.binds(key));
        match self.parhier {
            ParHier::Local => levels.next(),
            ParHier::Global => levels.last(),
        }
    }

    /// How the parameter `name` is bound, seen from this level: what decides
    /// its value here, and in the levels below that do not bind it
    /// themselves (with `parhier=global`, in all those below). Two levels
    /// with the same bindings for the parameters that an expression reads
    /// give it the same value.
    pub(crate) fn binding(&self, name: &str) -> Binding {
        let key = name.to_ascii_lowercase();
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
        if let Some(v) = self.cache.borrow().get(key) {
            return v.clone();
        }
        if self.active.borrow().iter().any(|k| k == key) {
            return Err(EvalError::Recursive(key.to_owned()));
        }
        if self.active.borrow().len() >= MAX_DEPTH {
            return Err(EvalError::TooDeep(key.to_owned()));
        }
        let passed = self.passed.get(key);
        self.active.borrow_mut().push(key.to_owned());
        let mut v = self.value_of(passed.unwrap_or_else(|| &self.defs[key]));
        self.active.borrow_mut().pop();
        // a value passed whose text leads back to it: as a whole, above
        if matches!(&v, Err(EvalError::Recursive(looped)) if looped == key)
            && let (Some(def), Some(parent)) = (passed, self.parent)
        {
            v = parent
                .value_of(def)
                .map_err(|_| EvalError::Recursive(key.to_owned()));
        }
        // (a cycle depends on the name where the evaluation started: read
        // later, this name may have a value)
        if !matches!(v, Err(EvalError::Recursive(_))) {
            self.cache.borrow_mut().insert(key.to_owned(), v.clone());
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
            .filter_map(|s| s.funcs.get(name).map(|f| (s, f)));
        match self.parhier {
            ParHier::Local => levels.next(),
            ParHier::Global => levels.last(),
        }
    }

    /// `locals`: the arguments of the user function being evaluated
    fn eval_tree(
        &self,
        e: &Expr,
        locals: &[(String, f64)],
        depth: usize,
    ) -> Result<f64, EvalError> {
        let eval = |e: &Expr| self.eval_tree(e, locals, depth);
        let truth = |v: f64| v != 0.0;
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        Ok(match e {
            Expr::Num(n) => *n,
            Expr::Var(name) => match locals.iter().find(|(k, _)| k == name) {
                Some((_, v)) => *v,
                None => self.get(name)?,
            },
            Expr::Neg(a) => -eval(a)?,
            Expr::Not(a) => flag(!truth(eval(a)?)),
            Expr::Cond(c, a, b) => {
                if truth(eval(c)?) {
                    eval(a)?
                } else {
                    eval(b)?
                }
            }
            Expr::Bin(Op::And, a, b) => flag(truth(eval(a)?) && truth(eval(b)?)),
            Expr::Bin(Op::Or, a, b) => flag(truth(eval(a)?) || truth(eval(b)?)),
            Expr::Bin(op, a, b) => {
                let (x, y) = (eval(a)?, eval(b)?);
                match op {
                    Op::Add => x + y,
                    Op::Sub => x - y,
                    Op::Mul => x * y,
                    Op::Div => x / y,
                    Op::Pow => power(x, y),
                    Op::Lt => flag(x < y),
                    Op::Le => flag(x <= y),
                    Op::Gt => flag(x > y),
                    Op::Ge => flag(x >= y),
                    Op::Eq => flag(x == y),
                    Op::Ne => flag(x != y),
                    Op::And | Op::Or => unreachable!(),
                }
            }
            Expr::Call(name, args) => {
                if is_simulation_function(name) {
                    return Err(EvalError::NotStatic(format!("{name}(...)")));
                }
                // statistical functions: the nominal value (first argument)
                if matches!(
                    name.as_str(),
                    "agauss" | "gauss" | "aunif" | "unif" | "limit"
                ) {
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
                    if depth > 64 {
                        return Err(EvalError::Recursive(name.clone()));
                    }
                    let values = args.iter().map(eval).collect::<Result<Vec<_>, _>>()?;
                    let locals: Vec<_> = f.args.iter().cloned().zip(values).collect();
                    return match &f.body {
                        Def::Num(n) => Ok(*n),
                        Def::Expr(body) => {
                            // (its names and calls: where it is defined)
                            let tree = self.parse(body)?;
                            holder.eval_tree(&tree, &locals, depth + 1)
                        }
                        Def::Err(e) => Err(e.clone()),
                    };
                }
                let values = args.iter().map(eval).collect::<Result<Vec<_>, _>>()?;
                builtin(name, &values)
                    .unwrap_or_else(|| Err(EvalError::UnknownFunction(name.clone())))?
            }
        })
    }
}

impl<'s> AST<'s> {
    /// The parameters of the top level: its `.param` statements, with the
    /// `.option parhier` rule. `temper` is the first `.temp` value (or 25).
    pub fn scope(&self) -> Scope<'static> {
        let parhier = match self.option_value("parhier") {
            Some(Value::Expr(e)) if e.eq_ignore_ascii_case("local") => ParHier::Local,
            _ => ParHier::Global,
        };
        let mut scope = Scope::new(parhier);
        let temper = self
            .unknwon
            .iter()
            .find(|u| {
                u.cmd.eq_ignore_ascii_case("temp") || u.cmd.eq_ignore_ascii_case("temperature")
            })
            .and_then(|u| crate::parser::number_prefix(u.raw.trim()))
            .map_or(25.0, |(t, _)| t);
        scope.set("temper", Ok(temper));
        for kv in &self.param {
            scope.define(kv);
        }
        scope
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
            Expr::Bin(_, a, b) => {
                walk(a, functions, out)?;
                walk(b, functions, out)?;
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

/// `x**y`, as HSPICE computes it: 0 for a zero base and any `y`, and the
/// integer part of `y` for a negative base
fn power(x: f64, y: f64) -> f64 {
    if x == 0.0 {
        0.0
    } else if x < 0.0 {
        x.powf(y.trunc())
    } else {
        x.powf(y)
    }
}

/// HSPICE's built-in functions; `None` if `name` is not one
fn builtin(name: &str, a: &[f64]) -> Option<Result<f64, EvalError>> {
    let arity = |expected: &'static str| EvalError::Arguments {
        function: name.to_owned(),
        expected,
        found: a.len(),
    };
    let one = |f: fn(f64) -> f64| match a {
        [x] => Ok(f(*x)),
        _ => Err(arity("1")),
    };
    let two = |f: fn(f64, f64) -> f64| match a {
        [x, y] => Ok(f(*x, *y)),
        _ => Err(arity("2")),
    };
    let sgn = |x: f64| {
        if x > 0.0 {
            1.0
        } else if x < 0.0 {
            -1.0
        } else {
            0.0
        }
    };
    Some(match name {
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        "asin" => one(f64::asin),
        "acos" => one(f64::acos),
        "atan" => one(f64::atan),
        "sinh" => one(f64::sinh),
        "cosh" => one(f64::cosh),
        "tanh" => one(f64::tanh),
        // (a negative argument gives the negative of the result for `|x|`)
        "sqrt" => one(|x| x.signum() * x.abs().sqrt()),
        "exp" => one(f64::exp),
        // natural logarithm, as in HSPICE
        "log" | "ln" => one(|x| x.signum() * x.abs().ln()),
        "log10" => one(|x| x.signum() * x.abs().log10()),
        "abs" => one(f64::abs),
        "floor" => one(f64::floor),
        "ceil" => one(f64::ceil),
        "int" => one(f64::trunc),
        "nint" => one(f64::round),
        "sgn" => one(sgn),
        "db" => one(|x| 20.0 * x.abs().log10()),
        // `pow`: the power with the integer part of the exponent;
        // `pwr`: the sign of `x` times `|x|^y`
        "pow" => two(|x, y| x.powf(y.trunc())),
        "pwr" => two(|x, y| x.signum() * x.abs().powf(y)),
        "sign" => two(|x, y| if y >= 0.0 { x.abs() } else { -x.abs() }),
        "atan2" => two(f64::atan2),
        "hypot" => two(f64::hypot),
        "min" | "max" if a.is_empty() => Err(arity("1 or more")),
        "min" => Ok(a.iter().copied().fold(f64::INFINITY, f64::min)),
        "max" => Ok(a.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

/// A parsed expression
#[derive(Debug)]
enum Expr {
    Num(f64),
    /// a parameter, lowercase
    Var(String),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// a function call; the name is lowercase
    Call(String, Vec<Expr>),
}

/// A recursive-descent parser, by increasing precedence:
/// `?:`, `||`, `&&`, `== !=`, `< <= > >=`, `+ -`, `* /`, unary `- + !`,
/// `** ^`. As in HSPICE, `** ^` associate to the left (`2**3**2` is 64) and bind
/// tighter than a unary minus before them (`-2**2` is -4); a unary minus right
/// after them takes the next operand only (`2**-3**2` is `(2**-3)**2`).
struct Parser<'e> {
    s: &'e str,
    pos: usize,
    depth: usize,
}

impl<'e> Parser<'e> {
    fn new(s: &'e str) -> Self {
        Self {
            s,
            pos: 0,
            depth: 0,
        }
    }

    fn parse(mut self) -> Result<Expr, EvalError> {
        let e = self.ternary()?;
        self.ws();
        if self.pos < self.s.len() {
            return Err(self.error(format!("unexpected `{}`", &self.s[self.pos..])));
        }
        Ok(e)
    }

    fn error(&self, message: String) -> EvalError {
        EvalError::Syntax {
            expr: self.s.trim().to_owned(),
            message,
        }
    }

    /// Skip white space, and the `+` of a continuation line
    fn ws(&mut self) {
        let b = self.s.as_bytes();
        let mut line_start = false;
        while let Some(&c) = b.get(self.pos) {
            match c {
                b' ' | b'\t' | b'\r' => {}
                b'\n' => line_start = true,
                b'+' if line_start => line_start = false,
                _ => break,
            }
            self.pos += 1;
        }
    }

    /// Consume `token` (after white space) if it is next
    fn eat(&mut self, token: &str) -> bool {
        self.ws();
        if self.s[self.pos..].starts_with(token) {
            self.pos += token.len();
            true
        } else {
            false
        }
    }

    /// Whether `token` is next, but not `longer` (e.g. `*` but not `**`)
    fn peek_not(&mut self, token: &str, longer: &str) -> bool {
        self.ws();
        let rest = &self.s[self.pos..];
        rest.starts_with(token) && !rest.starts_with(longer)
    }

    /// Count one more level of nesting
    fn nest(&mut self) -> Result<(), EvalError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("expression nested too deeply".to_owned()));
        }
        Ok(())
    }

    fn ternary(&mut self) -> Result<Expr, EvalError> {
        self.nest()?;
        let cond = self.or()?;
        let e = if self.eat("?") {
            let a = self.ternary()?;
            if !self.eat(":") {
                return Err(self.error("`?` without `:`".to_owned()));
            }
            let b = self.ternary()?;
            Expr::Cond(Box::new(cond), Box::new(a), Box::new(b))
        } else {
            cond
        };
        self.depth -= 1;
        Ok(e)
    }

    fn binary(
        &mut self,
        ops: &[(&str, &str, Op)],
        next: fn(&mut Self) -> Result<Expr, EvalError>,
    ) -> Result<Expr, EvalError> {
        let mut e = next(self)?;
        'outer: loop {
            for (token, longer, op) in ops {
                if self.peek_not(token, longer) {
                    self.pos += token.len();
                    let rhs = next(self)?;
                    e = Expr::Bin(*op, Box::new(e), Box::new(rhs));
                    continue 'outer;
                }
            }
            return Ok(e);
        }
    }

    fn or(&mut self) -> Result<Expr, EvalError> {
        self.binary(&[("||", "|||", Op::Or)], Self::and)
    }
    fn and(&mut self) -> Result<Expr, EvalError> {
        self.binary(&[("&&", "&&&", Op::And)], Self::equality)
    }
    fn equality(&mut self) -> Result<Expr, EvalError> {
        self.binary(
            &[("==", "===", Op::Eq), ("!=", "!==", Op::Ne)],
            Self::relational,
        )
    }
    fn relational(&mut self) -> Result<Expr, EvalError> {
        self.binary(
            &[
                ("<=", "<==", Op::Le),
                (">=", ">==", Op::Ge),
                ("<", "<=", Op::Lt),
                (">", ">=", Op::Gt),
            ],
            Self::additive,
        )
    }
    fn additive(&mut self) -> Result<Expr, EvalError> {
        self.binary(
            &[("+", "++", Op::Add), ("-", "--", Op::Sub)],
            Self::multiplicative,
        )
    }
    fn multiplicative(&mut self) -> Result<Expr, EvalError> {
        self.binary(&[("*", "**", Op::Mul), ("/", "//", Op::Div)], Self::unary)
    }

    fn unary(&mut self) -> Result<Expr, EvalError> {
        self.nest()?;
        let e = if self.eat("-") {
            Expr::Neg(Box::new(self.unary()?))
        } else if self.eat("+") {
            self.unary()?
        } else if self.peek_not("!", "!=") {
            self.pos += 1;
            Expr::Not(Box::new(self.unary()?))
        } else {
            self.power()?
        };
        self.depth -= 1;
        Ok(e)
    }

    fn power(&mut self) -> Result<Expr, EvalError> {
        let mut base = self.primary()?;
        while self.eat("**") || self.eat("^") {
            let exp = self.exponent()?;
            base = Expr::Bin(Op::Pow, Box::new(base), Box::new(exp));
        }
        Ok(base)
    }

    fn exponent(&mut self) -> Result<Expr, EvalError> {
        self.nest()?;
        let e = if self.eat("-") {
            Expr::Neg(Box::new(self.exponent()?))
        } else if self.eat("+") {
            self.exponent()?
        } else if self.peek_not("!", "!=") {
            self.pos += 1;
            Expr::Not(Box::new(self.exponent()?))
        } else {
            self.primary()?
        };
        self.depth -= 1;
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, EvalError> {
        self.ws();
        let rest = &self.s[self.pos..];
        let Some(c) = rest.chars().next() else {
            return Err(self.error("missing value".to_owned()));
        };
        if c == '(' {
            self.pos += 1;
            let e = self.ternary()?;
            if !self.eat(")") {
                return Err(self.error("`(` without `)`".to_owned()));
            }
            return Ok(e);
        }
        if c.is_ascii_digit() || c == '.' {
            let Some((v, len)) = crate::parser::number_prefix(rest) else {
                return Err(self.error(format!("bad number `{rest}`")));
            };
            self.pos += len;
            return Ok(Expr::Num(v));
        }
        if c.is_alphabetic() || c == '_' {
            let len = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            let name = rest[..len].to_ascii_lowercase();
            self.pos += len;
            if self.eat("(") {
                let mut args = Vec::new();
                if !self.eat(")") {
                    loop {
                        args.push(self.ternary()?);
                        if self.eat(")") {
                            break;
                        }
                        if !self.eat(",") {
                            return Err(self.error(format!("bad arguments of `{name}`")));
                        }
                    }
                }
                return Ok(Expr::Call(name, args));
            }
            return Ok(Expr::Var(name));
        }
        Err(self.error(format!("unexpected `{c}`")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(params: &[(&str, f64)]) -> Scope<'static> {
        let mut s = Scope::new(ParHier::Global);
        for (k, v) in params {
            s.set(k, Ok(*v));
        }
        s
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-12 * b.abs().max(1e-30)
    }

    #[test]
    fn numbers_and_operators() {
        let s = top(&[("w", 2e-6), ("l", 1e-7)]);
        let e = |x: &str| s.eval_str(x).unwrap();
        assert!(close(e("2.5u"), 2.5e-6));
        assert!(close(e("1meg"), 1e6));
        assert!(close(e("1+2*3"), 7.0));
        assert!(close(e("(1+2)*3"), 9.0));
        assert!(close(e("2^10"), 1024.0));
        // (as measured on HSPICE)
        assert!(close(e("2**3**2"), 64.0));
        assert!(close(e("2^3^2"), 64.0));
        assert!(close(e("2**-3**2"), 0.015625));
        assert!(close(e("-2**2"), -4.0));
        assert!(close(e("-2**2+10"), 6.0));
        assert!(close(e("(-2)**3"), -8.0));
        assert!(close(e("(-2)**1.5"), -2.0));
        assert!(close(e("(-2)**2.5"), 4.0));
        assert!(close(e("(-2)**0.5"), 1.0));
        assert!(close(e("1+0**0"), 1.0));
        assert!(close(e("1+0**-1"), 1.0));
        assert!(close(e("1+0**0.5"), 1.0));
        assert!(close(e("w/l"), 20.0));
        assert!(close(e("2*W+100n"), 4.1e-6));
        assert!(close(e("1 - 2 - 3"), -4.0));
        assert!(close(e("8/2/2"), 2.0));
        assert!(close(e("w > l ? 1 : 0"), 1.0));
        assert!(close(e("1>0?0>1?4:8:9"), 8.0));
        assert!(close(e("!(w<l) && (1 || 0)"), 1.0));
        assert!(close(e("1 == 1"), 1.0));
        assert!(close(e("1 != 1"), 0.0));
        // a continuation line inside a quoted expression
        assert!(close(e("1+\n+ 2"), 3.0));
    }

    #[test]
    fn numbers_as_measured_on_hspice() {
        let s = top(&[]);
        for (text, value) in [
            ("1me", 1e6),
            ("1meter", 1e6),
            ("1meg", 1e6),
            ("1megs", 1e6),
            ("1x", 1e6),
            ("1m", 1e-3),
            ("1M", 1e-3),
            ("1mohm", 1e-3),
            ("1mi", 25.4e-6),
            ("1mil", 25.4e-6),
            ("1mile", 25.4e-6),
            ("1min", 60.0),
            ("1minute", 60.0),
            ("1hr", 3600.0),
            ("1hrs", 3600.0),
            ("1day", 86400.0),
            ("1days", 86400.0),
            ("1yr", 31536000.0),
            ("1y", 1e-24),
            ("1years", 1e-24),
            ("1z", 1e-21),
            ("1zz", 1e-21),
            ("1a", 1e-18),
            ("20db", 10.0),
            ("20dbm", 10.0),
            ("10pF", 1e-11),
            ("1.8V", 1.8),
            // (an exponent makes the scale factor void)
            ("1e-3k", 1e-3),
            ("2E3meg", 2e3),
        ] {
            assert!(close(s.eval_str(text).unwrap(), value), "{text}");
        }
    }

    #[test]
    fn functions() {
        let s = top(&[("x", 4.0)]);
        let e = |x: &str| s.eval_str(x).unwrap();
        assert!(close(e("sqrt(x)"), 2.0));
        // (as measured on HSPICE)
        assert!(close(e("sqrt(-4)"), -2.0));
        assert!(close(e("log(-100)"), -100f64.abs().ln()));
        assert!(close(e("log10(-100)"), -2.0));
        assert!(close(e("max(1, x, 3)"), 4.0));
        assert!(close(e("min(1, -x)"), -4.0));
        assert!(close(e("log(exp(2))"), 2.0));
        assert!(close(e("log10(1k)"), 3.0));
        assert!(close(e("pow(2, 3.7)"), 8.0));
        assert!(close(e("pwr(-2, 2)"), -4.0));
        assert!(close(e("abs(-x)"), 4.0));
        assert!(close(e("int(2.7) + nint(2.5)"), 5.0));
        assert!(close(e("sin(pi/2)"), 1.0));
        // statistical functions: the nominal value
        assert!(close(e("agauss(1u, 0.1u, 3)"), 1e-6));
        assert!(close(e("2*aunif(x, 1)"), 8.0));
        assert_eq!(
            s.eval_str("v(out)*2"),
            Err(EvalError::NotStatic("v(...)".to_owned()))
        );
        assert_eq!(
            s.eval_str("foo(1)"),
            Err(EvalError::UnknownFunction("foo".to_owned()))
        );
        assert!(matches!(
            s.eval_str("sqrt(1, 2)"),
            Err(EvalError::Arguments { .. })
        ));
    }

    /// Deep nesting and long chains of parameters are errors, not stack
    /// overflows
    #[test]
    fn depth_limits() {
        let mut s = top(&[]);
        assert!(matches!(
            s.eval_str(&format!("{}1", "-".repeat(100_000))),
            Err(EvalError::Syntax { .. })
        ));
        assert!(matches!(
            s.eval_str(&format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000))),
            Err(EvalError::Syntax { .. })
        ));
        assert_eq!(s.eval_str("--1"), Ok(1.0));
        for i in 0..10_000 {
            let kv = crate::ast::KeyValue {
                k: format!("p{i}").into(),
                v: Value::Expr(format!("p{}+1", i + 1).into()),
            };
            s.define(&kv);
        }
        s.set("p10000", Ok(0.0));
        assert!(matches!(s.get("p0"), Err(EvalError::TooDeep(_))));
        assert_eq!(s.get("p9950"), Ok(50.0));
    }

    #[test]
    fn errors() {
        let mut s = top(&[]);
        assert_eq!(
            s.eval_str("2*wn"),
            Err(EvalError::Undefined("wn".to_owned()))
        );
        assert!(matches!(s.eval_str("2*"), Err(EvalError::Syntax { .. })));
        assert!(matches!(s.eval_str("(1"), Err(EvalError::Syntax { .. })));
        assert!(matches!(s.eval_str("1 2"), Err(EvalError::Syntax { .. })));
        assert_eq!(
            s.eval_str("1/0"),
            Err(EvalError::NotFinite("1/0".to_owned()))
        );
        s.set("a", Err(EvalError::Undefined("q".to_owned())));
        assert_eq!(s.get("a"), Err(EvalError::Undefined("q".to_owned())));
    }
}
