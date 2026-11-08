//! The owned model that the queries read. [`crate::lower`] builds it from
//! the parser's builders.
//!
//! Names are interned ([`Sym`]): compare `Sym`s, never name strings, so
//! that comparisons ignore letter case. Texts (parameter values, function bodies)
//! are byte ranges ([`TextRef`]) of the file texts, which the model keeps.
//! The pins and the parameters of the elements of a definition are in two
//! arenas of that definition, so an element owns no heap memory.

use crate::{api::Level, intern::Sym};

/// A definition: the top level ([`ROOT`]) or one `.subckt`/`.macro`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct DefId(pub(crate) u32);

/// An element of a definition: an index into [`Def::elements`].
pub(crate) type ElemIdx = usize;

/// The top-level statements: an implicit definition with no name.
pub(crate) const ROOT: DefId = DefId(0);

/// The display name of [`ROOT`].
pub(crate) const ROOT_DISPLAY: &str = "(top level)";

impl DefId {
    #[inline]
    pub(crate) fn idx(self) -> usize {
        self.0 as usize
    }
}

/// A line of a loaded file: an index into `Netlist::files`, and the line
/// number (from 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LocIdx {
    pub(crate) file: u32,
    pub(crate) line: u32,
}

/// The bytes `start..end` of the text of `Netlist::files[file]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextRef {
    pub(crate) file: u32,
    pub(crate) start: u32,
    pub(crate) end: u32,
}

/// A range of an arena of a [`Def`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Range32 {
    pub(crate) start: u32,
    pub(crate) end: u32,
}

impl Range32 {
    #[inline]
    pub(crate) fn range(self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// One loaded file (or `.lib` section of a file). The line numbers of a
/// section are those of the whole file.
#[derive(Debug)]
pub(crate) struct SourceFile {
    /// The path, as the parser opened it.
    pub(crate) path: String,
    /// The `.lib` section, for a section.
    pub(crate) section: Option<String>,
    /// The text that the parser read: the whole file, or the body of the
    /// section.
    pub(crate) text: String,
}

/// `name=value`: a `.param`, a header default, an `X` override, an element
/// parameter or an `.option`.
#[derive(Debug, Clone)]
pub(crate) struct ParamDef {
    pub(crate) name: Sym,
    /// The value as written (an expression without its quotes). `None`
    /// for a number whose text was not found (then [`ParamDef::num`] is
    /// the value) and for an `.option` without a value.
    pub(crate) text: Option<TextRef>,
    /// The value when the text is a plain number (with its unit).
    pub(crate) num: Option<f64>,
    pub(crate) loc: LocIdx,
    /// The `.alter` block (numbered from 1 in document order) that holds
    /// this statement; `None` for the first run.
    pub(crate) alter: Option<u32>,
}

/// A `.param` function: `.param f(a,b)='a*b'`.
#[derive(Debug, Clone)]
pub(crate) struct FuncDef {
    pub(crate) name: Sym,
    pub(crate) args: Vec<Sym>,
    pub(crate) body: TextRef,
    pub(crate) loc: LocIdx,
    pub(crate) alter: Option<u32>,
}

/// A `.model` card. Its parameters are in [`Def::card_params`]; the
/// queries evaluate them on demand (`crate::params::cards`).
#[derive(Debug, Clone)]
pub(crate) struct ModelDef {
    pub(crate) name: Sym,
    /// The model type (`nmos`, `r`, …).
    pub(crate) kind: Sym,
    pub(crate) loc: LocIdx,
    pub(crate) alter: Option<u32>,
    /// The `level` parameter of the card (the last `level` key, golden
    /// `ind_level_twice`): `None` when the card has none, `Some(None)` when
    /// its value is not a number. HSPICE rejects an `L` line whose `l` card
    /// is not `level=3` (golden `ind_model_nolevel`).
    pub(crate) level: Option<Option<f64>>,
    /// The `key=value` items of the card, in order, in
    /// [`Def::card_params`] of the definition that holds the card.
    pub(crate) params: Range32,
}

/// One `key=value` of a `.model` card, in [`Def::card_params`]. A PDK
/// library holds millions of them, so this keeps 24 bytes of each (a
/// [`ParamDef`] has 56): the file is the file of the card, and the
/// `.alter` block is the card's. [`CardParam::param`] gives the full form.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CardParam {
    pub(crate) name: Sym,
    /// The line of the key.
    pub(crate) line: u32,
    /// The value as written: the bytes `start..end` of the card's file
    /// (an expression without its quotes). `start == end` when the parser
    /// kept a number whose text was not found: then `num` is the value.
    pub(crate) start: u32,
    pub(crate) end: u32,
    /// The value when the text is a plain number (with its unit); NaN when
    /// it is not (a deck cannot write a NaN).
    pub(crate) num: f64,
}

impl CardParam {
    /// The compact form of `p`, a parameter of a card (its text is in the
    /// file of the card).
    pub(crate) fn new(p: &ParamDef) -> Self {
        let (start, end) = p.text.map_or((0, 0), |t| (t.start, t.end));
        CardParam {
            name: p.name,
            line: p.loc.line,
            start,
            end,
            num: p.num.unwrap_or(f64::NAN),
        }
    }

