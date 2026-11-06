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
    hash::{BuildHasherDefault, Hasher},
};

/// An iterator whose length is known, for [`api::page`].
struct Known<I> {
    it: I,
    left: usize,
}

impl<I: Iterator> Iterator for Known<I> {
    type Item = I::Item;

    fn next(&mut self) -> Option<I::Item> {
        let item = self.it.next()?;
        self.left = self.left.saturating_sub(1);
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.left, Some(self.left))
    }
}

impl<I: Iterator> ExactSizeIterator for Known<I> {}

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

/// A fast hasher for small integer keys (FxHash).
#[derive(Default)]
pub(crate) struct FxHasher(u64);

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }

    #[inline]
    fn write_u32(&mut self, x: u32) {
        self.write_u64(x as u64);
    }

    #[inline]
    fn write_u64(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    #[inline]
    fn write_usize(&mut self, x: usize) {
        self.write_u64(x as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub(crate) type FxSet<K> = HashSet<K, BuildHasherDefault<FxHasher>>;

/// Lists of definitions by name, in compressed rows.
#[derive(Default)]
struct ByName {
    /// The row of each name.
    row: FxMap<Sym, u32>,
    /// Row `r` is `defs[start[r]..start[r + 1]]`.
    start: Vec<u32>,
    defs: Vec<DefId>,
}

impl ByName {
    /// The lists of the (name, definition) pairs of `pairs` (read twice).
    fn new<I: Iterator<Item = (Sym, DefId)>>(pairs: impl Fn() -> I) -> ByName {
        let mut row: FxMap<Sym, u32> = FxMap::default();
        let mut start: Vec<u32> = vec![0];
        for (name, _) in pairs() {
            let r = *row.entry(name).or_insert_with(|| {
                start.push(0);
                start.len() as u32 - 2
            });
            start[r as usize + 1] += 1;
        }
        for r in 1..start.len() {
            start[r] += start[r - 1];
        }
        let mut fill = start.clone();
        let mut defs = vec![DefId(0); start[start.len() - 1] as usize];
        for (name, def) in pairs() {
            let at = &mut fill[row[&name] as usize];
            defs[*at as usize] = def;
            *at += 1;
        }
        ByName { row, start, defs }
    }

    /// The row of `name` (empty when it has none).
    fn get(&self, name: Sym) -> &[DefId] {
        match self.row.get(&name) {
            Some(&r) => {
                &self.defs[self.start[r as usize] as usize..self.start[r as usize + 1] as usize]
            }
            None => &[],
        }
    }

    fn contains(&self, name: Sym) -> bool {
        self.row.contains_key(&name)
    }
}

/// One reported call cycle: the (definition, `X` line) steps, from a node
/// of the first definition to a node of the same definition.
struct CycleReport {
    steps: Vec<(DefId, u32)>,
    /// The first node: the cycle is on an instance path from the root when
    /// the root reaches it.
    first: u32,
}

/// The state of one analysis.
struct Build<'n> {
    net: &'n Netlist,
    /// By definition: its first-run `X` lines (element indices) are
    /// `xl[xl_start[d]..xl_start[d + 1]]`.
    xl_start: Vec<u32>,
    xl: Vec<u32>,
    scoped: Vec<bool>,
    /// By definition: an `X` line in it or below it (by name, over every
    /// definition a name can bind to) calls a name in `nested_in`. The
    /// others bind the same way in every context: their context is empty.
    hot: Vec<bool>,
    /// The lookup scopes that nest each name that some `X` line calls.
    nested_in: ByName,
    /// The binding of (lookup chain, name), for the names in `nested_in`.
    bind_cache: FxMap<(u32, Sym), Option<DefId>>,
    /// The contexts; `ctxs[0]` is empty.
    ctxs: Vec<Vec<DefId>>,
    ctx_ids: FxMap<Vec<DefId>, u32>,
    /// The place of each definition in a context longer than
    /// [`SHORT_CTX`].
    ctx_pos: FxMap<(u32, DefId), u32>,
    /// The lookups of the binds in contexts longer than [`SHORT_CTX`].
    bind_steps: usize,
    /// The nodes `0..n_reached` are on instance paths from the root.
    n_reached: u32,
    nodes: Vec<Node>,
    /// By node: its context.
    node_ctx: Vec<u32>,
    /// By definition: its node with the empty context.
    plain: Vec<u32>,
    ctx_nodes: FxMap<(DefId, u32), u32>,
    /// By definition: its first and its last node.
    first_node: Vec<u32>,
    last_node: Vec<u32>,
    edges: Vec<Edge>,
    /// By node: 0 new, 1 on the search stack, 2 done.
    state: Vec<u8>,
    /// By node: its place on the search stack (while `state` is 1).
    stack_pos: Vec<u32>,
    /// By node: the first node of a cycle (the target of an edge that
    /// closes a cycle of nodes), or a node that reaches another node of
    /// its definition.
    cyclic: Vec<bool>,
    /// The nodes in the order the search finished them.
    post: Vec<u32>,
    /// The reported cycles.
    cycles: Vec<CycleReport>,
    /// The definition sets of the reported cycles.
    cycle_sets: HashSet<Vec<DefId>>,
    /// More cycles than [`MAX_CYCLES`], or more steps than
    /// [`MAX_CYCLE_STEPS`].
    more_cycles: bool,
    /// The steps read from the search stack so far.
    cycle_steps: usize,
    /// The unresolved `X` lines: (node, element).
    unresolved: Vec<(u32, u32)>,
    /// By definition: checked in its own context, as if instantiated at
    /// the top level (or inside its text parents), though no instance path
    /// from the root reaches it.
    alone: Vec<bool>,
    /// The most nodes, edges, context entries and bind lookups (see
    /// `bind_steps`) before the analysis stops.
    budget: usize,
    /// The entries of the contexts and of their place maps.
    ctx_cells: usize,
    too_large: bool,
}

/// Analyses the hierarchy of `net`: the bindings in every context, the
/// copies, the expanded totals, and the diagnostics (call cycles, `X`
/// lines with no visible definition).
pub(crate) fn analyze(net: &Netlist) -> (Hier, Vec<Diag>) {
    analyze_with(net, None)
}

/// [`analyze`] with at most `budget` nodes, edges, context entries and
/// lookups in long contexts (default: 4 × (definitions + `X` lines) + 4M).
/// Over the budget, the analysis stops: only nested definitions used in
/// very many combinations along the instance paths get there.
fn analyze_with(net: &Netlist, budget: Option<usize>) -> (Hier, Vec<Diag>) {
    let n_defs = net.defs.len();
    // the load diagnostics only, so far
    let load_errors = net.diags.iter().filter(|d| d.level == Level::Error).count();
    let protected = net
        .diags
        .iter()
        .filter(|d| d.code == "protected_region")
        .count();
    // active: in the first run, and neither a duplicate nor inside one (a
    // definition comes after the one it is written inside)
    let mut active = vec![false; n_defs];
    for (i, d) in net.defs.iter().enumerate() {
        active[i] =
            d.alter.is_none() && d.dup_of.is_none() && d.parent.is_none_or(|p| active[p.idx()]);
    }
    let mut xl_start = Vec::with_capacity(n_defs + 1);
    let mut xl = Vec::new();
    // the devices of every first-run definition (an inactive one too, for
    // its row), the `X` lines of the active ones
    let mut devices = vec![0_u32; n_defs];
    let mut called: FxSet<Sym> = FxSet::default();
    for (i, d) in net.defs.iter().enumerate() {
        xl_start.push(xl.len() as u32);
        if d.alter.is_some() {
            continue;
        }
        for (j, e) in d.elements.iter().enumerate() {
            if e.alter.is_some() {
                continue;
            }
            if e.kind != ElemKind::Instance {
                devices[i] += u32::from(!e.parasitic);
            } else if active[i] {
                xl.push(j as u32);
                called.extend(e.callee);
            }
        }
    }
    xl_start.push(xl.len() as u32);
    // a definition is a lookup scope when it nests a definition whose name
    // some `X` line calls: the other nested definitions change no lookup
    let mut scoped = vec![false; n_defs];
    let nested_in = ByName::new(|| {
        net.def_index
            .keys()
            .filter(|&&(scope, name)| scope != ROOT && called.contains(&name))
            .map(|&(scope, name)| (name, scope))
    });
    for &scope in &nested_in.defs {
        scoped[scope.idx()] = true;
    }
    // the instance paths from the root have only the lookup scopes that
    // they can reach in their contexts: the analysis from the root uses
    // these (so a scope on no instance path makes no definition hot
    // there), and the imagined contexts use all
    let reach = reach_by_name(net, &xl_start, &xl, &active);
    let mut all_nested_in = None;
    let nested_in = if nested_in.defs.iter().all(|s| reach[s.idx()]) {
        nested_in
    } else {
        let on_paths = ByName::new(|| {
            nested_in
                .row
                .keys()
                .flat_map(|&name| nested_in.get(name).iter().map(move |&s| (name, s)))
                .filter(|&(_, s)| reach[s.idx()])
        });
        all_nested_in = Some(nested_in);
        on_paths
    };
    drop(reach);
    let hot = hot_defs(net, &xl_start, &xl, &nested_in);
    let mut b = Build {
        net,
        budget: budget.unwrap_or(4 * (n_defs + xl.len()) + 4_000_000),
        ctx_cells: 0,
        xl_start,
        xl,
        scoped,
        hot,
        nested_in,
        bind_cache: FxMap::default(),
        ctxs: vec![Vec::new()],
        ctx_ids: FxMap::default(),
        ctx_pos: FxMap::default(),
        bind_steps: 0,
        n_reached: 0,
        nodes: Vec::new(),
        node_ctx: Vec::new(),
        plain: vec![NONE; n_defs],
        ctx_nodes: FxMap::default(),
        first_node: vec![NONE; n_defs],
        last_node: vec![NONE; n_defs],
        edges: Vec::new(),
        state: Vec::new(),
        stack_pos: Vec::new(),
        cyclic: Vec::new(),
        post: Vec::new(),
        cycles: Vec::new(),
        cycle_sets: HashSet::new(),
        more_cycles: false,
        cycle_steps: 0,
        unresolved: Vec::new(),
        alone: vec![false; n_defs],
        too_large: false,
    };
    b.ctx_ids.insert(Vec::new(), 0);

    // the root (its nodes come first), then the definitions no path from
    // the root reaches (their `X` lines are checked, and their counts are
    // 0): first those whose name no `X` line calls, and the definitions
    // below them; then, in document order, those that no check reached
    let root_node = match net.root {
        Root::TopLevel => Some(b.node(ROOT, 0)),
        Root::Subckt(id) => {
            let ctx = b.text_ctx(id);
            Some(b.node(id, ctx))
        }
        Root::None => None,
    };
    if let Some(n) = root_node {
        b.search(n);
    }
    b.n_reached = b.nodes.len() as u32;
    if let Some(all) = all_nested_in {
        // the cached binds stay valid: a bind reads the scopes of its
        // chain only
        b.hot = hot_defs(net, &b.xl_start, &b.xl, &all);
        b.nested_in = all;
    }
    if let Root::Subckt(_) = net.root
        && b.xl_start[1] > 0
    {
        let n = b.node(ROOT, 0);
        b.alone[ROOT.idx()] = true;
        b.search(n);
    }
    for uncalled_only in [true, false] {
        for (i, &on) in active.iter().enumerate().skip(1) {
            let id = DefId(i as u32);
            if !on
                || b.first_node[i] != NONE
                || b.too_large
                || (uncalled_only && called.contains(&net.defs[i].name))
            {
                continue;
            }
            let ctx = b.text_ctx(id);
            let n = b.node(id, ctx);
            b.alone[i] = true;
            b.search(n);
        }
    }
    drop(called);
    b.reentries();
    b.finish(root_node, active, devices, (load_errors, protected))
}

impl Build<'_> {
    /// The context of definition `id` from the definitions it is written
    /// inside: as if it were instantiated inside an instance of its text
    /// parent, inside one of the parent's parent, ... (a top-level
    /// definition: the empty context).
    fn text_ctx(&mut self, id: DefId) -> u32 {
        let mut ctx = Vec::new();
        let mut at = self.net.defs[id.idx()].parent;
        while let Some(p) = at {
            if self.scoped[p.idx()] {
                ctx.push(p);
            }
            at = self.net.defs[p.idx()].parent;
        }
        self.ctx_id(ctx)
    }

    fn ctx_id(&mut self, ctx: Vec<DefId>) -> u32 {
        if ctx.is_empty() {
            return 0;
        }
        if let Some(&id) = self.ctx_ids.get(&ctx) {
            return id;
        }
        let id = self.ctxs.len() as u32;
        // a context is kept twice, a long one three times
        self.ctx_cells += 2 * ctx.len();
        if ctx.len() > SHORT_CTX {
            self.ctx_cells += ctx.len();
            for (k, &d) in ctx.iter().enumerate() {
                self.ctx_pos.insert((id, d), k as u32);
            }
        }
        self.ctxs.push(ctx.clone());
        self.ctx_ids.insert(ctx, id);
        id
    }

    /// The node (`def`, `ctx`), added when new. A definition that is not
    /// hot binds the same way in every context: its context is empty.
    fn node(&mut self, def: DefId, ctx: u32) -> u32 {
        let ctx = if self.hot[def.idx()] { ctx } else { 0 };
        let found = if ctx == 0 {
            self.plain[def.idx()]
        } else {
            self.ctx_nodes.get(&(def, ctx)).copied().unwrap_or(NONE)
        };
        if found != NONE {
            return found;
        }
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            def,
            edges: Range32 { start: 0, end: 0 },
            next: NONE,
            copies: Copies::Count(0),
        });
        self.node_ctx.push(ctx);
        self.state.push(0);
        self.stack_pos.push(0);
        self.cyclic.push(false);
        if ctx == 0 {
            self.plain[def.idx()] = id;
        } else {
            self.ctx_nodes.insert((def, ctx), id);
        }
        let last = self.last_node[def.idx()];
        if last == NONE {
            self.first_node[def.idx()] = id;
        } else {
            self.nodes[last as usize].next = id;
        }
        self.last_node[def.idx()] = id;
        id
    }

    /// The definition that an `X` line calling `callee` binds to with the
    /// lookup chain `chain` (a context id): the first nested definition of
    /// that name along the chain, else the top-level one. A name that no
    /// lookup scope nests needs one lookup; the others are cached by
    /// (chain, name). A long chain is read from its nearest scope, in turn
    /// with the place in the chain of each scope that nests the name, so a
    /// bind costs at most twice the shorter of the two reads; these lookups
    /// count toward the budget.
    fn bind(&mut self, callee: Sym, chain: u32) -> Option<DefId> {
        let net = self.net;
        let top = || net.def_index.get(&(ROOT, callee)).copied();
        if chain == 0 || !self.nested_in.contains(callee) {
            return top();
        }
        if let Some(&bound) = self.bind_cache.get(&(chain, callee)) {
            return bound;
        }
        let ctx = &self.ctxs[chain as usize];
        let scopes = self.nested_in.get(callee);
        let nearest = if ctx.len() <= SHORT_CTX {
            ctx.iter()
                .find_map(|&s| net.def_index.get(&(s, callee)).copied())
        } else {
            // the context from its nearest scope, and the scopes of the
            // name with their places in the context, a step of each in
            // turn, until one of the two lists ends
            let mut best: Option<(u32, DefId)> = None;
            let mut found = None;
            for k in 0..ctx.len().max(scopes.len()) {
                if let Some(&s) = ctx.get(k) {
                    self.bind_steps += 1;
                    if let Some(&d) = net.def_index.get(&(s, callee)) {
                        found = Some(d);
                        break;
                    }
                } else {
                    break;
                }
                if let Some(&s) = scopes.get(k) {
                    self.bind_steps += 1;
                    if let Some(&at) = self.ctx_pos.get(&(chain, s))
                        && best.is_none_or(|(p, _)| at < p)
                    {
                        best = Some((at, s));
                    }
                    if k + 1 == scopes.len() {
                        found = best.and_then(|(_, s)| net.def_index.get(&(s, callee)).copied());
                        break;
                    }
                }
            }
            found
        };
        let bound = nearest.or_else(top);
        self.bind_cache.insert((chain, callee), bound);
        bound
    }

    /// The nodes, edges, context entries and long bind lookups so far.
    fn used(&self) -> usize {
        self.nodes.len() + self.edges.len() + self.ctx_cells + self.bind_steps
    }

    /// Binds the `X` lines of node `n`: its edges.
    fn expand(&mut self, n: u32) {
        let def = self.nodes[n as usize].def;
        let ctx = self.node_ctx[n as usize];
        // the lookup chain of the `X` lines of `def`, which is also the
        // context of the instances they make: `def` (when it has nested
        // definitions), then the context of `n`, each definition once. A
        // definition that is not hot calls no nested name: the empty chain
        let chain_id = if self.scoped[def.idx()] && self.hot[def.idx()] {
            let mut chain = Vec::with_capacity(self.ctxs[ctx as usize].len() + 1);
            chain.push(def);
            chain.extend(self.ctxs[ctx as usize].iter().filter(|&&d| d != def));
            self.ctx_id(chain)
        } else {
            ctx
        };
        let start = self.edges.len();
        let net = self.net;
        let elements = &net.defs[def.idx()].elements;
        for k in self.xl_start[def.idx()]..self.xl_start[def.idx() + 1] {
            let j = self.xl[k as usize];
            let target = elements[j as usize]
                .callee
                .and_then(|callee| self.bind(callee, chain_id))
                .map_or(NONE, |t| self.node(t, chain_id));
            if target == NONE {
                self.unresolved.push((n, j));
            }
            self.edges.push(Edge { target, elem: j });
        }
        self.nodes[n as usize].edges = Range32 {
            start: start as u32,
            end: self.edges.len() as u32,
        };
        if self.used() > self.budget {
            self.too_large = true;
        }
    }

    /// A depth-first search from node `start` (no recursion: a hierarchy
    /// can be deep): binds the nodes it reaches, finds the cycles, and
    /// records the order in which it finishes the nodes.
    fn search(&mut self, start: u32) {
        if self.state[start as usize] != 0 || self.too_large {
            return;
        }
        // (node, its next edge)
        let mut stack: Vec<(u32, u32)> = Vec::new();
        self.enter(start, &mut stack);
        while let Some(&(n, at)) = stack.last() {
            if self.too_large {
                return;
            }
            if at < self.nodes[n as usize].edges.end {
                stack.last_mut().expect("not empty").1 = at + 1;
                let t = self.edges[at as usize].target;
                if t == NONE {
                    continue;
                }
                match self.state[t as usize] {
                    0 => self.enter(t, &mut stack),
                    1 => self.back_edge(t, &stack),
                    _ => {}
                }
            } else {
                self.state[n as usize] = 2;
                self.post.push(n);
                stack.pop();
            }
        }
    }

    fn enter(&mut self, n: u32, stack: &mut Vec<(u32, u32)>) {
        self.expand(n);
        self.state[n as usize] = 1;
        self.stack_pos[n as usize] = stack.len() as u32;
        stack.push((n, self.nodes[n as usize].edges.start));
    }

    /// The edge just taken from the top of `stack` goes to node `t`, which
    /// is on the stack: a cycle.
    fn back_edge(&mut self, t: u32, stack: &[(u32, u32)]) {
        self.cyclic[t as usize] = true;
        let segment = &stack[self.stack_pos[t as usize] as usize..];
        if !self.may_report(segment.len()) {
            return;
        }
        let steps: Vec<(DefId, u32)> = segment
            .iter()
            .map(|&(n, at)| (self.nodes[n as usize].def, self.edges[at as usize - 1].elem))
            .collect();
        self.report_cycle(steps, t);
    }

    /// Counts `steps` more steps read to report a cycle: false when no more
    /// cycle can be reported.
    fn may_report(&mut self, steps: usize) -> bool {
        self.cycle_steps += steps;
        if self.cycles.len() >= MAX_CYCLES || self.cycle_steps > MAX_CYCLE_STEPS {
            self.more_cycles = true;
            return false;
        }
        true
    }

    /// Reports the cycle `steps` from node `first`, unless a cycle through
    /// the same definitions is reported.
    fn report_cycle(&mut self, steps: Vec<(DefId, u32)>, first: u32) {
        let mut set: Vec<DefId> = steps.iter().map(|s| s.0).collect();
        set.sort_unstable();
        set.dedup();
        if self.cycle_sets.insert(set) {
            self.cycles.push(CycleReport { steps, first });
        }
    }

    /// Finds the nodes that reach another node of their definition: on an
    /// instance path through such a node, the definition is instantiated
    /// inside an instance of itself (golden `hier_reentry`). Marks them
    /// `cyclic` and reports a cycle for each.
    ///
    /// A path between two nodes of one definition maps to a closed walk in
    /// the definition graph (an edge `d -> e` for each edge of the nodes),
    /// so it stays inside one strongly connected component of that graph.
    /// A component whose definitions have one node each is a cycle of
    /// nodes, which the search reported. So only the components with a
    /// definition of several nodes are checked: for each of their nodes,
    /// in the order the search finished them, the set of the component's
    /// definitions it reaches, as a bit set. With no lookup scope, every
    /// definition has one node, and nothing more is done.
    fn reentries(&mut self) {
        if self.too_large {
            return;
        }
        let n_defs = self.net.defs.len();
        let mut def_nodes = vec![0_u32; n_defs];
        for node in &self.nodes {
            def_nodes[node.def.idx()] += 1;
        }
        if def_nodes.iter().all(|&c| c <= 1) {
            return;
        }
        // the definition graph, in compressed rows
        let mut start = vec![0_u32; n_defs + 1];
        for node in &self.nodes {
            let calls = self.edges[node.edges.range()]
                .iter()
                .filter(|e| e.target != NONE)
                .count() as u32;
            start[node.def.idx() + 1] += calls;
        }
        for d in 0..n_defs {
            start[d + 1] += start[d];
        }
        let mut adj = vec![0_u32; start[n_defs] as usize];
        let mut fill = start.clone();
        for node in &self.nodes {
            for e in &self.edges[node.edges.range()] {
                if e.target != NONE {
                    let at = &mut fill[node.def.idx()];
                    adj[*at as usize] = self.nodes[e.target as usize].def.0;
                    *at += 1;
                }
            }
        }
        let comp = components(&start, &adj);
        let n_comps = comp.iter().map(|&c| c as usize + 1).max().unwrap_or(0);
        // the components to check: a definition of several nodes, and more
        // than one definition or a definition that calls itself
        let mut size = vec![0_u32; n_comps];
        let mut multi = vec![false; n_comps];
        let mut self_call = vec![false; n_comps];
        for d in 0..n_defs {
            let c = comp[d] as usize;
            size[c] += 1;
            multi[c] |= def_nodes[d] > 1;
            self_call[c] |= adj[start[d] as usize..start[d + 1] as usize].contains(&(d as u32));
        }
        let check: Vec<bool> = (0..n_comps)
            .map(|c| multi[c] && (size[c] > 1 || self_call[c]))
            .collect();
        drop((start, adj, fill, def_nodes, size, multi, self_call));
        // the bit of each definition in its component, the words of a set
        let mut bit = vec![0_u32; n_defs];
        let mut bits_of = vec![0_u32; n_comps];
        for d in 0..n_defs {
            let c = comp[d] as usize;
            if check[c] {
                bit[d] = bits_of[c];
                bits_of[c] += 1;
            }
        }
        let words = |c: usize| bits_of[c].div_ceil(64) as usize;
        // the set of each node to check: `sets[slot[n]..slot[n] + words]`
        let mut slot = vec![NONE; self.nodes.len()];
        let mut total = 0_usize;
        for (n, node) in self.nodes.iter().enumerate() {
            let c = comp[node.def.idx()] as usize;
            if check[c] {
                slot[n] = total as u32;
                total += words(c);
            }
        }
        if total == 0 {
            return;
        }
        if self.used() + total > self.budget || total >= NONE as usize {
            self.too_large = true;
            return;
        }
        let mut sets = vec![0_u64; total];
        let mut post_pos = vec![0_u32; self.nodes.len()];
        for (k, &n) in self.post.iter().enumerate() {
            post_pos[n as usize] = k as u32;
        }
        let has = |sets: &[u64], n: usize, b: u32| {
            (sets[slot[n] as usize + (b / 64) as usize] >> (b % 64)) & 1 == 1
        };
        let mut found = Vec::new();
        for &n in &self.post {
            let n = n as usize;
            if slot[n] == NONE {
                continue;
            }
            let c = comp[self.nodes[n].def.idx()];
            let (at, w) = (slot[n] as usize, words(c as usize));
            for e in &self.edges[self.nodes[n].edges.range()] {
                let t = e.target as usize;
                if e.target == NONE || comp[self.nodes[t].def.idx()] != c {
                    continue;
                }
                let b = bit[self.nodes[t].def.idx()];
                sets[at + (b / 64) as usize] |= 1 << (b % 64);
                // a node the search finished later (a cycle of nodes) has
                // no set yet
                if t != n {
                    let from = slot[t] as usize;
                    for k in 0..w {
                        let v = sets[from + k];
                        sets[at + k] |= v;
                    }
                }
            }
            if has(&sets, n, bit[self.nodes[n].def.idx()]) {
                found.push(n as u32);
            }
        }
        for n in found {
            self.cyclic[n as usize] = true;
            if self.more_cycles {
                continue;
            }
            // a path to another node of the definition: each step to a
            // node the search finished earlier that reaches one
            let def = self.nodes[n as usize].def;
            let b = bit[def.idx()];
            let c = comp[def.idx()];
            let mut steps = Vec::new();
            let mut at = n as usize;
            loop {
                let edges = &self.edges[self.nodes[at].edges.range()];
                let in_comp = |e: &&Edge| {
                    e.target != NONE && comp[self.nodes[e.target as usize].def.idx()] == c
                };
                if let Some(e) = edges
                    .iter()
                    .filter(in_comp)
                    .find(|e| self.nodes[e.target as usize].def == def)
                {
                    steps.push((self.nodes[at].def, e.elem));
                    break;
                }
                let e = edges
                    .iter()
                    .filter(in_comp)
                    .find(|e| {
                        let t = e.target as usize;
                        post_pos[t] < post_pos[at] && has(&sets, t, b)
                    })
                    .expect("the set of a node comes from its edges");
                steps.push((self.nodes[at].def, e.elem));
                at = e.target as usize;
            }
            if self.may_report(steps.len()) {
                self.report_cycle(steps, n);
            }
        }
    }

    /// The copies, the per-definition facts, the totals and the
    /// diagnostics.
    fn finish(
        self,
        root_node: Option<u32>,
        active: Vec<bool>,
        devices: Vec<u32>,
        (load_errors, protected): (usize, usize),
    ) -> (Hier, Vec<Diag>) {
        let net = self.net;
        let mut hier = Hier {
            too_large: self.too_large,
            n_reached: self.n_reached,
            bind_steps: self.bind_steps,
            ..Hier::default()
        };
        for (i, d) in net.defs.iter().enumerate() {
            if let Some(first) = d.dup_of {
                hier.dup_at.entry(first).or_insert(net.defs[i].loc);
            }
        }
        let mut nodes = self.nodes;
        let mut diags = Vec::new();
        // the copies of each node, in an order where each node comes after
        // the nodes that call it (but for the edges that close a cycle)
        let mut count: Vec<Option<u64>> = vec![Some(0); nodes.len()];
        let mut infinite = vec![false; nodes.len()];
        if let Some(r) = root_node {
            count[r as usize] = Some(1);
        }
        if !self.too_large {
            for &n in self.post.iter().rev() {
                let n = n as usize;
                // a cycle that no path from the root reaches makes nothing
                if self.cyclic[n] && count[n] != Some(0) {
                    infinite[n] = true;
                }
                let (c, inf) = (count[n], infinite[n]);
                for e in &self.edges[nodes[n].edges.range()] {
                    if e.target != NONE {
                        let t = e.target as usize;
                        count[t] = match (count[t], c) {
                            (Some(a), Some(b)) => a.checked_add(b),
                            _ => None,
                        };
                        infinite[t] |= inf;
                    }
                }
            }
        }
        for (n, node) in nodes.iter_mut().enumerate() {
            node.copies = if self.too_large {
                Copies::TooLarge
            } else if net.root == Root::None {
                Copies::NoRoot
            } else if infinite[n] {
                Copies::Cycle
            } else {
                count[n].map_or(Copies::Overflow, Copies::Count)
            };
        }

        hier.defs = (0..net.defs.len())
            .map(|i| DefHier {
                first_node: self.first_node[i],
                x_lines: 0,
                devices: devices[i],
                copies: Copies::Inactive,
                scoped: self.scoped[i],
                hot: self.hot[i],
            })
            .collect();
        for (d, &on) in hier.defs.iter_mut().zip(&active) {
            let mut at = d.first_node;
            let mut copies = None;
            while at != NONE {
                let node = &nodes[at as usize];
                copies = Some(copies.map_or(node.copies, |c: Copies| c.add(node.copies)));
                at = node.next;
            }
            d.copies = match copies {
                Some(c) => c,
                None if !on => Copies::Inactive,
                None => Copies::TooLarge,
            };
        }
        hier.nodes = nodes;
        hier.edges = self.edges;
        // `x_lines`: each `X` line once for each definition it binds to in
        // the binding nodes of its definition (the edges of the nodes of a
        // definition align). A stopped analysis left nodes without edges.
        if !self.too_large {
            let mut x_lines = vec![0_u32; net.defs.len()];
            let mut targets: Vec<DefId> = Vec::new();
            for i in 0..net.defs.len() {
                let id = DefId(i as u32);
                let Some((_, first)) = hier.def_nodes(id).next() else {
                    continue;
                };
                for j in 0..first.edges.range().len() {
                    targets.clear();
                    for (_, node) in hier.binding_nodes(id) {
                        let e = hier.edges[node.edges.start as usize + j];
                        if e.target != NONE {
                            targets.push(hier.nodes[e.target as usize].def);
                        }
                    }
                    targets.sort_unstable();
                    targets.dedup();
                    for t in &targets {
                        x_lines[t.idx()] += 1;
                    }
                }
            }
            for (d, n) in hier.defs.iter_mut().zip(x_lines) {
                d.x_lines = n;
            }
        }

        // the diagnostics. A cycle is on an instance path from the root when
        // its first node is (the copies are not computed when the analysis
        // stopped)
        let n_reached = self.n_reached;
        let on_path = |c: &CycleReport| c.first < n_reached;
        for c in &self.cycles {
            diags.push(cycle_diag(net, &c.steps, on_path(c)));
        }
        if self.more_cycles {
            let message = if self.cycles.len() >= MAX_CYCLES {
                format!(
                    "only the first {MAX_CYCLES} call cycles are reported: the deck can have more"
                )
            } else {
                format!(
                    "the search for call cycles stopped after {MAX_CYCLE_STEPS} steps: the deck can have more call cycles than the {} reported",
                    self.cycles.len()
                )
            };
            diags.push(Diag {
                level: if infinite.contains(&true) || self.cycles.iter().any(on_path) {
                    Level::Error
                } else {
                    Level::Warning
                },
                code: "subckt_cycle",
                message,
                loc: None,
            });
        }
        let mut unresolved = self.unresolved;
        // a definition that the root reaches: on its instance paths only
        unresolved.retain(|&(n, _)| hier.binding_node(n));
        unresolved.sort_unstable_by_key(|&(n, j)| (hier.nodes[n as usize].def, j));
        let mut counted_unresolved = 0_usize;
        let mut counted_va = 0_usize;
        let mut counted_prot = 0_usize;
        // the `.model` cards of the first run, by (scope, name): their
        // types. An `X` line can call a model of a Verilog-A module
        let mut models: FxMap<(DefId, Sym), Sym> = FxMap::default();
        if !unresolved.is_empty() {
            for (i, d) in net.defs.iter().enumerate() {
                for m in d.models.iter().filter(|m| m.alter.is_none()) {
                    models.entry((DefId(i as u32), m.name)).or_insert(m.kind);
                }
            }
        }
        // the type of the `.model` card named `c` that is visible from
        // definition `def` (in it, in a definition it is written inside,
        // or at the top level), when that type is not built in: then `c`
        // can be a model of a Verilog-A module. HSPICE rejects an `X` line
        // that calls a model of a built-in type (golden
        // `hier_model_callee`)
        let visible_model = |def: DefId, c: Sym| -> Option<Sym> {
            let mut at = Some(def);
            while let Some(d) = at {
                if let Some(&kind) = models.get(&(d, c)) {
                    return Some(kind);
                }
                at = (d != ROOT).then(|| net.defs[d.idx()].parent.unwrap_or(ROOT));
            }
            None
        };
        for group in unresolved.chunk_by(|a, b| {
            hier.nodes[a.0 as usize].def == hier.nodes[b.0 as usize].def && a.1 == b.1
        }) {
            let def = hier.nodes[group[0].0 as usize].def;
            let missing = group.iter().fold(Copies::Count(0), |c, &(n, _)| {
                c.add(hier.nodes[n as usize].copies)
            });
            let callee = net.defs[def.idx()].elements[group[0].1 as usize].callee;
            // a visible model card decides: a built-in type is never a
            // Verilog-A module, also with a `.hdl` line
            // (`hier_model_callee`)
            let va = match callee.and_then(|c| visible_model(def, c)) {
                Some(kind) if builtin_model_type(net.syms.name(kind)) => Va::No,
                Some(kind) => Va::Model(kind),
                None if net.hdl => Va::Hdl,
                None if net.protected => Va::Prot,
                None => Va::No,
            };
            if missing.count().is_some_and(|c| c > 0) {
                match va {
                    Va::No => counted_unresolved += 1,
                    Va::Prot => counted_prot += 1,
                    _ => counted_va += 1,
                }
            }
            let contexts = hier.binding_nodes(def).count();
            let reach = if hier.reached(def) {
                Reach::Path { va }
            } else {
                Reach::Imagined {
                    alone: self.alone[def.idx()],
                }
            };
            diags.push(unresolved_diag(
                net,
                def,
                group[0].1,
                (missing, hier.defs[def.idx()].copies),
                (group.len(), contexts),
                reach,
            ));
        }
        // an `X` line whose node count is not the port count of the
        // definition it binds to: HSPICE rejects it on an instance path
        // (golden `hier_port_more`, `hier_port_fewer`), and runs a deck
        // where no instance path reaches it (`hier_port_unused`). One
        // diagnostic per `X` line, for the first such definition. A stopped
        // analysis checks the `X` lines that bind the same way in every
        // context: to a definition nested in their own, or by a name that
        // no lookup scope nests. It does not know the paths: an Error then,
        // as for `m`
        let mut ports_rejected = 0_usize;
        let mut bad: Vec<(DefId, u32, DefId)> = Vec::new();
        if self.too_large {
            for (i, d) in net.defs.iter().enumerate() {
                if !active[i] {
                    continue;
                }
                let id = DefId(i as u32);
                for k in self.xl_start[i]..self.xl_start[i + 1] {
                    let j = self.xl[k as usize];
                    let x = &d.elements[j as usize];
                    let Some(c) = x.callee else { continue };
                    let own = (id != ROOT)
                        .then(|| net.def_index.get(&(id, c)).copied())
                        .flatten();
                    let t = match own {
                        Some(t) => t,
                        None if !self.nested_in.contains(c) => {
                            match net.def_index.get(&(ROOT, c)) {
                                Some(&t) => t,
                                None => continue,
                            }
                        }
                        None => continue,
                    };
                    if d.element_pins(x).len() != net.defs[t.idx()].ports.len() {
                        bad.push((id, j, t));
                    }
                }
            }
        } else {
            for (n, node) in hier.nodes.iter().enumerate() {
                if !hier.binding_node(n as u32) {
                    continue;
                }
                let d = &net.defs[node.def.idx()];
                for e in &hier.edges[node.edges.range()] {
                    if e.target == NONE {
                        continue;
                    }
                    let t = hier.nodes[e.target as usize].def;
                    let x = &d.elements[e.elem as usize];
                    if d.element_pins(x).len() != net.defs[t.idx()].ports.len() {
                        bad.push((node.def, e.elem, t));
                    }
                }
            }
        }
        bad.sort_unstable();
        bad.dedup_by_key(|b| (b.0, b.1));
        for (def, elem, t) in bad {
            let on_path = self.too_large || hier.reached(def);
            ports_rejected += usize::from(on_path);
            diags.push(port_diag(net, def, elem, t, on_path));
        }
        // `m` <= 0: HSPICE rejects it on an instance path (golden
        // `hier_m_zero`, `hier_m_negative`, `elem_m_zero`,
        // `elem_m_negative`), and does not check it in a definition that no
        // instance path reaches (`elem_m_zero_unused`). A stopped analysis
        // does not know the paths: an Error then
        let mut m_rejected = 0_usize;
        if let Some(m) = net.syms.get("m") {
            for (i, d) in net.defs.iter().enumerate() {
                if !active[i] {
                    continue;
                }
                let id = DefId(i as u32);
                let on_path = self.too_large || hier.reached(id);
                for e in d.elements.iter().filter(|e| e.alter.is_none()) {
                    if let Some(text) = net.m_not_positive(d, e, m) {
                        m_rejected += usize::from(on_path);
                        diags.push(m_diag(net, id, e, &text, on_path));
                    }
                }
            }
        }
        // two element lines with one name in one definition: HSPICE
        // rejects it on an instance path (golden `elem_dup_name`,
        // `elem_dup_name_sub`, `hier_dup_instance`,
        // `hier_dup_instance_sub`), and does not check a definition that no
        // instance path reaches (`elem_dup_name_unused`,
        // `hier_dup_instance_unused`). `seen`: by name, the last definition
        // that has it
        let mut dup_rejected = 0_usize;
        let mut seen = vec![NONE; net.syms.len()];
        let mut dups: Vec<(DefId, u32)> = Vec::new();
        for (i, d) in net.defs.iter().enumerate() {
            if !active[i] {
                continue;
            }
            for (j, e) in d.elements.iter().enumerate() {
                if e.alter.is_some() {
                    continue;
                }
                let s = &mut seen[e.name.0 as usize];
                if *s == i as u32 {
                    dups.push((DefId(i as u32), j as u32));
                } else {
                    *s = i as u32;
                }
            }
        }
        drop(seen);
        // the first line of each such name, in each definition with one
        let mut first: FxMap<Sym, u32> = FxMap::default();
        for group in dups.chunk_by(|a, b| a.0 == b.0) {
            let id = group[0].0;
            let d = &net.defs[id.idx()];
            first.clear();
            for &(_, j) in group {
                first.insert(d.elements[j as usize].name, NONE);
            }
            for (j, e) in d.elements.iter().enumerate() {
                if e.alter.is_none()
                    && let Some(f) = first.get_mut(&e.name)
                    && *f == NONE
                {
                    *f = j as u32;
                }
            }
            let on_path = self.too_large || hier.reached(id);
            for &(_, j) in group {
                let e = &d.elements[j as usize];
                hier.dup_elems.insert((id, e.name));
                dup_rejected += usize::from(on_path);
                let earlier = &d.elements[first[&e.name] as usize];
                diags.push(dup_elem_diag(net, id, e, earlier, on_path));
            }
        }
        // forms that HSPICE rejects and that parameter resolution finds
        // only when a query reaches them
        let forms_rejected = crate::rejects::check(
            net,
            &active,
            |id| self.too_large || hier.reached(id),
            &mut diags,
        );
        if self.too_large {
            diags.push(Diag {
                level: Level::Warning,
                code: "hierarchy_too_large",
                message: format!(
                    "the nested definitions are used in too many combinations along the instance paths (the analysis needs more than {} context entries, nodes, edges, lookups in long contexts and cycle-check entries): the hierarchy was not analysed, so copies, expanded totals, `x_lines` and `X` line bindings are not given (`null`)",
                    self.budget
                ),
                loc: None,
            });
        }

        // the expanded totals
        let none = |note: &str| (None, None, Some(note.to_owned()));
        let mut parasitics = None;
        let (devs, insts, note) = if self.too_large {
            none("too many lookup contexts (diagnostic `hierarchy_too_large`)")
        } else if net.root == Root::None {
            none("no top-level instances")
        } else if hier.defs.iter().any(|d| d.copies == Copies::Cycle) {
            none(
                "a call cycle: a definition is instantiated inside an instance of itself, and HSPICE rejects the deck (diagnostic `subckt_cycle`)",
            )
        } else if hier.defs.iter().any(|d| d.copies == Copies::Overflow) {
            none("overflow: more than 2^64 instances")
        } else {
            match expanded_totals(net, &hier) {
                None => none("overflow: more than 2^64 devices"),
                Some((devs, insts, par)) => {
                    parasitics = (!par.is_empty()).then_some(par);
                    let mut notes = Vec::new();
                    let s = |n: usize| if n == 1 { "" } else { "s" };
                    if counted_unresolved > 0 {
                        let verb = if counted_unresolved == 1 {
                            "calls"
                        } else {
                            "call"
                        };
                        notes.push(format!(
                            "lower bound: {counted_unresolved} `X` line{} {verb} a subckt with no visible definition on some instance paths, and HSPICE rejects the deck (diagnostic `unresolved_subckt`); the instances they would make are not counted",
                            s(counted_unresolved)
                        ));
                    }
                    if counted_prot > 0 {
                        notes.push(format!(
                            "lower bound: {counted_prot} `X` line{} on instance paths {} a subckt with no visible definition, which a `.prot` region (not parsed) can define (golden `prot_subckt`: HSPICE runs the deck); the instances they make are not counted",
                            s(counted_prot),
                            if counted_prot == 1 { "calls" } else { "call" }
                        ));
                    }
                    if counted_va > 0 {
                        notes.push(format!(
                            "{counted_va} `X` line{} on instance paths {} a name with no subckt definition, which can be a Verilog-A module (`.hdl`) or a model of one (diagnostic `unresolved_subckt`); {} counted neither as subckt instances nor as devices",
                            s(counted_va),
                            if counted_va == 1 { "calls" } else { "call" },
                            if counted_va == 1 { "it is" } else { "they are" }
                        ));
                    }
                    if ports_rejected > 0 {
                        notes.push(format!(
                            "caveat: {ports_rejected} `X` line{} on instance paths {} a node count that is not the port count of the subckt {}, and HSPICE rejects the deck (diagnostic `port_count_mismatch`); {} instances are counted",
                            s(ports_rejected),
                            if ports_rejected == 1 { "has" } else { "have" },
                            if ports_rejected == 1 { "it calls" } else { "they call" },
                            if ports_rejected == 1 { "its" } else { "their" }
                        ));
                    }
                    if m_rejected > 0 {
                        notes.push(format!(
                            "caveat: {m_rejected} element or `X` line{} on instance paths {} `m` <= 0, and HSPICE rejects the deck (diagnostic `m_not_positive`); they are counted",
                            s(m_rejected),
                            if m_rejected == 1 { "has" } else { "have" }
                        ));
                    }
                    if dup_rejected > 0 {
                        notes.push(format!(
                            "caveat: {dup_rejected} element or `X` line{} on instance paths {} the name of an earlier line of {} definition, and HSPICE rejects the deck (diagnostic `duplicate_element`); {} counted",
                            s(dup_rejected),
                            if dup_rejected == 1 { "has" } else { "have" },
                            if dup_rejected == 1 { "its" } else { "their" },
                            if dup_rejected == 1 { "it is" } else { "they are" }
                        ));
                    }
                    if forms_rejected > 0 {
                        notes.push(format!(
                            "caveat: {forms_rejected} line{} {} a form that HSPICE rejects (diagnostics `undefined_param`, `override_self`, `statistical_in_element`, `name_then_model`, `inductor_model_level`); the counts do not change",
                            s(forms_rejected),
                            if forms_rejected == 1 { "has" } else { "have" }
                        ));
                    }
                    if load_errors > 0 {
                        notes.push(format!(
                            "caveat: the load gave {load_errors} error diagnostic{}; a statement that an error skipped is not counted",
                            s(load_errors)
                        ));
                    }
                    if protected > 0 {
                        notes.push(format!(
                            "caveat: {protected} protected region{} (`.prot`, diagnostic `protected_region`) not parsed; the elements and instances inside are not counted",
                            s(protected)
                        ));
                    }
                    let note = (!notes.is_empty()).then(|| notes.join("; "));
                    (Some(devs), Some(insts), note)
                }
            }
        };
        hier.expanded_devices = devs;
        hier.expanded_parasitics = parasitics;
        hier.expanded_instances = insts;
        hier.expanded_note = note;
        (hier, diags)
    }
}

