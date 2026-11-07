//! DSPF parasitic files (spec 6.1a).
//!
//! A DSPF is SPICE text with structured `*|` comment lines. It arrives in
//! two ways:
//!
//! 1. Included (golden `dspf_included`): the DSPF is the deck, or a file of
//!    it. The parser reads it as netlist text (HSPICE treats `*|` lines as
//!    comments). A file with a `*|DSPF` line in its first 50 lines is also
//!    scanned here: the R and C lines of a `*|NET` block are parasitics,
//!    not devices ([`attach_included`]), and every node of the block maps
//!    to the logical net of the block.
//! 2. `.option sim_dspf="[scope] file"` (golden `dspf_backannot`): the
//!    pre-layout netlist gives the nets, and only the `*|NET` blocks of the
//!    file are read, to give parasitics to the nets of the scope
//!    ([`attach_sim_dspf`]). The scope is a subckt name (every instance of
//!    it) or an instance name. No scope: the top level.
//!
//! The scanner reads lines, not statements: a `*|` record or an R/C value
//! on a `+` or `*+` line is not read (the sum of the values is then not
//! given). A block ends at the next `*|NET`, at `.ENDS`, at a `*` line
//! with `Instance Section`, or at an element line that is not R or C. In a
//! block, a resistor is a parasitic when a chain of the block's resistors
//! joins it to the net, or to the first name of a `*|P`, `*|I` or `*|S`
//! record, and it touches no other net (another block, a `.SUBCKT` port,
//! ground). A capacitor is one when one of its nodes is a node of the block
//! or its first node is `<net>:…` (`*|DELIMITER`). The other R and C lines
//! are devices (Warning `dspf_device_in_block`). The nodes of a block are
//! the net, the record names, both nodes of each parasitic resistor and
//! the block node of each parasitic capacitor; a block named for ground
//! (`gnd`) holds ground.

use crate::{
    DefLookup, Netlist,
    api::{DefParasitics, DspfFile, Level, Parasitics},
    hier::{FxMap, FxSet},
    model::{DefId, Diag, ElemKind, LocIdx, ROOT, Root},
    nets::{GROUND as GROUND_NAMES, GROUND_KEY},
    paths::{Want, walk},
};
use std::{borrow::Cow, path::Path};

/// A file is a DSPF when one of its first lines starts with `*|DSPF`.
const DETECT_LINES: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Included,
    SimDspf,
}

/// The scopes whose nets get the parasitics of one DSPF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// Every instance of the definition.
    Def(DefId),
    /// One instance path (lowercase; `""`: the root), and its definition.
    Instance(String, DefId),
}

impl Target {
    pub(crate) fn def(&self) -> DefId {
        match self {
            Target::Def(d) | Target::Instance(_, d) => *d,
        }
    }

    /// The scope `path` with definition `def` is one of the target.
    pub(crate) fn matches(&self, path: &str, def: DefId) -> bool {
        match self {
            Target::Def(d) => *d == def,
            Target::Instance(p, _) => p == path,
        }
    }
}

/// One `*|NET` block.
#[derive(Debug, Default)]
pub(crate) struct DspfNet {
    /// The net name, as the DSPF writes it.
    pub(crate) name: String,
    /// Its net key (see [`crate::nets`]), when the deck has the name.
    pub(crate) key: Option<u32>,
    pub(crate) total_cap: Option<f64>,
    pub(crate) r_count: u64,
    /// The sum of the values that were read as numbers; `r_unread` values
    /// were not (a parameter name, a value on a `+` line).
    pub(crate) r_sum: f64,
    pub(crate) r_unread: u64,
    pub(crate) c_count: u64,
    pub(crate) c_sum: f64,
    pub(crate) c_unread: u64,
    pub(crate) pins: u32,
    /// The net keys of its nodes that the deck has, sorted, each once.
    pub(crate) nodes: Vec<u32>,
}

