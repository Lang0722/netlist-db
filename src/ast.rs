use std::sync::{Arc, OnceLock};

use crate::{
    err::ParseError,
    instance::InstanceBuilder,
    self_builder,
    span::{ParsedId, Pos, Span},
};
use netlist_macros::Builder;

#[derive(Debug, Clone, Copy, Builder)]
pub enum ValueBuilder {
    Num(f64),
    Expr(Span),
}

#[derive(Debug, Clone, Default, Copy, Builder)]
pub struct KeyValueBuilder {
    pub k: Span,
    pub v: ValueBuilder,
}

#[derive(Debug, Clone, Copy, Builder)]
pub enum TokenBuilder {
    KV(KeyValueBuilder),
    Value(ValueBuilder),
    V(Span),
    I(Span),
}

#[derive(Debug, Clone, Copy)]
pub enum GeneralCmd {
    /// `.ic` initial condition
    Ic,
    /// `.ic` initial condition
    Meas,
}
self_builder!(GeneralCmd);
#[derive(Debug, Clone, Builder)]
pub struct GeneralBuilder {
    pub cmd: GeneralCmd,
    pub tokens: Vec<TokenBuilder>,
}

/// A `.param` function definition: `.param f(a,b)='a*b'`.
#[derive(Debug, Clone, Builder)]
pub struct ParamFuncBuilder {
    pub name: Span,
    pub args: Vec<Span>,
    /// The expression, without its quotes.
    pub body: Span,
}

#[derive(Debug, Clone, Builder)]
pub struct UnknwonBuilder {
    pub cmd: Span,
    pub tokens: Vec<TokenBuilder>,
}

#[derive(Debug, Clone, Builder)]
pub struct ModelBuilder {
    pub name: Span,
    pub model_type: ModelTypeBuilder,
    pub params: Vec<KeyValueBuilder>,
}

#[derive(Debug, Clone, Copy, Builder)]
pub enum ModelTypeBuilder {
    /// operational amplifier model
    AMP,
    /// capacitor model
    C,
    /// magnetic core model
    CORE,
    /// diode model
    D,
    /// inductor model or magnetic core mutual inductor model
    L,
    /// n-channel JFET model
    NJF,
    /// n-channel MOSFET model
    NMOS,
    /// npn BJT model
    NPN,
    /// optimization model
    OPT,
    /// p-channel JFET model
    PJF,
    /// p-channel MOSFET model
    PMOS,
    /// pnp BJT model
    PNP,
    /// resistor model
    R,
    /// lossy transmission line model (lumped)
    U,
    /// lossy transmission line model
    W,
    /// S-parameter
    S,
    Unknown(Span),
}

#[derive(Debug, Clone, Builder)]
pub struct DataBuilder {
    pub name: Span,
    pub values: DataValuesBuilder,
}
#[derive(Debug, Clone, Builder)]
pub enum DataValuesBuilder {
    InlineExpr {
        params: Vec<Span>,
        values: Vec<ValueBuilder>,
    },
    InlineNum {
        params: Vec<Span>,
        values: Vec<f64>,
    },
    /// https://eda-cpu1.eias.junzhuo.site/~junzhuo/hspice/index.htm#page/hspice_14/data.htm
    /// Concatenated (series merging) data files to use.
    MER(DataFilesBuilder),
    /// Column-laminated (parallel merging) data files to use.
    LAM(DataFilesBuilder),
}
#[derive(Debug, Clone, Builder)]
pub struct DataFilesBuilder {
    pub files: Vec<DataFileBuilder>,
    pub out: Option<Span>,
}
#[derive(Debug, Clone, Builder)]
pub struct DataFileBuilder {
    pub file: Span,
    pub pname_col_num: Vec<PnameColNumBuilder>,
}

#[derive(Debug, Clone, Builder)]
pub struct PnameColNumBuilder {
    pub pname: Span,
    pub col_num: usize,
}

/// ``` spice
/// .subckt pulvt11ll_ckt d g s b w=1e-6 l=1e-6 sa='sar'
/// ...
/// .ends pulvt11ll_ckt
/// ```
/// Do NOT support `.include` / `.lib` in `.subckt`
#[derive(Debug)]
pub struct SubcktBuilder {
    pub name: Span,
    /// subckt/model name is the last arg
    pub ports: Vec<Span>,
    pub params: Vec<KeyValueBuilder>,
    pub ast: ASTBuilder,
    /// The text of the definition in its file: from the `.` of `.subckt`
    /// (or `.macro`) to the end of the `.ends` (or `.eom`) statement. With
    /// no `.ends`, to the `.end` line or to the end of the input.
    /// `line_num` is the line of the `.subckt`.
    pub text: Span,
}

