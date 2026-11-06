use super::*;

// dialect details found in real netlists (Infineon adsbenchmark)

/// Infineon's model files start with `simulator lang = spice`, which used to
/// become a fake element named `simulator`. It is a Spectre directive, not
/// HSPICE syntax: it is ignored, with a warning.
#[tokio::test]
async fn simulator_lang_spice_is_ignored_with_a_warning() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        // header of adsbenchmark/*/src/model/bip_nom.scs
        "simulator lang = spice\n\
         .model latpnp pnp \n\
         + is=1.8e-15 bf=120 rb=500 re=50 vaf=5  var=3 nf=1.25 ikf=3e-5 tf=100e-12\n\
         + cje=2.5e-15 vje=0.9 mje=0.55 cjc=25e-15 vjc=0.9 mjc=0.55\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("NonHspice", 1)]);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err, "a warning is not an error");
    assert!(
        ast.instance.is_empty(),
        "no element named `simulator`:\n{ast}"
    );
    assert_eq!(ast.model.len(), 1);
    assert_eq!(ast.model[0].params.len(), 15);
}

/// A Spectre section can't be read as SPICE: it is reported and skipped up to
/// `simulator lang=spice`, or to the end of the file.
#[tokio::test]
async fn simulator_lang_spectre_section_is_reported_and_skipped() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "R1 a b 1k\n\
         simulator lang=spectre\n\
         r2 (a b) resistor r=2k\n\
         parameters x=1\n\
         Simulator Lang=SPICE\n\
         R3 a b 3k\n\
         simulator lang=spectre\n\
         r4 (a b) resistor r=4k\n",
    )])
    .await;
    assert_eq!(
        diagnostics(&parsed),
        [
            diag("Unsupported", 2),
            diag("NonHspice", 5),
            diag("Unsupported", 7)
        ]
    );
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(names(&ast), ["R1", "R3"]);
}

/// HSPICE continues a statement with a trailing `\` (the line break counts as
/// white space). Infineon's PHY testbench continues a `.PARAM` line this way,
/// and the statement used to be dropped.
#[tokio::test]
async fn backslash_continues_a_line() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        // adsbenchmark/PHY/src/top.hsp, lines 17-18
        ".PARAM pll_wait_time=100n rbulkpanalog=1m rbulkpcore=1m rhsupanalog=1m rhsupcore=1m \\\n\
         \t\ttd_ps=1n tr_ps=1n vsup1v5=1.5 vsup2v5=2.5 phy_test=1\n\
         M1 d g s b nch W=1u \\\n\
         L=0.1u\n\
         .tran 1n \\\n\
         10n\n\
         .param bad=1 ( \\\n\
         c=2\n\
         R1 a b 1k $ note \\\n\
         R2 a b 2k\n",
    )])
    .await;
    // the bad statement is skipped together with its continuation line, and a
    // `\` inside a `$` comment continues nothing
    assert_eq!(diagnostics(&parsed), [diag("Trailing", 7)]);
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.param.len(), 10);
    assert_eq!(names(&ast), ["M1", "R1", "R2"]);
    let InstanceCtx::MOSFET(m) = &inst(&ast, "M1").ctx else {
        panic!()
    };
    assert_eq!(m.params.len(), 2);
    // the lines are joined when the file is read
    assert!(ast.to_string().contains(".tran 1n  10n"), "{ast}");
}

/// HSPICE: a trailing `\\` joins the lines with no white space (e.g. to split
/// a long path), and both forms also work inside quotes. Line numbers after
/// the joined lines stay right.
#[tokio::test]
async fn double_backslash_joins_lines_without_white_space() {
    let (_dir, parsed, files) = parse_text(&[
        (
            "top.sp",
            ".inc 'sm\\\\\n\
             \x20   all.sp'\n\
             .param y=12\\\\\n\
             34 z='1 + \\\n\
             2'\n\
             M9 d g\n",
        ),
        ("small.sp", "R0 a b 1k\n"),
    ])
    .await;
    // `M9 d g` (too few nodes) is on line 6 of the file
    assert_eq!(diagnostics(&parsed), [diag("NodeCount", 6)]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R0"], "the split include path resolves");
    let param = |k: &str| &ast.param.iter().find(|kv| kv.k == k).unwrap().v;
    assert!(close(num(param("y")), 1234.0));
    assert!(
        matches!(param("z"), Value::Expr(e) if !e.contains(['\\', '\n'])),
        "{:?}",
        param("z")
    );
}

/// After a `\`, a `+` starting the next line is a continuation mark, except
/// inside quotes, where it is an operator.
#[tokio::test]
async fn backslash_then_plus() {
    let (_dir, parsed, files) = parse_text(&[("top.sp", ".param a=1 \\\n+ b='1 \\\n+ 2'\n")]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.param.len(), 2);
    assert!(
        matches!(&ast.param[1].v, Value::Expr(e) if e.contains("+ 2")),
        "{ast}"
    );
}
