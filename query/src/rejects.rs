//! Load checks for forms that HSPICE rejects and that parameter resolution
//! otherwise finds only when a query reaches them. Each
//! check reads each line once. An `L` line looks its model card up once
//! for each definition and pair of words, so the cost is linear in the
//! input; only an R line whose value names a model card (a form that
//! HSPICE rejects) looks its cards up on each line.
//!
//! A line in a definition that no instance path from the root reaches gets
//! no diagnostic: HSPICE checks such lines only on instance paths where
//! this was probed (golden `elem_m_zero_unused`), and these forms were not
//! probed there. The forms that no linear check finds are listed in
//! `golden/NOTES.md`, "Known differences".

use crate::Netlist;
use crate::api::Level;
use crate::hier::FxSet;
use crate::intern::Sym;
use crate::model::{Def, DefId, Diag, ElemKind, Element, LocIdx, ModelDef, ROOT};
use crate::params::statistical;
use netlist_expr::{Expr, is_builtin, parse};

/// Names that an expression can use with no `.param`. Only `temper` was
/// probed. `hertz` and `time` are HSPICE names for frequency- and
/// time-dependent values: the checks do not report them.
const KNOWN: [&str; 3] = ["temper", "hertz", "time"];

/// Adds the diagnostics of the checks to `diags` (all of them Errors) and
/// returns their count. `on_path`: an instance path from the root reaches
/// the definition.
pub(crate) fn check(
    net: &Netlist,
    active: &[bool],
    on_path: impl Fn(DefId) -> bool,
    diags: &mut Vec<Diag>,
) -> usize {
    let mut out = Vec::new();
    // a load Error can skip a statement that defines a name (golden
    // `brace_quoted_param`), and the loader does not read a `.prot` region
    // (Warning `protected_region`; PDK files define names there): then the
    // checks of undefined names are off, so that they do not report a name
    // that the deck defines
    let names = !net
        .diags
        .iter()
        .any(|d| d.level == Level::Error || d.code == "protected_region");
    // the names defined anywhere (see `Sets`)
    let mut anywhere: FxSet<Sym> = FxSet::default();
    let mut models: FxSet<Sym> = FxSet::default();
    let mut funcs: FxSet<Sym> = FxSet::default();
    let mut odd_l: FxSet<Sym> = FxSet::default();
    for d in &net.defs {
        anywhere.extend(d.defaults.iter().chain(&d.params).map(|p| p.name));
        funcs.extend(d.funcs.iter().map(|f| f.name));
        models.extend(d.models.iter().map(|m| m.name));
        odd_l.extend(
            d.models
                .iter()
                .filter(|m| {
                    net.syms.name(m.kind).eq_ignore_ascii_case("l") && m.level != Some(Some(3.0))
                })
                .map(|m| m.name),
        );
        // an `X`-line override defines its name in the new instance, also
        // a name that the subckt does not declare (golden `pe_scope_local`)
        for e in d.elements.iter().filter(|e| e.kind == ElemKind::Instance) {
            anywhere.extend(d.element_params(e).iter().map(|p| p.name));
        }
    }
    anywhere.extend(&models);
    anywhere.extend(&funcs);
    anywhere.extend(net.swept.iter().map(|s| s.name));
    let sets = Sets {
        anywhere,
        models,
        funcs,
        odd_l,
        names,
    };
    if names {
        top_params(net, &sets, &mut out);
    }
    let root = &net.defs[ROOT.idx()];
    let top_names: FxSet<Sym> = root.params.iter().map(|p| p.name).collect();
    let mut cards = Cards::default();
    for (i, d) in net.defs.iter().enumerate() {
        let id = DefId(i as u32);
        if !active[i] || !on_path(id) {
            continue;
        }
        for (j, e) in d.elements.iter().enumerate() {
            if e.alter.is_some() || e.parasitic {
                continue;
            }
            match e.kind {
                ElemKind::Instance => {
                    if id == ROOT && sets.names {
                        top_override_self(net, d, e, &top_names, &mut out);
                    }
                }
                ElemKind::Other(_) => {}
                _ => element(net, id, j, &sets, &mut cards, &mut out),
            }
        }
    }
    let n = out.len();
    diags.append(&mut out);
    n
}

/// Names defined in the deck.
struct Sets {
    /// By a `.param`, a header default, an `X`-line override, a `.param`
    /// function, a `.model` card or a sweep (also of a `.data` block),
    /// anywhere, in any run.
    anywhere: FxSet<Sym>,
    /// By a `.model` card, anywhere, in any run.
    models: FxSet<Sym>,
    /// By a `.param` function, anywhere, in any run.
    funcs: FxSet<Sym>,
    /// The `l` cards, anywhere, in any run, with no `level=3` (a card of
    /// that name can be fine in another scope: `rcl_check` decides).
    odd_l: FxSet<Sym>,
    /// The checks of undefined names are on (no load Error, no `.prot`).
    names: bool,
}

