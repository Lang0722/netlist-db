//! Connectivity: the pins on a net in one scope ([`Netlist::net_info`]),
//! and where a net goes through ports ([`Netlist::trace_net`]).
//!
//! A net is a name in the definition of its scope. Ground (`0`, `gnd`,
//! `gnd!`, `ground`, any letter case: golden `ground_alias`) and the
//! `.global` names (golden `global_net`) are one net at every level, so a
//! trace does not follow them through ports. An `X` line binds along the
//! instance path (see [`crate::paths`]), so its port names come from that
//! binding. Nothing is expanded: a definition gets its net index on its
//! first query, and a trace visits only the instances it reaches, up to its
//! depth and [`MAX_HOPS`] steps.

use crate::{
    Netlist,
    api::{Conn, Hop, MAX_LIMIT, NetInfo, NetTrace, NetView, Page, QueryError, TraceDir, page},
    dspf::Dspf,
    hier::{FxMap, suggest},
    intern::Sym,
    model::{Def, DefId, ElemIdx, ElemKind, PinRole, ROOT, Root, pin_role},
    paths::{Capped, InstScope, Want, walk},
};
use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};
use std::sync::OnceLock;

/// The names that HSPICE reads as ground (golden `ground_alias`).
pub(crate) const GROUND: [&str; 4] = ["0", "gnd", "gnd!", "ground"];

/// The key of ground in a [`NetIndex`]; the other keys are symbol ids.
pub(crate) const GROUND_KEY: u32 = u32::MAX;

/// A key that no net has.
const NO_KEY: u32 = u32::MAX - 1;

/// The most steps of one trace: a trace that would find more stops there.
pub(crate) const MAX_HOPS: usize = 100_000;

/// The pins of one definition by net, built on the first query of the
/// definition, in two passes over its pins (count, then fill). The pins of
/// each net are a range of `pins`, in line order (8 bytes per pin). A
/// definition with many pins for the number of names in the deck keeps the
/// ranges in an array by name id (`dense`, 4 bytes per name); the others in
/// a map (one entry per net).
#[derive(Debug, Default)]
pub(crate) struct NetIndex {
    /// By name id, the start of its range, and ground at the end; empty when
    /// `nets` is used.
    dense: Vec<u32>,
    nets: FxMap<u32, (u32, u32)>,
    /// (element, pin of the element)
    pins: Vec<(u32, u32)>,
}

impl NetIndex {
    pub(crate) fn pins_of(&self, key: u32) -> &[(u32, u32)] {
        if self.dense.is_empty() {
            return self
                .nets
                .get(&key)
                .map_or(&[], |&(a, b)| &self.pins[a as usize..b as usize]);
        }
        let slot = if key == GROUND_KEY {
            self.dense.len() - 2
        } else {
            key as usize
        };
        match (self.dense.get(slot), self.dense.get(slot + 1)) {
            (Some(&a), Some(&b)) if slot + 1 < self.dense.len() => {
                &self.pins[a as usize..b as usize]
            }
            _ => &[],
        }
    }

    /// The keys of the nets with pins.
    fn keys(&self) -> Vec<u32> {
        if self.dense.is_empty() {
            return self.nets.keys().copied().collect();
        }
        let ground = self.dense.len() - 2;
        (0..self.dense.len() - 1)
            .filter(|&s| self.dense[s] < self.dense[s + 1])
            .map(|s| if s == ground { GROUND_KEY } else { s as u32 })
            .collect()
    }
}

/// An instance on a walked path, or one that a trace reaches.
#[derive(Debug, Clone)]
struct Node {
    scope: InstScope,
    /// Its instance path, lowercase (`""`: the root).
    path: String,
    /// Its place in the chain of the start scope (see [`Netlist::chain`]):
    /// the trace goes up only from there.
    up: Option<usize>,
}

/// One instance of the chain from the root to a scope.
struct Link {
    node: Node,
    /// The `X` line that makes it: (the definition of the parent, its
    /// line). `None` for the root.
    xline: Option<(DefId, ElemIdx)>,
}

fn is_ground_name(name: &str) -> bool {
    GROUND.iter().any(|g| g.eq_ignore_ascii_case(name))
}