/// The `.include` and `.lib file tt` will be directly evaluated
#[derive(Debug, Default)]
pub struct LocalAST {
    pub subckt: Vec<SubcktBuilder>,
    pub instance: Vec<InstanceBuilder>,
    pub model: Vec<ModelBuilder>,
    pub param: Vec<KeyValueBuilder>,
    /// `.param` function definitions
    pub param_func: Vec<ParamFuncBuilder>,
    /// `.global` node names
    pub global: Vec<Span>,
    /// The first and the last line of each `.prot`/`.protect` …
    /// `.unprot`/`.unprotect` region. The text in it is not parsed.
    pub protected: Vec<(u32, u32)>,
    pub option: Vec<(Span, Option<ValueBuilder>)>,
    pub data: Vec<DataBuilder>,
    pub general: Vec<GeneralBuilder>,
    pub init_condition: Vec<(Span, ValueBuilder, Option<Span>)>,
    pub nodeset: Vec<(Span, ValueBuilder, Option<Span>)>,
    pub unknwon: Vec<UnknwonBuilder>,
    pub errors: Vec<ParseError>,
    /// The lines of the `.end` statements of a file that is not a top
    /// file (an `.include` file or a `.lib` section). HSPICE ignores them,
    /// with a warning, and reads the lines after them (golden
    /// `end_in_include`); so does the parse.
    pub ignored_end: Vec<u32>,
}

impl LocalAST {
    pub fn is_empty(&self) -> bool {
        self.subckt.is_empty()
            && self.instance.is_empty()
            && self.model.is_empty()
            && self.param.is_empty()
            && self.param_func.is_empty()
            && self.global.is_empty()
            && self.protected.is_empty()
            && self.option.is_empty()
            && self.data.is_empty()
            && self.general.is_empty()
            && self.init_condition.is_empty()
            && self.nodeset.is_empty()
            && self.unknwon.is_empty()
            && self.errors.is_empty()
            && self.ignored_end.is_empty()
    }
}

#[derive(Debug)]
pub enum Segment {
    Local(Box<LocalAST>),
    /// An `.include` or `.lib <file> <section>` line: the parse of that
    /// file or section, and the position of the line (at its end).
    Include {
        res: Arc<OnceLock<Result<ParsedId, ParseError>>>,
        pos: Option<Pos>,
    },
    /// An `.alter` line. The segments after it, up to the next `Alter`,
    /// belong to that `.alter` run. `index` counts the `.alter` lines of
    /// the file from 1 (`.alter` #1 describes the second run).
    Alter {
        index: usize,
        pos: Option<Pos>,
    },
}

/// What the parse did with line 1 of a top file (see
/// `ParseOptions::title_line`).
#[derive(Debug, Clone)]
pub struct TitleLineInfo {
    /// Line 1, without its line end.
    pub line: Span,
    /// `true` when line 1 is the title line and was not parsed.
    pub skipped: bool,
    /// One sentence: what was done with line 1, and why.
    pub message: String,
}

/// The `.end` line of a top file. HSPICE does not read the lines after it
/// (golden `after_end`), so the parse stops there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AfterEnd {
    /// The line of `.end`.
    pub line: u32,
    /// The lines after `.end` that are not empty, not comments and not in
    /// a `.lib <section>` block (which a `.lib` call reads).
    pub lines: u32,
}

#[derive(Debug, Default)]
pub struct ASTBuilder {
    pub segments: Vec<Segment>,
    /// Set for a top file whose line 1 has text, when the parse looked for
    /// a title line (`TitleLine::Auto` or `TitleLine::Yes`).
    pub title: Option<TitleLineInfo>,
    /// Set for a top file with an `.end` line.
    pub after_end: Option<AfterEnd>,
    /// The first line with bytes that are not UTF-8, when the file (or
    /// the `.lib` section) has such bytes. They are read as U+FFFD, and the
    /// statements are kept.
    pub non_utf8: Option<u32>,
}

impl ASTBuilder {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for ValueBuilder {
    #[inline]
    fn default() -> Self {
        Self::Num(0.0)
    }
}

impl From<(&str, Span)> for ModelTypeBuilder {
    #[inline]
    fn from(value: (&str, Span)) -> Self {
        let (_str, _type) = value;
        match _str.to_uppercase().as_str() {
            "AMP" => Self::AMP,
            "C" => Self::C,
            "CORE" => Self::CORE,
            "D" => Self::D,
            "L" => Self::L,
            "NJF" => Self::NJF,
            "NMOS" => Self::NMOS,
            "NPN" => Self::NPN,
            "OPT" => Self::OPT,
            "PJF" => Self::PJF,
            "PMOS" => Self::PMOS,
            "PNP" => Self::PNP,
            "R" => Self::R,
            "U" => Self::U,
            "W" => Self::W,
            "S" => Self::S,
            _ => Self::Unknown(_type),
        }
    }
}
