//! Query engine over HSPICE netlists for LLM tools.
//!
//! [`Netlist::load`] parses a deck once (with `netlist-db`) and lowers the
//! parser's builders into an owned, interned model. The queries read that
//! model; they never expand the hierarchy.
//!
//! Every query returns `Result<_, QueryError>`. The lists of many items
//! (subckt rows, element lines, instances, diagnostics) are [`Paged`].
//! Inside one answer, every other list (ports, pins, children, parameters,
//! defaults, functions, overrides) holds at most [`MAX_LIMIT`] items, and
//! the model names of a device type at most 50, each with its total.
//! Names in answers keep the spelling of their first appearance in the
//! deck; instance paths are lowercase.

mod api;
mod dspf;
mod hier;
mod intern;
mod lower;
mod model;
mod nets;
mod params;
mod paths;
mod rejects;

pub use api::{
    ChainStep, ChildCall, Conn, DEFAULT_LIMIT, DefParasitics, DeviceGroup, Diagnostic, DspfFile,
    ElementBrief, ElementInfo, ElementParams, Hop, InstanceRef, Level, LoadError, Loc, MAX_LIMIT,
    NameText, NetInfo, NetTrace, NetView, Page, Paged, Parasitics, PinNet, QueryError, Resolved,
    StepKind, SubcktInfo, SubcktRow, SubcktSort, SubcktText, Summary, TraceDir,
};
pub use netlist_db::{LibOverride, TitleLine};

use intern::{Interner, Sym};
use model::{
    Def, DefId, Diag, LocIdx, ParamDef, Parhier, ROOT, ROOT_DISPLAY, Root, SourceFile, TextRef,
};
use std::{collections::HashMap, path::PathBuf};

/// What [`Netlist::load`] reads.
#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// The top file of the deck.
    pub path: PathBuf,
    /// The definition to use as the root of the instance paths, for a bare
    /// netlist (definitions only). A plain name is the active top-level
    /// definition, else the one active nested definition with that name;
    /// `outer>cell` names a nested definition.
    pub top: Option<String>,
    /// `.lib` section overrides (another corner).
    pub lib_override: Vec<LibOverride>,
    /// How line 1 of the top file is read.
    pub title_line: TitleLine,
}

impl LoadOptions {
    /// The options for the deck `path`: no `top`, no override, and
    /// `TitleLine::Auto`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            top: None,
            lib_override: Vec::new(),
            title_line: TitleLine::Auto,
        }
    }
}

/// A loaded netlist. It owns its data (`Send + Sync`), so one load serves
/// many queries.
#[derive(Debug)]
pub struct Netlist {
    /// The loaded files, by the parser's `ParsedId`.
    pub(crate) files: Vec<SourceFile>,
    /// The number of distinct files in `files` (by canonical path).
    pub(crate) distinct_files: usize,
    pub(crate) syms: Interner,
    /// `defs[0]` is the top level ([`ROOT`]).
    pub(crate) defs: Vec<Def>,
    /// The active first-run definition of each (scope, name): the scope is
    /// the enclosing definition, [`ROOT`] for the top level. A duplicate
    /// is not here; the first definition is.
    pub(crate) def_index: HashMap<(DefId, Sym), DefId>,
    pub(crate) root: Root,
    pub(crate) parhier: Parhier,
    /// Every `.option` item, in document order.
    pub(crate) options: Vec<ParamDef>,
    /// The last `.option tnom` with a number at the top level of the first
    /// run. The model selector reads TNOM so (`AST::top_level`).
    pub(crate) tnom: Option<ParamDef>,
    /// The `.global` node names.
    pub(crate) globals: Vec<Sym>,
    /// The first value of each `.temp` line.
    pub(crate) temps: Vec<ParamDef>,
    /// The line of `.alter` block `n` (from 1) is `alters[n - 1]`.
    pub(crate) alters: Vec<LocIdx>,
    /// The load diagnostics in document order, then those of the
    /// hierarchy analysis.
    pub(crate) diags: Vec<Diag>,
    /// The hierarchy analysis (bindings, copies, expanded totals).
    pub(crate) hier: hier::Hier,
    /// The first run has a `.hdl` line (a Verilog-A file, which the loader
    /// does not read): an `X` line can call one of its modules.
    pub(crate) hdl: bool,
    /// The names that the analyses of the first run sweep (`.dc`, a
    /// `sweep` of `.tran`/`.ac`, `.data` blocks they name), sorted by name
    /// (stable: in line order for one name).
    pub(crate) swept: Vec<model::Swept>,
    /// The name index of each definition, built on first use (paths).
    pub(crate) elem_names: std::sync::OnceLock<Vec<std::sync::OnceLock<paths::ElemNames>>>,
    /// The net index of each definition, built on first use (connectivity).
    pub(crate) net_index: std::sync::OnceLock<Vec<std::sync::OnceLock<nets::NetIndex>>>,
    /// An `X` line name holds a `.`: two instances can get one full name
    /// (see [`crate::paths`]). Built on first use.
    pub(crate) dotted_instances: std::sync::OnceLock<bool>,
    /// The DSPF files: included ones, then `.option sim_dspf` ones.
    pub(crate) dspf: Vec<dspf::Dspf>,
    /// The element lines (first run) that a load error skipped: their
    /// definition and name, sorted by definition. Their pins are unknown
    /// (see `crate::nets`).
    pub(crate) skipped: Vec<(model::DefId, String)>,
    /// The deck has a `.prot` region, which the loader does not read: a
    /// name can be defined there.
    pub(crate) protected: bool,
}