fn error(code: &'static str, message: String, e_loc: LocIdx) -> Diag {
    Diag {
        level: Level::Error,
        code,
        message,
        loc: Some(e_loc),
    }
}

/// The first name that `text` uses and that `defined` does not accept, if
/// `text` is an expression. The arguments of a call that is neither a
/// built-in function nor a `.param` function are not names: `v(out)` and
/// `i(v1)` name a node and an element (an output variable).
fn undefined(
    net: &Netlist,
    sets: &Sets,
    text: &str,
    defined: impl Fn(Sym) -> bool,
) -> Option<String> {
    fn walk<'a>(
        e: &'a Expr,
        f: &mut impl FnMut(&'a str) -> bool,
        net: &Netlist,
        sets: &Sets,
    ) -> bool {
        match e {
            Expr::Num(_) => false,
            Expr::Ident(n) => f(n),
            Expr::Call { name, args } => {
                (is_builtin(name) || net.syms.get(name).is_some_and(|s| sets.funcs.contains(&s)))
                    && args.iter().any(|a| walk(a, f, net, sets))
            }
            Expr::Unary { e, .. } => walk(e, f, net, sets),
            Expr::Binary { l, r, .. } => walk(l, f, net, sets) || walk(r, f, net, sets),
            Expr::Ternary { c, t, f: g } => {
                walk(c, f, net, sets) || walk(t, f, net, sets) || walk(g, f, net, sets)
            }
        }
    }
    let e = parse(text).ok()?;
    let mut found = None;
    walk(
        &e,
        &mut |n| {
            let bad = !KNOWN.iter().any(|k| k.eq_ignore_ascii_case(n))
                && !net.syms.get(n).is_some_and(&defined);
            if bad {
                found = Some(n.to_owned());
            }
            bad
        },
        net,
        sets,
    );
    found
}

/// `text` has `word` in it, in any letter case.
fn has(text: &str, word: &str) -> bool {
    text.as_bytes()
        .windows(word.len())
        .any(|w| w.eq_ignore_ascii_case(word.as_bytes()))
}

/// Top-level `.param` statements of the first run whose text names a
/// parameter that no top-level statement defines. HSPICE evaluates each
/// top-level `.param` at the top level, used or not, and rejects the deck
/// (golden `pe_scope_global`, `param_undef_unused`). A definition of an
/// `.alter` block counts, so that the check never reports a name that some
/// run defines. A function body is not checked: HSPICE runs a deck with an
/// unused function whose body names an undefined parameter (golden
/// `func_body_unused`).
fn top_params(net: &Netlist, sets: &Sets, out: &mut Vec<Diag>) {
    let root = &net.defs[ROOT.idx()];
    let mut top: FxSet<Sym> = root.params.iter().map(|p| p.name).collect();
    top.extend(root.funcs.iter().map(|f| f.name));
    top.extend(net.swept.iter().map(|s| s.name));
    // the last definition of each name in the first run wins (golden
    // `dup_param`): only its text is checked
    let mut last: crate::hier::FxMap<Sym, usize> = crate::hier::FxMap::default();
    for (i, p) in root.params.iter().enumerate() {
        if p.alter.is_none() {
            last.insert(p.name, i);
        }
    }
    let rule = "HSPICE evaluates a top-level `.param` at the top level, used or not, and rejects the deck (\"Undefined parameter or function definition\", golden `pe_scope_global`, `param_undef_unused`)";
    for (i, p) in root.params.iter().enumerate() {
        if p.alter.is_some() || p.num.is_some() || last.get(&p.name) != Some(&i) {
            continue;
        }
        let Some(text) = net.param_text(p) else {
            continue;
        };
        if let Some(name) = undefined(net, sets, &text, |s| top.contains(&s)) {
            out.push(error(
                "undefined_param",
                format!(
                    "top-level `.param {}` uses `{name}`, which no top-level `.param` defines: {rule}",
                    net.syms.name(p.name)
                ),
                p.loc,
            ));
        }
    }
}

/// A top-level `X` line with an override whose text names the overridden
/// parameter, when no top-level `.param` defines it: HSPICE looks the name
/// up above the new instance, where nothing defines it (golden
/// `ovr_self_global`). Parameter resolution gives the same reason.
fn top_override_self(
    net: &Netlist,
    root: &Def,
    e: &Element,
    top_names: &FxSet<Sym>,
    out: &mut Vec<Diag>,
) {
    for p in root.element_params(e) {
        if p.num.is_some() || top_names.contains(&p.name) {
            continue;
        }
        let name = net.syms.name(p.name);
        let Some(text) = net.param_text(p) else {
            continue;
        };
        if !has(&text, name) {
            continue;
        }
        let Ok(expr) = parse(&text) else {
            continue;
        };
        if netlist_expr::free_names(&expr)
            .iter()
            .any(|n| n.eq_ignore_ascii_case(name))
        {
            out.push(error(
                "override_self",
                format!(
                    "`{}` at the top level overrides `{name}` with `{text}`, which uses `{name}` itself, and no top-level `.param` defines `{name}`: HSPICE rejects the deck (\"cannot redefine parameter to itself\", golden `ovr_self_global`)",
                    net.syms.name(e.name)
                ),
                e.loc,
            ));
        }
    }
}

