//! The model HSPICE uses for each MOSFET (its "model selector").
//!
//! A MOSFET names a model, e.g. `nch`. HSPICE looks at the levels of the
//! hierarchy, from the MOSFET's own up to the top level. At each level, if a
//! model of that name is defined, it is the one. Otherwise it is the first
//! bin of that name, in netlist order, whose ranges hold the transistor's
//! length and width; if none does, the search goes on one level up. The bins
//! of `nch` are the models `nch.1`, `nch.mc`, ... (`nch.` and any suffix):
//!
//! ``` text
//! LMIN <= L < LMAX   and   WMIN <= W < WMAX
//! ```
//!
//! Details, as measured on HSPICE P-2019.06 with `scripts/hspice_kit`:
//! - The levels are those of the instance path: a model (or a subckt) defined
//!   in the subckt of any instance above is seen, though the text of the
//!   MOSFET is not inside that subckt. One line can then use different models
//!   on different paths.
//! - A name with a dot cannot be named on an instance line, and a name with
//!   two dots (`n.ch.1`) is a bin of nothing.
//! - A bin must set its four limits: HSPICE stops if it looks at one that
//!   does not.
//! - HSPICE stops at an `X` line with no definition on its instance path, also
//!   if the same line resolves on another path. Here that is the error
//!   [`SelectError::NoSubckt`], and the walk goes on without the MOSFETs below
//!   the line. HSPICE runs a line that calls a Verilog-A module (`.hdl`) or a
//!   subckt in an encrypted block. This crate reads neither, so it reports
//!   those lines too.
//! - The model name is the first word after the fourth node of a MOSFET line that
//!   names a model on the instance path, else the fourth word if it names one.
//!   The words after it are values, in any number, and `OFF` takes a place among
//!   them and gives no value. More than 7 nodes before it make HSPICE stop
//!   ([`SelectError::TooManyNodes`]), and so does a fifth word that starts with a
//!   digit before it ([`SelectError::DigitFifthWord`]).
//! - The limits are evaluated where the model is defined: those of a model
//!   of the top level with the top-level parameters (a subckt parameter of
//!   the same name does not change them), those of a model inside a subckt
//!   with the parameters of that subckt's instance (e.g.
//!   `LMIN='2.4e-7-(dxln+dlmisn)'` in a macro model).
//! - L and W are the instance's `L=` and `W=` or the first two values after
//!   its model name (else `.option DEFL`/`DEFW`, default 100u), times
//!   `.option SCALE`; the limits are multiplied by `.option SCALM`. The width
//!   per finger (`W/NF`) is compared, unless `WNFLAG=0` on the instance line
//!   or, without one, `.option WNFLAG=0`. A `WNFLAG` in a model is ignored.
//! - A value at most 2e-15 (relative) below a limit counts as on it, so that
//!   `L='0.1u*3'` and `LMAX=3e-7` compare equal. (Measured: 1e-15 below does,
//!   3e-15 below does not, at limits from 1e-8 to 1e-4.)
//!
//! Known gap. HSPICE evaluates the limits of each bin differently for one
//! MOSFET: the first one it makes (breadth first) that looks at the bin
//! evaluates them in its own instance, not where the model is defined (decks
//! `q_limits_*` of the kit, and the probe's case 7). This changes the choice
//! only if a level below the model binds a name that the limits read, in a way
//! that changes its value there (`parhier=local`, or a name no level above
//! binds), so for at most one MOSFET per bin, and for none if the limits are
//! numbers. It is not done here: a model is chosen for a transistor from its
//! own line and the levels above it, in any order of the walk.

use crate::{
    AST, Subckt,
    ast::{KeyValue, Model, Value},
    eval::{Binding, EvalError, ParHier, Scope, expr_names, function_name},
    instance::{Instance, InstanceCtx, MOSFET, is_off},
    utlis::MAX_HIERARCHY_DEPTH,
};
use core::fmt;
use indexmap::IndexSet;
use itertools::Itertools as _;
use std::{
    borrow::Cow,
    cell::OnceCell,
    collections::{HashMap, HashSet},
    iter::once,
    ptr,
    rc::Rc,
};

/// Why no model could be chosen
#[derive(Debug, Clone, PartialEq)]
pub enum SelectError {
    /// No model of this name, and no bins `name.N`
    NoModel(String),
    /// Bins `name.N` exist, but no bin holds this length and width
    NoBin {
        name: String,
        l: f64,
        w: f64,
        bins: usize,
    },
    /// A bin of the name does not set its four limits
    MissingLimits(String),
    /// A value (L, W, or a limit of a bin) could not be computed
    Eval { what: String, error: EvalError },
    /// An `X` line calls this name, and no subckt of this name is on the
    /// line's instance path. HSPICE stops there. This is the one error that
    /// does not concern a MOSFET: the MOSFETs below the line are not known.
    NoSubckt(String),
    /// More than 7 nodes before the model name of a MOSFET. HSPICE stops there
    /// (deck `q_nodes_8`).
    TooManyNodes { name: String, nodes: usize },
    /// A word that starts with a digit after the fourth node of a MOSFET, and
    /// the model name after it. HSPICE stops there: "Model name is missing"
    /// (deck `q_digit_word_5`). A sixth or seventh node can start with a digit.
    DigitFifthWord { name: String, word: String },
}

impl fmt::Display for SelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoModel(name) => write!(f, "no model `{name}`, nor bins `{name}.N`"),
            Self::NoBin { name, l, w, bins } => write!(
                f,
                "none of the {bins} bins `{name}.N` holds L={}, W={}",
                Short(*l),
                Short(*w)
            ),
            Self::MissingLimits(model) => write!(
                f,
                "model `{model}` has a `.` in its name: it must set LMIN, LMAX, WMIN and WMAX"
            ),
            Self::Eval { what, error } => write!(f, "{what}: {error}"),
            Self::NoSubckt(name) => write!(
                f,
                "X line with no definition of subckt `{name}`: the MOSFETs below it are not counted"
            ),
            Self::TooManyNodes { name, nodes } => write!(
                f,
                "{nodes} nodes before the model name `{name}`: HSPICE reads at most 7"
            ),
            Self::DigitFifthWord { name, word } => write!(
                f,
                "the word `{word}` after the fourth node, before the model name `{name}`: \
                 HSPICE wants the model name there, and a model name starts with a letter"
            ),
        }
    }
}

