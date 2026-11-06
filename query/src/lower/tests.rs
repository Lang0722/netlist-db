use super::{QuoteFault, one_line, quote_fault};
use crate::{
    LoadOptions, Netlist,
    model::{Def, DefId, ElemKind, Element, ParamDef, ROOT, Root},
};
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// A deck in a new temporary directory, which is removed on drop.
struct TempDeck {
    dir: PathBuf,
    /// The first file.
    main: PathBuf,
}

impl Drop for TempDeck {
    fn drop(&mut self) {
        _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Writes `files` (name, text) into a new directory for test `name`.
fn write_deck(name: &str, files: &[(&str, &str)]) -> TempDeck {
    let dir =
        std::env::temp_dir().join(format!("netlist-query-lower-{}-{name}", std::process::id()));
    _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        std::fs::write(dir.join(file), text).unwrap();
    }
    let main = dir.join(files[0].0);
    TempDeck { dir, main }
}

fn load(path: PathBuf) -> Netlist {
    Netlist::load(&LoadOptions::new(path)).unwrap()
}

/// The active top-level definition `name`.
fn top_def<'n>(net: &'n Netlist, name: &str) -> &'n Def {
    let sym = net.syms.get(name).unwrap();
    &net.defs[net.def_index[&(ROOT, sym)].idx()]
}

fn elem_names(net: &Netlist, d: &Def) -> Vec<String> {
    d.elements
        .iter()
        .map(|e| net.syms.name(e.name).to_owned())
        .collect()
}

fn element<'n>(net: &'n Netlist, d: &'n Def, name: &str) -> &'n Element {
    let sym = net.syms.get(name).unwrap();
    d.elements.iter().find(|e| e.name == sym).unwrap()
}

/// (name, text) of the parameters of `e`.
fn params(net: &Netlist, d: &Def, e: &Element) -> Vec<(String, String)> {
    d.element_params(e)
        .iter()
        .map(|p| (net.syms.name(p.name).to_owned(), text(net, p)))
        .collect()
}

fn text(net: &Netlist, p: &ParamDef) -> String {
    p.text
        .map_or_else(|| format!("{:?}", p.num), |t| net.text(t).to_owned())
}

fn pins(net: &Netlist, d: &Def, e: &Element) -> Vec<String> {
    d.element_pins(e)
        .iter()
        .map(|&s| net.syms.name(s).to_owned())
        .collect()
}

#[test]
fn include_inside_subckt_belongs_to_def() {
    let net = load(repo("tests/top.sp"));
    let names: Vec<String> = (1..net.defs.len() as u32)
        .map(|i| net.def_display(DefId(i)))
        .collect();
    assert_eq!(names, ["DEMO", "UNITS"]);
    let demo = top_def(&net, "demo");
    assert_eq!(elem_names(&net, demo), ["X0.CCC"]);
    assert_eq!(demo.defaults.len(), 2);
    // top level: X0.BBB from inc1.sp (line 2), then X0.AAA
    let top = elem_names(&net, &net.defs[ROOT.idx()]);
    assert_eq!(top, ["X0.BBB", "X0.AAA"]);
    // `.lib '../lib.sp' pre_layout` in inc2.sp, inside DEMO (the
    // parser does not read `.params flag=1`: an unknown command)
    let pre: Vec<&str> = demo.params.iter().map(|p| net.syms.name(p.name)).collect();
    assert_eq!(pre, ["prelayout", "flag_cc"]);
    // units.sp: `.param` lines inside UNITS
    assert!(top_def(&net, "units").params.len() >= 10);

    let net = load(repo("golden/cases/include_in_subckt/deck.sp"));
    let holder = top_def(&net, "holder");
    assert_eq!(elem_names(&net, holder), ["ri", "rj", "r0"]);
    let rj = element(&net, holder, "rj");
    assert_eq!(
        params(&net, holder, rj),
        [("r".to_owned(), "pv*2".to_owned())]
    );
    let loc = net.loc(rj.loc);
    assert!(loc.file.ends_with("inc.sp"), "{loc:?}");
    assert_eq!(loc.line, 3);
}

#[test]
fn duplicate_first_is_active() {
    let net = load(repo("golden/cases/dup_subckt/deck.sp"));
    let inv: Vec<DefId> = (1..net.defs.len() as u32).map(DefId).collect();
    assert_eq!(inv.len(), 2);
    let first = top_def(&net, "inv");
    assert_eq!(first.loc.line, 2);
    assert!(first.has_dup && first.dup_of.is_none());
    let second = &net.defs[2];
    assert_eq!(second.dup_of, Some(DefId(1)));
    let r1 = element(&net, first, "r1");
    assert_eq!(params(&net, first, r1), [("r".to_owned(), "1".to_owned())]);
}

#[test]
fn nested_definitions() {
    let net = load(repo("golden/cases/nested_def/deck.sp"));
    let names: Vec<String> = (0..net.defs.len() as u32)
        .map(|i| net.def_display(DefId(i)))
        .collect();
    assert_eq!(
        names,
        ["(top level)", "cell", "outer", "outer>cell", "outer>deep"]
    );
    let outer = top_def(&net, "outer");
    assert_eq!(outer.nested, [DefId(3), DefId(4)]);
    assert_eq!(net.defs[3].parent, Some(DefId(2)));
    assert_eq!(net.defs[1].parent, None);
    assert_eq!(net.defs[ROOT.idx()].nested, [DefId(1), DefId(2)]);
    // nested `cell` has its own index entry; not a duplicate
    let cell = net.syms.get("cell").unwrap();
    assert_eq!(net.def_index[&(DefId(2), cell)], DefId(3));
    assert_eq!(net.def_index[&(ROOT, cell)], DefId(1));
    assert!(net.defs.iter().all(|d| d.dup_of.is_none()));
    // `X` lines keep the callee name only
    let xc = element(&net, outer, "xc");
    assert_eq!(xc.kind, ElemKind::Instance);
    assert_eq!(xc.callee, Some(cell));
    assert_eq!(pins(&net, outer, xc), ["a", "b"]);
    let deep = &net.defs[4];
    assert_eq!(element(&net, deep, "xc").callee, Some(cell));
    assert_eq!(net.root, Root::TopLevel);
}

