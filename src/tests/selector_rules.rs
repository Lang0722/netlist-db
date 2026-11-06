use super::*;

// Rules of the selector, as measured on HSPICE (`golden/NOTES.md`)

/// `scale`, `scalm`, `defl` and `defw` take plain numbers only. The last plain
/// number wins, also after the MOSFET line. Other values are ignored (decks
/// `opt_scale_*`, `opd_*`, `os_*`).
#[tokio::test]
async fn option_sizes_are_plain_numbers() {
    let line = "M1 d g 0 0 n w=1u\n";
    for (head, l) in [
        (".option defl=7u\n", 7.0),
        (".option DEFL=7u\n", 7.0),
        (".option defl='7u'\n", 100.0),
        (".param q=7u\n.option defl=q\n", 100.0),
        (".option defl='3u+4u'\n", 100.0),
        (".option defl=7u\n.option defl='9u'\n", 7.0),
        (".param q=9u\n.option defl=7u\n.option defl=q\n", 7.0),
        (".option defl='9u'\n.option defl=7u\n", 7.0),
        (".option defl=9u\n.option defl=7u\n", 7.0),
    ] {
        let text = format!("{IF_N}{head}{line}");
        assert_eq!(one_size(&text).await, (l, 1.0), "{head}");
    }
    let line = "M1 d g 0 0 n l=2 w=3\n";
    for (head, l, w) in [
        (".option scale=1u\n", 2.0, 3.0),
        (".option scale='1u'\n", 2e6, 3e6),
        (".param q=1u\n.option scale=q\n", 2e6, 3e6),
        (".option scale=1u\n.option scale='4u'\n", 2.0, 3.0),
        (".param q=4u\n.option scale=1u\n.option scale=q\n", 2.0, 3.0),
        (".subckt z d g\n.option scale=4u\n.ends\n", 2e6, 3e6),
    ] {
        let text = format!("{IF_N}{head}{line}");
        assert_eq!(one_size(&text).await, (l, w), "{head}");
    }
    // after the line
    assert_eq!(
        one_size(&format!("{IF_N}{line}.option scale=1u\n")).await,
        (2.0, 3.0)
    );
    // `scalm` chooses the bin, `b.2` with these limits times 1000
    let bins = ".model b.1 nmos level=54 lmin=1e-9 lmax=1e-6 wmin=1e-9 wmax=1e-3\n\
                .model b.2 nmos level=54 lmin=1e-6 lmax=1e-3 wmin=1e-9 wmax=1\n";
    for (head, want) in [
        ("", "b.2"),
        (".option scalm=1e3\n", "b.1"),
        (".option scalm='1e3'\n", "b.2"),
        (".param q=1e3\n.option scalm=q\n", "b.2"),
    ] {
        let text = format!("* t\n{head}{bins}M1 d g 0 0 b l=2u w=1u\n");
        assert_eq!(one_model(&text).await, want, "{head}");
    }
}

/// `.option wnflag` takes a number, also in quotes. An expression or a name is
/// ignored. On the line, every value counts (decks `opw_*`).
#[tokio::test]
async fn wnflag_values() {
    let bins = ".model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6\n\
                .model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3\n";
    let line = "M1 d g 0 0 a w=4u l=1u nf=8";
    for (head, tail, want) in [
        ("", "", "a.1"),
        (".option wnflag=0\n", "", "a.2"),
        (".option wnflag=1\n", "", "a.1"),
        (".option wnflag\n", "", "a.1"),
        (".option wnflag='1'\n", "", "a.1"),
        (".option wnflag='0'\n", "", "a.2"),
        (".option wnflag='1-1'\n", "", "a.1"),
        (".param wf=0\n.option wnflag=wf\n", "", "a.1"),
        ("", " wnflag=0", "a.2"),
        ("", " wnflag='0'", "a.2"),
        (".param wf=0\n", " wnflag=wf", "a.2"),
        (".option wnflag=0\n", " wnflag=1", "a.1"),
    ] {
        let text = format!("* t\n{head}{bins}{line}{tail}\n");
        assert_eq!(one_model(&text).await, want, "{head}{tail}");
    }
}

