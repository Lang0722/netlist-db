//! Public output types and paging.

use serde::Serialize;
use std::{collections::BTreeMap, fmt};

/// The default number of items in one page.
pub const DEFAULT_LIMIT: usize = 50;
/// The largest number of items in one page.
pub const MAX_LIMIT: usize = 500;

/// A place in a loaded file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Loc {
    /// The path, as the parser opened it.
    pub file: String,
    /// The line number, from 1.
    pub line: u32,
}

/// Which part of a list to return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The number of items, at most [`MAX_LIMIT`]. A larger value is
    /// [`MAX_LIMIT`]. 0 gives no items, only the total.
    pub limit: usize,
    /// The `next_cursor` of the previous page (a decimal offset), or `None`
    /// for the first page.
    pub cursor: Option<String>,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            cursor: None,
        }
    }
}

/// One page of a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Paged<T> {
    pub items: Vec<T>,
    /// The number of items in the whole list.
    pub total: usize,
    /// The cursor of the next page; `None` on the last page.
    pub next_cursor: Option<String>,
}

impl<T> Paged<T> {
    /// The same page with `f` applied to each item.
    pub fn map<U>(self, f: impl FnMut(T) -> U) -> Paged<U> {
        Paged {
            items: self.items.into_iter().map(f).collect(),
            total: self.total,
            next_cursor: self.next_cursor,
        }
    }
}

/// The page `page` of `items`. Only the items of the page are taken from
/// the iterator, so map the items after paging when that costs.
pub(crate) fn page<T>(
    items: impl ExactSizeIterator<Item = T>,
    page: &Page,
) -> Result<Paged<T>, QueryError> {
    let total = items.len();
    let offset = match &page.cursor {
        None => 0,
        Some(cursor) => cursor.trim().parse::<usize>().map_err(|_| {
            QueryError::BadArgument(format!(
                "cursor `{cursor}` is not a decimal offset: pass the `next_cursor` of the previous page"
            ))
        })?,
    };
    let limit = page.limit.min(MAX_LIMIT);
    let items: Vec<T> = items.skip(offset).take(limit).collect();
    let end = offset.saturating_add(items.len());
    let next_cursor = (limit > 0 && end < total).then(|| end.to_string());
    Ok(Paged {
        items,
        total,
        next_cursor,
    })
}

/// A query that cannot be answered. Every query returns
/// `Result<_, QueryError>`. The lists of many items (subckt rows, element
/// lines, instances, diagnostics) are [`Paged`]. Inside one answer, every
/// other list (ports, pins, children, header defaults, `.param` lines,
/// functions, element parameters, `X` overrides) holds at most
/// [`MAX_LIMIT`] items, and the model names of a device type at most 50,
/// each with its total; the caveats of a parameter answer are at most 50
/// sentences (a last one counts the others). No list in an answer is
/// unbounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum QueryError {
    /// No `what` (subckt, element, net, …) is named `name`.
    NotFound {
        what: String,
        name: String,
        /// Names that are close to `name`.
        suggestions: Vec<String>,
    },
    /// An argument has a value that the query does not accept.
    BadArgument(String),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryError::NotFound {
                what,
                name,
                suggestions,
            } => {
                write!(f, "no {what} named `{name}`")?;
                if !suggestions.is_empty() {
                    write!(f, "; did you mean {}?", suggestions.join(", "))?;
                }
                Ok(())
            }
            QueryError::BadArgument(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for QueryError {}

/// A netlist that cannot be loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum LoadError {
    /// The top file cannot be read.
    Io(String),
    /// `LoadOptions::top` names no definition, or several.
    NoTopLevel(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(msg) | LoadError::NoTopLevel(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for LoadError {}

/// The severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// The deck is wrong, or HSPICE rejects it, or part of it is missing.
    Error,
    /// The answers can be incomplete.
    Warning,
    /// What the loader did, for information.
    Info,
}

impl std::str::FromStr for Level {
    type Err = QueryError;

    /// `error`, `warning` or `info`, in any letter case.
    fn from_str(s: &str) -> Result<Self, QueryError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" => Ok(Level::Error),
            "warning" => Ok(Level::Warning),
            "info" => Ok(Level::Info),
            _ => Err(QueryError::BadArgument(format!(
                "level `{s}` is not one of `error`, `warning`, `info`"
            ))),
        }
    }
}

