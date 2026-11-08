//! `resolve_model`: the `.model` card of an element on its instance path,
//! and its parameters evaluated where the card is defined. The choice of
//! the bin is checked against the model selector in `selector_agree.rs`.

mod common;

use common::{load, load_text, write_deck};
use netlist_query::{
    LibOverride, LoadOptions, ModelParams, Netlist, Page, QueryError, Resolved, StepKind,
};

/// A PDK library in the macro-model style: corner sections set global
/// variations and load one core section, whose wrapper subckt passes its
/// size to binned cards inside it, next to two parasitic capacitors and
/// the MOSFET itself.
const PDK: &str = "* pdk\n\
.lib tt\n.param dvth_g=0 du0_g=1.0\n.lib 'pdk.lib' mos_core\n.endl tt\n\
.lib ff\n.param dvth_g=-0.03 du0_g=1.1\n.lib 'pdk.lib' mos_core\n.endl ff\n\
.lib mos_core\n\
.subckt nch_mac d g s b l=100n w=1u nf=1 sa=0.2u\n\
.param dvth_l='5e-9/l'\n\
.param cgd_par='0.2f*w/1u'\n\
.model nch.1 nmos level=54 lmin=20n lmax=200n wmin=0.1u wmax=5u\n\
+ vth0='0.35+dvth_g+dvth_l' u0='0.03*du0_g' tox=1.2n\n\
.model nch.2 nmos level=54 lmin=200n lmax=2u wmin=0.1u wmax=5u\n\
+ vth0='0.40+dvth_g' u0='0.03*du0_g' tox=1.2n\n\
M0 d g s b nch l=l w=w nf=nf sa=sa delvto='dvth_l*0.1'\n\
Cgd d g cgd_par\n\
.ends nch_mac\n\
.endl mos_core\n";

/// A circuit that uses the library, corner `tt`.
const TOP: &str = "* circuit\n.lib 'pdk.lib' tt\n.param wp=2u\n\
.subckt amp in out\nX1 out in 0 0 nch_mac l=60n w=wp nf=2\nX2 out in 0 0 nch_mac l=500n w=1u\n\
.ends amp\nXamp in out amp\nV1 in 0 1\n.end\n";

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * a.abs().max(b.abs())
}

/// The item `name` of `m`.
fn item<'m>(m: &'m ModelParams, name: &str) -> &'m Resolved {
    m.params
        .items
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| panic!("no item `{name}` in {m:#?}"))
}

/// The value of the item `name` of `m`.
fn value(m: &ModelParams, name: &str) -> f64 {
    let p = item(m, name);
    p.value
        .unwrap_or_else(|| panic!("`{name}` has no value: {:?}", p.unresolved))
}

