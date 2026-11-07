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
//! - A level's `.if` blocks are evaluated for each instance (see [`crate::eval`]).
//!   The statements of the active branch take their place in the level: a model
//!   or subckt in it serves the whole level, also a line above the block, and
//!   bins keep the order of the file. Other branches are not read. A condition
//!   that cannot be computed is an error ([`SelectError::Condition`]): HSPICE
//!   stops there.
//! - The limits are evaluated where the model is defined: those of a model
//!   of the top level with the top-level parameters (a subckt parameter of
//!   the same name does not change them), those of a model inside a subckt
//!   with the parameters of that subckt's instance (e.g.
//!   `LMIN='2.4e-7-(dxln+dlmisn)'` in a macro model).
//! - L and W are the instance's `L=` and `W=` or the first two values after
//!   its model name (else `.option DEFL`/`DEFW`, default 100u), times
//!   `.option SCALE`; the limits are multiplied by `.option SCALM`. HSPICE
//!   assigns the plain numbers of a line first and then its expressions, each in
//!   the order of the line, and the last one stays: `1u k w=5u` has W=k, and
//!   `1u 2u w=5u` has W=5u. So an expression wins over a number. The parentheses
//!   of a line only separate words, also after a key: `w=(2u + 3u)` is 2u, and
//!   `w=(2u+3u)` is 5u. A lone `+` takes a place and gives no value, as `OFF`
//!   does. The values after `ic=` belong to it. The width
//!   per finger (`W/NF`) is compared, unless `WNFLAG=0` on the instance line
//!   or, without one, `.option WNFLAG=0`. A `WNFLAG` in a model is ignored.
//! - A value at most 2e-15 (relative) below a limit counts as on it, so that
//!   `L='0.1u*3'` and `LMAX=3e-7` compare equal. (Measured: 1e-15 below does,
//!   3e-15 below does not, at limits from 1e-8 to 1e-4.)
//! - A FinFET bin (level 72) reads `nfin*` and `wsheet*`, not `wmin` and `wmax`.
//!   A line with no `nfin` reads 1. A bin with `wsheet*` needs a sheet width above
//!   0 on the line.
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
//!
//! Other known limits (`golden/NOTES.md`, "Comparison rounds with HSPICE as the
//! reference", has the decks):
//! - HSPICE takes the fourth word as the model when another active line uses that
//!   word as a model. This crate looks at the definitions only.
//! - After the first use of a FinFET bin, HSPICE multiplies its `nfin` limits by
//!   `.option scalm`. So under `scalm` its choice can depend on the order of the
//!   lines. This crate uses the limits as written.
//! - HSPICE reads a parameter at most 31 levels above the reader, and a model at
//!   most 99 levels above. This crate has no such limits.
//! - Some `.if` conditions of one block give a branch in HSPICE that depends on
//!   the order of the instances. This crate gives the logical branch.

use crate::{
    AST, Subckt,
    ast::{KeyValue, Model, ModelType, Value},
    eval::{
        Active, Binding, CondError, EvalError, FarRead, HSPICE_READ_REACH, ParHier, Scope,
        expr_names, function_name, in_order, lower,
    },
    hierarchy::MAX_HIERARCHY_DEPTH,
    instance::{Instance, InstanceCtx, MOSFET, is_key_place, is_off, plain_word},
};
use core::fmt::{self, Write as _};
use indexmap::IndexSet;
use itertools::Itertools as _;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::{borrow::Cow, cell::OnceCell, cmp::Reverse, iter::once, ptr, rc::Rc};

mod short;
pub use short::Short;

mod summary;
pub use summary::ModelUsage;