/// One problem or note about the loaded netlist.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diagnostic {
    pub level: Level,
    /// A short fixed name of the kind of problem (`syntax_error`,
    /// `duplicate_subckt`, …).
    pub code: &'static str,
    /// One or two plain-English sentences.
    pub message: String,
    pub loc: Option<Loc>,
}

/// An overview of the loaded netlist.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    /// The number of files read (a file read as a whole and as `.lib`
    /// sections counts once).
    pub files: usize,
    /// The number of `.subckt`/`.macro` definitions of the first run,
    /// nested ones and duplicates included.
    pub definitions: usize,
    /// The number of definitions inside `.alter` blocks (not in the first
    /// run, so not in `definitions`).
    pub alter_definitions: usize,
    /// `top-level`, `subckt:<name>` (`LoadOptions::top`) or `none` (a bare
    /// netlist: no element line at the top level).
    pub root: String,
    /// The number of `X` lines at the top level (first run).
    pub top_level_instances: usize,
    /// The devices of the expanded hierarchy (first run), by type
    /// (`resistor`, `mosfet`, …; a letter the parser does not read by its
    /// letter, `e`): Σ copies × the devices of each definition. `m` is not
    /// multiplied in. `None` when no number exists: see `expanded_note`.
    pub expanded_devices: Option<BTreeMap<String, u64>>,
    /// The number of subckt instances of the expanded hierarchy (the
    /// entries of HSPICE's circuit name directory). `None` when no number
    /// exists: see `expanded_note`.
    pub expanded_instances: Option<u64>,
    /// Why the expanded totals are `None`, or caveats on them, joined by
    /// `; `. `None` totals: `no top-level instances` (a bare netlist), a
    /// call cycle on an instance path (diagnostic `subckt_cycle`),
    /// `overflow` (more than 2^64 instances or devices), or too many lookup
    /// contexts (diagnostic `hierarchy_too_large`). Caveats: a lower bound
    /// (`X` lines with no visible definition on some instance paths, whose
    /// instances are not counted: diagnostic `unresolved_subckt`); `X`
    /// lines that can call a Verilog-A module or a model of one, counted
    /// neither as subckt instances nor as devices; `X` lines whose node
    /// count is not the port count of their subckt (diagnostic
    /// `port_count_mismatch`), lines with `m` <= 0 (`m_not_positive`) and
    /// lines with the name of an earlier line of their definition
    /// (`duplicate_element`), all counted, though HSPICE rejects the deck;
    /// lines with another form that HSPICE rejects (diagnostics
    /// `undefined_param`, `override_self`, `statistical_in_element`,
    /// `name_then_model`, `inductor_model_level`; the counts do not
    /// change); load errors (a statement that an error skipped is not
    /// counted); and `.prot` regions (not parsed, so not counted).
    pub expanded_note: Option<String>,
    /// `.option parhier` of the first run: `global` (the default) or
    /// `local`.
    pub parhier: String,
    /// The number of diagnostics of all levels.
    pub diagnostics_count: usize,
    /// The number of `error` diagnostics.
    pub errors: usize,
    /// The number of `.alter` blocks (the runs after the first one).
    /// Answers describe the first run.
    pub alter_runs: usize,
    /// The parasitic R and C of included DSPF files in the expanded
    /// hierarchy, by type (not in `expanded_devices`). `None` when there are
    /// none, or when `expanded_devices` is `None`.
    pub expanded_parasitics: Option<BTreeMap<String, u64>>,
    /// The DSPF files: included ones (detected by a `*|DSPF` line in their
    /// first 50 lines) and `.option sim_dspf` ones.
    pub dspf: Vec<DspfFile>,
}

/// The order of [`crate::Netlist::list_subckts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SubcktSort {
    /// By qualified name, ignoring letter case.
    #[default]
    Name,
    /// Most copies first; the definitions with no number last.
    Copies,
    /// Most devices (in the definition itself) first.
    Devices,
}

