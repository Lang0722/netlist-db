//! The queries of the parameters: what the user asks, and how the answer reads.
use super::*;

impl Netlist {
    /// The parameter `p` of an element line is written as `key=value` (a
    /// `=` before its text, also before a quote), not as a bare token.
    fn after_eq(&self, p: &ParamDef) -> bool {
        p.text.is_some_and(|t| {
            after_eq(
                self.files[t.file as usize].text.as_bytes(),
                t.start as usize,
            )
        })
    }

    /// The token right after the text of `p` on its statement.
    fn word_after(&self, p: &ParamDef) -> Option<&str> {
        let t = p.text?;
        let text = &self.files[t.file as usize].text;
        let b = text.as_bytes();
        let mut at = t.end as usize;
        if b.get(at) == Some(&b'\'') {
            at += 1;
        }
        let start = next_token(b, at)?;
        let end = b[start..]
            .iter()
            .position(|&c| c.is_ascii_whitespace() || c == b'$')
            .map_or(b.len(), |n| start + n);
        text.get(start..end)
    }

    /// The token of the R, C or L line `e` that starts first at or after
    /// byte `at`, before the first `key=value`: its start and end. A quoted
    /// expression is one token.
    fn token_from(&self, e: &Element, at: usize) -> Option<(usize, usize)> {
        let b = self.files[e.loc.file as usize].text.as_bytes();
        let start = next_token(b, at)?;
        let end = if b[start] == b'\'' {
            b[start + 1..]
                .iter()
                .position(|&c| c == b'\'')
                .map_or(b.len(), |n| start + n + 2)
        } else {
            b[start..]
                .iter()
                .position(|&c| c.is_ascii_whitespace() || matches!(c, b'$' | b'='))
                .map_or(b.len(), |n| start + n)
        };
        // `r=5k`, `r = 5k`: the keywords start
        if end == start
            || b.get(end) == Some(&b'=')
            || next_token(b, end).is_some_and(|n| b[n] == b'=')
        {
            return None;
        }
        Some((start, end))
    }

    /// The tokens of the R, C or L line `e` from byte `at`, before the
    /// first `key=value`, at most 4 (`R1 a b rval 0.004 1e-5 r=5k` gives
    /// `0.004`, `1e-5` after `rval`).
    fn words_from(&self, e: &Element, mut at: usize) -> Vec<&str> {
        let text = &self.files[e.loc.file as usize].text;
        let mut out = Vec::new();
        while out.len() < 4
            && let Some((start, end)) = self.token_from(e, at)
        {
            out.push(&text[start..end]);
            at = end;
        }
        out
    }

    /// The end of the model token of the R, C or L line `e`, whose second
    /// node token ends at byte `nodes_end`, when it is the first token
    /// after the nodes (`R1 a b rval 0.004 r=5k`).
    fn model_end(&self, e: &Element, nodes_end: Option<usize>) -> Option<usize> {
        let m = self.syms.name(e.model?);
        let (start, end) = self.token_from(e, nodes_end?)?;
        let text = &self.files[e.loc.file as usize].text;
        text[start..end].eq_ignore_ascii_case(m).then_some(end)
    }

    /// The token of the R, C or L line `e` that starts first at or after
    /// byte `at` when it is a quoted expression (`R1 a b 'rval' r=5k`): its
    /// text inside the quotes, on one line (without the `+` markers of a
    /// text that goes on on a `+` line, as `param_text` gives it; a `$` and
    /// a `*` comment line inside the quotes are kept, see
    /// `crate::lower::one_line`), and the end of the token.
    fn quoted_at(&self, e: &Element, at: usize) -> Option<(Cow<'_, str>, usize)> {
        let (start, end) = self.token_from(e, at)?;
        let token = &self.files[e.loc.file as usize].text[start..end];
        let inner = token.strip_prefix('\'')?;
        let inner = inner.strip_suffix('\'').unwrap_or(inner);
        Some((one_line(inner, true), end))
    }

    /// The text of `p` when it is one bare name (not quoted, not a
    /// number).
    fn plain_word(&self, p: &ParamDef) -> Option<&str> {
        let t = p.text?;
        if p.num.is_some() {
            return None;
        }
        let text = &self.files[t.file as usize].text;
        if t.start > 0 && text.as_bytes()[t.start as usize - 1] == b'\'' {
            return None;
        }
        let word = text[t.start as usize..t.end as usize].trim();
        matches!(netlist_expr::parse(word), Ok(Expr::Ident(_))).then_some(word)
    }

    /// The `.model` card `m` of the first run that is visible from
    /// definition `def`: in it, in a definition it is written inside, or
    /// at the top level, the nearest one (the rule of the hierarchy
    /// analysis for `X` lines).
    pub(super) fn visible_model(&self, def: DefId, m: Sym) -> Option<&ModelDef> {
        let mut at = Some(def);
        while let Some(d) = at {
            let found = self.defs[d.idx()]
                .models
                .iter()
                .find(|md| md.name == m && md.alter.is_none());
            if found.is_some() {
                return found;
            }
            at = (d != ROOT).then(|| self.defs[d.idx()].parent.unwrap_or(ROOT));
        }
        None
    }

    /// The caveats of `.alter` blocks that set `.option parhier`, or that
    /// redefine a definition of the path `w`, an `X` line of the path or
    /// the element at its end.
    pub(super) fn alter_caveats(&self, w: &Walk) -> Vec<String> {
        let mut out = Vec::new();
        if self.alters.is_empty() {
            return out;
        }
        let parhier = self.syms.get("parhier");
        for p in self.options.iter().filter(|p| Some(p.name) == parhier) {
            if let Some(a) = p.alter {
                out.push(format!(
                    "`.option parhier` is set again by .alter #{a} at {}: this answer uses the first run's parhier ({}), and a later run can take other definitions (golden `alter`: the changes of the `.alter` blocks accumulate over the runs)",
                    loc_str(self, p.loc),
                    match self.parhier {
                        Parhier::Local => "local",
                        Parhier::Global => "global",
                    }
                ));
            }
        }
        out.extend(self.path_alter_caveats(w));
        out
    }

