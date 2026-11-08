//! The search for a parameter: its scopes, its definition, and its value.
use super::*;

/// The lookups in one long list in one call: a count of linear scans, then
/// an index (the last first-run item of each name).
pub(super) enum Scan {
    Count(u32),
    Index(HashMap<Sym, u32>),
}

/// A list of definitions of names that [`Resolver::last_of`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum List {
    /// The `.param` lines of a definition.
    Params(DefId),
    /// The defaults on the `.subckt` line of a definition.
    Defaults(DefId),
    /// The parameters of an `X` line (the definition that holds it, its
    /// index there).
    Overrides(DefId, ElemIdx),
}

/// A definition with at least this many `.param` functions gets an index
/// of them by name at the first lookup in a call.
const SHORT_FUNCS: usize = 16;

/// A list with at least this many items (`.param` lines, `.subckt`
/// defaults, `X`-line overrides) gets an index after [`SCANS`] linear
/// scans in one call, so that a call that looks up many names costs time
/// linear in the size of the deck.
const BIG_SCOPE: usize = 256;

const SCANS: u32 = 16;

impl<'n> Resolver<'n> {
    pub(super) fn new(net: &'n Netlist, scopes: Vec<Scope>) -> Self {
        Resolver {
            net,
            scopes,
            nodes: Vec::new(),
            memo: HashMap::new(),
            by_def: HashMap::new(),
            by_call: HashMap::new(),
            stack: Vec::new(),
            cycled: false,
            closed: None,
            calls: Vec::new(),
            frames: Vec::new(),
            height: 0,
            evals: 0,
            limited: 0,
            temp: None,
            m: net.syms.get("m"),
            scans: RefCell::new(HashMap::new()),
            alter_params: None,
            sticky: RefCell::new(HashMap::new()),
            sticky_visits: Cell::new(0),
            definers: RefCell::new(None),
            found: HashMap::new(),
            notes: Vec::new(),
            func_index: HashMap::new(),
            work: Cell::new(0),
            reached: RefCell::new((0, Vec::new())),
        }
    }

    /// Counts `n` steps of work (see [`Resolver::work`]).
    fn step(&self, n: usize) {
        self.work.set(self.work.get().saturating_add(n as u64));
    }

