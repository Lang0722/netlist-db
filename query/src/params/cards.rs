//! The `.model` card of an element and the values of its parameters
//! ([`Netlist::resolve_model`]).
//!
//! The card is looked up along the instance path of the element, from the
//! level of the element up to the top level, as the model selector of
//! `netlist-db` looks up the card of a MOSFET (`src/models.rs`; decks
//! `q_model_dynamic`, `q_localhide` of `scripts/hspice_kit`): at each
//! level, a card with the model name of the line, else, for a MOSFET, the
//! first bin `name.N` (in netlist order) whose ranges hold the MOSFET. The
//! ranges of a bin are evaluated at the level that defines the bin, and L
//! and W with the parameters of the line, times `.option scale`
//! (`src/models/bins.rs`). `query/tests/selector_agree.rs` checks that the
//! choice is the selector's.
//!
//! Each `key=value` of the card is evaluated where the card is defined: a
//! top-level card with the top-level parameters, a card inside a subckt
//! with the parameters of that subckt's instance on the path. HSPICE was
//! measured to do so for the ranges of a bin (decks `q_limits_mac_*`,
//! `q_limits_nested_*`), with one known gap (`scripts/hspice_kit/README.md`):
//! the first MOSFET that looks at a bin (breadth-first) evaluates the
//! ranges in its own instance. For the other parameters of a card this was
//! not probed, and every answer says so.
use super::*;
use crate::{
    api::{BinChoice, BinRange, ModelCard, ModelParams, Page, Paged, page},
    model::{BinOptions, CardParam, Def},
};

/// The scope rule of a card, on each answer with a card.
const SCOPE_RULE: &str = "the parameters of a card are evaluated where the card is defined: a top-level card with the top-level parameters, a card inside a subckt with the parameters of that subckt's instance on this path (HSPICE does so for the limits of a bin: decks `q_limits_mac_*`, `q_limits_nested_*` of `scripts/hspice_kit`; for the other parameters of a card this was not probed)";

/// What the values are, on each answer with a card.
const AS_WRITTEN: &str = "these are the values written on the card: the simulator derives the values that a device uses from them (the `l…`, `w…` and `p…` terms of a binned model with Leff and Weff, the temperature, the defaults of the parameters that the card does not set), and those are not computed";

/// The caveat of a deck with a `.prot` region.
const PROTECTED: &str = "the deck has a `.prot` region, which the loader does not read: a card defined there is not seen";

/// The keys of the limits of a bin (`src/models/bins.rs`, `LIMITS`).
const LIMITS: [&str; 8] = [
    "lmin",
    "lmax",
    "wmin",
    "wmax",
    "nfinmin",
    "nfinmax",
    "wsheetmin",
    "wsheetmax",
];

/// How far below a limit (relative) a value counts as on it
/// (`src/models/bins.rs`: 1e-15 below does, 3e-15 below does not).
const EDGE_TOLERANCE: f64 = 2e-15;

/// `min <= x < max`, a value just below a limit counting as on it.
fn within(x: f64, (min, max): (f64, f64)) -> bool {
    let tol = |v: f64| v.abs() * EDGE_TOLERANCE;
    x >= min - tol(min) && x < max - tol(max)
}

/// The size of a MOSFET, as its bin is chosen (`src/models/bins.rs`,
/// `Size`).
#[derive(Debug, Clone, Copy)]
struct Size {
    /// L and W, times `.option scale`.
    l: f64,
    w: f64,
    nf: f64,
    /// `WNFLAG` on the line, if given.
    wnflag: Option<bool>,
    /// The fin count, for a bin of level 72 (1 if not given).
    nfin: f64,
    /// The sheet width, for a bin of level 72 (0 if not given: then no bin
    /// with sheet limits holds it, deck `w_sheet_w`).
    wsheet: f64,
}

/// What a bin limits besides the length (`src/models/bins.rs`, `Across`).
#[derive(Debug, Clone, Copy)]
struct Across {
    /// `wmin`, `wmax`.
    width: bool,
    /// `nfinmin`, `nfinmax` (level 72).
    fins: bool,
    /// `wsheetmin`, `wsheetmax` (level 72).
    sheet: bool,
}