    /// The caveats of `.alter` blocks that redefine a definition of the
    /// path `w`, an `X` line of the path or the element at its end.
    pub(crate) fn path_alter_caveats(&self, w: &Walk) -> Vec<String> {
        let mut out = Vec::new();
        for &d in w.defs.iter().chain(&w.last_binding) {
            out.extend(self.def_alter_caveats(d));
        }
        for &(holder, i) in &w.elems {
            out.extend(self.elem_alter_caveats(holder, i));
        }
        out
    }

    /// The caveat of each `.alter` block that defines `d` again.
    pub(crate) fn def_alter_caveats(&self, d: DefId) -> Vec<String> {
        if self.alters.is_empty() || d == ROOT {
            return Vec::new();
        }
        let def = &self.defs[d.idx()];
        let parent = def.parent.unwrap_or(ROOT);
        self.defs[parent.idx()]
            .nested
            .iter()
            .map(|&other| &self.defs[other.idx()])
            .filter(|o| o.name == def.name)
            .filter_map(|o| {
                o.alter.map(|a| {
                    format!(
                        "subckt `{}` is defined again by .alter #{a} at {}: this answer describes the first run",
                        self.def_display(d),
                        loc_str(self, o.loc)
                    )
                })
            })
            .collect()
    }

    /// The caveat of each `.alter` line that changes element `i` of
    /// `holder`.
    pub(crate) fn elem_alter_caveats(&self, holder: DefId, i: ElemIdx) -> Vec<String> {
        if self.alters.is_empty() {
            return Vec::new();
        }
        let elements = &self.defs[holder.idx()].elements;
        let name = elements[i].name;
        elements
            .iter()
            .filter(|o| o.name == name)
            .filter_map(|o| {
                o.alter.map(|a| {
                    format!(
                        "`{}` is changed by .alter #{a} at {}: this answer describes the first run (golden `alter`: the changes of the `.alter` blocks accumulate over the runs)",
                        self.syms.name(name),
                        loc_str(self, o.loc)
                    )
                })
            })
            .collect()
    }

    /// The scopes of the instance path `scope_path` (`""`: the top level,
    /// also in a bare netlist), and the caveats of the path.
    fn param_scopes(&self, scope_path: &str) -> Result<(Vec<Scope>, Vec<String>), QueryError> {
        let p = scope_path.trim();
        if (p.is_empty() || p == ".") && self.root == Root::None {
            let top = Scope {
                def: ROOT,
                x: None,
                path: String::new(),
            };
            return Ok((vec![top], Vec::new()));
        }
        let w = walk(self, p, Want::Instance)?;
        if let Some(&(holder, i)) = w.elems.last() {
            let e = &self.defs[holder.idx()].elements[i];
            if e.kind != ElemKind::Instance {
                return Err(QueryError::BadArgument(format!(
                    "`{}` is a {}, not a subckt instance: give the path of the instance that holds it (or use resolve_element for its parameters)",
                    w.path,
                    e.kind.name()
                )));
            }
            if w.last_binding.is_none() {
                return Err(QueryError::BadArgument(format!(
                    "`{}` calls subckt `{}`, which has no visible definition on this path (diagnostic `unresolved_subckt`): it has no scope",
                    w.path,
                    e.callee.map_or("", |c| self.syms.name(c))
                )));
            }
        }
        let mut caveats = w.caveats.clone();
        caveats.extend(self.path_caveats(&w));
        caveats.extend(self.alter_caveats(&w));
        Ok((scopes(self, &w, true), caveats))
    }

    /// The value of parameter `name` (any letter case) as seen in the scope
    /// of the instance `scope_path` (`""`: the top level), under
    /// `.option parhier`, with its chain. `temper` is the circuit
    /// temperature. A name with no definition on the path is `NotFound`,
    /// with close names; a value that cannot be computed is `unresolved`
    /// with the reason.
    pub fn resolve_param(&self, scope_path: &str, name: &str) -> Result<Resolved, QueryError> {
        on_big_stack(|| self.resolve_param_here(scope_path, name))?
    }

    fn resolve_param_here(&self, scope_path: &str, name: &str) -> Result<Resolved, QueryError> {
        let wanted = name.trim();
        if wanted.is_empty() {
            return Err(QueryError::BadArgument(
                "the parameter name is empty".to_owned(),
            ));
        }
        let (scopes, mut path_caveats) = self.param_scopes(scope_path)?;
        let mut r = Resolver::new(self, scopes);
        let k = r.scopes.len() - 1;
        path_caveats.extend(r.path_m_caveats(k));
        let (shown, v) = if wanted.eq_ignore_ascii_case("temper") {
            ("temper".to_owned(), r.temperature())
        } else {
            let sym = self
                .syms
                .get(wanted)
                .filter(|&s| r.find(k, s).is_some())
                .ok_or_else(|| QueryError::NotFound {
                    what: "parameter".to_owned(),
                    name: wanted.to_owned(),
                    suggestions: r.visible_names(k, wanted),
                })?;
            (self.syms.name(sym).to_owned(), r.resolve(k, sym))
        };
        let frame = Frame {
            deps: v.node.into_iter().collect(),
            caveats: path_caveats,
            ..Frame::default()
        };
        let mut res = r.answer(shown, v.value, None, &frame);
        cap_caveats(&mut res.caveats);
        Ok(res)
    }

    /// The parameters of the element at `path` (a device or an `X` line),
    /// each resolved in the scope of the definition that holds it (the
    /// overrides of an `X` line in the scope of the instance it makes,
    /// golden `ovr_scope_local`): the R/C/L value as `r`/`c`/`l` (the
    /// element's own value, times its `scale=`, golden `src_delay_scale`,
    /// `elem_scale_cl`; the `.op` listing prints R/m under an instance with
    /// `m`), the `dc`, function, `ac` and `acphase` of a source, and every
    /// `key=value`. An `X`-line override that a definition above the new
    /// instance beats (GLOBAL, golden `nested_global`) keeps its own value,
    /// with a caveat that gives the value the instance uses.
    ///
    /// R/C/L: a word after the nodes is a model only when a `.model` card
    /// of that name is visible (golden `res_model`). With no such card,
    /// `R1 a 0 rval 0.004` reads as the value `rval` and TC1 0.004 (golden
    /// `res_tc_positional`, `rcl_param_tc`), and `L1 a b lval lmod` as the
    /// value `lval` and the model `lmod` (golden `ind_value_model`), each
    /// with a caveat; an `r=` after such a name is not used (golden
    /// `rcl_kw_after_more`). TC1/TC2 at a temperature other than TNOM give
    /// a caveat with the temperature-adjusted value that `.op` prints.
    ///
    /// Sources: with no DC value, `dc` is the value of the function at
    /// t=0 (golden `src_fn_nodc`, `src_sin_t0`, `src_delay_scale`, `pwl_*`:
    /// PULSE, EXP, SFFM their first argument, SIN offset + amplitude ×
    /// sin(phase), PWL its value at t=0, AM 0), with a caveat; other
    /// functions are unresolved. With no DC value and no function it is 0.
    /// An AC magnitude with no phase has the phase 0
    /// (`src_ac_default_phase`).
    pub fn resolve_element(&self, path: &str) -> Result<ElementParams, QueryError> {
        on_big_stack(|| self.resolve_element_here(path).map(|(ep, _)| ep))?
    }