impl std::str::FromStr for SubcktSort {
    type Err = QueryError;

    /// `name`, `copies` or `devices`, in any letter case.
    fn from_str(s: &str) -> Result<Self, QueryError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "name" => Ok(SubcktSort::Name),
            "copies" => Ok(SubcktSort::Copies),
            "devices" => Ok(SubcktSort::Devices),
            _ => Err(QueryError::BadArgument(format!(
                "sort `{s}` is not one of `name`, `copies`, `devices`"
            ))),
        }
    }
}

/// One definition in [`crate::Netlist::list_subckts`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubcktRow {
    /// The qualified name: a nested definition after the names of the
    /// definitions it is written inside, joined by `>` (`outer>cell`).
    pub name: String,
    /// The number of ports.
    pub ports: usize,
    /// The number of devices in the definition itself (element lines that
    /// are not `X` lines).
    pub devices: usize,
    /// The number of `X` lines in the deck that call this definition on
    /// some instance path from the root. An `X` line in a definition that
    /// the root does not reach counts when it binds to this definition in
    /// one of the contexts where that definition was checked (as if
    /// instantiated at the top level, or as called from definitions with
    /// no instances). `None` when the hierarchy was not analysed
    /// (diagnostic `hierarchy_too_large`).
    pub x_lines: Option<usize>,
    /// The number of instances in the expanded hierarchy (0: unused). `m`
    /// is not multiplied in. `None` when no number exists: see
    /// `copies_note`.
    pub copies: Option<u64>,
    /// Why `copies` is `None` (`no top-level instances`, a call cycle,
    /// `overflow`, a duplicate definition), or a caveat on it.
    pub copies_note: Option<String>,
    pub loc: Loc,
}

/// A name and its value as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NameText {
    pub name: String,
    /// The text as written, on one line, without quotes.
    pub text: String,
}

/// The devices of one type in a definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceGroup {
    /// `resistor`, `mosfet`, …, or the letter of a type that the parser
    /// does not read.
    #[serde(rename = "type")]
    pub r#type: String,
    pub count: usize,
    /// The model names, each once, in the order of their first use (at
    /// most 50; see `models_total`).
    pub models: Vec<String>,
    /// The number of distinct model names.
    pub models_total: usize,
}

/// The `X` lines of a definition that call one subckt name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChildCall {
    /// The subckt name as the `X` lines write it.
    pub subckt: String,
    /// The number of `X` lines.
    pub x_lines: usize,
    /// The qualified names of the definitions these lines bind to on the
    /// instance paths from the root. HSPICE binds an `X` line along the
    /// instance path, so one line can bind to different definitions on
    /// different paths. In a definition that the root does not reach: the
    /// bindings in the contexts where it was checked (as if instantiated at
    /// the top level, or as called from definitions with no instances).
    /// `None` when the hierarchy was not analysed (diagnostic
    /// `hierarchy_too_large`).
    pub binds_to: Option<Vec<String>>,
    /// On some instance path from the root (in a definition that the root
    /// does not reach: in some context where it was checked), no
    /// definition of the name is visible. `None` when the hierarchy was not
    /// analysed.
    pub unresolved: Option<bool>,
}

/// One element line, in brief.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ElementBrief {
    pub name: String,
    /// `resistor`, `mosfet`, `instance`, …, or the letter of a type that
    /// the parser does not read.
    pub kind: String,
    pub model: Option<String>,
    /// The subckt name of an `X` line, as written.
    pub callee: Option<String>,
    pub loc: Loc,
}