/// One DSPF file, for one definition.
#[derive(Debug)]
pub(crate) struct Dspf {
    pub(crate) file: String,
    pub(crate) mode: Mode,
    pub(crate) scope: Option<String>,
    pub(crate) target: Target,
    pub(crate) nets: Vec<DspfNet>,
    /// The net key of each block name: its index in `nets`.
    pub(crate) by_name: FxMap<u32, u32>,
    /// [`Mode::Included`]: the net key of each node of a block: its index
    /// in `nets` (the first block that names it).
    pub(crate) by_node: FxMap<u32, u32>,
    pub(crate) unmatched: usize,
    /// [`Mode::SimDspf`]: the deck sets `.option sim_dspf_rail=on`.
    pub(crate) rail: bool,
}

impl Dspf {
    fn new(
        file: String,
        mode: Mode,
        scope: Option<String>,
        target: Target,
        nets: Vec<DspfNet>,
    ) -> Self {
        let mut by_name = FxMap::default();
        let mut by_node = FxMap::default();
        for (i, n) in nets.iter().enumerate() {
            if let Some(k) = n.key {
                by_name.entry(k).or_insert(i as u32);
            }
            if mode == Mode::Included {
                for &k in &n.nodes {
                    by_node.entry(k).or_insert(i as u32);
                }
            }
        }
        Dspf {
            file,
            mode,
            scope,
            target,
            nets,
            by_name,
            by_node,
            unmatched: 0,
            rail: false,
        }
    }

    pub(crate) fn parasitics(&self, i: u32) -> Parasitics {
        let n = &self.nets[i as usize];
        let mut caveats = Vec::new();
        let unread = n.r_unread + n.c_unread;
        if unread > 0 {
            caveats.push(format!(
                "{unread} parasitic value{} of `{}` {} not read as a number (a parameter name, or a value on a `+` line): the sum of {} is not given",
                if unread == 1 { "" } else { "s" },
                n.name,
                if unread == 1 { "was" } else { "were" },
                match (n.r_unread > 0, n.c_unread > 0) {
                    (true, true) => "the resistors and of the capacitors",
                    (true, false) => "the resistors",
                    _ => "the capacitors",
                }
            ));
        }
        if self.mode == Mode::SimDspf && !self.rail {
            caveats.push("the numbers are those of the file: in a DC-only deck HSPICE skips each net as a power rail net, and expands no parasitics, unless `.option sim_dspf_rail=on` is set (golden `dspf_backannot`); other analyses were not probed".to_owned());
        }
        Parasitics {
            file: self.file.clone(),
            total_cap: n.total_cap,
            r_count: n.r_count,
            r_sum: (n.r_unread == 0).then_some(n.r_sum),
            c_count: n.c_count,
            c_sum: (n.c_unread == 0).then_some(n.c_sum),
            pins: n.pins,
            caveats,
        }
    }
}

/// The net key of a node name that the deck has.
type KeyOf<'a> = &'a dyn Fn(&str) -> Option<u32>;

/// What [`scan`] reads from one file.
#[derive(Debug, Default)]
struct Scan {
    /// The name on the first `.SUBCKT` line.
    subckt: Option<String>,
    nets: Vec<DspfNet>,
    /// The lines (from 1) of the parasitic R and C, in line order, with
    /// their block and the pin that is a node of the block (0, 1 or
    /// [`BOTH_PINS`]).
    lines: Vec<(u32, u32, u8)>,
    /// The R and C lines in a block that are not its parasitics (devices),
    /// and the first of them.
    stray: usize,
    stray_line: Option<u32>,
}

/// The definition that `name` names (the rule of `Netlist::find_def`).
fn found_def(net: &Netlist, name: &str) -> Option<DefId> {
    match net.find_def(name) {
        DefLookup::Found(id) => Some(id),
        _ => None,
    }
}

/// `text` has a `*|DSPF` line in its first [`DETECT_LINES`] lines.
fn is_dspf(text: &str) -> bool {
    text.lines().take(DETECT_LINES).any(|l| {
        l.trim_start()
            .get(..6)
            .is_some_and(|h| h.eq_ignore_ascii_case("*|dspf"))
    })
}

