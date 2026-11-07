use super::*;

// model selection

/// HSPICE's model selector: the model of that name, or the bin `name.N`
/// with LMIN <= L < LMAX and WMIN <= W < WMAX.
#[tokio::test]
async fn model_bins_by_length_and_width() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nch.1 nmos level=54 lmin=1e-6 lmax=1e-5 wmin=1e-6 wmax=1e-4\n\
         .model nch.2 nmos level=54 lmin=1.8e-7 lmax=1e-6 wmin=1e-6 wmax=1e-4\n\
         .model nch.3 nmos level=54 lmin=1.8e-7 lmax=1e-6 wmin=2.2e-7 wmax=1e-6\n\
         .model nch.4 nmos level=54 lmin=5e-8 lmax=1e-7 wmin=1e-7 wmax=1e-4\n\
         .model nch.5 nmos level=54 lmin=1e-7 lmax=1.8e-7 wmin=1e-7 wmax=1e-4\n\
         .model nmod nmos level=1\n\
         .model nmod.1 nmos level=1\n\
         .param lg=0.18u wn='2*0.5u'\n\
         M1 d g s b nch l=lg w=wn\n\
         M2 d g s b nch l=0.18u w=0.5u\n\
         M3 d g s b nch l=2u w=10u\n\
         M4 d g s b nch l=0.1u w=1u\n\
         M5 d g s b NCH l=1u w=0.5u\n\
         M6 d g s b nmod l=1u w=1u\n\
         M7 d g s b pch l=1u w=1u\n\
         M8 d g s b nch l=lx w=1u\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        pairs(&[
            // W = WMAX of nch.3 = WMIN of nch.2: the upper bin
            ("M1", "nch.2"),
            ("M2", "nch.3"),
            ("M3", "nch.1"),
            // 0.1u is 1.0000000000000001e-7: equal to LMAX of nch.4
            ("M4", "nch.5"),
            (
                "M5",
                "error: none of the 5 bins `NCH.N` holds L=1e-6, W=5e-7"
            ),
            // a model of that name: no selection
            ("M6", "nmod"),
            ("M7", "error: no model `pch`, nor bins `pch.N`"),
            ("M8", "error: L: undefined parameter `lx`"),
        ])
    );
    let s = ast.mosfet_models().remove(0).selection.unwrap();
    assert!(close(s.l, 1.8e-7) && close(s.w, 1e-6) && s.nf == 1.0);
    assert_eq!(ast.select_model("nch", 2e-6, 1e-5).unwrap().name, "nch.1");
}

/// Models defined in a subckt are local to it and hide the ones outside;
/// L, W and the limits of the bins are evaluated where the transistor is,
/// with the parameters of the subckt instances above it (as in the TSMC
/// `nch_mac` macro models, whose limits use mismatch parameters).
#[tokio::test]
async fn model_selection_through_the_hierarchy() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".param dxln=0\n\
         .subckt nch_mac d g s b l=1u w=1u nf=1 mismatch=0\n\
         .param dlmis='1e-9*mismatch'\n\
         MAIN d g s b nch l=l w=w nf=nf\n\
         .model nch.1 nmos lmin='1e-7-(dxln+dlmis)' lmax='5e-7-(dxln+dlmis)' wmin=1e-7 wmax=1e-5\n\
         .model nch.2 nmos lmin='5e-7-(dxln+dlmis)' lmax=1e-5 wmin=1e-7 wmax=1e-5\n\
         .ends\n\
         .subckt inv in out vdd vss wn=1u\n\
         Xn out in vss vss nch_mac l=200n w='wn/2'\n\
         Mp out in vdd vdd nch l=1u w='2*wn'\n\
         .ends\n\
         .model nch.1 nmos lmin=1e-8 lmax=1 wmin=1e-8 wmax=1\n\
         X1 a b c d nch_mac l=200n w=2u\n\
         X2 a b c d nch_mac l=800n w=2u\n\
         X3 a y vdd 0 inv wn=4u\n\
         M9 a b c d nch l=1u w=1u\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    let models = ast.mosfet_models();
    let local = &ast.subckt["nch_mac"].ast.model;
    let global = &ast.model[0];
    let which = |path: &str| {
        let m = models.iter().find(|m| m.path == path).unwrap();
        let s = m.selection.as_ref().unwrap();
        let place = if local.iter().any(|l| std::ptr::eq(l, s.model)) {
            "local"
        } else if std::ptr::eq(global, s.model) {
            "global"
        } else {
            "?"
        };
        (
            s.model.name.to_string(),
            place,
            (s.l * 1e9).round(),
            (s.w * 1e9).round(),
        )
    };
    assert_eq!(
        models.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(),
        ["X1.MAIN", "X2.MAIN", "X3.Xn.MAIN", "X3.Mp", "M9"]
    );
    // (L and W in nm)
    assert_eq!(
        which("X1.MAIN"),
        ("nch.1".to_owned(), "local", 200.0, 2000.0)
    );
    assert_eq!(
        which("X2.MAIN"),
        ("nch.2".to_owned(), "local", 800.0, 2000.0)
    );
    // `w='wn/2'` with `wn=4u` from the instance of `inv` above
    assert_eq!(
        which("X3.Xn.MAIN"),
        ("nch.1".to_owned(), "local", 200.0, 2000.0)
    );
    assert_eq!(
        which("X3.Mp"),
        ("nch.1".to_owned(), "global", 1000.0, 8000.0)
    );
    assert_eq!(which("M9"), ("nch.1".to_owned(), "global", 1000.0, 1000.0));
}

/// `.option parhier`: with GLOBAL (HSPICE's default), the definition closest
/// to the top level wins, also over a value passed on an instance line; with
/// LOCAL, the definition closest to the transistor wins.
#[tokio::test]
async fn parameter_hierarchy_rules() {
    let deck = |option: &str| {
        format!(
            "{option}\n.param w=1u\n.subckt inv a b w=2u\nM1 a b 0 0 nmod w=w l=1u\n.ends\n\
             .model nmod nmos level=1\nX1 a b inv\nX2 a b inv w=3u\n"
        )
    };
    let widths = |ast: &AST<'_>| -> Vec<f64> {
        ast.mosfet_models()
            .iter()
            .map(|m| m.selection.as_ref().unwrap().w)
            .collect()
    };
    let (_dir, parsed, files) = parse_text(&[("top.sp", &deck("* default"))]).await;
    assert_eq!(widths(&files.build(parsed).0), [1e-6, 1e-6]);
    let (_dir, parsed, files) = parse_text(&[("top.sp", &deck(".option parhier=local"))]).await;
    assert_eq!(widths(&files.build(parsed).0), [2e-6, 3e-6]);
}

/// `.option scale` multiplies L and W; `defl`/`defw` give L and W when the
/// instance has none; bins hold the width per finger.
#[tokio::test]
async fn model_selection_options_and_fingers() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".option scale=1e-6 defl=0.5 defw=2\n\
         .model nch.1 nmos wnflag=1 lmin=1e-7 lmax=1e-6 wmin=1e-7 wmax=1e-6\n\
         .model nch.2 nmos wnflag=1 lmin=1e-7 lmax=1e-6 wmin=1e-6 wmax=1e-5\n\
         .model pch.1 nmos lmin=1e-7 lmax=1e-6 wmin=1e-7 wmax=1e-6\n\
         .model pch.2 nmos lmin=1e-7 lmax=1e-6 wmin=1e-6 wmax=1e-5\n\
         M1 d g s b nch l=0.18 w=4 nf=8\n\
         M2 d g s b nch l=0.18 w=4\n\
         M3 d g s b pch l=0.18 w=4 nf=8\n\
         M4 d g s b pch\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        pairs(&[
            ("M1", "nch.1"),
            ("M2", "nch.2"),
            ("M3", "pch.1"),
            ("M4", "pch.2"),
        ])
    );
    let m4 = ast.mosfet_models().remove(3).selection.unwrap();
    assert!(close(m4.l, 5e-7) && close(m4.w, 2e-6));
}