    /// The last first-run `.param` of `n` in the body of `def`.
    fn local_param(&self, def: DefId, n: Sym) -> Option<&'n ParamDef> {
        let params = &self.net.defs[def.idx()].params;
        self.last_of(List::Params(def), params, n)
    }

    /// The last first-run item named `n` of `list` (the list `key`): a
    /// linear scan, or, for a long list scanned [`SCANS`] times in this
    /// call, an index.
    fn last_of(&self, key: List, list: &'n [ParamDef], n: Sym) -> Option<&'n ParamDef> {
        let scan = || {
            self.step(list.len());
            list.iter().rfind(|p| p.name == n && p.alter.is_none())
        };
        if list.len() < BIG_SCOPE {
            return scan();
        }
        let mut scans = self.scans.borrow_mut();
        let entry = scans.entry(key).or_insert(Scan::Count(0));
        if let Scan::Count(c) = entry {
            *c += 1;
            if *c <= SCANS {
                return scan();
            }
            // later items replace earlier ones: the last one wins
            self.step(list.len());
            let index = list
                .iter()
                .enumerate()
                .filter(|(_, p)| p.alter.is_none())
                .map(|(i, p)| (p.name, u32::try_from(i).expect("fewer than 2^32 items")))
                .collect();
            *entry = Scan::Index(index);
        }
        match entry {
            Scan::Index(index) => index.get(&n).map(|&i| &list[i as usize]),
            Scan::Count(_) => unreachable!("indexed above"),
        }
    }

    /// Adds the node of `step`, whose text's evaluation collected `frame`.
    fn node(&mut self, step: ChainStep, frame: Frame) -> u32 {
        let id = u32::try_from(self.nodes.len()).expect("fewer than 2^32 nodes");
        let quiet =
            frame.caveats.is_empty() && frame.deps.iter().all(|&d| self.nodes[d as usize].quiet);
        self.nodes.push(Node {
            step,
            caveats: frame.caveats,
            deps: frame.deps,
            quiet,
        });
        id
    }

    /// The answer for `name`: its value, the chain (`root`, then the nodes
    /// `frame.deps` use, depth first, each once, at most [`MAX_LIMIT`]
    /// steps) and the caveats (those of `frame`, then those of the nodes).
    pub(super) fn answer(
        &self,
        name: String,
        value: Result<f64, String>,
        root: Option<ChainStep>,
        frame: &Frame,
    ) -> Resolved {
        let mut chain: Vec<ChainStep> = root.into_iter().collect();
        let mut caveats: Vec<&str> = Vec::new();
        let mut known: HashSet<&str> = HashSet::new();
        for c in &frame.caveats {
            if known.insert(c) {
                caveats.push(c);
            }
        }
        let mut truncated = false;
        let mut seen: HashSet<u32> = HashSet::new();
        let mut todo: Vec<u32> = frame.deps.iter().rev().copied().collect();
        while let Some(id) = todo.pop() {
            if !seen.insert(id) {
                continue;
            }
            let node = &self.nodes[id as usize];
            if chain.len() < MAX_LIMIT {
                chain.push(node.step.clone());
            } else {
                truncated = true;
            }
            for c in &node.caveats {
                if known.insert(c) {
                    caveats.push(c);
                }
            }
            todo.extend(node.deps.iter().rev());
        }
        let mut caveats: Vec<String> = caveats.into_iter().map(str::to_owned).collect();
        if truncated {
            caveats.push(format!(
                "the chain has more than {MAX_LIMIT} steps: only the first {MAX_LIMIT} are listed"
            ));
        }
        let (value, unresolved) = match value {
            Ok(v) if v.is_finite() => (Some(v), None),
            Ok(_) => (None, Some(OUT_OF_RANGE.to_owned())),
            Err(why) => (None, Some(why)),
        };
        Resolved {
            name,
            value,
            unresolved,
            chain,
            caveats,
        }
    }

    /// The definition of `n` at level `j` alone: `I`, then `L`, then `D`;
    /// the last one of each kind.
    fn at_level(&self, j: usize, n: Sym) -> Option<Cand<'n>> {
        let net = self.net;
        let s = &self.scopes[j];
        if let Some((holder, i)) = s.x
            && Some(n) != self.m
        {
            let d = &net.defs[holder.idx()];
            let list = d.element_params(&d.elements[i]);
            if let Some(p) = self.last_of(List::Overrides(holder, i), list, n) {
                // evaluated in the new instance's scope (golden
                // `ovr_scope_local`, `ovr_scope_child_global`)
                return Some(Cand {
                    kind: StepKind::InstanceOverride,
                    level: j,
                    p,
                    eval_level: j,
                });
            }
        }
        let d = &net.defs[s.def.idx()];
        if let Some(p) = self.local_param(s.def, n) {
            let kind = if s.def == ROOT {
                StepKind::GlobalParam
            } else {
                StepKind::LocalParam
            };
            return Some(Cand {
                kind,
                level: j,
                p,
                eval_level: j,
            });
        }
        let defaults = &d.defaults;
        self.last_of(List::Defaults(s.def), defaults, n)
            .map(|p| Cand {
                kind: StepKind::SubcktDefault,
                level: j,
                p,
                eval_level: j,
            })
    }

    /// The definition of `n` that wins at level `k`: GLOBAL tries levels
    /// `0..=k`, LOCAL `k..=0`.
    pub(super) fn find(&self, k: usize, n: Sym) -> Option<Cand<'n>> {
        match self.net.parhier {
            Parhier::Global => (0..=k).find_map(|j| self.at_level(j, n)),
            Parhier::Local => (0..=k).rev().find_map(|j| self.at_level(j, n)),
        }
    }

    /// The value of `n` at level `k`.
    pub(super) fn resolve(&mut self, k: usize, n: Sym) -> Val {
        self.step(1);
        if let Some(v) = self.memo.get(&(k, n)) {
            return v.clone();
        }
        let net = self.net;
        let name = net.syms.name(n);
        let Some(c) = self.find(k, n) else {
            let v = Val {
                value: Err(undefined(net, name)),
                node: None,
            };
            self.memo.insert((k, n), v.clone());
            return v;
        };
        let key = (c.p as *const ParamDef, c.eval_level);
        if let Some(v) = self.by_def.get(&key) {
            let v = v.clone();
            self.memo.insert((k, n), v.clone());
            return v;
        }
        if let Some(at) = self
            .stack
            .iter()
            .position(|&(p, l, ..)| (p, l) == (key.0, key.1))
        {
            self.cycled = true;
            // src/eval.rs computes the values passed on a line in the order
            // of the line, so the cycle closes at the first one on the line
            // (the overrides at one level are of one `X` line, and they are
            // in line order in memory)
            let cycle = &self.stack[at..];
            self.closed = cycle.iter().find(|e| e.3).map(|&(_, l, ..)| {
                let first = cycle.iter().filter(|e| e.3 && e.1 == l).map(|e| e.0);
                (first.min().expect("one override"), l)
            });
            let names: Vec<&str> = self.stack[at..]
                .iter()
                .map(|&(_, _, s, _)| net.syms.name(s))
                .chain([name])
                .collect();
            // not memoized: each name in the cycle gets this reason
            return Val {
                value: Err(format!(
                    "cycle: {} (each value needs the next one)",
                    names.join(" -> ")
                )),
                node: None,
            };
        }
        let element = c.kind == StepKind::InstanceOverride;
        self.stack.push((key.0, key.1, n, element));
        let text = match c.kind {
            StepKind::InstanceOverride => Text::Override(n),
            StepKind::LocalParam | StepKind::SubcktDefault => Text::Default(n),
            _ => Text::Body,
        };
        let limited = self.limited;
        let outer_cycled = std::mem::take(&mut self.cycled);
        let (mut value, mut frame) = self.eval_param(c.p, c.eval_level, element, text);
        // A value passed whose text leads back to it through other names
        // (`a='c*k'` with `.param c='a*3'`) is computed, as a whole, one level
        // up. This is the measured rule (decks `q_cycle_pass_*`,
        // `q_cycle_text_*`), as src/eval.rs reads it.
        if value.is_err() && self.closed == Some(key) && c.eval_level > 0 {
            self.closed = None;
            (value, frame) = self.eval_param(c.p, c.eval_level - 1, element, Text::Body);
        }
        let cycled = self.cycled;
        self.cycled |= outer_cycled;
        self.stack.pop();
        // under LOCAL a definition below the top level wins in every run
        // over a top-level `.param` of an `.alter` block
        let alters = if net.parhier == Parhier::Local && c.level > 0 {
            Vec::new()
        } else {
            self.altered(n)
        };
        for (a, at) in alters {
            frame.caveats.push(format!(
                "`{name}` is changed by .alter #{a} at {at}: this is the first run's value (golden `alter`: the changes of the `.alter` blocks accumulate over the runs)"
            ));
        }
        // a sweep changes the top-level `.param`
        if c.kind == StepKind::GlobalParam {
            frame.caveats.extend(net.swept_caveats(n));
        }
        let step = ChainStep {
            kind: c.kind,
            scope: self.scopes[c.level].path.clone(),
            name: name.to_owned(),
            text: param_text(net, c.p),
            value: value.as_ref().ok().copied(),
            loc: Some(net.loc(c.p.loc)),
        };
        let v = Val {
            value,
            node: Some(self.node(step, frame)),
        };
        if self.limited == limited && (v.value.is_ok() || !cycled) {
            self.by_def.insert(key, v.clone());
            self.memo.insert((k, n), v.clone());
        }
        v
    }

    /// The `.alter` blocks that define the parameter `n` again (`.alter`
    /// lines are at the top level): the block number and `file:line`.
    fn altered(&mut self, n: Sym) -> Vec<(u32, String)> {
        let net = self.net;
        if net.alters.is_empty() {
            return Vec::new();
        }
        let by_name = self.alter_params.get_or_insert_with(|| {
            let mut m: HashMap<Sym, Vec<(u32, LocIdx)>> = HashMap::new();
            for p in &net.defs[ROOT.idx()].params {
                if let Some(a) = p.alter {
                    m.entry(p.name).or_default().push((a, p.loc));
                }
            }
            m
        });
        by_name
            .get(&n)
            .into_iter()
            .flatten()
            .map(|&(a, loc)| (a, loc_str(net, loc)))
            .collect()
    }

    /// The value of the parameter `p`, evaluated at level `level`.
    /// `element`: an expression of an element line. `text`: see [`Text`].
    pub(super) fn eval_param(
        &mut self,
        p: &ParamDef,
        level: usize,
        element: bool,
        text: Text,
    ) -> (Result<f64, String>, Frame) {
        if let Some(x) = p.num {
            // as `netlist_expr` does for a number in an expression
            let v = if x.is_finite() {
                Ok(x)
            } else {
                Err(format!(
                    "the number `{}` is out of range",
                    param_text(self.net, p)
                ))
            };
            return (v, Frame::default());
        }
        match self.net.param_text(p) {
            Some(s) => self.eval_text(&s, level, &[], element, text),
            None => (Err("no value".to_owned()), Frame::default()),
        }
    }

    /// The value of the expression `s` at level `level`, with the
    /// arguments `args` of a function bound (lowercase names). `text`: see
    /// [`Text`].
    fn eval_text(
        &mut self,
        s: &str,
        level: usize,
        args: &[(String, f64)],
        element: bool,
        text: Text,
    ) -> (Result<f64, String>, Frame) {
        let fail = |why: String| (Err(why), Frame::default());
        if s.contains(['{', '}']) {
            return fail(BRACES.to_owned());
        }
        match quote_fault(s) {
            Some(QuoteFault::CommentLine | QuoteFault::ParenCommentLine) => {
                return fail(COMMENT_LINE.to_owned());
            }
            Some(QuoteFault::Dollar) => return fail(DOLLAR.to_owned()),
            None => {}
        }
        self.evals += 1;
        self.step(1);
        if self.evals > MAX_EVALS {
            self.limited += 1;
            return fail(TOO_MUCH.to_owned());
        }
        let e = match netlist_expr::parse(s) {
            Ok(e) => e,
            Err(err) => {
                return fail(format!("cannot read the expression `{}`: {err}", quote(s)));
            }
        };
        // a `.param` function hides a built-in of its name (golden
        // `func_shadow_top`, `stat_user_func`)
        if element && let Some(f) = statistical(&e, &mut |n| self.user_func(level, n, text)) {
            return fail(format!(
                "HSPICE rejects `{f}()` directly in an element expression (golden `stat_direct`, `stat_in_mos`: \"cannot be defined in expressions directly\"): it belongs in a `.param`"
            ));
        }
        // the arguments of a call that is neither a built-in nor a `.param`
        // function are not parameters: `v(out)`, `i(v1)` (golden
        // `rcl_output_var`)
        if let Some(name) = self.unknown_call(&e, level, text) {
            return fail(not_visible(&name));
        }
        let h = height(&e);
        if self.stack.len() + self.calls.len() >= MAX_DEPTH || self.height + h > MAX_HEIGHT {
            self.limited += 1;
            return fail(TOO_DEEP.to_owned());
        }
        self.height += h;
        self.frames.push(Frame::default());
        let r = netlist_expr::eval(
            &e,
            &mut Cx {
                r: self,
                level,
                args,
                text,
            },
        );
        let mut frame = self.frames.pop().expect("pushed above");
        self.height -= h;
        match r {
            Ok((v, cs)) => {
                for c in cs {
                    let text = self.caveat_text(&c);
                    frame.caveats.push(text);
                }
                (Ok(v), frame)
            }
            // `1e308 * 1e308`: the evaluator writes each number in full
            Err(EvalError::Domain(msg)) => (Err(short_numbers(&msg)), frame),
            Err(err) => (Err(err.to_string()), frame),
        }
    }

    /// The caveat of an evaluator note.
    fn caveat_text(&mut self, c: &Caveat) -> String {
        match c {
            Caveat::StatisticalNominal(f) => format!(
                "`{f}` gives its nominal value, as HSPICE's `.op` without Monte Carlo does (golden `stat_nominal`)"
            ),
            Caveat::Temper => {
                let t = self.temperature();
                let source = match (
                    self.net.temps.iter().find(|p| p.alter.is_none()),
                    self.tnom_option(),
                ) {
                    (Some(p), _) => {
                        format!("the first `.temp` value, at {}", loc_str(self.net, p.loc))
                    }
                    (None, Some(p)) => format!(
                        "the first run has no `.temp` line, so HSPICE uses `.option tnom`, at {}",
                        loc_str(self.net, p.loc)
                    ),
                    (None, None) => {
                        "HSPICE's default: the first run has no `.temp` line".to_owned()
                    }
                };
                match t.value {
                    Ok(v) => {
                        format!("uses `temper` = {v} ({source}; golden `temper`, `temper_default`)")
                    }
                    Err(_) => "uses `temper`, whose value is not resolved".to_owned(),
                }
            }
        }
    }

    /// The last first-run `.param` function `sym` of definition `def`
    /// (golden `func_redef`): a linear scan of a short list, else an index
    /// of the list built once in this call.
    fn func_of(&mut self, def: DefId, sym: Sym) -> Option<&'n FuncDef> {
        let funcs: &'n [FuncDef] = &self.net.defs[def.idx()].funcs;
        if funcs.len() < SHORT_FUNCS {
            self.step(funcs.len());
            return funcs.iter().rfind(|f| f.name == sym && f.alter.is_none());
        }
        self.step(1);
        let i = self.func_entry(def).get(&sym)?.last?;
        Some(&funcs[i as usize])
    }

    /// The `.param` functions of `def` by name (see [`Resolver::func_index`]).
    fn func_entry(&mut self, def: DefId) -> &HashMap<Sym, FuncEntry> {
        let funcs: &'n [FuncDef] = &self.net.defs[def.idx()].funcs;
        let work = &self.work;
        self.func_index.entry(def).or_insert_with(|| {
            work.set(work.get().saturating_add(funcs.len() as u64));
            let mut index: HashMap<Sym, FuncEntry> = HashMap::new();
            for (i, f) in funcs.iter().enumerate() {
                let i = u32::try_from(i).expect("fewer than 2^32 functions");
                let entry = index.entry(f.name).or_default();
                match f.alter {
                    None => entry.last = Some(i),
                    Some(_) => entry.alters.push(i),
                }
            }
            index
        })
    }

    /// The `.alter` blocks that define the function `sym` of `def` again.
    fn func_alters(&mut self, def: DefId, sym: Sym) -> Vec<&'n FuncDef> {
        let funcs: &'n [FuncDef] = &self.net.defs[def.idx()].funcs;
        if self.net.alters.is_empty() {
            return Vec::new();
        }
        if funcs.len() < SHORT_FUNCS {
            self.step(funcs.len());
            return funcs
                .iter()
                .filter(|g| g.name == sym && g.alter.is_some())
                .collect();
        }
        self.step(1);
        let alters = self
            .func_entry(def)
            .get(&sym)
            .map(|e| e.alters.clone())
            .unwrap_or_default();
        alters.into_iter().map(|i| &funcs[i as usize]).collect()
    }

    /// The note `text`: its id in [`Resolver::notes`].
    fn note(&mut self, text: String) -> u32 {
        let id = u32::try_from(self.notes.len()).expect("fewer than 2^32 notes");
        self.notes.push(text);
        id
    }

    /// What a call of the function `sym` at level `level`, in a text of
    /// kind `text`, uses, and a caveat on the choice (a note). Memoized for
    /// the call by (level, name, text).
    ///
    /// GLOBAL: the top level first, then the instances down the path, as
    /// for a parameter (golden `func_scope`: a subckt's own `f` loses to the
    /// top-level `f`; `func_two_levels_global`: the outer of two instances
    /// wins). LOCAL: the nearest definition up the instance path (the level
    /// of the call first), then the top level (golden `ovr_self_combo`,
    /// `func_three_local`, `func_levels_local`, `func_first_inst_local`). A
    /// function defined only in an instance above is visible below it in
    /// both modes. In one scope the last definition wins (golden
    /// `func_redef`).
    ///
    /// The creation-order rule: under LOCAL, HSPICE keeps the top-level `f`
    /// for a text once an instance that it creates before this one
    /// evaluated the same text and found no `f` above it (golden
    /// `func_scope_local`, where the top-level `xt` of `leaf` comes before
    /// `x1.xl`, `func_sticky_local`, `func_sticky_more_local`,
    /// `func_sticky_ovr_local`; see [`Resolver::sticky`]).
    ///
    /// A name of a built-in function (`max`, `sgn`, ...): a visible
    /// `.param` function of that name hides the built-in, and the built-in
    /// is used where none is visible (golden `func_shadow_top`,
    /// `func_shadow_sub`, `func_shadow_scope`). With no top-level `.param`
    /// function of the name, the instance that HSPICE creates first among
    /// those that evaluate the text decides for the later ones, in both
    /// modes: when it finds no `.param` function above it, the later ones
    /// use the built-in too (golden `func_shadow_scope`: x2.xk.rk = 1k);
    /// when it finds one, a later one that finds none makes HSPICE reject
    /// the deck (golden `func_shadow_more`, `func_shadow_more_local`).
    ///
    /// A call in a function body ([`Text::Func`]) takes the nearest
    /// definition (the GLOBAL order under GLOBAL), or the built-in, with no
    /// creation-order rule (golden `sticky_lazy_local`: x5.xc.rh = 3k,
    /// x7.xc.rk = 3k, where the first instance of the subckt that defines
    /// the function found the top-level `f`).
    /// A `.param` function `name` is visible at `level` for `text`.
    fn user_func(&mut self, level: usize, name: &str, text: Text) -> bool {
        self.net
            .syms
            .get(name)
            .is_some_and(|s| matches!(self.find_func(level, s, text).0, Found::User(..)))
    }

    /// The first call in `e` of a name that is neither a built-in nor a
    /// `.param` function visible at `level`.
    fn unknown_call(&mut self, e: &Expr, level: usize, text: Text) -> Option<String> {
        match e {
            Expr::Num(_) | Expr::Ident(_) => None,
            Expr::Call { name, args } => {
                if !netlist_expr::is_builtin(name) && !self.user_func(level, name, text) {
                    return Some(name.clone());
                }
                args.iter().find_map(|a| self.unknown_call(a, level, text))
            }
            Expr::Unary { e, .. } => self.unknown_call(e, level, text),
            Expr::Binary { l, r, .. } => self
                .unknown_call(l, level, text)
                .or_else(|| self.unknown_call(r, level, text)),
            Expr::Ternary { c, t, f } => self
                .unknown_call(c, level, text)
                .or_else(|| self.unknown_call(t, level, text))
                .or_else(|| self.unknown_call(f, level, text)),
        }
    }

    fn find_func(&mut self, level: usize, sym: Sym, text: Text) -> (Found<'n>, Option<u32>) {
        self.step(1);
        if let Some(&found) = self.found.get(&(level, sym, text)) {
            return found;
        }
        let found = self.find_func_here(level, sym, text);
        self.found.insert((level, sym, text), found);
        found
    }

    /// [`Resolver::find_func`], not memoized.
    fn find_func_here(&mut self, level: usize, sym: Sym, text: Text) -> (Found<'n>, Option<u32>) {
        let net = self.net;
        let name = net.syms.name(sym);
        let builtin = netlist_expr::is_builtin(name);
        let near = match net.parhier {
            Parhier::Global => {
                (0..=level).find_map(|j| self.func_of(self.scopes[j].def, sym).map(|f| (j, f)))
            }
            Parhier::Local => (0..=level)
                .rev()
                .find_map(|j| self.func_of(self.scopes[j].def, sym).map(|f| (j, f))),
        };
        let top = self.func_of(self.scopes[0].def, sym);
        if text == Text::Func || !builtin || top.is_some() {
            let Some((j, f)) = near else {
                if builtin {
                    return (Found::Builtin, None);
                }
                let why = self.note(not_visible(name));
                return (Found::Nothing(why), None);
            };
            if text == Text::Func || net.parhier == Parhier::Global || j == 0 || j == level {
                return (Found::User(j, f), None);
            }
            let Some(top) = top else {
                return (Found::User(j, f), None);
            };
            let golden = "golden `func_scope_local`, `func_sticky_local`, `func_sticky_ovr_local`";
            let near_path = self.scopes[j].path.clone();
            let what = self.what(level, text);
            let set = self.definers(sym);
            return match self.sticky(level, set, text) {
                Sticky::No => (Found::User(j, f), None),
                Sticky::Top(first) => {
                    let why = self.note(format!(
                        "under parhier=local the nearest `{name}` is the one of `{near_path}`, but `{}`, an instance {what} that HSPICE creates before this one (breadth-first), evaluates the same text, finds no `{name}` above it and uses the top-level `{name}`; HSPICE then keeps the top-level `{name}` for this text in the later instances ({golden})",
                        first.path
                    ));
                    (Found::User(0, top), Some(why))
                }
                Sticky::Unknown(why) => {
                    let why = self.note(format!(
                        "under parhier=local HSPICE uses the top-level `{name}` instead of the one of `{near_path}` when an instance {what} that it creates before this one evaluates the same text and finds no `{name}` above it ({golden}); this was not checked: {why}"
                    ));
                    (Found::User(j, f), Some(why))
                }
            };
        }
        self.builtin_name(level, sym, text, near)
    }

    /// [`Resolver::find_func`] for the name `sym` of a built-in function
    /// with no top-level `.param` function, in a text other than a function
    /// body, where `near` is the `.param` function that the instance of
    /// `level` finds (with its level).
    fn builtin_name(
        &mut self,
        level: usize,
        sym: Sym,
        text: Text,
        near: Option<(usize, &'n FuncDef)>,
    ) -> (Found<'n>, Option<u32>) {
        let name = self.net.syms.name(sym);
        let set = self.definers(sym);
        if set == 0 {
            // no `.param` function of the name anywhere
            return (Found::Builtin, None);
        }
        if let Some((j, f)) = near
            && j == level
        {
            // every instance of the definition of the level finds it
            return (Found::User(j, f), None);
        }
        let golden = "golden `func_shadow_scope`, `func_shadow_more`, `func_shadow_more_local`";
        let what = self.what(level, text);
        // the first instance created before this one that evaluates the
        // text, and the first one with no `.param` function above it
        let any = self.sticky(level, 0, text);
        let none = match (&any, near) {
            (Sticky::No, None) => return (Found::Builtin, None),
            (Sticky::Unknown(why), _) => {
                let why = *why;
                let found = near.map_or(Found::Builtin, |(j, f)| Found::User(j, f));
                let used = if near.is_some() {
                    format!("the `.param` function `{name}`")
                } else {
                    format!("the built-in `{name}`")
                };
                let note = self.note(format!(
                    "`{name}` is also a built-in function: HSPICE keeps the kind of function (`.param` or built-in) that the instance {what} that it creates first uses for this text ({golden}); this answer uses {used}, and the order was not checked: {why}"
                ));
                return (found, Some(note));
            }
            _ => self.sticky(level, set, text),
        };
        let first_any = match &any {
            Sticky::Top(f) => Some(Rc::clone(f)),
            _ => None,
        };
        let first_none = match &none {
            Sticky::Top(f) => Some(Rc::clone(f)),
            Sticky::Unknown(why) => {
                let why = *why;
                let found = near.map_or(Found::Builtin, |(j, f)| Found::User(j, f));
                let note = self.note(format!(
                    "`{name}` is also a built-in function: HSPICE keeps the kind of function (`.param` or built-in) that the instance {what} that it creates first uses for this text ({golden}); the order was not checked: {why}"
                ));
                return (found, Some(note));
            }
            Sticky::No => None,
        };
        // the instance created first used the built-in
        let first_builtin = match (&first_any, &first_none) {
            (Some(a), Some(n)) => a.lines == n.lines,
            _ => false,
        };
        match (near, first_any) {
            (Some((j, f)), _) if first_none.is_none() => {
                // the instances up to this one use a `.param` function: a
                // later one that finds none makes HSPICE reject the deck
                // (golden `func_shadow_more`: x8.xk.rk2 after x7.xk.rk2)
                let note = match self.sticky_later(level, set, text) {
                    Sticky::No => None,
                    Sticky::Top(later) => Some(self.note(format!(
                        "HSPICE rejects this deck: the instances {what} that HSPICE creates up to this one use a `.param` function `{name}` for this text, and HSPICE keeps that kind of function for the later instances, but `{}`, created after this one, finds no `.param` function `{name}` above it ({golden}: \"Undefined parameter or function definition\" for the later instance); this answer is for this instance, which uses the `.param` function",
                        later.path
                    ))),
                    Sticky::Unknown(why) => Some(self.note(format!(
                        "`{name}` is also a built-in function: HSPICE rejects the deck when an instance {what} that it creates after this one finds no `.param` function `{name}` above it ({golden}); this was not checked: {why}"
                    ))),
                };
                (Found::User(j, f), note)
            }
            (Some((j, _)), Some(first)) if first_builtin => {
                let near_path = self.scopes[j].path.clone();
                let note = self.note(format!(
                    "`{name}` is a built-in function, and the `.param` function `{name}` of `{near_path}` is visible here, but `{}`, an instance {what} that HSPICE creates first (breadth-first), evaluates the same text, finds no `.param` function `{name}` above it and uses the built-in; HSPICE then keeps the built-in for this text in the later instances (golden `func_shadow_scope`: x2.xk.rk)",
                    first.path
                ));
                (Found::Builtin, Some(note))
            }
            (Some(_), first) => {
                let first = first.expect("an instance with none comes after the first one");
                let none = first_none.expect("checked above");
                let why = self.note(format!(
                    "HSPICE rejects this deck: `{}`, an instance {what} that HSPICE creates first (breadth-first), uses a `.param` function `{name}` for this text, and HSPICE keeps that kind of function for the later instances, but `{}`, created after it and before this one, finds no `.param` function `{name}` above it ({golden}: \"Undefined parameter or function definition\")",
                    first.path, none.path
                ));
                (Found::Nothing(why), None)
            }
            (None, Some(_)) if first_builtin => (Found::Builtin, None),
            (None, first) => {
                let first = first.expect("checked above");
                let why = self.note(format!(
                    "no `.param` function `{name}` is visible here, and `{}`, an instance {what} that HSPICE creates before this one (breadth-first), uses a `.param` function `{name}` for this text: HSPICE keeps that kind of function for the later instances and rejects this deck, though `{name}` is also a built-in function ({golden}: \"Undefined parameter or function definition\")",
                    first.path
                ));
                (Found::Nothing(why), None)
            }
        }
    }

    /// The instances that evaluate a text of kind `text` at level `level`,
    /// for a caveat.
    fn what(&self, level: usize, text: Text) -> String {
        let net = self.net;
        let def = net.def_display(self.scopes[level].def);
        match (text, self.scopes[level].x) {
            (Text::Override(_), Some((holder, i))) => format!(
                "made by the same `X` line (`{}` in `{}`)",
                net.syms.name(net.defs[holder.idx()].elements[i].name),
                net.def_display(holder)
            ),
            (Text::Default(n), _) => format!(
                "of `{def}` whose `X` line does not override `{}`",
                net.syms.name(n)
            ),
            _ => format!("of `{def}`"),
        }
    }

    /// Whether an instance that HSPICE creates before the instance of level
    /// `level` evaluates the same text (of kind `text`) and finds no
    /// definition of the function on its instance path (none of the
    /// definitions of set `set`, see [`Definers`]; any instance for the
    /// empty set 0), so that it uses the top-level one (or the built-in),
    /// with that instance. The instances that evaluate the text: for
    /// [`Text::Body`] each instance of the definition of the level, for
    /// [`Text::Default`] each one whose `X` line does not override the
    /// name, for [`Text::Override`] each instance that the `X` line of the
    /// level makes (golden `func_sticky_ovr_local`: a `.subckt` default
    /// that the `X` line of the first instance overrides does not count,
    /// and two `X` lines with the same override text count separately;
    /// golden `sticky_lazy_local`: x1.xl.rp, a body `.param` that the `X`
    /// line of the first instance overrides does not count either, and
    /// x9.xl.ru, a body `.param` that no element of the earlier instance
    /// uses still counts). HSPICE creates the instances breadth-first:
    /// level by level, in a level by the order of the parents, in a parent
    /// by `X`-line order (golden `hier_dotted_collision*`), so an instance
    /// comes first when it is less deep, or as deep with a smaller list of
    /// `X`-line indices. Any such earlier instance counts, not only the
    /// first one (golden `func_sticky_ovr_local`: cf6, then w6, then cf6);
    /// the walk gives the first one. Memoized for the call by (level, the
    /// instances that evaluate the text, set).
    fn sticky(&self, level: usize, set: u32, text: Text) -> Sticky {
        let unit = match text {
            Text::Override(_) => Unit::Override,
            Text::Default(n) => Unit::Default(n),
            Text::Body | Text::Func => Unit::Body,
        };
        let key = (level, unit, set, false);
        self.step(1);
        if let Some(s) = self.sticky.borrow().get(&key) {
            return s.clone();
        }
        let defs = self.definer_set(set);
        let s = match text {
            // the instances that evaluate a default are some of those of
            // `Text::Body`, in the same order: the first of those is the
            // first of these when its `X` line does not override the name
            Text::Default(n) => match self.sticky(level, set, Text::Body) {
                Sticky::Top(first) if self.overrides(first.line().0, first.line().1, n) => {
                    self.sticky_walk(level, text, &defs, false)
                }
                s => s,
            },
            _ => self.sticky_walk(level, text, &defs, false),
        };
        self.sticky.borrow_mut().insert(key, s.clone());
        s
    }

    /// As [`Resolver::sticky`], but for all the instances that evaluate
    /// the text, in creation order: the first one that finds no definition
    /// of set `set` above it. Asked when no instance before the one of
    /// level `level` is such an instance, so the one it gives comes after
    /// it.
    fn sticky_later(&self, level: usize, set: u32, text: Text) -> Sticky {
        let unit = match text {
            Text::Override(_) => Unit::Override,
            Text::Default(n) => Unit::Default(n),
            Text::Body | Text::Func => Unit::Body,
        };
        let key = (level, unit, set, true);
        self.step(1);
        if let Some(s) = self.sticky.borrow().get(&key) {
            return s.clone();
        }
        let s = self.sticky_walk(level, text, &self.definer_set(set), true);
        self.sticky.borrow_mut().insert(key, s.clone());
        s
    }

    /// Whether `X` line `i` of `holder` overrides the name `n`.
    fn overrides(&self, holder: DefId, i: ElemIdx, n: Sym) -> bool {
        let h = &self.net.defs[holder.idx()];
        let list = h.element_params(&h.elements[i]);
        self.last_of(List::Overrides(holder, i), list, n).is_some()
    }

    /// The id of the set of definitions of the function `sym` (see
    /// [`Definers`]), built for all names at the first use in this call.
    fn definers(&self, sym: Sym) -> u32 {
        self.step(1);
        let mut guard = self.definers.borrow_mut();
        let d = guard.get_or_insert_with(|| {
            let mut m: HashMap<Sym, Vec<DefId>> = HashMap::new();
            for (i, d) in self.net.defs.iter().enumerate().skip(ROOT.idx() + 1) {
                let id = DefId(u32::try_from(i).expect("fewer than 2^32 definitions"));
                self.step(d.funcs.len());
                for f in d.funcs.iter().filter(|f| f.alter.is_none()) {
                    let v = m.entry(f.name).or_default();
                    if v.last() != Some(&id) {
                        v.push(id);
                    }
                }
            }
            let empty: Rc<[DefId]> = Rc::from(Vec::new());
            let mut ids: HashMap<Rc<[DefId]>, u32> = HashMap::from([(Rc::clone(&empty), 0)]);
            let mut sets = vec![empty];
            let mut by_name = HashMap::with_capacity(m.len());
            for (name, defs) in m {
                self.step(defs.len());
                let defs: Rc<[DefId]> = Rc::from(defs);
                let id = *ids.entry(Rc::clone(&defs)).or_insert_with(|| {
                    sets.push(defs);
                    u32::try_from(sets.len() - 1).expect("fewer than 2^32 sets")
                });
                by_name.insert(name, id);
            }
            Definers { by_name, sets }
        });
        d.by_name.get(&sym).copied().unwrap_or(0)
    }

    /// The definitions of set `set` (see [`Definers`]).
    fn definer_set(&self, set: u32) -> Rc<[DefId]> {
        match &*self.definers.borrow() {
            Some(d) => Rc::clone(&d.sets[set as usize]),
            None => Rc::from(Vec::new()),
        }
    }

    /// A breadth-first walk of the hierarchy graph from the root, through
    /// the nodes whose definitions are not in `defs` (the definitions of
    /// the function), each node once (the first instance of a node comes
    /// before the other ones, and so do the instances below it), up to the
    /// instance of `level`, or with `later` to the end. Each `X` line it
    /// reads counts against [`MAX_STICKY`] for the call.
    fn sticky_walk(&self, level: usize, text: Text, defs: &[DefId], later: bool) -> Sticky {
        let net = self.net;
        let hier = &net.hier;
        let defines = |d: DefId| defs.binary_search(&d).is_ok();
        let target = self.scopes[level].def;
        let above = &self.scopes[1..=level];
        let Some(line) = self.scopes[level].x else {
            return Sticky::No;
        };
        // a scope above every instance (`top=`) that defines the function
        // hides the top-level one from all of them
        if defines(target) || above.iter().any(|s| s.x.is_none() && defines(s.def)) {
            return Sticky::No;
        }
        if net.root == Root::None {
            return Sticky::No;
        }
        if hier.too_large || hier.nodes.is_empty() {
            return Sticky::Unknown(
                "the hierarchy was not analysed (diagnostic `hierarchy_too_large`)",
            );
        }
        // the `X`-line indices of the instance of `level` from the root
        let seq: Vec<u32> = above
            .iter()
            .filter_map(|s| s.x.map(|(_, i)| i as u32))
            .collect();
        let count = |n: usize| {
            self.step(n);
            let v = self.sticky_visits.get().saturating_add(n);
            self.sticky_visits.set(v);
            v > MAX_STICKY
        };
        // the instance that `X` line `i` of `holder` makes, of definition
        // `d`, evaluates the text
        let evaluates = |holder: DefId, i: ElemIdx, d: DefId| match text {
            Text::Body | Text::Func => Ok(d == target),
            Text::Override(_) => Ok((holder, i) == line),
            Text::Default(n) => {
                if d != target {
                    return Ok(false);
                }
                let h = &net.defs[holder.idx()];
                let len = h.element_params(&h.elements[i]).len();
                if count(len.min(BIG_SCOPE)) {
                    return Err(());
                }
                Ok(!self.overrides(holder, i, n))
            }
        };
        // (graph node, previous step, `X` line, its `X`-line indices
        // against those of the instance of `level`); the root first
        let mut steps: Vec<(u32, u32, u32, Ordering)> = vec![(0, u32::MAX, 0, Ordering::Equal)];
        let mut level_steps: Vec<u32> = vec![0];
        let mut guard = self.reached.borrow_mut();
        let (walk, reached) = &mut *guard;
        *walk += 1;
        let walk = *walk;
        if reached.len() != hier.nodes.len() {
            if count(hier.nodes.len() / 64) {
                return Sticky::Unknown(TOO_LARGE);
            }
            *reached = vec![0; hier.nodes.len()];
        }
        reached[0] = walk;
        let mut depth = 0;
        while !level_steps.is_empty() && (later || depth < seq.len()) {
            depth += 1;
            let last = !later && depth == seq.len();
            let mut next = Vec::new();
            for &si in &level_steps {
                let (node, _, _, order) = steps[si as usize];
                let n = &hier.nodes[node as usize];
                for e in &hier.edges[n.edges.range()] {
                    let order = if later {
                        Ordering::Equal
                    } else {
                        order.then(e.elem.cmp(&seq[depth - 1]))
                    };
                    // as deep as the instance of `level`: the instances
                    // from here on come after it
                    if last && order != Ordering::Less {
                        return Sticky::No;
                    }
                    if count(1) {
                        return Sticky::Unknown(TOO_LARGE);
                    }
                    // `target` NONE: no visible definition
                    let Some(t) = hier.nodes.get(e.target as usize) else {
                        continue;
                    };
                    if defines(t.def) {
                        continue;
                    }
                    match evaluates(n.def, e.elem as ElemIdx, t.def) {
                        Err(()) => return Sticky::Unknown(TOO_LARGE),
                        Ok(true) => {
                            let mut lines = vec![(n.def, e.elem as ElemIdx)];
                            let mut at = si;
                            while at != 0 {
                                let (_, prev, elem, _) = steps[at as usize];
                                let holder = hier.nodes[steps[prev as usize].0 as usize].def;
                                lines.push((holder, elem as ElemIdx));
                                at = prev;
                            }
                            lines.reverse();
                            let names: Vec<String> = lines
                                .iter()
                                .map(|&(holder, elem)| {
                                    let e = &net.defs[holder.idx()].elements[elem];
                                    net.syms.name(e.name).to_ascii_lowercase()
                                })
                                .collect();
                            return Sticky::Top(Rc::new(First {
                                path: names.join("."),
                                lines,
                            }));
                        }
                        Ok(false) => {}
                    }
                    if !last && reached[e.target as usize] != walk {
                        reached[e.target as usize] = walk;
                        next.push(u32::try_from(steps.len()).expect("fewer than 2^32 steps"));
                        steps.push((e.target, si, e.elem, order));
                    }
                }
            }
            level_steps = next;
        }
        Sticky::No
    }

    /// Calls the function `name` (visible from level `level` in a text of
    /// kind `text`, see [`Resolver::find_func`]) with `args`. `None`: the
    /// built-in function of the name. The names in the body of a `.param`
    /// function, other than its arguments, are resolved where it is
    /// defined, not where it is called, and so are the functions it calls
    /// (golden `func_free_local`, `func_nested_local`, `func_global_more`),
    /// with no creation-order rule ([`Text::Func`]). An argument hides a
    /// parameter of the same name (golden `sticky_lazy_local`: r11, x10.ry,
    /// x12.rz).
    fn call(
        &mut self,
        level: usize,
        name: &str,
        args: &[f64],
        text: Text,
    ) -> Option<Result<(f64, Vec<Caveat>), EvalError>> {
        let net = self.net;
        let Some(sym) = net.syms.get(name) else {
            return (!netlist_expr::is_builtin(name))
                .then(|| Err(EvalError::Unresolved(not_visible(name))));
        };
        let (found, note) = self.find_func(level, sym, text);
        if let Some(id) = note {
            let top = self
                .frames
                .last_mut()
                .expect("a call is inside an evaluation");
            if top.notes.insert(id) {
                top.caveats.push(self.notes[id as usize].clone());
            }
        }
        let (j, f) = match found {
            Found::User(j, f) => (j, f),
            Found::Builtin => return None,
            Found::Nothing(why) => {
                return Some(Err(EvalError::Unresolved(self.notes[why as usize].clone())));
            }
        };
        let shown = net.syms.name(f.name);
        if f.args.len() != args.len() {
            return Some(Err(EvalError::BadCall(format!(
                "`{shown}` takes {} arguments, not {}",
                f.args.len(),
                args.len()
            ))));
        }
        let key = (
            j,
            sym,
            args.iter().map(|a| a.to_bits()).collect::<Vec<u64>>(),
        );
        self.step(1 + args.len());
        let v = match self.by_call.get(&key) {
            Some(v) => v.clone(),
            None if self.calls.contains(&(j, sym)) => {
                return Some(Err(EvalError::Unresolved(format!(
                    "the function `{shown}` calls itself"
                ))));
            }
            None => {
                let arg_names: Vec<String> = f
                    .args
                    .iter()
                    .map(|&a| net.syms.name(a).to_ascii_lowercase())
                    .collect();
                let bound: Vec<(String, f64)> = arg_names
                    .iter()
                    .cloned()
                    .zip(args.iter().copied())
                    .collect();
                let body = net.span_text(f.body).into_owned();
                self.calls.push((j, sym));
                let limited = self.limited;
                let (value, mut frame) = self.eval_text(&body, j, &bound, false, Text::Func);
                self.calls.pop();
                for g in self.func_alters(self.scopes[j].def, sym) {
                    if let Some(a) = g.alter {
                        frame.caveats.push(format!(
                            "the function `{shown}` is defined again by .alter #{a} at {}: this is the first run's value",
                            loc_str(net, g.loc)
                        ));
                    }
                }
                let step = ChainStep {
                    kind: StepKind::ParamFunction,
                    scope: self.scopes[j].path.clone(),
                    name: shown.to_owned(),
                    text: format!("{shown}({}) = {body}", arg_names.join(",")),
                    value: value.as_ref().ok().copied(),
                    loc: Some(net.loc(f.loc)),
                };
                let v = Val {
                    value,
                    node: Some(self.node(step, frame)),
                };
                if self.limited == limited {
                    self.by_call.insert(key, v.clone());
                }
                v
            }
        };
        let top = self
            .frames
            .last_mut()
            .expect("a call is inside an evaluation");
        top.deps.extend(v.node);
        Some(
            v.value
                .map(|x| (x, Vec::new()))
                .map_err(EvalError::Unresolved),
        )
    }

    /// The circuit temperature: the first `.temp` value of the first run,
    /// else the last `.option tnom` number, else 25 (golden `temper`,
    /// `temper_default`).
    pub(super) fn temperature(&mut self) -> Val {
        if let Some(t) = &self.temp {
            return t.clone();
        }
        let net = self.net;
        // a `.temp` value that uses `temper` gets this
        self.temp = Some(Val {
            value: Err("the `.temp` value uses `temper`: no value".to_owned()),
            node: None,
        });
        let temps: Vec<&ParamDef> = net.temps.iter().filter(|p| p.alter.is_none()).collect();
        let builtin = |text: String, value: Option<f64>, loc| ChainStep {
            kind: StepKind::Builtin,
            scope: String::new(),
            name: "temper".to_owned(),
            text,
            value,
            loc,
        };
        // a `.temp` line of an `.alter` block sets the temperature of the
        // later runs; a `.dc temp` sweeps it
        let mut altered = net.temp_alter_caveats();
        for t in ["temp", "temper"] {
            if let Some(sym) = net.syms.get(t) {
                altered.extend(net.swept_caveats(sym));
            }
        }
        let v = match temps.first() {
            // As measured, with no `.temp` HSPICE runs at TNOM: `.option
            // tnom=27` gives temp=27 (decks `t_tnom_op`, `p_option_tnom`)
            None if let Some(p) = self.tnom_option() => {
                let frame = Frame {
                    caveats: altered,
                    ..Frame::default()
                };
                let step = builtin(param_text(net, p), p.num, Some(net.loc(p.loc)));
                Val {
                    value: Ok(p.num.expect("a number")),
                    node: Some(self.node(step, frame)),
                }
            }
            None => {
                let step = builtin(
                    "25 (no `.temp` line in the first run: HSPICE's default)".to_owned(),
                    Some(25.0),
                    None,
                );
                let frame = Frame {
                    caveats: altered,
                    ..Frame::default()
                };
                Val {
                    value: Ok(25.0),
                    node: Some(self.node(step, frame)),
                }
            }
            Some(&p) => {
                let (value, mut frame) = self.eval_param(p, 0, false, Text::Body);
                if temps.len() > 1 || more_on_line(net, p) {
                    frame.caveats.push(format!(
                        "the deck sets more than one temperature (`.temp` at {}): HSPICE runs each one; this value uses the first",
                        loc_str(net, p.loc)
                    ));
                }
                frame.caveats.extend(altered);
                let step = builtin(
                    param_text(net, p),
                    value.as_ref().ok().copied(),
                    Some(net.loc(p.loc)),
                );
                Val {
                    value,
                    node: Some(self.node(step, frame)),
                }
            }
        };
        self.temp = Some(v.clone());
        v
    }

    /// The `.option tnom` of TNOM (see `Netlist::tnom`): also the
    /// temperature when the first run has no `.temp` line.
    fn tnom_option(&self) -> Option<&'n ParamDef> {
        self.net.tnom.as_ref()
    }

    /// TNOM: the value of `tnom_option`, else 25.
    pub(super) fn tnom(&self) -> f64 {
        self.tnom_option().and_then(|p| p.num).unwrap_or(25.0)
    }

    /// The value of the word `w` of a source function at level `k`; the
    /// nodes it uses go to the current frame.
    fn word(&mut self, w: Option<&&str>, k: usize) -> Result<f64, String> {
        let Some(w) = w else {
            return Err("the function has too few arguments".to_owned());
        };
        let (v, frame) = self.eval_text(w, k, &[], true, Text::Body);
        let top = self.frames.last_mut().expect("inside source_t0");
        top.deps.extend(frame.deps);
        top.caveats.extend(frame.caveats);
        v
    }

    /// The value at t=0 of the function `f` of a source at level `k`
    /// (golden `src_fn_nodc`, `src_sin_t0`, `pwl_*`); `td` is a `TD=`
    /// parameter of the line.
    fn source_t0(&mut self, f: &ParamDef, td: Option<&ParamDef>, k: usize) -> Result<f64, String> {
        let net = self.net;
        let fname = net.syms.name(f.name);
        let args = net.param_text(f).unwrap_or_default();
        let words = arg_words(&args);
        match fname.to_ascii_lowercase().as_str() {
            // the first argument: the value before the waveform starts
            "pulse" | "exp" | "sffm" => self.word(words.first(), k),
            "sin" => {
                // vo + va*sin(phase): the SIN formula at t=0, also before
                // the delay (golden `src_sin_t0`)
                let vo = self.word(words.first(), k)?;
                if words.get(5).is_none() {
                    return Ok(vo);
                }
                let phase = self.word(words.get(5), k)?;
                if phase == 0.0 {
                    return Ok(vo);
                }
                let va = self.word(words.get(1), k)?;
                Ok(vo + va * phase.to_radians().sin())
            }
            "pwl" => {
                let values = pwl_values(&args);
                // `TD=` inside the parentheses or on the line
                let td_word = words
                    .iter()
                    .find_map(|w| {
                        w.split_once('=')
                            .filter(|(key, _)| key.trim().eq_ignore_ascii_case("td"))
                    })
                    .map(|(_, v)| v);
                let delay = match (td_word, td) {
                    (Some(w), _) => self.word(Some(&w), k)?,
                    (None, Some(p)) => {
                        let text = param_text(net, p);
                        self.word(Some(&text.as_str()), k)?
                    }
                    (None, None) => 0.0,
                };
                self.pwl_at(&values, -delay, k)
            }
            // sa*(oc + sin(2*pi*fm*(t-td)))*sin(2*pi*fc*(t-td)): 0 at t=0
            // with no delay (golden `src_fn_nodc`), and 0 before a delay
            // (golden `src_delay_scale`)
            "am" => Ok(0.0),
            _ => Err(format!(
                "the value of a {fname} source at t=0 is not resolved"
            )),
        }
    }

    /// The value of the PWL `values` (time, value, …) at time `t`: the
    /// first value before the first time point, the last after the last,
    /// linear in between.
    fn pwl_at(&mut self, values: &[&str], t: f64, k: usize) -> Result<f64, String> {
        let pairs = values.len() / 2;
        if pairs == 0 {
            return Err("the PWL has no point".to_owned());
        }
        let mut t0 = self.word(values.first(), k)?;
        let mut v0 = self.word(values.get(1), k)?;
        if t <= t0 {
            return Ok(v0);
        }
        for i in 1..pairs {
            let t1 = self.word(values.get(2 * i), k)?;
            let v1 = self.word(values.get(2 * i + 1), k)?;
            if t <= t1 {
                return Ok(if t1 == t0 {
                    v1
                } else {
                    v0 + (v1 - v0) * (t - t0) / (t1 - t0)
                });
            }
            (t0, v0) = (t1, v1);
        }
        Ok(v0)
    }

    /// The answer for the element parameter `item`, at level `k` (the
    /// scope of the element; an `X`-line override at `k + 1`, see
    /// [`Text`]), for the element at `path`.
    pub(super) fn resolve_item(&mut self, item: Item<'n>, k: usize, path: &str) -> Resolved {
        let net = self.net;
        let step = |text: String, value: &Result<f64, String>, loc: Option<LocIdx>| ChainStep {
            kind: StepKind::ElementParam,
            scope: path.to_owned(),
            name: item.name.clone(),
            text,
            value: value.as_ref().ok().copied(),
            loc: loc.map(|l| net.loc(l)),
        };
        let (value, root, mut frame) = match &item.src {
            Src::Param(p) => {
                let (level, text) = match (item.child, item.sym) {
                    (true, Some(s)) => (k + 1, Text::Override(s)),
                    _ => (k, Text::Body),
                };
                let (value, frame) = self.eval_param(p, level, true, text);
                let root = step(param_text(net, p), &value, Some(p.loc));
                (value, Some(root), frame)
            }
            Src::Word(text, loc) => {
                let (value, frame) = self.eval_text(text, k, &[], true, Text::Body);
                let root = step(text.clone(), &value, Some(*loc));
                (value, Some(root), frame)
            }
            Src::Fail(why, p) => {
                let value = Err(why.clone());
                let root = p.map(|p| step(param_text(net, p), &value, Some(p.loc)));
                (value, root, Frame::default())
            }
            Src::Builtin(v, text) => {
                let value = Ok(*v);
                let root = ChainStep {
                    kind: StepKind::Builtin,
                    ..step((*text).to_owned(), &value, None)
                };
                (value, Some(root), Frame::default())
            }
            Src::SourceT0(func, td) => {
                self.frames.push(Frame::default());
                let value = self.source_t0(func, *td, k);
                let mut frame = self.frames.pop().expect("pushed above");
                let fname = net.syms.name(func.name);
                frame.caveats.push(format!(
                    "the line has no DC value: HSPICE's `.op` uses the value of its {fname} at t=0 (golden `src_fn_nodc`, `src_sin_t0`, `src_delay_scale`)"
                ));
                let root = ChainStep {
                    name: fname.to_owned(),
                    ..step(param_text(net, func), &value, Some(func.loc))
                };
                (value, Some(root), frame)
            }
        };
        frame.caveats.extend(item.caveats);
        self.answer(item.name, value, root, &frame)
    }

    /// The answer for `p`, a `key=value` of a `.model` card, evaluated at
    /// level `level`, the scope of the definition that holds the card, as
    /// a text that every instance of that definition evaluates (a statistical
    /// function gives its nominal value there, as in a `.param`). `caveats`
    /// go on the answer.
    pub(super) fn resolve_card_param(
        &mut self,
        p: &ParamDef,
        level: usize,
        caveats: Vec<String>,
    ) -> Resolved {
        let net = self.net;
        let name = net.syms.name(p.name).to_owned();
        let (value, mut frame) = self.eval_param(p, level, false, Text::Body);
        let root = ChainStep {
            kind: StepKind::ModelParam,
            scope: self.scopes[level].path.clone(),
            name: name.clone(),
            text: param_text(net, p),
            value: value.as_ref().ok().copied(),
            loc: Some(net.loc(p.loc)),
        };
        frame.caveats.extend(caveats);
        self.answer(name, value, Some(root), &frame)
    }
}