/// `.option wl` swaps the two values after the model name. It counts anywhere
/// in the deck: after the line, in a subckt that is not used, in a branch that
/// is not active, in an include. Only `wl` and `wl=1` set it, and the last
/// one wins (decks `opt_wl_*`, `wv_*`).
#[tokio::test]
async fn option_wl_swaps_the_values() {
    let line = "M1 d g 0 0 n 1u 2u\n";
    for (head, swap) in [
        ("", false),
        (".option wl\n", true),
        (".OPTION WL\n", true),
        (".option wl=1\n", true),
        (".option wl=1.0\n", true),
        (".option wl=0\n", false),
        (".option wl=2\n", false),
        (".option wl=0.5\n", false),
        (".option wl='1'\n", false),
        (".param q=1\n.option wl=q\n", false),
        (".option nowl\n", false),
        (".option wl\n.option wl=0\n", false),
        (".option wl=0\n.option wl\n", true),
        (".option wl\n.option wl='1'\n", false),
        (".option wl='1'\n.option wl\n", true),
        (".param q=1\n.option wl\n.option wl=q\n", false),
        (".subckt z d g\n.option wl\n.ends\n", true),
        (".if (0)\n.option wl\n.endif\n", true),
        (".if (1)\n.option wl\n.endif\n", true),
    ] {
        let text = format!("{IF_N}{head}{line}");
        let want = if swap { (2.0, 1.0) } else { (1.0, 2.0) };
        assert_eq!(one_size(&text).await, want, "{head}");
    }
    let wl = ".option wl\n";
    assert_eq!(one_size(&format!("{IF_N}{line}{wl}")).await, (2.0, 1.0));
    // a value given by name wins over its place; the other one is not given
    assert_eq!(
        one_size(&format!("{IF_N}{wl}M1 d g 0 0 n 1u w=2u\n")).await,
        (100.0, 2.0)
    );
    assert_eq!(
        one_size(&format!("{IF_N}{wl}M1 d g 0 0 n 3u\n")).await,
        (100.0, 3.0)
    );
    assert_eq!(
        one_size(&format!("{IF_N}{wl}M1 d g 0 0 n 1u 2u 3u\n")).await,
        (2.0, 1.0)
    );
    // in a subckt
    let text = format!("{IF_N}{wl}.subckt s d g\n{line}.ends\nX1 d g s\n");
    assert_eq!(one_size(&text).await, (2.0, 1.0));
    // in an include
    let (_dir, parsed, files) = parse_deck(&[
        ("deck.sp", &format!("{IF_N}.inc 'wl.inc'\n{line}")),
        ("wl.inc", wl),
    ])
    .await;
    let (ast, _) = files.build(parsed);
    let s = ast.mosfet_models().remove(0).selection.unwrap();
    assert!(close(s.l, 2e-6), "{}", s.l);
    // a bin is chosen with the swapped values
    let text = format!(
        "* t\n{wl}.model b.1 nmos level=54 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n\
         .model b.2 nmos level=54 lmin=1u lmax=1e-3 wmin=1e-8 wmax=1e-3\nM1 d g 0 0 b 1u 500n\n"
    );
    assert_eq!(one_model(&text).await, "b.1");
}

/// NF below 1 counts as 1 in the result, and the bin still divides W by it. NF=0
/// leaves W whole (decks `m_nf_zero`, `mx_nf_*`).
#[tokio::test]
async fn nf_below_one() {
    let bins = |limit: &str| {
        format!(
            "* t\n.model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax={limit}\n\
             .model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin={limit} wmax=1e-3\n"
        )
    };
    for (head, line, model, nf) in [
        (bins("3e-6"), "M1 d g 0 0 a w=2u l=1u nf=0.5\n", "a.2", 1.0),
        (bins("1e-6"), "M1 d g 0 0 a w=4u l=1u nf=0\n", "a.2", 1.0),
        (bins("1e-6"), "M1 d g 0 0 a w=4u l=1u nf=8\n", "a.1", 8.0),
        (
            bins("1e-6"),
            "M1 d g 0 0 a w=2.5u l=1u nf=2.5\n",
            "a.2",
            2.5,
        ),
    ] {
        let rows = deck_rows(&format!("{head}{line}")).await;
        assert_eq!(rows.len(), 1, "{line}");
        assert_eq!((rows[0].1.as_str(), rows[0].4), (model, nf), "{line}");
    }
}