/// The ranges of a bin, after `.option scalm` (not the fin counts).
#[derive(Debug, Clone, Copy)]
struct Limits {
    l: (f64, f64),
    width: Option<(f64, f64)>,
    fins: Option<(f64, f64)>,
    sheet: Option<(f64, f64)>,
}

impl Limits {
    /// The ranges, with the sizes that they compare, when all of them hold
    /// `size`; `None` when one does not. The width is W/NF unless `WNFLAG=0`
    /// (on the line, else `.option wnflag`), and the fin count and the sheet
    /// width must be above 0 (decks `hb_*`).
    fn hold(&self, size: &Size, opts: &BinOptions) -> Option<Vec<BinRange>> {
        let per_finger = size.wnflag.unwrap_or(opts.wnflag);
        let w = if per_finger && size.nf > 0.0 {
            size.w / size.nf
        } else {
            size.w
        };
        let range = |name: &str, value: f64, (min, max): (f64, f64)| BinRange {
            name: name.to_owned(),
            value,
            min,
            max,
        };
        if !within(size.l, self.l) {
            return None;
        }
        let mut out = vec![range("l", size.l, self.l)];
        if let Some(r) = self.width {
            if !within(w, r) {
                return None;
            }
            out.push(range("w", w, r));
        }
        if let Some(r) = self.fins {
            if !(size.nfin > 0.0 && within(size.nfin, r)) {
                return None;
            }
            out.push(range("nfin", size.nfin, r));
        }
        if let Some(r) = self.sheet {
            if !(size.wsheet > 0.0 && within(size.wsheet, r)) {
                return None;
            }
            out.push(range("wsheet", size.wsheet, r));
        }
        Some(out)
    }
}

/// `card` is the name of a bin of `base`: `base.suffix`, with a suffix and
/// no other `.` (`a.b.1` is a bin of nothing; `src/models.rs`,
/// `ModelIndex::new`).
fn bin_of(card: &str, base: &str) -> bool {
    card.split_once('.').is_some_and(|(b, suffix)| {
        !b.is_empty() && !suffix.is_empty() && !suffix.contains('.') && b.eq_ignore_ascii_case(base)
    })
}

/// The reason of a bin without its limits (`SelectError::MissingLimits`).
fn missing_limits(card: &str) -> String {
    format!(
        "model `{card}` has a `.` in its name: it must set LMIN, LMAX, WMIN and WMAX (at level 72: LMIN, LMAX, and NFINMIN and NFINMAX, or WSHEETMIN and WSHEETMAX), and HSPICE stops at a bin without them"
    )
}

/// The reason of a name that the card does not set.
fn not_set(name: &str) -> String {
    format!(
        "the card does not set `{name}`: the simulator uses the model's default for it, which this tool does not know"
    )
}

/// The caveat on a key that the card sets more than once, on `lines`.
fn twice(name: &str, lines: &[u32]) -> String {
    let shown: Vec<String> = lines.iter().map(u32::to_string).collect();
    format!(
        "the card sets `{name}` {} times (lines {}): HSPICE uses the first one for a limit of a bin (deck `mdl_dup_keys`) and the last `level` of an `L` card (golden `ind_level_twice`); for this parameter it was not probed",
        lines.len(),
        shown.join(", ")
    )
}

/// What [`Netlist::choose_bin`] and [`Netlist::choose_card`] found.
struct Choice<'n> {
    /// The level of the path whose definition holds the card, and the
    /// card; or why there is none.
    card: Result<(usize, &'n ModelDef), String>,
    /// For a MOSFET: its size and its bin.
    selection: Option<BinChoice>,
    /// The caveats on the choice.
    caveats: Vec<String>,
}

/// An item of [`ModelParams::params`]: a `key=value` of the card (its
/// index), or a name asked for that the card does not set.
enum Entry {
    Param(usize),
    Missing(String),
}