impl Netlist {
    /// Parses the deck `opts.path` and the files it includes, and builds the
    /// model. Statement errors become diagnostics; only an unreadable top
    /// file or a bad `opts.top` fails.
    pub fn load(opts: &LoadOptions) -> Result<Netlist, LoadError> {
        lower::load(opts)
    }

    /// An overview: files, definitions, root, `parhier`, diagnostics.
    pub fn summary(&self) -> Summary {
        let root = &self.defs[ROOT.idx()];
        let top_level_instances = root
            .elements
            .iter()
            .filter(|e| e.alter.is_none() && e.kind == model::ElemKind::Instance)
            .count();
        let alter_definitions = self.defs[1..].iter().filter(|d| d.alter.is_some()).count();
        Summary {
            files: self.distinct_files,
            definitions: self.defs.len() - 1 - alter_definitions,
            alter_definitions,
            root: match self.root {
                Root::TopLevel => "top-level".to_owned(),
                Root::Subckt(id) => format!("subckt:{}", self.def_display(id)),
                Root::None => "none".to_owned(),
            },
            top_level_instances,
            expanded_devices: self.hier.expanded_devices.clone(),
            expanded_instances: self.hier.expanded_instances,
            expanded_note: self.hier.expanded_note.clone(),
            parhier: self.parhier.as_str().to_owned(),
            diagnostics_count: self.diags.len(),
            errors: self
                .diags
                .iter()
                .filter(|d| d.level == Level::Error)
                .count(),
            alter_runs: self.alters.len(),
            expanded_parasitics: self.hier.expanded_parasitics.clone(),
            dspf: self.dspf_files(),
        }
    }

    /// The diagnostics of level `level` (all levels for `None`): those of
    /// the load in document order, then those of the hierarchy analysis:
    /// call cycles, then `X` lines with no visible definition, then `X`
    /// lines whose node count is not the port count of their subckt, then
    /// lines with `m` <= 0, then lines with the name of an earlier line of
    /// their definition, each by definition; then those of the
    /// `.option sim_dspf` files. Fails only for a bad `page.cursor`.
    pub fn diagnostics(
        &self,
        level: Option<Level>,
        page: &Page,
    ) -> Result<Paged<Diagnostic>, QueryError> {
        let picked: Vec<&Diag> = self
            .diags
            .iter()
            .filter(|d| level.is_none_or(|l| d.level == l))
            .collect();
        Ok(api::page(picked.into_iter(), page)?.map(|d| Diagnostic {
            level: d.level,
            code: d.code,
            message: d.message.clone(),
            loc: d.loc.map(|l| self.loc(l)),
        }))
    }