/// HSPICE stops at a size that is not positive; W=0 runs (decks `m_l_neg`,
/// `m_w_neg`, `m_nf_neg`, `m_l_zero`, `m_m_zero`, `m_w_zero`)
#[tokio::test]
async fn sizes_that_are_not_positive() {
    for (tail, stops) in [
        ("l=-1u w=1u", true),
        ("l=1u w=-1u", true),
        ("l=1u w=1u nf=-2", true),
        ("l=0 w=1u", true),
        ("l=1u w=1u m=0", true),
        ("l=1u w=1u m=-1", true),
        ("l=1u w=0", false),
        ("l=1u w=1u m=2", false),
        ("l=1u w=1u M=2", false),
        ("l=1u w=1u m='1+1'", false),
    ] {
        let rows = deck_rows(&format!("{IF_N}M1 d g 0 0 n {tail}\n")).await;
        assert_eq!(rows.len(), 1, "{tail}");
        assert_eq!(rows[0].1.starts_with("error"), stops, "{tail}: {rows:?}");
    }
}

/// With a limit given twice, the first one counts (deck `mdl_dup_keys`)
#[tokio::test]
async fn first_limit_of_two_counts() {
    let text = "* t\n.model b.1 nmos level=54 lmin=1e-8 lmax=1e-6 lmax=1e-3 wmin=1e-8 wmax=1e-3\n\
                .model b.2 nmos level=54 lmin=1e-3 lmax=1 wmin=1e-8 wmax=1e-3\n\
                M1 d g 0 0 b l=2u w=1u\n";
    assert!(one_model(text).await.starts_with("error"), "{text}");
    let text = text.replace("lmax=1e-6 lmax=1e-3", "lmax=1e-3 lmax=1e-6");
    assert_eq!(one_model(&text).await, "b.1");
}

/// Only NMOS and PMOS models serve an M line, and other types of the name are
/// not seen (decks `mt_*`, `mdl_diode_on_m`, `mdl_r_on_m`)
#[tokio::test]
async fn only_mos_models_serve_an_m_line() {
    let line = "M1 d g 0 0 q l=1u w=1u\n";
    for kind in ["nmos", "NMOS", "pmos", "PMos"] {
        assert_eq!(
            one_model(&format!("* t\n.model q {kind} level=54\n{line}")).await,
            "q",
            "{kind}"
        );
    }
    for kind in [
        "d", "r", "c", "nch", "pch", "n", "p", "nfet", "pfet", "nmosfet", "npn",
    ] {
        let text = format!("* t\n.model q {kind}\n{line}");
        assert!(one_model(&text).await.starts_with("error"), "{kind}");
    }
    // the bins of a name of another type
    let text = format!("* t\n.model q.1 nch level=54 lmin=1e-8 lmax=1 wmin=1e-8 wmax=1\n{line}");
    assert!(one_model(&text).await.starts_with("error"), "{text}");
    // the model at the top level serves past a diode model of the name
    let text =
        format!("* t\n.model q nmos level=54\n.subckt s d g\n.model q d\n{line}.ends\nX1 d g s\n");
    assert_eq!(one_model(&text).await, "q");
    assert_eq!(deck_rows(&text).await[0].1, "q");
}

