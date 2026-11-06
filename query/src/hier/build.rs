//! The build of the hierarchy: the walk of the definitions, the contexts, and the copies.
use super::*;

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
pub(super) struct Build<'n> {
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

/// [`analyze`] with at most `budget` nodes, edges, context entries and
/// lookups in long contexts (default: 4 × (definitions + `X` lines) + 4M).
/// Over the budget, the analysis stops: only nested definitions used in
/// very many combinations along the instance paths get there.
pub(super) fn analyze_with(net: &Netlist, budget: Option<usize>) -> (Hier, Vec<Diag>) {
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