    /// The full form, for the card `md` that holds it.
    pub(crate) fn param(&self, md: &ModelDef) -> ParamDef {
        let file = md.loc.file;
        ParamDef {
            name: self.name,
            text: (self.start < self.end).then_some(TextRef {
                file,
                start: self.start,
                end: self.end,
            }),
            num: (!self.num.is_nan()).then_some(self.num),
            loc: LocIdx {
                file,
                line: self.line,
            },
            alter: md.alter,
        }
    }
}

/// The `.option` items that choose the bin of a MOSFET, read as the model
/// selector reads them (`src/models/bins.rs`, `Options::new`): those of the
/// top level of the first run, the last plain number of each, where a value
/// of 0 or less gives the default back. HSPICE ignores other values (`'7u'`,
/// a name, an expression: decks `opt_scale_*`, `opd_*`, `os_*`, `zd_*`,
/// `zq_*`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BinOptions {
    /// `.option scale`: it multiplies L, W and the sheet width of a line.
    pub(crate) scale: f64,
    /// `.option scalm`: it multiplies the limits of a bin (not the fin
    /// counts).
    pub(crate) scalm: f64,
    /// `.option defl`, `.option defw`: the L and W of a line with none.
    pub(crate) defl: f64,
    pub(crate) defw: f64,
    /// `.option wnflag`: the bins compare W/NF unless it is 0 (by default
    /// they do). A bare `.option wnflag` is 1, and a number in quotes
    /// counts (decks `opw_*`).
    pub(crate) wnflag: bool,
}

impl Default for BinOptions {
    fn default() -> Self {
        BinOptions {
            scale: 1.0,
            scalm: 1.0,
            defl: 1e-4,
            defw: 1e-4,
            wnflag: true,
        }
    }
}

/// The type of an element line, from its first letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ElemKind {
    Resistor,
    Capacitor,
    Inductor,
    Vsource,
    Isource,
    Mosfet,
    Bjt,
    Diode,
    /// An `X` line.
    Instance,
    /// A letter that the parser does not read (`E`, `F`, `G`, `H`, `B`,
    /// `J`, `K`, `T`, `W`, `U`, `S`, …), in lowercase. Its node list is not
    /// trusted.
    Other(u8),
}

impl ElemKind {
    /// The type name in answers.
    pub(crate) fn name(self) -> &'static str {
        match self {
            ElemKind::Resistor => "resistor",
            ElemKind::Capacitor => "capacitor",
            ElemKind::Inductor => "inductor",
            ElemKind::Vsource => "vsource",
            ElemKind::Isource => "isource",
            ElemKind::Mosfet => "mosfet",
            ElemKind::Bjt => "bjt",
            ElemKind::Diode => "diode",
            ElemKind::Instance => "instance",
            ElemKind::Other(c) if c.is_ascii_lowercase() => LETTERS[(c - b'a') as usize],
            ElemKind::Other(_) => "other",
        }
    }
}

/// The type names of the letters the parser does not read: the letter.
const LETTERS: [&str; 26] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
    "t", "u", "v", "w", "x", "y", "z",
];

/// The role of a pin of an element, from its type and its place on the
/// line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PinRole {
    /// `d g s b` of a MOSFET, `c b e s` of a BJT, `p n` of a diode, `1 2`
    /// of a two-pin element.
    Fixed(&'static str),
    /// The nodes of a MOSFET after the 4th (`e1`, `e2`, …), from 1.
    Extra(u32),
    /// The port at this index of the definition that the `X` line binds
    /// to (the binding depends on the instance path).
    Port(u32),
    /// A node of a letter that the parser does not read (`n1`, `n2`, …),
    /// from 1.
    Node(u32),
}

