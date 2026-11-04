//! The model HSPICE uses for each MOSFET (its "model selector").
//!
//! A MOSFET names a model, e.g. `nch`. If a model of that name is defined,
//! it is the one. Otherwise HSPICE chooses among the bins of a binned model
//! library, the models named `nch.1`, `nch.2`, ..., the one whose ranges hold
//! the transistor's length and width:
//!
//! ``` text
//! LMIN <= L < LMAX   and   WMIN <= W < WMAX
//! ```
//!
//! (a limit that is not given does not limit). Details, as in HSPICE:
//! - A model defined inside a `.subckt` is local to it, and hides the models
//!   of the same name outside.
//! - The limits are evaluated where the model is defined: those of a model
//!   of the top level with the top-level parameters (a subckt parameter of
//!   the same name does not change them), those of a model inside a subckt
//!   with the parameters of that subckt's instance (e.g.
//!   `LMIN='2.4e-7-(dxln+dlmisn)'` in a macro model).
//! - L and W are the instance's `L=` and `W=` (or `.option DEFL`/`DEFW`,
//!   default 100u), times `.option SCALE`; the limits are multiplied by
//!   `.option SCALM`. The width per finger (`W/NF`) is compared if `WNFLAG`
//!   is set: on the instance line, else in the model, else by `.option
//!   WNFLAG` (by default, the total width).
//! - A value within 1e-9 (relative) of a limit counts as equal to it, so
//!   that `L=0.1u` and `LMAX=1e-7` compare equal.

use crate::{
    AST, Subckt,
    ast::{Model, Value},
    eval::{Binding, EvalError, ParHier, Scope, expr_names, function_name},
    instance::{Instance, InstanceCtx, MOSFET},
    utlis::MAX_HIERARCHY_DEPTH,
};
use core::fmt;
use indexmap::IndexSet;
use itertools::Itertools as _;
use std::{
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
    /// A value (L, W, or a limit of a bin) could not be computed
    Eval { what: String, error: EvalError },
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
            Self::Eval { what, error } => write!(f, "{what}: {error}"),
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

/// A MOSFET of the circuit, with the model chosen for it
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
    /// `.option wnflag`: whether bins compare the width per finger when
    /// neither the model nor the instance says
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
            wnflag: get("wnflag", 0.0) != 0.0,
        }
    }
}

/// `min <= x < max`, a value within 1e-9 (relative) of a limit counting as
/// equal to it
fn within(x: f64, min: f64, max: f64) -> bool {
    let tol = |v: f64| v.abs() * 1e-9;
    (min == f64::NEG_INFINITY || x >= min - tol(min))
        && (max == f64::INFINITY || x < max - tol(max))
}

/// The ranges of a bin
#[derive(Debug, Clone, Copy)]
struct Limits {
    lmin: f64,
    lmax: f64,
    wmin: f64,
    wmax: f64,
    /// the model's `WNFLAG`: whether the width per finger is compared
    per_finger: Option<bool>,
}

impl Limits {
    /// Whether these ranges hold a transistor of size `size`
    fn hold(&self, size: &Size, opts: &Options) -> bool {
        // the instance's WNFLAG, else the model's, else `.option wnflag`
        let per_finger = size.wnflag.or(self.per_finger).unwrap_or(opts.wnflag);
        let w = if per_finger && size.nf > 0.0 {
            size.w / size.nf
        } else {
            size.w
        };
        within(size.l, self.lmin, self.lmax) && within(w, self.wmin, self.wmax)
    }
}

/// The parameters of a bin that its ranges are made of
const LIMITS: [&str; 5] = ["lmin", "lmax", "wmin", "wmax", "wnflag"];