/// The answer of [`crate::Netlist::subckt_info`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubcktInfo {
    /// The qualified name (see [`SubcktRow::name`]).
    pub name: String,
    pub loc: Loc,
    /// The ports, in order (at most [`MAX_LIMIT`]; see `ports_total`).
    pub ports: Vec<String>,
    /// The number of ports.
    pub ports_total: usize,
    /// The header defaults (`.subckt inv a b wn=1u`), at most
    /// [`MAX_LIMIT`]; see `defaults_total`.
    pub defaults: Vec<NameText>,
    pub defaults_total: usize,
    /// The `.param` statements of the body (first run), at most
    /// [`MAX_LIMIT`]; see `params_total`.
    pub params: Vec<NameText>,
    pub params_total: usize,
    /// The `.param` functions of the body (first run): `f(a,b)` and its
    /// body, at most [`MAX_LIMIT`]; see `funcs_total`.
    pub funcs: Vec<NameText>,
    pub funcs_total: usize,
    /// The devices (not `X` lines), by type, most first. The parasitic R
    /// and C of an included DSPF are not here: see `parasitics`.
    pub devices_by_type: Vec<DeviceGroup>,
    /// The `X` lines, by the subckt name they call, in the order of the
    /// first line (at most [`MAX_LIMIT`]; see `children_total`).
    pub children: Vec<ChildCall>,
    /// The number of distinct subckt names that the `X` lines call.
    pub children_total: usize,
    /// See [`SubcktRow`].
    pub x_lines: Option<usize>,
    pub copies: Option<u64>,
    pub copies_note: Option<String>,
    /// What makes the answer less certain (a duplicate definition, element
    /// names of more than one line).
    pub caveats: Vec<String>,
    /// The element lines of the first run, in order, without the
    /// parasitic R and C of an included DSPF.
    pub elements: Paged<ElementBrief>,
    /// The parasitics of an included DSPF whose `*|NET` blocks are in this
    /// definition.
    pub parasitics: Option<DefParasitics>,
}

/// The answer of [`crate::Netlist::subckt_text`]: the text of a
/// definition, as written in its file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubcktText {
    /// The qualified name (see [`SubcktRow::name`]).
    pub name: String,
    /// The file, and the line of the `.subckt` (or `.macro`).
    pub loc: Loc,
    /// The line number of the first line of this page.
    pub first_line: u32,
    /// A page of the lines, from the `.subckt` line to the `.ends` line,
    /// without line ends. The lines of nested definitions are included.
    /// `total` is the number of lines.
    pub lines: Paged<String>,
    /// What the text does not show (the text of `.include` and `.lib`
    /// files), or a definition with no `.ends`.
    pub caveats: Vec<String>,
}

/// An `X` line that calls a definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstanceRef {
    /// The qualified name of the definition that holds the line
    /// (`(top level)` for the top level).
    pub parent: String,
    /// The instance name.
    pub instance: String,
    pub loc: Loc,
    /// The parameters of the line, `m` excepted (at most [`MAX_LIMIT`]; see
    /// `overrides_total`).
    pub overrides: Vec<NameText>,
    pub overrides_total: usize,
    /// The `m` value of the line, as written. It is reported, never
    /// multiplied into copies.
    pub m: Option<String>,
    /// The instances of the definition that this line makes in the
    /// expanded hierarchy (the copies of the parent on the instance paths
    /// where the line binds to the definition; 0 in a parent that the root
    /// does not reach). `None` when no number exists (see
    /// `list_subckts`).
    pub copies: Option<u64>,
    /// What makes the answer less certain (a duplicate definition of the
    /// called subckt or of the parent, `m` <= 0, a line name of more than
    /// one line).
    pub caveats: Vec<String>,
}

/// A pin of an element and the net on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PinNet {
    /// `d g s b` (MOSFET), `c b e s` (BJT), `p n` (diode), `1 2` (two
    /// pins), `e1`, `e2`, … (MOSFET nodes after the 4th), the port name
    /// (`X` line), or `n1`, `n2`, … (a type that the parser does not
    /// read).
    pub role: String,
    pub net: String,
}