impl Netlist {
    /// The `.model` card that the element at `path` (a MOSFET, an R, C or
    /// L line with a model, a diode, a BJT) uses on its instance path, and
    /// its `key=value` items, each as written and evaluated where the card
    /// is defined, with its chain (the first step is the card's item,
    /// [`StepKind::ModelParam`]). `names` (any letter case) picks the items
    /// of those names, in that order, and a name that the card does not set
    /// gives an item with no value; with no names, all the items, in card
    /// order. The items come in pages.
    ///
    /// The card is the nearest one on the instance path, from the
    /// element's level up: for a MOSFET, an NMOS or PMOS card of the model
    /// name, else the first bin `name.N` whose ranges hold its L and W (W/NF
    /// unless `WNFLAG=0`), as the model selector of `netlist-db` chooses it
    /// (`selection` gives the size and the ranges). With no card, the answer
    /// has `unresolved` and no items. A path that names no element, or an
    /// element with no model (an `X` line, a source, an R with no model
    /// word), is a bad argument.
    ///
    /// The values are those written on the card. The simulator derives the
    /// values that a transistor uses from them (bin terms with Leff and
    /// Weff, temperature): those are not computed.
    pub fn resolve_model(
        &self,
        path: &str,
        names: &[&str],
        page: &Page,
    ) -> Result<ModelParams, QueryError> {
        on_big_stack(|| self.resolve_model_here(path, names, page))?
    }

    fn resolve_model_here(
        &self,
        path: &str,
        names: &[&str],
        page_req: &Page,
    ) -> Result<ModelParams, QueryError> {
        let w = walk(self, path, Want::Element)?;
        let Some(&(holder, i)) = w.elems.last() else {
            return Err(QueryError::BadArgument(
                "the path is empty: it names the root, not an element".to_owned(),
            ));
        };
        let e = &self.defs[holder.idx()].elements[i];
        let model = self.line_model(&w.path, holder, i)?;
        let mut caveats = w.caveats.clone();
        caveats.extend(self.path_caveats(&w));
        caveats.extend(self.alter_caveats(&w));
        let mut r = Resolver::new(self, scopes(self, &w, false));
        let k = r.scopes.len() - 1;
        caveats.extend(r.path_m_caveats(k));
        let choice = if e.kind == ElemKind::Mosfet {
            self.choose_bin(&mut r, k, holder, i, model)
        } else {
            self.choose_card(&r, k, holder, model, e.kind)
        };
        caveats.extend(choice.caveats);
        if self.protected {
            caveats.push(PROTECTED.to_owned());
        }
        let mut answer = ModelParams {
            path: w.path.clone(),
            kind: e.kind.name().to_owned(),
            model: self.syms.name(model).to_owned(),
            card: None,
            unresolved: None,
            selection: choice.selection,
            params: Paged {
                items: Vec::new(),
                total: 0,
                next_cursor: None,
            },
            caveats: Vec::new(),
        };
        let (j, md) = match choice.card {
            Ok(found) => found,
            Err(why) => {
                answer.unresolved = Some(why);
                cap_caveats(&mut caveats);
                answer.caveats = caveats;
                return Ok(answer);
            }
        };
        let card_def = r.scopes[j].def;
        let d = &self.defs[card_def.idx()];
        let card = d.card_params(md);
        let params: Vec<ParamDef> = card.iter().map(|cp| cp.param(md)).collect();
        caveats.extend(self.card_caveats(d, card_def, md));
        let mut lines: HashMap<Sym, Vec<u32>> = HashMap::new();
        for p in &params {
            lines.entry(p.name).or_default().push(p.loc.line);
        }
        let entries = self.card_entries(&params, names);
        let items = page(entries.into_iter(), page_req)?.map(|entry| match entry {
            Entry::Param(at) => {
                let p = &params[at];
                let mut more = Vec::new();
                if let Some(on) = lines.get(&p.name).filter(|on| on.len() > 1) {
                    more.push(twice(self.syms.name(p.name), on));
                }
                more.extend(self.below_caveats(&r, p, j, k));
                let mut res = r.resolve_card_param(p, j, more);
                cap_caveats(&mut res.caveats);
                res
            }
            Entry::Missing(name) => Resolved {
                unresolved: Some(not_set(&name)),
                name,
                value: None,
                chain: Vec::new(),
                caveats: Vec::new(),
            },
        });
        answer.card = Some(ModelCard {
            name: self.syms.name(md.name).to_owned(),
            r#type: self.syms.name(md.kind).to_owned(),
            def: self.def_display(card_def),
            scope: r.scopes[j].path.clone(),
            loc: self.loc(md.loc),
            params_total: card.len(),
        });
        answer.params = items;
        cap_caveats(&mut caveats);
        answer.caveats = caveats;
        Ok(answer)
    }