/// The role of pin `idx` of an element of type `kind`.
pub(crate) fn pin_role(kind: ElemKind, idx: usize) -> PinRole {
    const MOS: [&str; 4] = ["d", "g", "s", "b"];
    const BJT: [&str; 4] = ["c", "b", "e", "s"];
    const DIODE: [&str; 2] = ["p", "n"];
    const TWO: [&str; 2] = ["1", "2"];
    let fixed = |names: &[&'static str]| {
        names
            .get(idx)
            .map_or(PinRole::Node(idx as u32 + 1), |n| PinRole::Fixed(n))
    };
    match kind {
        ElemKind::Mosfet if idx >= 4 => PinRole::Extra(idx as u32 - 3),
        ElemKind::Mosfet => fixed(&MOS),
        ElemKind::Bjt => fixed(&BJT),
        ElemKind::Diode => fixed(&DIODE),
        ElemKind::Resistor
        | ElemKind::Capacitor
        | ElemKind::Inductor
        | ElemKind::Vsource
        | ElemKind::Isource => fixed(&TWO),
        ElemKind::Instance => PinRole::Port(idx as u32),
        ElemKind::Other(_) => PinRole::Node(idx as u32 + 1),
    }
}

/// One element line.
#[derive(Debug, Clone)]
pub(crate) struct Element {
    /// The element name, with its letter.
    pub(crate) name: Sym,
    pub(crate) kind: ElemKind,
    /// The model name (R, C, L, M, Q, D).
    pub(crate) model: Option<Sym>,
    /// The subcircuit name of an `X` line. It is not bound to a
    /// definition here: HSPICE binds it along the instance path.
    pub(crate) callee: Option<Sym>,
    /// The nodes, in [`Def::pins`]. See [`pin_role`].
    pub(crate) pins: Range32,
    /// The parameters, in [`Def::elem_params`]: the value of R/C/L as `r`,
    /// `c`, `l` (one bare name after the nodes stays here, as written: it
    /// can be a parameter or a model, which parameter resolution decides),
    /// and the numbers after it as `tc1`, `tc2`; the DC value of V/I as
    /// `dc`, a source function by its name as written (`PULSE`, `pwl`, …)
    /// with its arguments as written (`Netlist::param_text` puts them on
    /// one line, see `crate::lower::one_line`), and the AC magnitude and
    /// phase as `ac` and `acphase`; the numbers after a model name by
    /// their names (`l`, `w`, … for M; `area` for Q; `area`, `pj` for D);
    /// then every `key=value` in order (overrides of an `X` line). A name
    /// is here once: its last value wins (golden `elem_dup_kw`), and the
    /// value of R/C/L keeps the text of the winning `r=`/`c=`/`l=`.
    pub(crate) params: Range32,
    pub(crate) loc: LocIdx,
    /// The `.alter` block that holds this line; `None` for the first run.
    pub(crate) alter: Option<u32>,
    /// A parasitic R or C of an included DSPF (a line of a `*|NET` block):
    /// not a device in counts and lists (see `crate::dspf`).
    pub(crate) parasitic: bool,
}

/// A definition: the top level ([`ROOT`]) or a `.subckt`/`.macro`.
#[derive(Debug, Clone)]
pub(crate) struct Def {
    /// The name (empty for the root). `Netlist::def_display` gives the
    /// name for answers.
    pub(crate) name: Sym,
    pub(crate) ports: Vec<Sym>,
    /// Header defaults (`.subckt inv a b wn=1u`).
    pub(crate) defaults: Vec<ParamDef>,
    /// `.param` statements in the body (top-level `.param` for the root).
    pub(crate) params: Vec<ParamDef>,
    pub(crate) funcs: Vec<FuncDef>,
    pub(crate) models: Vec<ModelDef>,
    /// The arena of the `key=value` items of `models`.
    pub(crate) card_params: Vec<CardParam>,
    pub(crate) elements: Vec<Element>,
    /// For the R, C and L lines of `elements` with a model word or a
    /// `key=value`: the element index and the byte right after its second
    /// node token in the text of its file, by element index. Parameter
    /// resolution reads the tokens after it again (`R1 a b rval 0.004
    /// r=5k`, `R1 a b 'rval' r=5k`): a `key=` value replaces the value after
    /// the nodes in `params`. A table, not a field of [`Element`], so that
    /// the other lines (most R and C lines of an extracted netlist) cost no
    /// memory for it.
    pub(crate) rcl_ends: Vec<(u32, u32)>,
    /// The arena of the pins of `elements`.
    pub(crate) pins: Vec<Sym>,
    /// The arena of the parameters of `elements`.
    pub(crate) elem_params: Vec<ParamDef>,
    /// The definitions written inside this one, in document order. For
    /// the root: the top-level definitions.
    pub(crate) nested: Vec<DefId>,
    /// The definition this one is written inside; `None` at the top level.
    pub(crate) parent: Option<DefId>,
    pub(crate) loc: LocIdx,
    /// The text of the definition, from the `.subckt` to the end of the
    /// `.ends` (or, with no `.ends`, to the `.end` line or the end of the
    /// file); empty for the root.
    pub(crate) text: TextRef,
    /// The line of the start of `text`.
    pub(crate) text_line: u32,
    /// The `.alter` block that holds the definition; `None` for the first
    /// run.
    pub(crate) alter: Option<u32>,
    /// For a later definition with the name of an earlier one in the same
    /// scope (HSPICE rejects the deck, golden `dup_subckt`): the earlier,
    /// active one.
    pub(crate) dup_of: Option<DefId>,
    /// A later definition in the same scope has this name.
    pub(crate) has_dup: bool,
    /// The body holds an `.if` block: two lines with one name can be in
    /// different branches (`duplicate_element` is then a Warning).
    pub(crate) has_if: bool,
}

