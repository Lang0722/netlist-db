//! Instance paths (`x1.x2.m3`), found as HSPICE names the instances.
//!
//! An instance name can hold a `.` (`X0.AAA`), so a path is not split at
//! every `.`, and two instances can get one full name. HSPICE creates the
//! instances breadth-first: level by level, in a level by the order of the
//! parent instances, and in a parent by `X`-line order. The instance it
//! creates first gets the full name; HSPICE lists a later one as circuit
//! `.` (golden `hier_dotted_collision`, `hier_dotted_collision_deep`,
//! `hier_dotted_collision_order`, `hier_dotted_collision_order_rev`,
//! `hier_dotted_collision_xline_order`). The instances below the later one
//! still get full names from that name (`hier_dotted_collision_below`).
//! HSPICE finds an element by splitting its path at the last `.`: the first
//! instance created with the full name before it, then the element of that
//! name in it (golden `elem_dotted_name_collision`,
//! `elem_dotted_name_collision_rev`). So a device that only a later
//! instance with that full name holds is not found
//! (`hier_dotted_collision_hidden`). [`walk`] searches the instances whose
//! full names are prefixes of the path in creation order. Each `X` line on
//! the path binds to its definition along the path (see [`crate::hier`]).

use crate::{
    Netlist,
    api::{ElementInfo, MAX_LIMIT, PinNet, QueryError},
    hier::{Copies, suggest},
    intern::Sym,
    model::{DefId, ElemIdx, ElemKind, PinRole, ROOT, Root, pin_role},
};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// No instance.
const NONE: u32 = u32::MAX;

/// The first-run elements of one definition by name (the first line of
/// each name), and the length of its longest element name. Built on the
/// first walk through the definition.
#[derive(Debug, Default)]
pub(crate) struct ElemNames {
    index: HashMap<Sym, u32>,
    longest: usize,
}

/// A walked instance path.
pub(crate) struct Walk {
    /// Each element of the path: the definition that holds it, and its
    /// index there. Empty for the root.
    pub(crate) elems: Vec<(DefId, ElemIdx)>,
    /// The definition that the last element binds to, when it is an `X`
    /// line with a visible definition.
    pub(crate) last_binding: Option<DefId>,
    /// The definitions of the instances on the path, the root first.
    pub(crate) defs: Vec<DefId>,
    /// The path, lowercase.
    pub(crate) path: String,
    /// Other instances or elements with a full name on the path.
    pub(crate) caveats: Vec<String>,
    /// The lookup scopes of the instances on the path, the root first.
    pub(crate) scopes: Vec<InstScope>,
    /// The lookup scope of the instance that the path names, when it ends
    /// at an `X` line with a visible definition.
    pub(crate) end_scope: Option<InstScope>,
}

/// The lookup scope of an instance: what its `X` lines bind with.
#[derive(Debug, Clone)]
pub(crate) struct InstScope {
    pub(crate) def: DefId,
    /// Its lookup context (see [`Ctxs`]).
    pub(crate) ctx: Vec<DefId>,
    /// The length of its full name in the path (0 for the root).
    pub(crate) name_len: usize,
}

impl InstScope {
    /// The definition that an `X` line calling `callee` in this instance
    /// binds to (as [`bind`]).
    pub(crate) fn bind(&self, net: &Netlist, callee: Sym) -> Option<DefId> {
        let top = || net.def_index.get(&(ROOT, callee)).copied();
        if !net.hier.defs[self.def.idx()].hot {
            return top();
        }
        let own = (self.def != ROOT)
            .then(|| net.def_index.get(&(self.def, callee)))
            .flatten();
        own.or_else(|| {
            self.ctx
                .iter()
                .find_map(|&s| net.def_index.get(&(s, callee)))
        })
        .copied()
        .or_else(top)
    }

    /// The scope of the instance of `t` that an `X` line of this instance
    /// makes, whose full name is `name_len` long.
    pub(crate) fn child(&self, net: &Netlist, t: DefId, name_len: usize) -> InstScope {
        let h = &net.hier.defs;
        let ctx = if !h[t.idx()].hot {
            Vec::new()
        } else if h[self.def.idx()].scoped {
            canon(
                net,
                std::iter::once(self.def).chain(self.ctx.iter().copied()),
            )
        } else {
            self.ctx.clone()
        };
        InstScope {
            def: t,
            ctx,
            name_len,
        }
    }
}