    /// The model name of element `i` of `holder` (at `path`), as HSPICE
    /// reads the line: for an R, C or L line, the reading of
    /// [`Netlist::elem_model`] (a word is a model only when a card of that
    /// name is visible). A bad argument for an element with none.
    fn line_model(&self, path: &str, holder: DefId, i: ElemIdx) -> Result<Sym, QueryError> {
        let e = &self.defs[holder.idx()].elements[i];
        let kind = e.kind.name();
        let model = match e.kind {
            ElemKind::Mosfet | ElemKind::Diode | ElemKind::Bjt => e.model,
            ElemKind::Resistor | ElemKind::Capacitor | ElemKind::Inductor => {
                self.elem_model(holder, i)
            }
            _ => None,
        };
        model.ok_or_else(|| {
            QueryError::BadArgument(match e.kind {
                ElemKind::Resistor | ElemKind::Capacitor | ElemKind::Inductor => format!(
                    "`{path}` has no model: a word after the nodes of a {kind} line is a model only when a `.model` card of that name is visible (golden `res_model`), and this line has none"
                ),
                ElemKind::Instance => format!(
                    "`{path}` is a subckt instance: an `X` line has no model card (resolve_element gives its parameters)"
                ),
                ElemKind::Other(_) => format!(
                    "`{path}` is an element of a type that the parser does not read: its model is not known"
                ),
                _ => format!("`{path}` is a {kind}: it has no model card"),
            })
        })
    }