/// The answer of [`crate::Netlist::element_info`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ElementInfo {
    /// The instance path, lowercase.
    pub path: String,
    /// `resistor`, `mosfet`, `instance`, …, or the letter of a type that
    /// the parser does not read.
    pub kind: String,
    /// The qualified name of the definition that holds the element.
    pub def: String,
    /// The pins, in order (at most [`MAX_LIMIT`]; see `pins_total`).
    pub pins: Vec<PinNet>,
    /// The number of pins.
    pub pins_total: usize,
    pub model: Option<String>,
    /// The subckt name of an `X` line, as written.
    pub callee: Option<String>,
    /// The qualified name of the definition that the `X` line binds to on
    /// this path; `None` when no definition is visible.
    pub binds_to: Option<String>,
    /// The parameters as written (`r` for the R value, `dc` for a source,
    /// overrides of an `X` line, …), at most [`MAX_LIMIT`]; see
    /// `params_total`. An R, C or L line is read as HSPICE reads the tokens
    /// after its nodes, as `resolve_element` does: `R1 1 0 rval 0.004`
    /// with no `.model rval` is `r=rval`, `tc1=0.004`, with no model.
    pub params: Vec<NameText>,
    pub params_total: usize,
    pub loc: Loc,
    /// What makes the answer less certain.
    pub caveats: Vec<String>,
}

/// One pin on a net, in [`NetInfo::connections`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Conn {
    /// The element name, as written.
    pub element: String,
    /// `resistor`, `mosfet`, `instance`, …, or the letter of a type that
    /// the parser does not read.
    pub kind: String,
    /// The pin: `d g s b` (MOSFET), `c b e s` (BJT), `p n` (diode), `1 2`
    /// (two pins), `e1`, … (MOSFET nodes after the 4th), the port name of
    /// the definition that an `X` line binds to on this path, or `n1`, `n2`,
    /// … (a type that the parser does not read, or an `X` line with no port
    /// names: see the caveats).
    pub pin_role: String,
    /// The subckt name of an `X` line, as written.
    pub callee: Option<String>,
    /// The node of the pin, as the deck spells it: a DSPF sub-node (`A:1`)
    /// or pin node (`MN1:G`) of the net. Only in [`NetView::Physical`].
    pub node: Option<String>,
}

/// Which nodes [`crate::Netlist::net_info`] lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NetView {
    /// The logical net: in a definition with an included DSPF, every
    /// sub-node and pin node of its `*|NET` block, without the parasitic R
    /// and C (they are in [`NetInfo::parasitics`]).
    #[default]
    Logical,
    /// The nodes as written, with the parasitic R and C: all the nodes of a
    /// DSPF net (each pin gives its [`Conn::node`]), or one sub-node.
    Physical,
}

impl std::str::FromStr for NetView {
    type Err = QueryError;

    /// `logical` or `physical`, in any letter case.
    fn from_str(s: &str) -> Result<Self, QueryError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "logical" => Ok(NetView::Logical),
            "physical" => Ok(NetView::Physical),
            _ => Err(QueryError::BadArgument(format!(
                "view `{s}` is not one of `logical`, `physical`"
            ))),
        }
    }
}

/// The parasitics of one DSPF net (`*|NET` block), in
/// [`NetInfo::parasitics`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Parasitics {
    /// The DSPF file, as the deck names it.
    pub file: String,
    /// The total capacitance on the `*|NET` line (farads).
    pub total_cap: Option<f64>,
    /// The parasitic resistors of the block, and the sum of their values
    /// (ohms; `None` when a value was not read as a number, see
    /// `caveats`).
    pub r_count: u64,
    pub r_sum: Option<f64>,
    /// The parasitic capacitors of the block, and the sum of their values
    /// (farads; `None` as for `r_sum`).
    pub c_count: u64,
    pub c_sum: Option<f64>,
    /// The `*|P` (port) and `*|I` (instance pin) lines of the block.
    pub pins: u32,
    /// What makes these numbers less certain.
    pub caveats: Vec<String>,
}

/// One DSPF file, in [`Summary::dspf`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DspfFile {
    /// The path, as the deck names it.
    pub file: String,
    /// `included` (read as netlist text: the DSPF is the deck, or a
    /// `.include`) or `sim_dspf` (`.option sim_dspf`: only its `*|NET`
    /// blocks are read, to give parasitics to the nets of a pre-layout
    /// netlist).
    pub mode: String,
    /// The scope of `.option sim_dspf="<scope> <file>"` as written: a subckt
    /// name (every instance of it) or an instance path. `None`: the top
    /// level, or an included DSPF.
    pub scope: Option<String>,
    /// The definition that gets the parasitics (`None`: the top level).
    pub subckt: Option<String>,
    /// The `*|NET` blocks.
    pub nets: usize,
    /// The parasitic resistors and capacitors of the blocks.
    pub resistors: u64,
    pub capacitors: u64,
    /// The `*|NET` blocks whose net is not a net of that definition
    /// (diagnostic `dspf_net_unmatched`).
    pub unmatched_nets: usize,
}

