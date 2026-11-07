use crate::{
    ast::{KeyValueBuilder, Value, ValueBuilder},
    self_builder,
    span::Span,
};
use netlist_macros::Builder;
use std::{borrow::Cow, collections::HashMap};

/// ``` spice
/// XX1 net48 D VDD VNW PHVT11LL_CKT W=0.22u L=40.00n
/// ```
#[derive(Debug, Clone, Builder)]
pub struct InstanceBuilder {
    pub name: Span,
    /// subckt/model name is the last arg
    /// (fisrt, rest)
    pub ctx: InstanceCtxBuilder,
}

#[derive(Debug, Clone, Builder)]
pub enum InstanceCtxBuilder {
    Resistor(RclBuilder),
    Capacitor(RclBuilder),
    Inductor(RclBuilder),
    Voltage(VoltageBuilder),
    Current(CurrentBuilder),
    MOSFET(MOSFETBuilder),
    BJT(BJTBuilder),
    Diode(DiodeBuilder),
    Subckt(SubcktBuilder),
    /// `E`/`F`/`G`/`H` controlled source
    ControlledSource(ControlledSourceBuilder),
    /// `K` mutual inductor
    MutualInductor(MutualInductorBuilder),
    Unknown {
        r#type: u8,
        nodes: Vec<Span>,
        params: Vec<KeyValueBuilder>,
    },
}
impl<'s> InstanceCtx<'s> {
    pub fn append_nodes<'a>(&'a self, nodes: &mut HashMap<String, &'a &'s str>) {
        match self {
            InstanceCtx::Resistor(r) | InstanceCtx::Capacitor(r) | InstanceCtx::Inductor(r) => {
                nodes.extend([(r.n1.to_lowercase(), &r.n1), (r.n2.to_lowercase(), &r.n2)])
            }
            InstanceCtx::Voltage(v) => {
                nodes.extend([(v.n1.to_lowercase(), &v.n1), (v.n2.to_lowercase(), &v.n2)])
            }
            InstanceCtx::Current(c) => {
                nodes.extend([(c.n1.to_lowercase(), &c.n1), (c.n2.to_lowercase(), &c.n2)])
            }
            InstanceCtx::MOSFET(m) => {
                nodes.extend([
                    (m.ng.to_lowercase(), &m.ng),
                    (m.nd.to_lowercase(), &m.nd),
                    (m.ns.to_lowercase(), &m.ns),
                ]);
                nodes.extend(m.nb.as_ref().map(|s| (s.to_lowercase(), s)));
                nodes.extend(m.extra_nodes.iter().map(|s| (s.to_lowercase(), s)))
            }
            InstanceCtx::BJT(b) => {
                nodes.extend([
                    (b.nb.to_lowercase(), &b.nb),
                    (b.nc.to_lowercase(), &b.nc),
                    (b.ne.to_lowercase(), &b.ne),
                ]);
                nodes.extend(b.ns.as_ref().map(|s| (s.to_lowercase(), s)))
            }
            InstanceCtx::Diode(d) => nodes.extend([
                (d.nminus.to_lowercase(), &d.nminus),
                (d.nplus.to_lowercase(), &d.nplus),
            ]),
            InstanceCtx::Subckt(x) => nodes.extend(x.nodes.iter().map(|s| (s.to_lowercase(), s))),
            InstanceCtx::ControlledSource(c) => {
                nodes.extend([(c.n1.to_lowercase(), &c.n1), (c.n2.to_lowercase(), &c.n2)]);
                for (p, n) in &c.ctrl_nodes {
                    nodes.extend([(p.to_lowercase(), p), (n.to_lowercase(), n)]);
                }
            }
            // `K` couples two inductors: it has no nodes of its own
            InstanceCtx::MutualInductor(_) => {}
            InstanceCtx::Unknown {
                r#type: _,
                nodes: _nodes,
                params: _,
            } => nodes.extend(_nodes.iter().map(|s| (s.to_lowercase(), s))),
        }
    }
}
/// The model and the value of an R, C or L line, in the orders that
/// HSPICE reads (golden `res_model`, `ind_value_model`):
///
/// - `R1 a b 1k`: one token after the nodes is the value. One name stays
///   the value: it can be a parameter or a model, which the query layer
///   decides.
/// - `R1 a b rmod 2k`: a plain word (not quoted, no digit first), then the
///   value. `R1 a b rval 0.004` is read so too (model `rval`, value
///   0.004): the query layer decides whether `rval` is a model or, with no
///   `.model rval` visible, a parameter value with `tc1` = 0.004.
/// - `L1 a b 1n lmod`: for L only, the value and then the model name. R
///   and C lines in this order are errors (golden `res_value_model`,
///   `cap_value_model`).
///
/// A name that starts with a digit is an error in each place where a model
/// name can be: a word such as `2n7002`, and a number with other letters
/// after its scale (`5vnmos`, `33nmos`, `2umos`: HSPICE rejects such a model
/// name, golden `mos_digit_model`). One token alone stays the value, read as HSPICE
/// reads a number (`1mohm` = 1e-3, golden `units2`).
///
/// The numbers after the value are [`RCL_POSITIONAL`] (`tc1`, `tc2`; golden
/// `res_tc_positional`), each with its token as written; a quoted
/// expression there is kept too (golden `quoted_plus_tc`). In `tc1` after a
/// model name and a value, and in `tc2`, the letters after a number's
/// scale are ignored (`2umos` = 2e-6, as golden `mos_suffix_letters` for
/// M). The last `r=` (`c=`, `l=`) parameter, in any case, wins over a
/// value after the nodes (golden `elem_dup_kw`), and it is not in
/// `params`. Then a plain word after the nodes is the model
/// (`R2 2 0 rmod r=3k`).
///
/// A C line has the same fields, with `c=`. An L line has them with `l=`,
/// and its model name may also come after the value (`L1 a b 1n lmod`).
#[derive(Debug, Clone, Builder)]
pub struct RclBuilder {
    pub n1: Span,
    pub n2: Span,
    /// The model name; see [`RclBuilder`].
    pub model: Option<Span>,
    /// The value; see [`RclBuilder`]. `None`: no value.
    pub value: Option<ValueBuilder>,
    /// The numbers after the value, as written and read: the values of
    /// [`RCL_POSITIONAL`] in that order.
    pub positional: Vec<(Span, ValueBuilder)>,
    pub params: Vec<KeyValueBuilder>,
}

