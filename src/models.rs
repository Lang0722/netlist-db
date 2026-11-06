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
        Active, Binding, CondError, EvalError, ParHier, Scope, expr_names, function_name, in_order,
    },
    instance::{Instance, InstanceCtx, MOSFET, is_key_place, is_off, plain_word},
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
    fn is_mosfet(&self) -> bool {
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
    /// `.option wl`: the first value after the model name is W, and the second
    /// one L
    wl: bool,
}

impl Options {
    /// The options of the top level and of its active branches `active`. Only
    /// a plain number sets `scale`, `scalm`, `defl` and `defw`, and the last one
    /// wins, wherever it is at that level. HSPICE ignores other values: `'7u'`,
    /// a name, an expression (decks `opt_scale_*`, `opd_*`, `os_*`).
    fn new<'a, 's>(ast: &'a AST<'s>, active: &[&'a AST<'s>]) -> Self {
        let options = ast.top_options(active);
        let values = |name: &'static str| {
            options
                .iter()
                .rev()
                .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_ref())
        };
        // (the last plain number. One of 0 or below gives the default back: decks
        // `zd_*`, `zq_*`)
        let number = |name: &'static str, default: f64| {
            values(name)
                .find_map(|v| match v {
                    Some(Value::Num(x)) => Some(*x),
                    _ => None,
                })
                .filter(|x| *x > 0.0)
                .unwrap_or(default)
        };
        // (a number in quotes counts here, an expression does not: decks `opw_*`)
        let wnflag = values("wnflag")
            .find_map(|v| match v {
                None => Some(true),
                Some(Value::Num(x)) => Some(*x != 0.0),
                Some(Value::Expr(e)) => e.parse::<f64>().ok().map(|x| x != 0.0),
            })
            .unwrap_or(true);
        Self {
            scale: number("scale", 1.0),
            scalm: number("scalm", 1.0),
            defl: number("defl", 1e-4),
            defw: number("defw", 1e-4),
            wnflag,
            wl: option_wl(ast),
        }
    }
}

/// `.option wl`. It counts anywhere in the deck: in a subckt that is not used,
/// and in a branch that is not active. Only `wl` and `wl=1` set it, and the last
/// one wins (decks `opt_wl_*`, `wv_*`). The levels do not keep their order, so
/// the top level reads as the last one.
fn option_wl(ast: &AST<'_>) -> bool {
    let mut levels = vec![ast];
    let mut at = 0;
    while let Some(&level) = levels.get(at) {
        at += 1;
        levels.extend(level.subckt.values().map(|def| &def.ast));
        for block in &level.cond {
            levels.extend(block.branches.iter().map(|branch| &branch.ast));
        }
    }
    levels
        .iter()
        .flat_map(|level| level.option.iter().rev())
        .find(|(k, _)| k.eq_ignore_ascii_case("wl"))
        .is_some_and(|(_, v)| matches!(v, None | Some(Value::Num(1.0))))
}

/// How far below a limit (relative) a value counts as on it
const EDGE_TOLERANCE: f64 = 2e-15;

/// `min <= x < max`, a value just below a limit counting as on it
fn within(x: f64, min: f64, max: f64) -> bool {
    let tol = |v: f64| v.abs() * EDGE_TOLERANCE;
    x >= min - tol(min) && x < max - tol(max)
}

/// What a bin limits, besides the length
#[derive(Debug, Clone, Copy)]
struct Across {
    /// the width: `wmin`, `wmax`
    width: bool,
    /// the fin count, at level 72: `nfinmin`, `nfinmax`
    fins: bool,
    /// the sheet width, at level 72: `wsheetmin`, `wsheetmax`
    sheet: bool,
}

/// The ranges of a bin
#[derive(Debug, Clone, Copy)]
struct Limits {
    lmin: f64,
    lmax: f64,
    width: Option<(f64, f64)>,
    fins: Option<(f64, f64)>,
    sheet: Option<(f64, f64)>,
}