impl std::error::Error for SelectError {}

/// A number in scientific notation, without the noise of its last bits:
/// `1.2e-4`, not `1.1999999999999999e-4`
#[derive(Debug, Clone, Copy)]
pub struct Short(pub f64);

impl fmt::Display for Short {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let x = self.0;
        // the shortest exact form, if short; else the fewest digits that
        // give back `x` within 1e-12 (relative)
        let exact = format!("{x:e}");
        if exact.len() <= 12 {
            return f.write_str(&exact);
        }
        for digits in 0..16 {
            let s = format!("{x:.digits$e}");
            if s.parse::<f64>()
                .is_ok_and(|y| (y - x).abs() <= x.abs() * 1e-12)
            {
                return f.write_str(&s);
            }
        }
        write!(f, "{x:e}")
    }
}

/// The model chosen for a MOSFET
#[derive(Debug, Clone)]
pub struct Selection<'a, 's> {
    pub model: &'a Model<'s>,
    /// the length and width it was chosen for (after `.option scale`)
    pub l: f64,
    pub w: f64,
    /// the number of fingers (`NF`, 1 if not given)
    pub nf: f64,
}

/// A MOSFET of the circuit, with the model chosen for it. An `X` line with no
/// definition also gets an entry: its `selection` is
/// [`SelectError::NoSubckt`], and `mname` is the subckt name on the line.
#[derive(Debug, Clone)]
pub struct MosfetModel<'a, 's> {
    /// the path of the instance, e.g. `Xinv.Mn` (the subckt instances above
    /// it, then its own name)
    pub path: String,
    pub instance: &'a Instance<'s>,
    /// the model name on the instance line
    pub mname: &'a str,
    pub selection: Result<Selection<'a, 's>, SelectError>,
}

/// The `.option` values used to choose models
#[derive(Debug, Clone, Copy)]
struct Options {
    scale: f64,
    scalm: f64,
    defl: f64,
    defw: f64,
    /// `.option wnflag`: whether bins compare the width per finger when the
    /// instance does not say (by default, they do)
    wnflag: bool,
}

impl Options {
    fn new(ast: &AST<'_>, scope: &Scope<'_>) -> Self {
        let get = |name: &str, default: f64| {
            ast.option_value(name)
                .and_then(|v| scope.eval(v).ok())
                .unwrap_or(default)
        };
        Self {
            scale: get("scale", 1.0),
            scalm: get("scalm", 1.0),
            defl: get("defl", 1e-4),
            defw: get("defw", 1e-4),
            wnflag: get("wnflag", 1.0) != 0.0,
        }
    }
}

/// How far below a limit (relative) a value counts as on it
const EDGE_TOLERANCE: f64 = 2e-15;

/// `min <= x < max`, a value just below a limit counting as on it
fn within(x: f64, min: f64, max: f64) -> bool {
    let tol = |v: f64| v.abs() * EDGE_TOLERANCE;
    x >= min - tol(min) && x < max - tol(max)
}

/// The ranges of a bin
#[derive(Debug, Clone, Copy)]
struct Limits {
    lmin: f64,
    lmax: f64,
    wmin: f64,
    wmax: f64,
}

impl Limits {
    /// Whether these ranges hold a transistor of size `size`
    fn hold(&self, size: &Size, opts: &Options) -> bool {
        // the instance's WNFLAG, else `.option wnflag`
        let per_finger = size.wnflag.unwrap_or(opts.wnflag);
        let w = if per_finger && size.nf > 0.0 {
            size.w / size.nf
        } else {
            size.w
        };
        within(size.l, self.lmin, self.lmax) && within(w, self.wmin, self.wmax)
    }
}

/// The parameters of a bin that its ranges are made of
const LIMITS: [&str; 4] = ["lmin", "lmax", "wmin", "wmax"];

/// The ranges of the bin `model`, evaluated in `scope`
fn limits(model: &Model<'_>, scope: &Scope<'_>, scalm: f64) -> Result<Limits, SelectError> {
    let param = |key: &str| -> Result<f64, SelectError> {
        let Some(kv) = model
            .params
            .iter()
            .rev()
            .find(|kv| kv.k.eq_ignore_ascii_case(key))
        else {
            return Err(SelectError::MissingLimits(model.name.to_string()));
        };
        scope
            .eval(&kv.v)
            .map(|v| v * scalm)
            .map_err(|error| SelectError::Eval {
                what: format!("{} of model `{}`", key.to_ascii_uppercase(), model.name),
                error,
            })
    };
    Ok(Limits {
        lmin: param("lmin")?,
        lmax: param("lmax")?,
        wmin: param("wmin")?,
        wmax: param("wmax")?,
    })
}

/// The size of a MOSFET, as its bin is chosen
#[derive(Debug, Clone, Copy)]
struct Size {
    /// L and W, times `.option scale`
    l: f64,
    w: f64,
    nf: f64,
    /// `WNFLAG` on the instance line, if given
    wnflag: Option<bool>,
}

/// The parameters of an instance line that the size of a MOSFET is made of
const SIZES: [&str; 4] = ["l", "w", "nf", "wnflag"];

/// The parameters that the first values after the model name give
const POSITIONAL: [&str; 2] = ["l", "w"];

/// The most nodes that HSPICE reads before the model name of a MOSFET
/// (decks `q_nodes_7`, `q_nodes_8`)
const MAX_MOSFET_NODES: usize = 7;

