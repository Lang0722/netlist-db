//! How many MOSFETs use each model, counted through the hierarchy without a walk of each copy.
use super::*;

/// How many MOSFETs of a circuit use each model, counted through the
/// hierarchy: the summary of [`AST::mosfet_models`], for circuits too large
/// to list transistor by transistor (e.g. a 30 MB netlist whose hierarchy
/// holds 300 million MOSFETs). Each subckt instance that differs (by its
/// definition, and the parameters that its contents read) is evaluated
/// once; its counts are multiplied by the number of such instances.
#[derive(Debug, Clone, Default)]
pub struct ModelUsage<'a, 's> {
    /// the MOSFETs of the circuit. This count is too low if `errors` holds an
    /// error for an `X` line or a condition (not for one MOSFET) that hides
    /// MOSFETs, for example [`SelectError::NoSubckt`] for a line whose definition
    /// holds MOSFETs.
    pub mosfets: u64,
    /// each model chosen, and how many MOSFETs use it: the most used first
    pub models: Vec<(&'a Model<'s>, u64)>,
    /// each reason why no model could be chosen, how many MOSFETs it
    /// concerns (for [`SelectError::NoSubckt`], how many `X` lines), and the
    /// path of one of them: the most frequent first
    pub errors: Vec<(SelectError, u64, String)>,
    /// the reads where HSPICE can use a different value
    pub far_reads: Vec<FarRead>,
}

impl<'s> AST<'s> {
    /// Whether the netlist only defines things: no instance line at the top
    /// level, also in no branch of an `.if` block
    pub fn is_library(&self) -> bool {
        self.instance.is_empty()
            && self
                .cond
                .iter()
                .flat_map(|block| &block.branches)
                .all(|branch| branch.ast.is_library())
    }

    /// The subckts on a cycle of calls, and those whose lines lead to such a
    /// cycle. A call goes by name, so this holds each subckt that an instance
    /// path can re-enter. Only under these subckts do the counts depend on the
    /// path above.
    pub(crate) fn reentrant_defs(&self) -> Vec<&Subckt<'s>> {
        let mut defs = Vec::new();
        let mut calls = HashMap::default();
        collect_calls(self, None, &mut defs, &mut calls);
        let reaching = names_that_reach_a_cycle(&calls);
        defs.retain(|def| reaching.contains(def.name.to_lowercase().as_str()));
        defs
    }

    /// How many MOSFETs of the circuit (the top level, and the subckt
    /// instances under it) use each model
    pub fn model_usage(&self) -> ModelUsage<'_, 's> {
        let (scope, active) = self.top_level();
        let mut usage = Usage::new(self, &active.branches, &scope);
        let on_path = with_branches(&[], self, &active);
        let scopes = with_scope(&[], &scope, &active);
        let counts = usage.level(self, &active, &scopes, &on_path, 0, 0);
        let far_reads = scope.far_reads();
        log_far_reads(&far_reads);
        ModelUsage {
            far_reads,
            ..usage.finish(&counts)
        }
    }

    /// [`AST::model_usage`] for an instance of the subckt `name` (defined at
    /// the top level) with its default parameters. `None` if there is no
    /// such subckt.
    pub fn model_usage_of(&self, name: &str) -> Option<ModelUsage<'_, 's>> {
        let (scope, top) = self.top_level();
        let on_top = with_branches(&[], self, &top);
        let def = resolve(name, &on_top)?;
        let (child, active) = instance_scope(&scope, def, &[]);
        let mut usage = Usage::new(self, &top.branches, &scope);
        usage.expanding.push(def);
        let visible = usage.visible_with(0, &def.ast, &active);
        let on_path = with_branches(&on_top, &def.ast, &active);
        let top_scopes = with_scope(&[], &scope, &top);
        let scopes = with_scope(&top_scopes, &child, &active);
        let counts = usage.level(&def.ast, &active, &scopes, &on_path, 1, visible);
        let far_reads = scope.far_reads();
        log_far_reads(&far_reads);
        Some(ModelUsage {
            far_reads,
            ..usage.finish(&counts)
        })
    }
}