/// The parameter names of the numbers after the value of an R, C or L
/// line, in order (golden `res_tc_positional` for R; C and L have the same
/// syntax in the HSPICE manual, `[[TC1=]val] [[TC2=]val]`, not probed).
pub const RCL_POSITIONAL: [&str; 2] = ["tc1", "tc2"];

impl RclBuilder {
    /// The numbers after the value, with their parameter names
    /// (`tc1`, `tc2`) and their tokens as written.
    pub fn positional_params(&self) -> impl Iterator<Item = (&'static str, &Span, &ValueBuilder)> {
        RCL_POSITIONAL
            .into_iter()
            .zip(&self.positional)
            .map(|(name, (sp, v))| (name, sp, v))
    }
}
impl<'s> Rcl<'s> {
    /// The numbers after the value, with their parameter names
    /// (`tc1`, `tc2`).
    pub fn positional_params(&self) -> impl Iterator<Item = (&'static str, &Value<'s>)> {
        RCL_POSITIONAL
            .into_iter()
            .zip(&self.positional)
            .map(|(name, (_, v))| (name, v))
    }
}

#[derive(Debug, Clone, Builder)]
pub struct SubcktBuilder {
    pub nodes: Vec<Span>,
    pub cktname: Span,
    pub params: Vec<KeyValueBuilder>,
}
/// A `Q` (BJT) element line.
#[derive(Debug, Clone, Builder)]
pub struct BJTBuilder {
    pub nc: Span,
    pub nb: Span,
    pub ne: Span,
    pub ns: Option<Span>,
    /// The model name: the last token before the parameters that is not a
    /// number and not `OFF`.
    pub mname: Span,
    /// The number after the model name (`Q1 c b e qmod 2`): the value of
    /// [`BJT_POSITIONAL`] (`area`). See [`BJTBuilder::positional_params`].
    pub positional: Vec<(Span, ValueBuilder)>,
    pub params: Vec<KeyValueBuilder>,
}

/// The parameter name of the number after the model name of a `Q` line
/// (golden `bjt_area`: `Qxxx nc nb ne [ns] mname [area]`).
pub const BJT_POSITIONAL: [&str; 1] = ["area"];