    /// [`Netlist::resolve_element`], and the work of the call (see
    /// [`Resolver::work`]).
    #[cfg(test)]
    pub(crate) fn resolve_element_work(
        &self,
        path: &str,
    ) -> Result<(ElementParams, u64), QueryError> {
        on_big_stack(|| self.resolve_element_here(path))?
    }

    fn resolve_element_here(&self, path: &str) -> Result<(ElementParams, u64), QueryError> {
        let w = walk(self, path, Want::Element)?;
        let Some(&(holder, i)) = w.elems.last() else {
            return Err(QueryError::BadArgument(
                "the path is empty: it names the root, not an element".to_owned(),
            ));
        };
        let d = &self.defs[holder.idx()];
        let e = &d.elements[i];
        let mut caveats = w.caveats.clone();
        match e.kind {
            ElemKind::Other(_) => caveats.push(
                "element type not parsed: its node list and its parameters are not trusted"
                    .to_owned(),
            ),
            ElemKind::Instance if w.last_binding.is_none() => caveats.push(format!(
                "no definition of subckt `{}` is visible on this path (diagnostic `unresolved_subckt`)",
                e.callee.map_or("", |c| self.syms.name(c))
            )),
            _ => {}
        }
        caveats.extend(self.path_caveats(&w));
        caveats.extend(self.alter_caveats(&w));
        // an `X` line's overrides are evaluated in the scope of the
        // instance it makes (one level below the line)
        let binding = e.kind == ElemKind::Instance && w.last_binding.is_some();
        let mut r = Resolver::new(self, scopes(self, &w, binding));
        let k = r.scopes.len() - 1 - usize::from(binding);
        caveats.extend(r.path_m_caveats(k));
        let params = d.element_params(e);
        let (items, model, more) = match e.kind {
            ElemKind::Resistor | ElemKind::Capacitor | ElemKind::Inductor => {
                self.rcl_items(holder, i)
            }
            ElemKind::Vsource | ElemKind::Isource => (self.source_items(params), None, Vec::new()),
            ElemKind::Instance => (self.x_items(params, binding), e.model, Vec::new()),
            ElemKind::Other(_) => {
                let items = params
                    .iter()
                    .map(|p| Item {
                        src: Src::Fail(
                            "element type not parsed: its parameters are not resolved".to_owned(),
                            Some(p),
                        ),
                        ..Item::param(self, p)
                    })
                    .collect();
                (items, None, Vec::new())
            }
            _ => (
                params.iter().map(|p| Item::param(self, p)).collect(),
                e.model,
                Vec::new(),
            ),
        };
        caveats.extend(more);
        let total = items.len();
        let syms: Vec<Option<Sym>> = items.iter().map(|it| it.sym).collect();
        let mut out: Vec<Resolved> = items
            .into_iter()
            .take(MAX_LIMIT)
            .map(|it| {
                // an override that a definition above the new instance
                // beats (GLOBAL)
                let shadow = match (&it.src, it.sym) {
                    (Src::Param(p), Some(s)) if binding && Some(s) != r.m => {
                        r.shadowed(k + 1, s, p)
                    }
                    _ => Vec::new(),
                };
                let mut res = r.resolve_item(it, k, &w.path);
                // the override text and the winning value can share a
                // caveat (an `.alter` or a sweep of a top-level `.param`)
                let known: HashSet<String> = res.caveats.iter().cloned().collect();
                res.caveats
                    .extend(shadow.into_iter().filter(|c| !known.contains(c)));
                res
            })
            .collect();
        self.value_caveats(&mut r, e.kind, &syms, &mut out);
        // `.dc v1 0 1 0.1`: the DC value of a top-level source is swept
        if matches!(e.kind, ElemKind::Vsource | ElemKind::Isource)
            && holder == ROOT
            && !self.swept.is_empty()
            && let Some(dc) = out.iter_mut().find(|p| p.name == "dc")
        {
            dc.caveats.extend(self.swept_caveats(e.name));
        }
        for p in &mut out {
            // `1e300 scale=1e10`, a PWL of huge values
            if p.value.is_some_and(|v| !v.is_finite()) {
                p.value = None;
                p.unresolved = Some(OUT_OF_RANGE.to_owned());
            }
        }
        for p in &mut out {
            cap_caveats(&mut p.caveats);
        }
        cap_caveats(&mut caveats);
        let ep = ElementParams {
            path: w.path,
            kind: e.kind.name().to_owned(),
            model: model.map(|m| self.syms.name(m).to_owned()),
            params: out,
            params_total: total,
            caveats,
        };
        Ok((ep, r.work.get()))
    }