    /// The public form of `loc`.
    pub(crate) fn loc(&self, loc: LocIdx) -> Loc {
        Loc {
            file: self.files[loc.file as usize].path.clone(),
            line: loc.line,
        }
    }

    /// The text `t`.
    pub(crate) fn text(&self, t: TextRef) -> &str {
        &self.files[t.file as usize].text[t.start as usize..t.end as usize]
    }

    /// The name of definition `id` in answers: its name, after the names of
    /// the definitions it is written inside, joined by `>` (`outer>cell`).
    /// The root is `(top level)`.
    pub(crate) fn def_display(&self, id: DefId) -> String {
        if id == ROOT {
            return ROOT_DISPLAY.to_owned();
        }
        let mut chain = vec![id];
        while let Some(parent) = self.defs[chain[chain.len() - 1].idx()].parent {
            chain.push(parent);
        }
        let names: Vec<&str> = chain
            .iter()
            .rev()
            .map(|d| self.syms.name(self.defs[d.idx()].name))
            .collect();
        names.join(">")
    }

    /// True when `id` and every definition it is written inside are active:
    /// in the first run and not a duplicate.
    pub(crate) fn def_active(&self, id: DefId) -> bool {
        let mut at = Some(id);
        while let Some(d) = at {
            let def = &self.defs[d.idx()];
            if def.alter.is_some() || def.dup_of.is_some() {
                return false;
            }
            at = def.parent;
        }
        true
    }

    /// The definition that `name` (any letter case) names: `a>b` is the
    /// nested `b` of the active top-level `a`; a plain name is the active
    /// top-level definition, else the one active nested definition (no
    /// inactive definition around it) with that name. `LoadOptions::top`
    /// and the subckt-name queries use this rule. A definition name can hold
    /// `>` (`bus<1>`), so the name as written is looked up first.
    pub(crate) fn find_def(&self, name: &str) -> DefLookup {
        let name = name.trim();
        let plain = self.find_plain_def(name);
        if plain != DefLookup::NotFound || !name.contains('>') {
            return plain;
        }
        self.find_qualified(ROOT, name)
            .map_or(DefLookup::NotFound, DefLookup::Found)
    }

    /// The active definition that the qualified name `rest` names below
    /// `scope`: `rest` itself, else a definition named by the text before
    /// one of its `>`, then the rest below that one.
    fn find_qualified(&self, scope: DefId, rest: &str) -> Option<DefId> {
        let here = |n: &str| {
            self.syms
                .get(n.trim())
                .and_then(|sym| self.def_index.get(&(scope, sym)))
                .copied()
        };
        here(rest).or_else(|| {
            rest.match_indices('>')
                .find_map(|(p, _)| self.find_qualified(here(&rest[..p])?, &rest[p + 1..]))
        })
    }

    /// [`Netlist::find_def`] for a name that is not qualified.
    fn find_plain_def(&self, name: &str) -> DefLookup {
        let Some(sym) = self.syms.get(name) else {
            return DefLookup::NotFound;
        };
        if let Some(&id) = self.def_index.get(&(ROOT, sym)) {
            return DefLookup::Found(id);
        }
        let nested: Vec<DefId> = (1..self.defs.len())
            .map(|i| DefId(i as u32))
            .filter(|&id| {
                let d = &self.defs[id.idx()];
                d.name == sym && d.parent.is_some() && self.def_active(id)
            })
            .collect();
        match nested.as_slice() {
            [] => DefLookup::NotFound,
            [one] => DefLookup::Found(*one),
            many => {
                let mut names: Vec<String> = many.iter().map(|&id| self.def_display(id)).collect();
                // two active definitions have different qualified names;
                // keep each name once anyway
                let mut seen = std::collections::HashSet::new();
                names.retain(|n| seen.insert(n.clone()));
                DefLookup::Ambiguous(names)
            }
        }
    }
}

/// What [`Netlist::find_def`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DefLookup {
    Found(DefId),
    NotFound,
    /// Several active nested definitions have the name: their qualified
    /// names, each once, in document order.
    Ambiguous(Vec<String>),
}