/// What a path names when it names more than one element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Want {
    /// An element (`element_info`, `resolve_element`): HSPICE's element
    /// rule first (the device of that name in the first instance with the
    /// full name before the last `.`), else the instance rule.
    Element,
    /// A scope: the instance rule (the `X` line of the first instance
    /// created with that full name), else a device.
    Instance,
}

/// The elements of instance path `path`: for each, the definition that
/// holds it and its index there. `""` or `"."` is the root (no elements).
#[cfg(test)]
pub(crate) fn split_path(net: &Netlist, path: &str) -> Result<Vec<(DefId, ElemIdx)>, QueryError> {
    walk(net, path, Want::Element).map(|w| w.elems)
}

/// One instance of the search of [`walk`]: its full name is the path up to
/// `pos`, without the `.` there.
#[derive(Debug, Clone, Copy)]
struct Inst {
    def: DefId,
    /// The start of the rest of the path (0 for the root).
    pos: usize,
    /// Its lookup context (see [`Ctxs`]): 0 when `def` is not hot.
    ctx: u32,
    /// The instance above it ([`NONE`] for the root), and its `X` line in
    /// the definition of that instance.
    parent: u32,
    elem: ElemIdx,
}

/// The lookup contexts of the search: the definitions above an instance
/// that are lookup scopes, nearest first, without those that change no
/// lookup (see [`canon`]). Context 0 is empty. Only a hot definition (one
/// whose `X` lines, or those below it, can call a nested name) keeps its
/// context: the others bind every `X` line at the top level.
struct Ctxs {
    list: Vec<Vec<DefId>>,
    ids: HashMap<Vec<DefId>, u32>,
    /// The scopes in all the contexts.
    cells: usize,
}

impl Ctxs {
    fn id(&mut self, ctx: Vec<DefId>) -> u32 {
        if ctx.is_empty() {
            return 0;
        }
        if let Some(&id) = self.ids.get(&ctx) {
            return id;
        }
        let id = self.list.len() as u32;
        self.cells += ctx.len();
        self.list.push(ctx.clone());
        self.ids.insert(ctx, id);
        id
    }
}

/// The lookup scopes `scopes` (nearest first) without those that change no
/// lookup: a scope is kept when it is the nearest one for a name that it
/// nests. A lookup takes the nearest scope that nests the name, so a scope
/// that is dropped is never taken, and two contexts that bind every name
/// the same way get one id. A context is at most as long as the number of
/// names that are nested somewhere, so a deep chain of scopes that nest the
/// same names has short contexts.
fn canon(net: &Netlist, scopes: impl Iterator<Item = DefId>) -> Vec<DefId> {
    let mut covered: HashSet<Sym> = HashSet::new();
    let mut out = Vec::new();
    for s in scopes {
        let mut new = false;
        for &c in &net.defs[s.idx()].nested {
            let name = net.defs[c.idx()].name;
            if net.def_index.contains_key(&(s, name)) {
                new |= covered.insert(name);
            }
        }
        if new {
            out.push(s);
        }
    }
    out
}

/// The definition that an `X` line calling `callee` in the definition of
/// instance `it` binds to: a nested definition of the instance's own
/// definition, else of the nearest scope of its context, else the top-level
/// one.
fn bind(net: &Netlist, ctxs: &Ctxs, it: &Inst, callee: Sym) -> Option<DefId> {
    let top = || net.def_index.get(&(ROOT, callee)).copied();
    if !net.hier.defs[it.def.idx()].hot {
        return top();
    }
    let own = (it.def != ROOT)
        .then(|| net.def_index.get(&(it.def, callee)))
        .flatten();
    own.or_else(|| {
        ctxs.list[it.ctx as usize]
            .iter()
            .find_map(|&s| net.def_index.get(&(s, callee)))
    })
    .copied()
    .or_else(top)
}

impl Netlist {
    /// The name index of definition `def` (built on first use).
    fn elem_names(&self, def: DefId) -> &ElemNames {
        let all = self
            .elem_names
            .get_or_init(|| (0..self.defs.len()).map(|_| OnceLock::new()).collect());
        all[def.idx()].get_or_init(|| {
            let mut names = ElemNames::default();
            for (i, e) in self.defs[def.idx()].elements.iter().enumerate() {
                if e.alter.is_some() {
                    continue;
                }
                names.index.entry(e.name).or_insert(i as u32);
                names.longest = names.longest.max(self.syms.name(e.name).len());
            }
            names
        })
    }
}