/// The ranges of a bin are evaluated where the model is defined: those of a
/// top-level model with the top-level parameters, even inside a subckt that
/// defines (or is passed) a parameter of the same name; those of a model in a
/// subckt with that subckt instance's. (The top-level `M1` is the first
/// MOSFET HSPICE makes, on purpose: see the known gap in `models.rs`.)
#[tokio::test]
async fn bin_ranges_are_evaluated_where_the_model_is() {
    for option in ["", ".option parhier=local\n"] {
        let text = format!(
            "{option}.param lb=2e-7\n\
             .model nch.1 nmos lmin=1e-8 lmax='lb' wmin=1e-8 wmax=1e-4\n\
             .model nch.2 nmos lmin='lb' lmax=1e-5 wmin=1e-8 wmax=1e-4\n\
             .subckt shadow d lb=1e-6\nM1 d d 0 0 nch w=1u l=5e-7\n.ends\n\
             .subckt inner d\n.param lb=1e-6\nM1 d d 0 0 nch w=1u l=5e-7\n.ends\n\
             .subckt mac d ll=1e-7 dx=0\n\
             .model lm.1 nmos lmin=1e-8 lmax='2e-7-dx' wmin=1e-8 wmax=1e-4\n\
             .model lm.2 nmos lmin='2e-7-dx' lmax=1e-5 wmin=1e-8 wmax=1e-4\n\
             M1 d d 0 0 lm w=1u l=ll\n.ends\n\
             X1 d shadow\nX2 d shadow lb=1e-6\nX3 d inner\n\
             X4 d mac ll=1.95e-7\nX5 d mac ll=1.95e-7 dx=1e-8\nM1 d d 0 0 nch w=1u l=5e-7\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        assert_eq!(
            chosen(&ast),
            pairs(&[
                ("X1.M1", "nch.2"),
                ("X2.M1", "nch.2"),
                ("X3.M1", "nch.2"),
                ("X4.M1", "lm.1"),
                ("X5.M1", "lm.2"),
                ("M1", "nch.2"),
            ]),
            "{option}"
        );
        let usage = ast.model_usage();
        let mut counts: Vec<_> = usage
            .models
            .iter()
            .map(|(m, n)| (m.name.to_string(), *n))
            .collect();
        counts.sort();
        assert_eq!(
            counts,
            [
                ("lm.1".to_owned(), 1),
                ("lm.2".to_owned(), 1),
                ("nch.2".to_owned(), 4)
            ]
        );
    }
}

/// A value passed on an instance line is computed in the new instance: with
/// LOCAL, the subckt's own `k=100` and `.param q=200` are used; with GLOBAL,
/// the definitions closest to the top, the parent's `k=7` and `q=8`. (HSPICE,
/// probe case 4.)
#[tokio::test]
async fn passed_values_are_computed_in_the_new_instance() {
    for (option, expected) in [("", 15e-6), (".option parhier=local\n", 300e-6)] {
        let text = format!(
            "{option}.model nmod nmos level=1\n\
             .subckt child d s=0 k=100\n.param q=200\nM1 d d 0 0 nmod w='s*1u' l=1u\n.ends\n\
             .subckt parent d k=7 q=8\nX1 d child s='k+q'\n.ends\n\
             X1 d parent\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let w = ast.mosfet_models().remove(0).selection.unwrap().w;
        assert!(close(w, expected), "{option}: {w}");
    }
}

/// Parameters, as measured on HSPICE (deck `q_override_*` of
/// `scripts/hspice_kit`). The text of a value passed is evaluated in the new
/// instance; its own name is looked up only above that instance; with GLOBAL,
/// a definition closer to the top wins over any value passed.
#[tokio::test]
async fn parameters_as_measured_on_hspice() {
    let deck = ".param w=4 q=2\n.model nv nmos level=54\n\
        .subckt cc d g s w=2 l=1\nM1 d g s s nv w='w*1u' l='l*1u'\n.ends cc\n\
        .subckt cb d g s\n.param v=2 w=2\nM1 d g s s nv w='w*1u' l='v*1u'\n.ends cb\n\
        .subckt cu d g s\nM1 d g s s nv w='q*1u' l=1u\n.ends cu\n\
        .subckt cm d g s w=3\nX1 d g s cc w='w*2'\n.ends cm\n\
        X1 d g 0 cc l='w*3'\nX2 d g 0 cc w=5 l='w*3'\nX3 d g 0 cc w='w*3'\n\
        X4 d g 0 cc w='w*2' l='w*3'\nX5 d g 0 cu q=5\nX6 d g 0 cb w=7\n\
        X7 d g 0 cb w='w*3'\nX8 d g 0 cm\n";
    let local = [
        ("X1.M1", 6.0, 2.0),
        ("X2.M1", 15.0, 5.0),
        ("X3.M1", 1.0, 12.0),
        ("X4.M1", 24.0, 8.0),
        ("X5.M1", 1.0, 5.0),
        ("X6.M1", 2.0, 7.0),
        ("X7.M1", 2.0, 12.0),
        ("X8.X1.M1", 1.0, 6.0),
    ];
    let global = [
        ("X1.M1", 12.0, 4.0),
        ("X2.M1", 12.0, 4.0),
        ("X3.M1", 1.0, 4.0),
        ("X4.M1", 12.0, 4.0),
        ("X5.M1", 1.0, 2.0),
        ("X6.M1", 2.0, 4.0),
        ("X7.M1", 2.0, 4.0),
        ("X8.X1.M1", 1.0, 4.0),
    ];
    let text = format!(".option parhier=local\n{deck}");
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    assert_sizes(&files.build(parsed).0, &local, "local");
    let (_dir, parsed, files) = parse_text(&[("top.sp", deck)]).await;
    assert_sizes(&files.build(parsed).0, &global, "global");

    // the probe (scripts/parhier_probe.sp), cases 1 to 6: W in um, by parhier
    let probe = ".param p=1\n.model nv nmos level=54\n\
        .subckt show d v=0\nM1 d d 0 0 nv w='v*1u' l=1u\n.ends show\n\
        .subckt c1 d p=3\nX1 d show v=p\n.ends c1\n\
        .subckt c2 d p=3\nX1 d show v=p\n.ends c2\n\
        .subckt c3leaf d\nX1 d show v=p\n.ends c3leaf\n\
        .subckt c3 d p=4\nX1 d c3leaf\n.ends c3\n\
        .subckt c4child d s=0 k=100\n.param q=200\nX1 d show v='s'\n.ends c4child\n\
        .subckt c4 d k=7 q=8\nX1 d c4child s='k+q'\n.ends c4\n\
        .subckt c5 d p=3\n.param p=5\nX1 d show v=p\n.ends c5\n\
        .subckt c6 d a=1 b='a*2'\nX1 d show v=b\n.ends c6\n\
        Xc1 d c1 p=2\nXc2 d c2\nXc3 d c3 p=5\nXc4 d c4\nXc5 d c5 p=2\nXc6 d c6 a=6\n";
    let row = |path, w| (path, 1.0, w);
    for (option, values) in [
        ("", [1.0, 1.0, 1.0, 15.0, 1.0, 12.0]),
        (".option parhier=local\n", [2.0, 3.0, 5.0, 300.0, 2.0, 12.0]),
    ] {
        let text = format!("{option}{probe}");
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        assert_sizes(
            &files.build(parsed).0,
            &[
                row("Xc1.X1.M1", values[0]),
                row("Xc2.X1.M1", values[1]),
                row("Xc3.X1.X1.M1", values[2]),
                row("Xc4.X1.X1.M1", values[3]),
                row("Xc5.X1.M1", values[4]),
                row("Xc6.X1.M1", values[5]),
            ],
            option,
        );
    }

    // a value passed that reads its own name, with nothing above the new
    // instance: HSPICE stops ("cannot redefine parameter to itself")
    for option in ["", ".option parhier=local\n"] {
        let text = format!(
            "{option}.model nv nmos level=54\n\
             .subckt cc d g s w=2\nM1 d g s s nv w='w*1u' l=1u\n.ends cc\n\
             X1 d g 0 cc w='w*3'\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        assert_eq!(
            chosen(&ast),
            pairs(&[("X1.M1", "error: W: parameter `w` depends on itself")]),
            "{option}"
        );
    }
}

/// Functions defined with `.param f(x)=...`, as measured on HSPICE (deck
/// `q_func_*` of `scripts/hspice_kit`): the definition closest to the top
/// with GLOBAL, the nearest with LOCAL; the names and the calls in a body are
/// resolved where the function is defined, not where it is called.
#[tokio::test]
async fn functions_as_measured_on_hspice() {
    let deck = ".param f(x)='x*2' k=2 h(x)='x*k'\n.model nv nmos level=54\n\
        .subckt cf d g s\n.param f(x)='x*3'\nM1 d g s s nv l=1u w='f(1)*1u'\n.ends cf\n\
        .subckt ck d g s k=7\nM1 d g s s nv l=1u w='h(1)*1u'\n.ends ck\n\
        .subckt cl d g s\n.param g(x)='x*kk' kk=2\nX1 d g s cn\n.ends cl\n\
        .subckt cn d g s kk=9\nM1 d g s s nv l=1u w='g(1)*1u'\n.ends cn\n\
        X1 d g 0 cf\nX2 d g 0 ck\nX3 d g 0 cl\n";
    for (option, x1) in [("", 2.0), (".option parhier=local\n", 3.0)] {
        let text = format!("{option}{deck}");
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        assert_sizes(
            &files.build(parsed).0,
            &[
                ("X1.M1", 1.0, x1),
                ("X2.M1", 1.0, 2.0),
                ("X3.X1.M1", 1.0, 2.0),
            ],
            option,
        );
    }
    // a function called in a body: where the outer function is defined
    // (`func_nested_local`)
    let text = ".option parhier=local\n.param f(x)='x*2' g(x)='f(x)'\n.model nv nmos level=54\n\
        .subckt cf d\n.param f(x)='x*3'\nM1 d d 0 0 nv l=1u w='g(1)*1u'\n.ends cf\nX1 d cf\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_sizes(&files.build(parsed).0, &[("X1.M1", 1.0, 2.0)], "nested");
}

/// Models and subckts are looked up along the instance path, not along the
/// nesting of the text: a definition in the subckt of any instance above is
/// seen, the nearest first (HSPICE, decks `q_model_dynamic` and
/// `q_subckt_dynamic`). One line can then bind to different definitions.
#[tokio::test]
async fn lookup_along_the_instance_path() {
    let limits = "lmin=1e-10 lmax=1 wmin=1e-10 wmax=1";
    let text = format!(
        ".model dy.1 nmos {limits}\n.subckt leaf d g s\nM1 d g s s dy l=1u w=1u\n.ends leaf\n\
         .subckt host d g s\n.model dy.2 nmos {limits}\nX1 d g s leaf\n.ends host\n\
         X1 d g 0 leaf\nX2 d g 0 host\n"
    );
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        pairs(&[("X1.M1", "dy.1"), ("X2.X1.M1", "dy.2")])
    );
    let usage = ast.model_usage();
    let mut counts: Vec<_> = usage
        .models
        .iter()
        .map(|(m, n)| (m.name.to_string(), *n))
        .collect();
    counts.sort();
    assert_eq!(counts, [("dy.1".to_owned(), 1), ("dy.2".to_owned(), 1)]);

    let text = ".model nv nmos level=54\n\
        .subckt cell d g s\nM1 d g s s nv l=1u w=1u\n.ends cell\n\
        .subckt leaf d g s\nX1 d g s cell\n.ends leaf\n\
        .subckt host d g s\n.subckt cell d g s\nM1 d g s s nv l=1u w=2u\n.ends cell\n\
        X1 d g s leaf\n.ends host\nX1 d g 0 leaf\nX2 d g 0 host\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_sizes(
        &files.build(parsed).0,
        &[("X1.X1.M1", 1.0, 1.0), ("X2.X1.X1.M1", 1.0, 2.0)],
        "subckt",
    );
}