impl BJTBuilder {
    /// The number after the model name, with its parameter name (`area`)
    /// and its token as written.
    pub fn positional_params(&self) -> impl Iterator<Item = (&'static str, &Span, &ValueBuilder)> {
        BJT_POSITIONAL
            .into_iter()
            .zip(&self.positional)
            .map(|(name, (sp, v))| (name, sp, v))
    }
}

impl<'s> BJT<'s> {
    /// The number after the model name, with its parameter name (`area`).
    pub fn positional_params(&self) -> impl Iterator<Item = (&'static str, &Value<'s>)> {
        BJT_POSITIONAL
            .into_iter()
            .zip(&self.positional)
            .map(|(name, (_, v))| (name, v))
    }
}
#[derive(Debug, Clone, Builder)]
pub struct MOSFETBuilder {
    pub nd: Span,
    pub ng: Span,
    pub ns: Span,
    pub nb: Option<Span>,
    /// The nodes after the 4th, before the model name (up to 3, for
    /// SOI-style models: `d g s e p b t`).
    pub extra_nodes: Vec<Span>,
    /// The model name: the last token before the parameters that is not a
    /// number and not `OFF`.
    pub mname: Span,
    /// The numbers after the model name (`M1 d g s b nch 2u 1u`): the
    /// values of [`MOS_POSITIONAL`] in that order, `l` first (golden
    /// `mos_positional`, `mos_positional_all`). A bare `OFF` holds a place
    /// here, see [`is_off`]. See [`MOSFETBuilder::positional_params`].
    pub positional: Vec<(Span, ValueBuilder)>,
    pub params: Vec<KeyValueBuilder>,
}

/// The parameter names of the numbers after the model name of an `M`
/// line, in order (golden `mos_positional`, `mos_positional_all`).
pub const MOS_POSITIONAL: [&str; 8] = ["l", "w", "ad", "as", "pd", "ps", "nrd", "nrs"];

/// Whether a value after the model name of a MOSFET is the word `OFF`. It takes
/// a place and gives no value: `M1 d g 0 0 nch OFF 1u` has the default L and
/// W=1u (probes on P-2019.06).
pub fn is_off(value: &Value<'_>) -> bool {
    is_key_place(value) || matches!(value, Value::Expr(e) if gives_no_value(e))
}
/// A word of a MOSFET line that takes a place and gives no value: `OFF`, or a lone `+`
/// (`1u + 2u` has no W, decks `w*`).
pub fn gives_no_value(word: &str) -> bool {
    word.eq_ignore_ascii_case("off") || word == "+"
}

/// Whether a place after the model name of a MOSFET holds a `key=value` parameter.
/// Each such parameter takes a place and leaves an empty value there, so the empty
/// values tell the order of the line: the first one stands for the first parameter.
pub fn is_key_place(value: &Value<'_>) -> bool {
    matches!(value, Value::Expr(e) if e.is_empty())
}

/// The number that `word`, a value without quotes on a MOSFET line, reads as. A
/// number with an operator right after it starts an expression (`2u*1`). HSPICE
/// reads any other word that starts with a number as that number: `2n7002` is 2n,
/// `2^2` is 2 and `2u.5` is 2u. `(2u)` is a number too (decks `gx_*`, `hx_*`, `ma_*`).
pub(crate) fn plain_word(word: &str) -> Option<f64> {
    let word = word
        .strip_prefix('(')
        .and_then(|w| w.strip_suffix(')'))
        .map_or(word, str::trim);
    if word.contains(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',')) {
        return None;
    }
    let (value, end, _) = netlist_expr::scan_number_exact(word)?;
    (!word[end..].starts_with(['*', '/', '+', '-'])).then_some(value)
}

impl MOSFETBuilder {
    /// The numbers after the model name, with their parameter names
    /// (`l`, `w`, …) and their tokens as written. A bare `OFF` is one of them.
    pub fn positional_params(&self) -> impl Iterator<Item = (&'static str, &Span, &ValueBuilder)> {
        MOS_POSITIONAL
            .into_iter()
            .zip(&self.positional)
            .map(|(name, (sp, v))| (name, sp, v))
    }
}