/// The words of the MOSFET line `m` that can be its model name, the last one
/// first. The parser takes the last word that is not a number, so in
/// `M1 d g s b nch pl pw` it takes `pw`, with `nch` and `pl` in the place of
/// nodes.
fn name_words<'m, 's>(m: &'m MOSFET<'s>) -> impl Iterator<Item = &'m Cow<'s, str>> {
    once(&m.mname)
        .chain(m.extra_nodes.iter().rev())
        .chain(&m.nb)
}

/// A MOSFET line as the selector reads it. [`Selector::read_line`] finds the
/// model name in [`name_words`], and the words after
/// it are values. HSPICE reads `M1 d g s b nch pl pw` so, with the parameters
/// `pl` and `pw` as L and W (deck `q_positional_names`).
#[derive(Clone, Copy)]
struct MosLine<'m, 's> {
    m: &'m MOSFET<'s>,
    /// the place of the model name in [`name_words`]
    model: usize,
}

impl<'m, 's> MosLine<'m, 's> {
    fn name(self) -> &'m str {
        name_words(self.m)
            .nth(self.model)
            .expect("a word of the line")
    }

    /// The number of words before the model name
    fn nodes(self) -> usize {
        3 + usize::from(self.m.nb.is_some()) + self.m.extra_nodes.len() - self.model
    }

    /// The value at the place `position` after the model name. A bare `OFF`
    /// takes a place and gives no value.
    fn value(self, position: usize) -> Option<Value<'s>> {
        match self.model.checked_sub(position + 1) {
            Some(word) => name_words(self.m).nth(word).map(|w| Value::Expr(w.clone())),
            None => self
                .m
                .positional
                .get(position - self.model)
                .map(|(_, v)| v.clone()),
        }
        .filter(|v| !is_off(v))
    }

    /// The values after the model name
    fn values(self) -> impl Iterator<Item = Value<'s>> {
        (0..self.model + self.m.positional.len()).filter_map(move |p| self.value(p))
    }
}

/// The size of the MOSFET `line`, evaluated in `scope`
fn size(line: MosLine<'_, '_>, scope: &Scope<'_>, opts: &Options) -> Result<Size, SelectError> {
    let m = line.m;
    let value = |key: &str| {
        // (a keyword wins over a number after the model name)
        let v = m
            .params
            .iter()
            .rev()
            .find(|kv| kv.k.eq_ignore_ascii_case(key))
            .map(|kv| Cow::Borrowed(&kv.v))
            .or_else(|| {
                let position = POSITIONAL.iter().position(|k| *k == key)?;
                line.value(position).map(Cow::Owned)
            });
        match v {
            Some(v) => scope.eval(&v).map(Some).map_err(|error| SelectError::Eval {
                what: key.to_ascii_uppercase(),
                error,
            }),
            None => Ok(None),
        }
    };
    Ok(Size {
        l: value("l")?.unwrap_or(opts.defl) * opts.scale,
        w: value("w")?.unwrap_or(opts.defw) * opts.scale,
        nf: value("nf")?.unwrap_or(1.0),
        wnflag: value("wnflag")?.map(|flag| flag != 0.0),
    })
}

/// The models of one level (the top level, or a subckt), by lowercase name
struct ModelIndex<'a, 's> {
    /// the model of each name without a dot (the last one, if one is defined
    /// twice)
    exact: HashMap<String, &'a Model<'s>>,
    /// the bins `name.suffix` of each `name`, in netlist order
    bins: HashMap<String, Vec<Bin<'a, 's>>>,
}

struct Bin<'a, 's> {
    model: &'a Model<'s>,
    /// whether it sets its four limits
    complete: bool,
    /// its ranges if they read no parameter (they are the same everywhere),
    /// once computed
    fixed: OnceCell<Option<Result<Limits, SelectError>>>,
}

impl<'a, 's> ModelIndex<'a, 's> {
    fn new(models: &'a [Model<'s>]) -> Self {
        let mut index = Self {
            exact: HashMap::new(),
            bins: HashMap::new(),
        };
        for model in models {
            let name = model.name.to_ascii_lowercase();
            match name.split_once('.') {
                None => {
                    index.exact.insert(name, model);
                }
                // a bin of `base`, any suffix; `a.b.1` is of no use
                Some((base, suffix))
                    if !base.is_empty() && !suffix.is_empty() && !suffix.contains('.') =>
                {
                    let sets =
                        |key: &str| model.params.iter().any(|kv| kv.k.eq_ignore_ascii_case(key));
                    index.bins.entry(base.to_owned()).or_default().push(Bin {
                        model,
                        complete: LIMITS.iter().all(|key| sets(key)),
                        fixed: OnceCell::new(),
                    });
                }
                Some(_) => {}
            }
        }
        index
    }
}

/// The functions defined with `.param f(x)=...` anywhere in `ast`
fn function_names(ast: &AST<'_>) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut stack = vec![ast];
    while let Some(a) = stack.pop() {
        names.extend(a.param.iter().filter_map(|kv| function_name(&kv.k)));
        names.extend(a.param_func.iter().map(|f| f.name.to_ascii_lowercase()));
        for def in a.subckt.values() {
            names.extend(def.params.iter().filter_map(|kv| function_name(&kv.k)));
            stack.push(&def.ast);
        }
    }
    names
}

/// Chooses models, keeping what that computes: the models of each level by
/// name, and the ranges of the bins
struct Selector<'a, 's> {
    opts: Options,
    /// the functions defined with `.param f(x)=...`
    functions: HashSet<String>,
    /// the models of a level (by address)
    index: HashMap<usize, Rc<ModelIndex<'a, 's>>>,
    /// the parameters that an expression reads (`None`: not known)
    names: HashMap<String, Option<Rc<[String]>>>,
    /// the parameters that the ranges of a bin (by address) read
    limit_names: HashMap<usize, Option<Rc<[String]>>>,
    /// the ranges of a bin, for the bindings of the parameters they read
    limits: HashMap<(usize, Vec<Binding>), Result<Limits, SelectError>>,
}

