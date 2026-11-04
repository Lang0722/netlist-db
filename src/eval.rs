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
//! parameter defined at several levels with HSPICE's rules
//! (`.option parhier`).
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

/// Which definition of a parameter defined at several levels of the
/// hierarchy is used (`.option parhier`). A value passed on the line of a
/// subckt instance (`X1 a b inv w=2u`) is always used inside that instance
/// (the closest one, if several instances above pass it); the rule decides
/// between the other definitions: `.param` statements and `.subckt` default
/// values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParHier {
    /// The definition closest to the top level: e.g. a top-level `.param w`
    /// overrides the default value of a `.subckt` parameter `w` (HSPICE's
    /// default)
    #[default]
    Global,
    /// The definition closest to the element
    Local,
}

/// How a parameter is bound at a level (see [`Scope::binding`]): with
/// `parhier=local`, the value of its closest definition or passed value; with
/// `parhier=global`, the closest value passed and the value of the definition
/// closest to the top. A value is the bits of a number, or the error that
/// computing it gave.
pub(crate) type Binding = (Option<Result<u64, String>>, Option<Result<u64, String>>);

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
    /// values passed on the instance line
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
        let body = match &kv.v {
            Value::Num(n) => Def::Num(*n),
            Value::Expr(e) => Def::Expr(e.to_string()),
        };
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
    /// (`X1 a b inv w=2u`); it is computed at the level of the instance line
    pub fn pass(&mut self, name: &str, value: Result<f64, EvalError>) {
        self.passed.insert(name.to_ascii_lowercase(), value.into());
    }

    /// Whether `name` is defined at this level (not above)
    pub fn defines(&self, name: &str) -> bool {
        let key = name.to_ascii_lowercase();
        self.defs.contains_key(&key) || self.passed.contains_key(&key)
    }

    /// The value of the parameter `name`
    pub fn get(&self, name: &str) -> Result<f64, EvalError> {
        let key = name.to_ascii_lowercase();
        match self.find(&key) {
            Some((scope, true)) => scope.passed_value(&key),
            Some((scope, false)) => scope.value_here(&key),
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

    /// The level whose definition of `key` is used, and whether it is a
    /// passed value
    fn find(&self, key: &str) -> Option<(&Scope<'p>, bool)> {
        match self.parhier {
            ParHier::Local => self.levels().find_map(|s| {
                if s.passed.contains_key(key) {
                    Some((s, true))
                } else if s.defs.contains_key(key) {
                    Some((s, false))
                } else {
                    None
                }
            }),
            // the closest passed value, else the definition closest to the top
            ParHier::Global => self
                .levels()
                .find(|s| s.passed.contains_key(key))
                .map(|s| (s, true))
                .or_else(|| {
                    self.levels()
                        .filter(|s| s.defs.contains_key(key))
                        .last()
                        .map(|s| (s, false))
                }),
        }
    }

    /// The value passed to this level for `key`
    fn passed_value(&self, key: &str) -> Result<f64, EvalError> {
        match &self.passed[key] {
            Def::Num(n) => Ok(*n),
            Def::Err(e) => Err(e.clone()),
            Def::Expr(e) => self.eval_str(e),
        }
    }

    /// How the parameter `name` is bound, seen from this level: what decides
    /// its value here, and in the levels below that do not bind it
    /// themselves. Two levels with the same bindings for the parameters that
    /// an expression reads give it the same value.
    pub(crate) fn binding(&self, name: &str) -> Binding {
        let key = name.to_ascii_lowercase();
        let bits = |v: Result<f64, EvalError>| v.map(f64::to_bits).map_err(|e| e.to_string());
        match self.parhier {
            // the closest definition or passed value
            ParHier::Local => (
                self.find(&key).map(|(s, passed)| {
                    bits(if passed {
                        s.passed_value(&key)
                    } else {
                        s.value_here(&key)
                    })
                }),
                None,
            ),
            // the closest passed value, and the definition closest to the top
            ParHier::Global => (
                self.levels()
                    .find(|s| s.passed.contains_key(&key))
                    .map(|s| bits(s.passed_value(&key))),
                self.levels()
                    .filter(|s| s.defs.contains_key(&key))
                    .last()
                    .map(|s| bits(s.value_here(&key))),
            ),
        }
    }

    /// The value of `key`, which this level defines
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
        self.active.borrow_mut().push(key.to_owned());
        let v = match &self.defs[key] {
            Def::Num(n) => Ok(*n),
            Def::Expr(e) => self.eval_str(e),
            Def::Err(e) => Err(e.clone()),
        };
        self.active.borrow_mut().pop();
        self.cache.borrow_mut().insert(key.to_owned(), v.clone());
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

    fn func(&self, name: &str) -> Option<&Func> {
        let mut level = Some(self);
        while let Some(s) = level {
            if let Some(f) = s.funcs.get(name) {
                return Some(f);
            }
            level = s.parent;
        }
        None
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
                    Op::Pow => x.powf(y),
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
                if let Some(f) = self.func(name) {
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
                            let tree = self.parse(body)?;
                            self.eval_tree(&tree, &locals, depth + 1)
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
/// f(x)=...` (the parameters a function's body reads are those of the level
/// where it is called). An expression that is not valid reads nothing: it is
/// an error wherever it is.
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
        "sqrt" => one(f64::sqrt),
        "exp" => one(f64::exp),
        // natural logarithm, as in HSPICE
        "log" | "ln" => one(f64::ln),
        "log10" => one(f64::log10),
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
/// `** ^` (right-associative, binds tighter than a unary minus before it).
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
        let base = self.primary()?;
        if self.eat("**") || self.eat("^") {
            let exp = self.unary()?;
            return Ok(Expr::Bin(Op::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
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
        assert!(close(e("2**3**2"), 512.0));
        assert!(close(e("2^10"), 1024.0));
        assert!(close(e("-2**2"), -4.0));
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
    fn functions() {
        let s = top(&[("x", 4.0)]);
        let e = |x: &str| s.eval_str(x).unwrap();
        assert!(close(e("sqrt(x)"), 2.0));
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
