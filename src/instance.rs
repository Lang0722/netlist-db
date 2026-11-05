use crate::{
    ast::{KeyValueBuilder, ValueBuilder},
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
    Resistor(ResistorBuilder),
    Capacitor(CapacitorBuilder),
    Inductor(InductorBuilder),
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
    pub fn append_nodes<'a>(&'a self, nodes: &mut HashMap<String, &'a Cow<'s, str>>) {
        match self {
            InstanceCtx::Resistor(r) => {
                nodes.extend([(r.n1.to_lowercase(), &r.n1), (r.n2.to_lowercase(), &r.n2)])
            }
            InstanceCtx::Capacitor(c) => {
                nodes.extend([(c.n1.to_lowercase(), &c.n1), (c.n2.to_lowercase(), &c.n2)])
            }
            InstanceCtx::Inductor(i) => {
                nodes.extend([(i.n1.to_lowercase(), &i.n1), (i.n2.to_lowercase(), &i.n2)])
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
                nodes.extend(m.nb.as_ref().map(|s| (s.to_lowercase(), s)))
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
/// ``` spice
/// Rxxx n1 n2 [mname] [R=]resistance [TC1=val] [M=val] ...
/// ```
#[derive(Debug, Clone, Builder)]
pub struct ResistorBuilder {
    pub n1: Span,
    pub n2: Span,
    pub value: ValueBuilder,
    /// model name, if given
    pub mname: Option<Span>,
    /// other parameters, e.g. `M=2 TC1=1e-3`
    pub params: Vec<KeyValueBuilder>,
}
/// ``` spice
/// Cxxx n1 n2 [mname] [C=]capacitance [M=val] ...
/// ```
#[derive(Debug, Clone, Builder)]
pub struct CapacitorBuilder {
    pub n1: Span,
    pub n2: Span,
    pub value: ValueBuilder,
    /// model name, if given
    pub mname: Option<Span>,
    /// other parameters, e.g. `M=2`
    pub params: Vec<KeyValueBuilder>,
}
/// ``` spice
/// Lxxx n1 n2 [L=]inductance [mname] [M=val] ...
/// ```
#[derive(Debug, Clone, Builder)]
pub struct InductorBuilder {
    pub n1: Span,
    pub n2: Span,
    pub value: ValueBuilder,
    /// model name, if given
    pub mname: Option<Span>,
    /// other parameters, e.g. `M=2`
    pub params: Vec<KeyValueBuilder>,
}
#[derive(Debug, Clone, Builder)]
pub struct SubcktBuilder {
    pub nodes: Vec<Span>,
    pub cktname: Span,
    pub params: Vec<KeyValueBuilder>,
}
/// https://eda-cpu1.eias.junzhuo.site/~junzhuo/hspice/index.htm#page/hspice_11/bipolar_junction_transistor_bjt_element.htm
#[derive(Debug, Clone, Builder)]
pub struct BJTBuilder {
    pub nc: Span,
    pub nb: Span,
    pub ne: Span,
    pub ns: Option<Span>,
    pub mname: Span,
    pub params: Vec<KeyValueBuilder>,
}
#[derive(Debug, Clone, Builder)]
pub struct MOSFETBuilder {
    pub nd: Span,
    pub ng: Span,
    pub ns: Span,
    pub nb: Option<Span>,
    pub mname: Span,
    /// the values after the model name: L, W, AD, AS, PD, PS, NRD, NRS
    pub positional: Vec<ValueBuilder>,
    pub params: Vec<KeyValueBuilder>,
}
/// https://eda-cpu1.eias.junzhuo.site/~junzhuo/hspice/index.htm#page/hspice_11/diode_element.htm
#[derive(Debug, Clone, Builder)]
pub struct DiodeBuilder {
    pub nplus: Span,
    pub nminus: Span,
    pub mname: Span,
    pub params: Vec<KeyValueBuilder>,
}
#[derive(Debug, Clone, Builder)]
pub struct VoltageBuilder {
    pub n1: Span,
    pub n2: Span,
    pub source: VoltageSourceBuilder,
}
#[derive(Debug, Clone, Builder)]
pub enum VoltageSourceBuilder {
    Params(Vec<KeyValueBuilder>),
    Value(ValueBuilder),
    /// (boxed, to keep every element small)
    PWL(Box<PWLBuilder>),
    /// Any other combination, e.g. `DC 0 AC 1 PULSE(0 1 0 1n 1n 5n 10n)`
    /// (boxed, to keep every element small)
    Spec(Box<SourceSpecBuilder>),
}
#[derive(Debug, Clone, Builder)]
pub struct CurrentBuilder {
    pub n1: Span,
    pub n2: Span,
    pub source: CurrentSourceBuilder,
}
#[derive(Debug, Clone, Builder)]
pub enum CurrentSourceBuilder {
    Params(Vec<KeyValueBuilder>),
    Value(ValueBuilder),
    /// (boxed, to keep every element small)
    PWL(Box<PWLBuilder>),
    /// Any other combination, e.g. `DC 0 AC 1 PULSE(0 1 0 1n 1n 5n 10n)`
    /// (boxed, to keep every element small)
    Spec(Box<SourceSpecBuilder>),
}
/// Independent source:
/// ``` spice
/// Vxxx n+ n- [[DC] dcval] [tranfun] [AC [=] acmag [acphase]] [M=val] ...
/// ```
#[derive(Debug, Clone, Default, Builder)]
pub struct SourceSpecBuilder {
    pub dc: Option<ValueBuilder>,
    pub ac: Option<ACBuilder>,
    pub tran: Option<TranSourceBuilder>,
    /// other parameters, e.g. `M=2`
    pub params: Vec<KeyValueBuilder>,
}
/// `AC acmag [acphase]`
#[derive(Debug, Clone, Builder)]
pub struct ACBuilder {
    pub mag: ValueBuilder,
    pub phase: Option<ValueBuilder>,
}
/// Transient function of an independent source
#[derive(Debug, Clone, Builder)]
pub enum TranSourceBuilder {
    PWL(PWLBuilder),
    /// `PULSE`, `SIN`, `EXP`, `SFFM`, `AM`, `PAT`, `LFSR`:
    /// the function name, its positional arguments and its `key=value` arguments
    Func {
        name: Span,
        args: Vec<ValueBuilder>,
        params: Vec<KeyValueBuilder>,
    },
}
#[derive(Debug, Clone, Builder)]
pub struct TimeValuePointBuilder {
    pub time: ValueBuilder,
    pub value: ValueBuilder,
}

/// https://eda-cpu1.eias.junzhuo.site/~junzhuo/hspice/index.htm#page/hspice_11/pwl_source.htm
#[derive(Debug, Clone, Default, Builder)]
pub struct PWLBuilder {
    pub points: Vec<TimeValuePointBuilder>,
    /// Keyword and time value to specify a repeating function.
    /// With no argument, the source repeats from the beginning of the function.
    /// repeat is the time, in units of seconds, which specifies
    /// the startpoint of the waveform to repeat. This time needs
    /// to be less than the greatest time point, tn.
    pub repeat: Option<ValueBuilder>,
    /// Specifies the stop time for the repeat.
    pub rstop: Option<ValueBuilder>,
    /// Specifies the value of the current/voltage source at the time of rstop.
    /// stopvalue can be either a real number or Z for high impedance state.
    pub stopvalue: Option<ValueBuilder>,
    /// stopeslope is the switching time from the last PWL value to the stopvalue.
    /// Default value is 30ps, if unspecified.
    pub stopslope: Option<ValueBuilder>,
    /// `TD=delay`
    ///
    /// Time, in units of seconds, which specifies the length of time to delay (propagation delay) the piecewise linear function.
    pub delay: Option<ValueBuilder>,
    /// `PWLFILE=file`: the points are read from this file
    pub pwlfile: Option<ValueBuilder>,
    pub edgetype: EdgeType,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum EdgeType {
    #[default]
    Linear,
    HalfSine,
}

self_builder!(EdgeType);

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