    /// The caveats that depend on the values of an element's parameters
    /// (`syms[i]` is the name of `out[i]`): TC1/TC2 at another temperature,
    /// `m` <= 0, `.option scale`.
    fn value_caveats(
        &self,
        r: &mut Resolver<'_>,
        kind: ElemKind,
        syms: &[Option<Sym>],
        out: &mut [Resolved],
    ) {
        let get = |name: &str| self.syms.get(name);
        let n = out.len();
        let at = |s: Option<Sym>| {
            s.and_then(|s| syms.iter().position(|&t| t == Some(s)))
                .filter(|&i| i < n)
        };
        let key = match kind {
            ElemKind::Resistor => Some("r"),
            ElemKind::Capacitor => Some("c"),
            ElemKind::Inductor => Some("l"),
            _ => None,
        };
        if let Some(key) = key
            && let Some(vi) = at(get(key))
        {
            // the element's `scale=` multiplies the value: `.option list`
            // `res eff`, `cap eff`, `ind eff` and the `.op` values (golden
            // `src_delay_scale`, `elem_scale_cl`)
            if let Some(si) = at(get("scale"))
                && out[vi].value.is_some()
            {
                match out[si].value {
                    Some(s) if s != 1.0 => {
                        let x = out[vi].value.expect("checked");
                        out[vi].value = Some(x * s);
                        out[vi].caveats.push(format!(
                            "the line's `scale={s}` multiplies the value as written ({}): HSPICE's effective value is {} (golden `src_delay_scale`, `elem_scale_cl`)",
                            fmt_num(x),
                            fmt_num(x * s)
                        ));
                        let more: Vec<ChainStep> = out[si].chain.clone();
                        let room = MAX_LIMIT.saturating_sub(out[vi].chain.len());
                        out[vi].chain.extend(more.into_iter().take(room));
                    }
                    Some(_) => {}
                    None => {
                        let why = out[si].unresolved.clone().unwrap_or_default();
                        out[vi].value = None;
                        out[vi].unresolved = Some(format!(
                            "HSPICE multiplies the value by the line's `scale=`, which has no value: {why}"
                        ));
                    }
                }
            }
            let tc = |name: &str| at(get(name)).map(|i| &out[i]);
            let (tc1, tc2, dtemp) = (tc("tc1"), tc("tc2"), tc("dtemp"));
            let set = |t: Option<&Resolved>| t.is_some_and(|t| t.value != Some(0.0));
            if set(tc1) || set(tc2) {
                let temp = r.temperature().value;
                let tn = r.tnom();
                // `dtemp=` on the line: the element is that much warmer
                let (dtemp, shown) = match dtemp.map(|d| d.value) {
                    None => (Some(0.0), String::new()),
                    Some(Some(d)) => (Some(d), format!(" + DTEMP {d}")),
                    Some(None) => (None, " + DTEMP (not resolved)".to_owned()),
                };
                if let Ok(t) = temp
                    && dtemp.is_none_or(|d| t + d != tn)
                {
                    let coef = |t: Option<&Resolved>| t.map_or(Some(0.0), |t| t.value);
                    let adjusted = match (out[vi].value, coef(tc1), coef(tc2), dtemp) {
                        (Some(x), Some(a), Some(b), Some(d)) => {
                            let dt = t + d - tn;
                            format!(" = {}", fmt_num(x * (1.0 + a * dt + b * dt * dt)))
                        }
                        _ => String::new(),
                    };
                    out[vi].caveats.push(format!(
                        "TC1/TC2 are set and the temperature is {t}{shown} (TNOM {tn}): HSPICE's `.op` uses {key}*(1 + TC1*dT + TC2*dT^2){adjusted} at that temperature; this value is the one at TNOM (golden `res_tc_positional`, `cl_tc_positional`, `rcl_param_tc`; TNOM from `.option tnom`, `dtemp=` and `scale=`: golden `tc_tnom_dtemp`)"
                    ));
                    let later = self
                        .temp_alter_caveats()
                        .into_iter()
                        .chain(self.option_alter_caveats("tnom"));
                    out[vi].caveats.extend(later);
                }
                // a sweep of the temperature changes the value, also from TNOM
                let swept = ["temp", "temper"]
                    .iter()
                    .filter_map(|t| self.syms.get(t))
                    .flat_map(|t| self.swept_caveats(t));
                out[vi].caveats.extend(swept);
            }
        }
        if !matches!(
            kind,
            ElemKind::Vsource | ElemKind::Isource | ElemKind::Other(_)
        ) && let Some(i) = at(get("m"))
            && out[i].value.is_some_and(|m| m <= 0.0)
        {
            out[i].caveats.push(
                "HSPICE rejects a multiplier of zero or less on an instance path (golden `elem_m_zero`, `hier_m_zero`, `m_expr_zero`; diagnostic `m_not_positive`)"
                    .to_owned(),
            );
        }
        let scale = get("scale").and_then(|s| {
            self.options
                .iter()
                .rfind(|p| p.name == s && p.alter.is_none())
        });
        if let Some(p) = scale
            && p.num != Some(1.0)
            && !matches!(
                kind,
                ElemKind::Vsource | ElemKind::Isource | ElemKind::Instance | ElemKind::Other(_)
            )
        {
            for name in GEOMETRIC {
                // an inductor's `l` is its inductance, not a length
                if kind == ElemKind::Inductor && name == "l" {
                    continue;
                }
                if let Some(i) = at(get(name)) {
                    out[i].caveats.push(format!(
                        "`.option scale` is set ({}): HSPICE multiplies this geometric parameter by it; the value here is as written",
                        loc_str(self, p.loc)
                    ));
                    out[i].caveats.extend(self.option_alter_caveats("scale"));
                }
            }
        }
    }

    /// The caveats of the analyses of the first run that sweep the name
    /// `n` (a parameter, a source, `temp`): the answer gives the value of
    /// the deck, the nominal one, which is the `.op` value (golden
    /// `sweep_op`).
    pub(super) fn swept_caveats(&self, n: Sym) -> Vec<String> {
        // `swept` is sorted by name
        let at = self.swept.partition_point(|s| s.name < n);
        self.swept[at..]
            .iter()
            .take_while(|s| s.name == n)
            .map(|s| {
                let via = s.data.map_or(String::new(), |(d, at)| {
                    format!(
                        " (a parameter of the `.data` block `{}` at {})",
                        self.syms.name(d),
                        loc_str(self, at)
                    )
                });
                format!(
                    "`{}` is swept by .{} at {}{via}; this is the nominal value, the one of the deck, which is the `.op` value",
                    self.syms.name(n),
                    s.cmd,
                    loc_str(self, s.loc)
                )
            })
            .collect()
    }

    /// The caveats of the `.temp` lines of `.alter` blocks: they set the
    /// temperature of the later runs.
    pub(super) fn temp_alter_caveats(&self) -> Vec<String> {
        self.temps
            .iter()
            .filter_map(|p| {
                p.alter.map(|a| {
                    format!(
                        "the temperature is set again by .alter #{a} at {}: this is the first run's temperature",
                        loc_str(self, p.loc)
                    )
                })
            })
            .collect()
    }