/// The text before an inline `$` comment (a `$` at the start or after a
/// space).
fn before_comment(l: &str) -> &str {
    let b = l.as_bytes();
    match (0..b.len()).find(|&i| b[i] == b'$' && (i == 0 || b[i - 1].is_ascii_whitespace())) {
        Some(i) => &l[..i],
        None => l,
    }
}

/// The value of an R or C line (`R1 a b 10`, `R1 a b r=10`, `C1 a b '1f'`).
fn element_value(tokens: &[&str], letter: char) -> Option<f64> {
    let mut bare = None;
    for t in tokens.iter().skip(3) {
        match t.split_once('=') {
            Some((k, v))
                if k.len() == 1 && k.starts_with(|c: char| c.eq_ignore_ascii_case(&letter)) =>
            {
                return number(v);
            }
            Some(_) => {}
            None if bare.is_none() => bare = Some(*t),
            None => {}
        }
    }
    bare.and_then(number)
}

fn number(s: &str) -> Option<f64> {
    let s = s.trim_matches(|c| c == '\'' || c == '"');
    netlist_expr::scan_number(s).map(|(v, _)| v)
}

/// An R or C line of a block that can be a parasitic: its line, its block,
/// its letter (`r`, `c`), its two nodes (lowercase), its value.
struct Cand<'t> {
    line: u32,
    block: usize,
    letter: char,
    /// Lowercase.
    nodes: [Cow<'t, str>; 2],
    value: Option<f64>,
}

/// `s` in lowercase, borrowed when it is.
fn lower(s: &str) -> Cow<'_, str> {
    if s.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(s.to_ascii_lowercase())
    } else {
        Cow::Borrowed(s)
    }
}

/// Reads the `*|NET` blocks of `text`. With `key_of` (the net key of a
/// node name that the deck has), it also gives the nodes of each block
/// that the deck has. The caller adds the block keys.
fn scan(text: &str, key_of: Option<KeyOf<'_>>) -> Scan {
    let mut out = Scan::default();
    let mut cur: Option<usize> = None;
    let mut delim = ':';
    let mut ports: Vec<String> = Vec::new();
    // the node names of each block from its records (lowercase)
    let mut named: Vec<Vec<String>> = Vec::new();
    let mut cands: Vec<Cand> = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = n as u32 + 1;
        let l = raw.trim_start();
        let Some(first) = l.chars().next() else {
            continue;
        };
        if let Some(rec) = l.strip_prefix("*|") {
            let end = rec
                .find(|c: char| c.is_whitespace() || c == '(')
                .unwrap_or(rec.len());
            let (kw, rest) = rec.split_at(end);
            if kw.eq_ignore_ascii_case("net") {
                let mut t = rest.split_whitespace();
                let Some(name) = t.next() else {
                    continue;
                };
                out.nets.push(DspfNet {
                    name: name.to_owned(),
                    total_cap: t.next().and_then(number),
                    ..DspfNet::default()
                });
                named.push(vec![name.to_ascii_lowercase()]);
                cur = Some(out.nets.len() - 1);
            } else if kw.eq_ignore_ascii_case("delimiter") {
                delim = rest.trim().chars().next().unwrap_or(':');
            } else if let Some(b) = cur
                && ["p", "i", "s"].iter().any(|k| kw.eq_ignore_ascii_case(k))
            {
                let net = &mut out.nets[b];
                if !kw.eq_ignore_ascii_case("s") {
                    net.pins += 1;
                }
                let node = rest
                    .trim_start()
                    .trim_start_matches('(')
                    .split(|c: char| c.is_whitespace() || c == ')')
                    .find(|s| !s.is_empty());
                if let Some(node) = node {
                    named[b].push(node.to_ascii_lowercase());
                    net.nodes.extend(key_of.and_then(|k| k(node)));
                }
            }
            continue;
        }
        if first == '*' {
            // StarRC and Calibre write `* Instance Section` before the
            // device lines, which can start with R or C
            if has(l, "instance section") {
                cur = None;
            }
            continue;
        }
        if first == '+' || first == '$' {
            continue;
        }
        if first == '.' {
            let mut tokens = l.split_whitespace();
            let word = tokens.next().unwrap_or("");
            if word.eq_ignore_ascii_case(".subckt") && out.subckt.is_none() {
                out.subckt = tokens.next().map(str::to_owned);
                ports = tokens
                    .take_while(|t| !t.contains('='))
                    .map(str::to_ascii_lowercase)
                    .collect();
            } else if word.eq_ignore_ascii_case(".ends") || word.eq_ignore_ascii_case(".eom") {
                cur = None;
            }
            continue;
        }
        let letter = first.to_ascii_lowercase();
        let Some(b) = cur.filter(|_| letter == 'r' || letter == 'c') else {
            // the instance section
            cur = None;
            continue;
        };
        let tokens: Vec<&str> = before_comment(l).split_whitespace().collect();
        if let [_, a, z, ..] = tokens.as_slice() {
            cands.push(Cand {
                line,
                block: b,
                letter,
                nodes: [lower(a), lower(z)],
                value: element_value(&tokens, letter),
            });
        }
    }
    classify(&mut out, &named, &ports, delim, cands);
    out
}