impl Limits {
    /// Whether these ranges hold a transistor of size `size`. All the ranges of a
    /// bin count, and the fin count and the sheet width must be above 0 (decks
    /// `hb_*`).
    fn hold(&self, size: &Size, opts: &Options) -> bool {
        let inside = |range: (f64, f64), x: f64| within(x, range.0, range.1);
        // the instance's WNFLAG, else `.option wnflag`
        let per_finger = size.wnflag.unwrap_or(opts.wnflag);
        let w = if per_finger && size.nf > 0.0 {
            size.w / size.nf
        } else {
            size.w
        };
        within(size.l, self.lmin, self.lmax)
            && self.width.is_none_or(|range| inside(range, w))
            && self
                .fins
                .is_none_or(|range| size.nfin > 0.0 && inside(range, size.nfin))
            && self
                .sheet
                .is_none_or(|range| size.wsheet > 0.0 && inside(range, size.wsheet))
    }
}

/// The parameters of a bin that its ranges are made of
const LIMITS: [&str; 8] = [
    "lmin",
    "lmax",
    "wmin",
    "wmax",
    "nfinmin",
    "nfinmax",
    "wsheetmin",
    "wsheetmax",
];

/// What the bin `model` limits, or `None` if it does not set the limits of its
/// level. Level 72 reads no `wmin` and `wmax` (decks `k_lw`, `h_six_w_out`).
fn bin_across(model: &Model<'_>) -> Option<Across> {
    let param = |key: &str| {
        model
            .params
            .iter()
            .find(|kv| kv.k.eq_ignore_ascii_case(key))
    };
    let sets = |key: &str| param(key).is_some();
    if !sets("lmin") || !sets("lmax") {
        return None;
    }
    let finfet = param("level").is_some_and(|kv| matches!(kv.v, Value::Num(72.0)));
    let across = Across {
        width: !finfet && sets("wmin") && sets("wmax"),
        fins: finfet && sets("nfinmin") && sets("nfinmax"),
        sheet: finfet && sets("wsheetmin") && sets("wsheetmax"),
    };
    (across.width || across.fins || across.sheet).then_some(across)
}

/// The ranges of the bin `model`, evaluated in `scope`. Of a limit given twice,
/// the first one counts (deck `mdl_dup_keys`).
fn limits(model: &Model<'_>, scope: &Scope<'_>, scalm: f64) -> Result<Limits, SelectError> {
    let across =
        bin_across(model).ok_or_else(|| SelectError::MissingLimits(model.name.to_string()))?;
    let param = |key: &str, factor: f64| -> Result<f64, SelectError> {
        let Some(kv) = model
            .params
            .iter()
            .find(|kv| kv.k.eq_ignore_ascii_case(key))
        else {
            return Err(SelectError::MissingLimits(model.name.to_string()));
        };
        scope
            .eval(&kv.v)
            .map(|v| v * factor)
            .map_err(|error| SelectError::Eval {
                what: format!("{} of model `{}`", key.to_ascii_uppercase(), model.name),
                error,
            })
    };
    let range = |set: bool, min: &str, max: &str, factor: f64| {
        Ok::<_, SelectError>(if set {
            Some((param(min, factor)?, param(max, factor)?))
        } else {
            None
        })
    };
    Ok(Limits {
        lmin: param("lmin", scalm)?,
        lmax: param("lmax", scalm)?,
        width: range(across.width, "wmin", "wmax", scalm)?,
        fins: range(across.fins, "nfinmin", "nfinmax", 1.0)?,
        sheet: range(across.sheet, "wsheetmin", "wsheetmax", scalm)?,
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
    /// the fin count, for a bin of level 72 (1 if not given)
    nfin: f64,
    /// the sheet width, for a bin of level 72 (if not given, no bin holds it:
    /// deck `w_sheet_w`)
    wsheet: f64,
}

/// The parameters of an instance line that the size of a MOSFET is made of
const SIZES: [&str; 7] = ["l", "w", "nf", "wnflag", "nfin", "wsheet", "m"];

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
            Some(word) => name_words(self.m).nth(word).map(|w| match plain_word(w) {
                Some(number) => Value::Num(number),
                None => Value::Expr(w.clone()),
            }),
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

    /// The value of the parameter `key`, which the place `position` can also give.
    /// HSPICE assigns the plain numbers first and then the expressions, each in the
    /// order of the line, and the last one stays. `1u 2u w=5u` has W=5u, and
    /// `1u k w=5u` has W=k (decks `kx_*`, `ky_*`, `ma_*`, `mb_*`, `mc_*`).
    fn param(self, key: &str, position: Option<usize>) -> Option<Cow<'m, Value<'s>>> {
        let m = self.m;
        let keys_before = position.map_or(0, |p| {
            let places = m.positional.iter().take(p.saturating_sub(self.model));
            let keys = places.filter(|(_, v)| is_key_place(v)).count();
            keys.min(m.params.len())
        });
        let (before, after) = m.params.split_at(keys_before);
        let named = |kvs: &'m [KeyValue<'s>]| {
            let kvs = kvs.iter().filter(|kv| kv.k.eq_ignore_ascii_case(key));
            kvs.map(|kv| Cow::Borrowed(&kv.v))
        };
        let placed = position.and_then(|p| self.value(p)).map(Cow::Owned);
        let (mut number, mut expression) = (None, None);
        for v in named(before).chain(placed).chain(named(after)) {
            if matches!(*v, Value::Num(_)) {
                number = Some(v);
            } else {
                expression = Some(v);
            }
        }
        expression.or(number)
    }
}