/// The DSPF parasitics of one definition, in [`SubcktInfo::parasitics`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefParasitics {
    /// The `*|NET` blocks.
    pub nets: usize,
    /// The parasitic R and C lines in it (not in `devices_by_type`, nor in
    /// `elements`).
    pub resistors: u64,
    pub capacitors: u64,
}

/// The answer of [`crate::Netlist::net_info`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NetInfo {
    /// The instance path of the scope, lowercase (`""`: the root).
    pub scope: String,
    /// The qualified name of the definition of the scope.
    pub def: String,
    /// The net, as the deck first spells it.
    pub net: String,
    /// A port of the definition.
    pub is_port: bool,
    /// A `.global` name: the same net at every level.
    pub is_global: bool,
    /// Ground (`0`, `gnd`, `gnd!`, `ground`, any letter case): one net at
    /// every level.
    pub is_ground: bool,
    /// An element of a type that the parser does not read appears to touch
    /// the net: its node list is not trusted, so the list can miss pins.
    pub connectivity_incomplete: bool,
    /// Those elements (at most [`MAX_LIMIT`]).
    pub incomplete_due_to: Vec<String>,
    /// The number of pins on the net in this scope.
    pub total: usize,
    /// The pins, in line order.
    pub connections: Paged<Conn>,
    /// The DSPF parasitics of the net: its `*|NET` block in an included
    /// DSPF of the definition, or in a `.option sim_dspf` file for this
    /// scope.
    pub parasitics: Option<Parasitics>,
    /// What makes the answer less certain.
    pub caveats: Vec<String>,
}

/// Which way [`crate::Netlist::trace_net`] follows a net.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceDir {
    /// From a net into the instances whose `X` lines have a pin on it.
    Down,
    /// From a port of the scope to the net of the parent on that pin.
    Up,
    /// Both, from every net reached.
    Both,
}

impl std::str::FromStr for TraceDir {
    type Err = QueryError;

    /// `down`, `up` or `both`, in any letter case.
    fn from_str(s: &str) -> Result<Self, QueryError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "down" => Ok(TraceDir::Down),
            "up" => Ok(TraceDir::Up),
            "both" => Ok(TraceDir::Both),
            _ => Err(QueryError::BadArgument(format!(
                "direction `{s}` is not one of `down`, `up`, `both`"
            ))),
        }
    }
}

/// One step of a net through a port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hop {
    pub from_scope: String,
    pub from_net: String,
    /// The path of the instance whose port the step goes through.
    pub via_instance: String,
    pub to_scope: String,
    pub to_net: String,
    /// `down` or `up`.
    pub direction: String,
    /// The number of steps from the start net.
    pub depth: u32,
    /// An element of a type that the parser does not read appears to touch
    /// `to_net` in `to_scope`.
    pub connectivity_incomplete: bool,
}

/// The answer of [`crate::Netlist::trace_net`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetTrace {
    pub scope: String,
    pub net: String,
    /// The steps, breadth-first: nearest first.
    pub hops: Paged<Hop>,
    /// The trace stopped at its step limit before `depth` (see the
    /// caveats); `hops.total` counts the steps found.
    pub truncated: bool,
    /// An element of a type that the parser does not read appears to touch
    /// a net of the trace (the start net too).
    pub connectivity_incomplete: bool,
    /// Those elements, as instance paths (at most [`MAX_LIMIT`]).
    pub incomplete_due_to: Vec<String>,
    /// What makes the answer less certain.
    pub caveats: Vec<String>,
}