    /// The caveats of the `.option <name>` items of `.alter` blocks.
    fn option_alter_caveats(&self, name: &str) -> Vec<String> {
        let Some(sym) = self.syms.get(name) else {
            return Vec::new();
        };
        self.options
            .iter()
            .filter(|p| p.name == sym)
            .filter_map(|p| {
                p.alter.map(|a| {
                    format!(
                        "`.option {name}` is set again by .alter #{a} at {}: this is the first run's value",
                        loc_str(self, p.loc)
                    )
                })
            })
            .collect()
    }

    /// The parameters of an `X` line. An override is evaluated in the scope
    /// of the instance that the line makes (`binding`: the line binds to a
    /// definition on this path), with the overridden name itself from the
    /// line's scope (see [`Text`]); `m` in the line's scope (golden
    /// `ovr_scope_more_local`). With no binding, the scope of an override
    /// is not known: only a number has a value.
    fn x_items<'n>(&'n self, params: &'n [ParamDef], binding: bool) -> Vec<Item<'n>> {
        let m = self.syms.get("m");
        params
            .iter()
            .map(|p| {
                let item = Item::param(self, p);
                if Some(p.name) == m || p.num.is_some() {
                    item
                } else if binding {
                    Item {
                        child: true,
                        ..item
                    }
                } else {
                    Item {
                        src: Src::Fail(
                            "the subckt that the line calls has no visible definition on this path, and HSPICE evaluates an override in the scope of the new instance: not resolved".to_owned(),
                            Some(p),
                        ),
                        ..item
                    }
                }
            })
            .collect()
    }

    /// The parameters of a V/I line: `dc` first (from the function at t=0,
    /// or 0, when the line has none), then the others in order, with
    /// `acphase` 0 after an `ac` with no phase.
    fn source_items<'n>(&'n self, params: &'n [ParamDef]) -> Vec<Item<'n>> {
        let sym = |n: &str| self.syms.get(n);
        let (dc, ac, acphase, td) = (sym("dc"), sym("ac"), sym("acphase"), sym("td"));
        let is_func = |p: &ParamDef| {
            let n = self.syms.name(p.name);
            SOURCE_FUNCS.iter().any(|f| f.eq_ignore_ascii_case(n)) && !self.after_eq(p)
        };
        let func = params.iter().find(|p| is_func(p));
        let mut items = Vec::with_capacity(params.len() + 2);
        match (params.iter().find(|p| Some(p.name) == dc), func) {
            (Some(p), _) => items.push(Item::param(self, p)),
            (None, Some(f)) => {
                let td = params.iter().find(|p| Some(p.name) == td);
                items.push(Item::named(self, "dc", Src::SourceT0(f, td)));
            }
            (None, None) => items.push(Item {
                caveats: vec![
                    "the line has no DC value and no function: its DC value is 0, as in SPICE (not probed)"
                        .to_owned(),
                ],
                ..Item::named(self, "dc", Src::Builtin(0.0, "0 (no DC value on the line)"))
            }),
        }
        let has_phase = params.iter().any(|p| Some(p.name) == acphase);
        let has_dc = params.iter().any(|p| Some(p.name) == dc);
        for p in params {
            if Some(p.name) == dc {
                continue;
            }
            if is_func(p) {
                let fname = self.syms.name(p.name);
                let why = if has_dc {
                    format!(
                        "a {fname} waveform has no single value: `.op` uses the DC value of the line (golden `src_forms`)"
                    )
                } else {
                    format!(
                        "a {fname} waveform has no single value: with no DC value on the line, `dc` is its value at t=0"
                    )
                };
                items.push(Item {
                    src: Src::Fail(why, Some(p)),
                    ..Item::param(self, p)
                });
                continue;
            }
            items.push(Item::param(self, p));
            if Some(p.name) == ac && !has_phase {
                // golden `src_ac_default_phase`
                items.push(Item::named(
                    self,
                    "acphase",
                    Src::Builtin(0.0, "0 (no AC phase on the line: the default)"),
                ));
            }
        }
        // the parts of the source have fixed names, whatever the
        // deck's spelling (`DC 1`)
        for it in &mut items {
            for (name, sym) in [("dc", dc), ("ac", ac), ("acphase", acphase)] {
                if it.sym.is_some() && it.sym == sym {
                    name.clone_into(&mut it.name);
                }
            }
        }
        items
    }

    /// For R, C or L line `i` of `holder`, as [`Netlist::resolve_element`]
    /// reads it: the reason HSPICE rejects the line (`R1 1 0 rval rmod`,
    /// golden `res_name_model`), and its visible model card. The reading
    /// does not depend on the instance path (the load checks of
    /// `crate::rejects`).
    pub(crate) fn rcl_check(
        &self,
        holder: DefId,
        i: ElemIdx,
    ) -> (Option<String>, Option<&ModelDef>) {
        let (items, model, _) = self.rcl_items(holder, i);
        let why = items.into_iter().find_map(|it| match it.src {
            Src::Fail(why, _) if why.contains(REJECTS) => Some(why),
            _ => None,
        });
        (why, model.and_then(|m| self.visible_model(holder, m)))
    }

    /// The model of element `i` of `holder` as HSPICE reads the line: for
    /// an R, C or L line with a model word, the reading of
    /// [`Netlist::rcl_reading`] (`R1 1 0 rval 0.004` has no model when no
    /// `.model rval` is visible: `rval` is the value, golden
    /// `rcl_param_tc`); else the model word of the line.
    pub(crate) fn elem_model(&self, holder: DefId, i: ElemIdx) -> Option<Sym> {
        let e = &self.defs[holder.idx()].elements[i];
        match e.kind {
            ElemKind::Resistor | ElemKind::Capacitor | ElemKind::Inductor if e.model.is_some() => {
                self.rcl_items(holder, i).1
            }
            _ => e.model,
        }
    }