/// The elements of definition `def` whose name is a prefix of `rest` that
/// ends at a `.` or at the end: (name length, index), in line order, the
/// first line of each name (HSPICE rejects two lines with one name,
/// diagnostic `duplicate_element`). Only the prefixes up to the longest
/// element name of `def` are looked up, in its name index.
fn candidates(net: &Netlist, def: DefId, rest: &str) -> Vec<(usize, ElemIdx)> {
    let names = net.elem_names(def);
    let bytes = rest.as_bytes();
    let scan = (names.longest + 1).min(bytes.len());
    let ends = (1..scan)
        .filter(|&k| bytes[k] == b'.')
        .chain((bytes.len() <= names.longest).then_some(bytes.len()));
    let mut out: Vec<(usize, ElemIdx)> = ends
        .filter_map(|end| {
            let s = net.syms.get(&rest[..end])?;
            names.index.get(&s).map(|&i| (end, i as ElemIdx))
        })
        .collect();
    out.sort_unstable_by_key(|c| c.1);
    out
}

/// Walks instance path `path` from the root.
///
/// The search goes through the instances whose full names are prefixes of
/// the path, in HSPICE's creation order (breadth-first, see the module
/// doc): at each one, the elements of its definition whose name is a prefix
/// of the rest of the path that ends at a `.` or at the end, in line order.
/// The first instance with a full name owns it. For an element,
/// [`Want::Element`] takes HSPICE's element rule: the device named by the
/// part after the last `.` in the first instance with the full name before
/// it (golden `elem_dotted_name_collision`). An instance path names the
/// first instance created with that full name (golden
/// `hier_dotted_collision_order`). The search keeps the later instances
/// with a full name, because the instances below them get full names too
/// (`hier_dotted_collision_below`). Two instances with one place in the
/// path, one definition and one lookup context have the same instances
/// below them, and the later one never creates an instance first, so only
/// the first is kept, and it counts for both. A definition that is not hot
/// has one context, so on a deck with no nested definitions the search has
/// at most one instance for each `.` of the path. When more than one
/// instance or element gets a full name on the path, the answer has a
/// caveat. With no element, the error comes from the first instance with
/// the longest full name.
pub(crate) fn walk(net: &Netlist, path: &str, want: Want) -> Result<Walk, QueryError> {
    let root = match net.root {
        Root::TopLevel => ROOT,
        Root::Subckt(id) => id,
        Root::None => {
            return Err(QueryError::BadArgument(
                "the deck has no top-level instances (a bare netlist), so instance paths have no root: load it with `top` set to a subckt name".to_owned(),
            ));
        }
    };
    let full = path.trim();
    let h = &net.hier.defs;
    let mut ctxs = Ctxs {
        list: vec![Vec::new()],
        ids: HashMap::new(),
        cells: 0,
    };
    // a nested `top` sees the definitions it is written inside, as if it
    // were instantiated in an instance of each, nearest first
    // (golden/NOTES.md, "Choices not probed")
    let root_ctx = if h[root.idx()].hot {
        let mut parents = Vec::new();
        let mut at = net.defs[root.idx()].parent;
        while let Some(p) = at {
            if h[p.idx()].scoped {
                parents.push(p);
            }
            at = net.defs[p.idx()].parent;
        }
        let ctx = canon(net, parents.into_iter());
        ctxs.id(ctx)
    } else {
        0
    };
    if full.is_empty() || full == "." {
        return Ok(Walk {
            elems: Vec::new(),
            last_binding: None,
            defs: vec![root],
            path: String::new(),
            caveats: Vec::new(),
            scopes: vec![InstScope {
                def: root,
                ctx: ctxs.list[root_ctx as usize].clone(),
                name_len: 0,
            }],
            end_scope: None,
        });
    }
    if full.ends_with('.') {
        return Err(QueryError::BadArgument(format!(
            "the path `{full}` ends with `.`: name an element after it"
        )));
    }
    let mut insts = vec![Inst {
        def: root,
        pos: 0,
        ctx: root_ctx,
        parent: NONE,
        elem: 0,
    }];
    // by place in the path: the first instance there
    let mut first = vec![NONE; full.len() + 2];
    first[0] = 0;
    // the instance kept for each (place, definition, context), and each
    // (instance kept, instance whose `X` line makes it): the instances that
    // are not kept count through these
    let mut kept: HashMap<(usize, DefId, u32), u32> = HashMap::new();
    let mut makers: Vec<(u32, u32)> = Vec::new();
    // the elements that the whole path names, in creation order: `X` lines
    // and devices, (instance, element)
    let mut found_x: Vec<(u32, ElemIdx)> = Vec::new();
    let mut found_dev: Vec<(u32, ElemIdx)> = Vec::new();
    // as the hierarchy analysis: only a deck that also stops it can get here
    let lines: usize = net.defs.iter().map(|d| d.elements.len()).sum();
    let budget = 4 * (net.defs.len() + lines) + 4_000_000;
    let mut k = 0;
    while k < insts.len() {
        let it = insts[k];
        let elements = &net.defs[it.def.idx()].elements;
        // the context of the instances it makes, once
        let mut below: Option<u32> = None;
        for (n, i) in candidates(net, it.def, &full[it.pos..]) {
            let e = &elements[i];
            let end = it.pos + n;
            if end == full.len() {
                if e.kind == ElemKind::Instance {
                    found_x.push((k as u32, i));
                } else {
                    found_dev.push((k as u32, i));
                }
                continue;
            }
            if e.kind != ElemKind::Instance {
                continue;
            }
            let Some(t) = e.callee.and_then(|c| bind(net, &ctxs, &it, c)) else {
                continue;
            };
            let ctx = if h[t.idx()].hot {
                *below.get_or_insert_with(|| {
                    if h[it.def.idx()].scoped {
                        let above = ctxs.list[it.ctx as usize].iter().copied();
                        let ctx = canon(net, std::iter::once(it.def).chain(above));
                        ctxs.id(ctx)
                    } else {
                        it.ctx
                    }
                })
            } else {
                0
            };
            let pos = end + 1;
            let made = match kept.get(&(pos, t, ctx)) {
                Some(&j) => j,
                None => {
                    if insts.len() + ctxs.cells >= budget {
                        return Err(QueryError::BadArgument(format!(
                            "the path `{full}` has too many readings: more than {budget} instances and lookup scopes have full names that are prefixes of it (instance names with a `.`, nested definitions used in very many combinations)"
                        )));
                    }
                    let j = insts.len() as u32;
                    if first[pos] == NONE {
                        first[pos] = j;
                    }
                    insts.push(Inst {
                        def: t,
                        pos,
                        ctx,
                        parent: k as u32,
                        elem: i,
                    });
                    kept.insert((pos, t, ctx), j);
                    j
                }
            };
            makers.push((made, k as u32));
        }
        k += 1;
    }
    // the number of instances each kept one stands for: the sum over the
    // instances that make it (a maker is at a lower place, so its number is
    // known first)
    let mut mult = vec![0_u64; insts.len()];
    mult[0] = 1;
    makers.sort_unstable_by_key(|&(m, _)| insts[m as usize].pos);
    for &(m, p) in &makers {
        mult[m as usize] = mult[m as usize].saturating_add(mult[p as usize]);
    }
    // by place: the number of instances with that full name; the `X` lines
    // that the whole path names make the instances at `full.len() + 1`
    let mut count = vec![0_u64; full.len() + 2];
    for (j, it) in insts.iter().enumerate() {
        count[it.pos] = count[it.pos].saturating_add(mult[j]);
    }
    for &(k, _) in &found_x {
        count[full.len() + 1] = count[full.len() + 1].saturating_add(mult[k as usize]);
    }
    // HSPICE's element rule: the first instance with the full name before
    // the last `.` (the root when the path has none), then the device
    let cpos = full.rfind('.').map_or(0, |l| l + 1);
    let owner = first[cpos];
    let dev = found_dev.iter().copied().find(|&(k, _)| k == owner);
    let pick = match want {
        Want::Element => dev.or_else(|| found_x.first().copied()),
        Want::Instance => found_x.first().copied().or(dev),
    };
    // a device with a `.` in its name, whose instance does not have the
    // full name before the last `.` of the path (not probed)
    let mut dotted = false;
    let (at, i) = match pick {
        Some(p) => p,
        None => {
            // a device that only a later instance with that full name holds
            let later = found_dev
                .iter()
                .copied()
                .find(|&(k, _)| insts[k as usize].pos == cpos);
            let own = found_dev
                .iter()
                .copied()
                .find(|&(k, _)| first[insts[k as usize].pos] == k);
            match (later, own, found_dev.first()) {
                (None, Some(p), _) => {
                    dotted = true;
                    p
                }
                (Some((k, i)), _, _) | (None, None, Some(&(k, i))) => {
                    return Err(hidden(net, &insts, &first, k, i, full));
                }
                (None, None, None) => return Err(no_element(net, &ctxs, &insts, &first, full)),
            }
        }
    };
    // the instances on the path, the root first
    let mut chain = vec![at];
    while let Some(&k) = chain.last()
        && insts[k as usize].parent != NONE
    {
        chain.push(insts[k as usize].parent);
    }
    chain.reverse();
    let it = &insts[at as usize];
    let e = &net.defs[it.def.idx()].elements[i];
    let last_binding = (e.kind == ElemKind::Instance)
        .then(|| e.callee.and_then(|c| bind(net, &ctxs, it, c)))
        .flatten();
    let mut elems: Vec<(DefId, ElemIdx)> = chain[1..]
        .iter()
        .map(|&k| {
            let x = &insts[k as usize];
            (insts[x.parent as usize].def, x.elem)
        })
        .collect();
    elems.push((it.def, i));
    let lower = full.to_ascii_lowercase();
    let mut caveats = Vec::new();
    // the full names on the path that more than one instance gets
    let mut places: Vec<usize> = chain[1..].iter().map(|&k| insts[k as usize].pos).collect();
    if e.kind == ElemKind::Instance {
        places.push(full.len() + 1);
    }
    let shared: Vec<usize> = places.into_iter().filter(|&p| count[p] > 1).collect();
    if let Some(&pos) = shared.first() {
        let more = match shared.len() - 1 {
            0 => String::new(),
            1 => ", and 1 more full name on the path is shared".to_owned(),
            m => format!(", and {m} more full names on the path are shared"),
        };
        caveats.push(format!(
            "{} instances have the full name `{}` (instance names with a `.`){more}: HSPICE gives a full name to the instance it creates first, breadth-first, and lists the others as circuit `.` (golden `hier_dotted_collision_order`); this answer follows it",
            count[pos],
            &lower[..pos - 1]
        ));
    }
    let named = found_x.len() + found_dev.len();
    if dotted {
        let why = if owner == NONE {
            format!("no instance has the full name `{}`", &lower[..cpos - 1])
        } else {
            format!(
                "the first instance with the full name `{}` has no `{}`",
                &lower[..cpos - 1],
                &lower[cpos..]
            )
        };
        caveats.push(format!(
            "HSPICE splits an element path at its last `.` (golden `elem_dotted_name_collision`), and {why}: this answer is a device with a `.` in its name. Not probed: HSPICE may not find it"
        ));
    } else if named > 1 {
        let rule = if Some((at, i)) == dev {
            "HSPICE splits an element path at its last `.` and takes the element of the first instance with the full name before it (golden `elem_dotted_name_collision`)"
        } else {
            "an instance path names the instance created first (golden `hier_dotted_collision_order`)"
        };
        caveats.push(format!(
            "`{lower}` names {named} elements (names with a `.`): {rule}; this answer follows it"
        ));
    }
    let scopes: Vec<InstScope> = chain
        .iter()
        .map(|&k| {
            let x = &insts[k as usize];
            InstScope {
                def: x.def,
                ctx: ctxs.list[x.ctx as usize].clone(),
                name_len: x.pos.saturating_sub(1),
            }
        })
        .collect();
    let end_scope = last_binding.map(|t| {
        scopes
            .last()
            .expect("the root is on the path")
            .child(net, t, full.len())
    });
    Ok(Walk {
        elems,
        last_binding,
        defs: chain.iter().map(|&k| insts[k as usize].def).collect(),
        path: lower,
        caveats,
        scopes,
        end_scope,
    })
}