fn all(net: &Netlist, path: &str) -> ModelParams {
    net.resolve_model(path, &[], &Page::default())
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn has(caveats: &[String], part: &str) -> bool {
    caveats.iter().any(|c| c.contains(part))
}

#[test]
fn macro_model_per_instance_and_corner() {
    let deck = write_deck("macro", &[("top.sp", TOP), ("pdk.lib", PDK)]);
    let net = load(&deck);
    let m = all(&net, "xamp.x1.m0");
    let card = m.card.as_ref().expect("a card");
    assert_eq!(
        (card.name.as_str(), card.r#type.as_str()),
        ("nch.1", "nmos")
    );
    assert_eq!(
        (card.def.as_str(), card.scope.as_str()),
        ("nch_mac", "xamp.x1")
    );
    assert!(card.loc.file.ends_with("pdk.lib"));
    assert_eq!((card.params_total, m.params.total), (8, 8));
    assert_eq!((m.kind.as_str(), m.model.as_str()), ("mosfet", "nch"));
    // L=60n from the X line, so dvth_l = 5e-9/60e-9
    assert!(close(value(&m, "vth0"), 0.35 + 5e-9 / 60e-9));
    assert!(close(value(&m, "u0"), 0.03));
    assert!(close(value(&m, "tox"), 1.2e-9));
    assert!(close(value(&m, "lmax"), 200e-9));
    // the chain: the card's item, then the names it uses, where each is
    // defined
    let chain = &item(&m, "vth0").chain;
    assert_eq!(chain[0].kind, StepKind::ModelParam);
    assert_eq!(chain[0].scope, "xamp.x1");
    assert_eq!(chain[0].text, "0.35+dvth_g+dvth_l");
    assert!(
        chain
            .iter()
            .any(|s| s.kind == StepKind::GlobalParam && s.name == "dvth_g" && s.value == Some(0.0))
    );
    assert!(
        chain
            .iter()
            .any(|s| s.kind == StepKind::LocalParam && s.name == "dvth_l" && s.scope == "xamp.x1")
    );
    assert!(
        chain
            .iter()
            .any(|s| s.kind == StepKind::InstanceOverride && s.name == "l" && s.text == "60n")
    );
    // the bin: L, and W per finger (2u / 2)
    let sel = m.selection.as_ref().unwrap();
    assert_eq!((sel.by.as_str(), sel.bins_passed), ("bin", 0));
    assert_eq!((sel.l, sel.w, sel.nf), (Some(60e-9), Some(2e-6), Some(2.0)));
    let ranges: Vec<(&str, f64, f64, f64)> = sel
        .ranges
        .iter()
        .map(|r| (r.name.as_str(), r.value, r.min, r.max))
        .collect();
    assert_eq!(
        ranges,
        [("l", 60e-9, 20e-9, 200e-9), ("w", 1e-6, 0.1e-6, 5e-6)]
    );
    assert!(has(&m.caveats, "evaluated where the card is defined"));
    assert!(has(&m.caveats, "values written on the card"));
    // the other instance: L=500n takes the second bin
    let m2 = all(&net, "xamp.x2.m0");
    let card2 = m2.card.as_ref().unwrap();
    assert_eq!(
        (card2.name.as_str(), card2.scope.as_str()),
        ("nch.2", "xamp.x2")
    );
    assert_eq!(m2.selection.as_ref().unwrap().bins_passed, 1);
    assert!(close(value(&m2, "vth0"), 0.40));
    // another corner, with the same circuit
    let ff = Netlist::load(&LoadOptions {
        lib_override: vec![LibOverride {
            file: "pdk.lib".into(),
            from: "tt".into(),
            to: "ff".into(),
        }],
        ..LoadOptions::new(deck.main.clone())
    })
    .unwrap();
    let f = ff
        .resolve_model("xamp.x1.m0", &["vth0", "u0"], &Page::default())
        .unwrap();
    assert!(close(value(&f, "vth0"), 0.35 - 0.03 + 5e-9 / 60e-9));
    assert!(close(value(&f, "u0"), 0.03 * 1.1));
    let f2 = ff
        .resolve_model("xamp.x2.m0", &["vth0"], &Page::default())
        .unwrap();
    assert!(close(value(&f2, "vth0"), 0.40 - 0.03));
}

#[test]
fn names_and_pages() {
    let deck = write_deck("names", &[("top.sp", TOP), ("pdk.lib", PDK)]);
    let net = load(&deck);
    // the names asked for, in that order, each once, in any letter case;
    // one that the card does not set has no value
    let m = net
        .resolve_model(
            "xamp.x1.m0",
            &["TOX", "nosuch", "vth0", "tox", " "],
            &Page::default(),
        )
        .unwrap();
    let names: Vec<&str> = m.params.items.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["tox", "nosuch", "vth0"]);
    assert_eq!(m.params.total, 3);
    let missing = item(&m, "nosuch");
    assert!(missing.value.is_none() && missing.chain.is_empty());
    assert!(
        missing
            .unresolved
            .as_deref()
            .unwrap()
            .contains("the card does not set `nosuch`")
    );
    // pages of all the items, in card order
    let first = net
        .resolve_model(
            "xamp.x1.m0",
            &[],
            &Page {
                limit: 3,
                cursor: None,
            },
        )
        .unwrap();
    let names: Vec<&str> = first.params.items.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["level", "lmin", "lmax"]);
    assert_eq!(
        (first.params.total, first.params.next_cursor.as_deref()),
        (8, Some("3"))
    );
    let last = net
        .resolve_model(
            "xamp.x1.m0",
            &[],
            &Page {
                limit: 50,
                cursor: Some("6".to_owned()),
            },
        )
        .unwrap();
    let names: Vec<&str> = last.params.items.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["u0", "tox"]);
    assert_eq!(last.params.next_cursor, None);
    // only the total
    let none = net
        .resolve_model(
            "xamp.x1.m0",
            &[],
            &Page {
                limit: 0,
                cursor: None,
            },
        )
        .unwrap();
    assert!(none.params.items.is_empty() && none.params.total == 8);
    assert!(matches!(
        net.resolve_model(
            "xamp.x1.m0",
            &[],
            &Page {
                limit: 5,
                cursor: Some("x".to_owned()),
            }
        ),
        Err(QueryError::BadArgument(_))
    ));
}