impl<'s> MOSFET<'s> {
    /// The numbers after the model name, with their parameter names
    /// (`l`, `w`, …). A bare `OFF` gives no value, see [`is_off`].
    pub fn positional_params(&self) -> impl Iterator<Item = (&'static str, &Value<'s>)> {
        MOS_POSITIONAL
            .into_iter()
            .zip(&self.positional)
            .map(|(name, (_, v))| (name, v))
            .filter(|(_, v)| !is_off(v))
    }
}
/// A `D` (diode) element line.
#[derive(Debug, Clone, Builder)]
pub struct DiodeBuilder {
    pub nplus: Span,
    pub nminus: Span,
    /// The model name: the last token before the parameters that is not a
    /// number and not `OFF`.
    pub mname: Span,
    /// The numbers after the model name (`D1 a k dmod 2`): the values of
    /// [`DIODE_POSITIONAL`] in that order. See
    /// [`DiodeBuilder::positional_params`].
    pub positional: Vec<(Span, ValueBuilder)>,
    pub params: Vec<KeyValueBuilder>,
}

/// The parameter names of the numbers after the model name of a `D` line,
/// in order (golden `diode_area`: `Dxxx nplus nminus mname [area [pj]]`).
pub const DIODE_POSITIONAL: [&str; 2] = ["area", "pj"];

impl DiodeBuilder {
    /// The numbers after the model name, with their parameter names
    /// (`area`, `pj`) and their tokens as written.
    pub fn positional_params(&self) -> impl Iterator<Item = (&'static str, &Span, &ValueBuilder)> {
        DIODE_POSITIONAL
            .into_iter()
            .zip(&self.positional)
            .map(|(name, (sp, v))| (name, sp, v))
    }
}

impl<'s> Diode<'s> {
    /// The numbers after the model name, with their parameter names
    /// (`area`, `pj`).
    pub fn positional_params(&self) -> impl Iterator<Item = (&'static str, &Value<'s>)> {
        DIODE_POSITIONAL
            .into_iter()
            .zip(&self.positional)
            .map(|(name, (_, v))| (name, v))
    }
}
#[derive(Debug, Clone, Builder)]
pub struct VoltageBuilder {
    pub n1: Span,
    pub n2: Span,
    /// Boxed: sources are few, and an inline `SourceBuilder` would make
    /// every `InstanceCtxBuilder` larger.
    pub source: Box<SourceBuilder>,
}
#[derive(Debug, Clone, Builder)]
pub struct CurrentBuilder {
    pub n1: Span,
    pub n2: Span,
    /// Boxed, as for `VoltageBuilder`.
    pub source: Box<SourceBuilder>,
}

/// The source of a V or I line, after its two nodes: `[[DC] v | DC=v]
/// [tranfun] [AC mag [phase] | AC=mag [phase]]` and `key=value` parameters
/// (golden `src_forms`, `src_ac_phase`, `src_order_ac_first`). A comma may
/// come before the phase (`AC=1,90`, HSPICE manual). A value with no
/// keyword is the DC value only as the first item. `DC` after the function
/// is an error (golden `src_order_fn_first`, `src_fn_dc_more`). A bare `R`
/// after a PWL function (`PWL(0 0 1n 1) R`) is the parameter `R=0`: HSPICE
/// gives the repeat time 0 for it, as for `R=0` (golden `pwl_repeat`,
/// `pwl_repeat_td`). A line with no source part (`V1 a 0`) is a 0 V (0 A)
/// source, as the HSPICE manual allows (not probed): every part is empty. Each value keeps its token as written (a
/// quoted expression with its quotes) and its value.
#[derive(Debug, Clone, Default, Builder)]
pub struct SourceBuilder {
    pub dc: Option<(Span, ValueBuilder)>,
    pub ac_mag: Option<(Span, ValueBuilder)>,
    pub ac_phase: Option<(Span, ValueBuilder)>,
    /// The transient function.
    pub func: Option<SourceFuncBuilder>,
    /// The other `key=value` parameters.
    pub params: Vec<KeyValueBuilder>,
}

/// A transient function of a V or I line: `PULSE(0 1 0 1n)`,
/// `SIN (0 1 1meg)`, or without parentheses, `PULSE 0 1 0 1n` (golden
/// `src_forms`).
#[derive(Debug, Clone, Builder)]
pub struct SourceFuncBuilder {
    /// The function name, as written (`PULSE`, `sin`, `PWL`, …).
    pub name: Span,
    /// The arguments as written: the text between the parentheses
    /// (without the spaces and `+` markers before the first argument and
    /// after the last), or from the first to the last argument. It can
    /// hold `+` lines and comments; [`source_args`] removes them.
    pub args: Span,
}