impl<'a, 's> Selector<'a, 's> {
    fn new(ast: &AST<'_>, scope: &Scope<'_>) -> Self {
        Self {
            opts: Options::new(ast, scope),
            functions: function_names(ast),
            index: HashMap::new(),
            names: HashMap::new(),
            limit_names: HashMap::new(),
            limits: HashMap::new(),
        }
    }

    fn index(&mut self, ast: &'a AST<'s>) -> Rc<ModelIndex<'a, 's>> {
        self.index
            .entry(ptr::from_ref(ast) as usize)
            .or_insert_with(|| Rc::new(ModelIndex::new(&ast.model)))
            .clone()
    }

    /// The parameters that the value `v` reads (`None`: not known)
    fn names(&mut self, v: &Value<'_>) -> Option<Rc<[String]>> {
        let Value::Expr(e) = v else {
            return Some(Rc::from([]));
        };
        if let Some(names) = self.names.get(&**e) {
            return names.clone();
        }
        let names: Option<Rc<[String]>> = expr_names(e, &self.functions).map(Rc::from);
        self.names.insert(e.to_string(), names.clone());
        names
    }

    /// The parameters that the ranges of the bin `model` read
    fn limit_names(&mut self, model: &Model<'_>) -> Option<Rc<[String]>> {
        let addr = ptr::from_ref(model) as usize;
        if let Some(names) = self.limit_names.get(&addr) {
            return names.clone();
        }
        let mut all = IndexSet::new();
        let mut known = true;
        for kv in &model.params {
            if LIMITS.iter().any(|k| kv.k.eq_ignore_ascii_case(k)) {
                match self.names(&kv.v) {
                    Some(names) => extend(&mut all, &names),
                    None => {
                        known = false;
                        break;
                    }
                }
            }
        }
        let names: Option<Rc<[String]>> = known.then(|| all.into_iter().collect());
        self.limit_names.insert(addr, names.clone());
        names
    }

    /// The ranges of the bin `model`, for a transistor in `scope`
    fn limits(&mut self, model: &Model<'_>, scope: &Scope<'_>) -> Result<Limits, SelectError> {
        let scalm = self.opts.scalm;
        // ranges that call a function are computed each time
        let Some(names) = self.limit_names(model) else {
            return limits(model, scope, scalm);
        };
        let key = (
            ptr::from_ref(model) as usize,
            names.iter().map(|n| scope.binding(n)).collect(),
        );
        self.limits
            .entry(key)
            .or_insert_with(|| limits(model, scope, scalm))
            .clone()
    }