    /// The card `model` of an element (not a MOSFET) of kind `kind` in
    /// `holder`, at level `k` of the path of `r`: the nearest level, from
    /// the element's own up, whose definition holds a first-run card of
    /// that name.
    fn choose_card<'n>(
        &'n self,
        r: &Resolver<'n>,
        k: usize,
        holder: DefId,
        model: Sym,
        kind: ElemKind,
    ) -> Choice<'n> {
        let found = (0..=k).rev().find_map(|j| {
            let def = r.scopes[j].def;
            self.defs[def.idx()]
                .models
                .iter()
                .find(|md| md.name == model && md.alter.is_none())
                .map(|md| (j, md))
        });
        let name = self.syms.name(model);
        let mut caveats = Vec::new();
        let card = match found {
            Some((j, md)) => {
                // the card that the definition of the line sees
                if let Some(seen) = self.visible_model(holder, model)
                    && !std::ptr::eq(seen, md)
                {
                    caveats.push(format!(
                        "the card is looked up along the instance path, as HSPICE looks up the card of a MOSFET (deck `q_model_dynamic` of `scripts/hspice_kit`); for a {} this was not probed, and the card `{name}` that the definition of the line sees is the one at {}",
                        kind.name(),
                        loc_str(self, seen.loc)
                    ));
                }
                Ok((j, md))
            }
            None => Err(format!(
                "no `.model {name}` card is on the instance path of the element"
            )),
        };
        Choice {
            card,
            selection: None,
            caveats,
        }
    }

    /// The card of MOSFET `i` of `holder`, whose model name is `model`, at
    /// level `k` of the path of `r`, as the model selector chooses it
    /// (`src/models.rs`, `Selector::choose`): at each level of the path, from
    /// the MOSFET's own up, an NMOS or PMOS card of that name, else the
    /// first bin `model.N` (netlist order) whose ranges, evaluated at that
    /// level, hold the MOSFET. A bin of the name without its limits, or a
    /// range with no value, stops the search, as it stops HSPICE.
    fn choose_bin<'n>(
        &'n self,
        r: &mut Resolver<'n>,
        k: usize,
        holder: DefId,
        i: ElemIdx,
        model: Sym,
    ) -> Choice<'n> {
        let opts = self.bin_opts;
        let mut sel = BinChoice {
            by: String::new(),
            l: None,
            w: None,
            nf: None,
            ranges: Vec::new(),
            bins_passed: 0,
        };
        let mut caveats = Vec::new();
        let fail = |why: String, sel: BinChoice, caveats: Vec<String>| Choice {
            card: Err(why),
            selection: Some(sel),
            caveats,
        };
        let size = match self.mos_size(r, k, holder, i, &mut sel) {
            Ok(size) => size,
            Err(why) => return fail(why, sel, caveats),
        };
        let name = self.syms.name(model);
        if name.contains('.') {
            let why = format!(
                "`{name}` has a `.`: a MOSFET line cannot name a bin, so HSPICE finds no model of that name (deck `q_bin_direct_in` of `scripts/hspice_kit`)"
            );
            return fail(why, sel, caveats);
        }
        let is_mos = |md: &ModelDef| {
            let t = self.syms.name(md.kind);
            md.alter.is_none() && (t.eq_ignore_ascii_case("nmos") || t.eq_ignore_ascii_case("pmos"))
        };
        let mut seen = 0;
        for j in (0..=k).rev() {
            let def = r.scopes[j].def;
            let d = &self.defs[def.idx()];
            // only an NMOS or PMOS card serves an M line, and a card of
            // another type hides nothing (decks `mt_*`, `mdl_diode_on_m`)
            if let Some(md) = d.models.iter().find(|md| is_mos(md) && md.name == model) {
                sel.by = "exact".to_owned();
                sel.bins_passed = seen;
                return Choice {
                    card: Ok((j, md)),
                    selection: Some(sel),
                    caveats,
                };
            }
            let bins: Vec<&ModelDef> = d
                .models
                .iter()
                .filter(|md| is_mos(md) && bin_of(self.syms.name(md.name), name))
                .collect();
            if bins.is_empty() {
                continue;
            }
            if let Some(bad) = bins.iter().find(|md| self.across(d, md).is_none()) {
                sel.bins_passed = seen;
                return fail(missing_limits(self.syms.name(bad.name)), sel, caveats);
            }
            for (n, &md) in bins.iter().enumerate() {
                let across = self.across(d, md).expect("checked above");
                let limits = match self.bin_limits(r, j, d, md, across) {
                    Ok(limits) => limits,
                    Err(why) => {
                        sel.bins_passed = seen + n;
                        return fail(why, sel, caveats);
                    }
                };
                if let Some(ranges) = limits.hold(&size, &opts) {
                    sel.by = "bin".to_owned();
                    sel.ranges = ranges;
                    sel.bins_passed = seen + n;
                    caveats.extend(self.limits_below(r, j, k, d, &bins[..=n]));
                    return Choice {
                        card: Ok((j, md)),
                        selection: Some(sel),
                        caveats,
                    };
                }
            }
            caveats.extend(self.limits_below(r, j, k, d, &bins));
            seen += bins.len();
        }
        sel.bins_passed = seen;
        let why = if seen == 0 {
            format!(
                "no NMOS or PMOS card `{name}`, nor bins `{name}.N`, on the instance path of the MOSFET"
            )
        } else {
            let mut why = format!(
                "none of the {seen} bins `{name}.N` holds L={}, W={}",
                fmt_num(size.l),
                fmt_num(size.w)
            );
            if size.wnflag.unwrap_or(opts.wnflag) && size.nf > 0.0 && size.nf != 1.0 {
                why.push_str(&format!(
                    " (the bins compare W/NF = {})",
                    fmt_num(size.w / size.nf)
                ));
            }
            why
        };
        fail(why, sel, caveats)
    }

    /// The size of MOSFET `i` of `holder`, evaluated at its level `k`, as
    /// the model selector reads it (`src/models/bins.rs`, `size`): `L` and
    /// `W` (else `.option defl`, `defw`) times `.option scale`, `NF`,
    /// `WNFLAG`, `NFIN`, `WSHEET`; `sel` gets L, W and NF. A value that
    /// cannot be computed, and a size at which HSPICE stops (L <= 0, W < 0,
    /// NF < 0, M <= 0: decks `m_l_neg`, `m_l_zero`, `m_w_neg`, `m_nf_neg`,
    /// `m_m_zero`), give the reason.
    fn mos_size(
        &self,
        r: &mut Resolver<'_>,
        k: usize,
        holder: DefId,
        i: ElemIdx,
        sel: &mut BinChoice,
    ) -> Result<Size, String> {
        let opts = self.bin_opts;
        let d = &self.defs[holder.idx()];
        let params = d.element_params(&d.elements[i]);
        let mut value = |key: &str| -> Result<Option<f64>, String> {
            let Some(sym) = self.syms.get(key) else {
                return Ok(None);
            };
            let Some(p) = params.iter().rfind(|p| p.name == sym) else {
                return Ok(None);
            };
            let what = key.to_ascii_uppercase();
            match r.eval_param(p, k, true, Text::Body).0 {
                Ok(v) if v.is_finite() => Ok(Some(v)),
                Ok(_) => Err(format!("{what}: {OUT_OF_RANGE}")),
                Err(why) => Err(format!("{what}: {why}")),
            }
        };
        let l = value("l")?.unwrap_or(opts.defl) * opts.scale;
        sel.l = Some(l);
        let w = value("w")?.unwrap_or(opts.defw) * opts.scale;
        sel.w = Some(w);
        let nf = value("nf")?.unwrap_or(1.0);
        sel.nf = Some(nf);
        let size = Size {
            l,
            w,
            nf,
            wnflag: value("wnflag")?.map(|flag| flag != 0.0),
            nfin: value("nfin")?.unwrap_or(1.0),
            wsheet: value("wsheet")?.unwrap_or(0.0) * opts.scale,
        };
        let bad = |what: &str, v: f64| -> Result<Size, String> {
            Err(format!("{what}={}: HSPICE stops at this size", fmt_num(v)))
        };
        if size.l <= 0.0 {
            return bad("L", size.l);
        }
        if size.w < 0.0 {
            return bad("W", size.w);
        }
        if size.nf < 0.0 {
            return bad("NF", size.nf);
        }
        match value("m")? {
            Some(m) if m <= 0.0 => bad("M", m),
            _ => Ok(size),
        }
    }

    /// The first item `key` of card `md` of `d`.
    fn card_key<'d>(&self, d: &'d Def, md: &ModelDef, key: &str) -> Option<&'d CardParam> {
        let sym = self.syms.get(key)?;
        d.card_params(md).iter().find(|cp| cp.name == sym)
    }

    /// The value of `cp`, an item of card `md`, when it is a plain number
    /// (the parser's number, not one in quotes).
    fn plain_number(&self, md: &ModelDef, cp: &CardParam) -> Option<f64> {
        if cp.num.is_nan() {
            return None;
        }
        let text = self.files[md.loc.file as usize].text.as_bytes();
        let quoted =
            cp.start < cp.end && cp.start > 0 && text.get(cp.start as usize - 1) == Some(&b'\'');
        (!quoted).then_some(cp.num)
    }

    /// What the bin `md` of `d` limits besides the length, or `None` when
    /// it does not set the limits of its level (`src/models/bins.rs`,
    /// `bin_across`): level 72 (the first `level` of the card, a plain
    /// number) reads `nfin*` and `wsheet*`, not `wmin` and `wmax` (decks
    /// `k_lw`, `h_six_w_out`).
    fn across(&self, d: &Def, md: &ModelDef) -> Option<Across> {
        let sets = |key: &str| self.card_key(d, md, key).is_some();
        if !sets("lmin") || !sets("lmax") {
            return None;
        }
        let finfet = self
            .card_key(d, md, "level")
            .is_some_and(|cp| self.plain_number(md, cp) == Some(72.0));
        let across = Across {
            width: !finfet && sets("wmin") && sets("wmax"),
            fins: finfet && sets("nfinmin") && sets("nfinmax"),
            sheet: finfet && sets("wsheetmin") && sets("wsheetmax"),
        };
        (across.width || across.fins || across.sheet).then_some(across)
    }

    /// The ranges of the bin `md` of `d`, evaluated at level `j`: each
    /// limit is the first item of its key (deck `mdl_dup_keys`), times
    /// `.option scalm` but for the fin counts (`src/models/bins.rs`,
    /// `limits`).
    fn bin_limits(
        &self,
        r: &mut Resolver<'_>,
        j: usize,
        d: &Def,
        md: &ModelDef,
        across: Across,
    ) -> Result<Limits, String> {
        let scalm = self.bin_opts.scalm;
        let card = self.syms.name(md.name);
        let mut param = |key: &str, factor: f64| -> Result<f64, String> {
            let cp = self
                .card_key(d, md, key)
                .ok_or_else(|| missing_limits(card))?;
            let what = key.to_ascii_uppercase();
            match r.eval_param(&cp.param(md), j, false, Text::Body).0 {
                Ok(v) if v.is_finite() => Ok(v * factor),
                Ok(_) => Err(format!("{what} of model `{card}`: {OUT_OF_RANGE}")),
                Err(why) => Err(format!("{what} of model `{card}`: {why}")),
            }
        };
        let l = (param("lmin", scalm)?, param("lmax", scalm)?);
        let width = if across.width {
            Some((param("wmin", scalm)?, param("wmax", scalm)?))
        } else {
            None
        };
        let fins = if across.fins {
            Some((param("nfinmin", 1.0)?, param("nfinmax", 1.0)?))
        } else {
            None
        };
        let sheet = if across.sheet {
            Some((param("wsheetmin", scalm)?, param("wsheetmax", scalm)?))
        } else {
            None
        };
        Ok(Limits {
            l,
            width,
            fins,
            sheet,
        })
    }

    /// The names that the text of `p`, evaluated at level `j`, reads and
    /// that level `k` (`j < k`) sees otherwise (another definition, or one
    /// where level `j` has none), each with the scope of the level that
    /// binds it for `k`.
    fn bound_below(
        &self,
        r: &Resolver<'_>,
        p: &ParamDef,
        j: usize,
        k: usize,
    ) -> Vec<(String, String)> {
        if j >= k || p.num.is_some() {
            return Vec::new();
        }
        let Some(text) = self.param_text(p) else {
            return Vec::new();
        };
        let Ok(e) = netlist_expr::parse(&text) else {
            return Vec::new();
        };
        netlist_expr::free_names(&e)
            .into_iter()
            .filter_map(|name| {
                let sym = self.syms.get(&name)?;
                let below = r.find(k, sym)?;
                let differs = r.find(j, sym).is_none_or(|at| !std::ptr::eq(at.p, below.p));
                differs.then(|| {
                    (
                        self.syms.name(sym).to_owned(),
                        r.scopes[below.level].path.clone(),
                    )
                })
            })
            .collect()
    }

    /// The caveats on `p`, an item of a card at level `j`, for the names it
    /// reads that the element's level `k` sees otherwise (see
    /// [`Netlist::bound_below`]): HSPICE evaluated the limits of a bin so
    /// in the first MOSFET that looked at the bin (the known gap).
    fn below_caveats(&self, r: &Resolver<'_>, p: &ParamDef, j: usize, k: usize) -> Vec<String> {
        self.bound_below(r, p, j, k)
            .into_iter()
            .map(|(name, at)| {
                format!(
                    "`{name}` is also defined below the card, in `{at}` on this path: HSPICE evaluates the limits of a bin in the instance of the first MOSFET that looks at the bin (breadth-first), not where the bin is defined (known gap, `scripts/hspice_kit/README.md`); for this parameter that was not probed, and this value is the one where the card is defined"
                )
            })
            .collect()
    }

    /// The caveat of the known gap, when the limits of the bins `bins` of
    /// `d`, at level `j` below the MOSFET's level `k`, read a name that the
    /// MOSFET's level sees otherwise.
    fn limits_below(
        &self,
        r: &Resolver<'_>,
        j: usize,
        k: usize,
        d: &Def,
        bins: &[&ModelDef],
    ) -> Option<String> {
        if j >= k {
            return None;
        }
        let mut names: Vec<(String, String)> = Vec::new();
        for &md in bins {
            for cp in d.card_params(md) {
                let key = self.syms.name(cp.name);
                if !LIMITS.iter().any(|l| l.eq_ignore_ascii_case(key)) {
                    continue;
                }
                for found in self.bound_below(r, &cp.param(md), j, k) {
                    if !names.iter().any(|(n, _)| *n == found.0) {
                        names.push(found);
                    }
                }
            }
        }
        let (first, at) = names.first()?;
        let more = match names.len() {
            1 => String::new(),
            n => format!(" (and {} more names)", n - 1),
        };
        Some(format!(
            "the limits of the bins read `{first}`{more}, which a level below the bins also defines (`{at}` on this path): HSPICE evaluates the limits of a bin in the instance of the first MOSFET that looks at the bin (breadth-first), so for that MOSFET its choice can differ (known gap, `scripts/hspice_kit/README.md`); this choice uses the limits where the bins are defined, as the model selector does"
        ))
    }

    /// The caveats of the card `md` of definition `def` (`d`).
    fn card_caveats(&self, d: &Def, def: DefId, md: &ModelDef) -> Vec<String> {
        let mut out = vec![SCOPE_RULE.to_owned(), AS_WRITTEN.to_owned()];
        let name = self.syms.name(md.name);
        let same = || d.models.iter().filter(|o| o.name == md.name);
        if same().filter(|o| o.alter.is_none()).count() > 1 {
            out.push(format!(
                "HSPICE rejects this deck: `{name}` names more than one card in `{}` (diagnostic `duplicate_model`); this answer uses the first",
                self.def_display(def)
            ));
        }
        for o in same() {
            if let Some(a) = o.alter {
                out.push(format!(
                    "`.alter #{a}` at {} defines the card `{name}` again: this answer describes the first run",
                    loc_str(self, o.loc)
                ));
            }
        }
        if d.has_if {
            out.push(format!(
                "`{}` has an `.if` block: the query layer reads all its branches, so a card of a branch that is not taken is seen too (diagnostic `if_block`)",
                self.def_display(def)
            ));
        }
        let scalm = self.bin_opts.scalm;
        if scalm != 1.0 {
            out.push(format!(
                "`.option scalm` is {}: HSPICE multiplies the geometric parameters of a card by it (the limits in `selection` are multiplied); the values in `params` are as written",
                fmt_num(scalm)
            ));
        }
        out
    }

    /// The items of the answer: with no names, every item of the card
    /// `params`; else, for each name (any letter case, each once), the
    /// items of that name, or a missing one.
    fn card_entries(&self, params: &[ParamDef], names: &[&str]) -> Vec<Entry> {
        let mut seen = HashSet::new();
        let wanted: Vec<&str> = names
            .iter()
            .map(|n| n.trim())
            .filter(|n| !n.is_empty() && seen.insert(n.to_ascii_lowercase()))
            .collect();
        if wanted.is_empty() {
            return (0..params.len()).map(Entry::Param).collect();
        }
        let mut out = Vec::new();
        for name in wanted {
            let sym = self.syms.get(name);
            let at: Vec<usize> = (0..params.len())
                .filter(|&i| Some(params[i].name) == sym)
                .collect();
            if at.is_empty() {
                out.push(Entry::Missing(name.to_owned()));
            } else {
                out.extend(at.into_iter().map(Entry::Param));
            }
        }
        out
    }
}
