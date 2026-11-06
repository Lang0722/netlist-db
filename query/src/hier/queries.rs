//! The queries of the hierarchy, and the paging and name matching that they use.
use super::*;

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

/// Case-insensitive glob match (`*`, `?`) of the whole `text`.
pub(super) fn glob(pattern: &[u8], text: &[u8]) -> bool {
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
pub(super) fn distance(a: &[u8], b: &[u8]) -> usize {
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