/// Level 72 bins hold a fin count, not a width: `nfinmin <= nfin < nfinmax`.
/// `wsheetmin <= wsheet < wsheetmax` is the other form (decks `f_*`, `k_*`,
/// `w_*`, `p_*`).
#[tokio::test]
async fn finfet_bins() {
    let bins = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\n\
                .model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=4 nfinmax=9\n";
    for (head, tail, want) in [
        ("", "l=500n nfin=1", "f.1"),
        ("", "l=500n nfin=2", "f.1"),
        ("", "l=500n nfin=3.5", "f.1"),
        ("", "l=500n nfin=4", "f.2"),
        ("", "l=500n nfin=8.5", "f.2"),
        ("", "l=500n NFIN=8.5", "f.2"),
        ("", "l=500n", "f.1"),
        ("", "l=500n w=1u", "f.1"),
        ("", "l=500n w=1u nfin=6", "f.2"),
        ("", "l=500n nfin=6 nf=2", "f.2"),
        ("", "l=500n nfin=3 nf=2", "f.1"),
        ("", "l=500n nfin=3 m=2", "f.1"),
        (".param k=6\n", "l=500n nfin=k", "f.2"),
        (".param k=3\n", "l=500n nfin='k*2'", "f.2"),
        ("", "500n 1u nfin=6", "f.2"),
        (".option scale=1e-6\n", "l=0.5 nfin=6", "f.2"),
        (".option scale=2\n", "l=250n nfin=3", "f.1"),
    ] {
        let text = format!("* t\n{head}{bins}M1 d g 0 0 f {tail}\n");
        assert_eq!(one_model(&text).await, want, "{head}{tail}");
    }
    for (head, tail) in [
        ("", "l=500n nfin=0.5"),
        ("", "l=500n nfin=9"),
        ("", "l=500n nfin=10"),
        ("", "l=1u nfin=3"),
        ("", "l=2u nfin=3"),
        (".option scalm=1e-1\n", "l=500n nfin=6"),
    ] {
        let text = format!("* t\n{head}{bins}M1 d g 0 0 f {tail}\n");
        assert!(one_model(&text).await.starts_with("error"), "{head}{tail}");
    }
    // the keys of a bin
    for (model, line, want) in [
        (
            ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3 nfinmin=1 nfinmax=4\n",
            "l=500n w=1u nfin=2",
            "f.1",
        ),
        (
            ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n",
            "l=500n w=1u",
            "error",
        ),
        (
            ".model f.1 nmos level=72 lmax=1u nfinmin=1 nfinmax=4\n",
            "l=500n nfin=2",
            "error",
        ),
        (
            ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1\n",
            "l=500n nfin=2",
            "error",
        ),
        (
            ".model f.1 nmos level=54 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\n",
            "l=500n w=1u nfin=2",
            "error",
        ),
        (
            ".model f.1 nmos level=70 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n",
            "l=500n w=1u",
            "f.1",
        ),
        (".model f nmos level=72\n", "l=500n nfin=6", "f"),
        (".model f nmos level=72\n", "l=500n", "f"),
    ] {
        let got = one_model(&format!("* t\n{model}M1 d g 0 0 f {line}\n")).await;
        assert!(got.starts_with(want), "{model}{line}: {got}");
    }
    // the sheet form
    let sheets = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-9 wsheetmax=1e-8\n\
                  .model f.2 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6\n";
    let text = format!("* t\n{sheets}M1 d g 0 0 f l=500n wsheet=2e-8\n");
    assert_eq!(one_model(&text).await, "f.2");
    let text = format!("* t\n{sheets}M1 d g 0 0 f l=500n w=2e-8\n");
    assert!(one_model(&text).await.starts_with("error"), "{text}");
    // a bin of another level, in the same set
    let text = "* t\n.model f.1 nmos level=54 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n\
                .model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\n\
                M1 d g 0 0 f l=500n w=1u nfin=2\n";
    assert_eq!(one_model(text).await, "f.1");
    // a parameter read by the fin count changes the bin in the counts too
    let text = format!(
        "* t\n{bins}.subckt s d g k=2\nM1 d g 0 0 f l=500n nfin=k\n.ends\nX1 d g s\nX2 d g s k=5\nX3 d g s\n"
    );
    let rows = deck_rows(&text).await;
    let models: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
    assert_eq!(models, ["f.1", "f.2", "f.1"]);
}

/// `m` on an X line is not passed to the subckt (decks `x_m_*`)
#[tokio::test]
async fn m_on_an_x_line_is_not_passed() {
    for (text, l) in [
        (
            ".subckt s d g m=3\nM1 d g 0 0 n l='m*1u' w=1u\n.ends\nX1 d g s m=2\n",
            3.0,
        ),
        (
            ".param m=5\n.subckt s d g\nM1 d g 0 0 n l='m*1u' w=1u\n.ends\nX1 d g s m=2\n",
            5.0,
        ),
        (
            ".option parhier=local\n.param m=5\n.subckt s d g\nM1 d g 0 0 n l='m*1u' w=1u\n.ends\nX1 d g s m=2\n",
            5.0,
        ),
        (
            ".subckt s d g m=3\nM1 d g 0 0 n l='m*1u' w=1u\n.ends\nX1 d g s M=2\n",
            3.0,
        ),
        (
            ".subckt s d g k=3\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s k=2 m=2\n",
            2.0,
        ),
    ] {
        let text = format!("{IF_N}{text}");
        assert_eq!(one_size(&text).await, (l, 1.0), "{text}");
    }
    let text = format!("{IF_N}.subckt s d g\nM1 d g 0 0 n l='m*1u' w=1u\n.ends\nX1 d g s m=2\n");
    assert!(one_model(&text).await.starts_with("error"), "{text}");
}