/// The error of a path whose device `i` only the later instance `k` with
/// its full name holds: HSPICE gives the name to the first instance, which
/// has no such element (golden `hier_dotted_collision_hidden`).
fn hidden(
    net: &Netlist,
    insts: &[Inst],
    first: &[u32],
    k: u32,
    i: ElemIdx,
    full: &str,
) -> QueryError {
    let lower = full.to_ascii_lowercase();
    let it = &insts[k as usize];
    let owner = &insts[first[it.pos] as usize];
    QueryError::BadArgument(format!(
        "`{}{}` is in a later instance with the full name `{}` (a `{}`): HSPICE gives that name to the instance it creates first (a `{}`), which has no such element (golden `hier_dotted_collision_hidden`)",
        &lower[..it.pos],
        net.syms
            .name(net.defs[it.def.idx()].elements[i].name)
            .to_ascii_lowercase(),
        &lower[..it.pos - 1],
        net.def_display(it.def),
        net.def_display(owner.def)
    ))
}

/// The error of a path that names no element: the error of the first
/// instance with the longest full name.
fn no_element(net: &Netlist, ctxs: &Ctxs, insts: &[Inst], first: &[u32], full: &str) -> QueryError {
    let lower = full.to_ascii_lowercase();
    let shown = |it: &Inst, i: ElemIdx| {
        format!(
            "{}{}",
            &lower[..it.pos],
            net.syms
                .name(net.defs[it.def.idx()].elements[i].name)
                .to_ascii_lowercase()
        )
    };
    let pos = (0..first.len().min(full.len() + 1))
        .rev()
        .find(|&p| first[p] != NONE)
        .expect("the root is at place 0");
    let it = &insts[first[pos] as usize];
    let d = &net.defs[it.def.idx()];
    let cands = candidates(net, it.def, &full[pos..]);
    // the candidates do not end the path: `X` lines with no visible
    // definition, or devices
    let unbound = cands.iter().map(|c| c.1).find(|&i| {
        let e = &d.elements[i];
        e.kind == ElemKind::Instance && e.callee.and_then(|c| bind(net, ctxs, it, c)).is_none()
    });
    let device = cands
        .iter()
        .map(|c| c.1)
        .find(|&i| d.elements[i].kind != ElemKind::Instance);
    if let Some(i) = unbound {
        let callee = d.elements[i].callee.map_or("", |c| net.syms.name(c));
        QueryError::BadArgument(format!(
            "`{}` calls subckt `{callee}`, which has no visible definition on this path (diagnostic `unresolved_subckt`): the path cannot go on below it",
            shown(it, i)
        ))
    } else if let Some(i) = device {
        QueryError::BadArgument(format!(
            "`{}` is a {}, not a subckt instance: the path cannot go on below it",
            shown(it, i),
            d.elements[i].kind.name()
        ))
    } else {
        let rest = &full[pos..];
        let segment = rest.split('.').next().unwrap_or(rest);
        let above = &lower[..pos];
        let names = d.elements.iter().filter(|e| e.alter.is_none()).map(|e| {
            let name = net.syms.name(e.name);
            (name, format!("{above}{}", name.to_ascii_lowercase()))
        });
        QueryError::NotFound {
            what: "element".to_owned(),
            name: full.to_owned(),
            suggestions: suggest(segment, names),
        }
    }
}