/// Which of the four controlled sources an element is
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlledKind {
    /// `E`: voltage-controlled voltage source
    E,
    /// `F`: current-controlled current source
    F,
    /// `G`: voltage-controlled current source
    G,
    /// `H`: current-controlled voltage source
    H,
}
self_builder!(ControlledKind);

/// `E`/`F`/`G`/`H` controlled source:
/// ``` spice
/// Exxx n+ n- [VCVS] in+ in- gain [MAX=val] ...
/// Exxx n+ n- [VCVS] POLY(ndim) in1+ in1- ... p0 p1 ...
/// Exxx n+ n- [VCVS] PWL(1) in+ in- [DELTA=val] x1,y1 x2,y2 ...
/// Exxx n+ n- VOL='expr'
/// Fxxx n+ n- [CCCS] vsrc gain
/// Gxxx n+ n- VCR PWL(1) in+ in- x1,y1 x2,y2 ...
/// Hxxx n+ n- [CCVS] POLY(ndim) vsrc1 ... p0 p1 ...
/// ```
#[derive(Debug, Clone, Builder)]
pub struct ControlledSourceBuilder {
    pub kind: ControlledKind,
    /// output nodes
    pub n1: Span,
    pub n2: Span,
    /// `VCVS`, `VCCS`, `CCCS`, `CCVS`, `VCR` or `VCCAP`, if given
    pub keyword: Option<Span>,
    /// `POLY(ndim)`
    pub poly: Option<usize>,
    /// a piecewise-linear transfer function: `PWL(1)`, `NPWL(1)`, `PPWL(1)`,
    /// or a multi-input gate `AND(n)`, `NAND(n)`, `OR(n)`, `NOR(n)`
    /// (boxed, to keep every element small)
    pub pwl: Option<Box<ControlledPwlBuilder>>,
    /// controlling node pairs (`E`, `G`)
    pub ctrl_nodes: Vec<(Span, Span)>,
    /// controlling voltage sources (`F`, `H`)
    pub ctrl_sources: Vec<Span>,
    /// the gain, or the polynomial coefficients
    pub coeffs: Vec<ValueBuilder>,
    /// e.g. `VOL='...'`, `CUR='...'`, `MAX=1`, `DELTA=0.1`
    pub params: Vec<KeyValueBuilder>,
}

/// The piecewise-linear transfer function of a controlled source:
/// ``` spice
/// Gxxx n+ n- VCR PWL(1) in+ in- x1,y1 x2,y2 ...
/// Exxx n+ n- AND(2) in1+ in1- in2+ in2- x1,y1 x2,y2 ...
/// ```
#[derive(Debug, Clone, Builder)]
pub struct ControlledPwlBuilder {
    /// `PWL`, `NPWL`, `PPWL`, `AND`, `NAND`, `OR` or `NOR`, as written
    pub function: Span,
    /// the number of controlling inputs: `n` in `PWL(n)`
    pub ndim: usize,
    /// the `(x, y)` points
    pub points: Vec<(ValueBuilder, ValueBuilder)>,
}

/// ``` spice
/// Kxxx Lyyy Lzzz [K=]coupling
/// ```
#[derive(Debug, Clone, Builder)]
pub struct MutualInductorBuilder {
    /// the coupled inductors (element names, not nodes)
    pub l1: Span,
    pub l2: Span,
    pub coupling: ValueBuilder,
    pub params: Vec<KeyValueBuilder>,
}

/// The arguments `args` of a source function (see
/// [`SourceFuncBuilder::args`]) on one line: the `+` markers, the `$`
/// comments and the `*` comment lines are removed, and the lines are
/// joined by one space. No copy when `args` is on one line with no `$`.
pub fn source_args(args: &str) -> Cow<'_, str> {
    if !args.contains(['\n', '$']) {
        return Cow::Borrowed(args);
    }
    let mut out = String::with_capacity(args.len());
    for (k, line) in args.split('\n').enumerate() {
        let mut line = line.trim();
        if k > 0 {
            if line.starts_with('*') {
                continue;
            }
            line = line.strip_prefix('+').unwrap_or(line).trim_start();
        }
        // a `$` starts a comment, also right after a number (as in the
        // parse of the arguments)
        let line = line[..line.find('$').unwrap_or(line.len())].trim_end();
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    Cow::Owned(out)
}