    /// The model named `name` for a transistor of size `size`. `on_path`: the
    /// definitions on its instance path, from the top level to its own;
    /// `scopes`: the parameters of each, the top level and the instances. The
    /// ranges of a bin are evaluated at the level that defines it.
    fn choose(
        &mut self,
        name: &str,
        size: Size,
        scopes: &[&Scope<'_>],
        on_path: &[&'a AST<'s>],
    ) -> Result<Selection<'a, 's>, SelectError> {
        let Size { l, w, nf, .. } = size;
        let key = name.to_ascii_lowercase();
        let mut seen = 0;
        for (&ast, &scope) in on_path.iter().zip(scopes).rev() {
            let index = self.index(ast);
            // a model of this very name
            if let Some(&model) = index.exact.get(&key) {
                return Ok(Selection { model, l, w, nf });
            }
            let Some(bins) = index.bins.get(&key) else {
                continue;
            };
            if let Some(bin) = bins.iter().find(|bin| !bin.complete) {
                return Err(SelectError::MissingLimits(bin.model.name.to_string()));
            }
            // the first bin that holds the transistor; a bin before it whose
            // ranges cannot be computed makes the choice uncertain: an error
            for bin in bins {
                if bin.fixed.get().is_none() {
                    let fixed = self
                        .limit_names(bin.model)
                        .is_some_and(|names| names.is_empty())
                        .then(|| limits(bin.model, scope, self.opts.scalm));
                    _ = bin.fixed.set(fixed);
                }
                let limits = match bin.fixed.get() {
                    Some(Some(Ok(limits))) => *limits,
                    Some(Some(Err(e))) => return Err(e.clone()),
                    _ => self.limits(bin.model, scope)?,
                };
                if limits.hold(&size, &self.opts) {
                    return Ok(Selection {
                        model: bin.model,
                        l,
                        w,
                        nf,
                    });
                }
            }
            seen += bins.len();
        }
        if seen == 0 {
            return Err(SelectError::NoModel(name.to_owned()));
        }
        Err(SelectError::NoBin {
            name: name.to_owned(),
            l,
            w,
            bins: seen,
        })
    }

    /// How the level with the definitions `on_path` reads the MOSFET line `m`
    ///
    /// The model name is the first word after the fourth node that names a model
    /// on the instance path, else the fourth word if it names one, else the last
    /// word (so the error names it). The rest of the line after it holds values:
    /// HSPICE reads `M1 d g 0 0 a b w=1u` with `b` as L, and `M1 d g 0 a b w=1u`
    /// with `a` as a node (decks `q_two_model_words*`).
    fn read_line<'m, 'l>(&mut self, m: &'m MOSFET<'l>, on_path: &[&'a AST<'s>]) -> MosLine<'m, 'l> {
        let mut names_model = |word: &str| {
            let key = word.to_ascii_lowercase();
            on_path.iter().any(|&ast| {
                let index = self.index(ast);
                index.exact.contains_key(&key) || index.bins.contains_key(&key)
            })
        };
        let after_fourth = m.extra_nodes.iter().chain(once(&m.mname));
        let model = after_fourth
            .into_iter()
            .position(|word| names_model(word))
            .map(|k| m.extra_nodes.len() - k)
            .or_else(|| {
                m.nb.as_deref()
                    .is_some_and(&mut names_model)
                    .then_some(m.extra_nodes.len() + 1)
            })
            .unwrap_or(0);
        MosLine { m, model }
    }

    /// The model of the MOSFET `m`, at the level whose instance path has the
    /// definitions `on_path` and the parameters `scopes` (its own last)
    fn select(
        &mut self,
        m: &MOSFET<'_>,
        scopes: &[&Scope<'_>],
        on_path: &[&'a AST<'s>],
    ) -> Result<Selection<'a, 's>, SelectError> {
        let scope = scopes.last().expect("the top level at least");
        let line = self.read_line(m, on_path);
        let size = size(line, scope, &self.opts)?;
        let selection = self.choose(line.name(), size, scopes, on_path)?;
        if line.nodes() > MAX_MOSFET_NODES {
            return Err(SelectError::TooManyNodes {
                name: line.name().to_owned(),
                nodes: line.nodes(),
            });
        }
        if line.nodes() > 4 && m.extra_nodes[0].starts_with(|c: char| c.is_ascii_digit()) {
            return Err(SelectError::DigitFifthWord {
                name: line.name().to_owned(),
                word: m.extra_nodes[0].to_string(),
            });
        }
        Ok(selection)
    }
}

/// Adds the names of `more` that `names` lacks
fn extend(names: &mut IndexSet<String>, more: &[String]) {
    names.extend(more.iter().cloned());
}

/// The definition of the subckt `name`, seen from the level whose instance
/// path has the definitions `on_path`: the nearest one
fn resolve<'a, 's>(name: &str, on_path: &[&'a AST<'s>]) -> Option<&'a Subckt<'s>> {
    let key = name.to_lowercase();
    on_path.iter().rev().find_map(|a| a.subckt.get(&key))
}

/// The parameters of an instance of `def` whose line passes `line`: its
/// `.subckt` defaults and `.param` statements, and the values passed. Each
/// value passed is computed now, so that a value passed down a deep hierarchy
/// is not a deep recursion later.
fn instance_scope<'p>(scope: &'p Scope<'p>, def: &Subckt<'_>, line: &[KeyValue<'_>]) -> Scope<'p> {
    let mut child = scope.child();
    for kv in def.params.iter().chain(&def.ast.param) {
        child.define(kv);
    }
    for f in &def.ast.param_func {
        child.define_func(f);
    }
    for kv in line {
        child.pass(kv);
    }
    for kv in line {
        _ = child.get(&kv.k);
    }
    child
}

impl<'s> AST<'s> {
    /// The model HSPICE uses for a MOSFET at the top level whose model name
    /// is `name`, of length `l` and width `w` (in meters) and one finger:
    /// the model named `name`, or the bin `name.N` that holds `l` and `w`
    pub fn select_model(&self, name: &str, l: f64, w: f64) -> Result<&Model<'s>, SelectError> {
        let scope = self.scope();
        let mut sel = Selector::new(self, &scope);
        let size = Size {
            l,
            w,
            nf: 1.0,
            wnflag: None,
        };
        sel.choose(name, size, &[&scope], &[self]).map(|s| s.model)
    }

    /// Every MOSFET of the circuit, in netlist order: the MOSFETs at the top
    /// level, and in the subckt instances under it. For each one: its
    /// instance path, its length and width (with the parameters of every
    /// subckt instance above it), and the model chosen for it.
    ///
    /// The list holds every transistor; for a circuit with millions of them,
    /// see [`AST::for_each_mosfet_model`] and [`AST::model_usage`].
    pub fn mosfet_models(&self) -> Vec<MosfetModel<'_, 's>> {
        let mut out = Vec::new();
        self.for_each_mosfet_model(|m| out.push(m));
        out
    }

    /// The MOSFETs of the subckt `name` (defined at the top level), as in
    /// [`AST::mosfet_models`], for an instance of it with its default
    /// parameters; e.g. the cells of a library. `None` if there is no such
    /// subckt. The paths start below the subckt (`MM1`, `XI0.MM2`).
    pub fn mosfet_models_of(&self, name: &str) -> Option<Vec<MosfetModel<'_, 's>>> {
        let mut out = Vec::new();
        self.for_each_mosfet_model_of(name, |m| out.push(m))
            .then_some(out)
    }

    /// Calls `f` with each MOSFET of [`AST::mosfet_models`], in the same
    /// order, without keeping them: for circuits whose hierarchy holds too
    /// many transistors to list in memory
    pub fn for_each_mosfet_model<'a>(&'a self, f: impl FnMut(MosfetModel<'a, 's>)) {
        let scope = self.scope();
        let mut walk = Walk {
            sel: Selector::new(self, &scope),
            f,
            path: Vec::new(),
            expanding: Vec::new(),
        };
        walk.ast(self, &[&scope], &[self]);
    }

    /// [`AST::for_each_mosfet_model`] for the subckt `name`, as in
    /// [`AST::mosfet_models_of`]. `false` if there is no such subckt.
    pub fn for_each_mosfet_model_of<'a>(
        &'a self,
        name: &str,
        f: impl FnMut(MosfetModel<'a, 's>),
    ) -> bool {
        let Some(def) = self.subckt.get(&name.to_lowercase()) else {
            return false;
        };
        let scope = self.scope();
        let child = instance_scope(&scope, def, &[]);
        let mut walk = Walk {
            sel: Selector::new(self, &scope),
            f,
            path: Vec::new(),
            expanding: vec![def],
        };
        walk.ast(&def.ast, &[&scope, &child], &[self, &def.ast]);
        true
    }
}

/// How many MOSFETs of a circuit use each model, counted through the
/// hierarchy: the summary of [`AST::mosfet_models`], for circuits too large
/// to list transistor by transistor (e.g. a 30 MB netlist whose hierarchy
/// holds 300 million MOSFETs). Each subckt instance that differs (by its
/// definition, and the parameters that its contents read) is evaluated
/// once; its counts are multiplied by the number of such instances.
#[derive(Debug, Clone, Default)]
pub struct ModelUsage<'a, 's> {
    /// the MOSFETs of the circuit. This count is too low if `errors` holds a
    /// [`SelectError::NoSubckt`] for a line whose definition holds MOSFETs.
    pub mosfets: u64,
    /// each model chosen, and how many MOSFETs use it: the most used first
    pub models: Vec<(&'a Model<'s>, u64)>,
    /// each reason why no model could be chosen, how many MOSFETs it
    /// concerns (for [`SelectError::NoSubckt`], how many `X` lines), and the
    /// path of one of them: the most frequent first
    pub errors: Vec<(SelectError, u64, String)>,
}

