//! `X`-line binding along instance paths, the counts of the expanded
//! hierarchy (without expanding it), and the subckt queries.
//!
//! HSPICE finds the definition of an `X` line along the instance path
//! (golden `nested_def`, `nested_def_dynamic`, `nested_def_reach`): for an
//! `X` line in the definition of instance P, it looks in the nested
//! definitions of P's definition, then in those of the definition of P's
//! parent instance, and so on up the path, then at the top level. So one
//! `X` line can bind to different definitions on different paths.
//!
//! [`analyze`] counts over a graph whose nodes are (definition, context).
//! The context is the list of the definitions above on the instance path
//! that are lookup scopes (they nest a definition whose name some `X` line
//! calls), nearest first, each once: a farther occurrence of a definition
//! is shadowed by the nearer one, so dropping it changes no lookup. A
//! definition below which no `X` line can call a nested name (by name, over
//! every definition a name can bind to) binds the same way in every
//! context, so its context is empty: it has one node. On the instance
//! paths from the root, only the scopes that such a path can reach (by
//! name) count for this; the imagined contexts below count them all. With
//! no lookup scope anywhere, every context is empty and the graph is the
//! plain definition graph, so the cost is linear in the number of
//! definitions and `X` lines. Nodes are memoized and instances are never
//! expanded: `copies(root) = 1`, and a node gets the copies of each node
//! that calls it, once per `X` line.
//!
//! A bind looks for the nearest scope of the context that nests the name:
//! it reads the context, or, in a long context, the place in the context of
//! each scope that nests the name, whichever list is shorter. The lookups
//! in long contexts count toward the budget of the analysis, so its cost
//! stays linear in that budget.
//!
//! HSPICE rejects a deck where one definition is instantiated inside an
//! instance of itself, also when the inner instance binds its `X` lines to
//! other definitions so that the expansion would end (golden
//! `hier_reentry`, `hier_reentry_unscoped`: "subcircuit aa is defined
//! recursively"). Such a path can visit two nodes of one definition, so
//! besides the cycles of nodes, [`analyze`] looks for a node that reaches
//! another node of its definition.
//!
//! The nodes that the root reaches come first. Then each definition that
//! no instance path from the root reaches and whose name no `X` line calls
//! is checked as if instantiated at the top level (a nested one: inside an
//! instance of each definition it is written inside), and the definitions
//! below it in those imagined contexts too. Last, each definition that no
//! check has reached yet (in a call cycle, or called only by a name that
//! binds elsewhere) is checked in the same way, in document order
//! (golden/NOTES.md, "Choices not probed"). The imagined nodes have 0
//! copies. The answers about a definition that the root reaches use its
//! reached nodes only.

use crate::{
    DefLookup, Netlist,
    api::{
        self, ChildCall, DeviceGroup, ElementBrief, InstanceRef, Level, MAX_LIMIT, NameText, Page,
        Paged, QueryError, SubcktInfo, SubcktRow, SubcktSort, SubcktText,
    },
    intern::Sym,
    model::{Def, DefId, Diag, ElemKind, Element, LocIdx, ParamDef, ROOT, Range32, Root},
};
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{BTreeMap, HashMap, HashSet},
};

pub(crate) use rustc_hash::{FxHashMap as FxMap, FxHashSet as FxSet};

mod diags;
use diags::{
    Reach, Va, builtin_model_type, cycle_diag, dup_elem_diag, m_diag, port_diag, unresolved_diag,
};

mod queries;
pub(crate) use queries::suggest;

mod build;
use build::analyze_with;

/// No node: an `X` line with no visible definition, or the end of a list.
const NONE: u32 = u32::MAX;

/// The most call cycles reported, each once.
const MAX_CYCLES: usize = 20;

/// The most steps read from the search stack to report cycles (a cycle
/// found again through another `X` line costs its length too).
const MAX_CYCLE_STEPS: usize = 1_000_000;

/// A context longer than this has a map of the places of its definitions,
/// and the lookups of its binds count toward the budget.
const SHORT_CTX: usize = 8;

/// The number of instances in the expanded hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Copies {
    Count(u64),
    /// A call cycle is at or above it: a definition is instantiated inside
    /// an instance of itself, and HSPICE rejects the deck.
    Cycle,
    /// More than `u64::MAX`.
    Overflow,
    /// A bare netlist: no top-level instances, no `top`.
    NoRoot,
    /// A duplicate definition, or one inside it: not used.
    Inactive,
    /// The analysis stopped (too many lookup contexts).
    TooLarge,
}

impl Copies {
    pub(crate) fn count(self) -> Option<u64> {
        match self {
            Copies::Count(n) => Some(n),
            _ => None,
        }
    }

    /// The copies of two sets of instances together.
    fn add(self, other: Copies) -> Copies {
        use Copies::*;
        match (self, other) {
            (Count(a), Count(b)) => a.checked_add(b).map_or(Overflow, Count),
            (Cycle, _) | (_, Cycle) => Cycle,
            (Overflow, _) | (_, Overflow) => Overflow,
            (NoRoot, _) | (_, NoRoot) => NoRoot,
            (TooLarge, _) | (_, TooLarge) => TooLarge,
            (Inactive, x) | (x, Inactive) => x,
        }
    }
}