#[test]
fn top_level_card_scope() {
    // a top-level card reads the top-level parameters, also from a MOSFET
    // in a subckt that defines the same name (as the bin limits do: decks
    // `q_limits_nested_*`, `q_limits_notop_local`)
    let body = ".param dv=0.01\n\
        .model n nmos level=54 vth0='0.3+dv' k1='kk*2' u0=0.02\n\
        .subckt cell d g dv=0.5 kk=0.25\nM1 d g 0 0 n l=1u w=1u\n.ends cell\n\
        X1 d g cell\nM2 d g 0 0 n l=1u w=1u\nV1 d 0 1\n.end\n";
    for (mode, opt) in [("global", ""), ("local", ".option parhier=local\n")] {
        let (_deck, net) = load_text(&format!("top_{mode}"), &format!("* t\n{opt}{body}"));
        let m = all(&net, "x1.m1");
        let card = m.card.as_ref().unwrap();
        assert_eq!(
            (card.scope.as_str(), card.def.as_str()),
            ("", "(top level)")
        );
        assert_eq!(m.selection.as_ref().unwrap().by, "exact");
        assert!(close(value(&m, "vth0"), 0.31), "{mode}");
        assert!(close(value(&m, "u0"), 0.02));
        // `kk` is defined below the card only: no value where the card is
        // defined, and a caveat that HSPICE can evaluate it in the instance
        let k1 = item(&m, "k1");
        assert!(k1.value.is_none(), "{mode}: {k1:?}");
        assert!(
            k1.unresolved
                .as_deref()
                .unwrap()
                .contains("`kk` is not defined")
        );
        assert!(has(
            &k1.caveats,
            "`kk` is also defined below the card, in `x1`"
        ));
        // under LOCAL the subckt's `dv` is nearer to the MOSFET: a caveat
        let vth0 = item(&m, "vth0");
        assert_eq!(
            has(&vth0.caveats, "`dv` is also defined below the card"),
            mode == "local",
            "{mode}: {vth0:?}"
        );
        // the top-level MOSFET: no level below the card
        let top = all(&net, "m2");
        assert!(close(value(&top, "vth0"), 0.31));
        assert!(item(&top, "k1").caveats.is_empty());
    }
}

