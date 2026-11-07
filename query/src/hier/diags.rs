//! The diagnostics of the hierarchy: cycles, names with no definition, and x lines that do not fit.
use super::*;

/// The diagnostic of a call cycle (`a` (`xb`) -> `b` (`xa`) -> `a`). On an
/// instance path from the root (`reached`), HSPICE rejects the deck
/// ("subcircuit a is defined recursively", golden `hier_reentry`): an
/// Error. A cycle that no instance path goes through makes no instances,
/// and HSPICE runs the deck (golden `hier_unused_cycle_unres`): a Warning.
pub(super) fn cycle_diag(net: &Netlist, steps: &[(DefId, u32)], reached: bool) -> Diag {
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
pub(super) enum Va {
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
pub(super) fn builtin_model_type(kind: &str) -> bool {
    const TYPES: [&str; 23] = [
        "amp", "c", "core", "csw", "d", "l", "lpnp", "nmf", "njf", "nmos", "npn", "opt", "pjf",
        "plot", "pmf", "pmos", "pnp", "r", "s", "sp", "sw", "u", "w",
    ];
    TYPES.iter().any(|t| t.eq_ignore_ascii_case(kind))
}

/// How the `X` lines of a definition were checked.
#[derive(Debug, Clone, Copy)]
pub(super) enum Reach {
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
pub(super) fn unresolved_diag(
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
pub(super) fn m_diag(net: &Netlist, def: DefId, e: &Element, text: &str, on_path: bool) -> Diag {
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
pub(super) fn dup_elem_diag(
    net: &Netlist,
    def: DefId,
    e: &Element,
    earlier: &Element,
    on_path: bool,
) -> Diag {
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
    let (level, message) = if on_path && net.defs[def.idx()].has_if {
        (
            Level::Warning,
            format!(
                "`{x}` {place} {what}: the lines can be in different `.if` branches, which the query layer does not tell apart; HSPICE rejects the deck only when one run reads both"
            ),
        )
    } else if on_path {
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
pub(super) fn port_diag(
    net: &Netlist,
    def: DefId,
    elem: u32,
    callee: DefId,
    on_path: bool,
) -> Diag {
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