impl Resolver<'_> {
    /// The caveat of the override `p` of `n` on the `X` line that makes
    /// level `level`, when another definition wins there: under GLOBAL a
    /// definition above the new instance beats the override (golden
    /// `parhier_global`, `nested_global`), so the instance does not use
    /// the override's value. The caveats of the value that the instance
    /// uses follow (a sweep or an `.alter` change of the winning top-level
    /// `.param`).
    pub(super) fn shadowed(&mut self, level: usize, n: Sym, p: &ParamDef) -> Vec<String> {
        let Some(c) = self.find(level, n) else {
            return Vec::new();
        };
        if std::ptr::eq(c.p, p) {
            return Vec::new();
        }
        let net = self.net;
        let name = net.syms.name(n);
        let what = match c.kind {
            StepKind::GlobalParam => "the top-level `.param`".to_owned(),
            StepKind::LocalParam => format!("the body `.param` of `{}`", self.scopes[c.level].path),
            StepKind::SubcktDefault => {
                format!("the `.subckt` default of `{}`", self.scopes[c.level].path)
            }
            _ => format!("the `X`-line override of `{}`", self.scopes[c.level].path),
        };
        let val = self.resolve(level, n);
        let used = match &val.value {
            Ok(v) => format!("the instance uses {name} = {}", fmt_num(*v)),
            Err(why) => format!("the instance's `{name}` has no value ({why})"),
        };
        let mut out = vec![format!(
            "HSPICE does not use this override: under parhier=global {what} `{name}={}` at {} wins over it, so {used} (golden `parhier_global`, `nested_global`; resolve_param on `{}` gives the chain)",
            param_text(net, c.p),
            loc_str(net, c.p.loc),
            self.scopes[level].path
        )];
        out.extend(self.node_caveats(val.node));
        out
    }

    /// The caveats of the node `id` and of the nodes it depends on, as
    /// [`Resolver::answer`] lists them, without the chain; the walk passes
    /// over the nodes with no caveat below them.
    fn node_caveats(&self, id: Option<u32>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut known: HashSet<&str> = HashSet::new();
        let mut seen: HashSet<u32> = HashSet::new();
        let mut todo: Vec<u32> = id.into_iter().collect();
        while let Some(id) = todo.pop() {
            if self.nodes[id as usize].quiet || !seen.insert(id) {
                continue;
            }
            let node = &self.nodes[id as usize];
            for c in &node.caveats {
                if known.insert(c) {
                    out.push(c.clone());
                }
            }
            todo.extend(node.deps.iter().rev());
        }
        out
    }

    /// The caveats of the `X` lines that make levels `1..=upto` and whose
    /// `m` is an expression with a value of zero or less: HSPICE rejects
    /// the deck (golden `m_expr_zero`). `m` is evaluated in the scope of
    /// the line, not in the new instance (golden `ovr_scope_more_local`).
    /// A number as written is checked by `path_caveats`.
    pub(super) fn path_m_caveats(&mut self, upto: usize) -> Vec<String> {
        let net = self.net;
        let mut out = Vec::new();
        let Some(m) = self.m else { return out };
        for j in 1..=upto {
            let Some((holder, i)) = self.scopes[j].x else {
                continue;
            };
            let d = &net.defs[holder.idx()];
            let e = &d.elements[i];
            let Some(p) = d.element_params(e).iter().rfind(|p| p.name == m) else {
                continue;
            };
            if p.num.is_some() || net.m_not_positive(d, e, m).is_some() {
                continue;
            }
            if let (Ok(v), _) = self.eval_param(p, j - 1, true, Text::Body)
                && v <= 0.0
            {
                out.push(format!(
                    "`{}` has `m={}` = {v}: HSPICE rejects a multiplier of zero or less on an instance path (golden `m_expr_zero`, `hier_m_zero`)",
                    self.scopes[j].path,
                    param_text(net, p)
                ));
            }
        }
        out
    }

    /// The parameter names defined on the path up to level `k` that are
    /// close to `wanted`.
    pub(super) fn visible_names(&self, k: usize, wanted: &str) -> Vec<String> {
        let net = self.net;
        let mut names: Vec<&str> = vec!["temper"];
        for s in &self.scopes[..=k] {
            if let Some((holder, i)) = s.x {
                let d = &net.defs[holder.idx()];
                names.extend(
                    d.element_params(&d.elements[i])
                        .iter()
                        .filter(|p| Some(p.name) != self.m)
                        .map(|p| net.syms.name(p.name)),
                );
            }
            let d = &net.defs[s.def.idx()];
            names.extend(
                d.params
                    .iter()
                    .filter(|p| p.alter.is_none())
                    .map(|p| net.syms.name(p.name)),
            );
            names.extend(d.defaults.iter().map(|p| net.syms.name(p.name)));
        }
        suggest(wanted, names.into_iter().map(|n| (n, n.to_owned())))
    }
}