    /// For R, C or L line `i` of `holder`: the model and the parameters
    /// (name, text) as HSPICE reads the tokens after the nodes, as
    /// [`Netlist::resolve_element`] reads them (`R1 1 0 rval 0.004` is
    /// `r=rval`, `tc1=0.004` with no `.model rval`, golden
    /// `rcl_param_tc`), and the reasons the line has no value or HSPICE
    /// rejects it. `None` for another type.
    pub(crate) fn rcl_reading(
        &self,
        holder: DefId,
        i: ElemIdx,
    ) -> Option<(Option<Sym>, Vec<NameText>, Vec<String>)> {
        let e = &self.defs[holder.idx()].elements[i];
        if !matches!(
            e.kind,
            ElemKind::Resistor | ElemKind::Capacitor | ElemKind::Inductor
        ) {
            return None;
        }
        let (items, model, _) = self.rcl_items(holder, i);
        let mut params = Vec::new();
        let mut why = Vec::new();
        for it in items {
            let text = match it.src {
                Src::Param(p) => Some(self.name_text(p).text),
                Src::Word(w, _) => Some(w),
                Src::Fail(reason, p) => {
                    why.push(reason);
                    p.map(|p| self.name_text(p).text)
                }
                Src::Builtin(..) | Src::SourceT0(..) => None,
            };
            if let Some(text) = text {
                params.push(NameText {
                    name: it.name,
                    text,
                });
            }
        }
        Some((model, params, why))
    }