mod bins;
use bins::{
    LIMITS, Limits, MAX_MOSFET_NODES, MosLine, Options, SIZES, Size, bin_across, limits, size,
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
    /// The condition of an `.if` or `.elseif` line could not be computed.
    /// HSPICE stops there. This error does not concern a MOSFET: the block
    /// counts no line.
    Condition { cond: String, error: EvalError },
    /// A size of a MOSFET that is not positive (L, M), or negative (W, NF).
    /// HSPICE stops there (decks `m_l_neg`, `m_l_zero`, `m_w_neg`, `m_nf_neg`,
    /// `m_m_zero`).
    BadSize { what: &'static str, value: f64 },
    /// An `X` line whose node count is not that of the subckt. HSPICE stops
    /// there. The MOSFETs below the line are not counted.
    NodeCount {
        subckt: String,
        ports: usize,
        nodes: usize,
    },
    /// An `X` line inside the definition that it calls. HSPICE stops there. The
    /// MOSFETs below the line are not counted.
    Recursive(String),
    /// An `X` line below [`MAX_HIERARCHY_DEPTH`] levels. HSPICE reads deeper
    /// hierarchies. The MOSFETs below the line are not counted.
    TooDeep(String),
}

impl SelectError {
    /// Whether this row stands for a MOSFET: the other ones stand for a line
    /// that holds an unknown number of them, or none
    pub fn is_mosfet(&self) -> bool {
        !matches!(
            self,
            Self::NoSubckt(_)
                | Self::Condition { .. }
                | Self::NodeCount { .. }
                | Self::Recursive(_)
                | Self::TooDeep(_)
        )
    }
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
                "model `{model}` has a `.` in its name: it must set LMIN, LMAX, WMIN and WMAX \
                 (at level 72: LMIN, LMAX, and NFINMIN and NFINMAX, or WSHEETMIN and WSHEETMAX)"
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
            Self::Condition { cond, error } => write!(
                f,
                "the `.if` condition `{cond}` cannot be computed ({error}): the block is not counted"
            ),
            Self::BadSize { what, value } => {
                write!(f, "{what}={}: HSPICE stops at this size", Short(*value))
            }
            Self::NodeCount {
                subckt,
                ports,
                nodes,
            } => write!(
                f,
                "X line with {nodes} nodes, and subckt `{subckt}` has {ports}: \
                 the MOSFETs below it are not counted"
            ),
            Self::Recursive(name) => write!(
                f,
                "X line inside the definition of subckt `{name}`, which it calls: \
                 the MOSFETs below it are not counted"
            ),
            Self::TooDeep(name) => write!(
                f,
                "X line of subckt `{name}` below {MAX_HIERARCHY_DEPTH} levels: \
                 the MOSFETs below it are not counted"
            ),
        }
    }
}

impl std::error::Error for SelectError {}

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
    /// whether it sets the limits of its level
    complete: bool,
    /// its ranges if they read no parameter (they are the same everywhere),
    /// once computed
    fixed: OnceCell<Option<Result<Limits, SelectError>>>,
}