/// The ranges of the bin `model`, evaluated in `scope`
fn limits(model: &Model<'_>, scope: &Scope<'_>, scalm: f64) -> Result<Limits, SelectError> {
    let param = |key: &str| -> Result<Option<f64>, SelectError> {
        let Some(kv) = model
            .params
            .iter()
            .rev()
            .find(|kv| kv.k.eq_ignore_ascii_case(key))
        else {
            return Ok(None);
        };
        scope
            .eval(&kv.v)
            .map(Some)
            .map_err(|error| SelectError::Eval {
                what: format!("{} of model `{}`", key.to_ascii_uppercase(), model.name),
                error,
            })
    };
    Ok(Limits {
        lmin: param("lmin")?.map_or(f64::NEG_INFINITY, |v| v * scalm),
        lmax: param("lmax")?.map_or(f64::INFINITY, |v| v * scalm),
        wmin: param("wmin")?.map_or(f64::NEG_INFINITY, |v| v * scalm),
        wmax: param("wmax")?.map_or(f64::INFINITY, |v| v * scalm),
        per_finger: param("wnflag")?.map(|flag| flag != 0.0),
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

/// The size of a MOSFET, evaluated in `scope`
fn size(m: &MOSFET<'_>, scope: &Scope<'_>, opts: &Options) -> Result<Size, SelectError> {
    let value = |key: &str| match m
        .params
        .iter()
        .rev()
        .find(|kv| kv.k.eq_ignore_ascii_case(key))
    {
        Some(kv) => scope
            .eval(&kv.v)
            .map(Some)
            .map_err(|error| SelectError::Eval {
                what: key.to_ascii_uppercase(),
                error,
            }),
        None => Ok(None),
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
    /// the model of each name (the last one, if one is defined twice)
    exact: HashMap<String, &'a Model<'s>>,
    /// the bins `name.N` of each `name`, in netlist order
    bins: HashMap<String, Vec<Bin<'a, 's>>>,
}

struct Bin<'a, 's> {
    model: &'a Model<'s>,
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
            // a bin of each name before a `.`: `a.b.1` of `a` and of `a.b`
            for (i, _) in name.match_indices('.') {
                if i > 0 && i + 1 < name.len() {
                    index
                        .bins
                        .entry(name[..i].to_owned())
                        .or_default()
                        .push(Bin {
                            model,
                            fixed: OnceCell::new(),
                        });
                }
            }
            index.exact.insert(name, model);
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

    /// The model named `name` for a transistor of length `l`, width `w` and
    /// `nf` fingers. `lexical`: the definitions around it, from the top level
    /// to its own, whose models it sees (the innermost first); `scopes`: the
    /// parameters of each (the top level, and the instances of the subckts
    /// above the transistor that they define). The ranges of a bin are
    /// evaluated where the bin is defined: those of the top level with the
    /// top-level parameters, those of a subckt with its instance's.
    fn choose(
        &mut self,
        name: &str,
        size: Size,
        scopes: &[&Scope<'_>],
        lexical: &[&'a AST<'s>],
    ) -> Result<Selection<'a, 's>, SelectError> {
        let Size { l, w, nf, .. } = size;
        let key = name.to_ascii_lowercase();
        for (&ast, &scope) in lexical.iter().zip(scopes).rev() {
            let index = self.index(ast);
            // a model of this very name
            if let Some(&model) = index.exact.get(&key) {
                return Ok(Selection { model, l, w, nf });
            }
            let Some(bins) = index.bins.get(&key) else {
                continue;
            };
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
            return Err(SelectError::NoBin {
                name: name.to_owned(),
                l,
                w,
                bins: bins.len(),
            });
        }
        Err(SelectError::NoModel(name.to_owned()))
    }

    /// The model of the MOSFET `m`, at the level whose definitions around it
    /// are `lexical` and their parameters `scopes` (its own last)
    fn select(
        &mut self,
        m: &MOSFET<'_>,
        scopes: &[&Scope<'_>],
        lexical: &[&'a AST<'s>],
    ) -> Result<Selection<'a, 's>, SelectError> {
        let scope = scopes.last().expect("the top level at least");
        let size = size(m, scope, &self.opts)?;
        self.choose(&m.mname, size, scopes, lexical)
    }
}

/// Adds the names of `more` that `names` lacks
fn extend(names: &mut IndexSet<String>, more: &[String]) {
    names.extend(more.iter().cloned());
}

/// The definition of the subckt `name`, seen from the level whose
/// definitions around it are `lexical` (the innermost one that has it); with
/// the definitions around it: `lexical` up to where it is defined, then its
/// own
fn resolve<'a, 's>(
    name: &str,
    lexical: &[&'a AST<'s>],
) -> Option<(&'a Subckt<'s>, Vec<&'a AST<'s>>)> {
    let key = name.to_lowercase();
    let (depth, def) = lexical
        .iter()
        .enumerate()
        .rev()
        .find_map(|(d, a)| a.subckt.get(&key).map(|s| (d, s)))?;
    let inner = lexical[..=depth]
        .iter()
        .copied()
        .chain(once(&def.ast))
        .collect();
    Some((def, inner))
}

/// The parameters of an instance of `def` whose line passes `passed` (each
/// value computed where the instance is): its `.subckt` defaults and `.param`
/// statements, and the values passed
fn instance_scope<'p>(
    scope: &'p Scope<'p>,
    def: &Subckt<'_>,
    passed: impl IntoIterator<Item = (&'p str, Result<f64, EvalError>)>,
) -> Scope<'p> {
    let mut child = scope.child();
    for kv in def.params.iter().chain(&def.ast.param) {
        child.define(kv);
    }
    for (k, v) in passed {
        child.pass(k, v);
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
        let child = instance_scope(&scope, def, []);
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
    /// the MOSFETs of the circuit
    pub mosfets: u64,
    /// each model chosen, and how many MOSFETs use it: the most used first
    pub models: Vec<(&'a Model<'s>, u64)>,
    /// each reason why no model could be chosen, how many MOSFETs it
    /// concerns, and the path of one of them: the most frequent first
    pub errors: Vec<(SelectError, u64, String)>,
}

impl<'s> AST<'s> {
    /// How many MOSFETs of the circuit (the top level, and the subckt
    /// instances under it) use each model
    pub fn model_usage(&self) -> ModelUsage<'_, 's> {
        let scope = self.scope();
        let mut usage = Usage::new(self, &scope);
        let counts = usage.level(self, &[&scope], &[self], 0);
        usage.finish(&counts)
    }

    /// [`AST::model_usage`] for an instance of the subckt `name` (defined at
    /// the top level) with its default parameters. `None` if there is no
    /// such subckt.
    pub fn model_usage_of(&self, name: &str) -> Option<ModelUsage<'_, 's>> {
        let def = self.subckt.get(&name.to_lowercase())?;
        let scope = self.scope();
        let child = instance_scope(&scope, def, []);
        let mut usage = Usage::new(self, &scope);
        usage.expanding.push(def);
        let counts = usage.level(&def.ast, &[&scope, &child], &[self, &def.ast], 1);
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

/// The values passed on an instance line: name, and the value's bits (or
/// the error that computing it gave)
type Passed = Vec<(String, Result<u64, String>)>;

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
    /// A definition whose contents read known parameters: the values
    /// passed that they read, and how the parameters they read from above
    /// are bound there
    Reads(usize, Passed, Vec<Binding>),
    /// Any other (its contents call a function defined with `.param f(x)`,
    /// or a subckt instantiates itself): the values passed, and the context
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
    memo: HashMap<MemoKey, Rc<Counts>>,
    /// parameter contexts: a level that passes or defines parameters gets a
    /// new one; other levels see the parameters of the level above
    contexts: HashMap<(usize, Passed, usize), usize>,
    /// what the contents of a definition (by address) read (`None`: not
    /// known)
    reads: HashMap<usize, Option<Rc<Reads>>>,
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
            contexts: HashMap::new(),
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

    /// What the contents of `def`, whose definitions around it are
    /// `lexical`, read; `None` if not known
    fn reads(&mut self, def: &'a Subckt<'s>, lexical: &[&'a AST<'s>]) -> Option<Rc<Reads>> {
        let addr = ptr::from_ref(def) as usize;
        if let Some(reads) = self.reads.get(&addr) {
            return reads.clone();
        }
        // a subckt that instantiates itself, or too deep a hierarchy
        if self.reading.contains(&addr) || self.reading.len() >= MAX_HIERARCHY_DEPTH {
            return None;
        }
        self.reading.push(addr);
        let reads = self.find_reads(def, lexical).map(Rc::new);
        self.reading.pop();
        self.reads.insert(addr, reads.clone());
        reads
    }

    fn find_reads(&mut self, def: &'a Subckt<'s>, lexical: &[&'a AST<'s>]) -> Option<Reads> {
        let mut all = IndexSet::new();
        // a function's body is read where it is called (and a call makes
        // what is read unknown)
        let definitions = def
            .params
            .iter()
            .chain(&def.ast.param)
            .filter(|kv| function_name(&kv.k).is_none());
        for kv in definitions.clone() {
            extend(&mut all, &self.sel.names(&kv.v)?);
        }
        for inst in &def.ast.instance {
            match &inst.ctx {
                InstanceCtx::MOSFET(m) => {
                    for kv in &m.params {
                        if ["l", "w", "nf", "wnflag"]
                            .iter()
                            .any(|k| kv.k.eq_ignore_ascii_case(k))
                        {
                            extend(&mut all, &self.sel.names(&kv.v)?);
                        }
                    }
                    // the ranges of the bins it may use, read where each is
                    // defined: those of the top level read the top level's
                    // parameters (the same everywhere), those of this subckt
                    // its own; those of a subckt around it (a definition
                    // nested in another) the parameters of that subckt's
                    // instance, which are not followed: unknown
                    let name = m.mname.to_ascii_lowercase();
                    let last = lexical.len() - 1;
                    for (depth, &ast) in lexical.iter().enumerate().skip(1) {
                        let index = self.sel.index(ast);
                        for bin in index.bins.get(&name).into_iter().flatten() {
                            if depth < last {
                                return None;
                            }
                            extend(&mut all, &self.sel.limit_names(bin.model)?);
                        }
                    }
                }
                InstanceCtx::Subckt(x) => {
                    for kv in &x.params {
                        extend(&mut all, &self.sel.names(&kv.v)?);
                    }
                    if let Some((child, inner)) = resolve(&x.cktname, lexical) {
                        extend(&mut all, &self.reads(child, &inner)?.outside);
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

    /// The counts of the instances of `ast` (context `context`); `scopes`
    /// and `lexical` as in [`Walk::ast`]
    fn level(
        &mut self,
        ast: &'a AST<'s>,
        scopes: &[&Scope<'_>],
        lexical: &[&'a AST<'s>],
        context: usize,
    ) -> Counts {
        let scope = *scopes.last().expect("the top level at least");
        let mut counts = Counts::new();
        for inst in &ast.instance {
            match &inst.ctx {
                InstanceCtx::MOSFET(m) => {
                    let used = match self.sel.select(m, scopes, lexical) {
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
                    let Some((def, inner)) = resolve(&x.cktname, lexical) else {
                        continue;
                    };
                    if self.expanding.iter().any(|s| ptr::eq(*s, def))
                        || self.expanding.len() >= MAX_HIERARCHY_DEPTH
                    {
                        continue;
                    }
                    let values: Vec<_> = x.params.iter().map(|kv| scope.eval(&kv.v)).collect();
                    let passed: Passed = x
                        .params
                        .iter()
                        .zip(&values)
                        .map(|(kv, v)| {
                            let v = v.as_ref().map(|f| f.to_bits()).map_err(|e| e.to_string());
                            (kv.k.to_ascii_lowercase(), v)
                        })
                        .collect();
                    let addr = ptr::from_ref(def) as usize;
                    let key = match self.reads(def, &inner) {
                        Some(reads) => MemoKey::Reads(
                            addr,
                            passed
                                .iter()
                                .filter(|(k, _)| reads.all.contains(k))
                                .cloned()
                                .collect(),
                            reads.outside.iter().map(|n| scope.binding(n)).collect(),
                        ),
                        None => MemoKey::Context(addr, passed.clone(), context),
                    };
                    if let Some(c) = self.memo.get(&key) {
                        add(&mut counts, c);
                        continue;
                    }
                    let binds =
                        !passed.is_empty() || !def.params.is_empty() || !def.ast.param.is_empty();
                    let inner_context = if binds {
                        let next = self.contexts.len() + 2;
                        *self.contexts.entry((addr, passed, context)).or_insert(next)
                    } else {
                        context
                    };
                    let child =
                        instance_scope(scope, def, x.params.iter().map(|kv| &*kv.k).zip(values));
                    let depth = inner.len() - 2;
                    let inner_scopes: Vec<_> = scopes[..=depth]
                        .iter()
                        .copied()
                        .chain(once(&child))
                        .collect();
                    self.path.push(&inst.name);
                    self.expanding.push(def);
                    let c = Rc::new(self.level(&def.ast, &inner_scopes, &inner, inner_context));
                    self.expanding.pop();
                    self.path.pop();
                    add(&mut counts, &c);
                    self.memo.insert(key, c);
                }
                _ => {}
            }
        }
        counts
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

fn add(counts: &mut Counts, more: &Counts) {
    for (&u, &n) in more {
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
    /// `lexical`: the definitions around `ast`, from the top level to `ast`
    /// itself (where its models and subckts are looked up); `scopes`: their
    /// parameters, the top level's and those of the instances (above, and
    /// of `ast` last) that they define
    fn ast(&mut self, ast: &'a AST<'s>, scopes: &[&Scope<'_>], lexical: &[&'a AST<'s>]) {
        let scope = *scopes.last().expect("the top level at least");
        for inst in &ast.instance {
            match &inst.ctx {
                InstanceCtx::MOSFET(m) => {
                    let selection = self.sel.select(m, scopes, lexical);
                    (self.f)(MosfetModel {
                        path: self.path.iter().copied().chain(once(&*inst.name)).join("."),
                        instance: inst,
                        mname: &m.mname,
                        selection,
                    });
                }
                InstanceCtx::Subckt(x) => {
                    let Some((def, inner)) = resolve(&x.cktname, lexical) else {
                        continue;
                    };
                    if self.expanding.iter().any(|s| ptr::eq(*s, def))
                        || self.expanding.len() >= MAX_HIERARCHY_DEPTH
                    {
                        continue;
                    }
                    // the values passed on the instance line are computed
                    // where the instance is
                    let child = instance_scope(
                        scope,
                        def,
                        x.params.iter().map(|kv| (&*kv.k, scope.eval(&kv.v))),
                    );
                    let depth = inner.len() - 2;
                    let inner_scopes: Vec<_> = scopes[..=depth]
                        .iter()
                        .copied()
                        .chain(once(&child))
                        .collect();
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