/// `name.suffix` is a bin of `name`, whatever the suffix; a name with a dot
/// cannot be named on an instance line; a name with two dots is of no use
/// (HSPICE, decks `q_mc_as_bin`, `q_nonnumeric`, `q_1a_as_bin`,
/// `q_bin_direct_in`, `q_two_dots_*`).
#[tokio::test]
async fn bin_names_as_measured_on_hspice() {
    let limits = "lmin=1e-10 lmax=1 wmin=1e-10 wmax=1";
    let text = format!(
        ".model mc.mc nmos {limits}\n.model a1.1a nmos {limits}\n\
         .model nm.1 nmos lmin=1e-7 lmax=1e-6 wmin=1e-10 wmax=1\n.model nm.mc nmos {limits}\n\
         .model nm.1a nmos {limits}\n.model e.4 nmos {limits}\n.model n.ch.1 nmos {limits}\n\
         M1 d g 0 0 mc l=1u w=1u\nM2 d g 0 0 a1 l=1u w=1u\nM3 d g 0 0 nm l=5u w=1u\n\
         M4 d g 0 0 e.4 l=5u w=5u\nM5 d g 0 0 n l=1u w=1u\nM6 d g 0 0 n.ch l=1u w=1u\n\
         M7 d g 0 0 n.ch.1 l=1u w=1u\nM8 d g 0 0 mc.mc l=1u w=1u\n"
    );
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        pairs(&[
            ("M1", "mc.mc"),
            ("M2", "a1.1a"),
            // the first that holds it, in netlist order
            ("M3", "nm.mc"),
            ("M4", "error: no model `e.4`, nor bins `e.4.N`"),
            ("M5", "error: no model `n`, nor bins `n.N`"),
            ("M6", "error: no model `n.ch`, nor bins `n.ch.N`"),
            ("M7", "error: no model `n.ch.1`, nor bins `n.ch.1.N`"),
            ("M8", "error: no model `mc.mc`, nor bins `mc.mc.N`"),
        ])
    );
}

/// A bin needs its four limits when it is used, not otherwise (HSPICE, decks
/// `q_incomplete_*`, `q_mc_unused`, `q_bin_nolimits_unused`): "you must set
/// all four parameters (wmin,wmax,lmin,lmax)".
#[tokio::test]
async fn bins_need_their_four_limits() {
    let limits = "lmin=1e-10 lmax=1 wmin=1e-10 wmax=1";
    for cards in [
        format!(".model nm.1 nmos {limits}\n.model nm.mc nmos level=54\n"),
        format!(".model nm.mc nmos level=54\n.model nm.1 nmos {limits}\n"),
    ] {
        let text = format!(
            "{cards}.model ok nmos level=54\nM1 d g 0 0 nm l=1u w=1u\nM2 d g 0 0 ok l=1u w=1u\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        assert_eq!(
            chosen(&ast),
            pairs(&[
                (
                    "M1",
                    "error: model `nm.mc` has a `.` in its name: it must set LMIN, LMAX, WMIN and WMAX \
                     (at level 72: LMIN, LMAX, and NFINMIN and NFINMAX, or WSHEETMIN and WSHEETMAX)"
                ),
                ("M2", "ok"),
            ])
        );
    }
}

/// At each level from the transistor's up: a model of that name, else the
/// first bin that holds L and W; a level whose bins do not hold them hides
/// nothing (HSPICE, decks `q_localhide`, `q_local_exact`,
/// `q_localbin_globalexact`, `q_localbin_out_globalexact`, `q_exact_and_bin`,
/// `q_order`).
#[tokio::test]
async fn levels_of_models_as_measured_on_hspice() {
    let wide = "lmin=1e-10 lmax=1 wmin=1e-10 wmax=1";
    let small = "lmin=1e-10 lmax=5e-7 wmin=1e-10 wmax=1";
    let cases = [
        // (global cards, local cards, whether the global one is chosen)
        (
            format!(".model loc.1 nmos {wide}\n"),
            format!(".model loc.1 nmos {small}\n"),
            true,
        ),
        (
            format!(".model loc.1 nmos {small}\n"),
            format!(".model loc.1 nmos {wide}\n"),
            false,
        ),
        (
            format!(".model loc.1 nmos {wide}\n"),
            ".model loc nmos level=54\n".to_owned(),
            false,
        ),
        (
            ".model loc nmos level=54\n".to_owned(),
            format!(".model loc.1 nmos {wide}\n"),
            false,
        ),
        (
            ".model loc nmos level=54\n".to_owned(),
            format!(".model loc.1 nmos {small}\n"),
            true,
        ),
    ];
    for (global, local, global_chosen) in cases {
        let text = format!(
            "{global}.subckt h d g s\n{local}M1 d g s s loc l=1u w=1u\n.ends h\nX1 d g 0 h\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let m = ast.mosfet_models().remove(0);
        let s = m
            .selection
            .as_ref()
            .unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(
            std::ptr::eq(s.model, &ast.model[0]),
            global_chosen,
            "{text}"
        );
    }
    // no level holds them: an error that counts the bins seen
    let text = format!(
        ".model loc.1 nmos {small}\n.subckt h d g s\n.model loc.1 nmos {small}\n\
         M1 d g s s loc l=1u w=1u\n.ends h\nX1 d g 0 h\n"
    );
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        pairs(&[(
            "X1.M1",
            "error: none of the 2 bins `loc.N` holds L=1e-6, W=1e-6"
        )])
    );
    // a model and a bin of that name: the model; two bins hold: the first in
    // the text, not the lowest number
    let text = format!(
        ".model eb nmos level=54\n.model eb.1 nmos {wide}\n\
         .model ov.2 nmos {wide}\n.model ov.1 nmos {wide}\n\
         M1 d g 0 0 eb l=1u w=1u\nM2 d g 0 0 ov l=1u w=1u\n"
    );
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(chosen(&ast), pairs(&[("M1", "eb"), ("M2", "ov.2")]));
}