    /// The parameters of an R, C or L line, read as HSPICE reads the tokens
    /// after the nodes (see [`Netlist::resolve_element`]), the model, and
    /// caveats on the element. The value is named `r`, `c` or `l`.
    pub(super) fn rcl_items<'n>(
        &'n self,
        holder: DefId,
        i: ElemIdx,
    ) -> (Vec<Item<'n>>, Option<Sym>, Vec<String>) {
        let d = &self.defs[holder.idx()];
        let e = &d.elements[i];
        let nodes_end = d.nodes_end(i);
        let params = d.element_params(e);
        let key = match e.kind {
            ElemKind::Resistor => "r",
            ElemKind::Capacitor => "c",
            _ => "l",
        };
        let ksym = self.syms.get(key);
        let mut items: Vec<Item<'n>> = params.iter().map(|p| Item::param(self, p)).collect();
        let vi = params.iter().position(|p| Some(p.name) == ksym);
        let mut model = e.model;
        let mut caveats = Vec::new();
        let card = |m: Sym| self.visible_model(holder, m);
        let model_caveat = |md: &ModelDef| {
            let mut c = format!(
                "the element has the model `{}` (`.model` at {}): some parameters of the card (a scale, a TC) can change the effective value, and this value does not use them (resolve_model gives the card's parameters)",
                self.syms.name(md.name),
                loc_str(self, md.loc)
            );
            for o in self.defs[ROOT.idx()]
                .models
                .iter()
                .filter(|o| o.name == md.name)
            {
                if let Some(a) = o.alter {
                    c.push_str(&format!(
                        ". `.alter #{a}` at {} defines the model again (this is the first run)",
                        loc_str(self, o.loc)
                    ));
                }
            }
            c
        };
        let no_card = |m: &str| format!("no `.model {m}` card is visible from the line");
        // `R1 a b 'rval' r=5k`: a quoted expression in the place of the
        // value, which the `key=` value replaced in `params`: right after
        // the nodes, or after the name of a visible model card that comes
        // first (`R1 a b rmod 'rval' r=5k`); an L line can also have its
        // model after it
        let model_end = self.model_end(e, nodes_end);
        let quoted =
            vi.filter(|&vi| self.after_eq(&params[vi]))
                .and_then(|_| match (e.model, model_end) {
                    // a first name with no visible card is the value (golden
                    // `rcl_kw_after_more`)
                    (Some(m), Some(at)) => card(m).and_then(|_| self.quoted_at(e, at)),
                    (Some(_), None) if e.kind != ElemKind::Inductor => None,
                    _ => self.quoted_at(e, nodes_end?),
                });
        if let (Some(vi), Some((q, q_end))) = (vi, quoted) {
            let mut rest = self.words_from(e, q_end);
            let mut more = Vec::new();
            if let Some(m) = e.model {
                let mname = self.syms.name(m);
                if model_end.is_some() {
                    more.push(format!(
                        "`{mname}` before it is the model, as in `R1 1 0 rmod 2k` (golden `res_model`); a quoted value after a model name, then the `{key}=`, was not probed"
                    ));
                } else {
                    // `L1 a b 'lval' lmod l=5n`: the value, then the model
                    // (golden `ind_value_model`)
                    if rest.first().is_some_and(|w| w.eq_ignore_ascii_case(mname)) {
                        rest.remove(0);
                    }
                    more.push(format!(
                        "`{mname}` after it is the model, as in `L1 1 2 1n lmod` (golden `ind_value_model`); with a quoted value this was not probed"
                    ));
                }
                caveats.push(card(m).map_or_else(|| no_card(mname), model_caveat));
            }
            let mut why = vec![self.before_key_why(&params[vi], &q, &rest, e.kind, true)];
            why.extend(more);
            items = self.name_before_key(params, vi, &q, &rest, why);
            for it in &mut items {
                if it.sym.is_some() && it.sym == ksym {
                    key.clone_into(&mut it.name);
                }
            }
            return (items, model, caveats);
        }
        match (e.model, vi) {
            (Some(m), Some(vi)) if card(m).is_some() => {
                let md = card(m).expect("checked");
                items[vi].caveats.push(model_caveat(md));
            }
            (Some(m), None) => {
                caveats.push(no_card(self.syms.name(m)));
                items.insert(
                    0,
                    Item::named(
                        self,
                        key,
                        Src::Fail("the line has no value".to_owned(), None),
                    ),
                );
            }
            (Some(m), Some(vi)) => {
                let v = &params[vi];
                let mname = self.syms.name(m);
                let word = self.plain_word(v);
                let word_card = word.and_then(|w| self.syms.get(w)).and_then(card);
                // `R1 1 0 rval rmod`, `C1 1 0 cval cmod`: a value, then a
                // model name
                let name_model = |w: &str| {
                    if e.kind == ElemKind::Resistor {
                        format!(
                            "`{mname}` is followed by the model name `{w}`: HSPICE reads `{mname}` as a third node and {REJECTS} (golden `res_value_model`, `res_name_model`)"
                        )
                    } else {
                        format!(
                            "`{mname}` is followed by the model name `{w}`: HSPICE does not read `{mname}` as the value; it uses a capacitance of 0 and warns (golden `cap_value_model`, `cap_name_model`)"
                        )
                    }
                };
                if self.after_eq(v) && model_end.is_none() {
                    // `L1 a b 1n lmod l=5n`: the model comes after a number
                    // value, which `l=` replaces (golden `elem_dup_kw`)
                    caveats.push(no_card(mname));
                } else if self.after_eq(v) {
                    // `r6 6 0 rval r=5k`: HSPICE uses the name as the value
                    // and not the `r=` after it (golden `rcl_kw_after_more`,
                    // `rcl_tc_kw_after`)
                    let rest = model_end.map_or_else(Vec::new, |at| self.words_from(e, at));
                    let first = rest
                        .first()
                        .copied()
                        .filter(|w| matches!(netlist_expr::parse(w), Ok(Expr::Ident(_))));
                    let first_card = first.and_then(|w| self.syms.get(w)).and_then(card);
                    model = None;
                    match (first, first_card) {
                        (Some(w), Some(md)) if e.kind != ElemKind::Inductor => {
                            model = Some(md.name);
                            items[vi] = Item {
                                src: Src::Fail(name_model(w), Some(v)),
                                ..Item::param(self, v)
                            };
                        }
                        (Some(w), md) if e.kind == ElemKind::Inductor => {
                            // `L1 a b lval lmod l=5n`: the value, then the
                            // model (golden `ind_value_model`)
                            model = md.map(|md| md.name);
                            let more = format!(
                                "`{w}` after it is the model, as in `L1 1 2 1n lmod` (golden `ind_value_model`)"
                            );
                            let tcs = &rest[1..];
                            let why = self.before_key_why(v, mname, tcs, e.kind, false);
                            items = self.name_before_key(params, vi, mname, tcs, vec![why, more]);
                            match md {
                                Some(md) => caveats.push(model_caveat(md)),
                                None => caveats.push(no_card(w)),
                            }
                        }
                        _ => {
                            let why = self.before_key_why(v, mname, &rest, e.kind, false);
                            items = self.name_before_key(params, vi, mname, &rest, vec![why]);
                        }
                    }
                } else if self
                    .word_after(v)
                    .is_some_and(|w| w.eq_ignore_ascii_case(mname))
                {
                    // `L1 a b 1n lmod`: the value, then the model
                    caveats.push(no_card(mname));
                } else if e.kind != ElemKind::Inductor
                    && let Some(md) = word_card
                {
                    let w = word.expect("a card has a word");
                    model = Some(md.name);
                    items[vi] = Item {
                        src: Src::Fail(name_model(w), Some(v)),
                        ..Item::param(self, v)
                    };
                } else if e.kind == ElemKind::Inductor
                    && let Some(word) = word
                {
                    // `L1 a b lval lmod`: the value, then the model (golden
                    // `ind_value_model`)
                    items[vi] = Item {
                        src: Src::Word(mname.to_owned(), v.loc),
                        caveats: vec![format!(
                            "`{mname}` names no visible `.model` card, so it is read as the value and `{word}` as the model, as in `L1 1 2 1n lmod` (golden `ind_value_model`; with a parameter name as the value, golden `ind_name_model`)"
                        )],
                        ..Item::param(self, v)
                    };
                    model = self.syms.get(word);
                    match model.and_then(card) {
                        Some(md) => items[vi].caveats.push(model_caveat(md)),
                        None => caveats.push(no_card(word)),
                    }
                } else {
                    model = None;
                    items = self.shifted_tc(params, vi, mname);
                }
            }
            (None, Some(vi)) => {
                let v = &params[vi];
                let md = (!self.after_eq(v))
                    .then(|| self.plain_word(v))
                    .flatten()
                    .and_then(|w| self.syms.get(w))
                    .and_then(card);
                if let Some(md) = md {
                    // `R1 a b rmod`: one bare name that names a visible
                    // model is the model (golden `res_model`)
                    model = Some(md.name);
                    let name = self.syms.name(md.name);
                    items[vi].src = Src::Fail(
                        format!(
                            "`{name}` is a model (`.model {name} {}` at {}): the value comes from the model card, which this query does not read (resolve_model gives the card's parameters)",
                            self.syms.name(md.kind),
                            loc_str(self, md.loc)
                        ),
                        Some(v),
                    );
                }
            }
            (None, None) => items.insert(
                0,
                Item::named(
                    self,
                    key,
                    Src::Fail("the line has no value".to_owned(), None),
                ),
            ),
        }
        // the value has a fixed name, whatever the deck's spelling of
        // `r=` or of a `.param R`
        for it in &mut items {
            if it.sym.is_some() && it.sym == ksym {
                key.clone_into(&mut it.name);
            }
        }
        (items, model, caveats)
    }

    /// The caveat on the value `value` (a name that names no visible
    /// `.model` card, or with `quoted`, the text of a quoted expression)
    /// right after the nodes of an R, C or L line, before the `key=` value
    /// `v`, with the words `tcs` between them (see
    /// [`Netlist::name_before_key`]).
    fn before_key_why(
        &self,
        v: &ParamDef,
        value: &str,
        tcs: &[&str],
        kind: ElemKind,
        quoted: bool,
    ) -> String {
        let kv = format!("{}={}", self.syms.name(v.name), param_text(self, v));
        let mut why = if quoted {
            format!(
                "the quoted expression `'{value}'` in the place of the value is the value: HSPICE uses it and not the `{kv}` after it, on R, C and L lines (golden `rcl_quoted_sticky_local`, `rcl_quoted_more`); a number there gives the `{kv}` value (golden `elem_dup_kw`)"
            )
        } else {
            format!(
                "`{value}` names no visible `.model` card, so it is read as the value: HSPICE uses it and not the `{kv}` after it (golden `rcl_kw_after_more`: `r6 6 0 rval r=5k` gives 2k; a number before `r=` gives the `r=` value, golden `elem_dup_kw`)"
            )
        };
        if !tcs.is_empty() {
            why.push_str(if quoted {
                ". The numbers after it are TC1 and TC2, as after a name (golden `rcl_tc_kw_after`; not probed after a quoted expression)"
            } else {
                ". The numbers after the name are TC1 and TC2 (golden `rcl_tc_kw_after`)"
            });
        }
        if let Some(w) = tcs.get(2) {
            why.push_str(&format!(
                ". The third number after the value (`{w}`) has no place and is not used"
            ));
        }
        if kind != ElemKind::Resistor && !quoted {
            // golden `rcl_name_first_cl`, `ind_name_model`: C and L with a
            // name, then TC1 and TC2, and no keyword
            why.push_str(&format!(
                ". A name before the `{}=` was probed for R only",
                self.syms.name(v.name)
            ));
        }
        why
    }

    /// The items of `R1 a b rval 0.004 1e-5 r=5k`, where `rval` names no
    /// visible `.model` card and the `key=` value `params[vi]` follows:
    /// HSPICE uses the name as the value and not the `key=` value (golden
    /// `rcl_kw_after_more`: `r6 6 0 rval r=5k` gives 2k, and `r8 8 0 rval
    /// 0.004 r=5k` 2k with TC1 0.004), and the words `tcs` after the name
    /// are TC1, TC2 (golden `rcl_tc_kw_after`). The same for a quoted
    /// expression `R1 a b 'rval' r=5k` (golden `rcl_quoted_sticky_local`,
    /// `rcl_quoted_more`). A keyword `tc1=` or `tc2=` wins over the word in
    /// its place, as in [`Netlist::shifted_tc`]. `value`: the text of the
    /// value; `caveats`: the caveats on it.
    fn name_before_key<'n>(
        &'n self,
        params: &'n [ParamDef],
        vi: usize,
        value: &str,
        tcs: &[&str],
        mut caveats: Vec<String>,
    ) -> Vec<Item<'n>> {
        let (tc1, tc2) = (self.syms.get("tc1"), self.syms.get("tc2"));
        let is_tc = |p: &ParamDef, s: Option<Sym>| s.is_some() && Some(p.name) == s;
        let kw = |s: Option<Sym>| params.iter().rfind(|p| is_tc(p, s) && self.after_eq(p));
        let v = &params[vi];
        let mut out: Vec<Item<'n>> = Vec::with_capacity(params.len() + 2);
        for (idx, p) in params.iter().enumerate() {
            if idx == vi {
                out.push(Item {
                    src: Src::Word(value.to_owned(), v.loc),
                    caveats: std::mem::take(&mut caveats),
                    ..Item::param(self, v)
                });
                for (s, name, w) in [(tc1, "tc1", tcs.first()), (tc2, "tc2", tcs.get(1))] {
                    match (kw(s), w) {
                        (Some(k), _) => out.push(Item::param(self, k)),
                        (None, Some(w)) => {
                            // a quoted word can go on on a `+` line
                            let w = one_line(w, false).into_owned();
                            out.push(Item::named(self, name, Src::Word(w, v.loc)));
                        }
                        (None, None) => {}
                    }
                }
            } else if !is_tc(p, tc1) && !is_tc(p, tc2) {
                out.push(Item::param(self, p));
            }
        }
        out
    }

    /// The items of `R1 a 0 rval 0.004 1e-5`, where `rval` names no visible
    /// `.model` card (stored as model `rval`, value `0.004`, `tc1` `1e-5`):
    /// the value `rval`, then TC1 and TC2 from the numbers after it (golden
    /// `res_tc_positional`, `rcl_param_tc`). A third number has no place.
    /// A keyword `tc1=` or `tc2=` wins over the number in its place (the
    /// last value wins, golden `elem_dup_kw`; the lowering keeps only the
    /// keyword when it replaced the stored `tc1`, so the number after the
    /// value is read again from the line).
    fn shifted_tc<'n>(&'n self, params: &'n [ParamDef], vi: usize, mname: &str) -> Vec<Item<'n>> {
        let (tc1, tc2) = (self.syms.get("tc1"), self.syms.get("tc2"));
        let v = &params[vi];
        let positional = |p: &ParamDef| !self.after_eq(p);
        let is_tc = |p: &ParamDef, s: Option<Sym>| s.is_some() && Some(p.name) == s;
        let kw = |s: Option<Sym>| params.iter().rfind(|p| is_tc(p, s) && !positional(p));
        let (kw1, kw2) = (kw(tc1), kw(tc2));
        // the number right after the value: the stored positional `tc1`,
        // else the token after the value on the line
        let second = params.iter().find(|p| is_tc(p, tc1) && positional(p));
        let second_word = match second {
            Some(_) => None,
            None => self
                .word_after(v)
                .filter(|w| {
                    let unsigned = w.strip_prefix(['-', '+']).unwrap_or(w);
                    matches!(netlist_expr::parse(unsigned), Ok(Expr::Num(_)))
                })
                .map(str::to_owned),
        };
        let third = params.iter().find(|p| is_tc(p, tc2) && positional(p));
        let vtext = param_text(self, v);
        let mut why = format!(
            "`{mname}` names no visible `.model` card, so it is read as the value and `{vtext}` as TC1, as in `R1 1 0 1k 0.004` (golden `res_tc_positional`, `cl_tc_positional`; with a parameter name as the value, golden `rcl_param_tc`, `rcl_name_first_cl`)"
        );
        if let Some(p) = third {
            why.push_str(&format!(
                ". The third number after the value (`{}`) has no place and is not used",
                param_text(self, p)
            ));
        }
        let mut out: Vec<Item<'n>> = Vec::with_capacity(params.len() + 1);
        for (idx, p) in params.iter().enumerate() {
            if idx == vi {
                out.push(Item {
                    src: Src::Word(mname.to_owned(), v.loc),
                    caveats: vec![why.clone()],
                    ..Item::param(self, v)
                });
                out.push(match kw1 {
                    Some(k) => Item::param(self, k),
                    None => Item::named(self, "tc1", Src::Param(v)),
                });
                match (kw2, second, &second_word) {
                    (Some(k), ..) => out.push(Item::param(self, k)),
                    (None, Some(p), _) => out.push(Item::named(self, "tc2", Src::Param(p))),
                    (None, None, Some(w)) => {
                        out.push(Item::named(self, "tc2", Src::Word(w.clone(), v.loc)));
                    }
                    (None, None, None) => {}
                }
            } else if !is_tc(p, tc1) && !is_tc(p, tc2) {
                out.push(Item::param(self, p));
            }
        }
        out
    }
}
