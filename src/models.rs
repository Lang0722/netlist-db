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
//! - The limits are evaluated where the transistor is, so they may use the
//!   parameters of its subckt (e.g. `LMIN='2.4e-7-(dxln+dlmisn)'`).
//! - L and W are the instance's `L=` and `W=` (or `.option DEFL`/`DEFW`,
//!   default 100u), times `.option SCALE`; the limits are multiplied by
//!   `.option SCALM`. With the model parameter `WNFLAG=1`, the width per
//!   finger (`W/NF`) is used.
//! - A value within 1e-9 (relative) of a limit counts as equal to it, so
//!   that `L=0.1u` and `LMAX=1e-7` compare equal.

use crate::{
    AST, Subckt,
    ast::Model,
    eval::{EvalError, Scope},
    instance::{Instance, InstanceCtx, MOSFET},
    utlis::MAX_HIERARCHY_DEPTH,
};
use core::fmt;
use itertools::Itertools as _;
use std::{iter::once, ptr};

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
                "none of the {bins} bins `{name}.N` holds L={l:e}, W={w:e}"
            ),
            Self::Eval { what, error } => write!(f, "{what}: {error}"),
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
        }
    }
}

/// Whether the model `model` is a bin of `name`: `name.N`
fn is_bin(model: &str, name: &str) -> bool {
    model.len() > name.len() + 1
        && model.as_bytes()[name.len()] == b'.'
        && model
            .get(..name.len())
            .is_some_and(|base| base.eq_ignore_ascii_case(name))
}

/// `min <= x < max`, a value within 1e-9 (relative) of a limit counting as
/// equal to it
fn within(x: f64, min: f64, max: f64) -> bool {
    let tol = |v: f64| v.abs() * 1e-9;
    (min == f64::NEG_INFINITY || x >= min - tol(min))
        && (max == f64::INFINITY || x < max - tol(max))
}

/// Whether the bin `model` holds a transistor of length `l`, width `w` and
/// `nf` fingers; its limits are evaluated in `scope`
fn bin_holds(
    model: &Model<'_>,
    l: f64,
    w: f64,
    nf: f64,
    scope: &Scope<'_>,
    scalm: f64,
) -> Result<bool, SelectError> {
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
    let lmin = param("lmin")?.map_or(f64::NEG_INFINITY, |v| v * scalm);
    let lmax = param("lmax")?.map_or(f64::INFINITY, |v| v * scalm);
    let wmin = param("wmin")?.map_or(f64::NEG_INFINITY, |v| v * scalm);
    let wmax = param("wmax")?.map_or(f64::INFINITY, |v| v * scalm);
    let per_finger = param("wnflag")?.is_some_and(|flag| flag != 0.0) && nf > 0.0;
    let w = if per_finger { w / nf } else { w };
    Ok(within(l, lmin, lmax) && within(w, wmin, wmax))
}