impl<'s> AST<'s> {
    /// How many MOSFETs of the circuit (the top level, and the subckt
    /// instances under it) use each model
    pub fn model_usage(&self) -> ModelUsage<'_, 's> {
        let scope = self.scope();
        let mut usage = Usage::new(self, &scope);
        let counts = usage.level(self, &[&scope], &[self], 0, 0);
        usage.finish(&counts)
    }

    /// [`AST::model_usage`] for an instance of the subckt `name` (defined at
    /// the top level) with its default parameters. `None` if there is no
    /// such subckt.
    pub fn model_usage_of(&self, name: &str) -> Option<ModelUsage<'_, 's>> {
        let def = self.subckt.get(&name.to_lowercase())?;
        let scope = self.scope();
        let child = instance_scope(&scope, def, &[]);
        let mut usage = Usage::new(self, &scope);
        usage.expanding.push(def);
        let visible = usage.visible_below(0, def);
        let counts = usage.level(&def.ast, &[&scope, &child], &[self, &def.ast], 1, visible);
        Some(usage.finish(&counts))
    }
}

/// What a MOSFET uses: a model (by address), or an error (by number)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Use {
    Model(usize),
    Error(usize),
}

type Counts = HashMap<Use, u64>;

/// The values passed on an instance line: name, and the bits of the value
/// that name has in the new instance (or the error that computing it gave)
type Passed = Vec<(String, Result<u64, String>)>;