/// A value at most about 2e-15 (relative) below a limit counts as on it, at
/// every size of limit (HSPICE, decks `q_edge_tol_*`): 3e-15 below it does
/// not, 1e-15 below it does.
#[tokio::test]
async fn bin_edges_as_measured_on_hspice() {
    for limit in [1e-8, 1e-7, 1e-6, 1e-5, 1e-4_f64] {
        let (lo, hi) = (limit / 10.0, limit * 10.0);
        let text = format!(
            ".model tl.1 nmos lmin={lo:e} lmax={limit:e} wmin=1e-10 wmax=1\n\
             .model tl.2 nmos lmin={limit:e} lmax={hi:e} wmin=1e-10 wmax=1\n\
             .model tw.1 nmos lmin=1e-10 lmax=1 wmin={lo:e} wmax={limit:e}\n\
             .model tw.2 nmos lmin=1e-10 lmax=1 wmin={limit:e} wmax={hi:e}\n\
             M1 d g 0 0 tl l='{limit:e}*(1-3e-15)' w=1u\n\
             M2 d g 0 0 tl l='{limit:e}*(1-1e-15)' w=1u\n\
             M3 d g 0 0 tl l={limit:e} w=1u\n\
             M4 d g 0 0 tl l='{limit:e}*(1+1e-15)' w=1u\n\
             M5 d g 0 0 tw l=1u w='{limit:e}*(1-1e-12)'\n\
             M6 d g 0 0 tw l=1u w='{limit:e}*(1-1e-15)'\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        assert_eq!(
            chosen(&ast),
            pairs(&[
                ("M1", "tl.1"),
                ("M2", "tl.2"),
                ("M3", "tl.2"),
                ("M4", "tl.2"),
                ("M5", "tw.1"),
                ("M6", "tw.2"),
            ]),
            "{limit}"
        );
    }
}

/// The numbers after the model name are L, then W; a keyword after them wins
/// (`mos_positional`, `mos_kw_and_positional`, deck `q_positional`).
#[tokio::test]
async fn positional_length_and_width() {
    let text = ".model a nmos level=54\n.param ll=7u\n\
        M1 d g 0 0 a 2u 1u\nM2 d g 0 a 2u 1u\nM3 d g 0 0 a 2u 1u l=3u\nM4 d g 0 0 a 2u w=5u\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_sizes(
        &ast,
        &[
            ("M1", 2.0, 1.0),
            ("M2", 2.0, 1.0),
            ("M3", 3.0, 1.0),
            ("M4", 2.0, 5.0),
        ],
        "positional",
    );
    // printed back as numbers after the model name
    let printed = ast.to_string();
    assert!(
        printed.contains("\nM1 d g 0 0 a 2.0000000e-6 1.0000000e-6\n"),
        "{printed}"
    );
    let (_dir2, parsed2, files2) = parse_text(&[("top.sp", &printed)]).await;
    let (ast2, _) = files2.build(parsed2);
    assert_sizes(
        &ast2,
        &[
            ("M1", 2.0, 1.0),
            ("M2", 2.0, 1.0),
            ("M3", 3.0, 1.0),
            ("M4", 2.0, 5.0),
        ],
        "printed",
    );
}

/// The model name of a MOSFET line is the first word after the fourth node that
/// names a model, else the fourth word if it names one (`M1 d g 0 a pl pw`: 3
/// nodes, then the names of L and W, deck `q_positional_names_3`). A line with no
/// such word keeps the last word that is not a number, so the error names it.
/// HSPICE reads the other lines below as the test does (decks
/// `q_two_model_words*`, and probes on P-2019.06).
#[tokio::test]
async fn model_name_is_the_first_word_that_names_a_model() {
    let text = ".model a nmos level=54\n.param pl=2u pw=1u\n\
        M1 d g 0 a pl pw\nM2 d g 0 0 a pl pw\nM3 d g 0 a pl 1u\nM4 d g s e p b t a\n\
        M5 d g 0 0 nosuch pl pw\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let models = ast.mosfet_models();
    for (name, l, w) in [
        ("M1", 2.0, 1.0),
        ("M2", 2.0, 1.0),
        ("M3", 2.0, 1.0),
        ("M4", 100.0, 100.0),
    ] {
        let m = models.iter().find(|m| m.path == name).unwrap();
        assert_eq!(m.mname, "a", "{name}");
        let s = m.selection.as_ref().expect(name);
        assert!(close(s.l * 1e6, l) && close(s.w * 1e6, w), "{name}: {s:?}");
    }
    let m = models.iter().find(|m| m.path == "M5").unwrap();
    assert!(matches!(&m.selection, Err(crate::models::SelectError::NoModel(n)) if n == "pw"));

    // two words name a model
    let text = ".model a nmos level=54\n.model b nmos level=54\n.model n nmos level=54\n\
        .param b=2u a=1u p=2u\n\
        M1 d g 0 0 a b w=1u\nM2 d g 0 a b w=1u\nM3 d g 0 0 x a b w=1u\nM4 d g 0 b a w=1u\n\
        M5 d g 0 0 a a b\nM6 d g 0 0 b b a\nM7 d g n n n l=1u w=1u\nM8 d g 0 n a p w=1u\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let models = ast.mosfet_models();
    for (name, model, l, w) in [
        ("M1", "a", 2.0, 1.0),
        ("M2", "b", 100.0, 1.0),
        ("M3", "a", 2.0, 1.0),
        ("M4", "a", 100.0, 1.0),
        ("M5", "a", 1.0, 2.0),
        ("M6", "b", 2.0, 1.0),
        ("M7", "n", 1.0, 1.0),
        ("M8", "a", 2.0, 1.0),
    ] {
        let m = models.iter().find(|m| m.path == name).unwrap();
        assert_eq!(m.mname, model, "{name}");
        let s = m.selection.as_ref().expect(name);
        assert!(close(s.l * 1e6, l) && close(s.w * 1e6, w), "{name}: {s:?}");
    }
}

/// A MOSFET line with many words, as measured on HSPICE: any number of values
/// after the model name (decks `q_positional_names_4`, `q_positional_names_8`,
/// `q_positional_numbers_9`), up to 7 nodes before it (`q_nodes_7`). HSPICE
/// stops at 8 nodes (`q_nodes_8`).
#[tokio::test]
async fn mosfet_line_with_many_words() {
    let text = ".model a nmos level=54\n.param pl=2u pw=1u pa=1p pb=1p\n\
        M1 d g 0 0 a pl pw pa pb\nM2 d g 0 a pl pw pa pb\n\
        M3 d g 0 0 a pl pw pa pb pa pb pa pb\nM4 d g 0 0 a 2u 1u 1p 1p 1u 1u 1 1 1\n\
        M5 d g 0 0 a pl pw pa pb w=3u\nM6 d g 0 0 x y z a pl pw\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    let expected = [
        ("M1", 2.0, 1.0),
        ("M2", 2.0, 1.0),
        ("M3", 2.0, 1.0),
        ("M4", 2.0, 1.0),
        // (`pw` is an expression, so it wins over the plain `w=3u`: deck `mb_*`)
        ("M5", 2.0, 1.0),
        ("M6", 2.0, 1.0),
    ];
    assert_sizes(&ast, &expected, "many words");
    // printed, and read back
    let printed = ast.to_string();
    let (_dir2, parsed2, files2) = parse_text(&[("top.sp", &printed)]).await;
    assert_eq!(diagnostics(&parsed2), [], "{printed}");
    assert_sizes(&files2.build(parsed2).0, &expected, "printed");

    let text = ".model a nmos level=54\nM1 d g 0 0 x y z w a l=1u w=1u\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let too_many = |e: &crate::models::SelectError| matches!(e, crate::models::SelectError::TooManyNodes { name, nodes: 8 } if name == "a");
    let models = ast.mosfet_models();
    assert!(models[0].selection.as_ref().is_err_and(too_many));
    let usage = ast.model_usage();
    assert_eq!(usage.errors.len(), 1);
    assert!(too_many(&usage.errors[0].0));
}

/// `OFF` on a MOSFET line takes a place and gives no value. HSPICE's L and W
/// (probes on P-2019.06).
#[tokio::test]
async fn off_takes_a_place() {
    let lines = [
        ("M1 d g 0 0 a OFF", 100.0, 100.0),
        ("M1 d g 0 0 a OFF 1u", 100.0, 1.0),
        ("M1 d g 0 0 a OFF 1u 2u", 100.0, 1.0),
        ("M1 d g 0 0 a 1u OFF", 1.0, 100.0),
        ("M1 d g 0 0 a 1u OFF 2u", 1.0, 100.0),
        ("M1 d g 0 0 a 1u 2u OFF", 1.0, 2.0),
        ("M1 d g 0 0 a 1u 2u OFF 3p", 1.0, 2.0),
        ("M1 d g 0 0 a OFF OFF 1u 2u", 100.0, 100.0),
        ("M1 d g 0 0 a OFF l=2u w=1u", 2.0, 1.0),
        ("M1 d g 0 0 a OFF 1u 2u l=3u", 3.0, 1.0),
        ("M1 d g 0 0 a OFF 1u 2u w=3u", 100.0, 3.0),
        ("M1 d g 0 0 a OFF pl pw", 100.0, 1.0),
        ("M1 d g 0 0 a pl OFF pw", 1.0, 100.0),
        ("M1 d g 0 0 a pl pw OFF", 1.0, 2.0),
        ("M1 d g 0 0 a off '1u' '2u'", 100.0, 1.0),
        ("M1 d g 0 a OFF 1u 2u", 100.0, 1.0),
    ];
    for (line, l, w) in lines {
        let text = format!(".model a nmos level=54\n.param pl=1u pw=2u\n{line}\n");
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        assert_eq!(diagnostics(&parsed), [], "{line}");
        let (ast, _) = files.build(parsed);
        assert_sizes(&ast, &[("M1", l, w)], line);
        // printed, and read back
        let printed = ast.to_string();
        let (_dir2, parsed2, files2) = parse_text(&[("top.sp", &printed)]).await;
        assert_eq!(diagnostics(&parsed2), [], "{printed}");
        assert_sizes(&files2.build(parsed2).0, &[("M1", l, w)], &printed);
    }
}