/// Adds to `defs` the subckts of `ast` and of its branches, and to `calls` the
/// lowercase names that the lines of each subckt call. `owner`: the name of the
/// subckt that `ast` is in.
fn collect_calls<'a, 's>(
    ast: &'a AST<'s>,
    owner: Option<&'a str>,
    defs: &mut Vec<&'a Subckt<'s>>,
    calls: &mut HashMap<&'a str, HashSet<String>>,
) {
    if let Some(owner) = owner {
        for inst in &ast.instance {
            if let InstanceCtx::Subckt(x) = &inst.ctx {
                calls
                    .entry(owner)
                    .or_default()
                    .insert(x.cktname.to_lowercase());
            }
        }
    }
    for branch in ast.cond.iter().flat_map(|block| &block.branches) {
        collect_calls(&branch.ast, owner, defs, calls);
    }
    for (name, def) in &ast.subckt {
        defs.push(def);
        collect_calls(&def.ast, Some(name), defs, calls);
    }
}

/// The names in `calls` that are on a cycle or lead to one. The search keeps an
/// explicit stack, because a chain of calls can be long.
fn names_that_reach_a_cycle<'c>(calls: &'c HashMap<&str, HashSet<String>>) -> HashSet<&'c str> {
    let mut open = HashSet::default();
    let mut done = HashSet::default();
    let mut reaching = HashSet::default();
    for (&start, callees) in calls {
        if done.contains(start) {
            continue;
        }
        open.insert(start);
        let mut stack = vec![(start, callees.iter())];
        while let Some((name, next)) = stack.last_mut() {
            let name = *name;
            let Some(callee) = next.next() else {
                stack.pop();
                open.remove(name);
                done.insert(name);
                if reaching.contains(name)
                    && let Some((caller, _)) = stack.last()
                {
                    reaching.insert(*caller);
                }
                continue;
            };
            let callee = callee.as_str();
            if open.contains(callee) || reaching.contains(callee) {
                reaching.insert(name);
            } else if !done.contains(callee)
                && let Some((&key, more)) = calls.get_key_value(callee)
            {
                open.insert(key);
                stack.push((key, more.iter()));
            }
        }
    }
    reaching
}

/// What a MOSFET uses: a model (by address), or an error (by number)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Use {
    Model(usize),
    Error(usize),
}

type Counts = HashMap<Use, u64>;

/// The values passed on an instance line: name, and the bits of the value
/// that name has in the new instance (or the error that computing it gave)
type Passed = Vec<(String, Result<u64, String>)>;