/// The evaluator's view of the names at one level.
struct Cx<'a, 'n> {
    r: &'a mut Resolver<'n>,
    level: usize,
    args: &'a [(String, f64)],
    text: Text,
}

impl Env for Cx<'_, '_> {
    fn var(&mut self, name: &str) -> Result<(f64, Vec<Caveat>), EvalError> {
        if let Some(&(_, v)) = self.args.iter().find(|(a, _)| a == name) {
            return Ok((v, Vec::new()));
        }
        // `time` and `hertz` with no `.param` of the name read 0 in a value,
        // as measured (decks `p_time_in_expr`, `sv_hertz*`)
        if matches!(name, "time" | "hertz")
            && self
                .r
                .net
                .syms
                .get(name)
                .is_none_or(|s| self.r.find(self.level, s).is_none())
        {
            let top = self.r.frames.last_mut().expect("inside an evaluation");
            top.caveats.push(format!(
                "`{name}` reads 0: in a value HSPICE gives it the value at the `.op` point"
            ));
            return Ok((0.0, Vec::new()));
        }
        let v = if name == "temper" {
            self.r.temperature()
        } else {
            match self.r.net.syms.get(name) {
                Some(sym) => match self.text {
                    // `x1 a b cell w='w*3'`: `w` from the parent's scope
                    // (golden `ovr_self_local`); with none there, HSPICE
                    // rejects the deck (golden `ovr_self_global`)
                    Text::Override(own) if own == sym => {
                        let up = self.level - 1;
                        if self.r.find(up, sym).is_none() {
                            return Err(EvalError::Unresolved(format!(
                                "the `X`-line override of `{name}` uses `{name}` itself, which HSPICE looks up above the new instance, and no scope above it defines `{name}`: HSPICE rejects the deck (golden `ovr_self_global`: \"cannot redefine parameter to itself\")"
                            )));
                        }
                        self.r.resolve(up, sym)
                    }
                    // A body `.param` or a default `c='c*2'` reads `c` from
                    // above, as an override does. This is the measured rule
                    // (decks `q_cycle_*_self*`), as src/eval.rs reads it. With
                    // no `c` above, it is a cycle.
                    Text::Default(own)
                        if own == sym
                            && self.level > 0
                            && self.r.find(self.level - 1, sym).is_some() =>
                    {
                        self.r.resolve(self.level - 1, sym)
                    }
                    _ => self.r.resolve(self.level, sym),
                },
                None => {
                    return Err(EvalError::Unresolved(undefined(self.r.net, name)));
                }
            }
        };
        let top = self.r.frames.last_mut().expect("inside an evaluation");
        top.deps.extend(v.node);
        v.value
            .map(|x| (x, Vec::new()))
            .map_err(EvalError::Unresolved)
    }

    /// `None`: the built-in function of the name (see
    /// [`Resolver::find_func`]).
    fn user_fn(
        &mut self,
        name: &str,
        args: &[f64],
    ) -> Option<Result<(f64, Vec<Caveat>), EvalError>> {
        self.r.call(self.level, name, args, self.text)
    }
}