#[test]
fn element_lowering() {
    let deck = write_deck(
        "elements",
        &[(
            "main.sp",
            "* t\n\
                 .model nch nmos level=1\n\
                 R1 a b 1k tc1=1e-3\n\
                 R2 a b rmod 2k\n\
                 R3 a b r='2*x' m=2\n\
                 C1 a 0 c=10pF\n\
                 M1 d g s b nch 2u 1u\n\
                 M2 d g s nch W=0.135u L=40.00n\n\
                 M3 d g s b e1 nch w=1u\n\
                 V1 a 0 1.2\n\
                 V2 a 0 PULSE(0,1,0,1n,1n,5n,10n)\n\
                 I1 a 0 PWL(0 0 1n 1m\n\
                 + 2n 0)\n\
                 Q1 c b e qmod 2\n\
                 D1 a k dmod\n\
                 X1 a b inv WN=2u m=2\n\
                 E1 out 0 in 0 2\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    assert_eq!(p("r1"), [s("r", "1k"), s("tc1", "1e-3")]);
    assert_eq!(p("r2"), [s("r", "2k")]);
    assert_eq!(element(&net, top, "r2").model, net.syms.get("rmod"));
    assert_eq!(p("r3"), [s("r", "2*x"), s("m", "2")]);
    assert_eq!(p("c1"), [s("c", "10pF")], "the keyword value as written");
    // the positional names take the deck's first spelling, from M2's
    // `W=` and `L=` (a later line)
    assert_eq!(p("m1"), [s("L", "2u"), s("W", "1u")]);
    assert_eq!(p("m2"), [s("W", "0.135u"), s("L", "40.00n")]);
    assert_eq!(pins(&net, top, element(&net, top, "m2")), ["d", "g", "s"]);
    assert_eq!(
        pins(&net, top, element(&net, top, "m3")),
        ["d", "g", "s", "b", "e1"]
    );
    assert_eq!(p("v1"), [s("dc", "1.2")]);
    // names as written
    assert_eq!(p("v2"), [s("PULSE", "0,1,0,1n,1n,5n,10n")]);
    // a source function: its arguments only, as written
    assert_eq!(p("i1"), [s("PWL", "0 0 1n 1m\n+ 2n 0")]);
    assert_eq!(p("q1"), [s("area", "2")]);
    assert_eq!(element(&net, top, "d1").model, net.syms.get("dmod"));
    assert_eq!(p("x1"), [s("WN", "2u"), s("m", "2")]);
    assert_eq!(element(&net, top, "x1").callee, net.syms.get("inv"));
    let e1 = element(&net, top, "e1");
    assert_eq!(e1.kind, ElemKind::Other(b'e'));
    assert_eq!(e1.kind.name(), "e");
    assert_eq!(pins(&net, top, e1), ["out", "0", "in", "0"]);
    assert_eq!(top.models.len(), 1);
    // numbers: text as written, value as read
    let w = &top.element_params(element(&net, top, "m2"))[0];
    assert_eq!(w.num, Some(0.135e-6));
}

#[test]
fn alter_flags() {
    let net = load(repo("golden/cases/alter/deck.sp"));
    let top = &net.defs[ROOT.idx()];
    let alters: Vec<(String, Option<u32>)> = top
        .params
        .iter()
        .map(|p| (text(&net, p), p.alter))
        .collect();
    assert_eq!(
        alters,
        [("1k".to_owned(), None), ("3k".to_owned(), Some(1))]
    );
    let r2_sym = net.syms.get("r2").unwrap();
    let r2: Vec<Option<u32>> = top
        .elements
        .iter()
        .filter(|e| e.name == r2_sym)
        .map(|e| e.alter)
        .collect();
    assert_eq!(r2, [None, Some(2)]);
    assert_eq!(net.alters.len(), 2);
}

#[test]
fn options_globals_temps() {
    let net = load(repo("golden/cases/global_net/deck.sp"));
    assert_eq!(net.globals, [net.syms.get("vdd").unwrap()]);
    let net = load(repo("golden/cases/temper/deck.sp"));
    assert_eq!(net.temps.len(), 1);
    assert_eq!(net.temps[0].num, Some(50.0));
    let net = load(repo("tests/top.sp"));
    // `.option gmindc=1e-14 scale=0.9` in the `pre_layout` section
    let opts: Vec<(&str, String)> = net
        .options
        .iter()
        .map(|p| (net.syms.name(p.name), text(&net, p)))
        .collect();
    assert_eq!(
        opts,
        [("gmindc", "1e-14".to_owned()), ("scale", "0.9".to_owned())]
    );
    // A `.temp` line whose value is not read gives no temperature, with
    // a Warning (only the Info `analysis_commands` before). Not probed.
    let deck = write_deck(
        "temp_not_read",
        &[("main.sp", "* t\n.temp=25\n.temp 25,50\n.temp\n.TEMP 30\n")],
    );
    let net = load(deck.main.clone());
    let lines: Vec<String> = diags_of(&net, "temp_not_read")
        .into_iter()
        .map(|d| d.1)
        .collect();
    assert_eq!(
        lines,
        (2..=4).map(|n| format!("main.sp:{n}")).collect::<Vec<_>>()
    );
    assert_eq!(net.temps.len(), 1);
    assert_eq!(net.temps[0].num, Some(30.0));
    // HSPICE reads a value in quotes (probes `.temp "50"` and `.temp '50'`:
    // `temp= 50.000`), and it rejects a group (`.temp (50)`)
    for line in [".temp \"50\"", ".temp '50'"] {
        let deck = write_deck("temp_quoted", &[("main.sp", &format!("* t\n{line}\n"))]);
        let net = load(deck.main.clone());
        assert_eq!(net.temps.len(), 1, "{line}");
    }
    let deck = write_deck("temp_group", &[("main.sp", "* t\n.temp (50)\n")]);
    let net = load(deck.main.clone());
    assert!(net.temps.is_empty());
    assert_eq!(diags_of(&net, "syntax_error").len(), 1);
}

/// The parser leaves the upper limit of the nodes of a MOSFET to the
/// readers of the line.
#[test]
fn mosfet_node_count() {
    let deck = write_deck(
        "mos_nodes",
        &[(
            "main.sp",
            "* t\nM1 d g s b x y z w nch W=1u\nM2 d g s b x y z nch W=1u\nM3 d g nch\n",
        )],
    );
    let net = load(deck.main.clone());
    let lines: Vec<String> = diags_of(&net, "node_count")
        .into_iter()
        .map(|d| d.1)
        .collect();
    assert_eq!(lines, ["main.sp:2", "main.sp:4"]);
    assert_eq!(elem_names(&net, &net.defs[ROOT.idx()]), ["M2"]);
}

#[test]
fn funcs_and_macro() {
    let net = load(repo("golden/cases/funcs/deck.sp"));
    let top = &net.defs[ROOT.idx()];
    assert_eq!(top.funcs.len(), 1);
    let f = &top.funcs[0];
    assert_eq!(f.args.len(), 2);
    assert!(!net.text(f.body).is_empty());
    let net = load(repo("golden/cases/macro_eom/deck.sp"));
    let inv = top_def(&net, "inv");
    assert_eq!(inv.defaults.len(), 1);
    assert_eq!(elem_names(&net, inv), ["r1"]);
}

#[test]
fn twice_included_file_reports_once() {
    let deck = write_deck(
        "twice",
        &[
            ("main.sp", "* t\n.inc 'a.sp'\n.inc 'a.sp'\n"),
            ("a.sp", "R1 a b 1\nR2 a b\n"),
        ],
    );
    let net = load(deck.main.clone());
    // both copies are in the model
    assert_eq!(net.defs[ROOT.idx()].elements.len(), 2);
    // the error of `R2 a b` is reported once
    let errors = net.diags.iter().filter(|d| d.code == "no_value").count();
    assert_eq!(errors, 1);
}

#[test]
fn diagnostics_in_document_order() {
    let deck = write_deck(
        "order",
        &[(
            "main.sp",
            "* t\nR1 a b\n.subckt s a\nR2 a b\n.ends\nR3 a b\n",
        )],
    );
    let net = load(deck.main.clone());
    let lines: Vec<u32> = net
        .diags
        .iter()
        .filter(|d| d.code == "no_value")
        .map(|d| d.loc.unwrap().line)
        .collect();
    assert_eq!(lines, [2, 4, 6]);
}

#[test]
fn temp_decks_are_removed() {
    let deck = write_deck("removed", &[("main.sp", "* t\nR1 a b 1\n")]);
    let dir = deck.dir.clone();
    assert!(dir.is_dir());
    drop(deck);
    assert!(!dir.exists());
}

#[test]
fn interner_single_copy() {
    // the X line comes first: its spellings are the first ones
    let deck = write_deck(
        "spelling",
        &[(
            "main.sp",
            "* t\nX1 VDD 0 INV WN=2u\n.subckt Inv A b wn=1u\nR1 A b 1k\n.param Pv=1\n.ends\nr2 vdd Gnd 1\n",
        )],
    );
    let net = load(deck.main.clone());
    let name = |s| net.syms.name(s);
    let inv = top_def(&net, "inv");
    assert_eq!(name(inv.name), "INV");
    let ports: Vec<&str> = inv.ports.iter().map(|&p| name(p)).collect();
    assert_eq!(ports, ["A", "b"]);
    assert_eq!(name(inv.defaults[0].name), "WN");
    assert_eq!(name(inv.params[0].name), "Pv");
    assert_eq!(elem_names(&net, inv), ["R1"]);
    let top = &net.defs[ROOT.idx()];
    assert_eq!(elem_names(&net, top), ["X1", "r2"]);
    assert_eq!(pins(&net, top, element(&net, top, "x1")), ["VDD", "0"]);
    assert_eq!(pins(&net, top, element(&net, top, "R2")), ["VDD", "Gnd"]);
    let x1 = element(&net, top, "x1");
    assert_eq!(x1.callee, Some(inv.name));
    assert_eq!(params(&net, top, x1), [("WN".to_owned(), "2u".to_owned())]);
}

#[test]
fn builtin_names_take_deck_spelling() {
    // `r`, `l`, `w`, `dc`, model types, ... are names that lowering adds
    // itself. Their first spelling in the deck is the one answers show,
    // also when the deck spells them after lowering added them. The
    // deck spells them as a keyword (`DC 1`, `AC=2`, `C=1p`, `L=1n`),
    // as a model type (`.model nch NMOS`), as `.TEMP`, or as any other
    // name (`.param R=2`, a net `Nmos`).
    let deck = write_deck(
        "builtins",
        &[(
            "main.sp",
            "* t\n\
                 .model nch NMOS level=1\n\
                 R1 a b 1k\n\
                 M1 d g s b nch 2u 1u\n\
                 V1 a 0 DC 1 AC=2\n\
                 .param R=2 Dc=3\n\
                 X1 Nmos b inv W=2\n\
                 C1 a b C=1p\n\
                 L1 a b lmod L='2*x'\n\
                 V2 b 0 dc=2 ac 1\n\
                 .model rm R\n\
                 R2 a b rm 1k\n\
                 .TEMP 50\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    assert_eq!(p("r1"), [s("R", "1k")]);
    assert_eq!(p("m1"), [s("L", "2u"), s("W", "1u")]);
    assert_eq!(p("v1"), [s("DC", "1"), s("AC", "2")]);
    assert_eq!(p("v2"), [s("DC", "2"), s("AC", "1")]);
    assert_eq!(p("x1"), [s("W", "2")]);
    assert_eq!(p("c1"), [s("C", "1p")]);
    assert_eq!(p("l1"), [s("L", "2*x")]);
    let pins = pins(&net, top, element(&net, top, "x1"));
    assert_eq!(pins, ["NMOS", "b"]);
    let kinds: Vec<&str> = top.models.iter().map(|m| net.syms.name(m.kind)).collect();
    assert_eq!(kinds, ["NMOS", "R"]);
    let names: Vec<&str> = top.params.iter().map(|p| net.syms.name(p.name)).collect();
    assert_eq!(names, ["R", "DC"]);
    assert_eq!(net.syms.name(net.temps[0].name), "TEMP");
    // No deck spelling: the built-in spelling.
    let deck = write_deck(
        "builtins_plain",
        &[(
            "main.sp",
            "* t\n.model nch nmos\nR1 a b 1k\nV1 a 0 1 AC 1 0\nM1 d g s b nch 2u\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    assert_eq!(p("r1"), [s("r", "1k")]);
    assert_eq!(p("v1"), [s("dc", "1"), s("AC", "1"), s("acphase", "0")]);
    assert_eq!(p("m1"), [s("l", "2u")]);
    assert_eq!(net.syms.name(top.models[0].kind), "nmos");
    // `R=` is the first spelling, also with spaces and an expression.
    for line in ["R1 a b R=1k", "R1 a b rmod R = 1k", "R1 a b R='1k'"] {
        let deck = write_deck("builtins_r", &[("main.sp", &format!("* t\n{line}\n"))]);
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        let r = &top.element_params(element(&net, top, "r1"))[0];
        assert_eq!(net.syms.name(r.name), "R", "{line}");
    }
}

#[test]
fn keyword_value_text() {
    let deck = write_deck(
        "kw_text",
        &[(
            "main.sp",
            "* t\n\
                 R1 a b r=1k\n\
                 C1 a 0 c=10pF\n\
                 R2 2 0 1k r=3k\n\
                 R3 a b 2k\n\
                 + r = 4k $X=9k\n\
                 R4 a b rmod R=5K\n\
                 L1 a b l='2*x'\n\
                 R5 a b 3000 r=3k\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    let s = |a: &str, b: &str| vec![(a.to_owned(), b.to_owned())];
    assert_eq!(p("r1"), s("r", "1k"));
    assert_eq!(p("c1"), s("c", "10pF"));
    assert_eq!(p("r2"), s("r", "3k"));
    assert_eq!(p("r3"), s("r", "4k"));
    assert_eq!(p("r4"), s("r", "5K"));
    assert_eq!(p("l1"), s("l", "2*x"));
    // the text of the winning `r=`, not the positional number
    assert_eq!(p("r5"), s("r", "3k"));
    let r3 = &top.element_params(element(&net, top, "r3"))[0];
    assert_eq!((r3.num, r3.loc.line), (Some(4e3), 6));
}

#[test]
fn key_value_next_line() {
    // `after_eq`: the `=` can end the line before the `+` line of the
    // value (golden `kw_value_next_line`)
    let at = |t: &str, v: &str| super::after_eq(t.as_bytes(), t.rfind(v).unwrap());
    assert!(at("r1 a b r=\n+ 5k", "5k"));
    assert!(at("r1 a b r= $ c\n* c\n\n   \n+ 5k", "5k"));
    assert!(at("r1 a b r=\n+ $ c\n+ 5k", "5k"));
    assert!(at("r1 a b r=\n+ '5k'", "5k'"));
    assert!(at("r1 a b\n+ r=\n+ 5k", "5k"));
    assert!(!at("r1 a b\n+ 5k", "5k"));
    assert!(!at("r1 a b r='a'\n+ 5k", "5k"));
    assert!(!at("r1 a b 'x=$'\n+ 5k", "5k"));
    assert!(!at("+ 5k", "5k"));
    assert!(!at("r1 a b 1k +5k", "5k"));
    let deck = write_deck(
        "kw_next_line",
        &[(
            "main.sp",
            "* t\n\
                 R1 a b r=\n\
                 + 5k\n\
                 R2 a b r= $ c\n\
                 * c\n\
                 \n\
                 + 6k tc1=\n\
                 + 0.004\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    assert_eq!(p("r1"), vec![("r".to_owned(), "5k".to_owned())]);
    assert_eq!(
        p("r2"),
        vec![
            ("r".to_owned(), "6k".to_owned()),
            ("tc1".to_owned(), "0.004".to_owned())
        ]
    );
    let r1 = &top.element_params(element(&net, top, "r1"))[0];
    assert_eq!((r1.num, r1.loc.line), (Some(5e3), 3));
    assert!(
        net.diags
            .iter()
            .all(|d| d.level != crate::api::Level::Error)
    );
}

#[test]
fn element_params_unique() {
    let deck = write_deck(
        "unique",
        &[(
            "main.sp",
            "* t\n\
                 R1 1 0 r=1k r=2k\n\
                 R2 2 0 1k tc1=1 TC1=2\n\
                 M1 d g s b nch 2u 1u l=3u\n\
                 X1 a b inv w=1 W=2 m=2\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    assert_eq!(p("r1"), [s("r", "2k")]);
    assert_eq!(p("r2"), [s("r", "1k"), s("tc1", "2")]);
    assert_eq!(p("m1"), [s("w", "1u"), s("l", "3u")]);
    assert_eq!(p("x1"), [s("w", "2"), s("m", "2")]);
}

#[test]
fn alter_in_included_subckt() {
    let deck = write_deck(
        "alter_sub",
        &[
            (
                "main.sp",
                "* t\n.subckt holder a b\n.inc 'alt.sp'\nr9 a b 9\n.ends\nx1 1 0 holder\nr0 1 0 1\n",
            ),
            ("alt.sp", "r1 a b 1\n.alter\nr2 a b 2\n"),
        ],
    );
    let net = load(deck.main.clone());
    let holder = top_def(&net, "holder");
    assert_eq!(elem_names(&net, holder), ["r1", "r2", "r9"]);
    // the marker is ignored: everything stays in the first run
    assert!(holder.elements.iter().all(|e| e.alter.is_none()));
    let top = &net.defs[ROOT.idx()];
    assert!(top.elements.iter().all(|e| e.alter.is_none()));
    assert!(net.alters.is_empty());
    let errs: Vec<_> = net
        .diags
        .iter()
        .filter(|d| d.code == "alter_in_subckt")
        .collect();
    assert_eq!(errs.len(), 1, "{:#?}", net.diags);
    assert_eq!(errs[0].level, crate::api::Level::Error);
    let loc = net.loc(errs[0].loc.unwrap());
    assert!(loc.file.ends_with("alt.sp"), "{loc:?}");
    assert_eq!(loc.line, 2);
}

#[test]
fn include_cycle_guard() {
    use netlist_db::{
        FileId, Files, Parsed, ParsedId,
        ast::{ASTBuilder, Segment},
    };
    use std::sync::{Arc, OnceLock};
    // top -> a -> b -> a: a parse result that the parser's file reuse
    // can give
    let include = |id: usize| {
        let res = OnceLock::new();
        assert!(res.set(Ok(ParsedId(id))).is_ok());
        Segment::Include {
            res: Arc::new(res),
            pos: None,
        }
    };
    let file = |name: &str, segments: Vec<Segment>| {
        let id = FileId::Include { path: name.into() };
        let ast = ASTBuilder {
            segments,
            ..Default::default()
        };
        (id, ast)
    };
    let parsed = Parsed {
        top_ids: vec![ParsedId(0)],
        id2idx: std::collections::HashMap::new(),
        inner: vec![
            file("top.sp", vec![include(1), include(2)]),
            file("a.sp", vec![include(2)]),
            file("b.sp", vec![include(1)]),
        ],
        lib_override_matched: Vec::new(),
    };
    let files = Files {
        inner: vec![String::new(); 3],
    };
    let net = super::lower(parsed, files, &LoadOptions::new("top.sp")).unwrap();
    let cycles: Vec<&crate::model::Diag> = net
        .diags
        .iter()
        .filter(|d| d.code == "include_cycle")
        .collect();
    // reported once: `b.sp` is read again from `top.sp`, but its
    // diagnostics are reported on its first visit only
    assert_eq!(cycles.len(), 1, "{:#?}", net.diags);
    assert_eq!(cycles[0].level, crate::api::Level::Error);
    assert!(
        cycles[0]
            .message
            .starts_with(r#"circular include: "a.sp" -> "b.sp" -> "a.sp"."#),
        "{}",
        cycles[0].message
    );
}

#[test]
fn include_cycles_deterministic() {
    // a -> b -> c -> a, and the top file includes `a` and `c`. The
    // parser reads each file once and no longer decides where a cycle
    // closes (that depended on the order in which the files were read).
    // The walk does, in document order: every load gives the same
    // elements and diagnostics.
    let deck = write_deck(
        "cycles_same",
        &[
            ("main.sp", "* t\n.inc 'a.sp'\n.inc 'c.sp'\n"),
            ("a.sp", ".inc 'b.sp'\nra 1 0 1\n"),
            ("b.sp", ".inc 'c.sp'\nrb 1 0 1\n"),
            ("c.sp", ".inc 'a.sp'\nrc 1 0 1\n"),
        ],
    );
    let dir = format!("{}/", deck.dir.display());
    let run = || {
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        let elements: Vec<String> = top
            .elements
            .iter()
            .map(|e| {
                let loc = net.loc(e.loc);
                format!("{} {}:{}", net.syms.name(e.name), loc.file, loc.line).replace(&dir, "")
            })
            .collect();
        let diags: Vec<(crate::api::Level, String, String)> = diags_of(&net, "include_cycle")
            .into_iter()
            .map(|(l, at, m)| (l, at, m.replace(&dir, "")))
            .collect();
        let all: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
        (elements, diags, all.join(" "))
    };
    let first = run();
    for _ in 0..19 {
        assert_eq!(run(), first);
    }
    let (elements, cycles, all) = first;
    // main -> a -> b -> c (its include of `a` closes a cycle), then
    // main -> c -> a -> b (its include of `c` closes one; a second read
    // reports nothing again)
    assert_eq!(
        elements,
        [
            "rc c.sp:2",
            "rb b.sp:2",
            "ra a.sp:2",
            "rb b.sp:2",
            "ra a.sp:2",
            "rc c.sp:2"
        ]
    );
    assert_eq!(
            cycles,
            [(
                crate::api::Level::Error,
                "c.sp:1".to_owned(),
                r#"circular include: "a.sp" -> "b.sp" -> "c.sp" -> "a.sp". The include of "a.sp" in "c.sp" was skipped"#
                    .to_owned()
            )]
        );
    // `ra`, `rb`, `rc` are read twice at the top level: HSPICE rejects
    // two lines with one name there (golden `elem_dup_name`)
    assert_eq!(
        all,
        "title_line include_cycle duplicate_element duplicate_element duplicate_element"
    );
    // A file that includes itself: one cycle, at its include line.
    let deck = write_deck(
        "cycle_self",
        &[("main.sp", "* t\nR1 a b 1\n.inc 'main.sp'\nR2 a b 2\n")],
    );
    let net = load(deck.main.clone());
    assert_eq!(elem_names(&net, &net.defs[ROOT.idx()]), ["R1", "R2"]);
    let cycles = diags_of(&net, "include_cycle");
    assert_eq!(cycles.len(), 1, "{:#?}", net.diags);
    assert_eq!(cycles[0].1, "main.sp:3");
    // A cycle of `.lib` sections of one file: the message names the
    // sections.
    let deck = write_deck(
        "cycle_sections",
        &[
            ("main.sp", "* t\n.lib 'm.lib' tt\n"),
            (
                "m.lib",
                ".lib tt\nRtt a b 1\n.lib 'm.lib' ff\n.endl tt\n.lib ff\nRff a b 2\n.lib 'm.lib' tt\n.endl ff\n",
            ),
        ],
    );
    let net = load(deck.main.clone());
    assert_eq!(elem_names(&net, &net.defs[ROOT.idx()]), ["Rtt", "Rff"]);
    let cycles = diags_of(&net, "include_cycle");
    assert_eq!(cycles.len(), 1, "{:#?}", net.diags);
    assert_eq!(cycles[0].1, "m.lib:7");
    let message = cycles[0].2.replace(&format!("{}/", deck.dir.display()), "");
    assert_eq!(
        message,
        r#"circular include: "m.lib (section tt)" -> "m.lib (section ff)" -> "m.lib (section tt)". The include of "m.lib (section tt)" in "m.lib (section ff)" was skipped"#
    );
}

/// The diagnostics with `code`: (level, file name and line, message).
fn diags_of(net: &Netlist, code: &str) -> Vec<(crate::api::Level, String, String)> {
    net.diags
        .iter()
        .filter(|d| d.code == code)
        .map(|d| {
            let at = d.loc.map_or_else(String::new, |l| {
                let loc = net.loc(l);
                let name = Path::new(&loc.file).file_name().unwrap().to_string_lossy();
                format!("{name}:{}", loc.line)
            });
            (d.level, at, d.message.clone())
        })
        .collect()
}

#[test]
fn source_lowering() {
    // Golden `src_forms`, `src_ac_phase`, `src_order_ac_first`: the DC
    // value as `dc`, the AC magnitude and phase as `ac` and `acphase`,
    // a function by its name with its arguments only. These decks
    // spell the keywords `DC` and `AC`, so answers show that spelling.
    let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    let net = load(repo("golden/cases/src_forms/deck.sp"));
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    assert_eq!(p("v1"), [s("DC", "1.2")]);
    assert_eq!(p("v2"), [s("DC", "1"), s("PULSE", "0 1 0 1n 1n 5n 10n")]);
    assert_eq!(p("v3"), [s("DC", "1.2"), s("AC", "1"), s("acphase", "0")]);
    assert_eq!(p("v4"), [s("PULSE", "0 1 0 1n 1n 5n 10n")]);
    assert_eq!(p("v5"), [s("DC", "2")]);
    assert_eq!(p("i1"), [s("DC", "1m")]);
    assert_eq!(p("v7"), [s("PULSE", "0.3 1 0 1n 1n 5n 10n")]);
    let dc = &top.element_params(element(&net, top, "i1"))[0];
    assert_eq!((dc.num, dc.loc.line), (Some(1e-3), 8));
    assert!(
        net.diags
            .iter()
            .all(|d| d.level != crate::api::Level::Error)
    );

    let net = load(repo("golden/cases/src_ac_phase/deck.sp"));
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    assert_eq!(p("v1"), [s("DC", "1.2"), s("AC", "1"), s("acphase", "45")]);
    assert_eq!(p("v2"), [s("DC", "0.5"), s("AC", "2"), s("acphase", "90")]);
    let net = load(repo("golden/cases/src_order_ac_first/deck.sp"));
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    assert_eq!(p("v1"), [s("DC", "0.7"), s("AC", "1"), s("acphase", "45")]);
    let acphase = &top.element_params(element(&net, top, "v1"))[2];
    assert_eq!(acphase.num, Some(45.0));

    // Expressions, `key=value`, and a function on `+` lines.
    let deck = write_deck(
        "sources",
        &[(
            "main.sp",
            "* t\n\
                 V1 a 0 'vdd/2' AC=1 foo=2\n\
                 V2 a 0 SIN(0 1\n\
                 + 1k) $ c\n\
                 V3 a 0 PWL(0 0 $ start\n\
                 * a comment line\n\
                 + 1n 1)\n\
                 V4 a 0\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    assert_eq!(p("v1"), [s("dc", "vdd/2"), s("AC", "1"), s("foo", "2")]);
    assert_eq!(p("v2"), [s("SIN", "0 1\n+ 1k")]);
    let f = &top.element_params(element(&net, top, "v2"))[0];
    assert_eq!(f.loc.line, 3);
    // The stored text keeps the `+` markers and comments; answers show
    // it on one line through `param_text`.
    assert_eq!(net.param_text(f).as_deref(), Some("0 1 1k"));
    let f = &top.element_params(element(&net, top, "v3"))[0];
    assert_eq!(
        net.text(f.text.unwrap()),
        "0 0 $ start\n* a comment line\n+ 1n 1"
    );
    assert_eq!(net.param_text(f).as_deref(), Some("0 0 1n 1"));
    let v1 = &top.element_params(element(&net, top, "v1"))[0];
    assert!(matches!(net.param_text(v1), Some(Cow::Borrowed("vdd/2"))));
    // A `$` inside quotes is not a comment (golden `rcl_quoted_dollar`,
    // `rcl_quoted_dollar_line`): it stays, with the text after it; a
    // `$` outside quotes starts one, also after a quoted word.
    assert_eq!(one_line("rval*2 $ c", true), "rval*2 $ c");
    assert_eq!(one_line("rval $ c\n+ *2", true), "rval $ c *2");
    assert_eq!(one_line("rval\n+ *3", true), "rval *3");
    assert_eq!(one_line("0 '0.3 $ c' 1n 1 $ d", false), "0 '0.3 $ c' 1n 1");
    assert_eq!(one_line("0 'a' 1n $ it's\n+ 1", false), "0 'a' 1n 1");
    assert_eq!(one_line("0 0 $ c\n* it's\n+ 1n 1", false), "0 0 1n 1");
    // A `*` comment line inside quotes stays on a line of its own
    // (golden `quoted_cmtline`); outside quotes it is passed over.
    assert_eq!(one_line("a\n* c\n+ *5", true), "a\n* c\n*5");
    assert_eq!(one_line("\n* c\n+ a", true), "\n* c\na");
    assert_eq!(
        one_line("0 '0.3\n* c\n+ *1' 1n", false),
        "0 '0.3\n* c\n*1' 1n"
    );
    assert_eq!(one_line("'a' 1n\n* it's\n+ 1", false), "'a' 1n 1");
    assert_eq!(quote_fault("a\n* c\n*5"), Some(QuoteFault::CommentLine));
    assert_eq!(quote_fault("a $ c *2"), Some(QuoteFault::Dollar));
    assert_eq!(quote_fault("0 'a' 1n 1"), None);
    for (t, quoted) in [
        ("a\n* c\n+ *5", true),
        ("a $ c\n* d\n+ *5", true),
        ("a $ c\n+ *5", true),
        ("a\n+ *5", true),
        ("0 '0.3\n* c\n+ *1' 1n", false),
        ("'a' 1n\n* it's\n+ 1", false),
        ("'a' $ it's\n+ '1 $ c'", false),
        ("0 0 $ c\n* it's\n+ 1n 1", false),
        ("x*2 $ c", true),
        ("\n* c\n+ a", true),
        ("a\n\n+ *5", true),
        ("a\n   \t\n+ *5", true),
        ("a\n$ c\n+ *5", true),
        ("a\n+\n+ *5", true),
        ("max(1k,\n\n+ 3k)", false),
        ("max(1k,\n$ c\n+ 3k)", false),
        ("max(1k,\n* c\n+ 3k)", false),
        ("'(' x\n* c\n+ 1", false),
        ("0 0.3\n* c\n+ 1n 1", false),
    ] {
        // `quote_fault` does not tell a comment line in parentheses
        // from one in quotes
        let raw = super::raw_quote_fault(t, quoted).map(|f| match f {
            QuoteFault::ParenCommentLine => QuoteFault::CommentLine,
            f => f,
        });
        assert_eq!(raw, quote_fault(&one_line(t, quoted)), "{t:?}");
    }
    // Inside quotes, a line that is not a `+` line stays on a line of
    // its own: a blank line (golden `quoted_blank_line`), a line with
    // only spaces and a tab (`quoted_spaces_elem`) and a `$` comment
    // line (`quoted_dollarline_elem`). A `+` line with no text is
    // passed over (`quoted_plus_empty_elem`).
    assert_eq!(one_line("a\n\n+ *5", true), "a\n\n*5");
    assert_eq!(one_line("a\n   \t\n+ *5", true), "a\n\n*5");
    assert_eq!(one_line("a\n$ c\n+ *5", true), "a\n$ c\n*5");
    assert_eq!(one_line("a\n+\n+ *5", true), "a *5");
    for t in ["a\n\n+ *5", "a\n   \t\n+ *5", "a\n$ c\n+ *5"] {
        assert_eq!(
            super::raw_quote_fault(t, true),
            Some(QuoteFault::CommentLine)
        );
    }
    assert_eq!(super::raw_quote_fault("a\n+\n+ *5", true), None);
    // Inside open parentheses with no quotes, a blank line and a `$`
    // comment line are passed over (golden `paren_blank_call`), and a
    // `*` comment line stays (golden `paren_cmtline_call`). With the
    // parentheses closed, it is passed over (`paren_cmtline_pwl`: the
    // argument text of a source function has no outer parentheses).
    assert_eq!(one_line("max(1k,\n\n+ 3k)", false), "max(1k, 3k)");
    assert_eq!(one_line("max(1k,\n$ c\n+ 3k)", false), "max(1k, 3k)");
    assert_eq!(one_line("max(1k,\n* c\n+ 3k)", false), "max(1k,\n* c\n3k)");
    assert_eq!(
        super::raw_quote_fault("max(1k,\n* c\n+ 3k)", false),
        Some(QuoteFault::ParenCommentLine)
    );
    assert_eq!(one_line("0 0.3\n* c\n+ 1n 1", false), "0 0.3 1n 1");
    assert_eq!(super::raw_quote_fault("0 0.3\n* c\n+ 1n 1", false), None);
    // a `(` inside quotes does not count
    assert_eq!(one_line("'(' x\n* c\n+ 1", false), "'(' x 1");
    assert_eq!(super::raw_quote_fault("'(' x\n* c\n+ 1", false), None);
    // No source part: a 0 V source with no parameters (HSPICE manual).
    assert!(p("v4").is_empty());
    assert!(
        net.diags
            .iter()
            .all(|d| d.level != crate::api::Level::Error),
        "{:#?}",
        net.diags
    );
    // `ac_then_function`: a function with no space in it, after the AC
    // magnitude, is the function (it was the AC phase before).
    let deck = write_deck(
        "ac_then_function",
        &[(
            "main.sp",
            "* t\n\
                 V1 a 0 AC 1 SIN(0,1,1meg)\n\
                 V2 b 0 DC 0 AC 1 PULSE(0,1.8,0,1n,1n,5n,10n)\n\
                 V3 c 0 AC=1 PWL(0,0,1n,1)\n\
                 V4 d 0 PULSE(0 1) AC 1 SIN(0,1,1k)\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    assert_eq!(p("v1"), [s("SIN", "0,1,1meg"), s("AC", "1")]);
    assert_eq!(
        p("v2"),
        [
            s("DC", "0"),
            s("PULSE", "0,1.8,0,1n,1n,5n,10n"),
            s("AC", "1")
        ]
    );
    assert_eq!(p("v3"), [s("PWL", "0,0,1n,1"), s("AC", "1")]);
    assert_eq!(elem_names(&net, top), ["V1", "V2", "V3"]);
    let errs = diags_of(&net, "bad_source");
    assert_eq!(errs.len(), 1, "{:#?}", net.diags);
    assert_eq!(errs[0].1, "main.sp:5");
    assert!(
        errs[0].2.contains("two transient functions"),
        "{}",
        errs[0].2
    );
    // `PULSE(...) DC 0.7` is rejected (golden `src_order_fn_first`).
    let net = load(repo("golden/cases/src_order_fn_first/deck.sp"));
    let errs = diags_of(&net, "bad_source");
    assert_eq!(errs.len(), 1, "{:#?}", net.diags);
    assert!(errs[0].2.contains("Write `DC` first"), "{}", errs[0].2);
}

#[test]
fn rcl_order_lowering() {
    let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    // Golden `res_tc_positional`: the numbers after the value are tc1
    // and tc2.
    let net = load(repo("golden/cases/res_tc_positional/deck.sp"));
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    assert_eq!(p("r1"), [s("r", "1k"), s("tc1", "0.004")]);
    assert_eq!(p("r2"), [s("r", "1k"), s("tc1", "0.004"), s("tc2", "1e-5")]);
    assert_eq!(p("r3"), p("r1"));
    assert_eq!(p("r4"), p("r2"));
    // The numbers after a value or a model name keep their text as
    // written: on `+` lines, after a value that `r=` replaces, and
    // after `OFF`, which takes the place of `l`.
    let deck = write_deck(
        "positional_text",
        &[(
            "main.sp",
            "* t\n\
                 R9 a 0 1k\n\
                 + 4m $ c\n\
                 + 1e-5\n\
                 R20 a 0 1k 0.004 r=2k\n\
                 M6 d g s b nch off 2u\n\
                 + 1u\n\
                 M7 d g s b nch w=2u 1u\n\
                 M8 d g s b nch 1u w=2u 3u\n\
                 M9 d g s b nch 1u 'k' w=5u\n\
                 M10 d g s b nch 1u 2u w=5u\n\
                 M11 d g s b nch w=k w=5u\n\
                 D1 a 0 dmod 2 1e-6\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    assert_eq!(p("r9"), [s("r", "1k"), s("tc1", "4m"), s("tc2", "1e-5")]);
    assert_eq!(p("r20"), [s("r", "2k"), s("tc1", "0.004")]);
    assert_eq!(p("m6"), [s("w", "2u"), s("ad", "1u")]);
    assert_eq!(p("m7"), [s("w", "1u")]);
    assert_eq!(p("m8"), [s("l", "1u"), s("w", "2u"), s("ad", "3u")]);
    // the plain numbers come first and then the expressions, and the last stays
    assert_eq!(p("m9"), [s("l", "1u"), s("w", "k")]);
    assert_eq!(p("m10"), [s("l", "1u"), s("w", "5u")]);
    assert_eq!(p("m11"), [s("w", "k")]);
    assert_eq!(p("d1"), [s("area", "2"), s("pj", "1e-6")]);
    let tc2 = &top.element_params(element(&net, top, "r9"))[2];
    assert_eq!((tc2.num, tc2.loc.line), (Some(1e-5), 4));
    // The value after the nodes or the model name keeps its text and its
    // line also on a `+` line and before a `$` comment.
    let deck = write_deck(
        "value_text",
        &[(
            "main.sp",
            "* t\n\
                 R1 a b rmod\n\
                 + 2k\n\
                 R3 a b\n\
                 * c\n\
                 + 1k $ c\n\
                 R2 a b 1k$c\n\
                 L1 a b\n\
                 + 1n lmod\n\
                 C1 a b 1p\n\
                 + 0.004\n\
                 R4 a b 2k\n\
                 R5 a b 3k\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    for (name, key, want, line) in [
        ("R1", "r", "2k", 3),
        ("R3", "r", "1k", 6),
        ("R2", "r", "1k", 7),
        ("L1", "l", "1n", 9),
        ("C1", "c", "1p", 10),
        ("R4", "r", "2k", 12),
    ] {
        let v = &top.element_params(element(&net, top, name))[0];
        assert_eq!(net.syms.name(v.name), key, "{name}");
        assert_eq!(
            (text(&net, v), v.loc.line),
            (want.to_owned(), line),
            "{name}"
        );
    }
    // Golden `ind_value_model`: L reads "value, then model".
    let net = load(repo("golden/cases/ind_value_model/deck.sp"));
    let top = &net.defs[ROOT.idx()];
    let l1 = element(&net, top, "l1");
    assert_eq!(l1.model, net.syms.get("lmod"));
    assert_eq!(params(&net, top, l1), [s("l", "1n")]);
    // Golden `res_value_model`, `cap_value_model`: an error that says
    // what HSPICE does.
    for (case, line, what) in [
        ("res_value_model", 5, "reads the value as a third node"),
        ("cap_value_model", 5, "uses a capacitance of 0"),
    ] {
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let errs = diags_of(&net, "value_then_model");
        assert_eq!(errs.len(), 1, "{case}: {:#?}", net.diags);
        assert_eq!(errs[0].0, crate::api::Level::Error);
        assert_eq!(errs[0].1, format!("deck.sp:{line}"), "{case}");
        assert!(errs[0].2.contains(what), "{case}: {}", errs[0].2);
    }
    // Golden `mos_digit_model_ref`: a model name that starts with a
    // digit.
    let net = load(repo("golden/cases/mos_digit_model_ref/deck.sp"));
    let errs = diags_of(&net, "digit_first_model");
    assert!(!errs.is_empty(), "{:#?}", net.diags);
    assert!(errs[0].2.contains("`5vnmos`"), "{}", errs[0].2);
    // A token before the value where a model name goes: a value with a
    // unit, or a name with a character that model names do not have.
    // Comment-only `+` lines (any number) keep the element.
    let deck = write_deck(
        "model_place",
        &[(
            "main.sp",
            "* t\n\
                 C1 a b 10pF 0.004\n\
                 R1 a b 1kohm r=2k\n\
                 R2 a b rmod-1 2k\n\
                 R6 a b 1k $X=1\n\
                 + $Y=2\n\
                 + $Z=3\n\
                 R7 a b 2k\n\
                 M1 d g s b e 5vnmos\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    assert_eq!(elem_names(&net, top), ["R6", "R7", "M1"]);
    let p = |name: &str| params(&net, top, element(&net, top, name));
    assert_eq!(p("r6"), [s("r", "1k")]);
    assert_eq!(p("m1"), [s("l", "5vnmos")]);
    assert_eq!(element(&net, top, "m1").model, net.syms.get("e"));
    let digit = diags_of(&net, "digit_first_model");
    let at: Vec<&str> = digit.iter().map(|e| e.1.as_str()).collect();
    assert_eq!(at, ["main.sp:2", "main.sp:3"], "{:#?}", net.diags);
    assert!(
        digit[0].2.starts_with(
            "a capacitor line has `10pF` before its value: R and C take \"model, then value\""
        ),
        "{}",
        digit[0].2
    );
    let bad = diags_of(&net, "bad_model_name");
    assert_eq!(bad.len(), 1, "{:#?}", net.diags);
    assert_eq!(bad[0].1, "main.sp:4");
    assert!(bad[0].2.contains("`rmod-1`"), "{}", bad[0].2);
    assert_eq!(
        net.diags
            .iter()
            .filter(|d| d.level == crate::api::Level::Error)
            .count(),
        3,
        "{:#?}",
        net.diags
    );
}

#[test]
fn after_end_info() {
    // Golden `after_end`: the lines after `.end` are not read.
    let net = load(repo("golden/cases/after_end/deck.sp"));
    let top = &net.defs[ROOT.idx()];
    assert_eq!(elem_names(&net, top), ["v1", "r1"]);
    assert_eq!(top.params.len(), 1);
    let info = diags_of(&net, "after_end");
    assert_eq!(
            info,
            [(
                crate::api::Level::Info,
                "deck.sp:6".to_owned(),
                "3 lines after `.end` were not read: they do not join this deck, as in HSPICE. A second deck after `.end` (HSPICE runs it as a second job) is not loaded".to_owned()
            )]
        );
    // Only comments after `.end`: nothing to say.
    let deck = write_deck("end_comments", &[("main.sp", "* t\nR1 a b 1\n.end\n* c\n")]);
    let net = load(deck.main.clone());
    assert!(diags_of(&net, "after_end").is_empty());
    // A `.lib` section after `.end` is read by its `.lib` call; only
    // the other lines are "not read".
    let deck = write_deck(
        "end_lib",
        &[(
            "main.sp",
            "* t\n.lib 'main.sp' tt\nR1 a b p\n.end\n.lib tt\n.param p=3\n.endl tt\nR2 a b 2\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    assert_eq!(elem_names(&net, top), ["R1"]);
    assert_eq!(top.params.len(), 1);
    let info = diags_of(&net, "after_end");
    assert_eq!(info.len(), 1, "{:#?}", net.diags);
    assert_eq!(
        info[0].2,
        "1 line after `.end` was not read: it does not join this deck, as in HSPICE. A second deck after `.end` (HSPICE runs it as a second job) is not loaded"
    );
    let deck = write_deck(
        "end_lib_only",
        &[(
            "main.sp",
            "* t\n.lib 'main.sp' tt\n.end\n.lib tt\n.param p=3\n.endl tt\n",
        )],
    );
    let net = load(deck.main.clone());
    assert!(diags_of(&net, "after_end").is_empty(), "{:#?}", net.diags);
}

/// The walk of the includes has the limit of `Files::build`. (A big stack: the
/// frames of a debug build are big.)
#[test]
fn include_chain_has_a_limit() {
    let n = netlist_db::parser::MAX_INCLUDE_DEPTH + 10;
    let files: Vec<(String, String)> = (0..n)
        .map(|i| {
            (
                format!("f{i}.sp"),
                format!("R{i} a b 1\n.inc 'f{}.sp'\n", i + 1),
            )
        })
        .chain([(format!("f{n}.sp"), "R a b 1\n".to_owned())])
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let deck = write_deck("include_limit", &refs);
    let path = deck.main.clone();
    let codes = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let net = load(path);
            diags_of(&net, "include_limit").len()
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(codes, 1);
}

#[test]
fn non_utf8_warning() {
    // Golden `nonutf8_comment`: the deck is read, with a warning.
    let net = load(repo("golden/cases/nonutf8_comment/deck.sp"));
    let top = &net.defs[ROOT.idx()];
    assert_eq!(elem_names(&net, top), ["v1", "r1"]);
    let warn = diags_of(&net, "non_utf8");
    assert_eq!(warn.len(), 1, "{:#?}", net.diags);
    assert_eq!(warn[0].0, crate::api::Level::Warning);
    assert_eq!(warn[0].1, "deck.sp:2");
    assert!(warn[0].2.contains("private character"), "{}", warn[0].2);
    // Once per file, also when the file is included twice.
    let deck = write_deck(
        "non_utf8",
        &[("main.sp", "* t\n.inc 'a.sp'\n.inc 'a.sp'\n"), ("a.sp", "")],
    );
    std::fs::write(deck.dir.join("a.sp"), b"R\xe91 a b 1\n").unwrap();
    let net = load(deck.main.clone());
    let warn = diags_of(&net, "non_utf8");
    assert_eq!(warn.len(), 1, "{:#?}", net.diags);
    assert_eq!(warn[0].1, "a.sp:1");
    // One file read for two `.lib` sections: one warning, at the first
    // bad line in a section that was read. A bad byte outside the
    // sections that were read gives none.
    let deck = write_deck(
        "non_utf8_lib",
        &[(
            "main.sp",
            "* t\n.lib 'm.lib' tt\n.lib 'm.lib' ff\n.lib 'n.lib' tt\n",
        )],
    );
    std::fs::write(
        deck.dir.join("m.lib"),
        b".lib tt\nR1 a b 1 $ \xe9\n.endl tt\n.lib ff\nR2 a b 2 $ \xe9\n.endl ff\n",
    )
    .unwrap();
    std::fs::write(
        deck.dir.join("n.lib"),
        b".lib tt\nR3 a b 3\n.endl tt\n* \xe9\n.lib ff\nR4 a b 4\n.endl ff\n",
    )
    .unwrap();
    let net = load(deck.main.clone());
    let warn = diags_of(&net, "non_utf8");
    assert_eq!(warn.len(), 1, "{:#?}", net.diags);
    assert_eq!(warn[0].1, "m.lib:2");
}

#[test]
fn comment_only_plus_at_end_of_file() {
    // A comment-only `+` line that ends the file with no line end after
    // it continues the statement, as with the line end: the statement
    // is kept, with no diagnostic of its own (`X1` calls `inv`, which
    // the deck does not define, so the hierarchy analysis gives the
    // Error `unresolved_subckt` for it).
    for (k, (body, elem)) in [
        ("R1 a b 1k $X=1\n+ $Y=2", Some("R1")),
        ("X1 a b inv $c\n+ $d", Some("X1")),
        ("V1 a 0 DC 1 $c\n+ $d", Some("V1")),
        ("R1 a b 1k $c\n+", Some("R1")),
        ("R1 a b 1k $c\n+ $d\n+ $e", Some("R1")),
        (".param a=1 $c\n+ $d", None),
    ]
    .into_iter()
    .enumerate()
    {
        // CRLF also on every line, not only at the end
        let crlf = body.replace('\n', "\r\n");
        for (body, end) in [
            (body, ""),
            (body, "\n"),
            (body, "\r\n"),
            (crlf.as_str(), ""),
            (crlf.as_str(), "\r\n"),
        ] {
            let text = if end.is_empty() || end == "\n" {
                format!("* t\n{body}{end}")
            } else {
                format!("* t\r\n{body}{end}")
            };
            let deck = write_deck(&format!("plus_eof_{k}"), &[("main.sp", &text)]);
            let net = load(deck.main.clone());
            let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
            // `X1` calls `inv`, which the deck does not define: the
            // hierarchy analysis reports it
            let want: &[&str] = if elem == Some("X1") {
                &["title_line", "unresolved_subckt"]
            } else {
                &["title_line"]
            };
            assert_eq!(codes, want, "{text:?}: {:#?}", net.diags);
            let top = &net.defs[ROOT.idx()];
            match elem {
                Some(name) => assert_eq!(elem_names(&net, top), [name], "{text:?}"),
                None => assert_eq!(top.params.len(), 1, "{text:?}"),
            }
        }
    }
}

#[test]
fn param_braces() {
    // Golden `param_braces`, `param_braces_param`: HSPICE rejects
    // `{...}`, and the error says so.
    let net = load(repo("golden/cases/param_braces/deck.sp"));
    let errs = diags_of(&net, "braces");
    let lines: Vec<&str> = errs.iter().map(|e| e.1.as_str()).collect();
    assert_eq!(lines, ["deck.sp:2", "deck.sp:4"], "{:#?}", net.diags);
    for e in &errs {
        assert!(e.2.contains("Write the expression in quotes"), "{}", e.2);
    }
    let net = load(repo("golden/cases/param_braces_param/deck.sp"));
    assert_eq!(diags_of(&net, "braces").len(), 1, "{:#?}", net.diags);
    // Also after `DC` and `AC` on a source line, where the parser says
    // that no value follows.
    // Also in the arguments of a source function, in or out of
    // parentheses.
    let deck = write_deck(
        "braces_source",
        &[(
            "main.sp",
            "* t\nV1 a 0 DC {x}\nV2 b 0 AC {x}\nV3 a 0 PULSE({x} 1 0 1n)\nV4 a 0 SIN(0 1\n+ {f})\nV5 a 0 PULSE {x} 1 0 1n\nV6 a 0 PULSE(0 1 0 1n) $ {x}\nR1 a b 1\n",
        )],
    );
    let net = load(deck.main.clone());
    let lines: Vec<String> = diags_of(&net, "braces").into_iter().map(|e| e.1).collect();
    assert_eq!(
        lines,
        [
            "main.sp:2",
            "main.sp:3",
            "main.sp:4",
            "main.sp:5",
            "main.sp:7"
        ],
        "{:#?}",
        net.diags
    );
    assert!(diags_of(&net, "bad_source").is_empty(), "{:#?}", net.diags);
    let top = &net.defs[ROOT.idx()];
    assert_eq!(elem_names(&net, top), ["V6", "R1"]);
    // In quotes too (golden `brace_quoted_param`, `brace_quoted_elem`,
    // `brace_quoted_fn`): HSPICE reads `{a}` there as the name `[a]`
    // and rejects the deck. Each line gets `braces` (they were kept
    // with no diagnostic before, and `'{c}'` read as `c`), also with
    // another error on the line.
    let deck = write_deck(
        "braces_quoted",
        &[(
            "main.sp",
            "* t\nR2 a b '{c}'\n.param p='{2}'\nV5 a 0 PWL(0 0 1n '{x}')\nV7 a 0 PWL(0 0 1n '{x}') PULSE(0 1)\nR8 a b 1\n",
        )],
    );
    let net = load(deck.main.clone());
    let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
    assert_eq!(
        codes,
        ["title_line", "braces", "braces", "braces", "braces"],
        "{:#?}",
        net.diags
    );
    let top = &net.defs[ROOT.idx()];
    assert_eq!(elem_names(&net, top), ["R8"]);
    assert!(top.params.is_empty());
}

#[test]
fn braces_quoted_golden() {
    // Golden `brace_quoted_param`, `brace_quoted_elem`,
    // `brace_quoted_fn`, `brace_fn_unquoted`: HSPICE aborts on `{a}`
    // inside quotes in all three places, as on `{a}` without quotes.
    // One `braces` Error at the line, which is skipped: no value for
    // it, never a number.
    for (case, line, elements, params) in [
        ("brace_quoted_param", "deck.sp:2", &["v1", "R1"][..], 0),
        ("brace_quoted_elem", "deck.sp:4", &["v1"], 1),
        ("brace_quoted_fn", "deck.sp:4", &["r1"], 1),
        ("brace_fn_unquoted", "deck.sp:4", &["r1"], 1),
    ] {
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let errs = diags_of(&net, "braces");
        assert_eq!(errs.len(), 1, "{case}: {:#?}", net.diags);
        assert_eq!(errs[0].0, crate::api::Level::Error, "{case}");
        assert_eq!(errs[0].1, line, "{case}");
        assert!(errs[0].2.contains("also inside quotes"), "{}", errs[0].2);
        let errors = net
            .diags
            .iter()
            .filter(|d| d.level == crate::api::Level::Error)
            .count();
        assert_eq!(errors, 1, "{case}: {:#?}", net.diags);
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), elements, "{case}");
        assert_eq!(top.params.len(), params, "{case}");
    }
    // Golden `brace_quoted_temp`, `brace_temp`: HSPICE aborts on `{a}`
    // in a `.temp` value too, in quotes or not. The line gets `braces`
    // (the temperature was lost with no diagnostic before), and it
    // gives no temperature.
    for case in ["brace_quoted_temp", "brace_temp"] {
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let errs = diags_of(&net, "braces");
        assert_eq!(errs.len(), 1, "{case}: {:#?}", net.diags);
        assert_eq!(errs[0].1, "deck.sp:3", "{case}");
        let errors = net
            .diags
            .iter()
            .filter(|d| d.level == crate::api::Level::Error)
            .count();
        assert_eq!(errors, 1, "{case}: {:#?}", net.diags);
        assert!(net.temps.is_empty(), "{case}");
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["v1", "R1"], "{case}");
    }
    // Braces in a parameter of a D, Q or M line, or in another analysis
    // command, also give `braces`, not the error that the failed
    // parameter causes (`D1` gave `node_count`: the failed `area=`
    // read as a node).
    let deck = write_deck(
        "braces_shape",
        &[(
            "main.sp",
            "* t\n.model dmod d\nD1 a k dmod area='{a}'\nD2 a k dmod area={a}\nQ1 c b e qmod area='{a}'\nM1 d g s b nch l='{a}'\nM2 d g s b nch 1u w='{a}'\n.tran 1n '{a}'\n.tran 1n 2n\nR8 a b 1\n",
        )],
    );
    let net = load(deck.main.clone());
    let lines: Vec<String> = diags_of(&net, "braces").into_iter().map(|e| e.1).collect();
    assert_eq!(
        lines,
        (3..=8).map(|n| format!("main.sp:{n}")).collect::<Vec<_>>(),
        "{:#?}",
        net.diags
    );
    let errors = net
        .diags
        .iter()
        .filter(|d| d.level == crate::api::Level::Error)
        .count();
    assert_eq!(errors, 6, "{:#?}", net.diags);
    let top = &net.defs[ROOT.idx()];
    assert_eq!(elem_names(&net, top), ["R8"]);
    // `brace_quoted_param`: no `p` (nor `a`, on the same line), so the
    // `p` of R1 has no value.
    let net = load(repo("golden/cases/brace_quoted_param/deck.sp"));
    assert!(
        net.syms
            .get("p")
            .is_none_or(|p| { net.defs[ROOT.idx()].params.iter().all(|d| d.name != p) })
    );
    // A `{` on a `+` line after a `$` comment line (the lowering passed
    // over `*` comment lines only, so it gave `syntax_error`), and in
    // quotes after a `*` comment line.
    let deck = write_deck(
        "braces_dollar_line",
        &[(
            "main.sp",
            "* t\nR6 a b 1k\n$ c\n+ m={x}\nR7 a b 1k\n* c\n$ d\n+ m='{x}'\nR8 a b 1\n",
        )],
    );
    let net = load(deck.main.clone());
    let lines: Vec<String> = diags_of(&net, "braces").into_iter().map(|e| e.1).collect();
    assert_eq!(lines, ["main.sp:2", "main.sp:5"], "{:#?}", net.diags);
    assert!(
        diags_of(&net, "syntax_error").is_empty(),
        "{:#?}",
        net.diags
    );
    assert!(
        diags_of(&net, "trailing_text").is_empty(),
        "{:#?}",
        net.diags
    );
}

#[test]
fn pwl_repeat_golden() {
    // Golden `pwl_repeat`, `pwl_repeat_td`, `pwl_td`: HSPICE accepts
    // `R`, `R=0 TD=1n` and `TD=1n` after the `)` of a PWL (`R` gave
    // `trailing_text` and lost V1 before). `R` alone is `R=0` (repeat
    // time 0 in `.option list` for both); the PWL keeps its points.
    // Golden `pwl_repeat_noparen`, `pwl_repeat_key_bare`, `pwl_ac_r`:
    // `R` is the repeat flag also after PWL values with no parentheses,
    // after `R=1n` (the last one wins), and after the AC magnitude.
    let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    for (case, want) in [
        (
            "pwl_repeat",
            vec![s("PWL", "0 0.3 1n 1"), s("R", "Some(0.0)")],
        ),
        (
            "pwl_repeat_td",
            vec![s("PWL", "0 0.3 1n 1"), s("R", "0"), s("TD", "1n")],
        ),
        ("pwl_td", vec![s("PWL", "0 0.3 1n 1"), s("TD", "1n")]),
        // `R` is the repeat flag, not a fifth argument
        (
            "pwl_repeat_noparen",
            vec![s("PWL", "0 0.3 1n 1"), s("R", "Some(0.0)")],
        ),
        // `R=1n R`: the bare `R` wins (repeat delay 0)
        (
            "pwl_repeat_key_bare",
            vec![s("PWL", "0 0.3 1n 1"), s("R", "Some(0.0)")],
        ),
        // `AC 1 R`: `R` is the repeat flag, not the AC phase (phase 0)
        (
            "pwl_ac_r",
            vec![s("PWL", "0 0.3 1n 1"), s("AC", "1"), s("R", "Some(0.0)")],
        ),
        // `R=1n` before the last time point: repeat delay 1n
        (
            "pwl_repeat_key_mid",
            vec![s("PWL", "0 0.3 1n 1 2n 0.5"), s("R", "1n")],
        ),
        // `R=1n R`: the last one wins (repeat delay 0)
        (
            "pwl_repeat_key_bare_mid",
            vec![s("PWL", "0 0.3 1n 1 2n 0.5"), s("R", "Some(0.0)")],
        ),
    ] {
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
        assert_eq!(codes, ["title_line", "analysis_commands"], "{case}");
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["V1", "r1"], "{case}");
        assert_eq!(params(&net, top, element(&net, top, "v1")), want, "{case}");
    }
    let net = load(repo("golden/cases/pwl_repeat/deck.sp"));
    let top = &net.defs[ROOT.idx()];
    let r = &top.element_params(element(&net, top, "v1"))[1];
    assert_eq!((r.num, r.loc.line), (Some(0.0), 3));
}

#[test]
fn pwl_points_golden() {
    // Golden `pwl_r_param_last`: a parameter named `r` as the last word
    // of a PWL with no parentheses is the repeat flag (repeat delay 0),
    // not the value of `.param r=2`, and HSPICE warns about the last
    // time point, which has no value. Golden `pwl_repeat_key_only`:
    // `R=` at the last time point drops the repeat, with a warning.
    // The values and `R` are kept as written, with a Warning (there
    // was no diagnostic before).
    let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    for (case, code, line, want) in [
        (
            "pwl_r_param_last",
            "pwl_odd_values",
            "deck.sp:4",
            vec![s("PWL", "0 0.3 1n"), s("r", "Some(0.0)")],
        ),
        (
            "pwl_repeat_key_only",
            "pwl_repeat_dropped",
            "deck.sp:3",
            vec![s("PWL", "0 0.3 1n 1"), s("R", "1n")],
        ),
    ] {
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
        assert_eq!(codes, ["title_line", code, "analysis_commands"], "{case}");
        let warn = diags_of(&net, code);
        assert_eq!(warn[0].0, crate::api::Level::Warning, "{case}");
        assert_eq!(warn[0].1, line, "{case}");
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["V1", "r1"], "{case}");
        assert_eq!(params(&net, top, element(&net, top, "v1")), want, "{case}");
    }
    // Golden `pulse_r_noparen`: after PULSE values with no parentheses,
    // `R` is a fifth argument (HSPICE rejects the deck because no
    // parameter `r` exists; the queries report an unknown parameter
    // when they evaluate it).
    let net = load(repo("golden/cases/pulse_r_noparen/deck.sp"));
    let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
    assert_eq!(codes, ["title_line", "analysis_commands"]);
    let top = &net.defs[ROOT.idx()];
    let want = vec![s("PULSE", "0 1 0 1n R")];
    assert_eq!(params(&net, top, element(&net, top, "v1")), want);
    // Forms that were not probed: an odd count in parentheses, on an I
    // line, and `R=` after the last time point give the Warning. No
    // Warning: an even count, `R=` before the last point, an `R` that
    // is not a plain number, and `R` or `TD=` inside the parentheses
    // (not values).
    let deck = write_deck(
        "pwl_points",
        &[(
            "main.sp",
            "* t\nV1 a 0 PWL(0 0 1n)\nI2 a 0 pwl 0 0 1n 1 2n\nV3 a 0 PWL(0 0 1n 1) R=2n\nV4 a 0 PWL(0,0,1n,1)\nV5 a 0 PWL(0 0 1n 1 2n 0) R=1n\nV6 a 0 PWL(0 0 1n 1) R='x'\nV7 a 0 PWL(0 0 1n 1 R TD=1n)\nV8 a 0 PWL(0 0 1n '2 * 3') R\n",
        )],
    );
    let net = load(deck.main.clone());
    let at = |code| -> Vec<String> { diags_of(&net, code).into_iter().map(|d| d.1).collect() };
    assert_eq!(
        at("pwl_odd_values"),
        ["main.sp:2", "main.sp:3"],
        "{:#?}",
        net.diags
    );
    assert_eq!(at("pwl_repeat_dropped"), ["main.sp:4"], "{:#?}", net.diags);
    assert_eq!(net.diags.len(), 4, "{:#?}", net.diags);
}

#[test]
fn pwl_values_words() {
    use super::pwl_values;
    assert_eq!(pwl_values("0 0.3 1n 1"), ["0", "0.3", "1n", "1"]);
    assert_eq!(pwl_values("0,0, 1n ,1"), ["0", "0", "1n", "1"]);
    assert_eq!(
        pwl_values("0 '1 + 2' 1n (3)"),
        ["0", "'1 + 2'", "1n", "(3)"]
    );
    assert_eq!(pwl_values("0 0 1n 1 r td=1n"), ["0", "0", "1n", "1"]);
    assert_eq!(pwl_values("0 0 1n 1 TD=1n"), ["0", "0", "1n", "1"]);
    assert_eq!(pwl_values("0 'a=b' 1n"), ["0", "'a=b'", "1n"]);
    assert_eq!(pwl_values("0 'x"), ["0", "'x"]);
    assert!(pwl_values("").is_empty());
}

#[test]
fn end_text_golden() {
    // Golden `end_quoted_arg`, `end_kv_arg`: `.end 'x'` and `.end x=1`
    // end the first deck, as `.end` does. They gave `trailing_text`
    // before, and the second deck's v2 and r2 joined the first.
    for case in ["end_quoted_arg", "end_kv_arg"] {
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            ["title_line", "analysis_commands", "after_end"],
            "{case}"
        );
        assert_eq!(diags_of(&net, "after_end")[0].1, "deck.sp:5", "{case}");
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["v1", "r1"], "{case}");
    }
}

#[test]
fn braces_output_golden() {
    // Golden `brace_print`, `brace_probe`, `brace_probe_quoted`: HSPICE
    // ignores a `.print` or `.probe` output with `{...}`, with a
    // warning, and runs the deck: the Warning `braces_output`, no
    // Error (`.probe v('{a}')` gave the Error `braces` before, and
    // `.probe v({a})` only the Info). Golden `brace_meas`: HSPICE
    // aborts on `{...}` in a `.meas`: the Error `braces` (only the
    // Info before).
    for (case, code, level) in [
        ("brace_print", "braces_output", crate::api::Level::Warning),
        ("brace_probe", "braces_output", crate::api::Level::Warning),
        (
            "brace_probe_quoted",
            "braces_output",
            crate::api::Level::Warning,
        ),
        ("brace_meas", "braces", crate::api::Level::Error),
    ] {
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let found = diags_of(&net, code);
        assert_eq!(found.len(), 1, "{case}: {:#?}", net.diags);
        assert_eq!(
            (found[0].0, found[0].1.as_str()),
            (level, "deck.sp:6"),
            "{case}"
        );
        let errors = net
            .diags
            .iter()
            .filter(|d| d.level == crate::api::Level::Error)
            .count();
        let want = usize::from(level == crate::api::Level::Error);
        assert_eq!(errors, want, "{case}: {:#?}", net.diags);
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["v1", "r1"], "{case}");
    }
    // Every analysis command but `.print` and `.probe` fails on a brace,
    // also where its tokens read (not probed except `.temp`, `.meas`).
    let deck = write_deck(
        "braces_commands",
        &[(
            "main.sp",
            "* t\n.tran 1n {a}\n.dc v1 0 '{a}' 1\n.measure tran m2 avg v({a})\n.print tran v(a) $ {a}\n.probe v({a})\n.print\n+ v('{a}')\nR1 a b 1\n",
        )],
    );
    let net = load(deck.main.clone());
    let at = |code| -> Vec<String> { diags_of(&net, code).into_iter().map(|d| d.1).collect() };
    assert_eq!(
        at("braces"),
        ["main.sp:2", "main.sp:3", "main.sp:4"],
        "{:#?}",
        net.diags
    );
    assert_eq!(
        at("braces_output"),
        ["main.sp:6", "main.sp:7"],
        "{:#?}",
        net.diags
    );
}

#[test]
fn cl_tc_positional_golden() {
    // Golden `cl_tc_positional`: the numbers after a C or L value are
    // TC1, then TC2, with or without a model name; for L the model name
    // comes after the value.
    let net = load(repo("golden/cases/cl_tc_positional/deck.sp"));
    assert!(
        net.diags
            .iter()
            .all(|d| d.level != crate::api::Level::Error),
        "{:#?}",
        net.diags
    );
    let top = &net.defs[ROOT.idx()];
    let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    // the value as `c` or `l`, then `tc1` and `tc2` as written
    let tcs = |key: &str, v: &str, n: usize| {
        let mut p = vec![s(key, v)];
        p.extend([s("tc1", "0.004"), s("tc2", "1e-5")].into_iter().take(n));
        p
    };
    let cases = [
        ("c1", Some("cmod"), tcs("c", "1p", 1)),
        ("c2", None, tcs("c", "1p", 1)),
        ("c3", Some("cmod"), tcs("c", "1p", 2)),
        ("c4", None, tcs("c", "1p", 2)),
        ("c5", Some("cmod"), tcs("c", "1p", 1)),
        ("c6", None, tcs("c", "1p", 2)),
        ("c7", None, tcs("c", "1p", 0)),
        ("l1", Some("lmod"), tcs("l", "1n", 1)),
        ("l2", None, tcs("l", "1n", 1)),
        ("l3", Some("lmod"), tcs("l", "1n", 2)),
        ("l4", None, tcs("l", "1n", 2)),
        ("l5", Some("lmod"), tcs("l", "1n", 1)),
        ("l6", None, tcs("l", "1n", 2)),
        ("l7", None, tcs("l", "1n", 0)),
    ];
    for (name, model, want) in cases {
        let e = element(&net, top, name);
        assert_eq!(
            e.model.map(|m| net.syms.name(m).to_owned()).as_deref(),
            model,
            "{name}"
        );
        assert_eq!(params(&net, top, e), want, "{name}");
    }
    let c4 = top.element_params(element(&net, top, "c4"));
    assert_eq!((c4[1].num, c4[2].num), (Some(0.004), Some(1e-5)));
}

#[test]
fn src_fn_dc_more_golden() {
    // Golden `src_fn_dc_more`, `src_fn_dc_more_noparen`: HSPICE rejects
    // `DC` after a function, with or without parentheses, as in
    // `src_order_fn_first`. V1 is skipped with one `bad_source` Error
    // that says why.
    for case in ["src_fn_dc_more", "src_fn_dc_more_noparen"] {
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let errs: Vec<_> = net
            .diags
            .iter()
            .filter(|d| d.level == crate::api::Level::Error)
            .collect();
        assert_eq!(errs.len(), 1, "{case}: {:#?}", net.diags);
        assert_eq!(errs[0].code, "bad_source", "{case}");
        assert_eq!(errs[0].loc.map(|l| l.line), Some(3), "{case}");
        assert!(
            errs[0]
                .message
                .contains("a transient function comes before `DC`"),
            "{}",
            errs[0].message
        );
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["r1"], "{case}");
    }
}

#[test]
fn after_end_second_deck_golden() {
    // Golden `after_end_second_deck`: a complete deck after `.end`
    // runs as a second job in HSPICE. The first job has only the
    // first deck (r1 = 1k), and the queries describe the first job.
    let net = load(repo("golden/cases/after_end_second_deck/deck.sp"));
    let top = &net.defs[ROOT.idx()];
    assert_eq!(elem_names(&net, top), ["v1", "r1"]);
    let info = diags_of(&net, "after_end");
    assert_eq!(info.len(), 1, "{:#?}", net.diags);
    assert_eq!(info[0].1, "deck.sp:5");
    assert!(
        info[0].2.starts_with("4 lines after `.end` were not read")
            && info[0].2.contains("second job"),
        "{}",
        info[0].2
    );
    assert!(
        net.diags.iter().all(|d| d.level == crate::api::Level::Info),
        "{:#?}",
        net.diags
    );
}

#[test]
fn end_in_include_golden() {
    // Golden `end_in_include`: HSPICE ignores a `.end` in an included
    // file, warns, and reads the lines after it (r2) and after the
    // `.include` (r3). The loader does the same, with a Warning at the
    // `.end` line.
    let net = load(repo("golden/cases/end_in_include/deck.sp"));
    let top = &net.defs[ROOT.idx()];
    assert_eq!(elem_names(&net, top), ["v1", "r1", "r2", "r3"]);
    let warn = diags_of(&net, "end_in_include");
    assert_eq!(warn.len(), 1, "{:#?}", net.diags);
    assert_eq!(warn[0].0, crate::api::Level::Warning);
    assert_eq!(warn[0].1, "inc.sp:2");
    assert!(warn[0].2.contains("HSPICE ignores"), "{}", warn[0].2);
    let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
    assert_eq!(
        codes,
        ["title_line", "analysis_commands", "end_in_include"],
        "{:#?}",
        net.diags
    );
    // In line order with the other diagnostics of the file, once for a
    // file included twice, and also in a `.subckt` body.
    let deck = write_deck(
        "end_in_include",
        &[
            ("main.sp", "* t\n.inc 'inc.sp'\n.inc 'inc.sp'\n"),
            (
                "inc.sp",
                "R1 a b {x}\n.end\n.subckt s{0} a\n.subckt s a\n.end\n.ends\nR2 a b {y}\n",
            ),
        ],
    );
    let net = load(deck.main.clone());
    let got: Vec<(&str, String)> = net
        .diags
        .iter()
        .filter(|d| d.code != "title_line" && d.code != "duplicate_subckt")
        .map(|d| {
            let l = net.loc(d.loc.unwrap());
            let name = Path::new(&l.file).file_name().unwrap().to_string_lossy();
            (d.code, format!("{name}:{}", l.line))
        })
        .collect();
    let at = |c: &'static str, l: &str| (c, l.to_owned());
    assert_eq!(
        got,
        [
            at("braces", "inc.sp:1"),
            at("end_in_include", "inc.sp:2"),
            at("braces", "inc.sp:3"),
            at("end_in_include", "inc.sp:5"),
            at("braces", "inc.sp:7"),
        ],
        "{:#?}",
        net.diags
    );
}

#[test]
fn block_recovery_stops_at_end() {
    // The skip of a failed `.subckt` does not pass the `.end` of the top
    // file: R3 is not loaded, and the lines after `.end` get the
    // `after_end` Info. With no `.ends` before the `.end`, only the
    // failed line is skipped, so R1 is read at the top level (as for a
    // failed block with no `.ends` at all; golden/NOTES.md "Choices not
    // probed").
    let deck = write_deck(
        "block_end",
        &[(
            "main.sp",
            "* t\n.subckt\nR1 a b 1\n.end\nR2 a b 2\n.ends\nR3 a b 3\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    assert_eq!(elem_names(&net, top), ["R1"]);
    let info = diags_of(&net, "after_end");
    assert_eq!(info.len(), 1, "{:#?}", net.diags);
    assert_eq!(info[0].1, "main.sp:4");
    assert!(
        info[0].2.starts_with("3 lines after `.end`"),
        "{}",
        info[0].2
    );
}

#[test]
fn include_cycle_canonical_paths() {
    // `main.sp` -> `sub/b.sp` -> `../main.sp` is a cycle: the paths
    // are compared after `..` and links are resolved (each spelling
    // was a new file before, until "File name too long", and the
    // statements were read many times).
    let deck = write_deck(
        "cycle_canonical",
        &[("main.sp", "* t\n.include 'sub/b.sp'\nR1 a b 1\n")],
    );
    std::fs::create_dir_all(deck.dir.join("sub")).unwrap();
    std::fs::write(
        deck.dir.join("sub/b.sp"),
        "R2 a b 2\n.include '../main.sp'\n",
    )
    .unwrap();
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    assert_eq!(elem_names(&net, top), ["R2", "R1"]);
    let cycles = diags_of(&net, "include_cycle");
    assert_eq!(cycles.len(), 1, "{:#?}", net.diags);
    assert_eq!(cycles[0].1, "b.sp:2");
    let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
    assert_eq!(codes, ["title_line", "include_cycle"], "{:#?}", net.diags);
    assert_eq!(net.summary().files, 2);
}

#[test]
fn rcl_keyword_first_spelling() {
    // The name of the R/C/L value keeps the first spelling of its
    // keyword on the line, not the spelling of the winning (last) one.
    let deck = write_deck(
        "kw_first",
        &[(
            "main.sp",
            "* t\nR1 a b R=1k r=2k\nC1 a b C='x' c='2*x'\nL1 a b lmod L = 1n\n+ l=2n\n",
        )],
    );
    let net = load(deck.main.clone());
    let top = &net.defs[ROOT.idx()];
    let p = |name: &str| params(&net, top, element(&net, top, name));
    let s = |a: &str, b: &str| vec![(a.to_owned(), b.to_owned())];
    assert_eq!(p("r1"), s("R", "2k"));
    assert_eq!(p("c1"), s("C", "2*x"));
    assert_eq!(p("l1"), s("L", "2n"));
}

#[test]
fn repeated_include() {
    // Golden `inc_twice`, `lib_twice`: the second read of the same
    // definition is a duplicate. The message names both include lines,
    // not the same file:line twice.
    for (case, file, def_at, first, again) in [
        (
            "inc_twice",
            "sub.inc",
            "sub.inc:2",
            "deck.sp:2",
            "deck.sp:4",
        ),
        ("lib_twice", "m.lib", "m.lib:3", "deck.sp:2", "deck.sp:4"),
    ] {
        let dir = repo(&format!("golden/cases/{case}/"));
        let net = load(dir.join("deck.sp"));
        let dups = diags_of(&net, "duplicate_subckt");
        assert_eq!(dups.len(), 1, "{case}: {:#?}", net.diags);
        let (level, at, message) = &dups[0];
        assert_eq!(*level, crate::api::Level::Error);
        assert_eq!(at, def_at, "{case}");
        let message = message.replace(&dir.display().to_string(), "");
        assert!(
                message.contains(&format!(
                    "\"{file}\" is read twice: through the include line at {first} and again through the include line at {again} ("
                )),
                "{case}: {message}"
            );
        assert_eq!(message.matches(def_at).count(), 0, "{case}: {message}");
    }
    // Golden `inc_twice_params`: the same `.model` card read twice.
    let dir = repo("golden/cases/inc_twice_params/");
    let net = load(dir.join("deck.sp"));
    let dups = diags_of(&net, "duplicate_model");
    assert_eq!(dups.len(), 1, "{:#?}", net.diags);
    assert_eq!(dups[0].1, "params.inc:3");
    let message = dups[0].2.replace(&dir.display().to_string(), "");
    assert!(
            message.contains(
                "duplicated model definition `rmod` (\"params.inc\" is read twice: through the include line at deck.sp:2 and again through the include line at deck.sp:4 ("
            ),
            "{message}"
        );
    // A file read only for its `.param` lines: no error (golden
    // `inc_twice_param_only`), and the second read is the last
    // definition.
    let net = load(repo("golden/cases/inc_twice_param_only/deck.sp"));
    assert!(
        net.diags
            .iter()
            .all(|d| d.level != crate::api::Level::Error),
        "{:#?}",
        net.diags
    );
    // Nested: `a.sp` (included twice) includes `sub.sp`. The message
    // names only the include lines where the two reads differ: the two
    // lines of main.sp, not a.sp:2 twice.
    let deck = write_deck(
        "nested_twice",
        &[
            ("main.sp", "* t\n.inc 'a.sp'\n.inc 'a.sp'\n"),
            ("a.sp", "R1 a b 1\n.inc 'sub.sp'\n"),
            ("sub.sp", ".subckt cell a b\nR1 a b 1\n.ends\n"),
        ],
    );
    let net = load(deck.main.clone());
    let dups = diags_of(&net, "duplicate_subckt");
    assert_eq!(dups.len(), 1, "{:#?}", net.diags);
    assert_eq!(dups[0].1, "sub.sp:1");
    let message = dups[0].2.replace(&format!("{}/", deck.dir.display()), "");
    assert!(
            message.contains(
                "\"sub.sp\" is read twice: through the include line at main.sp:2 and again through the include line at main.sp:3 ("
            ),
            "{message}"
        );
    assert!(!message.contains("a.sp:2"), "{message}");
    // Include chains of different lengths: `a.sp` is read by main.sp:2,
    // and again by b.sp:1, which main.sp:3 reads.
    let deck = write_deck(
        "chain_lengths",
        &[
            ("main.sp", "* t\n.inc 'a.sp'\n.inc 'b.sp'\n"),
            ("a.sp", ".subckt cell a b\nR1 a b 1\n.ends\n"),
            ("b.sp", ".inc 'a.sp'\n"),
        ],
    );
    let net = load(deck.main.clone());
    let dups = diags_of(&net, "duplicate_subckt");
    assert_eq!(dups.len(), 1, "{:#?}", net.diags);
    let message = dups[0].2.replace(&format!("{}/", deck.dir.display()), "");
    assert!(
            message.contains(
                "\"a.sp\" is read twice: through the include line at main.sp:2 and again through the include lines at main.sp:3 and b.sp:1 ("
            ),
            "{message}"
        );
    // Two definitions on different lines keep the old message.
    let deck = write_deck(
        "two_defs",
        &[("main.sp", "* t\n.subckt c a\n.ends\n.subckt c a\n.ends\n")],
    );
    let net = load(deck.main.clone());
    let dups = diags_of(&net, "duplicate_subckt");
    assert_eq!(dups.len(), 1);
    assert!(dups[0].2.contains("(the first is at "), "{}", dups[0].2);
    // Two chains that share an include line in the middle: `x.sp` is
    // read by main.sp:2 and by y.sp:1 (main.sp:3); each read of x.sp
    // reads z.sp at x.sp:1. Only the lines where they differ are named.
    let deck = write_deck(
        "shared_middle",
        &[
            ("main.sp", "* t\n.inc 'x.sp'\n.inc 'y.sp'\n"),
            ("x.sp", ".inc 'z.sp'\n"),
            ("y.sp", ".inc 'x.sp'\n"),
            ("z.sp", ".model m r\n"),
        ],
    );
    let net = load(deck.main.clone());
    let dups = diags_of(&net, "duplicate_model");
    assert_eq!(dups.len(), 1, "{:#?}", net.diags);
    let message = dups[0].2.replace(&format!("{}/", deck.dir.display()), "");
    assert!(
            message.contains(
                "\"z.sp\" is read twice: through the include line at main.sp:2 and again through the include lines at main.sp:3 and y.sp:1 ("
            ),
            "{message}"
        );
}

#[test]
fn duplicate_model() {
    // Two `.model` cards with one name in one scope (any letter case),
    // on different lines. Not probed: HSPICE aborts on the same card
    // read twice (golden `inc_twice_params`, in `repeated_include`), and
    // this gets the same error. Other scopes and `.alter` blocks do not
    // count.
    let deck = write_deck(
        "two_models",
        &[(
            "main.sp",
            "* t\n.model m r\n.model M r\n.subckt s a\n.model m r\n.ends\nR1 a b 1\n.alter\n.model m r\n",
        )],
    );
    let net = load(deck.main.clone());
    let dups = diags_of(&net, "duplicate_model");
    assert_eq!(dups.len(), 1, "{:#?}", net.diags);
    assert_eq!(dups[0].0, crate::api::Level::Error);
    assert_eq!(dups[0].1, "main.sp:3");
    let message = dups[0].2.replace(&format!("{}/", deck.dir.display()), "");
    assert_eq!(
        message,
        "duplicated model definition `m` (the first is at main.sp:2): HSPICE rejects this deck. Queries use the first definition"
    );
}