/// A condition stops at `temper` and `time`, also through a parameter (decks
/// `tc_*`). In a value, `time` reads 0 (deck `p_time_in_expr`).
#[tokio::test]
async fn temper_and_time() {
    for head in [
        ".temp 75\n.param t='temper'\n.if (t>50)\n",
        ".temp 75\n.param t='temper*1'\n.if (t)\n",
        ".if (temper>50)\n",
        ".if (time>0)\n",
        ".param t='time'\n.if (t==0)\n",
    ] {
        let text = format!("{IF_N}{head}{IF_M1}.else\n{IF_M2}.endif\n");
        let (_, rows) = if_deck(&text).await;
        assert_eq!(rows.len(), 1, "{head}: {rows:?}");
        assert!(rows[0].1.starts_with("error"), "{head}: {rows:?}");
    }
    // the same parameters read in a value are not a problem
    let text = format!("{IF_N}.temp 75\n.param t='temper'\nM1 d g 0 0 n l='t*1u' w=1u\n");
    assert_eq!(one_size(&text).await, (75.0, 1.0));
    let text = format!("{IF_N}M1 d g 0 0 n l='time+1u' w=1u\n");
    assert_eq!(one_size(&text).await, (1.0, 1.0));
    let text = format!("{IF_N}.param t='time+1u'\nM1 d g 0 0 n l=t w=1u\n");
    assert_eq!(one_size(&text).await, (1.0, 1.0));
}

/// `log(0)` is the logarithm of 1e-28 (decks `g_log0`, `g_log100`)
#[tokio::test]
async fn logarithm_of_zero() {
    let text = format!("{IF_N}.param c='log(0)'\nM1 d g 0 0 n l='(c+1000)*1e-9' w=1u\n");
    let (l, _) = one_size(&text).await;
    assert!((l - 0.9355276).abs() < 1e-6, "{l}");
    let text = format!("{IF_N}.param c='log10(0)'\nM1 d g 0 0 n l='(c+1000)*1e-9' w=1u\n");
    let (l, _) = one_size(&text).await;
    assert!((l - 0.972).abs() < 1e-6, "{l}");
}

/// An X line that HSPICE rejects gives an error row, and no rows below it:
/// node counts (decks `x_too_few_nodes`, `x_too_many_nodes`, `subckt_param_kw`),
/// recursion (`x_self`, `x_mutual`), and a depth over the limit (`dp_loc_*`).
#[tokio::test]
async fn x_lines_that_cannot_be_expanded() {
    let body = "M1 d g 0 0 n l=1u w=1u\n";
    let s = format!(".subckt s d g\n{body}.ends\n");
    for line in ["X1 d s\n", "X1 d g h s\n", "X1 s\n"] {
        let rows = deck_rows(&format!("{IF_N}{s}{line}")).await;
        assert_eq!(rows.len(), 1, "{line}: {rows:?}");
        assert_eq!(rows[0].0, "X1", "{line}");
        assert!(rows[0].1.starts_with("error"), "{line}: {rows:?}");
    }
    // the keyword `param:` is a node there
    let text =
        format!("{IF_N}.subckt s d g param: w=1u\nM1 d g 0 0 n l=1u w=w\n.ends\nX1 d g s w=3u\n");
    assert!(deck_rows(&text).await[0].1.starts_with("error"), "{text}");
    // the right count
    assert_eq!(deck_rows(&format!("{IF_N}{s}X1 d g s\n")).await[0].1, "n");
    // recursion
    for text in [
        format!(".subckt a d g\n{body}X1 d g a\n.ends\nX1 d g a\n"),
        format!(".subckt a d g\nX1 d g b\n.ends\n.subckt b d g\n{body}X1 d g a\n.ends\nX1 d g a\n"),
    ] {
        let text = format!("{IF_N}{text}");
        let rows = deck_rows(&text).await;
        assert!(
            rows.iter().any(|r| r.1.starts_with("error")),
            "{text}: {rows:?}"
        );
    }
    // too deep for the walk
    let mut deep = format!("{IF_N}.subckt s0 d g\n{body}.ends\n");
    for k in 1..300 {
        deep += &format!(".subckt s{k} d g\nX1 d g s{}\n.ends\n", k - 1);
    }
    deep += "X1 d g s299\n";
    let rows = deck_rows(&deep).await;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].1.starts_with("error"), "{}", rows[0].1);
    // such errors do not count as MOSFETs
    let (_dir, parsed, files) = parse_deck(&[("deck.sp", &deep)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.model_usage().mosfets, 0);
    let text = format!("{IF_N}{s}X1 d s\nX2 d g s\n");
    let (_dir, parsed, files) = parse_deck(&[("deck.sp", &text)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.model_usage().mosfets, 1);
}