/// After four nodes, HSPICE wants the model name: it stops on a word that starts
/// with a digit there ("Model name is missing", decks `q_digit_word_5`). A sixth
/// or seventh node can start with a digit.
#[tokio::test]
async fn mosfet_digit_first_fifth_word() {
    let digit = |e: &crate::models::SelectError| matches!(e, crate::models::SelectError::DigitFifthWord { word, .. } if word == "5a");
    let text = ".model a nmos level=54\nM1 d g 0 0 5a a l=1u w=1u\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert!(ast.mosfet_models()[0].selection.as_ref().is_err_and(digit));
    assert!(digit(&ast.model_usage().errors[0].0));
    let text = ".model a nmos level=54\nM1 d g 0 0 x 6 a l=1u w=1u\n\
        M2 d g 0 0 x y 7a a l=1u w=1u\nM3 d g 0 5 a l=1u w=1u\nM4 d g 5 a l=1u w=1u\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let expected: Vec<_> = ["M1", "M2", "M3", "M4"].map(|n| (n, 1.0, 1.0)).into();
    assert_sizes(&files.build(parsed).0, &expected, "digit-first nodes");
}

/// A value in parentheses on a `.param` or `X` line can go on on a `+` line. On a
/// MOSFET line, the parentheses only separate words: see
/// `parentheses_after_a_key_on_a_mosfet_line`. A comma can follow `.ends` and its
/// name (probes on P-2019.06).
#[tokio::test]
async fn group_on_plus_lines_and_ends_commas() {
    let text = ".model a nmos level=54\n.subckt c d g w=1u\nM1 d g 0 0 a l=1u w=w\n.ends\n\
        .param p=(1u+\n+ 2u)\n.param q=max(1u,\n+ 2u)\nM1 d g 0 0 a l=p w=q\n\
        M2 d g 0 0 a l='(1u\n+ +2u)' w=1u\nX1 d g c w=(1u\n+ +2u)\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let expected = [("M1", 3.0, 2.0), ("M2", 3.0, 1.0), ("X1.M1", 1.0, 3.0)];
    assert_sizes(&files.build(parsed).0, &expected, "groups on + lines");
    for ends in [
        ".ends,",
        ".ends,c1",
        ".ends, c1",
        ".ends ,c1",
        ".ends , c1",
        ".ends c1,",
        ".ends (c1)",
        ".ends (c1),",
        ".ends,$ c",
        ".ends c1 $ c",
        ".ends $ c",
        ".ends * c",
        ".ends c1 * c",
        ".ends\n+ c1",
        ".ends c1",
    ] {
        let text = format!(
            ".model a nmos level=54\n.subckt c1 d g\nM1 d g 0 0 a l=1u w=1u\n{ends}\nX1 d g c1\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        assert_eq!(diagnostics(&parsed), [], "{ends}");
        assert_sizes(&files.build(parsed).0, &[("X1.M1", 1.0, 1.0)], ends);
    }
}

/// Values after the model name may also be quoted expressions and names of
/// parameters (decks `q_positional_expr`, `q_positional_names`,
/// `q_positional_mix`). Words after `key=value` are still an error.
#[tokio::test]
async fn positional_expressions() {
    let text = ".model a nmos level=54\n.param pl=2u pw=1u\n\
        M1 d g 0 0 a 'pl' 'pw'\nM2 d g 0 0 a pl pw\nM3 d g 0 0 a 2u pw\nM4 d g 0 0 a pl 1u\n\
        M5 d g 0 a 'pl' 1u\nM6 d g 0 0 a pl\nM7 d g 0 0 a 'pl' w=3u\nM8 d g 0 0 a\n+ 'pl*2'\n+ 'pw*2'\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let expected = [
        ("M1", 2.0, 1.0),
        ("M2", 2.0, 1.0),
        ("M3", 2.0, 1.0),
        ("M4", 2.0, 1.0),
        ("M5", 2.0, 1.0),
        ("M6", 2.0, 100.0),
        ("M7", 2.0, 3.0),
        ("M8", 4.0, 2.0),
    ];
    assert_sizes(&ast, &expected, "positional expressions");
    // printed, and read back
    let printed = ast.to_string();
    let (_dir2, parsed2, files2) = parse_text(&[("top.sp", &printed)]).await;
    assert_eq!(diagnostics(&parsed2), [], "{printed}");
    let (ast2, _) = files2.build(parsed2);
    assert_sizes(&ast2, &expected, "printed");

    // (a value after a keyword takes the next place: deck `rk_key_quoted_name`)
    let (_dir, parsed, _) = parse_text(&[("top.sp", "M1 d g 0 0 a l=1u 'pl'\n")]).await;
    assert_eq!(diagnostics(&parsed), []);
}