/// `text` has `word` in it, in any letter case.
fn has(text: &str, word: &str) -> bool {
    text.as_bytes()
        .windows(word.len())
        .any(|w| w.eq_ignore_ascii_case(word.as_bytes()))
}

/// [`Scan::lines`]: both nodes of the line are nodes of its block.
const BOTH_PINS: u8 = 2;

/// Picks the parasitics among the R and C lines of the blocks, in any
/// order of the lines. A resistor is a parasitic of its block when a chain
/// of the block's resistors joins it to a node of the block (the net, a
/// `*|P`, `*|I` or `*|S` node), and it touches no other net: another
/// block's net, a port of the `.SUBCKT` or ground. A capacitor is one when
/// a node is a node of the block (or its first node is `<net><delimiter>…`).
/// The other lines are devices (`stray`): an instance section with no
/// `* Instance Section` line before it.
fn classify(
    out: &mut Scan,
    named: &[Vec<String>],
    ports: &[String],
    delim: char,
    cands: Vec<Cand<'_>>,
) {
    fn root(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    fn slot<'a>(id: &mut FxMap<&'a str, usize>, parent: &mut Vec<usize>, n: &'a str) -> usize {
        *id.entry(n).or_insert_with(|| {
            parent.push(parent.len());
            parent.len() - 1
        })
    }
    // the names of nets: the blocks, the ports, ground
    let mut nets: FxSet<&str> = named.iter().map(|n| n[0].as_str()).collect();
    nets.extend(ports.iter().map(String::as_str));
    nets.extend(GROUND_NAMES);
    // the containers of one block, used again for the next one: a
    // union-find over the record names and the nodes of the resistors that
    // touch no other net; its sets that hold a record; the first node of
    // each resistor in it (`None`: the resistor touches another net)
    let mut id: FxMap<&str, usize> = FxMap::default();
    let mut parent: Vec<usize> = Vec::new();
    let mut inside: Vec<bool> = Vec::new();
    let mut r_ix: Vec<Option<usize>> = Vec::new();
    // the scan gives the lines in block order
    for list in cands.chunk_by(|a, z| a.block == z.block) {
        let b = list[0].block;
        let own = named[b][0].as_str();
        let other = |n: &str| n != own && nets.contains(n);
        id.clear();
        parent.clear();
        r_ix.clear();
        for n in &named[b] {
            slot(&mut id, &mut parent, n);
        }
        let records = parent.len();
        for c in list.iter().filter(|c| c.letter == 'r') {
            let [n0, n1] = [c.nodes[0].as_ref(), c.nodes[1].as_ref()];
            if other(n0) || other(n1) {
                r_ix.push(None);
                continue;
            }
            let (a, z) = (
                slot(&mut id, &mut parent, n0),
                slot(&mut id, &mut parent, n1),
            );
            let (ra, rz) = (root(&mut parent, a), root(&mut parent, z));
            parent[ra] = rz;
            r_ix.push(Some(a));
        }
        inside.clear();
        inside.resize(parent.len(), false);
        for i in 0..records {
            let r = root(&mut parent, i);
            inside[r] = true;
        }
        let mut rs = r_ix.iter();
        for c in list {
            let pin = if c.letter == 'r' {
                match rs.next() {
                    Some(&Some(a)) if inside[root(&mut parent, a)] => Some(BOTH_PINS),
                    _ => None,
                }
            } else {
                let mut in_block =
                    |n: &str| id.get(n).is_some_and(|&i| inside[root(&mut parent, i)]);
                let n0 = c.nodes[0].as_ref();
                if in_block(n0) || n0.strip_prefix(own).is_some_and(|r| r.starts_with(delim)) {
                    Some(0)
                } else if in_block(&c.nodes[1]) {
                    Some(1)
                } else {
                    None
                }
            };
            let Some(pin) = pin else {
                out.stray += 1;
                out.stray_line.get_or_insert(c.line);
                continue;
            };
            out.lines.push((c.line, b as u32, pin));
            let net = &mut out.nets[b];
            let (count, sum, unread) = if c.letter == 'r' {
                (&mut net.r_count, &mut net.r_sum, &mut net.r_unread)
            } else {
                (&mut net.c_count, &mut net.c_sum, &mut net.c_unread)
            };
            *count += 1;
            match c.value {
                Some(v) => *sum += v,
                None => *unread += 1,
            }
        }
    }
    out.lines.sort_unstable();
}