impl Netlist {
    /// One element (device or `X` line) by its instance path from the root
    /// (`x1.x2.m3`; any letter case): type, the definition that holds it,
    /// pins and nets, model, the definition an `X` line binds to on this
    /// path, and its parameters as written.
    pub fn element_info(&self, path: &str) -> Result<ElementInfo, QueryError> {
        let mut w = walk(self, path, Want::Element)?;
        let Some(&(def, i)) = w.elems.last() else {
            return Err(QueryError::BadArgument(
                "the path is empty: it names the root, not an element".to_owned(),
            ));
        };
        let d = &self.defs[def.idx()];
        let e = &d.elements[i];
        let bound = w.last_binding.map(|t| &self.defs[t.idx()]);
        let pins_total = d.element_pins(e).len();
        // port names only when the node count is the port count, as in
        // `net_info`: else `n1`, `n2`, …
        let port_names = bound.filter(|b| b.ports.len() == pins_total);
        let pins: Vec<PinNet> = d
            .element_pins(e)
            .iter()
            .take(MAX_LIMIT)
            .enumerate()
            .map(|(k, &net)| PinNet {
                role: match pin_role(e.kind, k) {
                    PinRole::Fixed(name) => name.to_owned(),
                    PinRole::Extra(n) => format!("e{n}"),
                    PinRole::Port(p) => port_names
                        .and_then(|b| b.ports.get(p as usize))
                        .map_or_else(|| format!("n{}", p + 1), |&s| self.syms.name(s).to_owned()),
                    PinRole::Node(n) => format!("n{n}"),
                },
                net: self.syms.name(net).to_owned(),
            })
            .collect();
        let mut caveats = std::mem::take(&mut w.caveats);
        if let ElemKind::Other(_) = e.kind {
            caveats.push(
                "element type not parsed: its node list and its parameters are not trusted"
                    .to_owned(),
            );
        }
        let callee = e.callee.map(|c| self.syms.name(c).to_owned());
        if let (ElemKind::Instance, None, Some(callee)) = (e.kind, bound, &callee) {
            caveats.push(format!(
                "no definition of subckt `{callee}` is visible on this path (diagnostic `unresolved_subckt`); its pins are listed as n1, n2, …"
            ));
        }
        if let (Some(b), None) = (bound, port_names) {
            caveats.push(format!(
                "`{}` has {pins_total} nodes, but subckt `{}` has {} ports: HSPICE rejects the deck (diagnostic `port_count_mismatch`); its pins are listed as n1, n2, …",
                self.syms.name(e.name),
                self.def_display(w.last_binding.expect("bound")),
                b.ports.len()
            ));
        }
        caveats.extend(self.path_caveats(&w));
        caveats.extend(self.path_alter_caveats(&w));
        // an R, C or L line as HSPICE reads the tokens after the nodes
        let (model, params, params_total) = match self.rcl_reading(def, i) {
            Some((model, params, why)) => {
                caveats.extend(why);
                let total = params.len();
                let params = params.into_iter().take(MAX_LIMIT).collect();
                (model, params, total)
            }
            None => (
                e.model,
                d.element_params(e)
                    .iter()
                    .take(MAX_LIMIT)
                    .map(|p| self.name_text(p))
                    .collect(),
                d.element_params(e).len(),
            ),
        };
        Ok(ElementInfo {
            path: w.path,
            kind: e.kind.name().to_owned(),
            def: self.def_display(def),
            pins,
            pins_total,
            model: model.map(|m| self.syms.name(m).to_owned()),
            callee,
            binds_to: w.last_binding.map(|t| self.def_display(t)),
            params,
            params_total,
            loc: self.loc(e.loc),
            caveats,
        })
    }