/// The values passed on the instance line `line`, of the new instance `child`
/// (`None` if the line passes nothing); only those of `only`, if given
fn passed_values(
    child: Option<&Scope<'_>>,
    line: &[KeyValue<'_>],
    only: Option<&HashSet<String>>,
) -> Passed {
    let Some(child) = child else {
        return Passed::new();
    };
    line.iter()
        .filter(|kv| !is_multiplier(kv))
        .map(|kv| kv.k.to_ascii_lowercase())
        .filter(|k| only.is_none_or(|only| only.contains(k)))
        .map(|k| {
            let v = child.get(&k).map(f64::to_bits).map_err(|e| e.to_string());
            (k, v)
        })
        .collect()
}

/// The parameters that the contents of a subckt read (its own, and those of
/// the instances under it)
#[derive(Debug)]
struct Reads {
    /// every parameter that a level of an instance of it reads from the
    /// instance's own level or above: the values passed on its line that
    /// matter
    all: HashSet<String>,
    /// those read from above the instance's level, in a fixed order
    outside: Vec<String>,
}

/// What the counts of a subckt instance depend on
#[derive(Debug, PartialEq, Eq, Hash)]
enum MemoKey {
    /// A definition whose contents read known parameters: the definitions
    /// its lines can see above it (see [`Usage::visible_below`]), the values
    /// passed that they read, and how the parameters they read from above
    /// are bound there
    Reads(usize, usize, Passed, Vec<Binding>),
    /// Any other (its contents call a function defined with `.param f(x)`, a
    /// bin of an instance above reads parameters, or a subckt instantiates
    /// itself): the values passed, and the context
    Context(usize, Passed, usize),
}

/// The key of `Usage::memo`: what the counts depend on, and the level of an instance
/// that can read past [`HSPICE_READ_REACH`]
type KeptKey = (MemoKey, Option<usize>);

/// The counting walk of [`AST::model_usage`]
struct Usage<'a, 's> {
    sel: Selector<'a, 's>,
    parhier: ParHier,
    models: HashMap<usize, &'a Model<'s>>,
    /// errors, numbered; with the path of the first MOSFET that had each
    errors: Vec<(SelectError, String)>,
    error_numbers: HashMap<String, usize>,
    /// the counts of the subckt instances evaluated
    memo: HashMap<KeptKey, Rc<[(Use, u64)]>>,
    /// how many levels the instances of a definition (by address) hold, at most:
    /// their own and those below it. An instance that can read past
    /// [`HSPICE_READ_REACH`] has its level in the key of `memo`, so that each such
    /// level is evaluated and its reads are found.
    heights: HashMap<usize, usize>,
    /// the deepest level that the current walk reached
    deepest: usize,
    /// how often the counts of a definition (by address) were found in, or
    /// missing from, `memo`: (found, missing)
    memo_use: HashMap<usize, (u64, u64)>,
    /// the definitions whose instances are not kept in `memo`: a path can
    /// re-enter them, so that their counts depend on the path (see
    /// [`AST::reentrant_defs`]); or almost each one is different (a value passed
    /// that differs on every instance, read below), so that keeping them would
    /// only take memory
    unkept: HashSet<usize>,
    /// the context of the next level that passes or defines parameters, or
    /// defines models or subckts (a level that does none of these sees the
    /// context above it). A level whose instance repeats one already
    /// evaluated is found in `memo`, so each context evaluated is new.
    next_context: usize,
    /// the numbers of the lists of definitions that hold models or subckts,
    /// on the instance paths seen: by the number of the list above and the
    /// definition (by address)
    visible: HashMap<(usize, usize), usize>,
    /// what the contents of a definition (by address), under a list of
    /// definitions (by number), read (`None`: not known)
    reads: HashMap<(usize, usize), Option<Rc<Reads>>>,
    /// the definitions whose reads are being found
    reading: Vec<usize>,
    path: Vec<&'a str>,
    expanding: Vec<&'a Subckt<'s>>,
}

impl<'a, 's> Usage<'a, 's> {
    fn new(ast: &'a AST<'s>, active: &[&'a AST<'s>], scope: &Scope<'_>) -> Self {
        Self {
            sel: Selector::new(ast, active),
            parhier: scope.parhier(),
            models: HashMap::default(),
            errors: Vec::new(),
            error_numbers: HashMap::default(),
            memo: HashMap::default(),
            heights: HashMap::default(),
            deepest: 0,
            memo_use: HashMap::default(),
            unkept: ast
                .reentrant_defs()
                .into_iter()
                .map(|def| ptr::from_ref(def) as usize)
                .collect(),
            next_context: 2,
            visible: HashMap::default(),
            reads: HashMap::default(),
            reading: Vec::new(),
            path: Vec::new(),
            expanding: Vec::new(),
        }
    }

    fn finish(self, counts: &Counts) -> ModelUsage<'a, 's> {
        let mut usage = ModelUsage {
            mosfets: counts.values().sum(),
            ..Default::default()
        };
        let mut errors: Vec<_> = self.errors.into_iter().map(Some).collect();
        for (&u, &n) in counts {
            match u {
                Use::Model(addr) => usage.models.push((self.models[&addr], n)),
                Use::Error(number) => {
                    let (e, path) = errors[number].take().expect("each error is counted once");
                    if !e.is_mosfet() {
                        usage.mosfets -= n;
                    }
                    usage.errors.push((e, n, path));
                }
            }
        }
        usage
            .models
            .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));
        usage
            .errors
            .sort_by_cached_key(|(e, n, path)| (Reverse(*n), path.clone(), e.to_string()));
        usage
    }

    /// What the lines of a level see, as a number: the definitions on its
    /// instance path that hold models or subckts. `visible`: the number of
    /// the level above; `ast`: this level's definition, or a branch active in
    /// it. Two levels with the same number find the same definition for each
    /// name.
    fn visible_below(&mut self, visible: usize, ast: &AST<'_>) -> usize {
        if ast.model.is_empty() && ast.subckt.is_empty() {
            return visible;
        }
        let next = self.visible.len() + 1;
        *self
            .visible
            .entry((visible, ptr::from_ref(ast) as usize))
            .or_insert(next)
    }

    /// [`Usage::visible_below`] for a level with its active branches
    fn visible_with(&mut self, visible: usize, ast: &AST<'_>, active: &Active<'_, '_>) -> usize {
        let mut visible = self.visible_below(visible, ast);
        for branch in &active.branches {
            visible = self.visible_below(visible, branch);
        }
        visible
    }

    /// What the contents of `def` read, for an instance of it under a level
    /// with the definitions `on_path` and the number `visible`; `None` if not
    /// known
    fn reads(
        &mut self,
        def: &'a Subckt<'s>,
        on_path: &[&'a AST<'s>],
        visible: usize,
    ) -> Option<Rc<Reads>> {
        let addr = ptr::from_ref(def) as usize;
        if let Some(reads) = self.reads.get(&(addr, visible)) {
            return reads.clone();
        }
        // a subckt that instantiates itself, or too deep a hierarchy
        if self.reading.contains(&addr) || self.reading.len() >= MAX_HIERARCHY_DEPTH {
            return None;
        }
        self.reading.push(addr);
        let reads = self.find_reads(def, on_path, visible).map(Rc::new);
        self.reading.pop();
        self.reads.insert((addr, visible), reads.clone());
        reads
    }

    fn find_reads(
        &mut self,
        def: &'a Subckt<'s>,
        on_path: &[&'a AST<'s>],
        visible: usize,
    ) -> Option<Reads> {
        let mut all = IndexSet::new();
        // (a call of a function makes what is read unknown)
        let definitions = def
            .params
            .iter()
            .chain(&def.ast.param)
            .filter(|kv| function_name(&kv.k).is_none());
        for kv in definitions.clone() {
            extend(&mut all, &self.sel.names(&kv.v)?);
        }
        let inner: Vec<_> = on_path.iter().copied().chain(once(&def.ast)).collect();
        let inner_visible = self.visible_below(visible, &def.ast);
        self.lines_read(&def.ast, &inner, inner_visible, &mut all)?;
        let mut outside: Vec<String> = all.iter().cloned().collect();
        if self.parhier == ParHier::Local {
            // the closest definition is used: one of this level hides those
            // above
            let here: HashSet<String> = definitions.map(|kv| kv.k.to_ascii_lowercase()).collect();
            outside.retain(|n| !here.contains(n));
        }
        Some(Reads {
            all: all.into_iter().collect(),
            outside,
        })
    }

    /// Adds to `all` what the lines of `ast` read, and what its `.if` blocks read
    /// in all their branches (any of them can be active). `None` if not known:
    /// so also for a branch with models or subckts, as what the lines of the
    /// level see then depends on the branch. `inner`: the definitions that the
    /// lines of the level see.
    fn lines_read(
        &mut self,
        ast: &'a AST<'s>,
        inner: &[&'a AST<'s>],
        inner_visible: usize,
        all: &mut IndexSet<String>,
    ) -> Option<()> {
        for inst in &ast.instance {
            match &inst.ctx {
                InstanceCtx::MOSFET(m) => {
                    for kv in &m.params {
                        if SIZES.iter().any(|k| kv.k.eq_ignore_ascii_case(k)) {
                            extend(all, &self.sel.names(&kv.v)?);
                        }
                    }
                    let line = self.sel.read_line(m, inner);
                    for v in line.values() {
                        extend(all, &self.sel.names(&v)?);
                    }
                    // the ranges of the bins it may use, read where each is
                    // defined: those of the top level read the top level's
                    // parameters (the same everywhere), those of this subckt
                    // its own; those of an instance above it the parameters
                    // of that instance, which are not followed: unknown
                    let name = line.name().to_ascii_lowercase();
                    let last = inner.len() - 1;
                    for (depth, &level) in inner.iter().enumerate().skip(1) {
                        let index = self.sel.index(level);
                        for bin in index.bins.get(&name).into_iter().flatten() {
                            let names = self.sel.limit_names(bin.model)?;
                            if depth < last && !names.is_empty() {
                                return None;
                            }
                            extend(all, &names);
                        }
                    }
                }
                InstanceCtx::Subckt(x) => {
                    for kv in &x.params {
                        extend(all, &self.sel.names(&kv.v)?);
                    }
                    if let Some(child) = resolve(&x.cktname, inner) {
                        extend(all, &self.reads(child, inner, inner_visible)?.outside);
                    }
                }
                _ => {}
            }
        }
        for block in &ast.cond {
            for branch in &block.branches {
                let inside = &branch.ast;
                if !inside.model.is_empty() || !inside.subckt.is_empty() {
                    return None;
                }
                if let Some(cond) = &branch.cond {
                    extend(all, &self.sel.text_names(cond)?);
                }
                for kv in inside
                    .param
                    .iter()
                    .filter(|kv| function_name(&kv.k).is_none())
                {
                    extend(all, &self.sel.names(&kv.v)?);
                }
                self.lines_read(inside, inner, inner_visible, all)?;
            }
        }
        Some(())
    }

    /// The counts of the instances of `ast` (context `context`, seeing the
    /// definitions numbered `visible`); `active`, `scopes` and `on_path` as in
    /// [`Walk::ast`]
    fn level(
        &mut self,
        ast: &'a AST<'s>,
        active: &Active<'a, 's>,
        scopes: &[&Scope<'_>],
        on_path: &[&'a AST<'s>],
        context: usize,
        visible: usize,
    ) -> Counts {
        let scope = *scopes.last().expect("the top level at least");
        let mut counts = Counts::default();
        for e in &active.errors {
            let used = Use::Error(self.error_number(condition_error(e), &e.block.line.name));
            *counts.entry(used).or_default() += 1;
        }
        if ast.cond.is_empty() {
            for inst in &ast.instance {
                self.line(inst, scope, scopes, on_path, context, visible, &mut counts);
            }
        } else {
            let lines = in_order(ast, &active.branches, &|a| &a.instance, &|b| b.instance_at);
            for inst in lines {
                self.line(inst, scope, scopes, on_path, context, visible, &mut counts);
            }
        }
        counts
    }

    /// Adds the counts of the line `inst` of a level to `counts`
    #[allow(clippy::too_many_arguments)]
    fn line(
        &mut self,
        inst: &'a Instance<'s>,
        scope: &Scope<'_>,
        scopes: &[&Scope<'_>],
        on_path: &[&'a AST<'s>],
        context: usize,
        visible: usize,
        counts: &mut Counts,
    ) {
        {
            match &inst.ctx {
                InstanceCtx::MOSFET(m) => {
                    let used = match self.sel.select(m, scopes, on_path) {
                        Ok(s) => {
                            let addr = ptr::from_ref(s.model) as usize;
                            self.models.entry(addr).or_insert(s.model);
                            Use::Model(addr)
                        }
                        Err(e) => Use::Error(self.error_number(e, &inst.name)),
                    };
                    *counts.entry(used).or_default() += 1;
                }
                InstanceCtx::Subckt(x) => {
                    let Some(def) = resolve(&x.cktname, on_path) else {
                        let e = SelectError::NoSubckt(x.cktname.to_string());
                        let used = Use::Error(self.error_number(e, &inst.name));
                        *counts.entry(used).or_default() += 1;
                        return;
                    };
                    if let Some(e) = expansion_error(x, def, &self.expanding) {
                        let used = Use::Error(self.error_number(e, &inst.name));
                        *counts.entry(used).or_default() += 1;
                        return;
                    }
                    let addr = ptr::from_ref(def) as usize;
                    let level = self.expanding.len();
                    let height = self.heights.get(&addr).copied().unwrap_or(0);
                    let far = (level + height > HSPICE_READ_REACH).then_some(level);
                    // (a line that passes nothing needs no new instance to
                    // look in the memo)
                    let child = (!x.params.is_empty() || !def.ast.cond.is_empty())
                        .then(|| instance_scope(scope, def, &x.params));
                    let key = if self.unkept.contains(&addr) {
                        None
                    } else {
                        let key = match self.reads(def, on_path, visible) {
                            Some(reads) => MemoKey::Reads(
                                addr,
                                visible,
                                passed_values(
                                    child.as_ref().map(|c| &c.0),
                                    &x.params,
                                    Some(&reads.all),
                                ),
                                reads.outside.iter().map(|n| scope.binding(n)).collect(),
                            ),
                            None => MemoKey::Context(
                                addr,
                                passed_values(child.as_ref().map(|c| &c.0), &x.params, None),
                                context,
                            ),
                        };
                        let key = (key, far);
                        if let Some(c) = self.memo.get(&key) {
                            add(counts, c);
                            self.memo_use.entry(addr).or_default().0 += 1;
                            self.deepest = self.deepest.max(level + height);
                            return;
                        }
                        Some(key)
                    };
                    let binds = !x.params.is_empty()
                        || !def.params.is_empty()
                        || !def.ast.param.is_empty()
                        || !def.ast.param_func.is_empty()
                        || !def.ast.model.is_empty()
                        || !def.ast.subckt.is_empty()
                        || !def.ast.cond.is_empty();
                    let inner_context = if binds {
                        self.next_context += 1;
                        self.next_context
                    } else {
                        context
                    };
                    let (child, active) = child.unwrap_or_else(|| instance_scope(scope, def, &[]));
                    let inner_visible = self.visible_with(visible, &def.ast, &active);
                    let inner_scopes = with_scope(scopes, &child, &active);
                    let inner = with_branches(on_path, &def.ast, &active);
                    self.path.push(&inst.name);
                    self.expanding.push(def);
                    let deepest_before = core::mem::replace(&mut self.deepest, level + 1);
                    let c = self.level(
                        &def.ast,
                        &active,
                        &inner_scopes,
                        &inner,
                        inner_context,
                        inner_visible,
                    );
                    if self.deepest - level > height {
                        self.heights.insert(addr, self.deepest - level);
                    }
                    self.deepest = self.deepest.max(deepest_before);
                    self.expanding.pop();
                    self.path.pop();
                    let c: Rc<[(Use, u64)]> = c.into_iter().collect();
                    add(counts, &c);
                    if let Some(key) = key {
                        self.keep(addr, key, c);
                    }
                }
                _ => {}
            }
        }
    }

    /// Keeps the counts `c` of an instance of the definition `addr`, unless
    /// its instances seldom repeat
    fn keep(&mut self, addr: usize, key: KeptKey, c: Rc<[(Use, u64)]>) {
        /// instances of a definition evaluated before judging how often
        /// they repeat
        const TRIAL: u64 = 4096;
        /// how many entries `memo` holds at most (it is emptied then)
        const MAX_ENTRIES: usize = 1 << 22;
        let (found, missing) = self.memo_use.entry(addr).or_default();
        *missing += 1;
        if *missing >= TRIAL && *found * 16 < *missing {
            self.unkept.insert(addr);
            return;
        }
        if self.memo.len() >= MAX_ENTRIES {
            self.memo.clear();
        }
        self.memo.insert(key, c);
    }

    /// The number of the error `e`, met at the instance `name` of the
    /// current level
    fn error_number(&mut self, e: SelectError, name: &str) -> usize {
        let text = e.to_string();
        if let Some(&n) = self.error_numbers.get(&text) {
            return n;
        }
        let n = self.errors.len();
        let path = self.path.iter().copied().chain(once(name)).join(".");
        self.errors.push((e, path));
        self.error_numbers.insert(text, n);
        n
    }
}

fn add(counts: &mut Counts, more: &[(Use, u64)]) {
    for &(u, n) in more {
        *counts.entry(u).or_default() += n;
    }
}