/// Scans the included DSPF files and marks their parasitic R and C. Runs
/// before the hierarchy analysis, so that the device counts leave them out,
/// and before any query, because it adds the block names that the deck
/// does not have to the names.
pub(crate) fn attach_included(net: &mut Netlist) -> Vec<Diag> {
    let mut diags = Vec::new();
    let mut scans: Vec<Option<Scan>> = {
        let key_of = |name: &str| net.syms.get(name).map(|s| net.net_key(s));
        net.files
            .iter()
            .map(|f| {
                (f.section.is_none() && is_dspf(&f.text)).then(|| scan(&f.text, Some(&key_of)))
            })
            .collect()
    };
    if scans.iter().all(Option::is_none) {
        return diags;
    }
    // the definition of each block (the one that holds its first
    // parasitic), and the nodes of its resistors as the parser read them
    let mut block_def: Vec<Vec<Option<DefId>>> = scans
        .iter()
        .map(|s| {
            s.as_ref()
                .map_or_else(Vec::new, |s| vec![None; s.nets.len()])
        })
        .collect();
    let mut marked: Vec<(u32, u32)> = Vec::new();
    for (di, d) in net.defs.iter().enumerate() {
        for (ei, e) in d.elements.iter().enumerate() {
            if e.alter.is_some() || !matches!(e.kind, ElemKind::Resistor | ElemKind::Capacitor) {
                continue;
            }
            let f = e.loc.file as usize;
            let Some(Some(s)) = scans.get_mut(f) else {
                continue;
            };
            let Ok(j) = s.lines.binary_search_by_key(&e.loc.line, |x| x.0) else {
                continue;
            };
            marked.push((di as u32, ei as u32));
            let (_, b, pin) = s.lines[j];
            let b = b as usize;
            block_def[f][b].get_or_insert(DefId(di as u32));
            // the nodes of the block, as the parser read them
            s.nets[b].nodes.extend(
                d.element_pins(e)
                    .iter()
                    .enumerate()
                    .filter(|&(k, _)| pin == BOTH_PINS || k == pin as usize)
                    .map(|(_, &p)| net.net_key(p)),
            );
        }
    }
    for (di, ei) in marked {
        net.defs[di as usize].elements[ei as usize].parasitic = true;
    }
    for (f, s) in scans.into_iter().enumerate() {
        let Some(s) = s else {
            continue;
        };
        if let Some(line) = s.stray_line {
            diags.push(Diag {
                level: Level::Warning,
                code: "dspf_device_in_block",
                message: format!(
                    "{} R or C line{} after a `*|NET` block of this DSPF {} not connect to the nodes of the block: {} read as {} of the netlist, not as parasitics (an instance section with no `* Instance Section` line before it)",
                    s.stray,
                    if s.stray == 1 { "" } else { "s" },
                    if s.stray == 1 { "does" } else { "do" },
                    if s.stray == 1 { "it is" } else { "they are" },
                    if s.stray == 1 { "a device" } else { "devices" },
                ),
                loc: Some(LocIdx {
                    file: f as u32,
                    line,
                }),
            });
        }
        // a block with no parasitic: the `.SUBCKT` of the file, else the top
        let fallback = s
            .subckt
            .as_deref()
            .and_then(|n| found_def(net, n))
            .unwrap_or(ROOT);
        let mut groups: Vec<(DefId, Vec<DspfNet>)> = Vec::new();
        for (b, mut n) in s.nets.into_iter().enumerate() {
            // an internal net is often named only on its `*|NET` line
            let sym = net.syms.intern(&n.name);
            let k = net.net_key(sym);
            n.key = Some(k);
            n.nodes.push(k);
            // ground maps to a block named for it only
            n.nodes.retain(|&x| x != GROUND_KEY || k == GROUND_KEY);
            n.nodes.sort_unstable();
            n.nodes.dedup();
            let def = block_def[f][b].unwrap_or(fallback);
            match groups.iter_mut().find(|g| g.0 == def) {
                Some(g) => g.1.push(n),
                None => groups.push((def, vec![n])),
            }
        }
        let file = net.files[f].path.clone();
        for (def, nets) in groups {
            net.dspf.push(Dspf::new(
                file.clone(),
                Mode::Included,
                None,
                Target::Def(def),
                nets,
            ));
        }
    }
    diags
}