/// The values passed on the instance line `line`, of the new instance `child`
/// (`None` if the line passes nothing); only those of `only`, if given
fn passed_values(
    child: Option<&Scope<'_>>,
    line: &[KeyValue<'_>],
    only: Option<&HashSet<String>>,
) -> Passed {
    let Some(child) = child else {
        return Passed::new();
    };
    line.iter()
        .map(|kv| kv.k.to_ascii_lowercase())
        .filter(|k| only.is_none_or(|only| only.contains(k)))
        .map(|k| {
            let v = child.get(&k).map(f64::to_bits).map_err(|e| e.to_string());
            (k, v)
        })
        .collect()
}

/// The parameters that the contents of a subckt read (its own, and those of
/// the instances under it)
#[derive(Debug)]
struct Reads {
    /// every parameter that a level of an instance of it reads from the
    /// instance's own level or above: the values passed on its line that
    /// matter
    all: HashSet<String>,
    /// those read from above the instance's level, in a fixed order
    outside: Vec<String>,
}

/// What the counts of a subckt instance depend on
#[derive(Debug, PartialEq, Eq, Hash)]
enum MemoKey {
    /// A definition whose contents read known parameters: the definitions
    /// its lines can see above it (see [`Usage::visible_below`]), the values
    /// passed that they read, and how the parameters they read from above
    /// are bound there
    Reads(usize, usize, Passed, Vec<Binding>),
    /// Any other (its contents call a function defined with `.param f(x)`, a
    /// bin of an instance above reads parameters, or a subckt instantiates
    /// itself): the values passed, and the context
    Context(usize, Passed, usize),
}

/// The counting walk of [`AST::model_usage`]
struct Usage<'a, 's> {
    sel: Selector<'a, 's>,
    parhier: ParHier,
    models: HashMap<usize, &'a Model<'s>>,
    /// errors, numbered; with the path of the first MOSFET that had each
    errors: Vec<(SelectError, String)>,
    error_numbers: HashMap<String, usize>,
    /// the counts of the subckt instances evaluated
    memo: HashMap<MemoKey, Rc<[(Use, u64)]>>,
    /// how often the counts of a definition (by address) were found in, or
    /// missing from, `memo`: (found, missing)
    memo_use: HashMap<usize, (u64, u64)>,
    /// the definitions whose instances are not kept in `memo`: almost each
    /// one is different (a value passed that differs on every instance, read
    /// below), so that keeping them would only take memory
    unkept: HashSet<usize>,
    /// the context of the next level that passes or defines parameters, or
    /// defines models or subckts (a level that does none of these sees the
    /// context above it). A level whose instance repeats one already
    /// evaluated is found in `memo`, so each context evaluated is new.
    next_context: usize,
    /// the numbers of the lists of definitions that hold models or subckts,
    /// on the instance paths seen: by the number of the list above and the
    /// definition (by address)
    visible: HashMap<(usize, usize), usize>,
    /// what the contents of a definition (by address), under a list of
    /// definitions (by number), read (`None`: not known)
    reads: HashMap<(usize, usize), Option<Rc<Reads>>>,
    /// the definitions whose reads are being found
    reading: Vec<usize>,
    path: Vec<&'a str>,
    expanding: Vec<&'a Subckt<'s>>,
}

impl<'a, 's> Usage<'a, 's> {
    fn new(ast: &AST<'_>, scope: &Scope<'_>) -> Self {
        Self {
            sel: Selector::new(ast, scope),
            parhier: scope.parhier(),
            models: HashMap::new(),
            errors: Vec::new(),
            error_numbers: HashMap::new(),
            memo: HashMap::new(),
            memo_use: HashMap::new(),
            unkept: HashSet::new(),
            next_context: 2,
            visible: HashMap::new(),
            reads: HashMap::new(),
            reading: Vec::new(),
            path: Vec::new(),
            expanding: Vec::new(),
        }
    }

    fn finish(self, counts: &Counts) -> ModelUsage<'a, 's> {
        let mut usage = ModelUsage {
            mosfets: counts.values().sum(),
            ..Default::default()
        };
        let mut errors: Vec<_> = self.errors.into_iter().map(Some).collect();
        for (&u, &n) in counts {
            match u {
                Use::Model(addr) => usage.models.push((self.models[&addr], n)),
                Use::Error(number) => {
                    let (e, path) = errors[number].take().expect("each error is counted once");
                    // (an `X` line is not a MOSFET)
                    if matches!(e, SelectError::NoSubckt(_)) {
                        usage.mosfets -= n;
                    }
                    usage.errors.push((e, n, path));
                }
            }
        }
        usage
            .models
            .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));
        usage
            .errors
            .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
        usage
    }

    /// What the lines of a level see, as a number: the definitions on its
    /// instance path that hold models or subckts. `visible`: the number of
    /// the level above; `def`: this level's definition. Two levels with the
    /// same number find the same definition for each name.
    fn visible_below(&mut self, visible: usize, def: &Subckt<'_>) -> usize {
        if def.ast.model.is_empty() && def.ast.subckt.is_empty() {
            return visible;
        }
        let next = self.visible.len() + 1;
        *self
            .visible
            .entry((visible, ptr::from_ref(def) as usize))
            .or_insert(next)
    }

    /// What the contents of `def` read, for an instance of it under a level
    /// with the definitions `on_path` and the number `visible`; `None` if not
    /// known
    fn reads(
        &mut self,
        def: &'a Subckt<'s>,
        on_path: &[&'a AST<'s>],
        visible: usize,
    ) -> Option<Rc<Reads>> {
        let addr = ptr::from_ref(def) as usize;
        if let Some(reads) = self.reads.get(&(addr, visible)) {
            return reads.clone();
        }
        // a subckt that instantiates itself, or too deep a hierarchy
        if self.reading.contains(&addr) || self.reading.len() >= MAX_HIERARCHY_DEPTH {
            return None;
        }
        self.reading.push(addr);
        let reads = self.find_reads(def, on_path, visible).map(Rc::new);
        self.reading.pop();
        self.reads.insert((addr, visible), reads.clone());
        reads
    }

    fn find_reads(
        &mut self,
        def: &'a Subckt<'s>,
        on_path: &[&'a AST<'s>],
        visible: usize,
    ) -> Option<Reads> {
        let mut all = IndexSet::new();
        // (a call of a function makes what is read unknown)
        let definitions = def
            .params
            .iter()
            .chain(&def.ast.param)
            .filter(|kv| function_name(&kv.k).is_none());
        for kv in definitions.clone() {
            extend(&mut all, &self.sel.names(&kv.v)?);
        }
        let inner: Vec<_> = on_path.iter().copied().chain(once(&def.ast)).collect();
        let inner_visible = self.visible_below(visible, def);
        for inst in &def.ast.instance {
            match &inst.ctx {
                InstanceCtx::MOSFET(m) => {
                    for kv in &m.params {
                        if SIZES.iter().any(|k| kv.k.eq_ignore_ascii_case(k)) {
                            extend(&mut all, &self.sel.names(&kv.v)?);
                        }
                    }
                    let line = self.sel.read_line(m, &inner);
                    for v in line.values() {
                        extend(&mut all, &self.sel.names(&v)?);
                    }
                    // the ranges of the bins it may use, read where each is
                    // defined: those of the top level read the top level's
                    // parameters (the same everywhere), those of this subckt
                    // its own; those of an instance above it the parameters
                    // of that instance, which are not followed: unknown
                    let name = line.name().to_ascii_lowercase();
                    let last = inner.len() - 1;
                    for (depth, &ast) in inner.iter().enumerate().skip(1) {
                        let index = self.sel.index(ast);
                        for bin in index.bins.get(&name).into_iter().flatten() {
                            let names = self.sel.limit_names(bin.model)?;
                            if depth < last && !names.is_empty() {
                                return None;
                            }
                            extend(&mut all, &names);
                        }
                    }
                }
                InstanceCtx::Subckt(x) => {
                    for kv in &x.params {
                        extend(&mut all, &self.sel.names(&kv.v)?);
                    }
                    if let Some(child) = resolve(&x.cktname, &inner) {
                        extend(&mut all, &self.reads(child, &inner, inner_visible)?.outside);
                    }
                }
                _ => {}
            }
        }
        let mut outside: Vec<String> = all.iter().cloned().collect();
        if self.parhier == ParHier::Local {
            // the closest definition is used: one of this level hides those
            // above
            let here: HashSet<String> = definitions.map(|kv| kv.k.to_ascii_lowercase()).collect();
            outside.retain(|n| !here.contains(n));
        }
        Some(Reads {
            all: all.into_iter().collect(),
            outside,
        })
    }

    /// The counts of the instances of `ast` (context `context`, seeing the
    /// definitions numbered `visible`); `scopes` and `on_path` as in
    /// [`Walk::ast`]
    fn level(
        &mut self,
        ast: &'a AST<'s>,
        scopes: &[&Scope<'_>],
        on_path: &[&'a AST<'s>],
        context: usize,
        visible: usize,
    ) -> Counts {
        let scope = *scopes.last().expect("the top level at least");
        let mut counts = Counts::new();
        for inst in &ast.instance {
            match &inst.ctx {
                InstanceCtx::MOSFET(m) => {
                    let used = match self.sel.select(m, scopes, on_path) {
                        Ok(s) => {
                            let addr = ptr::from_ref(s.model) as usize;
                            self.models.entry(addr).or_insert(s.model);
                            Use::Model(addr)
                        }
                        Err(e) => Use::Error(self.error_number(e, &inst.name)),
                    };
                    *counts.entry(used).or_default() += 1;
                }
                InstanceCtx::Subckt(x) => {
                    let Some(def) = resolve(&x.cktname, on_path) else {
                        let e = SelectError::NoSubckt(x.cktname.to_string());
                        let used = Use::Error(self.error_number(e, &inst.name));
                        *counts.entry(used).or_default() += 1;
                        continue;
                    };
                    if self.expanding.iter().any(|s| ptr::eq(*s, def))
                        || self.expanding.len() >= MAX_HIERARCHY_DEPTH
                    {
                        continue;
                    }
                    let addr = ptr::from_ref(def) as usize;
                    // (a line that passes nothing needs no new instance to
                    // look in the memo)
                    let child =
                        (!x.params.is_empty()).then(|| instance_scope(scope, def, &x.params));
                    let key = if self.unkept.contains(&addr) {
                        None
                    } else {
                        let key = match self.reads(def, on_path, visible) {
                            Some(reads) => MemoKey::Reads(
                                addr,
                                visible,
                                passed_values(child.as_ref(), &x.params, Some(&reads.all)),
                                reads.outside.iter().map(|n| scope.binding(n)).collect(),
                            ),
                            None => MemoKey::Context(
                                addr,
                                passed_values(child.as_ref(), &x.params, None),
                                context,
                            ),
                        };
                        if let Some(c) = self.memo.get(&key) {
                            add(&mut counts, c);
                            self.memo_use.entry(addr).or_default().0 += 1;
                            continue;
                        }
                        Some(key)
                    };
                    let binds = !x.params.is_empty()
                        || !def.params.is_empty()
                        || !def.ast.param.is_empty()
                        || !def.ast.param_func.is_empty()
                        || !def.ast.model.is_empty()
                        || !def.ast.subckt.is_empty();
                    let inner_context = if binds {
                        self.next_context += 1;
                        self.next_context
                    } else {
                        context
                    };
                    let inner_visible = self.visible_below(visible, def);
                    let child = child.unwrap_or_else(|| instance_scope(scope, def, &[]));
                    let inner_scopes: Vec<_> = scopes.iter().copied().chain(once(&child)).collect();
                    let inner: Vec<_> = on_path.iter().copied().chain(once(&def.ast)).collect();
                    self.path.push(&inst.name);
                    self.expanding.push(def);
                    let c = self.level(
                        &def.ast,
                        &inner_scopes,
                        &inner,
                        inner_context,
                        inner_visible,
                    );
                    self.expanding.pop();
                    self.path.pop();
                    let c: Rc<[(Use, u64)]> = c.into_iter().collect();
                    add(&mut counts, &c);
                    if let Some(key) = key {
                        self.keep(addr, key, c);
                    }
                }
                _ => {}
            }
        }
        counts
    }

    /// Keeps the counts `c` of an instance of the definition `addr`, unless
    /// its instances seldom repeat
    fn keep(&mut self, addr: usize, key: MemoKey, c: Rc<[(Use, u64)]>) {
        /// instances of a definition evaluated before judging how often
        /// they repeat
        const TRIAL: u64 = 4096;
        /// how many entries `memo` holds at most (it is emptied then)
        const MAX_ENTRIES: usize = 1 << 22;
        let (found, missing) = self.memo_use.entry(addr).or_default();
        *missing += 1;
        if *missing >= TRIAL && *found * 16 < *missing {
            self.unkept.insert(addr);
            return;
        }
        if self.memo.len() >= MAX_ENTRIES {
            self.memo.clear();
        }
        self.memo.insert(key, c);
    }

    /// The number of the error `e`, met at the instance `name` of the
    /// current level
    fn error_number(&mut self, e: SelectError, name: &str) -> usize {
        let text = e.to_string();
        if let Some(&n) = self.error_numbers.get(&text) {
            return n;
        }
        let n = self.errors.len();
        let path = self.path.iter().copied().chain(once(name)).join(".");
        self.errors.push((e, path));
        self.error_numbers.insert(text, n);
        n
    }
}