/// Texts that lead back to themselves, as measured on HSPICE (decks
/// `q_cycle_*` of the kit): the L and W of `X1.M1`, in um.
#[tokio::test]
async fn loops_as_measured_on_hspice() {
    let ac = "M1 d g s s nv l='a*1u' w='c*1u'\n.ends s\n";
    let kc = "M1 d g s s nv l='k*1u' w='c*1u'\n.ends s\n";
    let ak = "M1 d g s s nv l='a*1u' w='k*1u'\n.ends s\n";
    for (top, def, line, l, w) in [
        // a value passed, and a `.param` that read each other: as a whole, above
        (
            "a=1 c=2",
            format!(".subckt s d g s\n.param c='a*3'\n{ac}"),
            "a='c'",
            2.0,
            6.0,
        ),
        (
            "a=1 c=2",
            format!(".subckt s d g s a=5\n.param c='a*3'\n{ac}"),
            "a='c'",
            2.0,
            6.0,
        ),
        (
            "c=2",
            format!(".subckt s d g s\n.param c='a*3'\n{ac}"),
            "a='c'",
            2.0,
            6.0,
        ),
        (
            "a=1 b=2 c=3",
            ".subckt s d g s\n.param b='c*2' c='a*3'\nM1 d g s s nv l='a*1u' w='b*1u'\n.ends s\n"
                .to_owned(),
            "a='b'",
            2.0,
            12.0,
        ),
        // ... with the order of the line
        (
            "a=1 c=2",
            format!(".subckt s d g s\n{ac}"),
            "a='c' c='a'",
            2.0,
            2.0,
        ),
        (
            "a=1 c=2",
            format!(".subckt s d g s\n{ac}"),
            "c='a' a='c'",
            1.0,
            1.0,
        ),
        (
            "a=1 b=2 c=3",
            format!(".subckt s d g s\n{ac}"),
            "a='b' b='c' c='a'",
            2.0,
            2.0,
        ),
        // ... and all of its names
        (
            "a=1 c=2 k=2",
            format!(".subckt s d g s k=3\n.param c='a*3'\n{ak}"),
            "a='c*k'",
            4.0,
            3.0,
        ),
        (
            "a=1 c=2 k=2",
            format!(".subckt s d g s\n.param c='a*3'\n{ak}"),
            "k=7 a='c*k'",
            4.0,
            7.0,
        ),
        // a text that reads its own name: that name above, the others as usual
        (
            "a=1 c=2 k=2",
            format!(".subckt s d g s k=3\n{kc}"),
            "c='c*k'",
            3.0,
            6.0,
        ),
        (
            "a=1 c=2 k=2",
            format!(".subckt s d g s\n.param k=3 c='c*k'\n{kc}"),
            "",
            3.0,
            6.0,
        ),
        (
            "a=1 c=2 k=2",
            format!(".subckt s d g s k=3 c='c*k'\n{kc}"),
            "",
            3.0,
            6.0,
        ),
        (
            "a=1 c=2 k=2",
            format!(".subckt s d g s\n.param c='c*k'\n{kc}"),
            "k=7",
            7.0,
            14.0,
        ),
        (
            "a=1 c=2",
            format!(".subckt s d g s\n.param c='c*2'\n{ac}"),
            "",
            1.0,
            4.0,
        ),
        (
            "a=1 c=2",
            format!(".subckt s d g s c='c*2'\n{ac}"),
            "",
            1.0,
            4.0,
        ),
        // `c` reads a value later on the line, which reads its own name
        (
            "a=4",
            ".subckt s d g s c=1 a=2\nM1 d g s s nv l='a*1u' w='c*1u'\n.ends s\n".to_owned(),
            "c='a*3' a='a*2'",
            8.0,
            24.0,
        ),
    ] {
        let text = format!(
            ".option parhier=local\n.param {top}\n.model nv nmos level=54\n{def}X1 d g 0 s {line}\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        assert_sizes(&ast, &[("X1.M1", l, w)], &text);
    }
    // the loops that stop HSPICE
    for text in [
        // two `.param` that read each other, also with both names above
        ".param a=1 c=2\n.model nv nmos level=54\n.subckt s d g s\n.param a='c*2' c='a*3'\n\
         M1 d g s s nv l='a*1u' w='c*1u'\n.ends s\nX1 d g 0 s\n",
        // at the top level
        ".param a='c*2' c='a*3'\n.model nv nmos level=54\nM1 d g 0 0 nv l='a*1u' w='c*1u'\n",
        // with no name above the new instance
        ".model nv nmos level=54\n.subckt s d g s\n.param c='a*3'\n\
         M1 d g s s nv l='a*1u' w='c*1u'\n.ends s\nX1 d g 0 s a='c'\n",
    ] {
        let text = format!(".option parhier=local\n{text}");
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let models = ast.mosfet_models();
        assert!(models.iter().all(|m| m.selection.is_err()), "{text}");
    }
    // with parhier=global, the top level wins: no loop
    let text = ".param a=1 c=2\n.model nv nmos level=54\n.subckt s d g s\n.param c='a*3'\n\
        M1 d g s s nv l='a*1u' w='c*1u'\n.ends s\nX1 d g 0 s a='c'\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    let (ast, _) = files.build(parsed);
    assert_sizes(&ast, &[("X1.M1", 1.0, 2.0)], text);
}

/// Whether bins compare the width per finger (`W/NF`): `WNFLAG` on the
/// instance line, else `.option wnflag`, else they do. HSPICE ignores a
/// `WNFLAG` in a model (decks `q_nf_default`, `q_wnflag*`).
#[tokio::test]
async fn width_per_finger_flags() {
    let models = ".model a.1 nmos lmin=1e-8 lmax=1e-4 wmin=1e-8 wmax=1e-6\n\
        .model a.2 nmos lmin=1e-8 lmax=1e-4 wmin=1e-6 wmax=1e-3\n\
        .model b.1 nmos wnflag=1 lmin=1e-8 lmax=1e-4 wmin=1e-8 wmax=1e-6\n\
        .model b.2 nmos wnflag=1 lmin=1e-8 lmax=1e-4 wmin=1e-6 wmax=1e-3\n\
        .model c.1 nmos wnflag=0 lmin=1e-8 lmax=1e-4 wmin=1e-8 wmax=1e-6\n\
        .model c.2 nmos wnflag=0 lmin=1e-8 lmax=1e-4 wmin=1e-6 wmax=1e-3\n";
    // 4u over 8 fingers: 0.5u per finger; M=8 is not fingers
    let devices = "M1 d g s b a w=4u l=1u nf=8\nM2 d g s b b w=4u l=1u nf=8\n\
        M3 d g s b b w=4u l=1u nf=8 wnflag=0\nM4 d g s b a w=4u l=1u nf=8 wnflag=1\n\
        M5 d g s b c w=4u l=1u nf=8\nM6 d g s b a w=4u l=1u m=8\nM7 d g s b a w=4u l=1u nf=8 m=2\n";
    for (option, expected) in [
        ("", ["a.1", "b.1", "b.2", "a.1", "c.1", "a.2", "a.1"]),
        (
            ".option wnflag=1\n",
            ["a.1", "b.1", "b.2", "a.1", "c.1", "a.2", "a.1"],
        ),
        (
            ".option wnflag=0\n",
            ["a.2", "b.2", "b.2", "a.1", "c.2", "a.2", "a.2"],
        ),
    ] {
        let text = format!("{option}{models}{devices}");
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let got: Vec<_> = chosen(&ast).into_iter().map(|(_, m)| m).collect();
        assert_eq!(got, expected, "{option}");
    }
}

/// Counts through a binary tree whose every instance gets a different value
/// (each leaf's `p` is its path in binary): beyond a trial, the instances
/// of a definition that never repeat are not kept (they used to fill the
/// memory); the counts stay those of the listing.
#[tokio::test]
async fn model_usage_of_instances_that_never_repeat() {
    let depth = 13;
    let mut deck = String::from(
        ".option parhier=local\n\
         .model nw.1 nmos lmin=1e-10 lmax=1 wmin=1e-10 wmax=2.5e-7\n\
         .model nw.2 nmos lmin=1e-10 lmax=1 wmin=2.5e-7 wmax=4.5e-7\n\
         .model nw.3 nmos lmin=1e-10 lmax=1 wmin=4.5e-7 wmax=1e-5\n\
         .subckt b0 d p=0\nM0 d d 0 0 nw w='(1+p-7*floor(p/7))*100n' l=1u\n.ends\n",
    );
    for k in 1..=depth {
        deck += &format!(
            ".subckt b{k} d p=0\nX0 d b{} p='2*p'\nX1 d b{} p='2*p+1'\n.ends\n",
            k - 1,
            k - 1
        );
    }
    deck += &format!("X0 d b{depth} p=0\n");
    let (_dir, parsed, files) = parse_text(&[("top.sp", &deck)]).await;
    let (ast, _) = files.build(parsed);
    let mut listed = HashMap::<String, u64>::new();
    for m in ast.mosfet_models() {
        *listed
            .entry(m.selection.unwrap().model.name.to_string())
            .or_default() += 1;
    }
    let usage = ast.model_usage();
    let counted: HashMap<String, u64> = usage
        .models
        .iter()
        .map(|(m, n)| (m.name.to_string(), *n))
        .collect();
    assert_eq!(counted, listed);
    // p mod 7 = 0, 1 (W=100n, 200n): nw.1; 2, 3: nw.2; 4, 5, 6: nw.3
    let n = 1u64 << depth;
    let of = |r: u64| (n - 1 - r) / 7 + 1;
    assert_eq!(counted["nw.1"], of(0) + of(1));
    assert_eq!(counted["nw.2"], of(2) + of(3));
    assert_eq!(counted["nw.3"], of(4) + of(5) + of(6));
}

/// A subckt that instantiates itself does not make the walk loop forever, and
/// the line gets an error row: HSPICE stops there (decks `x_self`, `x_mutual`).
#[tokio::test]
async fn model_selection_stops_at_recursive_subckts() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nmod nmos level=1\n.subckt a x\nM1 x x 0 0 nmod\nX1 x a\n.ends\nX0 n a\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    let recursive = format!(
        "error: {}",
        crate::models::SelectError::Recursive("a".to_owned())
    );
    assert_eq!(
        chosen(&ast),
        pairs(&[("X0.M1", "nmod"), ("X0.X1", &recursive)])
    );
}