impl Def {
    /// Frees the spare capacity of the lists, once the body is lowered.
    pub(crate) fn shrink(&mut self) {
        self.elements.shrink_to_fit();
        self.pins.shrink_to_fit();
        self.elem_params.shrink_to_fit();
        self.params.shrink_to_fit();
        self.card_params.shrink_to_fit();
    }

    /// The `key=value` items of card `md`, a card of this definition.
    pub(crate) fn card_params(&self, md: &ModelDef) -> &[CardParam] {
        &self.card_params[md.params.range()]
    }

    /// The nodes of `e`, an element of this definition.
    pub(crate) fn element_pins(&self, e: &Element) -> &[Sym] {
        &self.pins[e.pins.range()]
    }

    /// The parameters of `e`, an element of this definition.
    pub(crate) fn element_params(&self, e: &Element) -> &[ParamDef] {
        &self.elem_params[e.params.range()]
    }

    /// The byte right after the second node token of R, C or L line `i`
    /// (see [`Def::rcl_ends`]).
    pub(crate) fn nodes_end(&self, i: ElemIdx) -> Option<usize> {
        let i = u32::try_from(i).ok()?;
        let at = self.rcl_ends.binary_search_by_key(&i, |&(e, _)| e).ok()?;
        Some(self.rcl_ends[at].1 as usize)
    }
}

/// A name that an analysis of the first run sweeps (spec 6.2a): a
/// parameter, a source, or `temp`.
#[derive(Debug, Clone)]
pub(crate) struct Swept {
    /// The swept name.
    pub(crate) name: Sym,
    /// The analysis command: `dc`, `tran`, `ac` or `noise`.
    pub(crate) cmd: &'static str,
    /// The analysis line.
    pub(crate) loc: LocIdx,
    /// The `.data` block that holds the name, when the analysis sweeps a
    /// `.data` block (`.dc data=d1`): its name and line.
    pub(crate) data: Option<(Sym, LocIdx)>,
}

/// The root of the instance paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Root {
    /// The top level has element lines.
    TopLevel,
    /// `LoadOptions::top`.
    Subckt(DefId),
    /// A bare netlist: definitions only.
    None,
}

/// `.option parhier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Parhier {
    /// The outermost definition wins (HSPICE's default).
    Global,
    /// The innermost definition wins.
    Local,
}

impl Parhier {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Parhier::Global => "global",
            Parhier::Local => "local",
        }
    }
}

/// A diagnostic, with its location as indices.
#[derive(Debug, Clone)]
pub(crate) struct Diag {
    pub(crate) level: Level,
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) loc: Option<LocIdx>,
}

#[cfg(test)]
mod tests {
    /// An `Element` stays 56 bytes: an extracted netlist has tens of
    /// millions of them (the R/C/L node end is in [`super::Def::rcl_ends`]).
    #[test]
    fn element_size() {
        assert_eq!(std::mem::size_of::<super::Element>(), 56);
    }

    /// A `CardParam` stays 24 bytes: a PDK library has millions of card
    /// parameters.
    #[test]
    fn card_param_size() {
        assert_eq!(std::mem::size_of::<super::CardParam>(), 24);
    }
}