/// The checks of element line `j` of definition `id` (not an `X` line).
/// The card of an `L` line by its definition, model word and value word
/// (the card does not depend on the other words of the line).
type Cards<'n> = crate::hier::FxMap<(DefId, Option<Sym>, Option<Sym>), Option<&'n ModelDef>>;

fn element<'n>(
    net: &'n Netlist,
    id: DefId,
    j: usize,
    sets: &Sets,
    cards: &mut Cards<'n>,
    out: &mut Vec<Diag>,
) {
    let d = &net.defs[id.idx()];
    let e = &d.elements[j];
    let x = net.syms.name(e.name);
    let place = || {
        if id == ROOT {
            "at the top level".to_owned()
        } else {
            format!("in subckt `{}`", net.def_display(id))
        }
    };
    let rcl = matches!(
        e.kind,
        ElemKind::Resistor | ElemKind::Capacitor | ElemKind::Inductor
    );
    // an R, C or L line with only nodes, a value and TC numbers: its
    // parameters are expressions (a line with a model word or `key=` is
    // read again by parameter resolution)
    let simple = rcl
        && e.model.is_none()
        && d.rcl_ends
            .binary_search_by_key(&(j as u32), |r| r.0)
            .is_err();
    // the value of R or L, as written, when it is one word that names a
    // `.model` card (`R1 1 0 rval rmod`, `L1 1 2 lval lmod`)
    let mut value_word: Option<Sym> = None;
    for p in d.element_params(e).iter().filter(|p| p.num.is_none()) {
        let Some(text) = net.param_text(p) else {
            continue;
        };
        let key = match e.kind {
            ElemKind::Resistor => "r",
            ElemKind::Inductor => "l",
            _ => "",
        };
        if !key.is_empty() && net.syms.name(p.name).eq_ignore_ascii_case(key) {
            value_word = net
                .syms
                .get(text.trim())
                .filter(|s| sets.models.contains(s));
        }
        // a `.param` function hides a built-in function of its name
        // (golden `func_shadow_top`, `stat_user_func`)
        if (has(&text, "gauss") || has(&text, "unif"))
            && let Ok(expr) = parse(&text)
            && let Some(f) = statistical(&expr, &mut |n| {
                net.syms.get(n).is_some_and(|s| sets.funcs.contains(&s))
            })
        {
            out.push(error(
                "statistical_in_element",
                format!(
                    "`{x}` {} calls `{f}()` in `{}` = `{text}`: HSPICE rejects a statistical function directly in an element expression (\"cannot be defined in expressions directly\", golden `stat_direct`, `stat_in_mos`); it belongs in a `.param`",
                    place(),
                    net.syms.name(p.name).to_ascii_lowercase(),
                ),
                e.loc,
            ));
        } else if simple
            && sets.names
            && let Some(name) = undefined(net, sets, &text, |s| sets.anywhere.contains(&s))
        {
            out.push(error(
                "undefined_param",
                format!(
                    "`{x}` {} uses `{name}`, which no `.param`, subckt default, `X`-line override or `.model` card of the deck defines: HSPICE rejects the deck (\"Undefined parameter or function definition\", golden `unquoted_space`)",
                    place()
                ),
                e.loc,
            ));
        }
    }
    if e.model.is_none() {
        return;
    }
    // `rcl_check` looks the cards up: only for a line that can fail. An R
    // line with a model name as its value (`res_name_model`)
    if e.kind == ElemKind::Resistor {
        if value_word.is_some()
            && let (Some(why), _) = net.rcl_check(id, j)
        {
            out.push(error(
                "name_then_model",
                format!("`{x}` {}: {why}", place()),
                e.loc,
            ));
        }
        return;
    }
    // `L1 1 2 1n lmod` with `.model lmod l` and no `level=3` (a `level`
    // that is an expression is not checked). HSPICE reads a name, then a
    // model name, on an L line as a value and a model (`ind_name_model`),
    // so the line has no other reason to fail
    let odd = |s: Option<Sym>| s.is_some_and(|s| sets.odd_l.contains(&s));
    if e.kind == ElemKind::Inductor
        && (odd(e.model) || odd(value_word))
        && let Some(md) = *cards
            .entry((id, e.model, value_word))
            .or_insert_with(|| net.rcl_check(id, j).1)
        && net.syms.name(md.kind).eq_ignore_ascii_case("l")
        && md.level.is_none_or(|l| l.is_some_and(|l| l != 3.0))
    {
        out.push(error(
            "inductor_model_level",
            format!(
                "`{x}` {} uses the model `{}`, an `l` card whose `level` is not 3 (the last `level` key counts): HSPICE rejects the deck (\"is not for L_element, its level must be 3\", golden `ind_model_nolevel`, `ind_level_twice`)",
                place(),
                net.syms.name(md.name)
            ),
            e.loc,
        ));
    }
}
