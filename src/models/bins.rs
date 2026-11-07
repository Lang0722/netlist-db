//! The rules of a bin: the options, the limits of each bin, and the L and W of a MOSFET line.
use super::*;

/// The `.option` values used to choose models
#[derive(Debug, Clone, Copy)]
pub(super) struct Options {
    scale: f64,
    pub(super) scalm: f64,
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
    pub(super) fn new<'a, 's>(ast: &'a AST<'s>, active: &[&'a AST<'s>]) -> Self {
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
pub(super) struct Across {
    /// the width: `wmin`, `wmax`
    width: bool,
    /// the fin count, at level 72: `nfinmin`, `nfinmax`
    fins: bool,
    /// the sheet width, at level 72: `wsheetmin`, `wsheetmax`
    sheet: bool,
}

/// The ranges of a bin
#[derive(Debug, Clone, Copy)]
pub(super) struct Limits {
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
    pub(super) fn hold(&self, size: &Size, opts: &Options) -> bool {
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
pub(super) const LIMITS: [&str; 8] = [
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
pub(super) fn bin_across(model: &Model<'_>) -> Option<Across> {
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
pub(super) fn limits(
    model: &Model<'_>,
    scope: &Scope<'_>,
    scalm: f64,
) -> Result<Limits, SelectError> {
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
pub(super) struct Size {
    /// L and W, times `.option scale`
    pub(super) l: f64,
    pub(super) w: f64,
    pub(super) nf: f64,
    /// `WNFLAG` on the instance line, if given
    pub(super) wnflag: Option<bool>,
    /// the fin count, for a bin of level 72 (1 if not given)
    pub(super) nfin: f64,
    /// the sheet width, for a bin of level 72 (if not given, no bin holds it:
    /// deck `w_sheet_w`)
    pub(super) wsheet: f64,
}

/// The parameters of an instance line that the size of a MOSFET is made of
pub(super) const SIZES: [&str; 7] = ["l", "w", "nf", "wnflag", "nfin", "wsheet", "m"];

/// The parameters that the first values after the model name give
const POSITIONAL: [&str; 2] = ["l", "w"];

/// The most nodes that HSPICE reads before the model name of a MOSFET
/// (decks `q_nodes_7`, `q_nodes_8`)
pub(super) const MAX_MOSFET_NODES: usize = 7;

/// The words of the MOSFET line `m` that can be its model name, the last one
/// first. The parser takes the last word that is not a number, so in
/// `M1 d g s b nch pl pw` it takes `pw`, with `nch` and `pl` in the place of
/// nodes.
fn name_words<'m, 's>(m: &'m MOSFET<'s>) -> impl Iterator<Item = &'m &'s str> {
    once(&m.mname)
        .chain(m.extra_nodes.iter().rev())
        .chain(&m.nb)
}

/// A MOSFET line as the selector reads it. [`Selector::read_line`] finds the
/// model name in [`name_words`], and the words after
/// it are values. HSPICE reads `M1 d g s b nch pl pw` so, with the parameters
/// `pl` and `pw` as L and W (deck `q_positional_names`).
#[derive(Clone, Copy)]
pub(super) struct MosLine<'m, 's> {
    pub(super) m: &'m MOSFET<'s>,
    /// the place of the model name in [`name_words`]
    pub(super) model: usize,
}

impl<'m, 's> MosLine<'m, 's> {
    pub(super) fn name(self) -> &'m str {
        name_words(self.m)
            .nth(self.model)
            .expect("a word of the line")
    }

    /// The number of words before the model name
    pub(super) fn nodes(self) -> usize {
        3 + usize::from(self.m.nb.is_some()) + self.m.extra_nodes.len() - self.model
    }

    /// The value at the place `position` after the model name. A bare `OFF`
    /// takes a place and gives no value.
    fn value(self, position: usize) -> Option<Value<'s>> {
        match self.model.checked_sub(position + 1) {
            Some(word) => name_words(self.m).nth(word).map(|w| match plain_word(w) {
                Some(number) => Value::Num(number),
                None => Value::Expr(*w),
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
    pub(super) fn values(self) -> impl Iterator<Item = Value<'s>> {
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
pub(super) fn size(
    line: MosLine<'_, '_>,
    scope: &Scope<'_>,
    opts: &Options,
) -> Result<Size, SelectError> {
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