impl Netlist {
    /// The key of net `s` in a [`NetIndex`].
    pub(crate) fn net_key(&self, s: Sym) -> u32 {
        if is_ground_name(self.syms.name(s)) {
            GROUND_KEY
        } else {
            s.0
        }
    }

    /// The key of a net name as asked (any letter case).
    fn asked_key(&self, name: &str) -> u32 {
        if is_ground_name(name) {
            GROUND_KEY
        } else {
            self.syms.get(name).map_or(NO_KEY, |s| s.0)
        }
    }

    /// The logical net of net `key` of `def`: with an included DSPF, the
    /// `*|NET` block that names the node (and the net key of the block);
    /// else `key`.
    fn logical(&self, def: DefId, key: u32) -> (u32, Option<(&Dspf, u32)>) {
        if let Some(d) = self.included_dspf(def)
            && let Some(&i) = d.by_node.get(&key)
        {
            return (d.nets[i as usize].key.unwrap_or(key), Some((d, i)));
        }
        (key, None)
    }

    /// The pins on `nodes` of `def`, in line order. In a definition with an
    /// included DSPF, without its parasitic R and C unless `physical`.
    fn pins_on<'a>(&'a self, def: DefId, nodes: &[u32], physical: bool) -> Cow<'a, [(u32, u32)]> {
        let index = self.net_index(def);
        let filter = !physical && self.included_dspf(def).is_some();
        if let [one] = nodes
            && !filter
        {
            return Cow::Borrowed(index.pins_of(*one));
        }
        let d = &self.defs[def.idx()];
        let mut out: Vec<(u32, u32)> = nodes
            .iter()
            .flat_map(|&n| index.pins_of(n))
            .filter(|&&(i, _)| !filter || !d.elements[i as usize].parasitic)
            .copied()
            .collect();
        out.sort_unstable();
        Cow::Owned(out)
    }

    /// The pins of the logical net `key` of `def` (see [`Self::logical`]).
    fn logical_pins(&self, def: DefId, key: u32) -> Cow<'_, [(u32, u32)]> {
        match self.logical(def, key) {
            (_, Some((d, i))) => self.pins_on(def, &d.nets[i as usize].nodes, false),
            (k, None) => self.pins_on(def, &[k], false),
        }
    }

    /// The name of logical net `lkey`, for the node `node` that it holds
    /// (`0` for ground: a DSPF block named `gnd` holds other nodes).
    fn logical_name(&self, lkey: u32, node: Sym) -> &str {
        if lkey == self.net_key(node) {
            self.syms.name(node)
        } else if lkey == GROUND_KEY {
            "0"
        } else {
            self.syms.name(Sym(lkey))
        }
    }

    /// The net index of definition `def` (built on first use).
    pub(crate) fn net_index(&self, def: DefId) -> &NetIndex {
        let all = self
            .net_index
            .get_or_init(|| (0..self.defs.len()).map(|_| OnceLock::new()).collect());
        all[def.idx()].get_or_init(|| {
            let ground: Vec<Sym> = GROUND.iter().filter_map(|g| self.syms.get(g)).collect();
            let key = |n: Sym| if ground.contains(&n) { GROUND_KEY } else { n.0 };
            let d = &self.defs[def.idx()];
            let live = || {
                d.elements
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.alter.is_none())
            };
            let total: usize = live().map(|(_, e)| d.element_pins(e).len()).sum();
            let mut pins = vec![(0_u32, 0_u32); total];
            let names = self.syms.len();
            if total * 4 >= names {
                // by name id, ground last: the end of each range, then a
                // fill in reverse line order moves it to the start
                let slot = |n: Sym| match key(n) {
                    GROUND_KEY => names,
                    k => k as usize,
                };
                let mut dense = vec![0_u32; names + 2];
                for (_, e) in live() {
                    for &n in d.element_pins(e) {
                        dense[slot(n) + 1] += 1;
                    }
                }
                for s in 1..dense.len() {
                    dense[s] += dense[s - 1];
                }
                // `dense[s + 1]` is the end of the range of `s`
                for (i, e) in d.elements.iter().enumerate().rev() {
                    if e.alter.is_some() {
                        continue;
                    }
                    for (k, &n) in d.element_pins(e).iter().enumerate().rev() {
                        let end = &mut dense[slot(n) + 1];
                        *end -= 1;
                        pins[*end as usize] = (i as u32, k as u32);
                    }
                }
                // now `dense[s + 1]` is the start of `s`: shift down by one
                dense.remove(0);
                dense.push(total as u32);
                return NetIndex {
                    dense,
                    nets: FxMap::default(),
                    pins,
                };
            }
            // count the pins of each net, then give each net its range
            let mut nets: FxMap<u32, (u32, u32)> = FxMap::default();
            for (_, e) in live() {
                for &n in d.element_pins(e) {
                    nets.entry(key(n)).or_default().1 += 1;
                }
            }
            let mut at = 0;
            for range in nets.values_mut() {
                let count = range.1;
                *range = (at, at);
                at += count;
            }
            // fill, in line order: the end of each range is its cursor
            for (i, e) in live() {
                for (k, &n) in d.element_pins(e).iter().enumerate() {
                    let range = nets.get_mut(&key(n)).expect("counted");
                    pins[range.1 as usize] = (i as u32, k as u32);
                    range.1 += 1;
                }
            }
            nets.shrink_to_fit();
            NetIndex {
                dense: Vec::new(),
                nets,
                pins,
            }
        })
    }

    /// The `.global` names: by key.
    fn is_global_key(&self, key: u32) -> bool {
        key != GROUND_KEY && self.globals.iter().any(|g| g.0 == key)
    }

    /// The chain of instances from the root to the instance that
    /// `scope_path` names, and the caveats of the path. A device, or an `X`
    /// line with no visible definition, is not a scope.
    fn chain(&self, scope_path: &str) -> Result<(Vec<Link>, Vec<String>), QueryError> {
        let p = scope_path.trim();
        if (p.is_empty() || p == ".") && self.root == Root::None {
            // a bare netlist: the top level
            let node = Node {
                scope: InstScope {
                    def: ROOT,
                    ctx: Vec::new(),
                    name_len: 0,
                },
                path: String::new(),
                up: Some(0),
            };
            return Ok((vec![Link { node, xline: None }], Vec::new()));
        }
        let w = walk(self, p, Want::Instance)?;
        if let Some(&(holder, i)) = w.elems.last() {
            let e = &self.defs[holder.idx()].elements[i];
            if e.kind != ElemKind::Instance {
                return Err(QueryError::BadArgument(format!(
                    "`{}` is a {}, not a subckt instance: give the path of the instance that holds it (or use element_info for its nets)",
                    w.path,
                    e.kind.name()
                )));
            }
            if w.end_scope.is_none() {
                return Err(QueryError::BadArgument(format!(
                    "`{}` calls subckt `{}`, which has no visible definition on this path (diagnostic `unresolved_subckt`): it has no nets",
                    w.path,
                    e.callee.map_or("", |c| self.syms.name(c))
                )));
            }
        }
        let mut caveats = w.caveats.clone();
        caveats.extend(self.path_caveats(&w));
        let mut links: Vec<Link> = w
            .scopes
            .iter()
            .enumerate()
            .map(|(j, s)| Link {
                node: Node {
                    scope: s.clone(),
                    path: w.path[..s.name_len].to_owned(),
                    up: Some(j),
                },
                xline: (j > 0).then(|| w.elems[j - 1]),
            })
            .collect();
        if let Some(end) = &w.end_scope {
            let j = links.len();
            links.push(Link {
                node: Node {
                    scope: end.clone(),
                    path: w.path.clone(),
                    up: Some(j),
                },
                xline: w.elems.last().copied(),
            });
        }
        Ok((links, caveats))
    }

    /// The role of pin `k` of element `i` of `d`, whose `X` lines bind in
    /// `scope`. An `X` line with no visible definition, or with a node count
    /// that is not the port count, has no port names: `n1`, `n2`, … and a
    /// note in `notes`.
    fn pin_role_in(
        &self,
        d: &Def,
        scope: &InstScope,
        i: usize,
        k: usize,
        notes: &mut (Capped, Capped),
    ) -> String {
        let e = &d.elements[i];
        match pin_role(e.kind, k) {
            PinRole::Fixed(n) => n.to_owned(),
            PinRole::Extra(n) => format!("e{n}"),
            PinRole::Node(n) => format!("n{n}"),
            PinRole::Port(p) => {
                let x = self.syms.name(e.name);
                let callee = e.callee.map_or("", |c| self.syms.name(c));
                match e.callee.and_then(|c| scope.bind(self, c)) {
                    Some(t) => {
                        let ports = &self.defs[t.idx()].ports;
                        let nodes = d.element_pins(e).len();
                        if ports.len() == nodes {
                            self.syms.name(ports[p as usize]).to_owned()
                        } else {
                            notes.1.add(|| {
                                format!(
                                    "`{x}` has {nodes} nodes, but subckt `{}` has {} ports: HSPICE rejects the deck (diagnostic `port_count_mismatch`); its pins are listed as n1, n2, …",
                                    self.def_display(t),
                                    ports.len()
                                )
                            });
                            format!("n{}", p + 1)
                        }
                    }
                    None => {
                        notes.0.add(|| {
                            format!(
                                "`{x}` calls subckt `{callee}`, which has no visible definition on this path (diagnostic `unresolved_subckt`); its pins are listed as n1, n2, …"
                            )
                        });
                        format!("n{}", p + 1)
                    }
                }
            }
        }
    }

    /// The elements of a type that the parser does not read among `pins`
    /// of `def`.
    /// The names of the element lines of `def` that a load error skipped.
    fn skipped_in(&self, def: DefId) -> impl Iterator<Item = &str> {
        let at = self.skipped.partition_point(|s| s.0 < def);
        self.skipped[at..]
            .iter()
            .take_while(move |s| s.0 == def)
            .map(|s| s.1.as_str())
    }

    fn unparsed_in(&self, def: DefId, pins: &[(u32, u32)]) -> Vec<ElemIdx> {
        let d = &self.defs[def.idx()];
        let mut out: Vec<ElemIdx> = pins
            .iter()
            .map(|&(i, _)| i as ElemIdx)
            .filter(|&i| matches!(d.elements[i].kind, ElemKind::Other(_)))
            .collect();
        // the pins of one element are next to each other
        out.dedup();
        out
    }

    /// The pins on net `net` (any letter case) in the instance `scope_path`
    /// (`""`: the root; in a bare netlist, the top level), with their roles:
    /// device pins by type, and the port names of the definition that each
    /// `X` line binds to on this path. Ground and the `.global` names are
    /// one net at every level: the list holds the pins in this scope only.
    /// An element of a type that the parser does not read makes the answer
    /// `connectivity_incomplete`.
    ///
    /// In a definition with an included DSPF, [`NetView::Logical`] answers
    /// for the logical net of the node (its `*|NET` block): the pins on all
    /// its nodes, without the parasitic R and C. [`NetView::Physical`] lists
    /// the pins on the nodes as written, with the parasitic R and C: all
    /// the nodes of a DSPF net, or one node. `parasitics` comes from the
    /// `*|NET` block of the net (included, or `.option sim_dspf` for this
    /// scope).
    pub fn net_info(
        &self,
        scope_path: &str,
        net: &str,
        view: NetView,
        page_req: &Page,
    ) -> Result<NetInfo, QueryError> {
        let asked = net.trim();
        if asked.is_empty() {
            return Err(QueryError::BadArgument("the net name is empty".to_owned()));
        }
        let (links, mut caveats) = self.chain(scope_path)?;
        let here = &links.last().expect("the chain has the root").node;
        let def = here.scope.def;
        let d = &self.defs[def.idx()];
        let asked_key = self.asked_key(asked);
        let physical = view == NetView::Physical;
        let (key, block) = if physical {
            let block = self
                .included_dspf(def)
                .and_then(|x| Some((x, *x.by_name.get(&asked_key)?)));
            (asked_key, block)
        } else {
            self.logical(def, asked_key)
        };
        let pins = match block {
            Some((x, i)) => self.pins_on(def, &x.nets[i as usize].nodes, physical),
            None => self.pins_on(def, &[key], physical),
        };
        let is_ground = key == GROUND_KEY;
        let is_global = self.is_global_key(key);
        let is_port = d.ports.iter().any(|&p| self.net_key(p) == key);
        let index = self.net_index(def);
        // a node that only parasitic R and C touch (logical view)
        let only_parasitic = pins.is_empty() && !index.pins_of(asked_key).is_empty();
        if pins.is_empty()
            && !only_parasitic
            && !is_port
            && !is_global
            && !is_ground
            && block.is_none()
        {
            let keys = index.keys();
            let names = keys
                .iter()
                .filter(|&&k| k != GROUND_KEY)
                .map(|&k| Sym(k))
                .chain(d.ports.iter().copied())
                .map(|s| {
                    let n = self.syms.name(s);
                    (n, n.to_owned())
                })
                .filter(|(n, _)| !n.eq_ignore_ascii_case(asked));
            return Err(QueryError::NotFound {
                what: format!("net in `{}`", self.def_display(def)),
                name: asked.to_owned(),
                suggestions: suggest(asked, names),
            });
        }
        let shown = if let Some((x, i)) = block {
            x.nets[i as usize].name.clone()
        } else if is_ground {
            asked.to_owned()
        } else {
            self.syms
                .get(asked)
                .map_or(asked, |s| self.syms.name(s))
                .to_owned()
        };
        if is_ground {
            caveats.push("ground (`0`, `gnd`, `gnd!`, `ground`) is one net at every level (golden `ground_alias`): these are its pins in this scope only".to_owned());
        } else if is_global {
            caveats.push(format!("`{shown}` is a `.global` net: the same net at every level (golden `global_net`); these are its pins in this scope only"));
        }
        if only_parasitic {
            caveats.push(format!("only parasitic R and C of a DSPF touch `{shown}`: the logical view lists no pin; the physical view lists them"));
        }
        if is_port && (is_global || is_ground) {
            caveats.push(format!("`{shown}` is a port of `{}` and a name that is one net at every level: the answer reads it as that net, not as the net that each `X` line connects to the port (this conflict was not probed in HSPICE)", self.def_display(def)));
        }
        if let Some((x, _)) = block
            && !physical
            && key != asked_key
        {
            caveats.push(format!(
                "`{asked}` is a node of the DSPF net `{shown}` (its `*|NET` block in `{}`): the answer is for that net; the physical view lists the pins of the node itself",
                x.file
            ));
        }
        let unparsed = self.unparsed_in(def, &pins);
        let mut incomplete_due_to: Vec<String> = unparsed
            .iter()
            .take(MAX_LIMIT)
            .map(|&i| self.syms.name(d.elements[i].name).to_owned())
            .collect();
        let skipped: Vec<&str> = self.skipped_in(def).collect();
        if !skipped.is_empty() {
            caveats.push(format!(
                "{} element line{} of this definition {} skipped by a load error (`diagnostics`): {} pins are not known, so {} can touch this net",
                skipped.len(),
                if skipped.len() == 1 { "" } else { "s" },
                if skipped.len() == 1 { "was" } else { "were" },
                if skipped.len() == 1 { "its" } else { "their" },
                if skipped.len() == 1 { "it" } else { "they" },
            ));
            let room = MAX_LIMIT.saturating_sub(incomplete_due_to.len());
            incomplete_due_to.extend(skipped.iter().take(room).map(|s| (*s).to_owned()));
        }
        if unparsed.len() > MAX_LIMIT {
            caveats.push(format!(
                "{} elements of types that the parser does not read touch this net; `incomplete_due_to` lists {MAX_LIMIT}",
                unparsed.len()
            ));
        }
        let mut notes = (Capped::new("`X` line"), Capped::new("`X` line"));
        let connections = page(pins.iter(), page_req)?.map(|&(i, k)| {
            let e = &d.elements[i as usize];
            Conn {
                element: self.syms.name(e.name).to_owned(),
                kind: e.kind.name().to_owned(),
                pin_role: self.pin_role_in(d, &here.scope, i as usize, k as usize, &mut notes),
                callee: e.callee.map(|c| self.syms.name(c).to_owned()),
                node: physical.then(|| self.syms.name(d.element_pins(e)[k as usize]).to_owned()),
            }
        });
        caveats.extend(notes.0.done());
        caveats.extend(notes.1.done());
        Ok(NetInfo {
            scope: here.path.clone(),
            def: self.def_display(def),
            net: shown,
            is_port,
            is_global,
            is_ground,
            connectivity_incomplete: !unparsed.is_empty() || !skipped.is_empty(),
            incomplete_due_to,
            total: pins.len(),
            connections,
            parasitics: self.net_parasitics(&here.path, def, key),
            caveats,
        })
    }

    /// Where net `net` of the instance `scope_path` goes through ports, up
    /// to `depth` steps: `Down` into the instances whose `X` lines have a
    /// pin on it (to the port of that pin), `Up` from a port of the scope to
    /// the net of the parent on that pin, `Both` from every net reached.
    /// Breadth-first, nearest first; each (instance, net) once. Ground and
    /// the `.global` names connect at every level by name, so the trace
    /// does not follow them. At most [`MAX_HOPS`] steps: more gives
    /// `truncated`.
    pub fn trace_net(
        &self,
        scope_path: &str,
        net: &str,
        dir: TraceDir,
        depth: u32,
        page_req: &Page,
    ) -> Result<NetTrace, QueryError> {
        let asked = net.trim();
        // the start net must exist in the scope
        let info = self.net_info(
            scope_path,
            asked,
            NetView::Logical,
            &Page {
                limit: 0,
                cursor: None,
            },
        )?;
        let (links, mut caveats) = self.chain(scope_path)?;
        let start = links.last().expect("the chain has the root").node.clone();
        let key = self.logical(start.scope.def, self.asked_key(asked)).0;
        let (down, up) = match dir {
            TraceDir::Down => (true, false),
            TraceDir::Up => (false, true),
            TraceDir::Both => (true, true),
        };
        let mut hops: Vec<Hop> = Vec::new();
        let mut truncated = false;
        let mut incomplete: Vec<String> = Vec::new();
        let mut incomplete_total = 0_usize;
        let mut record_incomplete = |node: &Node, key: u32, out: &mut Vec<String>| -> bool {
            let d = &self.defs[node.scope.def.idx()];
            let mut any = false;
            let pins = self.logical_pins(node.scope.def, key);
            let unparsed = self.unparsed_in(node.scope.def, &pins);
            let names = unparsed
                .iter()
                .map(|&i| self.syms.name(d.elements[i].name).to_ascii_lowercase())
                .chain(self.skipped_in(node.scope.def).map(str::to_owned));
            for name in names {
                any = true;
                incomplete_total += 1;
                if out.len() < MAX_LIMIT {
                    out.push(if node.path.is_empty() {
                        name
                    } else {
                        format!("{}.{name}", node.path)
                    });
                }
            }
            any
        };
        let mut start_incomplete = record_incomplete(&start, key, &mut incomplete);
        if key == GROUND_KEY || self.is_global_key(key) {
            caveats.push(format!(
                "`{}` is {}: it is one net at every level by its name, so the trace does not follow it through ports",
                info.net,
                if key == GROUND_KEY { "ground" } else { "a `.global` net" }
            ));
        } else {
            let mut seen: HashSet<(String, DefId, u32)> = HashSet::new();
            seen.insert((start.path.clone(), start.scope.def, key));
            let mut queue: VecDeque<(Node, u32, u32)> = VecDeque::new();
            queue.push_back((start, key, 0));
            let mut unbound = Capped::new("`X` line");
            let mut mismatch = Capped::new("`X` line");
            let mut stopped = Capped::new("net");
            'search: while let Some((node, key, at)) = queue.pop_front() {
                if at >= depth {
                    continue;
                }
                let from_net = self.syms.name(Sym(key)).to_owned();
                let d = &self.defs[node.scope.def.idx()];
                // the next steps: (hop, the node it reaches, its net key)
                let mut next: Vec<(Hop, Node, u32)> = Vec::new();
                if down {
                    for &(i, k) in self.logical_pins(node.scope.def, key).iter() {
                        let e = &d.elements[i as usize];
                        if e.kind != ElemKind::Instance {
                            continue;
                        }
                        let x = self.syms.name(e.name).to_ascii_lowercase();
                        let child_path = if node.path.is_empty() {
                            x.clone()
                        } else {
                            format!("{}.{x}", node.path)
                        };
                        let Some(t) = e.callee.and_then(|c| node.scope.bind(self, c)) else {
                            unbound.add(|| format!("`{child_path}` calls subckt `{}`, which has no visible definition on this path (diagnostic `unresolved_subckt`): the trace stops there", e.callee.map_or("", |c| self.syms.name(c))));
                            continue;
                        };
                        let ports = &self.defs[t.idx()].ports;
                        let nodes = d.element_pins(e).len();
                        if ports.len() != nodes {
                            mismatch.add(|| format!("`{child_path}` has {nodes} nodes, but subckt `{}` has {} ports: HSPICE rejects the deck (diagnostic `port_count_mismatch`); the trace stops there", self.def_display(t), ports.len()));
                            continue;
                        }
                        let port = ports[k as usize];
                        let to_key = self.logical(t, self.net_key(port)).0;
                        let child = Node {
                            scope: node.scope.child(self, t, child_path.len()),
                            path: child_path.clone(),
                            up: None,
                        };
                        let hop = Hop {
                            from_scope: node.path.clone(),
                            from_net: from_net.clone(),
                            via_instance: child_path,
                            to_scope: child.path.clone(),
                            to_net: self.logical_name(to_key, port).to_owned(),
                            direction: "down".to_owned(),
                            depth: at + 1,
                            connectivity_incomplete: false,
                        };
                        next.push((hop, child, to_key));
                    }
                }
                if up
                    && let Some(j) = node.up
                    && j > 0
                    && let Some((holder, xi)) = links[j].xline
                {
                    let parent = &links[j - 1].node;
                    let hd = &self.defs[holder.idx()];
                    let x = &hd.elements[xi];
                    let pins = hd.element_pins(x);
                    let on_port = d
                        .ports
                        .iter()
                        .any(|&port| self.logical(node.scope.def, self.net_key(port)).0 == key);
                    // as down: an `X` line whose node count is not the port
                    // count binds no port
                    let bound = if pins.len() == d.ports.len() {
                        d.ports.as_slice()
                    } else {
                        &[]
                    };
                    if on_port && bound.is_empty() {
                        mismatch.add(|| format!("`{}` has {} nodes, but subckt `{}` has {} ports: HSPICE rejects the deck (diagnostic `port_count_mismatch`); the trace stops there", node.path, pins.len(), self.def_display(node.scope.def), d.ports.len()));
                    }
                    for (p, &port) in bound.iter().enumerate() {
                        if self.logical(node.scope.def, self.net_key(port)).0 != key {
                            continue;
                        }
                        let m = pins[p];
                        let to_key = self.logical(parent.scope.def, self.net_key(m)).0;
                        let hop = Hop {
                            from_scope: node.path.clone(),
                            from_net: from_net.clone(),
                            via_instance: node.path.clone(),
                            to_scope: parent.path.clone(),
                            to_net: self.logical_name(to_key, m).to_owned(),
                            direction: "up".to_owned(),
                            depth: at + 1,
                            connectivity_incomplete: false,
                        };
                        next.push((hop, parent.clone(), to_key));
                    }
                }
                for (mut hop, to, to_key) in next {
                    if !seen.insert((to.path.clone(), to.scope.def, to_key)) {
                        continue;
                    }
                    if hops.len() >= MAX_HOPS {
                        truncated = true;
                        break 'search;
                    }
                    hop.connectivity_incomplete = record_incomplete(&to, to_key, &mut incomplete);
                    if to_key == GROUND_KEY || self.is_global_key(to_key) {
                        stopped.add(|| format!("`{}` in `{}` is {}: one net at every level by its name, so the trace does not follow it further", hop.to_net, hop.to_scope, if to_key == GROUND_KEY { "ground" } else { "a `.global` net" }));
                    } else {
                        queue.push_back((to, to_key, at + 1));
                    }
                    hops.push(hop);
                }
            }
            caveats.extend(unbound.done());
            caveats.extend(mismatch.done());
            caveats.extend(stopped.done());
            if truncated {
                caveats.push(format!(
                    "the trace stopped at {MAX_HOPS} steps before depth {depth}: trace from a deeper scope or with a smaller depth"
                ));
            }
        }
        if incomplete_total > incomplete.len() {
            caveats.push(format!(
                "{incomplete_total} elements of types that the parser does not read, or element lines that a load error skipped, can touch the nets of the trace; `incomplete_due_to` lists {}",
                incomplete.len()
            ));
        }
        start_incomplete |= incomplete_total > 0;
        Ok(NetTrace {
            scope: info.scope,
            net: info.net,
            hops: page(hops.into_iter(), page_req)?,
            truncated,
            connectivity_incomplete: start_incomplete,
            incomplete_due_to: incomplete,
            caveats,
        })
    }
}