fn add(counts: &mut Counts, more: &[(Use, u64)]) {
    for &(u, n) in more {
        *counts.entry(u).or_default() += n;
    }
}

/// A walk down the instance hierarchy, calling `f` with each MOSFET
struct Walk<'a, 's, F> {
    sel: Selector<'a, 's>,
    f: F,
    /// the subckt instances above the current level
    path: Vec<&'a str>,
    /// the subckts being expanded (to stop at a subckt that instantiates itself)
    expanding: Vec<&'a Subckt<'s>>,
}

impl<'a, 's, F: FnMut(MosfetModel<'a, 's>)> Walk<'a, 's, F> {
    /// `on_path`: the definitions on the instance path of `ast`, from the top
    /// level to `ast` itself (where its models and subckts are looked up);
    /// `scopes`: their parameters, the top level's and those of the instances
    /// (`ast`'s last)
    fn ast(&mut self, ast: &'a AST<'s>, scopes: &[&Scope<'_>], on_path: &[&'a AST<'s>]) {
        let scope = *scopes.last().expect("the top level at least");
        for inst in &ast.instance {
            match &inst.ctx {
                InstanceCtx::MOSFET(m) => {
                    let selection = self.sel.select(m, scopes, on_path);
                    let mname = self.sel.read_line(m, on_path).name();
                    (self.f)(MosfetModel {
                        path: self.path.iter().copied().chain(once(&*inst.name)).join("."),
                        instance: inst,
                        mname,
                        selection,
                    });
                }
                InstanceCtx::Subckt(x) => {
                    let Some(def) = resolve(&x.cktname, on_path) else {
                        (self.f)(MosfetModel {
                            path: self.path.iter().copied().chain(once(&*inst.name)).join("."),
                            instance: inst,
                            mname: &x.cktname,
                            selection: Err(SelectError::NoSubckt(x.cktname.to_string())),
                        });
                        continue;
                    };
                    if self.expanding.iter().any(|s| ptr::eq(*s, def))
                        || self.expanding.len() >= MAX_HIERARCHY_DEPTH
                    {
                        continue;
                    }
                    let child = instance_scope(scope, def, &x.params);
                    let inner_scopes: Vec<_> = scopes.iter().copied().chain(once(&child)).collect();
                    let inner: Vec<_> = on_path.iter().copied().chain(once(&def.ast)).collect();
                    self.path.push(&inst.name);
                    self.expanding.push(def);
                    self.ast(&def.ast, &inner_scopes, &inner);
                    self.expanding.pop();
                    self.path.pop();
                }
                _ => {}
            }
        }
    }
}