    /// The caveats of an answer about the walked path `w` (beyond
    /// `w.caveats`): HSPICE rejects the deck because of an `X` line on the
    /// path or the element at its end (a node count that is not the port
    /// count, `m` <= 0 as written, a name of more than one line), a call
    /// cycle, or a duplicate definition on the path. One caveat for each
    /// kind, with the number of other lines or definitions of that kind.
    pub(crate) fn path_caveats(&self, w: &Walk) -> Vec<String> {
        let mut caveats = Vec::new();
        // an `X` line on the path whose node count is not the port count
        // of the definition it binds to (golden `hier_port_more`,
        // `hier_port_fewer`): `defs[k + 1]` is the binding of `elems[k]`
        let mut ports = Capped::new("`X` line");
        for (k, &(at, j)) in w.elems.iter().enumerate() {
            let holder = &self.defs[at.idx()];
            let x = &holder.elements[j];
            let binding = w.defs.get(k + 1).copied().or(w.last_binding);
            if x.kind != ElemKind::Instance {
                continue;
            }
            if let Some(b) = binding {
                let (nodes, ports_n) =
                    (holder.element_pins(x).len(), self.defs[b.idx()].ports.len());
                if nodes != ports_n {
                    ports.add(|| {
                        format!(
                            "`{}` has {nodes} nodes, but subckt `{}` has {ports_n} ports: HSPICE rejects the deck (diagnostic `port_count_mismatch`)",
                            self.syms.name(x.name),
                            self.def_display(b)
                        )
                    });
                }
            }
        }
        caveats.extend(ports.done());
        // the definitions of the instances on the path, and the one the
        // last `X` line binds to
        let in_cycle = w
            .defs
            .iter()
            .chain(&w.last_binding)
            .find(|d| self.hier.defs[d.idx()].copies == Copies::Cycle);
        if let Some(&d) = in_cycle {
            caveats.push(format!(
                "subckt `{}` is in or below a call cycle (diagnostic `subckt_cycle`): HSPICE rejects this deck",
                self.def_display(d)
            ));
        }
        // `m` <= 0 on the element or on an `X` line of the path
        if let Some(m) = self.syms.get("m") {
            let mut ms = Capped::new("line");
            for &(at, k) in &w.elems {
                let holder = &self.defs[at.idx()];
                let x = &holder.elements[k];
                if let Some(text) = self.m_not_positive(holder, x, m) {
                    ms.add(|| {
                        format!(
                            "`{}` has `m={text}`: HSPICE rejects a multiplier of zero or less on an instance path (diagnostic `m_not_positive`)",
                            self.syms.name(x.name)
                        )
                    });
                }
            }
            caveats.extend(ms.done());
        }
        // a name of more than one line of its definition: the walk took
        // the first line
        let mut dups = Capped::new("name");
        for &(at, k) in &w.elems {
            let x = &self.defs[at.idx()].elements[k];
            if self.hier.dup_element(at, x.name) {
                let why = if self.defs[at.idx()].has_if {
                    "the lines can be in different `.if` branches"
                } else {
                    "HSPICE rejects this deck"
                };
                dups.add(|| {
                    format!(
                        "`{}` names more than one element line in `{}`: {why} (diagnostic `duplicate_element`). This answer uses the first line",
                        self.syms.name(x.name),
                        self.def_display(at)
                    )
                });
            }
        }
        caveats.extend(dups.done());
        let mut defs = Capped::new("subckt");
        for &on_path in w.defs.iter().chain(&w.last_binding) {
            if self.defs[on_path.idx()].has_dup {
                defs.add(|| {
                    format!(
                        "HSPICE rejects this deck: subckt `{}` is defined more than once (golden `dup_subckt`). Queries use the first definition",
                        self.def_display(on_path)
                    )
                });
            }
        }
        caveats.extend(defs.done());
        caveats
    }
}