/// A (definition, context) node.
#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub(crate) def: DefId,
    /// Its edges in [`Hier::edges`]: one per first-run `X` line of `def`,
    /// in line order (so the edges of two nodes of one definition align).
    pub(crate) edges: Range32,
    /// The next node of the same definition; [`NONE`] at the end.
    next: u32,
    pub(crate) copies: Copies,
}

/// One `X` line of a node.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Edge {
    /// The node it instantiates; [`NONE`]: no visible definition.
    pub(crate) target: u32,
    /// The `X` line, in the elements of the node's definition.
    pub(crate) elem: u32,
}

/// The hierarchy facts of one definition.
#[derive(Debug, Clone)]
pub(crate) struct DefHier {
    /// Its first node; [`NONE`]: not analysed (an inactive definition).
    first_node: u32,
    /// The number of `X` lines that bind to it on some instance path from
    /// the root, or in a definition that the root does not reach, as if
    /// that definition were instantiated at the top level.
    pub(crate) x_lines: u32,
    /// The number of its first-run element lines that are not `X` lines.
    pub(crate) devices: u32,
    /// Σ over its nodes.
    pub(crate) copies: Copies,
    /// It nests a definition whose name some `X` line calls: it is a
    /// lookup scope.
    pub(crate) scoped: bool,
    /// An `X` line in it or below it (by name, over every definition a
    /// name can bind to) can call a name that a lookup scope nests. The
    /// other definitions bind every `X` line at the top level, in every
    /// context.
    pub(crate) hot: bool,
}

/// The hierarchy analysis, computed once at load.
#[derive(Debug, Default)]
pub(crate) struct Hier {
    pub(crate) nodes: Vec<Node>,
    pub(crate) edges: Vec<Edge>,
    /// The nodes `0..n_reached` are on instance paths from the root; the
    /// others are imagined contexts of definitions that the root does not
    /// reach (0 copies).
    n_reached: u32,
    /// By `DefId`.
    pub(crate) defs: Vec<DefHier>,
    pub(crate) expanded_devices: Option<BTreeMap<String, u64>>,
    /// The parasitic R and C of included DSPF files, expanded like
    /// `expanded_devices`; `None` when there are none.
    pub(crate) expanded_parasitics: Option<BTreeMap<String, u64>>,
    pub(crate) expanded_instances: Option<u64>,
    pub(crate) expanded_note: Option<String>,
    /// For a definition with a duplicate: the line of its first duplicate.
    dup_at: HashMap<DefId, LocIdx>,
    /// The (definition, name) of the names of more than one first-run
    /// element line of an active definition (diagnostic
    /// `duplicate_element`).
    dup_elems: HashSet<(DefId, Sym)>,
    /// The analysis stopped: too many lookup contexts.
    pub(crate) too_large: bool,
    /// The lookups of the binds in long contexts (see [`Build::bind`]).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) bind_steps: usize,
}

impl Hier {
    /// The nodes of definition `def`, with their indices, in index order.
    fn def_nodes(&self, def: DefId) -> impl Iterator<Item = (u32, &Node)> + '_ {
        let mut at = self.defs.get(def.idx()).map_or(NONE, |d| d.first_node);
        std::iter::from_fn(move || {
            let node = self.nodes.get(at as usize)?;
            let id = at;
            at = node.next;
            Some((id, node))
        })
    }

    /// The root reaches definition `def` (on some instance path).
    fn reached(&self, def: DefId) -> bool {
        self.defs
            .get(def.idx())
            .is_some_and(|d| d.first_node < self.n_reached)
    }

    /// The nodes whose bindings the answers use for definition `def`: its
    /// reached nodes when the root reaches it, else all its nodes (as if
    /// instantiated at the top level).
    fn binding_nodes(&self, def: DefId) -> impl Iterator<Item = (u32, &Node)> + '_ {
        let reached = self.reached(def);
        let n_reached = self.n_reached;
        self.def_nodes(def)
            .take_while(move |&(id, _)| !reached || id < n_reached)
    }

    /// Node `id` is used for the bindings of its definition (see
    /// [`Hier::binding_nodes`]).
    fn binding_node(&self, id: u32) -> bool {
        id < self.n_reached || !self.reached(self.nodes[id as usize].def)
    }

    /// More than one first-run element line of definition `def` is named
    /// `name` (diagnostic `duplicate_element`).
    pub(crate) fn dup_element(&self, def: DefId, name: Sym) -> bool {
        self.dup_elems.contains(&(def, name))
    }
}

/// Analyses the hierarchy of `net`: the bindings in every context, the
/// copies, the expanded totals, and the diagnostics (call cycles, `X`
/// lines with no visible definition).
pub(crate) fn analyze(net: &Netlist) -> (Hier, Vec<Diag>) {
    analyze_with(net, None)
}

#[cfg(test)]
mod tests;