/// How a step of a [`Resolved::chain`] defines its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum StepKind {
    /// A `name=value` on the `X` line that makes the instance (evaluated
    /// in the scope of that instance; the overridden name itself is looked
    /// up in the parent's scope).
    InstanceOverride,
    /// A `.param` in the body of the instance's definition.
    LocalParam,
    /// A default on the `.subckt` line of the instance's definition.
    SubcktDefault,
    /// A top-level `.param`.
    GlobalParam,
    /// A call of a `.param` function (`.param f(a,b)=…`).
    ParamFunction,
    /// A parameter of the element line itself (the R value, `w=`, an `X`
    /// override when the element is an `X` line, a source's `dc`, …).
    ElementParam,
    /// A `key=value` of a `.model` card, evaluated in the scope of the
    /// definition that holds the card (see [`crate::Netlist::resolve_model`]).
    ModelParam,
    /// A value that HSPICE gives with no line in the deck for it, or from a
    /// command: `temper` (the first `.temp` value, else 25), the default AC
    /// phase 0, the DC value 0 of a source with none.
    Builtin,
}

/// One definition that took part in a value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChainStep {
    pub kind: StepKind,
    /// The instance path (lowercase) of the scope that holds the
    /// definition: `""` for the top level; for an `X`-line override the
    /// path of the instance that the line makes; for an element parameter
    /// the element's path; with `top=`, the definition (and each one it is
    /// written inside) in parentheses: `(outer>deep)`.
    pub scope: String,
    /// The name that the step defines (the function name for a function).
    pub name: String,
    /// The value as written, on one line (a function: `f(a,b) = body`).
    pub text: String,
    /// The value of this step; `None` when it has none.
    pub value: Option<f64>,
    /// The line of the definition; `None` for a [`StepKind::Builtin`]
    /// value with no line in the deck.
    pub loc: Option<Loc>,
}

/// The value of one parameter, or why it has none.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Resolved {
    /// The parameter name, in its first spelling in the deck.
    pub name: String,
    /// The value. `None` exactly when `unresolved` is set: no guessed
    /// number is ever given.
    pub value: Option<f64>,
    /// Why there is no value (an undefined name, a cycle, a form HSPICE
    /// rejects, a value that comes from a model card, …).
    pub unresolved: Option<String>,
    /// The definitions that gave the value, depth first: the definition of
    /// the name itself, then the definitions of the names its text uses,
    /// each once (at most [`MAX_LIMIT`] steps). The winner of a name comes
    /// before the names it uses, so the last step is the deepest
    /// definition that was used.
    pub chain: Vec<ChainStep>,
    /// What makes the value less certain (a statistical function's nominal
    /// value, `temper`, an `.alter` change, TC1/TC2 at another
    /// temperature, a sweep, a choice HSPICE was not probed for, …). At
    /// most 50: then the last one counts the caveats that are not listed.
    pub caveats: Vec<String>,
}

/// The answer of [`crate::Netlist::resolve_element`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ElementParams {
    /// The instance path, lowercase.
    pub path: String,
    /// `resistor`, `mosfet`, `instance`, …, or the letter of a type that
    /// the parser does not read.
    pub kind: String,
    /// The model name. For R, C and L it follows the reading of the
    /// tokens after the nodes (a word is a model only when a `.model` card
    /// of that name is visible, golden `res_model`).
    pub model: Option<String>,
    /// The parameters: `r`/`c`/`l` for the R/C/L value; `dc` (first), the
    /// function, `ac` and `acphase` of a source; then every other
    /// parameter of the line in order, each once (the last value of a name
    /// wins). At most [`MAX_LIMIT`]; see `params_total`.
    pub params: Vec<Resolved>,
    pub params_total: usize,
    /// What makes the whole answer less certain (the path, the element
    /// type, an `.alter` change of the element, a duplicate definition).
    /// At most 50, as in [`Resolved::caveats`].
    pub caveats: Vec<String>,
}

/// The `.model` card of an element, in [`ModelParams::card`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelCard {
    /// The name of the card, in its first spelling (`nch.1` for a bin).
    pub name: String,
    /// The model type, as the card writes it (`nmos`, `r`, …).
    #[serde(rename = "type")]
    pub r#type: String,
    /// The qualified name of the definition that holds the card
    /// (`(top level)` for the top level).
    pub def: String,
    /// The instance path (lowercase) of the scope where the parameters of
    /// the card are evaluated: the instance of `def` on the element's path
    /// (`""` for the top level; with `top=`, a definition in parentheses, as
    /// in [`ChainStep::scope`]).
    pub scope: String,
    /// The line of the card.
    pub loc: Loc,
    /// The number of `key=value` items on the card.
    pub params_total: usize,
}