/// The first caveat of one kind, and the number of the others.
pub(crate) struct Capped {
    first: Option<String>,
    more: usize,
    what: &'static str,
}

impl Capped {
    pub(crate) fn new(what: &'static str) -> Self {
        Capped {
            first: None,
            more: 0,
            what,
        }
    }

    pub(crate) fn add(&mut self, text: impl FnOnce() -> String) {
        if self.first.is_none() {
            self.first = Some(text());
        } else {
            self.more += 1;
        }
    }

    pub(crate) fn done(self) -> Option<String> {
        let mut first = self.first?;
        match self.more {
            0 => {}
            1 => first.push_str(&format!(
                " (1 more {} on the path has the same problem)",
                self.what
            )),
            m => first.push_str(&format!(
                " ({m} more {}s on the path have the same problem)",
                self.what
            )),
        }
        Some(first)
    }
}

#[cfg(test)]
mod tests {
    use super::split_path;
    use crate::{
        LoadOptions, Netlist, QueryError,
        model::{DefId, ROOT},
    };
    use std::path::Path;

    #[test]
    fn split_path_levels() {
        let deck = Path::new(env!("CARGO_MANIFEST_DIR")).join("../golden/cases/nested_def/deck.sp");
        let net = Netlist::load(&LoadOptions::new(deck)).unwrap();
        let def = |name: &str| match net.find_def(name) {
            crate::DefLookup::Found(id) => id,
            other => panic!("{other:?}"),
        };
        assert!(split_path(&net, "").unwrap().is_empty());
        assert!(split_path(&net, " . ").unwrap().is_empty());
        let got = split_path(&net, "X1.xd.XC.rc").unwrap();
        let defs: Vec<DefId> = got.iter().map(|&(d, _)| d).collect();
        assert_eq!(
            defs,
            [ROOT, def("outer"), def("outer>deep"), def("outer>cell")]
        );
        let (d, i) = got[3];
        assert_eq!(net.syms.name(net.defs[d.idx()].elements[i].name), "rc");
        assert!(matches!(
            split_path(&net, "x1.nosuch"),
            Err(QueryError::NotFound { .. })
        ));
        assert!(matches!(
            split_path(&net, "x1."),
            Err(QueryError::BadArgument(_))
        ));
    }
}