#[test]
fn other_elements() {
    let text = "* t\n.param k=2\n\
        .model rmod r tc1r='0.001*k'\n.model dmod d is='1e-14*k' n=1.1\n\
        .model qmod npn bf='100*k'\n.model cmod c\n\
        R1 a 0 rmod 2k\nD1 a 0 dmod\nQ1 c b 0 qmod\nC1 a 0 cmod 1p\n\
        R2 a 0 1k\nV1 a 0 1\nX1 a b cell\n.subckt cell a b\nR3 a b 1k\n.ends\n.end\n";
    let (_deck, net) = load_text("others", text);
    let r = all(&net, "r1");
    assert_eq!(r.card.as_ref().unwrap().r#type, "r");
    assert_eq!(r.kind, "resistor");
    assert!(r.selection.is_none());
    assert!(close(value(&r, "tc1r"), 0.002));
    let d = all(&net, "d1");
    assert!(close(value(&d, "is"), 2e-14) && close(value(&d, "n"), 1.1));
    let q = all(&net, "q1");
    assert_eq!(q.kind, "bjt");
    assert!(close(value(&q, "bf"), 200.0));
    let c = all(&net, "c1");
    assert_eq!(
        (c.card.as_ref().unwrap().params_total, c.params.total),
        (0, 0)
    );
    // the model as the only word after the nodes, with L and W: the wire
    // model, as `element_info` reads the line (golden `res_model`)
    let wire = "* t\n.param rs=50\n.model rw r rsh='rs*2'\nR1 a 0 rw l=10u w=1u\nV1 a 0 1\n.end\n";
    let (_wire_deck, wire_net) = load_text("others_wire", wire);
    assert_eq!(
        wire_net.element_info("r1").unwrap().model.as_deref(),
        Some("rw")
    );
    let w = all(&wire_net, "r1");
    assert_eq!(w.card.as_ref().unwrap().name, "rw");
    assert!(close(value(&w, "rsh"), 100.0));
    // elements with no card
    for path in ["r2", "v1", "x1", "x1.r3"] {
        assert!(
            matches!(
                net.resolve_model(path, &[], &Page::default()),
                Err(QueryError::BadArgument(_))
            ),
            "{path}"
        );
    }
    assert!(matches!(
        net.resolve_model("", &[], &Page::default()),
        Err(QueryError::BadArgument(_))
    ));
    assert!(matches!(
        net.resolve_model("nosuch", &[], &Page::default()),
        Err(QueryError::NotFound { .. })
    ));
}

#[test]
fn card_along_the_instance_path() {
    // `leaf` is a top-level subckt, and `mid` calls it: the R line sees the
    // top-level card, and the instance path has `mid`'s card nearer
    let text = "* t\n.model rmod r tc1r=1\n.subckt leaf a b\nR1 a b rmod 1k\n.ends\n\
        .subckt mid a b\n.model rmod r tc1r=2\nX1 a b leaf\n.ends\n\
        X1 a 0 mid\nX2 a 0 leaf\nV1 a 0 1\n.end\n";
    let (_deck, net) = load_text("dynamic", text);
    let under_mid = all(&net, "x1.x1.r1");
    assert_eq!(under_mid.card.as_ref().unwrap().def, "mid");
    assert!(close(value(&under_mid, "tc1r"), 2.0));
    assert!(has(&under_mid.caveats, "looked up along the instance path"));
    let at_top = all(&net, "x2.r1");
    assert_eq!(at_top.card.as_ref().unwrap().def, "(top level)");
    assert!(close(value(&at_top, "tc1r"), 1.0));
    assert!(!has(&at_top.caveats, "looked up along the instance path"));
}

#[test]
fn no_card() {
    let text = "* t\n\
        .model b.1 nmos level=54 lmin=0 lmax=1e-6 wmin=0 wmax=1e-6\n\
        .model b.2 nmos level=54 lmin=1e-6 lmax=1 wmin=0 wmax=1e-6\n\
        .model h.1 nmos level=54 lmin=0 lmax=1\n\
        .model dio d\n\
        M1 d g 0 0 none l=1u w=1u\nM2 d g 0 0 b l=1u w=4u nf=2\n\
        M3 d g 0 0 h l=1u w=1u\nM4 d g 0 0 dio l=1u w=1u\nM5 d g 0 0 b l=1u w=1u m=0\n\
        V1 d 0 1\n.end\n";
    let (_deck, net) = load_text("no_card", text);
    let why = |path: &str| {
        let m = all(&net, path);
        assert!(
            m.card.is_none() && m.params.items.is_empty(),
            "{path}: {m:?}"
        );
        m.unresolved.unwrap()
    };
    assert!(why("m1").starts_with("no NMOS or PMOS card `none`"));
    assert_eq!(
        why("m2"),
        "none of the 2 bins `b.N` holds L=1e-6, W=4e-6 (the bins compare W/NF = 2e-6)"
    );
    assert_eq!(all(&net, "m2").selection.unwrap().bins_passed, 2);
    assert!(why("m3").contains("must set LMIN, LMAX, WMIN and WMAX"));
    // a diode card does not serve an M line
    assert!(why("m4").starts_with("no NMOS or PMOS card `dio`"));
    assert_eq!(why("m5"), "M=0: HSPICE stops at this size");
}

#[test]
fn caveats_of_a_card() {
    let text = "* t\n.option scalm=2\n\
        .model n nmos level=54 vth0=0.3 vth0=0.4 u0='0.01*agauss(1,0.1,3)'\n\
        M1 d g 0 0 n l=1u w=1u\nV1 d 0 1\n.op\n.alter\n.model n nmos level=54 vth0=0.5\n.end\n";
    let (_deck, net) = load_text("caveats", text);
    let m = all(&net, "m1");
    // a key twice: both items, each with a caveat
    let vth0: Vec<&Resolved> = m.params.items.iter().filter(|p| p.name == "vth0").collect();
    assert_eq!(vth0.len(), 2);
    assert!(
        vth0.iter()
            .all(|p| has(&p.caveats, "the card sets `vth0` 2 times (lines 3, 3)"))
    );
    // a statistical function gives its nominal value
    let u0 = item(&m, "u0");
    assert!(close(u0.value.unwrap(), 0.01));
    assert!(has(&u0.caveats, "nominal value"));
    assert!(has(&m.caveats, "`.option scalm` is 2"));
    assert!(has(&m.caveats, "defines the card `n` again"));
    // an `.if` block in the definition of the card
    let text = "* t\n.param s=1\n.if (s==1)\n.model n nmos level=54 vth0=0.3\n.else\n\
        .model n nmos level=54 vth0=0.7\n.endif\nM1 d g 0 0 n l=1u w=1u\nV1 d 0 1\n.end\n";
    let (_deck, net) = load_text("caveats_if", text);
    let m = all(&net, "m1");
    assert!(has(&m.caveats, "has an `.if` block"));
}

#[test]
fn card_of_a_bare_library() {
    // a library cell as the root (`top`): its defaults are the scope
    let text = "* lib\n.param dvth_g=0.02\n.subckt nch_mac d g s b l=100n w=1u\n\
        .param dvth_l='5e-9/l'\n\
        .model nch.1 nmos level=54 lmin=20n lmax=2u wmin=0.1u wmax=5u vth0='0.35+dvth_g+dvth_l'\n\
        M0 d g s b nch l=l w=w\n.ends nch_mac\n";
    let deck = write_deck("bare", &[("lib.sp", text)]);
    let net = Netlist::load(&LoadOptions {
        top: Some("nch_mac".to_owned()),
        ..LoadOptions::new(deck.main.clone())
    })
    .unwrap();
    let m = all(&net, "m0");
    assert_eq!(m.card.as_ref().unwrap().scope, "(nch_mac)");
    assert!(close(value(&m, "vth0"), 0.35 + 0.02 + 0.05));
}