/// HSPICE stops at an `X` line with no definition on its instance path (golden
/// cases `hier_top_nested_only`, `hier_unres_in_subckt`, `hier_model_callee`).
/// The MOSFETs below that line are not known, so both walks report the line,
/// and a total counts only the MOSFETs found.
#[tokio::test]
async fn x_line_with_no_definition_is_an_error() {
    use crate::models::SelectError;
    let no_def = |name: &str| format!("error: {}", SelectError::NoSubckt(name.to_owned()));
    let usage_errors = |u: &crate::models::ModelUsage<'_, '_>| -> Vec<(String, u64, String)> {
        u.errors
            .iter()
            .map(|(e, n, path)| (format!("error: {e}"), *n, path.clone()))
            .collect()
    };

    // a top-level line, and a name that only another subckt nests
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nmod nmos level=1\n.subckt outer a b\n.subckt only a b\nM1 a b 0 0 nmod\n.ends\n\
         Xo a b only\n.ends\nX1 1 0 outer\nX2 2 0 only\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        vec![
            ("X1.Xo.M1".to_owned(), "nmod".to_owned()),
            ("X2".to_owned(), no_def("only")),
        ]
    );
    let u = ast.model_usage();
    assert_eq!(u.mosfets, 1);
    assert_eq!(usage_errors(&u), [(no_def("only"), 1, "X2".to_owned())]);

    // one line that resolves on one instance path and not on another
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nmod nmos level=1\n.subckt t a b\nXn a b only\n.ends\n.subckt outer a b\n\
         .subckt only a b\nM1 a b 0 0 nmod\n.ends\nXt a b t\n.ends\nX1 1 0 outer\nX2 2 0 t\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        vec![
            ("X1.Xt.Xn.M1".to_owned(), "nmod".to_owned()),
            ("X2.Xn".to_owned(), no_def("only")),
        ]
    );
    let u = ast.model_usage();
    assert_eq!(u.mosfets, 1);
    assert_eq!(usage_errors(&u), [(no_def("only"), 1, "X2.Xn".to_owned())]);
    // the same line from an instance of `t` alone
    let of = ast.mosfet_models_of("t").unwrap();
    assert_eq!(of.len(), 1);
    assert_eq!(of[0].path, "Xn");
    let u = ast.model_usage_of("t").unwrap();
    assert_eq!(u.mosfets, 0);
    assert_eq!(usage_errors(&u), [(no_def("only"), 1, "Xn".to_owned())]);

    // the name of a model is not a subckt
    let (_dir, parsed, files) = parse_text(&[("top.sp", ".model rmod r\nX1 1 0 rmod\n")]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(chosen(&ast), vec![("X1".to_owned(), no_def("rmod"))]);
    assert_eq!(ast.model_usage().mosfets, 0);

    // counted through the hierarchy, as the MOSFETs are
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nmod nmos level=1\n.subckt leaf a\nM1 a a 0 0 nmod\nXgone a gone\n.ends\n\
         .subckt mid a\nX1 a leaf\nX2 a leaf\n.ends\nX1 n mid\nX2 n mid\nX3 n mid\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    let u = ast.model_usage();
    assert_eq!(u.mosfets, 6);
    assert_eq!(u.models.len(), 1);
    assert_eq!(u.models[0].1, 6);
    assert_eq!(
        usage_errors(&u),
        [(no_def("gone"), 6, "X1.X1.Xgone".to_owned())]
    );
    let listed = chosen(&ast);
    assert_eq!(listed.len(), 12);
    assert_eq!(
        listed.iter().filter(|(_, m)| *m == no_def("gone")).count(),
        6
    );

    // a definition in a plain `.prot` block is a definition (golden case
    // `prot_subckt`), and a name is found in any case
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nmod nmos level=1\n.prot\n.subckt secret a b\nM1 a b 0 0 nmod\n.ends\n.unprot\n\
         X1 1 0 SECRET\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    assert_eq!(chosen(&ast), pairs(&[("X1.M1", "nmod")]));
    assert!(ast.model_usage().errors.is_empty());
}

/// HSPICE: the netlist ends at the first `.alter`; what follows describes
/// other runs (other corners, other parameters). It used to be parsed into
/// the same netlist, so every corner's `.lib` and `.param` were merged (the
/// last one winning). It is now kept verbatim, not parsed.
#[tokio::test]
async fn alter_blocks_are_not_merged() {
    let (_dir, parsed, files) = parse_text(&[
        (
            "top.sp",
            ".lib 'corners.l' tt\n.param w=1u\nM1 d g s b nmod w=w l=1u\n\
             .alter ss corner\n.del lib 'corners.l' tt\n.lib 'corners.l' ss\n.param w=2u\n\
             .alter\n.param w=3u\n.end\n",
        ),
        (
            "corners.l",
            ".lib tt\n.param dx=0\n.model nmod nmos level=1\n.endl tt\n\
             .lib ss\n.param dx=1n\n.model nmod nmos level=1\n.endl ss\n",
        ),
    ])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    // one corner, one model, one value of each parameter
    assert_eq!(ast.model.len(), 1);
    let scope = ast.scope();
    assert_eq!((scope.get("w"), scope.get("dx")), (Ok(1e-6), Ok(0.0)));
    // the `.alter` blocks are kept, verbatim, and printed back
    let alter = ast
        .unknown
        .iter()
        .find(|u| u.cmd.eq_ignore_ascii_case("alter"))
        .unwrap();
    assert!(alter.raw.starts_with("ss corner\n.del lib") && alter.raw.ends_with(".end"));
    let printed = ast.to_string();
    let (_dir2, parsed2, files2) = parse_deck(&[("top.sp", &printed)]).await;
    assert_eq!(files2.build(parsed2).0.to_string(), printed);
}

/// A hierarchy 250 subckts deep (the walk stops at `MAX_HIERARCHY_DEPTH`)
/// does not overflow the stack, and parameters pass all the way down.
#[tokio::test]
async fn model_selection_in_a_deep_hierarchy() {
    let mut text = String::from(".option parhier=local\n.model nmod nmos level=1\n");
    for i in 0..250 {
        writeln!(
            text,
            ".subckt s{i} a w=1u\nX{i} a s{} w='w+1n'\n.ends",
            i + 1
        )
        .unwrap();
    }
    text.push_str(".subckt s250 a w=1u\nM1 a a 0 0 nmod w=w l=1u\n.ends\nX a s0\n");
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    let mosfets = ast.mosfet_models();
    assert_eq!(mosfets.len(), 1);
    let s = mosfets[0].selection.as_ref().unwrap();
    assert!(close(s.w, 1e-6 + 250e-9), "{}", s.w);
    assert_eq!(
        crate::Subckt::append(
            "s0",
            |_| false,
            &mut Vec::new(),
            &mut vec![&ast.subckt],
            &mut Vec::new(),
            &mut Vec::new()
        ),
        Ok(())
    );
}