/// The strongly connected components of a graph (Tarjan, without
/// recursion): the vertex `v` has the edges `adj[start[v]..start[v + 1]]`.
/// Returns the component of each vertex, numbered from 0.
fn components(start: &[u32], adj: &[u32]) -> Vec<u32> {
    let n = start.len() - 1;
    let mut index = vec![NONE; n];
    let mut low = vec![0_u32; n];
    let mut on_stack = vec![false; n];
    let mut comp = vec![NONE; n];
    let mut stack: Vec<u32> = Vec::new();
    // (vertex, its next edge)
    let mut calls: Vec<(u32, u32)> = Vec::new();
    let (mut next_index, mut n_comps) = (0_u32, 0_u32);
    for root in 0..n {
        if index[root] != NONE {
            continue;
        }
        index[root] = next_index;
        low[root] = next_index;
        next_index += 1;
        stack.push(root as u32);
        on_stack[root] = true;
        calls.push((root as u32, start[root]));
        while let Some(&(v, e)) = calls.last() {
            let v = v as usize;
            if e < start[v + 1] {
                calls.last_mut().expect("not empty").1 = e + 1;
                let w = adj[e as usize] as usize;
                if index[w] == NONE {
                    index[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    stack.push(w as u32);
                    on_stack[w] = true;
                    calls.push((w as u32, start[w]));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            calls.pop();
            if let Some(&(p, _)) = calls.last() {
                low[p as usize] = low[p as usize].min(low[v]);
            }
            if low[v] == index[v] {
                loop {
                    let w = stack.pop().expect("on the stack") as usize;
                    on_stack[w] = false;
                    comp[w] = n_comps;
                    if w == v {
                        break;
                    }
                }
                n_comps += 1;
            }
        }
    }
    comp
}

/// By definition: an instance path from the root can reach it (by name,
/// over every definition a name can bind to, so a superset), or it is a
/// definition that a nested `top` is written inside (its context holds
/// them). The cost is linear in the definitions and `X` lines.
fn reach_by_name(net: &Netlist, xl_start: &[u32], xl: &[u32], active: &[bool]) -> Vec<bool> {
    let n_defs = net.defs.len();
    let mut reach = vec![false; n_defs];
    let start = match net.root {
        Root::TopLevel => ROOT,
        Root::Subckt(id) => id,
        Root::None => return reach,
    };
    let mut at = net.defs[start.idx()].parent;
    while let Some(p) = at {
        reach[p.idx()] = true;
        at = net.defs[p.idx()].parent;
    }
    let by_name = ByName::new(|| {
        (1..n_defs)
            .filter(|&i| active[i])
            .map(|i| (net.defs[i].name, DefId(i as u32)))
    });
    reach[start.idx()] = true;
    let mut work = vec![start.idx()];
    let mut done: FxSet<Sym> = FxSet::default();
    while let Some(d) = work.pop() {
        let elements = &net.defs[d].elements;
        for &j in &xl[xl_start[d] as usize..xl_start[d + 1] as usize] {
            let Some(c) = elements[j as usize].callee else {
                continue;
            };
            if !done.insert(c) {
                continue;
            }
            for &t in by_name.get(c) {
                if !reach[t.idx()] {
                    reach[t.idx()] = true;
                    work.push(t.idx());
                }
            }
        }
    }
    reach
}

/// By definition: an `X` line in it, or in a definition that one of its
/// `X` lines can bind to, and so on down, calls a name in `nested_in`. A
/// name can bind to every definition of that name (top-level or nested
/// anywhere), so this is a superset. An `X` line below a definition that
/// is not hot binds to a top-level definition in every context. The cost
/// is linear in the definitions and `X` lines.
fn hot_defs(net: &Netlist, xl_start: &[u32], xl: &[u32], nested_in: &ByName) -> Vec<bool> {
    let n_defs = net.defs.len();
    let mut hot = vec![false; n_defs];
    if nested_in.defs.is_empty() {
        return hot;
    }
    let callees = |d: usize| {
        let elements = &net.defs[d].elements;
        xl[xl_start[d] as usize..xl_start[d + 1] as usize]
            .iter()
            .filter_map(move |&j| elements[j as usize].callee)
    };
    // the definitions that call each name
    let callers = ByName::new(|| {
        (0..n_defs).flat_map(move |d| callees(d).map(move |c| (c, DefId(d as u32))))
    });
    let mut work: Vec<usize> = (0..n_defs)
        .filter(|&d| callees(d).any(|c| nested_in.contains(c)))
        .collect();
    for &d in &work {
        hot[d] = true;
    }
    let mut done: FxSet<Sym> = FxSet::default();
    while let Some(d) = work.pop() {
        let name = net.defs[d].name;
        if d == ROOT.idx() || !done.insert(name) {
            continue;
        }
        for &c in callers.get(name) {
            if !hot[c.idx()] {
                hot[c.idx()] = true;
                work.push(c.idx());
            }
        }
    }
    hot
}

/// The slot of a device type in [`expanded_totals`]; `None` for an `X`
/// line.
fn slot(kind: ElemKind) -> Option<usize> {
    Some(match kind {
        ElemKind::Instance => return None,
        ElemKind::Resistor => 0,
        ElemKind::Capacitor => 1,
        ElemKind::Inductor => 2,
        ElemKind::Vsource => 3,
        ElemKind::Isource => 4,
        ElemKind::Mosfet => 5,
        ElemKind::Bjt => 6,
        ElemKind::Diode => 7,
        ElemKind::Other(c) if c.is_ascii_lowercase() => 8 + (c - b'a') as usize,
        ElemKind::Other(_) => 34,
    })
}

/// The expanded device totals by type, the expanded instances, and the
/// expanded parasitic R and C by type; `None` on overflow. All copies are
/// counts.
#[allow(clippy::type_complexity)]
fn expanded_totals(
    net: &Netlist,
    hier: &Hier,
) -> Option<(BTreeMap<String, u64>, u64, BTreeMap<String, u64>)> {
    let mut parasitic: BTreeMap<String, u64> = BTreeMap::new();
    let mut totals = [0_u64; 35];
    let mut names: [&str; 35] = [""; 35];
    let mut instances = 0_u64;
    for (i, d) in net.defs.iter().enumerate() {
        let c = hier.defs[i].copies.count().unwrap_or(0);
        if c == 0 {
            continue;
        }
        if i != ROOT.idx() {
            instances = instances.checked_add(c)?;
        }
        if hier.defs[i].devices == 0 && net.included_dspf(DefId(i as u32)).is_none() {
            continue;
        }
        for e in d.elements.iter().filter(|e| e.alter.is_none()) {
            if e.parasitic {
                let n = parasitic.entry(e.kind.name().to_owned()).or_default();
                *n = n.checked_add(c)?;
                continue;
            }
            if let Some(k) = slot(e.kind) {
                totals[k] = totals[k].checked_add(c)?;
                names[k] = e.kind.name();
            }
        }
    }
    if let Root::Subckt(_) = net.root {
        // the root definition is not an instance
        instances -= 1;
    }
    let map = totals
        .iter()
        .zip(names)
        .filter(|(n, _)| **n > 0)
        .map(|(n, name)| (name.to_owned(), *n))
        .collect();
    Some((map, instances, parasitic))
}

/// The diagnostic of a call cycle (`a` (`xb`) -> `b` (`xa`) -> `a`). On an
/// instance path from the root (`reached`), HSPICE rejects the deck
/// ("subcircuit a is defined recursively", golden `hier_reentry`): an
/// Error. A cycle that no instance path goes through makes no instances,
/// and HSPICE runs the deck (golden `hier_unused_cycle_unres`): a Warning.
fn cycle_diag(net: &Netlist, steps: &[(DefId, u32)], reached: bool) -> Diag {
    let mut chain = String::new();
    for &(def, elem) in steps {
        let e = &net.defs[def.idx()].elements[elem as usize];
        chain.push_str(&format!(
            "`{}` (`{}`) -> ",
            net.def_display(def),
            net.syms.name(e.name)
        ));
    }
    let first = net.def_display(steps[0].0);
    chain.push_str(&format!("`{first}`"));
    let (last_def, last_elem) = steps[steps.len() - 1];
    let loc = net.defs[last_def.idx()].elements[last_elem as usize].loc;
    let rule = "HSPICE rejects a deck where an instance path goes through such a cycle (\"defined recursively\"), also when the inner instance binds its `X` lines to other definitions, so that the expansion would end (golden `hier_reentry`)";
    let (level, effect) = if net.root == Root::None {
        (
            Level::Warning,
            format!(
                "{rule}. The deck has no top-level instances, so the copies of these definitions are unknown"
            ),
        )
    } else if reached {
        (
            Level::Error,
            format!(
                "{rule}. The copies of these definitions and of the definitions below them, and the expanded totals, are unknown"
            ),
        )
    } else {
        (
            Level::Warning,
            format!(
                "No instance path from the root goes through it, so it makes no instances, and HSPICE runs the deck (golden `hier_unused_cycle_unres`). {rule}"
            ),
        )
    };
    Diag {
        level,
        code: "subckt_cycle",
        message: format!(
            "call cycle: {chain}: an instance of `{first}` is inside an instance of `{first}`. {effect}"
        ),
        loc: Some(loc),
    }
}

/// Why the callee of an `X` line with no subckt definition can be a
/// Verilog-A module (not probed: `hspice.exe` runs no Verilog-A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Va {
    No,
    /// The first run has a `.hdl` line.
    Hdl,
    /// The deck has a `.prot` region, which is not parsed and can define
    /// the subckt: HSPICE runs such a deck (golden `prot_subckt`).
    Prot,
    /// A `.model` card of that name, visible from the line, has this type,
    /// which is not a built-in HSPICE type.
    Model(Sym),
}

/// The model types that HSPICE builds in (`.model <name> <type>`). An `X`
/// line that calls a model of one of these types has no definition, and
/// HSPICE rejects the deck (golden `hier_model_callee`).
fn builtin_model_type(kind: &str) -> bool {
    const TYPES: [&str; 23] = [
        "amp", "c", "core", "csw", "d", "l", "lpnp", "nmf", "njf", "nmos", "npn", "opt", "pjf",
        "plot", "pmf", "pmos", "pnp", "r", "s", "sp", "sw", "u", "w",
    ];
    TYPES.iter().any(|t| t.eq_ignore_ascii_case(kind))
}

/// How the `X` lines of a definition were checked.
#[derive(Debug, Clone, Copy)]
enum Reach {
    /// On the instance paths from the root. `va`: the callee can be a
    /// Verilog-A module or a model of one.
    Path { va: Va },
    /// No instance path from the root reaches the definition: in imagined
    /// contexts, its own one (as if instantiated at the top level, or
    /// inside its text parents) when `alone`, and those of the definitions
    /// with no instances that call it.
    Imagined { alone: bool },
}

/// The diagnostic of `X` line `elem` of `def`, which has no visible
/// definition in `contexts.0` of the `contexts.1` binding contexts of
/// `def`, for `copies.0` of the `copies.1` copies of `def`. On an instance
/// path from the root, HSPICE rejects the deck (golden
/// `hier_top_nested_only`, `hier_unres_in_subckt`): an Error. In a
/// definition that the root does not reach, HSPICE does not check the line
/// (golden `hier_unused_cycle_unres`, `hier_unused_nested_unres`): a
/// Warning. The message is short (the summary notes that the totals leave
/// these instances out): a deck loaded without its cell library has one
/// for each `X` line.
fn unresolved_diag(
    net: &Netlist,
    def: DefId,
    elem: u32,
    (missing, total): (Copies, Copies),
    contexts: (usize, usize),
    reach: Reach,
) -> Diag {
    let e = &net.defs[def.idx()].elements[elem as usize];
    let callee = e.callee.map_or("", |c| net.syms.name(c));
    let x = net.syms.name(e.name);
    let shown = net.def_display(def);
    let place = if def == ROOT {
        "at the top level".to_owned()
    } else {
        format!("in subckt `{shown}`")
    };
    let (level, message) = if let Reach::Imagined { alone } = reach {
        let why = if net.root == Root::None {
            "the deck has no top-level instances".to_owned()
        } else if def == ROOT {
            "the top level is not on the instance paths: `top` names the root".to_owned()
        } else {
            format!("`{shown}` has no instances")
        };
        let own = match net.defs[def.idx()].parent {
            _ if def == ROOT => "at the top level".to_owned(),
            None => "as if instantiated at the top level".to_owned(),
            Some(p) => format!(
                "as if instantiated inside an instance of `{}`, the definition it is written in",
                net.def_display(p)
            ),
        };
        let callers = "as called from definitions with no instances";
        let checked = match (alone, contexts) {
            (true, (_, 1)) => format!("when checked {own}"),
            (true, (k, m)) => {
                format!("in {k} of the {m} contexts where it was checked ({own}, and {callers})")
            }
            (false, (_, 1)) => {
                "when checked as called from a definition with no instances".to_owned()
            }
            (false, (k, m)) => {
                format!("in {k} of the {m} contexts where it was checked ({callers})")
            }
        };
        (
            Level::Warning,
            format!(
                "`{x}` {place} calls subckt `{callee}`, which has no visible definition {checked} ({why}). HSPICE does not check a line that no instance path reaches"
            ),
        )
    } else {
        let va = match reach {
            Reach::Path { va } => va,
            Reach::Imagined { .. } => Va::No,
        };
        let scope = match (missing, total) {
            (Copies::Count(m), Copies::Count(t)) if def != ROOT => match (m, t) {
                (1, 1) => format!(" on the one instance path of `{shown}`"),
                _ if m == t => format!(" on all {t} instance paths of `{shown}`"),
                _ => format!(" on {m} of the {t} instance paths of `{shown}`"),
            },
            (Copies::Count(_), Copies::Count(_)) => String::new(),
            _ if contexts.1 > 1 => format!(
                " in {} of the {} lookup contexts of `{shown}`",
                contexts.0, contexts.1
            ),
            _ => String::new(),
        };
        if va == Va::Prot {
            (
                Level::Warning,
                format!(
                    "`{x}` {place} calls subckt `{callee}`, which has no visible definition{scope}: a `.prot` region, which is not parsed, can define it (golden `prot_subckt`: HSPICE runs the deck)"
                ),
            )
        } else if va != Va::No {
            // short, with no scope: a deck loaded without its cell library
            // has one for each `X` line, and the summary note gives the
            // rule and the counts
            let why = match va {
                Va::Model(kind) => format!("`.model` type `{}`", net.syms.name(kind)),
                _ => "`.hdl` line".to_owned(),
            };
            (
                Level::Warning,
                format!("`{x}` {place} calls `{callee}`: no subckt; can be Verilog-A ({why})"),
            )
        } else {
            (
                Level::Error,
                format!(
                    "`{x}` {place} calls subckt `{callee}`, which has no visible definition{scope}: HSPICE rejects the deck"
                ),
            )
        }
    };
    Diag {
        level,
        code: "unresolved_subckt",
        // an allocation of the exact size (`format!` reserves more)
        message: message.as_str().to_owned(),
        loc: Some(e.loc),
    }
}

/// The diagnostic of element `e` of `def`, whose `m` is `text`, a number
/// <= 0: an Error on an instance path from the root (`on_path`), else a
/// Warning.
fn m_diag(net: &Netlist, def: DefId, e: &Element, text: &str, on_path: bool) -> Diag {
    let x = net.syms.name(e.name);
    let place = if def == ROOT {
        "at the top level".to_owned()
    } else {
        format!("in subckt `{}`", net.def_display(def))
    };
    let rule = "HSPICE rejects an element or `X` line whose multiplier is zero or less (\"Multiplication factor(M) ... cannot be < or = zero\", golden `elem_m_zero`, `hier_m_zero`)";
    let (level, message) = if on_path {
        (
            Level::Error,
            format!(
                "`{x}` {place} has `m={text}`: {rule}. The counts still count it once per instance"
            ),
        )
    } else {
        (
            Level::Warning,
            format!(
                "`{x}` {place} has `m={text}`: {rule}, but only on an instance path, and no instance path from the root reaches this line, so HSPICE runs the deck (golden `elem_m_zero_unused`)"
            ),
        )
    };
    Diag {
        level,
        code: "m_not_positive",
        message,
        loc: Some(e.loc),
    }
}

/// The diagnostic of element line `e` of `def`, which has the name of the
/// earlier line `earlier` of `def`: an Error on an instance path from the
/// root (`on_path`), else a Warning. HSPICE rejects two element or `X`
/// lines with one name in one definition on an instance path ("attempts to
/// redefine", golden `elem_dup_name`, `hier_dup_instance`) and does not
/// check a definition that no instance path reaches
/// (`elem_dup_name_unused`). The message is short, as a file included
/// twice gives one for each of its lines.
fn dup_elem_diag(net: &Netlist, def: DefId, e: &Element, earlier: &Element, on_path: bool) -> Diag {
    let x = net.syms.name(e.name);
    let place = if def == ROOT {
        "at the top level".to_owned()
    } else {
        format!("in `{}`", net.def_display(def))
    };
    let what = if earlier.loc == e.loc {
        // a file read twice (`.include` twice, golden `inc_twice`)
        "is read twice".to_owned()
    } else if earlier.loc.file == e.loc.file {
        format!("repeats the name of line {}", earlier.loc.line)
    } else {
        let l = net.loc(earlier.loc);
        let file = std::path::Path::new(&l.file)
            .file_name()
            .map_or(l.file.as_str(), |f| f.to_str().unwrap_or(&l.file));
        format!("repeats the name of {file}:{}", l.line)
    };
    let (level, message) = if on_path {
        (
            Level::Error,
            format!("`{x}` {place} {what}: HSPICE rejects the deck"),
        )
    } else {
        (
            Level::Warning,
            format!("`{x}` {place} {what}; no instance path reaches it, so HSPICE runs the deck"),
        )
    };
    Diag {
        level,
        code: "duplicate_element",
        message,
        loc: Some(e.loc),
    }
}

/// The diagnostic of `X` line `elem` of `def`, whose node count is not the
/// port count of definition `callee`, which it binds to: an Error on an
/// instance path from the root (`on_path`), else a Warning.
fn port_diag(net: &Netlist, def: DefId, elem: u32, callee: DefId, on_path: bool) -> Diag {
    let d = &net.defs[def.idx()];
    let e = &d.elements[elem as usize];
    let place = if def == ROOT {
        "at the top level".to_owned()
    } else {
        format!("in subckt `{}`", net.def_display(def))
    };
    let head = format!(
        "`{}` {place} has {} nodes, but subckt `{}` has {} ports",
        net.syms.name(e.name),
        d.element_pins(e).len(),
        net.def_display(callee),
        net.defs[callee.idx()].ports.len()
    );
    let (level, message) = if on_path {
        (Level::Error, format!("{head}: HSPICE rejects the deck"))
    } else {
        (
            Level::Warning,
            format!("{head}; no instance path reaches it, so HSPICE runs the deck"),
        )
    };
    Diag {
        level,
        code: "port_count_mismatch",
        message,
        loc: Some(e.loc),
    }
}

/// Case-insensitive glob match (`*`, `?`) of the whole `text`.
fn glob(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p].eq_ignore_ascii_case(&text[t])) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if let Some((sp, st)) = star {
            p = sp + 1;
            t = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// The edit distance of `a` and `b`, ignoring ASCII letter case.
fn distance(a: &[u8], b: &[u8]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = if ca.eq_ignore_ascii_case(cb) {
                diag
            } else {
                1 + diag.min(row[j]).min(row[j + 1])
            };
            diag = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// The names among `names` that are close to `wanted`, closest first (at
/// most 5).
pub(crate) fn suggest<'a>(
    wanted: &str,
    names: impl Iterator<Item = (&'a str, String)>,
) -> Vec<String> {
    let wanted = wanted.trim();
    let w = wanted.as_bytes();
    let limit = (w.len() / 3).max(1);
    let lower = wanted.to_ascii_lowercase();
    // a name that holds the wanted text comes after the close names
    let holds = |key: &str| w.len() >= 3 && key.to_ascii_lowercase().contains(&lower);
    let mut close: Vec<(usize, String)> = names
        .filter_map(|(key, shown)| {
            let near = key.len().abs_diff(w.len()) <= limit;
            match near.then(|| distance(w, key.as_bytes())) {
                Some(d) if d <= limit => Some((d, shown)),
                _ if holds(key) => Some((limit + 1, shown)),
                _ => None,
            }
        })
        .collect();
    close.sort();
    close.dedup_by(|a, b| a.1 == b.1);
    close.into_iter().take(5).map(|(_, s)| s).collect()
}

impl Netlist {
    /// The definition that `name` names (the rule of `find_def`), or a
    /// `NotFound` with close names, or with the qualified names of an
    /// ambiguous nested name.
    fn subckt_by_name(&self, name: &str) -> Result<DefId, QueryError> {
        match self.find_def(name) {
            DefLookup::Found(id) => Ok(id),
            DefLookup::Ambiguous(names) => Err(QueryError::NotFound {
                what: "subckt".to_owned(),
                name: name.to_owned(),
                suggestions: names,
            }),
            DefLookup::NotFound => {
                let wanted = name.trim();
                let plain = !wanted.contains('>');
                let names = (1..self.defs.len())
                    .map(|i| DefId(i as u32))
                    .filter(|&id| self.def_active(id))
                    .map(|id| {
                        let shown = self.def_display(id);
                        let key = if plain {
                            Cow::Borrowed(self.syms.name(self.defs[id.idx()].name))
                        } else {
                            Cow::Owned(shown.clone())
                        };
                        (key, shown)
                    })
                    .collect::<Vec<_>>();
                Err(QueryError::NotFound {
                    what: "subckt".to_owned(),
                    name: name.to_owned(),
                    suggestions: suggest(
                        wanted,
                        names.iter().map(|(k, s)| (k.as_ref(), s.clone())),
                    ),
                })
            }
        }
    }

    /// The text of `p` for answers (`param_text`, or its number).
    pub(crate) fn name_text(&self, p: &ParamDef) -> NameText {
        NameText {
            name: self.syms.name(p.name).to_owned(),
            text: self
                .param_text(p)
                .map(Cow::into_owned)
                .or_else(|| p.num.map(|n| n.to_string()))
                .unwrap_or_default(),
        }
    }

    /// The text of the `m` of element `e` of `def` (`m` is the `Sym` of
    /// `m`), when it is a number <= 0 (with its sign, as written). An
    /// expression is not evaluated here. HSPICE rejects such an R or `X`
    /// line on an instance path (golden `elem_m_zero`, `hier_m_zero`); the
    /// other types the parser reads (C, L, M, Q, D) get the same check.
    pub(crate) fn m_not_positive(&self, def: &Def, e: &Element, m: Sym) -> Option<String> {
        if matches!(
            e.kind,
            ElemKind::Vsource | ElemKind::Isource | ElemKind::Other(_)
        ) {
            return None;
        }
        let p = def.element_params(e).iter().find(|p| p.name == m)?;
        let text = self.param_text(p);
        let value = p.num.or_else(|| {
            let t = text.as_deref()?.trim();
            let (sign, rest) = match t.strip_prefix('-') {
                Some(rest) => (-1.0, rest),
                None => (1.0, t.strip_prefix('+').unwrap_or(t)),
            };
            netlist_expr::scan_number(rest)
                .filter(|&(_, n)| n == rest.len())
                .map(|(x, _)| sign * x)
        })?;
        (value <= 0.0).then(|| {
            text.map(Cow::into_owned)
                .unwrap_or_else(|| value.to_string())
        })
    }

    /// `path:line` of `loc`.
    fn loc_string(&self, loc: LocIdx) -> String {
        let l = self.loc(loc);
        format!("{}:{}", l.file, l.line)
    }

    /// The note on the copies of definition `id`: why they are `None`, or
    /// a caveat.
    fn copies_note(&self, id: DefId) -> Option<String> {
        let shown = || self.def_display(id);
        match self.hier.defs[id.idx()].copies {
            Copies::Count(_) => self.hier.dup_at.get(&id).map(|&at| {
                format!(
                    "HSPICE rejects this deck: `{}` is defined again at {}. The counts use this (first) definition",
                    shown(),
                    self.loc_string(at)
                )
            }),
            Copies::Cycle => Some(
                "call cycle: a definition is instantiated inside an instance of itself on an instance path through this one, and HSPICE rejects the deck (diagnostic `subckt_cycle`)"
                    .to_owned(),
            ),
            Copies::Overflow => Some("overflow".to_owned()),
            Copies::NoRoot => Some("no top-level instances".to_owned()),
            Copies::TooLarge => Some("too many lookup contexts".to_owned()),
            Copies::Inactive => {
                let def = &self.defs[id.idx()];
                Some(match def.dup_of {
                    Some(first) => format!(
                        "duplicate definition: HSPICE rejects this deck. Queries use the first definition, at {}",
                        self.loc_string(self.defs[first.idx()].loc)
                    ),
                    None => {
                        let mut at = def.parent;
                        while let Some(p) = at {
                            if self.defs[p.idx()].dup_of.is_some() {
                                break;
                            }
                            at = self.defs[p.idx()].parent;
                        }
                        format!(
                            "inside the duplicate definition `{}`, which queries do not use",
                            at.map_or_else(String::new, |p| self.def_display(p))
                        )
                    }
                })
            }
        }
    }

    /// The `x_lines` of definition `id`; `None` when the analysis stopped
    /// (diagnostic `hierarchy_too_large`).
    fn x_lines(&self, id: DefId) -> Option<usize> {
        (!self.hier.too_large).then(|| self.hier.defs[id.idx()].x_lines as usize)
    }

    fn subckt_row(&self, id: DefId) -> SubcktRow {
        let def = &self.defs[id.idx()];
        let h = &self.hier.defs[id.idx()];
        SubcktRow {
            name: self.def_display(id),
            ports: def.ports.len(),
            devices: h.devices as usize,
            x_lines: self.x_lines(id),
            copies: h.copies.count(),
            copies_note: self.copies_note(id),
            loc: self.loc(def.loc),
        }
    }

    /// The `.subckt`/`.macro` definitions of the first run (nested ones
    /// and duplicates included), with their counts. `filter` keeps the
    /// qualified names that contain it (ignoring letter case), or that
    /// match it as a glob when it has `*` or `?`.
    pub fn list_subckts(
        &self,
        filter: Option<&str>,
        sort: SubcktSort,
        page: &Page,
    ) -> Result<Paged<SubcktRow>, QueryError> {
        let filter = filter.map(str::trim).filter(|f| !f.is_empty());
        let lower_filter = filter.map(str::to_ascii_lowercase);
        let mut rows: Vec<(String, DefId)> = (1..self.defs.len())
            .filter(|&i| self.defs[i].alter.is_none())
            .map(|i| {
                let id = DefId(i as u32);
                (self.def_display(id).to_ascii_lowercase(), id)
            })
            .filter(|(key, _)| match (filter, &lower_filter) {
                (Some(f), _) if f.contains(['*', '?']) => glob(f.as_bytes(), key.as_bytes()),
                (_, Some(lf)) => key.contains(lf.as_str()),
                _ => true,
            })
            .collect();
        let h = &self.hier.defs;
        match sort {
            SubcktSort::Name => rows.sort_unstable(),
            SubcktSort::Copies => rows.sort_unstable_by(|a, b| {
                let (ca, cb) = (h[a.1.idx()].copies.count(), h[b.1.idx()].copies.count());
                match (ca, cb) {
                    (Some(x), Some(y)) => y.cmp(&x),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                }
                .then_with(|| a.cmp(b))
            }),
            SubcktSort::Devices => rows.sort_unstable_by(|a, b| {
                h[b.1.idx()]
                    .devices
                    .cmp(&h[a.1.idx()].devices)
                    .then_with(|| a.cmp(b))
            }),
        }
        Ok(api::page(rows.into_iter(), page)?.map(|(_, id)| self.subckt_row(id)))
    }

    /// One definition: ports, header defaults, local parameters, devices by
    /// type, the `X` lines by the subckt they call, counts, and a page of
    /// its element lines. `name` is the active top-level definition, else
    /// the one active nested definition with that name; `outer>cell` names
    /// a nested definition.
    pub fn subckt_info(&self, name: &str, page: &Page) -> Result<SubcktInfo, QueryError> {
        let id = self.subckt_by_name(name)?;
        let def = &self.defs[id.idx()];
        let first_run: Vec<(usize, &Element)> = def
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.alter.is_none() && !e.parasitic)
            .collect();
        let elements = api::page(first_run.iter(), page)?.map(|&(i, e)| ElementBrief {
            name: self.syms.name(e.name).to_owned(),
            kind: e.kind.name().to_owned(),
            model: self.elem_model(id, i).map(|m| self.syms.name(m).to_owned()),
            callee: e.callee.map(|c| self.syms.name(c).to_owned()),
            loc: self.loc(e.loc),
        });
        let (children, children_total) = self.children(id, def);
        let mut caveats: Vec<String> = self.dup_def_caveat(id).into_iter().collect();
        caveats.extend(self.def_alter_caveats(id));
        // the names of more than one element line of the definition
        let dup_names = self
            .hier
            .dup_elems
            .iter()
            .filter(|&&(d, _)| d == id)
            .count();
        if dup_names > 0 {
            caveats.push(format!(
                "{dup_names} element name{} of `{}` {} more than one line (diagnostic `duplicate_element`): HSPICE rejects the deck when an instance path reaches it. Queries use the first line",
                if dup_names == 1 { "" } else { "s" },
                self.def_display(id),
                if dup_names == 1 { "names" } else { "name" }
            ));
        }
        let h = &self.hier.defs[id.idx()];
        let params = || def.params.iter().filter(|p| p.alter.is_none());
        let funcs = || def.funcs.iter().filter(|f| f.alter.is_none());
        Ok(SubcktInfo {
            name: self.def_display(id),
            loc: self.loc(def.loc),
            ports: def
                .ports
                .iter()
                .take(MAX_LIMIT)
                .map(|&p| self.syms.name(p).to_owned())
                .collect(),
            ports_total: def.ports.len(),
            defaults: def
                .defaults
                .iter()
                .take(MAX_LIMIT)
                .map(|p| self.name_text(p))
                .collect(),
            defaults_total: def.defaults.len(),
            params: params()
                .take(MAX_LIMIT)
                .map(|p| self.name_text(p))
                .collect(),
            params_total: params().count(),
            funcs: funcs()
                .take(MAX_LIMIT)
                .map(|f| {
                    let args: Vec<&str> = f.args.iter().map(|&a| self.syms.name(a)).collect();
                    NameText {
                        name: format!("{}({})", self.syms.name(f.name), args.join(",")),
                        text: self.span_text(f.body).into_owned(),
                    }
                })
                .collect(),
            funcs_total: funcs().count(),
            devices_by_type: self.devices_by_type(id, &first_run),
            children,
            children_total,
            x_lines: self.x_lines(id),
            copies: h.copies.count(),
            copies_note: self.copies_note(id),
            caveats,
            elements,
            parasitics: self.def_parasitics(id),
        })
    }

    /// The caveat of a definition `id` that is defined again in its scope
    /// (HSPICE rejects the deck).
    fn dup_def_caveat(&self, id: DefId) -> Option<String> {
        self.hier.dup_at.get(&id).map(|&at| {
            format!(
                "HSPICE rejects this deck: `{}` is defined again at {} (golden `dup_subckt`). Queries use this (first) definition",
                self.def_display(id),
                self.loc_string(at)
            )
        })
    }

    /// The text of subckt `name` as written, from its `.subckt` line to
    /// its `.ends` line, as a page of lines. The text stops at the line of
    /// its last non-blank byte, so a definition with no `.ends` stops
    /// before the `.end` line.
    pub fn subckt_text(&self, name: &str, page: &Page) -> Result<SubcktText, QueryError> {
        let id = self.subckt_by_name(name)?;
        let def = &self.defs[id.idx()];
        let file = &self.files[def.text.file as usize].text;
        let (from, to) = (def.text.start as usize, def.text.end as usize);
        let start = file[..from].rfind('\n').map_or(0, |n| n + 1);
        let last = from + file[from..to].trim_end().len();
        let end = file[last..].find('\n').map_or(file.len(), |n| last + n);
        let text = &file[start..end];
        let total = text.matches('\n').count() + 1;
        let lines = Known {
            it: text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)),
            left: total,
        };
        let lines = api::page(lines, page)?.map(str::to_owned);
        // `api::page` checked the cursor
        let offset = page
            .cursor
            .as_deref()
            .map_or(0, |c| c.trim().parse::<usize>().unwrap_or(0));
        let mut caveats: Vec<String> = self.dup_def_caveat(id).into_iter().collect();
        caveats.extend(self.def_alter_caveats(id));
        let loc = LocIdx {
            file: def.text.file,
            line: def.text_line,
        };
        if self
            .diags
            .iter()
            .any(|d| d.code == "missing_ends" && d.loc == Some(loc))
        {
            caveats.push(
                "The definition has no `.ends` (diagnostic `missing_ends`): the text goes to the `.end` line or to the end of the file".to_owned(),
            );
        }
        let includes = text
            .split('\n')
            .filter(|l| {
                let l = l.trim_start().as_bytes();
                [&b".inc"[..], b".lib"]
                    .iter()
                    .any(|k| l.len() >= k.len() && l[..k.len()].eq_ignore_ascii_case(k))
            })
            .count();
        if includes > 0 {
            caveats.push(format!(
                "The text has {includes} `.include` or `.lib` line{}: this answer does not show the text of those files. `subckt_info` gives the elements that they add",
                if includes == 1 { "" } else { "s" }
            ));
        }
        Ok(SubcktText {
            name: self.def_display(id),
            loc: self.loc(loc),
            first_line: def
                .text_line
                .saturating_add(u32::try_from(offset).unwrap_or(u32::MAX)),
            lines,
            caveats,
        })
    }

    /// The devices of `elements` by type, most first.
    fn devices_by_type(&self, id: DefId, elements: &[(usize, &Element)]) -> Vec<DeviceGroup> {
        const MAX_MODELS: usize = 50;
        // (type, count, models, models seen)
        let mut groups: Vec<(&'static str, usize, Vec<Sym>, HashSet<Sym>)> = Vec::new();
        for &(i, e) in elements
            .iter()
            .filter(|(_, e)| e.kind != ElemKind::Instance)
        {
            let name = e.kind.name();
            let g = match groups.iter().position(|g| g.0 == name) {
                Some(g) => g,
                None => {
                    groups.push((name, 0, Vec::new(), HashSet::new()));
                    groups.len() - 1
                }
            };
            let g = &mut groups[g];
            g.1 += 1;
            if let Some(m) = self.elem_model(id, i)
                && g.3.insert(m)
            {
                g.2.push(m);
            }
        }
        groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        groups
            .into_iter()
            .map(|(name, count, models, seen)| DeviceGroup {
                r#type: name.to_owned(),
                count,
                models: models
                    .iter()
                    .take(MAX_MODELS)
                    .map(|&m| self.syms.name(m).to_owned())
                    .collect(),
                models_total: seen.len(),
            })
            .collect()
    }

    /// The `X` lines of definition `id` by the subckt name they call, in
    /// the order of the first line (at most [`MAX_LIMIT`]), and the number
    /// of names.
    fn children(&self, id: DefId, def: &Def) -> (Vec<ChildCall>, usize) {
        // (callee, x_lines, bound definitions, unresolved)
        let mut groups: Vec<(Sym, usize, Vec<DefId>, bool)> = Vec::new();
        let mut index: HashMap<Sym, usize> = HashMap::new();
        // a stopped analysis left nodes without edges: no bindings then
        let nodes: Vec<&Node> = if self.hier.too_large {
            Vec::new()
        } else {
            self.hier.binding_nodes(id).map(|(_, n)| n).collect()
        };
        let mut line = 0;
        for e in def
            .elements
            .iter()
            .filter(|e| e.alter.is_none() && e.kind == ElemKind::Instance)
        {
            let Some(callee) = e.callee else { continue };
            let g = *index.entry(callee).or_insert_with(|| {
                groups.push((callee, 0, Vec::new(), false));
                groups.len() - 1
            });
            let g = &mut groups[g];
            g.1 += 1;
            for node in &nodes {
                let edge = self.hier.edges[node.edges.start as usize + line];
                if edge.target == NONE {
                    g.3 = true;
                } else {
                    let t = self.hier.nodes[edge.target as usize].def;
                    if !g.2.contains(&t) {
                        g.2.push(t);
                    }
                }
            }
            line += 1;
        }
        let total = groups.len();
        let analysed = !self.hier.too_large;
        let children = groups
            .into_iter()
            .take(MAX_LIMIT)
            .map(|(callee, x_lines, mut defs, unresolved)| {
                defs.sort_unstable();
                ChildCall {
                    subckt: self.syms.name(callee).to_owned(),
                    x_lines,
                    binds_to: analysed
                        .then(|| defs.into_iter().map(|d| self.def_display(d)).collect()),
                    unresolved: analysed.then_some(unresolved),
                }
            })
            .collect();
        (children, total)
    }

    /// The `X` lines that call the definition `name` (the rule of
    /// [`Netlist::subckt_info`]) on some instance path from the root, or
    /// in a definition that the root does not reach, as if that definition
    /// were instantiated at the top level (0 copies): the definition that
    /// holds each, its overrides, its `m`, and the instances it makes.
    pub fn instances_of(&self, name: &str, page: &Page) -> Result<Paged<InstanceRef>, QueryError> {
        let id = self.subckt_by_name(name)?;
        if self.hier.too_large {
            return Err(QueryError::BadArgument(
                "the hierarchy was not analysed (diagnostic `hierarchy_too_large`), so the `X` lines of a definition are not known".to_owned(),
            ));
        }
        let mut lines: HashMap<(DefId, u32), Copies> = HashMap::new();
        for (n, node) in self.hier.nodes.iter().enumerate() {
            if !self.hier.binding_node(n as u32) {
                continue;
            }
            for e in &self.hier.edges[node.edges.range()] {
                if e.target != NONE && self.hier.nodes[e.target as usize].def == id {
                    lines
                        .entry((node.def, e.elem))
                        .and_modify(|c| *c = c.add(node.copies))
                        .or_insert(node.copies);
                }
            }
        }
        let mut lines: Vec<((DefId, u32), Copies)> = lines.into_iter().collect();
        lines.sort_unstable_by_key(|l| l.0);
        let m = self.syms.get("m");
        // a duplicate `.subckt`: the lines and copies come from the first
        // definition of the called subckt, and of the subckt that holds
        // the line
        let dup_caveat = |d: DefId| {
            format!(
                "HSPICE rejects this deck: subckt `{}` is defined more than once (golden `dup_subckt`). The `X` lines and copies come from its first definition",
                self.def_display(d)
            )
        };
        Ok(
            api::page(lines.into_iter(), page)?.map(|((def, elem), copies)| {
                let d = &self.defs[def.idx()];
                let e = &d.elements[elem as usize];
                let params = d.element_params(e);
                let mut dups = vec![id, def];
                dups.dedup();
                let mut caveats: Vec<String> = dups
                    .into_iter()
                    .filter(|&x| self.defs[x.idx()].has_dup)
                    .map(dup_caveat)
                    .collect();
                if let Some(text) = m.and_then(|m| self.m_not_positive(d, e, m)) {
                    caveats.push(format!(
                        "`m={text}`: HSPICE rejects an `X` line whose multiplier is zero or less on an instance path (diagnostic `m_not_positive`). The copies still count its instances"
                    ));
                }
                if self.hier.dup_element(def, e.name) {
                    caveats.push(format!(
                        "`{}` names more than one line of `{}` (diagnostic `duplicate_element`): HSPICE rejects the deck when an instance path reaches it",
                        self.syms.name(e.name),
                        self.def_display(def)
                    ));
                }
                caveats.extend(self.def_alter_caveats(def));
                caveats.extend(self.elem_alter_caveats(def, elem as usize));
                let overrides = || params.iter().filter(|p| Some(p.name) != m);
                InstanceRef {
                    parent: self.def_display(def),
                    instance: self.syms.name(e.name).to_owned(),
                    loc: self.loc(e.loc),
                    overrides: overrides()
                        .take(MAX_LIMIT)
                        .map(|p| self.name_text(p))
                        .collect(),
                    overrides_total: overrides().count(),
                    m: params
                        .iter()
                        .find(|p| Some(p.name) == m)
                        .map(|p| self.name_text(p).text),
                    copies: copies.count(),
                    caveats,
                }
            }),
        )
    }
}

#[cfg(test)]
mod tests;