/// Reads the files of `.option sim_dspf` (first run). Runs after the
/// hierarchy analysis: an instance scope is a walked path.
pub(crate) fn attach_sim_dspf(net: &mut Netlist) -> Vec<Diag> {
    let mut diags = Vec::new();
    // `.option sim_dspf_rail=on` (the last one of the first run)
    let rail = net
        .options
        .iter()
        .rfind(|p| p.alter.is_none() && net.syms.name(p.name).eq_ignore_ascii_case("sim_dspf_rail"))
        .is_some_and(|p| {
            net.param_text(p).is_some_and(|t| {
                let t = t.trim().trim_matches(|c| c == '\'' || c == '"');
                ["on", "1", "yes", "true"]
                    .iter()
                    .any(|v| v.eq_ignore_ascii_case(t))
            }) || p.num.is_some_and(|x| x != 0.0)
        });
    let options: Vec<(String, LocIdx)> = net
        .options
        .iter()
        .filter(|p| p.alter.is_none() && net.syms.name(p.name).eq_ignore_ascii_case("sim_dspf"))
        .filter_map(|p| Some((net.param_text(p)?.into_owned(), p.loc)))
        .collect();
    for (text, loc) in options {
        let words: Vec<&str> = text.split_whitespace().collect();
        let (scope, file) = match words.as_slice() {
            [] => continue,
            [file] => (None, *file),
            [scope, file, ..] => (Some(*scope), *file),
        };
        let path = Path::new(&net.files[loc.file as usize].path)
            .parent()
            .unwrap_or(Path::new(""))
            .join(file);
        let Ok(bytes) = std::fs::read(&path) else {
            diags.push(Diag {
                level: Level::Warning,
                code: "dspf_file_missing",
                message: format!(
                    "`.option sim_dspf` names `{file}`, which cannot be read (looked for `{}`): its parasitics are not given",
                    path.display()
                ),
                loc: Some(loc),
            });
            continue;
        };
        let target = match scope {
            None => Some(Target::Instance(
                String::new(),
                match net.root {
                    Root::Subckt(id) => id,
                    _ => ROOT,
                },
            )),
            Some(s) => match found_def(net, s) {
                Some(d) => Some(Target::Def(d)),
                None => {
                    let p = s.to_ascii_lowercase().replace('/', ".");
                    walk(net, &p, Want::Instance)
                        .ok()
                        .and_then(|w| Some(Target::Instance(w.path.clone(), w.end_scope?.def)))
                }
            },
        };
        let Some(target) = target else {
            diags.push(Diag {
                level: Level::Warning,
                code: "dspf_scope_unknown",
                message: format!(
                    "`.option sim_dspf` scope `{}` names no subckt and no instance (HSPICE: \"Wrong instance or subckt name\"): the parasitics of `{file}` are not given",
                    scope.unwrap_or("")
                ),
                loc: Some(loc),
            });
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut s = scan(&text, None);
        for n in &mut s.nets {
            n.key = net.syms.get(&n.name).map(|s| net.net_key(s));
        }
        let def = target.def();
        let d = &net.defs[def.idx()];
        let index = net.net_index(def);
        let unmatched: Vec<&str> = s
            .nets
            .iter()
            .filter(|n| {
                !n.key.is_some_and(|k| {
                    !index.pins_of(k).is_empty() || d.ports.iter().any(|&p| net.net_key(p) == k)
                })
            })
            .map(|n| n.name.as_str())
            .collect();
        if !unmatched.is_empty() {
            let shown: Vec<String> = unmatched.iter().take(5).map(|n| format!("`{n}`")).collect();
            diags.push(Diag {
                level: Level::Warning,
                code: "dspf_net_unmatched",
                message: format!(
                    "{} of the {} `*|NET` blocks of `{file}` name no net of `{}` ({}{}): they give no parasitics (HSPICE: \"no match for net\")",
                    unmatched.len(),
                    s.nets.len(),
                    net.def_display(def),
                    shown.join(", "),
                    if unmatched.len() > 5 { ", …" } else { "" }
                ),
                loc: Some(loc),
            });
        }
        let unmatched = unmatched.len();
        let mut dspf = Dspf::new(
            file.to_owned(),
            Mode::SimDspf,
            scope.map(str::to_owned),
            target,
            s.nets,
        );
        dspf.unmatched = unmatched;
        dspf.rail = rail;
        net.dspf.push(dspf);
    }
    diags
}

impl Netlist {
    /// The included DSPF of definition `def` (the first one).
    pub(crate) fn included_dspf(&self, def: DefId) -> Option<&Dspf> {
        self.dspf
            .iter()
            .find(|d| d.mode == Mode::Included && d.target == Target::Def(def))
    }

    /// The parasitics of net `key` (a logical net key) in the scope `path`
    /// with definition `def`.
    pub(crate) fn net_parasitics(&self, path: &str, def: DefId, key: u32) -> Option<Parasitics> {
        self.dspf
            .iter()
            .filter(|d| d.target.matches(path, def))
            .find_map(|d| d.by_name.get(&key).map(|&i| d.parasitics(i)))
    }

    /// The DSPF files, for [`crate::Summary::dspf`].
    pub(crate) fn dspf_files(&self) -> Vec<DspfFile> {
        self.dspf
            .iter()
            .map(|d| {
                let def = d.target.def();
                DspfFile {
                    file: d.file.clone(),
                    mode: match d.mode {
                        Mode::Included => "included",
                        Mode::SimDspf => "sim_dspf",
                    }
                    .to_owned(),
                    scope: d.scope.clone(),
                    subckt: (def != ROOT).then(|| self.def_display(def)),
                    nets: d.nets.len(),
                    resistors: d.nets.iter().map(|n| n.r_count).sum(),
                    capacitors: d.nets.iter().map(|n| n.c_count).sum(),
                    unmatched_nets: d.unmatched,
                }
            })
            .collect()
    }

    /// The included DSPF parasitics of definition `def`.
    pub(crate) fn def_parasitics(&self, def: DefId) -> Option<DefParasitics> {
        let d = self.included_dspf(def)?;
        Some(DefParasitics {
            nets: d.nets.len(),
            resistors: d.nets.iter().map(|n| n.r_count).sum(),
            capacitors: d.nets.iter().map(|n| n.c_count).sum(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_blocks() {
        let text = "*|DSPF 1.0\n.SUBCKT inv A Y\n*|NET A 1.2e-15\n*|P (A I 0 0 0)\n*|I (M1:G M1 G I 1e-16 0 0)\n*|S (A:1 0 0)\nR1 A A:1 10 $ c\nC1 A:1 0 '1f'\nR2 A:1 M1:G r=5\n*|NET Y 1pf\nC2 Y 0 c=2f\nM1 Y M1:G 0 0 n\nR9 x y 1\n.ENDS\n";
        let names = ["a", "a:1", "m1:g", "y", "0"];
        let key_of = |n: &str| {
            names
                .iter()
                .position(|x| x.eq_ignore_ascii_case(n))
                .map(|i| if i == 4 { GROUND_KEY } else { i as u32 })
        };
        assert!(is_dspf(text));
        let s = scan(text, Some(&key_of));
        assert_eq!(s.subckt.as_deref(), Some("inv"));
        assert_eq!(s.nets.len(), 2);
        let a = &s.nets[0];
        assert_eq!((a.r_count, a.c_count, a.pins), (2, 1, 2));
        assert_eq!((a.r_sum, a.c_sum), (15.0, 1e-15));
        assert_eq!(a.total_cap, Some(1.2e-15));
        // the `*|P`, `*|I` and `*|S` nodes (the caller adds the net and
        // the nodes of the parasitics, from `lines`)
        assert_eq!(a.nodes, [0, 2, 1]);
        let y = &s.nets[1];
        assert_eq!((y.r_count, y.c_count, y.c_sum), (0, 1, 2e-15));
        assert_eq!(y.total_cap, Some(1e-12));
        // `R9` is after the instance section began: a device
        // R lines: both pins; `C1 A:1 0` and `C2 Y 0`: the first pin
        assert_eq!(
            s.lines,
            [(7, 0, BOTH_PINS), (8, 0, 0), (9, 0, BOTH_PINS), (11, 1, 0)]
        );
        assert_eq!(s.stray, 0);
        assert!(!is_dspf("* deck\nr1 a b 1\n"));
    }

    #[test]
    fn scan_picks_parasitics() {
        // resistors in any order; an instance section with no marker that
        // starts with R and C devices; a resistor to another net
        let text = "*|DSPF 1.0\n.SUBCKT amp IN OUT VDD VSS\n*|NET OUT 1e-15\n*|P (OUT O 0 0 0)\n*|I (M1:D M1 D B 0 0 0)\nR3 OUT:2 OUT:3 1\nR2 OUT:1 OUT:2 2\nR1 OUT OUT:1 4\nR4 OUT:3 M1:D 8\nC1 OUT:2 VSS 1f\nC2 OUT:7 VSS 2f\nRX OUT:3 VDD 5\nCMIM0 M9:A M9:B 1p\nRPOLY0 VDD M1:D 10k\nM1 M1:D IN VSS VSS n\n.ENDS\n";
        let s = scan(text, None);
        let o = &s.nets[0];
        assert_eq!((o.r_count, o.r_sum, o.c_count), (4, 15.0, 2));
        assert!((o.c_sum - 3e-15).abs() < 1e-27, "{}", o.c_sum);
        assert_eq!(
            s.lines.iter().map(|l| l.0).collect::<Vec<_>>(),
            [6, 7, 8, 9, 10, 11]
        );
        // `RX` and `RPOLY0` touch `VDD`; `CMIM0` touches no node of `OUT`
        assert_eq!((s.stray, s.stray_line), (3, Some(12)));
        // a marker line ends the block
        let marked = text.replace("CMIM0", "* Instance Section\nCMIM0");
        assert_eq!(scan(&marked, None).stray, 1);
        // a value that is not a number
        let s = scan(
            &text.replace("R2 OUT:1 OUT:2 2", "R2 OUT:1 OUT:2 rpar"),
            None,
        );
        assert_eq!((s.nets[0].r_count, s.nets[0].r_unread), (4, 1));
    }
}