/// `model_usage` (counted through the hierarchy, each distinct subckt
/// instance once) gives the same counts as listing every MOSFET.
#[tokio::test]
async fn model_usage_counts_like_the_list() {
    /// MOSFETs; how many use each model; how many have each error
    type Tally = (u64, Vec<(String, u64)>, Vec<(String, u64)>);
    fn tally(ast: &AST<'_>) -> Tally {
        let mut models = HashMap::<String, u64>::new();
        let mut errors = HashMap::<String, u64>::new();
        let list = ast.mosfet_models();
        for m in &list {
            match &m.selection {
                Ok(s) => *models.entry(s.model.name.to_string()).or_default() += 1,
                Err(e) => *errors.entry(e.to_string()).or_default() += 1,
            }
        }
        let mut models: Vec<_> = models.into_iter().collect();
        let mut errors: Vec<_> = errors.into_iter().collect();
        models.sort();
        errors.sort();
        (list.len() as u64, models, errors)
    }
    fn usage(ast: &AST<'_>) -> Tally {
        let u = ast.model_usage();
        let mut models = HashMap::<String, u64>::new();
        for (m, n) in &u.models {
            *models.entry(m.name.to_string()).or_default() += n;
        }
        let mut models: Vec<_> = models.into_iter().collect();
        let mut errors: Vec<_> = u
            .errors
            .iter()
            .map(|(e, n, _)| (e.to_string(), *n))
            .collect();
        models.sort();
        errors.sort();
        (u.mosfets, models, errors)
    }
    // the same subckt under different parameters, levels without parameters,
    // local models, errors, and repeated instances
    let deck = ".param dxln=0 lg=30n\n\
        .model nch.1 nmos lmin=1e-8 lmax=1e-7 wmin=1e-8 wmax=1e-6\n\
        .model nch.2 nmos lmin=1e-8 lmax=1e-7 wmin=1e-6 wmax=1e-4\n\
        .model nch.3 nmos lmin=1e-7 lmax=1e-4 wmin=1e-8 wmax=1e-4\n\
        .subckt cell a b wn=0.5u l=lg\n\
        M1 a b 0 0 nch w=wn l=l\nM2 a b 0 0 nch w='4*wn' l=l\nM3 a b 0 0 pch w=wn l=l\n.ends\n\
        .subckt blk a b\n\
        X1 a b cell\nX2 a b cell wn=1u\nX3 a b cell l=200n\nX4 a b cell\nM9 a b 0 0 nch w=2u l=lx\n.ends\n\
        .subckt tile a b sz=1u\n\
        Xb1 a b blk\nXb2 a b blk\nXc a b cell wn=sz\n.ends\n\
        .subckt mac a b l=1u w=1u\n\
        MAIN a b 0 0 nch l=l w=w\n\
        .model nch.1 nmos lmin=1e-8 lmax=1 wmin=1e-8 wmax=1\n.ends\n\
        Xt1 a b tile\nXt2 a b tile sz=2u\nXt3 a b tile\nXm1 a b mac\nXm2 a b mac l=50n\nM0 a b 0 0 nch w=1u l=1u\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", deck)]).await;
    let (ast, _) = files.build(parsed);
    let expected = tally(&ast);
    assert_eq!(expected.0, 3 * (2 * (4 * 3 + 1) + 3) + 2 + 1);
    assert_eq!(usage(&ast), expected);
    // and the models of one subckt
    let u = ast.model_usage_of("tile").unwrap();
    assert_eq!(u.mosfets, 2 * (4 * 3 + 1) + 3);
    assert!(ast.model_usage_of("nothing").is_none());
    // with `.option parhier=local`, `.option scale`, and fingers
    for deck in [
        ".option parhier=local\n.param w=1u\n.subckt inv a b w=2u\nM1 a b 0 0 nmod w=w l=1u\n.ends\n\
         .model nmod nmos level=1\nX1 a b inv\nX2 a b inv w=3u\n",
        ".option scale=1e-6\n.model nch.1 nmos wnflag=1 lmin=1e-7 lmax=1e-6 wmin=1e-7 wmax=1e-6\n\
         .model nch.2 nmos wnflag=1 lmin=1e-7 lmax=1e-6 wmin=1e-6 wmax=1e-5\n\
         M1 d g s b nch l=0.18 w=4 nf=8\nM2 d g s b nch l=0.18 w=4\n",
    ] {
        let (_dir, parsed, files) = parse_text(&[("top.sp", deck)]).await;
        let (ast, _) = files.build(parsed);
        assert_eq!(usage(&ast), tally(&ast));
    }
    // the counts of a subckt instance are kept for the parameters its
    // contents read: values passed that nothing reads (`id`), values read
    // levels below (`k`, `lsel`), a definition that hides another (`wq`),
    // the ranges of local bins (`dx`), and functions; with each `parhier`
    let models = ".model nch.1 nmos lmin=1e-8 lmax=1e-7 wmin=1e-8 wmax=1e-6\n\
        .model nch.2 nmos lmin=1e-8 lmax=1e-7 wmin=1e-6 wmax=1e-4\n\
        .model nch.3 nmos lmin=1e-7 lmax=1e-4 wmin=1e-8 wmax=1e-6\n\
        .model nch.4 nmos lmin=1e-7 lmax=1e-4 wmin=1e-6 wmax=1e-4\n";
    let decks = [
        ".param lsel=50n k=0.3u\n\
         .subckt leaf a b wl=1u\nM1 a b 0 0 nch w=wl l=lsel\n.ends\n\
         .subckt mid a b id=0\nX1 a b leaf\nX2 a b leaf wl='2*k'\nX3 a b leaf id=id\n.ends\n\
         .subckt top a b id=0 k=1u\nXm1 a b mid id=1\nXm2 a b mid id=2\nXm3 a b mid\n.ends\n\
         Xt1 a b top id=1\nXt2 a b top id=2 k=0.6u\nXt3 a b top k=0.6u\nXt4 a b top id=4\n\
         Xt5 a b mid\nXt6 a b leaf lsel=200n\n",
        ".param wq=0.5u\n\
         .subckt cellq a b\nM1 a b 0 0 nch w=wq l=50n\n.ends\n\
         .subckt blkq a b wq=3u id=0\nX1 a b cellq\nX2 a b cellq wq=0.2u\n.ends\n\
         .subckt topq a b\nXa a b blkq id=1\nXb a b blkq id=2\nXc a b blkq wq=2u\n.ends\n\
         X1 a b blkq\nX2 a b cellq\nX3 a b topq\nX4 a b topq\n",
        ".subckt mac2 a b l=1u w=1u dx=0 tag=0\n\
         .model nmac.1 nmos lmin='1e-7-dx' lmax='5e-7-dx' wmin=1e-8 wmax=1\n\
         .model nmac.2 nmos lmin='5e-7-dx' lmax=1 wmin=1e-8 wmax=1\n\
         M1 a b 0 0 nmac l=l w=w\n.ends\n\
         .subckt pair a b tag=0 dx=0\nX1 a b mac2 l=4.95e-7 tag=tag\nX2 a b mac2 l=4.95e-7 dx=dx\n.ends\n\
         X1 a b mac2 l=4.95e-7 tag=1\nX2 a b mac2 l=4.95e-7 dx=1e-8 tag=2\n\
         X3 a b mac2 l=4.95e-7 dx=1e-8 tag=3\nX4 a b pair tag=4\nX5 a b pair dx=1e-8\nX6 a b pair tag=6\n",
        ".param f(x)='2*x' wf=0.4u\n\
         .subckt fcell a b wf=0.3u\nM1 a b 0 0 nch w='f(wf)' l=50n\n.ends\n\
         .subckt fblk a b id=0\nX1 a b fcell\nX2 a b fcell wf=0.6u\n.ends\n\
         X1 a b fblk id=1\nX2 a b fblk id=2\nX3 a b fcell\n",
        // a subckt that calls a function is counted for its context: what
        // its parent passes (`k`) changes it
        ".param f(x)='2*x'\n\
         .subckt fc a b\nM1 a b 0 0 nch w='f(k)*1u' l=50n\n.ends\n\
         .subckt fp a b k=0.1\nX1 a b fc\nX2 a b fc\n.ends\n\
         X1 a b fp k=0.1\nX2 a b fp k=0.3\nX3 a b fp k=0.6\nX4 a b fp\n",
        ".temp 85\n.subckt tcell a b\nM1 a b 0 0 nch w='temper*1e-8' l=50n\n.ends\n\
         .subckt tblk a b temper=1\nX1 a b tcell\n.ends\n\
         X1 a b tcell\nX2 a b tblk\nX3 a b tblk temper=200\n",
        // a value passed is computed in the new instance: `k` of its subckt,
        // of the line, or from above, by parhier
        ".param k=0.3u\n\
         .subckt oc a b k=0.2u wl=0\nM1 a b 0 0 nch w=wl l=50n\n.ends\n\
         .subckt op a b\nX1 a b oc wl='k'\nX2 a b oc wl='2*k'\nX3 a b oc k=3u wl='k'\n.ends\n\
         X1 a b op\nX2 a b oc wl='k'\nX3 a b oc k=3u wl='k'\nX4 a b op\n",
        // the model of a line depends on the instances above: bins of an
        // instance above, without parameters and with them
        ".subckt leaf a b\nM1 a b 0 0 nch w=1u l=50n\n.ends\n\
         .subckt host a b\n.model nch.1 nmos lmin=1e-8 lmax=1 wmin=1e-8 wmax=1\nX1 a b leaf\n.ends\n\
         X1 a b leaf\nX2 a b host\nX3 a b leaf\nX4 a b host\n",
        ".subckt leafm a b\nM1 a b 0 0 lo w=1u l=5e-7\n.ends\n\
         .subckt hostm a b lb=1e-6\n\
         .model lo.1 nmos lmin=1e-8 lmax='lb' wmin=1e-8 wmax=1\n\
         .model lo.2 nmos lmin='lb' lmax=1 wmin=1e-8 wmax=1\n\
         X1 a b leafm\nX2 a b leafm\n.ends\n\
         X1 a b hostm\nX2 a b hostm lb=1e-7\nX3 a b hostm\n",
        // so does the subckt of a line; here each host also passes a value
        ".subckt sel a b\nX1 a b cell\n.ends\n\
         .subckt cell a b wc=1u\nM1 a b 0 0 nch w=wc l=50n\n.ends\n\
         .subckt host2 a b wc=0.5u\n.subckt cell a b\nM1 a b 0 0 nch w=wc l=200n\n.ends\n\
         X1 a b sel\n.ends\n\
         X1 a b sel\nX2 a b host2\nX3 a b host2 wc=2u\nX4 a b sel\n",
    ];
    for deck in decks {
        for option in ["", ".option parhier=local\n"] {
            let text = format!("{option}{models}{deck}");
            let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
            let (ast, _) = files.build(parsed);
            let expected = tally(&ast);
            // (with GLOBAL, a top-level definition can leave one model)
            assert!(expected.1.len() > 1 || option.is_empty(), "{text}");
            assert_eq!(usage(&ast), expected, "{text}");
        }
    }
}

/// Under `parhier=local`, a text that reads its own name reads that name at
/// the level above, so the counts of the summary depend on the value above.
/// HSPICE on this deck: x1.m1 has L=1u and nch.1, x2.x1.m1 has L=4u and nch.2.
/// `deck_rows` also checks the counts of the summary against the list.
#[tokio::test]
async fn usage_follows_a_text_that_reads_its_own_name() {
    let models = ".option parhier=local\n\
        .model nch.1 nmos level=54 lmin=0 lmax=1.5u wmin=0 wmax=1\n\
        .model nch.2 nmos level=54 lmin=1.5u lmax=100u wmin=0 wmax=1\n.param w=0.5u\n";
    let tail = "M1 d d 0 0 nch l=w w=1u\n.ends\n\
        .subckt big d w=2u\nX1 d leaf\n.ends\nX1 d leaf\nX2 d big\n";
    let text = format!("* t\n{models}.subckt leaf d\n.param w='w*2'\n{tail}");
    let rows: Vec<_> = deck_rows(&text)
        .await
        .into_iter()
        .map(|r| (r.0, r.1, r.2))
        .collect();
    let want = [
        ("X1.M1".to_owned(), "nch.1".to_owned(), 1.0),
        ("X2.X1.M1".to_owned(), "nch.2".to_owned(), 4.0),
    ];
    assert_eq!(rows, want, "{text}");
    // the same read in a default of the subckt, and with both forms on one
    // level (the counts must agree with the list)
    for leaf in [
        ".subckt leaf d w='w*2'\n",
        ".subckt leaf d w=1u\n.param w='w*2'\n",
    ] {
        deck_rows(&format!("* t\n{models}{leaf}{tail}")).await;
    }
}