impl<'a, 's> ModelIndex<'a, 's> {
    fn new(models: impl IntoIterator<Item = &'a Model<'s>>) -> Self {
        let mut index = Self {
            exact: HashMap::default(),
            bins: HashMap::default(),
        };
        for model in models {
            // (no other type serves an M line, and it hides nothing: decks
            // `mt_*`, `mdl_diode_on_m`)
            if !matches!(model.model_type, ModelType::NMOS | ModelType::PMOS) {
                continue;
            }
            let name = model.name.to_ascii_lowercase();
            match name.split_once('.') {
                None => {
                    index.exact.insert(name, model);
                }
                // a bin of `base`, any suffix; `a.b.1` is of no use
                Some((base, suffix))
                    if !base.is_empty() && !suffix.is_empty() && !suffix.contains('.') =>
                {
                    index.bins.entry(base.to_owned()).or_default().push(Bin {
                        model,
                        complete: bin_across(model).is_some(),
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
    let mut names = HashSet::default();
    let mut stack = vec![ast];
    while let Some(a) = stack.pop() {
        names.extend(a.param.iter().filter_map(|kv| function_name(kv.k)));
        names.extend(a.param_func.iter().map(|f| f.name.to_ascii_lowercase()));
        for def in a.subckt.values() {
            names.extend(def.params.iter().filter_map(|kv| function_name(kv.k)));
            stack.push(&def.ast);
        }
        for block in &a.cond {
            stack.extend(block.branches.iter().map(|branch| &branch.ast));
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
    /// the models of a level with active `.if` branches (by the addresses of
    /// the level and the branches), in netlist order
    merged: HashMap<Vec<usize>, Rc<ModelIndex<'a, 's>>>,
    /// the parameters that an expression reads (`None`: not known)
    names: HashMap<String, Option<Rc<[String]>>>,
    /// the parameters that the ranges of a bin (by address) read
    limit_names: HashMap<usize, Option<Rc<[String]>>>,
    /// the ranges of a bin, for the bindings of the parameters they read
    limits: HashMap<(usize, Vec<Binding>), Result<Limits, SelectError>>,
}

impl<'a, 's> Selector<'a, 's> {
    fn new(ast: &'a AST<'s>, active: &[&'a AST<'s>]) -> Self {
        Self {
            opts: Options::new(ast, active),
            functions: function_names(ast),
            index: HashMap::default(),
            merged: HashMap::default(),
            names: HashMap::default(),
            limit_names: HashMap::default(),
            limits: HashMap::default(),
        }
    }

    fn index(&mut self, ast: &'a AST<'s>) -> Rc<ModelIndex<'a, 's>> {
        self.index
            .entry(ptr::from_ref(ast) as usize)
            .or_insert_with(|| Rc::new(ModelIndex::new(&ast.model)))
            .clone()
    }

    /// The models of one level: `group` is its definition, then its active
    /// branches
    fn group_index(&mut self, group: &[&'a AST<'s>]) -> Rc<ModelIndex<'a, 's>> {
        let [ast, branches @ ..] = group else {
            unreachable!("a level has a definition");
        };
        if branches.is_empty() {
            return self.index(ast);
        }
        let key: Vec<usize> = group.iter().map(|a| ptr::from_ref(*a) as usize).collect();
        self.merged
            .entry(key)
            .or_insert_with(|| {
                let models = in_order(ast, branches, &|a| &a.model, &|b| b.model_at);
                Rc::new(ModelIndex::new(models))
            })
            .clone()
    }

    /// The parameters that the value `v` reads (`None`: not known)
    fn names(&mut self, v: &Value<'_>) -> Option<Rc<[String]>> {
        match v {
            Value::Expr(e) => self.text_names(e),
            Value::Num(_) => Some(Rc::from([])),
        }
    }

    /// The parameters that the expression `e` reads (`None`: not known)
    fn text_names(&mut self, e: &str) -> Option<Rc<[String]>> {
        if let Some(names) = self.names.get(e) {
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
    /// definitions on its instance path, from the top level to its own, each
    /// with its active `.if` branches after it; `scopes`: the parameters of each,
    /// the top level and the instances (the same one for a level's entries). The
    /// ranges of a bin are evaluated at the level that defines it.
    fn choose(
        &mut self,
        name: &str,
        size: Size,
        scopes: &[&Scope<'_>],
        on_path: &[&'a AST<'s>],
    ) -> Result<Selection<'a, 's>, SelectError> {
        let Size { l, w, nf, .. } = size;
        // (a bin divides W by a smaller NF, and the circuit has one finger:
        // decks `mx_nf_*`)
        let nf = nf.max(1.0);
        let key = lower(name);
        let key: &str = &key;
        let mut seen = 0;
        let mut end = on_path.len();
        while end > 0 {
            let scope = scopes[end - 1];
            let start = scopes[..end]
                .iter()
                .rposition(|s| !ptr::eq(*s, scope))
                .map_or(0, |k| k + 1);
            let index = self.group_index(&on_path[start..end]);
            end = start;
            // a model of this very name
            if let Some(&model) = index.exact.get(key) {
                return Ok(Selection { model, l, w, nf });
            }
            let Some(bins) = index.bins.get(key) else {
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
            let key = lower(word);
            let key: &str = &key;
            on_path.iter().any(|&ast| {
                let index = self.index(ast);
                index.exact.contains_key(key) || index.bins.contains_key(key)
            })
        };
        let after_fourth = m.extra_nodes.iter().chain(once(&m.mname));
        let model = after_fourth
            .into_iter()
            .position(|word| names_model(word))
            .map(|k| m.extra_nodes.len() - k)
            .or_else(|| {
                m.nb.is_some_and(&mut names_model)
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
        let line = self.read_line(m, on_path);
        self.select_line(line, scopes, on_path)
    }

    /// The model of a MOSFET whose line reads `line`: see [`Selector::select`]
    fn select_line(
        &mut self,
        line: MosLine<'_, '_>,
        scopes: &[&Scope<'_>],
        on_path: &[&'a AST<'s>],
    ) -> Result<Selection<'a, 's>, SelectError> {
        let scope = scopes.last().expect("the top level at least");
        let size = size(line, scope, &self.opts)?;
        let selection = self.choose(line.name(), size, scopes, on_path)?;
        if line.nodes() > MAX_MOSFET_NODES {
            return Err(SelectError::TooManyNodes {
                name: line.name().to_owned(),
                nodes: line.nodes(),
            });
        }
        if line.nodes() > 4 && line.m.extra_nodes[0].starts_with(|c: char| c.is_ascii_digit()) {
            return Err(SelectError::DigitFifthWord {
                name: line.name().to_owned(),
                word: line.m.extra_nodes[0].to_string(),
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
/// `.subckt` defaults, `.param` statements and active `.if` branches, and the
/// values passed. Each value passed is computed now, so that a value passed
/// down a deep hierarchy is not a deep recursion later.
fn instance_scope<'p, 'a, 's>(
    scope: &'p Scope<'p>,
    def: &'a Subckt<'s>,
    line: &[KeyValue<'_>],
) -> (Scope<'p>, Active<'a, 's>) {
    let mut child = scope.child();
    child.define_shared(
        ptr::from_ref(def) as usize,
        def.params.iter().chain(&def.ast.param),
        &def.ast.param_func,
    );
    let line = || line.iter().filter(|kv| !is_multiplier(kv));
    for kv in line() {
        child.pass(kv);
    }
    let mut active = Active::default();
    child.activate(&def.ast, &mut active);
    for kv in line() {
        _ = child.get(kv.k);
    }
    (child, active)
}

/// Whether `kv`, of an `X` line, is the `m` of the line: it multiplies the
/// instance, and the subckt does not see it (decks `x_m_*`)
fn is_multiplier(kv: &KeyValue<'_>) -> bool {
    kv.k.eq_ignore_ascii_case("m")
}

/// Why the line `x`, which calls `def`, cannot be expanded below the
/// definitions `expanding`
fn expansion_error<'s>(
    x: &crate::instance::Subckt<'_>,
    def: &Subckt<'s>,
    expanding: &[&Subckt<'s>],
) -> Option<SelectError> {
    if x.nodes.len() != def.ports.len() {
        Some(SelectError::NodeCount {
            subckt: def.name.to_string(),
            ports: def.ports.len(),
            nodes: x.nodes.len(),
        })
    } else if expanding.iter().any(|s| ptr::eq(*s, def)) {
        Some(SelectError::Recursive(def.name.to_string()))
    } else if expanding.len() >= MAX_HIERARCHY_DEPTH {
        Some(SelectError::TooDeep(def.name.to_string()))
    } else {
        None
    }
}

/// The error of a condition that could not be computed
fn condition_error(e: &CondError<'_, '_>) -> SelectError {
    SelectError::Condition {
        cond: e.cond.to_owned(),
        error: e.error.clone(),
    }
}

/// The definitions of a level for its lines: `def.ast`, its active branches
fn with_branches<'a, 's>(
    above: &[&'a AST<'s>],
    ast: &'a AST<'s>,
    active: &Active<'a, 's>,
) -> Vec<&'a AST<'s>> {
    let mut all = Vec::with_capacity(above.len() + 1 + active.branches.len());
    all.extend_from_slice(above);
    all.push(ast);
    all.extend_from_slice(&active.branches);
    all
}

/// The parameters of each entry of [`with_branches`]
fn with_scope<'q, 'p>(
    above: &[&'q Scope<'p>],
    scope: &'q Scope<'p>,
    active: &Active<'_, '_>,
) -> Vec<&'q Scope<'p>> {
    let mut all = Vec::with_capacity(above.len() + 1 + active.branches.len());
    all.extend_from_slice(above);
    all.resize(above.len() + 1 + active.branches.len(), scope);
    all
}

impl<'s> AST<'s> {
    /// The model HSPICE uses for a MOSFET at the top level whose model name
    /// is `name`, of length `l` and width `w` (in meters) and one finger:
    /// the model named `name`, or the bin `name.N` that holds `l` and `w`
    pub fn select_model(&self, name: &str, l: f64, w: f64) -> Result<&Model<'s>, SelectError> {
        let (scope, active) = self.top_level();
        let mut sel = Selector::new(self, &active.branches);
        let size = Size {
            l,
            w,
            nf: 1.0,
            wnflag: None,
            nfin: 1.0,
            wsheet: 0.0,
        };
        let on_path = with_branches(&[], self, &active);
        let scopes = with_scope(&[], &scope, &active);
        sel.choose(name, size, &scopes, &on_path).map(|s| s.model)
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
        log_far_reads(&self.for_each_mosfet_model(|m| out.push(m)));
        out
    }

    /// The MOSFETs of the subckt `name` (defined at the top level), as in
    /// [`AST::mosfet_models`], for an instance of it with its default
    /// parameters; e.g. the cells of a library. `None` if there is no such
    /// subckt. The paths start below the subckt (`MM1`, `XI0.MM2`).
    pub fn mosfet_models_of(&self, name: &str) -> Option<Vec<MosfetModel<'_, 's>>> {
        let mut out = Vec::new();
        log_far_reads(&self.for_each_mosfet_model_of(name, |m| out.push(m))?);
        Some(out)
    }

    /// Calls `f` with each MOSFET of [`AST::mosfet_models`], in the same
    /// order, without keeping them: for circuits whose hierarchy holds too
    /// many transistors to list in memory. Returns the reads where HSPICE can use
    /// a different value.
    pub fn for_each_mosfet_model<'a>(&'a self, f: impl FnMut(MosfetModel<'a, 's>)) -> Vec<FarRead> {
        let (scope, active) = self.top_level();
        let mut walk = Walk {
            sel: Selector::new(self, &active.branches),
            f,
            path: Vec::new(),
            expanding: Vec::new(),
        };
        let on_path = with_branches(&[], self, &active);
        let scopes = with_scope(&[], &scope, &active);
        walk.ast(self, &active, &scopes, &on_path);
        scope.far_reads()
    }

    /// [`AST::for_each_mosfet_model`] for the subckt `name`, as in
    /// [`AST::mosfet_models_of`]. `None` if there is no such subckt.
    pub fn for_each_mosfet_model_of<'a>(
        &'a self,
        name: &str,
        f: impl FnMut(MosfetModel<'a, 's>),
    ) -> Option<Vec<FarRead>> {
        let (scope, top) = self.top_level();
        let on_top = with_branches(&[], self, &top);
        let def = resolve(name, &on_top)?;
        let (child, active) = instance_scope(&scope, def, &[]);
        let mut walk = Walk {
            sel: Selector::new(self, &top.branches),
            f,
            path: Vec::new(),
            expanding: vec![def],
        };
        let on_path = with_branches(&on_top, &def.ast, &active);
        let top_scopes = with_scope(&[], &scope, &top);
        let scopes = with_scope(&top_scopes, &child, &active);
        walk.ast(&def.ast, &active, &scopes, &on_path);
        Some(scope.far_reads())
    }
}

/// Logs the reads where HSPICE can use a different value
fn log_far_reads(far_reads: &[FarRead]) {
    for far in far_reads {
        crate::warn!("{far}");
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
    /// level to `ast` itself (where its models and subckts are looked up), each
    /// with its active `.if` branches after it; `scopes`: their parameters, the
    /// top level's and those of the instances (`ast`'s last); `active`: what
    /// the `.if` blocks of `ast` give for this instance
    fn ast(
        &mut self,
        ast: &'a AST<'s>,
        active: &Active<'a, 's>,
        scopes: &[&Scope<'_>],
        on_path: &[&'a AST<'s>],
    ) {
        for e in &active.errors {
            let path = self.path_of(e.block.line.name);
            (self.f)(MosfetModel {
                path,
                instance: &e.block.line,
                mname: "",
                selection: Err(condition_error(e)),
            });
        }
        if ast.cond.is_empty() {
            for inst in &ast.instance {
                self.line(inst, scopes, on_path);
            }
        } else {
            let lines = in_order(ast, &active.branches, &|a| &a.instance, &|b| b.instance_at);
            for inst in lines {
                self.line(inst, scopes, on_path);
            }
        }
    }

    /// The path of the line `name` of the current level
    fn path_of(&self, name: &str) -> String {
        let size = self.path.iter().map(|p| p.len() + 1).sum::<usize>() + name.len();
        let mut path = String::with_capacity(size);
        for level in &self.path {
            path.push_str(level);
            path.push('.');
        }
        path.push_str(name);
        path
    }

    fn line(&mut self, inst: &'a Instance<'s>, scopes: &[&Scope<'_>], on_path: &[&'a AST<'s>]) {
        let scope = *scopes.last().expect("the top level at least");
        match &inst.ctx {
            InstanceCtx::MOSFET(m) => {
                let line = self.sel.read_line(m, on_path);
                let selection = self.sel.select_line(line, scopes, on_path);
                let mname = line.name();
                let path = self.path_of(inst.name);
                (self.f)(MosfetModel {
                    path,
                    instance: inst,
                    mname,
                    selection,
                });
            }
            InstanceCtx::Subckt(x) => {
                let Some(def) = resolve(x.cktname, on_path) else {
                    let path = self.path_of(inst.name);
                    (self.f)(MosfetModel {
                        path,
                        instance: inst,
                        mname: x.cktname,
                        selection: Err(SelectError::NoSubckt(x.cktname.to_string())),
                    });
                    return;
                };
                if let Some(e) = expansion_error(x, def, &self.expanding) {
                    let path = self.path_of(inst.name);
                    (self.f)(MosfetModel {
                        path,
                        instance: inst,
                        mname: x.cktname,
                        selection: Err(e),
                    });
                    return;
                }
                let (child, active) = instance_scope(scope, def, &x.params);
                let inner_scopes = with_scope(scopes, &child, &active);
                let inner = with_branches(on_path, &def.ast, &active);
                self.path.push(inst.name);
                self.expanding.push(def);
                self.ast(&def.ast, &active, &inner_scopes, &inner);
                self.expanding.pop();
                self.path.pop();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