/// The model named `name` for a transistor of length `l`, width `w` and `nf`
/// fingers. `levels`: the models visible from the transistor, its own
/// subckt's first; `scope`: its parameters.
fn select<'a, 's>(
    name: &str,
    l: f64,
    w: f64,
    nf: f64,
    levels: &[&'a [Model<'s>]],
    scope: &Scope<'_>,
    scalm: f64,
) -> Result<&'a Model<'s>, SelectError> {
    for &models in levels {
        // a model of this very name (the last one, if defined twice)
        if let Some(model) = models
            .iter()
            .rev()
            .find(|m| m.name.eq_ignore_ascii_case(name))
        {
            return Ok(model);
        }
        let bins: Vec<_> = models.iter().filter(|m| is_bin(&m.name, name)).collect();
        if bins.is_empty() {
            continue;
        }
        // the first bin that holds the transistor; a bin before it whose
        // limits cannot be computed makes the choice uncertain: an error
        for model in &bins {
            if bin_holds(model, l, w, nf, scope, scalm)? {
                return Ok(model);
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

/// L, W and NF of a MOSFET, evaluated in `scope`, L and W times `.option
/// scale`
fn size(m: &MOSFET<'_>, scope: &Scope<'_>, opts: &Options) -> Result<(f64, f64, f64), SelectError> {
    let value = |key: &str, default: f64| match m
        .params
        .iter()
        .rev()
        .find(|kv| kv.k.eq_ignore_ascii_case(key))
    {
        Some(kv) => scope.eval(&kv.v).map_err(|error| SelectError::Eval {
            what: key.to_ascii_uppercase(),
            error,
        }),
        None => Ok(default),
    };
    Ok((
        value("l", opts.defl)? * opts.scale,
        value("w", opts.defw)? * opts.scale,
        value("nf", 1.0)?,
    ))
}

impl<'s> AST<'s> {
    /// The model HSPICE uses for a MOSFET at the top level whose model name
    /// is `name`, of length `l` and width `w` (in meters) and one finger:
    /// the model named `name`, or the bin `name.N` that holds `l` and `w`
    pub fn select_model(&self, name: &str, l: f64, w: f64) -> Result<&Model<'s>, SelectError> {
        let scope = self.scope();
        let opts = Options::new(self, &scope);
        select(name, l, w, 1.0, &[&self.model], &scope, opts.scalm)
    }

    /// Every MOSFET of the circuit, in netlist order: the MOSFETs at the top
    /// level, and in the subckt instances under it. For each one: its
    /// instance path, its length and width (with the parameters of every
    /// subckt instance above it), and the model chosen for it.
    pub fn mosfet_models(&self) -> Vec<MosfetModel<'_, 's>> {
        let scope = self.scope();
        let mut walk = Walk {
            opts: Options::new(self, &scope),
            out: Vec::new(),
            path: Vec::new(),
            expanding: Vec::new(),
        };
        walk.ast(self, &scope, &[self]);
        walk.out
    }

    /// The MOSFETs of the subckt `name` (defined at the top level), as in
    /// [`AST::mosfet_models`], for an instance of it with its default
    /// parameters; e.g. the cells of a library. `None` if there is no such
    /// subckt. The paths start below the subckt (`MM1`, `XI0.MM2`).
    pub fn mosfet_models_of(&self, name: &str) -> Option<Vec<MosfetModel<'_, 's>>> {
        let def = self.subckt.get(&name.to_lowercase())?;
        let scope = self.scope();
        let mut child = scope.child();
        for kv in &def.params {
            child.define(kv);
        }
        for kv in &def.ast.param {
            child.define(kv);
        }
        let mut walk = Walk {
            opts: Options::new(self, &scope),
            out: Vec::new(),
            path: Vec::new(),
            expanding: vec![def],
        };
        walk.ast(&def.ast, &child, &[self, &def.ast]);
        Some(walk.out)
    }
}

/// A walk down the instance hierarchy
struct Walk<'a, 's> {
    opts: Options,
    out: Vec<MosfetModel<'a, 's>>,
    /// the subckt instances above the current level
    path: Vec<&'a str>,
    /// the subckts being expanded (to stop at a subckt that instantiates itself)
    expanding: Vec<&'a Subckt<'s>>,
}

impl<'a, 's> Walk<'a, 's> {
    /// `lexical`: the definitions around `ast`, from the top level to `ast`
    /// itself (where its models and subckts are looked up)
    fn ast(&mut self, ast: &'a AST<'s>, scope: &Scope<'_>, lexical: &[&'a AST<'s>]) {
        for inst in &ast.instance {
            match &inst.ctx {
                InstanceCtx::MOSFET(m) => {
                    let selection = size(m, scope, &self.opts).and_then(|(l, w, nf)| {
                        let levels: Vec<&'a [Model<'s>]> =
                            lexical.iter().rev().map(|a| a.model.as_slice()).collect();
                        let model = select(&m.mname, l, w, nf, &levels, scope, self.opts.scalm)?;
                        Ok(Selection { model, l, w, nf })
                    });
                    self.out.push(MosfetModel {
                        path: self.path.iter().copied().chain(once(&*inst.name)).join("."),
                        instance: inst,
                        mname: &m.mname,
                        selection,
                    });
                }
                InstanceCtx::Subckt(x) => {
                    // the definition: in the innermost scope that has one
                    let key = x.cktname.to_lowercase();
                    let Some((depth, def)) = lexical
                        .iter()
                        .enumerate()
                        .rev()
                        .find_map(|(d, a)| a.subckt.get(&key).map(|s| (d, s)))
                    else {
                        continue;
                    };
                    if self.expanding.iter().any(|s| ptr::eq(*s, def))
                        || self.expanding.len() >= MAX_HIERARCHY_DEPTH
                    {
                        continue;
                    }
                    // the parameters of the instance: its `.subckt` defaults
                    // and `.param` statements, and the values passed on the
                    // instance line (computed where the instance is)
                    let mut child = scope.child();
                    for kv in &def.params {
                        child.define(kv);
                    }
                    for kv in &def.ast.param {
                        child.define(kv);
                    }
                    for kv in &x.params {
                        child.pass(&kv.k, scope.eval(&kv.v));
                    }
                    let inner: Vec<_> = lexical[..=depth]
                        .iter()
                        .copied()
                        .chain(once(&def.ast))
                        .collect();
                    self.path.push(&inst.name);
                    self.expanding.push(def);
                    self.ast(&def.ast, &child, &inner);
                    self.expanding.pop();
                    self.path.pop();
                }
                _ => {}
            }
        }
    }
}