/// One range of a bin that holds a MOSFET, in [`BinChoice::ranges`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BinRange {
    /// `l` (the length), `w` (the width per finger, W/NF, unless `WNFLAG=0`
    /// on the line, else `.option wnflag=0`), `nfin` (level 72) or
    /// `wsheet` (level 72).
    pub name: String,
    /// The size of the MOSFET that the range holds.
    pub value: f64,
    /// The limits of the bin (`lmin` and `lmax`, …), after `.option
    /// scalm` (not the fin counts): `min <= value < max`, where a value at
    /// most 2e-15 (relative) below a limit counts as on it.
    pub min: f64,
    pub max: f64,
}

/// How the card of a MOSFET was chosen, in [`ModelParams::selection`]: as
/// the model selector of `netlist-db` chooses it (`AST::mosfet_models`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BinChoice {
    /// `exact`: a card with the model name of the line; `bin`: the first
    /// bin `name.N` (in netlist order) whose ranges hold the MOSFET. Each
    /// level of the instance path is tried, from the MOSFET's own up to the
    /// top level.
    pub by: String,
    /// The length and width of the MOSFET (`.option defl`/`defw` when the
    /// line has none), times `.option scale`; `None` when one has no value
    /// (see [`ModelParams::unresolved`]).
    pub l: Option<f64>,
    pub w: Option<f64>,
    /// The number of fingers (`NF`, 1 when the line has none).
    pub nf: Option<f64>,
    /// The ranges of the bin, for `by` = `bin`.
    pub ranges: Vec<BinRange>,
    /// The bins of the name that do not hold the MOSFET: at the levels below
    /// the card's, and before it at its level.
    pub bins_passed: usize,
}

/// The answer of [`crate::Netlist::resolve_model`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelParams {
    /// The instance path of the element, lowercase.
    pub path: String,
    /// `mosfet`, `resistor`, `diode`, ….
    pub kind: String,
    /// The model name on the element line, as written.
    pub model: String,
    /// The card that the element uses on this path. `None` exactly when
    /// `unresolved` is set.
    pub card: Option<ModelCard>,
    /// Why no card was found: no card of the name on the instance path, no
    /// bin holds the MOSFET, a bin without its limits, a size or a limit
    /// with no value.
    pub unresolved: Option<String>,
    /// How the card of a MOSFET was chosen (also when none was: the size).
    pub selection: Option<BinChoice>,
    /// The `key=value` items of the card, in card order, each as written
    /// and evaluated where the card is defined, with its chain; or, for the
    /// names asked for, the items of those names (a name the card does not
    /// set has no value). A page.
    pub params: Paged<Resolved>,
    /// What makes the whole answer less certain (the scope rule of a card,
    /// the path, `.alter` changes, `.option scalm`, …). At most 50, as in
    /// [`Resolved::caveats`].
    pub caveats: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{Page, QueryError, page};

    #[test]
    fn pages() {
        let p = |limit: usize, cursor: Option<&str>| {
            page(
                0..7,
                &Page {
                    limit,
                    cursor: cursor.map(str::to_owned),
                },
            )
        };
        let first = p(3, None).unwrap();
        assert_eq!(first.items, [0, 1, 2]);
        assert_eq!(first.total, 7);
        assert_eq!(first.next_cursor.as_deref(), Some("3"));
        let last = p(3, Some("6")).unwrap();
        assert_eq!(last.items, [6]);
        assert_eq!(last.next_cursor, None);
        // past the end: no items, no cursor
        let past = p(3, Some("99")).unwrap();
        assert!(past.items.is_empty());
        assert_eq!(past.next_cursor, None);
        // limit 0: the total only
        let none = p(0, None).unwrap();
        assert!(none.items.is_empty());
        assert_eq!((none.total, none.next_cursor), (7, None));
        assert!(matches!(p(3, Some("-1")), Err(QueryError::BadArgument(_))));
    }
}