/// The size of the MOSFET `line`, evaluated in `scope`
fn size(line: MosLine<'_, '_>, scope: &Scope<'_>, opts: &Options) -> Result<Size, SelectError> {
    let value = |key: &str| {
        let position = POSITIONAL.iter().position(|k| *k == key);
        let position = position.map(|p| if opts.wl { 1 - p } else { p });
        match line.param(key, position) {
            Some(v) => scope.eval(&v).map(Some).map_err(|error| SelectError::Eval {
                what: key.to_ascii_uppercase(),
                error,
            }),
            None => Ok(None),
        }
    };
    let size = Size {
        l: value("l")?.unwrap_or(opts.defl) * opts.scale,
        w: value("w")?.unwrap_or(opts.defw) * opts.scale,
        nf: value("nf")?.unwrap_or(1.0),
        wnflag: value("wnflag")?.map(|flag| flag != 0.0),
        nfin: value("nfin")?.unwrap_or(1.0),
        wsheet: value("wsheet")?.unwrap_or(0.0) * opts.scale,
    };
    let bad = |what, value| Err(SelectError::BadSize { what, value });
    if size.l <= 0.0 {
        return bad("L", size.l);
    }
    if size.w < 0.0 {
        return bad("W", size.w);
    }
    if size.nf < 0.0 {
        return bad("NF", size.nf);
    }
    match value("m")? {
        Some(m) if m <= 0.0 => bad("M", m),
        _ => Ok(size),
    }
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
            exact: HashMap::new(),
            bins: HashMap::new(),
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
    let mut names = HashSet::new();
    let mut stack = vec![ast];
    while let Some(a) = stack.pop() {
        names.extend(a.param.iter().filter_map(|kv| function_name(&kv.k)));
        names.extend(a.param_func.iter().map(|f| f.name.to_ascii_lowercase()));
        for def in a.subckt.values() {
            names.extend(def.params.iter().filter_map(|kv| function_name(&kv.k)));
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
            index: HashMap::new(),
            merged: HashMap::new(),
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
        let key = name.to_ascii_lowercase();
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
/// `.subckt` defaults, `.param` statements and active `.if` branches, and the
/// values passed. Each value passed is computed now, so that a value passed
/// down a deep hierarchy is not a deep recursion later.
fn instance_scope<'p, 'a, 's>(
    scope: &'p Scope<'p>,
    def: &'a Subckt<'s>,
    line: &[KeyValue<'_>],
) -> (Scope<'p>, Active<'a, 's>) {
    let mut child = scope.child();
    for kv in def.params.iter().chain(&def.ast.param) {
        child.define(kv);
    }
    for f in &def.ast.param_func {
        child.define_func(f);
    }
    let line = || line.iter().filter(|kv| !is_multiplier(kv));
    for kv in line() {
        child.pass(kv);
    }
    let mut active = Active::default();
    child.activate(&def.ast, &mut active);
    for kv in line() {
        _ = child.get(&kv.k);
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
    }

    /// [`AST::for_each_mosfet_model`] for the subckt `name`, as in
    /// [`AST::mosfet_models_of`]. `false` if there is no such subckt.
    pub fn for_each_mosfet_model_of<'a>(
        &'a self,
        name: &str,
        f: impl FnMut(MosfetModel<'a, 's>),
    ) -> bool {
        let (scope, top) = self.top_level();
        let on_top = with_branches(&[], self, &top);
        let Some(def) = resolve(name, &on_top) else {
            return false;
        };
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
    /// the MOSFETs of the circuit. This count is too low if `errors` holds an
    /// error for an `X` line or a condition (not for one MOSFET) that hides
    /// MOSFETs, for example [`SelectError::NoSubckt`] for a line whose definition
    /// holds MOSFETs.
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
        let (scope, active) = self.top_level();
        let mut usage = Usage::new(self, &active.branches, &scope);
        let on_path = with_branches(&[], self, &active);
        let scopes = with_scope(&[], &scope, &active);
        let counts = usage.level(self, &active, &scopes, &on_path, 0, 0);
        usage.finish(&counts)
    }

    /// [`AST::model_usage`] for an instance of the subckt `name` (defined at
    /// the top level) with its default parameters. `None` if there is no
    /// such subckt.
    pub fn model_usage_of(&self, name: &str) -> Option<ModelUsage<'_, 's>> {
        let (scope, top) = self.top_level();
        let on_top = with_branches(&[], self, &top);
        let def = resolve(name, &on_top)?;
        let (child, active) = instance_scope(&scope, def, &[]);
        let mut usage = Usage::new(self, &top.branches, &scope);
        usage.expanding.push(def);
        let visible = usage.visible_with(0, &def.ast, &active);
        let on_path = with_branches(&on_top, &def.ast, &active);
        let top_scopes = with_scope(&[], &scope, &top);
        let scopes = with_scope(&top_scopes, &child, &active);
        let counts = usage.level(&def.ast, &active, &scopes, &on_path, 1, visible);
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
        .filter(|kv| !is_multiplier(kv))
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
    fn new(ast: &'a AST<'s>, active: &[&'a AST<'s>], scope: &Scope<'_>) -> Self {
        Self {
            sel: Selector::new(ast, active),
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
                    if !e.is_mosfet() {
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
    /// the level above; `ast`: this level's definition, or a branch active in
    /// it. Two levels with the same number find the same definition for each
    /// name.
    fn visible_below(&mut self, visible: usize, ast: &AST<'_>) -> usize {
        if ast.model.is_empty() && ast.subckt.is_empty() {
            return visible;
        }
        let next = self.visible.len() + 1;
        *self
            .visible
            .entry((visible, ptr::from_ref(ast) as usize))
            .or_insert(next)
    }

    /// [`Usage::visible_below`] for a level with its active branches
    fn visible_with(&mut self, visible: usize, ast: &AST<'_>, active: &Active<'_, '_>) -> usize {
        let mut visible = self.visible_below(visible, ast);
        for branch in &active.branches {
            visible = self.visible_below(visible, branch);
        }
        visible
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
        let inner_visible = self.visible_below(visible, &def.ast);
        self.lines_read(&def.ast, &inner, inner_visible, &mut all)?;
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

    /// Adds to `all` what the lines of `ast` read, and what its `.if` blocks read
    /// in all their branches (any of them can be active). `None` if not known:
    /// so also for a branch with models or subckts, as what the lines of the
    /// level see then depends on the branch. `inner`: the definitions that the
    /// lines of the level see.
    fn lines_read(
        &mut self,
        ast: &'a AST<'s>,
        inner: &[&'a AST<'s>],
        inner_visible: usize,
        all: &mut IndexSet<String>,
    ) -> Option<()> {
        for inst in &ast.instance {
            match &inst.ctx {
                InstanceCtx::MOSFET(m) => {
                    for kv in &m.params {
                        if SIZES.iter().any(|k| kv.k.eq_ignore_ascii_case(k)) {
                            extend(all, &self.sel.names(&kv.v)?);
                        }
                    }
                    let line = self.sel.read_line(m, inner);
                    for v in line.values() {
                        extend(all, &self.sel.names(&v)?);
                    }
                    // the ranges of the bins it may use, read where each is
                    // defined: those of the top level read the top level's
                    // parameters (the same everywhere), those of this subckt
                    // its own; those of an instance above it the parameters
                    // of that instance, which are not followed: unknown
                    let name = line.name().to_ascii_lowercase();
                    let last = inner.len() - 1;
                    for (depth, &level) in inner.iter().enumerate().skip(1) {
                        let index = self.sel.index(level);
                        for bin in index.bins.get(&name).into_iter().flatten() {
                            let names = self.sel.limit_names(bin.model)?;
                            if depth < last && !names.is_empty() {
                                return None;
                            }
                            extend(all, &names);
                        }
                    }
                }
                InstanceCtx::Subckt(x) => {
                    for kv in &x.params {
                        extend(all, &self.sel.names(&kv.v)?);
                    }
                    if let Some(child) = resolve(&x.cktname, inner) {
                        extend(all, &self.reads(child, inner, inner_visible)?.outside);
                    }
                }
                _ => {}
            }
        }
        for block in &ast.cond {
            for branch in &block.branches {
                let inside = &branch.ast;
                if !inside.model.is_empty() || !inside.subckt.is_empty() {
                    return None;
                }
                if let Some(cond) = &branch.cond {
                    extend(all, &self.sel.text_names(cond)?);
                }
                for kv in inside
                    .param
                    .iter()
                    .filter(|kv| function_name(&kv.k).is_none())
                {
                    extend(all, &self.sel.names(&kv.v)?);
                }
                self.lines_read(inside, inner, inner_visible, all)?;
            }
        }
        Some(())
    }

    /// The counts of the instances of `ast` (context `context`, seeing the
    /// definitions numbered `visible`); `active`, `scopes` and `on_path` as in
    /// [`Walk::ast`]
    fn level(
        &mut self,
        ast: &'a AST<'s>,
        active: &Active<'a, 's>,
        scopes: &[&Scope<'_>],
        on_path: &[&'a AST<'s>],
        context: usize,
        visible: usize,
    ) -> Counts {
        let scope = *scopes.last().expect("the top level at least");
        let mut counts = Counts::new();
        for e in &active.errors {
            let used = Use::Error(self.error_number(condition_error(e), &e.block.line.name));
            *counts.entry(used).or_default() += 1;
        }
        if ast.cond.is_empty() {
            for inst in &ast.instance {
                self.line(inst, scope, scopes, on_path, context, visible, &mut counts);
            }
        } else {
            let lines = in_order(ast, &active.branches, &|a| &a.instance, &|b| b.instance_at);
            for inst in lines {
                self.line(inst, scope, scopes, on_path, context, visible, &mut counts);
            }
        }
        counts
    }

    /// Adds the counts of the line `inst` of a level to `counts`
    #[allow(clippy::too_many_arguments)]
    fn line(
        &mut self,
        inst: &'a Instance<'s>,
        scope: &Scope<'_>,
        scopes: &[&Scope<'_>],
        on_path: &[&'a AST<'s>],
        context: usize,
        visible: usize,
        counts: &mut Counts,
    ) {
        {
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
                        return;
                    };
                    if let Some(e) = expansion_error(x, def, &self.expanding) {
                        let used = Use::Error(self.error_number(e, &inst.name));
                        *counts.entry(used).or_default() += 1;
                        return;
                    }
                    let addr = ptr::from_ref(def) as usize;
                    // (a line that passes nothing needs no new instance to
                    // look in the memo)
                    let child = (!x.params.is_empty() || !def.ast.cond.is_empty())
                        .then(|| instance_scope(scope, def, &x.params));
                    let key = if self.unkept.contains(&addr) {
                        None
                    } else {
                        let key = match self.reads(def, on_path, visible) {
                            Some(reads) => MemoKey::Reads(
                                addr,
                                visible,
                                passed_values(
                                    child.as_ref().map(|c| &c.0),
                                    &x.params,
                                    Some(&reads.all),
                                ),
                                reads.outside.iter().map(|n| scope.binding(n)).collect(),
                            ),
                            None => MemoKey::Context(
                                addr,
                                passed_values(child.as_ref().map(|c| &c.0), &x.params, None),
                                context,
                            ),
                        };
                        if let Some(c) = self.memo.get(&key) {
                            add(counts, c);
                            self.memo_use.entry(addr).or_default().0 += 1;
                            return;
                        }
                        Some(key)
                    };
                    let binds = !x.params.is_empty()
                        || !def.params.is_empty()
                        || !def.ast.param.is_empty()
                        || !def.ast.param_func.is_empty()
                        || !def.ast.model.is_empty()
                        || !def.ast.subckt.is_empty()
                        || !def.ast.cond.is_empty();
                    let inner_context = if binds {
                        self.next_context += 1;
                        self.next_context
                    } else {
                        context
                    };
                    let (child, active) = child.unwrap_or_else(|| instance_scope(scope, def, &[]));
                    let inner_visible = self.visible_with(visible, &def.ast, &active);
                    let inner_scopes = with_scope(scopes, &child, &active);
                    let inner = with_branches(on_path, &def.ast, &active);
                    self.path.push(&inst.name);
                    self.expanding.push(def);
                    let c = self.level(
                        &def.ast,
                        &active,
                        &inner_scopes,
                        &inner,
                        inner_context,
                        inner_visible,
                    );
                    self.expanding.pop();
                    self.path.pop();
                    let c: Rc<[(Use, u64)]> = c.into_iter().collect();
                    add(counts, &c);
                    if let Some(key) = key {
                        self.keep(addr, key, c);
                    }
                }
                _ => {}
            }
        }
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
            let path = self.path_of(&e.block.line.name);
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
        self.path.iter().copied().chain(once(name)).join(".")
    }

    fn line(&mut self, inst: &'a Instance<'s>, scopes: &[&Scope<'_>], on_path: &[&'a AST<'s>]) {
        let scope = *scopes.last().expect("the top level at least");
        match &inst.ctx {
            InstanceCtx::MOSFET(m) => {
                let selection = self.sel.select(m, scopes, on_path);
                let mname = self.sel.read_line(m, on_path).name();
                let path = self.path_of(&inst.name);
                (self.f)(MosfetModel {
                    path,
                    instance: inst,
                    mname,
                    selection,
                });
            }
            InstanceCtx::Subckt(x) => {
                let Some(def) = resolve(&x.cktname, on_path) else {
                    let path = self.path_of(&inst.name);
                    (self.f)(MosfetModel {
                        path,
                        instance: inst,
                        mname: &x.cktname,
                        selection: Err(SelectError::NoSubckt(x.cktname.to_string())),
                    });
                    return;
                };
                if let Some(e) = expansion_error(x, def, &self.expanding) {
                    let path = self.path_of(&inst.name);
                    (self.f)(MosfetModel {
                        path,
                        instance: inst,
                        mname: &x.cktname,
                        selection: Err(e),
                    });
                    return;
                }
                let (child, active) = instance_scope(scope, def, &x.params);
                let inner_scopes = with_scope(scopes, &child, &active);
                let inner = with_branches(on_path, &def.ast, &active);
                self.path.push(&inst.name);
                self.expanding.push(def);
                self.ast(&def.ast, &active, &inner_scopes, &inner);
                self.expanding.pop();
                self.path.pop();
            }
            _ => {}
        }
    }
}
