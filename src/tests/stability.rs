use super::*;

// stability, and HSPICE syntax found in the HSPICE corpus

/// A `.subckt` nested in the one before it, 400 deep (as in a cell library
/// whose `.ends` lines are missing), used to overflow the stack and abort.
#[tokio::test]
async fn deep_subckt_nesting_does_not_overflow_the_stack() {
    use crate::parser::MAX_NESTING;
    const DEPTH: usize = 400;
    let mut text = String::new();
    for i in 0..DEPTH {
        writeln!(text, ".subckt s{i} a").unwrap();
    }
    text.push_str("R1 a 0 1k\n");
    text.push_str(&".ends\n".repeat(DEPTH));
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    let diags = diagnostics(&parsed);
    let too_deep: Vec<_> = diags.iter().filter(|d| d.kind == "TooDeep").collect();
    // one error, for the first `.subckt` too deep, on its own line; the part
    // below it is skipped
    assert_eq!(too_deep, [&diag("TooDeep", MAX_NESTING as u32 + 1)]);
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(count_instances(&ast), 0);
    // building and printing it do not overflow either
    assert!(ast.to_string().contains(".SUBCKT s0 a"), "{ast}");

    // a cell library without `.ends`: the cells up to the limit are kept, and
    // the error names the limit
    let mut lib = String::new();
    for i in 0..500 {
        writeln!(lib, ".subckt cell{i} a b\nM1 a b 0 0 nch w=1u l=1u").unwrap();
    }
    let (_dir, parsed, files) = parse_text(&[("lib.sp", &lib)]).await;
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(count_instances(&ast), MAX_NESTING);
}

/// `Subckt::append` on a subckt that instantiates itself used to recurse
/// until the stack overflowed.
#[tokio::test]
async fn recursive_subckt_instantiation_is_an_error() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".subckt a x\nX1 x b\n.ends\n.subckt b x\nX2 x a\n.ends\n\
         .subckt c x\nX3 x c\n.ends\n.subckt d x\nX4 x b\n.ends\n",
    )])
    .await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    let append = |name: &str| {
        crate::Subckt::append(
            name,
            |_| false,
            &mut Vec::new(),
            &mut vec![&ast.subckt],
            &mut Vec::new(),
            &mut Vec::new(),
        )
    };
    assert_eq!(
        append("a"),
        Err("X1.X2.a: subckt `a` instantiates itself (a -> b -> a)".to_owned())
    );
    assert_eq!(
        append("c"),
        Err("X3.c: subckt `c` instantiates itself (c -> c)".to_owned())
    );
    assert_eq!(
        append("d"),
        Err("X4.X2.X1.b: subckt `b` instantiates itself (b -> a -> b)".to_owned())
    );
}

/// Line ends of classic Mac OS (CR only) used to make the whole file one
/// line, a UTF-8 byte order mark broke the first statement, and one byte that
/// is not UTF-8 (a Latin-1 `µ` in a comment) made the whole file unreadable.
#[tokio::test]
async fn cr_line_ends_bom_and_latin1_are_read() {
    let dir = TempDir::new("encoding");
    dir.write_bytes("cr.sp", b"R1 a b 1k\rC1 b 0 1p\rV1 a 0 1.8\r");
    // (and one inside the file, as when files are joined)
    dir.write_bytes("bom.sp", b"\xef\xbb\xbfR2 a b 1k\n\xef\xbb\xbfC2 b 0 1p\n");
    dir.write_bytes("latin1.sp", b"* bias 10 \xb5A\nR3 a b 1k\n");
    // a deck whose title line follows a byte order mark
    let deck = dir.write_bytes(
        "deck.sp",
        b"\xef\xbb\xbf* deck\n.inc 'cr.sp'\n.inc 'bom.sp'\n.inc 'latin1.sp'\n",
    );
    let (parsed, files) = parse_files(&[deck]).await;
    assert_eq!(
        diagnostics(&parsed),
        [Diag {
            kind: "Encoding",
            line: None,
            warning: true
        }]
    );
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(names(&ast), ["R1", "C1", "V1", "R2", "C2", "R3"]);
}

/// The sections of a library file are read from one reading of it (a PDK
/// model file of tens of MB used to be read and kept in memory once per
/// section, all at once): sections loaded from a corner, a missing section,
/// a library that is not UTF-8, and one that does not exist.
#[tokio::test]
async fn library_sections_share_one_reading() {
    let dir = TempDir::new("lib");
    dir.write_bytes(
        "models.l",
        b"* library\n.lib tt\n* \xb5\n.param d=0\n.lib 'models.l' core\n.lib 'models.l' io\n.endl tt\n\
          .lib core\n.model nch.1 nmos lmin=1e-8 lmax=1e-6 wmin=1e-8 wmax=1\n\
          .model nch.2 nmos lmin=1e-6 lmax=1e-4 wmin=1e-8 wmax=1\n.endl core\n\
          .lib io\n.model nio nmos level=54\n.endl io\n",
    );
    let deck = dir.write_bytes(
        "deck.sp",
        b"* deck\n.lib 'models.l' tt\n.lib 'models.l' rf\n.lib 'nothing.l' tt\n.lib 'nothing.l' ss\n\
          M1 d g s b nch l=2u w=1u\nM2 d g s b nio l=1u w=1u\n",
    );
    let (parsed, files) = parse_files(&[deck]).await;
    let mut kinds: Vec<_> = diagnostics(&parsed).iter().map(|d| d.kind).collect();
    kinds.sort();
    // the encoding warning of the section with the bad byte (tt), the missing
    // section, and each section of the missing file
    assert_eq!(kinds, ["Encoding", "IO", "IO", "NoLibSection"]);
    let (ast, _) = files.build(parsed);
    let models: Vec<_> = ast.model.iter().map(|m| m.name.to_string()).collect();
    assert_eq!(models, ["nch.1", "nch.2", "nio"]);
    let chosen: Vec<_> = ast
        .mosfet_models()
        .into_iter()
        .map(|m| m.selection.unwrap().model.name.to_string())
        .collect();
    assert_eq!(chosen, ["nch.2", "nio"]);
}

/// A quote that was not closed used to take in every line up to the next
/// quote in the file (here, two resistors), and the error pointed there.
#[tokio::test]
async fn unterminated_quote_stays_in_its_statement() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "R0 a b 1k\n.param x='1+2\nR1 a b 1k\nR2 a b 1k\n\
         .param y='3' z='1+\n+ 2'\nR3 a b 1k\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("Trailing", 2)]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R0", "R1", "R2", "R3"]);
    // a quoted expression may still go on over a continuation line
    assert_eq!(ast.param.len(), 2);
}

/// A source with no value (an ammeter) used to be an error when a comment or
/// a space followed its nodes.
#[tokio::test]
async fn source_without_value() {
    let (_dir, parsed, files) =
        parse_text(&[("top.sp", "V1 a 0 $ ammeter\nVm a b \nI1 c d\n")]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["V1", "Vm", "I1"]);
    let InstanceCtx::Voltage(v) = &inst(&ast, "V1").ctx else {
        panic!()
    };
    let s = &v.source;
    assert!(s.dc.is_none() && s.ac_mag.is_none() && s.func.is_none() && s.params.is_empty());
    assert!(
        ast.to_string().contains("\nV1 a 0\nVm a b\nI1 c d"),
        "{ast}"
    );
}

/// Values that used to print as text that parses back differently: a number
/// too large for `f64` (printed `inf`), a `.data` file name with a `'`, a
/// parameter named `*e` (a comment), and a function call with a quoted
/// argument (printed in nested quotes).
#[tokio::test]
async fn awkward_values_print_back_the_same() {
    const DECK: &str = "\
R1 a b 1e999
R2 a b 50 TC=0,0
.param a=1 b=2 c=3 d=4 *e=5
.param val2=agauss(0,0,'2*p1')
.param delay = OPT1 ( 0n, 0n, 5n )
.data d MER
+ FILE=\"it's.dat\" p1=1
.enddata
";
    let (_dir, parsed, files) = parse_text(&[("top.sp", DECK)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    // `*e=5` is a comment; `1e999` is kept as written
    assert_eq!(ast.param.len(), 6);
    let InstanceCtx::Resistor(r) = &inst(&ast, "R1").ctx else {
        panic!()
    };
    assert!(matches!(&r.value, Some(Value::Expr(e)) if e == "1e999"));
    let first = ast.to_string();
    assert!(first.contains("TC=0,0"), "{first}");
    assert!(first.contains("delay=OPT1 ( 0n, 0n, 5n )"), "{first}");
    assert!(first.contains("val2=agauss(0,0,'2*p1')"), "{first}");
    let (_dir2, parsed2, files2) = parse_deck(&[("top.sp", &first)]).await;
    assert_eq!(diagnostics(&parsed2), [], "printed netlist:\n{first}");
    let (ast2, _) = files2.build(parsed2);
    assert_eq!(ast2.to_string(), first);

    // HSPICE rejects a quote in a function call without quotes around it
    // ("cannot use quotation marks"), but not in a statistical function or in
    // `str`, when the call is the whole value. It also reads a group at the
    // start of a value (probes on P-2019.06).
    for text in [
        ".param v=max(1,'2')\n",
        ".param v=sin('p')\n",
        ".param v=(1+'2')\n",
        ".param v=2*agauss(0,0,'2')\n",
        ".param v=agauss(0,0,'2')*2\n",
        ".param v=max(1,agauss(0,0,'2'))\n",
        ".param v=agauss(0,0,max(1,'2'))\n",
        ".param v='max'(1,2)\n",
    ] {
        let (_dir, parsed, _) = parse_text(&[("top.sp", text)]).await;
        assert_eq!(diagnostics(&parsed), [diag("Trailing", 1)], "{text}");
    }
    for text in [
        ".param v=AGAUSS('1', 0, '2') w=2\n",
        ".param v=agauss(0,0,\n+ '2')\n",
        ".param v=gauss(1,0.1,'2')\n.param w=aunif(1,0.1,'2')\n",
        ".param v=unif(1,0.1,'2')\n.param w=limit(1,0.1,'2')\n",
        ".param v=str('x')\n.param w=str(\"x\")\n",
        ".subckt s a w=agauss(1u,0,'2') f=str('x')\n.ends\nX1 a s w=agauss(1u,0,'2') f=str('x')\n",
        "V1 a 0 PWL PWLFILE=str('x.pwl')\n",
    ] {
        let (_dir, parsed, _) = parse_text(&[("top.sp", text)]).await;
        assert_eq!(diagnostics(&parsed), [], "{text}");
    }
    // ... and such a value has its nominal value
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model a nmos level=54\n.param q=agauss(1u,0,'2')\nM1 d g 0 0 a l=q w=1u\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    assert_sizes(&files.build(parsed).0, &[("M1", 1.0, 1.0)], "agauss");
    // HSPICE takes commas in `.global`
    let (_dir, parsed, files) = parse_text(&[("top.sp", ".global a,b, c\n+ d,\n+ e\n")]).await;
    assert_eq!(diagnostics(&parsed), []);
    assert_eq!(files.build(parsed).0.global.len(), 5);
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".param p=(1+2)*1k q=max(1k, 3k)\nX1 a b s w=(3k)\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.param.len(), 2);
    assert!(matches!(&ast.param[1].v, Value::Expr(e) if e == "max(1k, 3k)"));
}

/// HSPICE also delimits expressions with double quotes (2,641 `X` lines in
/// the HSPICE corpus were rejected).
#[tokio::test]
async fn double_quoted_values() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "XNAND2_0 NA1 NA0 Y vdd gnd NAND2 size = \"2\" Lg = '20n'\n\
         X2 a b y vdd gnd NAND2 SN=\"SN_NAND2\" SP=\"1\"\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let InstanceCtx::Subckt(x) = &inst(&ast, "XNAND2_0").ctx else {
        panic!()
    };
    assert_eq!((x.cktname.as_ref(), x.nodes.len()), ("NAND2", 5));
    assert!(matches!(&x.params[0].v, Value::Expr(e) if e == "2"));
    let InstanceCtx::Subckt(x) = &inst(&ast, "X2").ctx else {
        panic!()
    };
    assert!(matches!(&x.params[0].v, Value::Expr(e) if e == "SN_NAND2"));
}

/// The rows of a `.data` block may be written without `+` (about 400 rows in
/// the HSPICE corpus were rejected); without `.enddata`, only `+` lines
/// belong to the block.
#[tokio::test]
async fn data_rows_without_plus() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".data vectorload\nload\n0.004f\n0.012f\n.enddata\n\
         .data truth bit0 bit1\n0 0 $ first row\n* a comment\n+ 1 0\n.enddata\n\
         .data open p\n+ 1\nR1 a b 1k\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R1"]);
    let shape: Vec<_> = ast
        .data
        .iter()
        .map(|d| match &d.values {
            crate::ast::DataValues::InlineNum { params, values } => (params.len(), values.len()),
            _ => panic!(),
        })
        .collect();
    assert_eq!(shape, [(1, 2), (2, 4), (1, 1)]);
}

/// `PWL(1)` controlled sources (166 statements in the HSPICE corpus) used to
/// be reported as not supported.
#[tokio::test]
async fn pwl_controlled_sources() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "G_Switch BE 0 vcr PWL(1) ctrl 0 2.6v,1m 2.4v,100g\n\
         e1 s_clip 0 pwl(1) s 0 DELTA=0.01 -0.1,-0.1 0.1,0.1\n\
         G1 IOUT+ IOUT- VCR PWL(1) VC+ VC-\n\
         +(0,     2.39E-05)\n\
         +(40.33, 1.05E-03)\n\
         F1 a b NAND(2) v1 v2 0,0 1,1 SCALE=2\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let controlled = |name: &str| match &inst(&ast, name).ctx {
        InstanceCtx::ControlledSource(c) => c.clone(),
        _ => panic!(),
    };
    let g = controlled("G_Switch");
    let pwl = g.pwl.as_ref().unwrap();
    assert_eq!(
        (pwl.function.as_ref(), pwl.ndim, pwl.points.len()),
        ("PWL", 1, 2)
    );
    assert!(close(num(&pwl.points[1].1), 100e9));
    assert_eq!(nets(inst(&ast, "G_Switch")), ["0", "be", "ctrl"]);
    let e = controlled("e1");
    assert_eq!(e.params.len(), 1);
    assert!(close(num(&e.pwl.as_ref().unwrap().points[0].0), -0.1));
    assert_eq!(controlled("G1").pwl.unwrap().points.len(), 2);
    let f = controlled("F1");
    assert_eq!((f.ctrl_sources.len(), f.params.len()), (2, 1));
    // printed, they parse back the same
    let first = ast.to_string();
    let (_dir2, parsed2, files2) = parse_text(&[("top.sp", &first)]).await;
    assert_eq!(diagnostics(&parsed2), [], "printed netlist:\n{first}");
    assert_eq!(files2.build(parsed2).0.to_string(), first);
}

/// HSPICE reads the first line of the netlist as its title (62 decks in the
/// HSPICE corpus had a title reported as a syntax error, and titles such as
/// `DIC Lab 7: ring` were read as elements). An included file has no title.
#[tokio::test]
async fn first_line_is_the_title() {
    let (_dir, parsed, files) = parse_deck(&[
        ("top.sp", "DIC Lab 7: ring\nR1 a b 1k\n.inc 'lib.sp'\n"),
        ("lib.sp", "R2 a b 1k\n"),
    ])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("TitleIsElement", 1)]);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(ast.title.as_deref(), Some("DIC Lab 7: ring"));
    assert_eq!(names(&ast), ["R1", "R2"]);
    // the title is printed on the first line, and read back as the title
    let printed = ast.to_string();
    assert!(printed.starts_with("DIC Lab 7: ring\n"), "{printed}");

    // a comment, a dot-command or a blank first line is not a title
    for text in [
        "* deck\nR1 a b 1k\n",
        ".param p=1\nR1 a b 1k\n",
        "\nR1 a b 1k\n",
    ] {
        let (_dir, parsed, files) = parse_deck(&[("top.sp", text)]).await;
        assert_eq!(diagnostics(&parsed), []);
        let (ast, _) = files.build(parsed);
        assert_eq!(
            (ast.title.as_deref(), names(&ast)),
            (None, vec!["R1".to_owned()])
        );
    }
    // a title that is not an element: no warning
    let (_dir, parsed, files) =
        parse_deck(&[("top.sp", "mux 2 pra 1 com transmission gate\nR1 a b 1k\n")]).await;
    assert_eq!(diagnostics(&parsed), []);
    assert_eq!(names(&files.build(parsed).0), ["R1"]);
}

/// HSPICE also reads `(`, `)` and `,` as delimiters (about 150 `X` lines and
/// some `E` and `.SUBCKT` lines in the HSPICE corpus were rejected).
#[tokio::test]
async fn parentheses_and_commas_are_delimiters() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".SUBCKT (via) in out zvia=45 tpropvia=15p\nR1 in out 1\n.ENDS (via)\n\
         Xvn7    jn13  jn14  (via)\t\t* Package via\n\
         X_U1          ESDP N24207 OPA (FLW=10 GLF=0.0565  RNV=9.617)\n\
         xpmos0 (vout vin vvdd vvdd) DGPMOS\n\
         E1 vdiff 0 (out1,out2) 1\n\
         W1 a 0 b 0\n+ N=1 LAYERSTACK=stack_1, FSOPTIONS=opt1\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert!(ast.subckt.contains_key("via"));
    let subckt = |name: &str| match &inst(&ast, name).ctx {
        InstanceCtx::Subckt(x) => (x.cktname.to_string(), x.nodes.len(), x.params.len()),
        _ => panic!(),
    };
    assert_eq!(subckt("Xvn7"), ("via".to_owned(), 2, 0));
    assert_eq!(subckt("X_U1"), ("OPA".to_owned(), 2, 3));
    assert_eq!(subckt("xpmos0"), ("DGPMOS".to_owned(), 4, 0));
    assert_eq!(nets(inst(&ast, "E1")), ["0", "out1", "out2", "vdiff"]);
    let InstanceCtx::Unknown { params, .. } = &inst(&ast, "W1").ctx else {
        panic!()
    };
    assert_eq!(params.len(), 3);
}

/// Statements found in the HSPICE corpus: `.lib "file"tt` without a space;
/// functions and optimization parameters in `.param`; a `PWL` source read
/// from a file; a `*` comment after a statement; command short forms and
/// variation blocks; a Windows path in `.inc`.
#[tokio::test]
async fn more_hspice_statements() {
    let (_dir, parsed, files) = parse_text(&[
        (
            "top.sp",
            ".lib \"models.l\"tt\n\
             .PARAM a1=11 square(X) = 'X*X' a2=3\n\
             .param fs(x) = 'x*2*pi' DelayTime = Opt1 ( 0n, 0n, 5n )\n\
             Vsig1 sig1 0 PWL PWLFILE=str(wav_left)  TD=1n\n\
             cload out gnd 1000f *output loading\n\
             .Local_Variation\n.End_Local_Variation\n\
             .mea tran t1 trig v(a) val=0.5 rise=1 targ v(b) val=0.5 rise=1\n\
             .inc \".\\sub.sp\"\n",
        ),
        ("models.l", ".lib tt\nR1 a b 1k\n.endl tt\n"),
        ("sub.sp", "R2 a b 1k\n"),
    ])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R1", "Vsig1", "cload", "R2"]);
    let keys: Vec<_> = ast.param.iter().map(|kv| kv.k.to_string()).collect();
    assert_eq!(keys, ["a1", "a2", "DelayTime"]);
    let funcs: Vec<_> = ast.param_func.iter().map(|f| f.name.to_string()).collect();
    assert_eq!(funcs, ["square", "fs"]);
    let InstanceCtx::Voltage(v) = &inst(&ast, "Vsig1").ctx else {
        panic!()
    };
    assert_eq!(v.source.func.as_ref().map(|f| &*f.name), Some("PWL"));
    assert!(func_args(&v.source).is_empty());
    assert_eq!(v.source.params[0].k, "PWLFILE");
    assert!(matches!(&v.source.params[0].v, Value::Expr(e) if e == "str(wav_left)"));
    assert!(
        ast.to_string()
            .contains("Vsig1 sig1 0 PWL PWLFILE=str(wav_left) TD="),
        "{ast}"
    );
    let InstanceCtx::Capacitor(c) = &inst(&ast, "cload").ctx else {
        panic!()
    };
    assert!(close(value_of(&c.value), 1e-12));
}

/// A block encrypted for HSPICE (`.prot freelib`) used to give one error per
/// line (over a million in the HSPICE corpus); a plain `.prot` block is
/// parsed as usual.
#[tokio::test]
async fn encrypted_block_is_skipped() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "R1 a b 1k\n.PROT freelib\nH]=3g\\#/%M_lgOq+y\neGLW-[=:\\>al\n.UNPROT\n\
         .protect\nR2 b c 1k\n.unprotect\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("Encrypted", 2)]);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(names(&ast), ["R1", "R2"]);
}

/// Every element is stored in the AST: keep them small (they were 272 and
/// 232 bytes before the `PWL` sources were boxed).
#[test]
fn element_sizes() {
    use crate::instance::{Instance, InstanceBuilder};
    println!(
        "InstanceBuilder: {}, Instance: {}",
        std::mem::size_of::<InstanceBuilder>(),
        std::mem::size_of::<Instance<'_>>()
    );
    assert!(std::mem::size_of::<InstanceBuilder>() <= 232);
    assert!(std::mem::size_of::<Instance<'_>>() <= 232);
}

/// A file included from several places at once used to be read and parsed
/// once per include.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_included_twice_is_parsed_once() {
    let (_dir, parsed, files) = parse_text(&[
        ("top.sp", ".inc 'leaf.sp'\n.inc 'leaf.sp'\n.inc 'leaf.sp'\n"),
        ("leaf.sp", "R1 a b 1k\n"),
    ])
    .await;
    assert_eq!(parsed.inner.len(), 3);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R1", "R1", "R1"]);
}

/// At most `MAX_DIAGNOSTICS_PER_FILE` diagnostics are logged per file (a
/// Liberty file read as a netlist used to log hundreds of thousands); all of
/// them still count.
#[test]
fn diagnostics_are_capped_per_file() {
    use crate::{MAX_DIAGNOSTICS_PER_FILE, Reporter};
    let file_id: FileId = PathBuf::from("x.sp").into();
    let mut reporter = Reporter::default();
    for _ in 0..MAX_DIAGNOSTICS_PER_FILE + 50 {
        reporter.report(&ParseErrorInner::TrailingText.with(None), &file_id, "");
    }
    assert_eq!(reporter.counts[&file_id], (MAX_DIAGNOSTICS_PER_FILE, 50, 0));
    assert!(reporter.finish());
}

/// More forms from the HSPICE corpus: `~` in node names (schematic exports),
/// `,` between the points of a `PWL` source without parentheses, the nodes
/// of a source or resistor in parentheses, `,` between `.model` parameters,
/// and a value on the continuation line after `key=`.
#[tokio::test]
async fn more_delimiters_and_continuations() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "XI24 temp1 ~s0 ~s1 net90 vdd vss / nand3 n_width=29.44u\n\
         VC clk gnd pwl 0n 0.0, 4n 0.0, 4.05n 5.0\n\
         vdd (vdd 0) 1\n\
         R9 (a b) 1k\n\
         .model sample_brd W modeltype=RLGC N=1\n\
         + Lo=\n\
         +   3.381601808482054e-007\n\
         + Co=1p\n\
         .model diff_stripline W MODELTYPE=FieldSolver, LAYERSTACK=stack_1, FSOPTIONS=opt1\n\
         .model nch nmos (level=54\n+ vth0=0.3)\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["XI24", "VC", "vdd", "R9"]);
    assert_eq!(
        nets(inst(&ast, "XI24")),
        ["net90", "temp1", "vdd", "vss", "~s0", "~s1"]
    );
    let InstanceCtx::Voltage(v) = &inst(&ast, "VC").ctx else {
        panic!()
    };
    assert_eq!(func_args(&v.source).len(), 6);
    assert_eq!(nets(inst(&ast, "vdd")), ["0", "vdd"]);
    assert_eq!(nets(inst(&ast, "R9")), ["a", "b"]);
    let params: Vec<_> = ast.model.iter().map(|m| m.params.len()).collect();
    assert_eq!(params, [4, 3, 2]);
    assert!(close(num(&ast.model[0].params[2].v), 3.381601808482054e-7));
}

/// Found by fuzzing: both CDL forms of `/` at once, and a lone `+` on the
/// last line (an empty continuation line, as it is on any other line).
#[tokio::test]
async fn fuzzed_round_trip_cases() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "Xcdl a<0> b VDD! 0 / /INV\n.meas tran t1 trig v(a) val=1\n+ targ v(b) val=1\n+",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let InstanceCtx::Subckt(x) = &inst(&ast, "Xcdl").ctx else {
        panic!()
    };
    assert_eq!((x.nodes.len(), x.cktname.as_ref()), (4, "INV"));
    let first = ast.to_string();
    let (_dir2, parsed2, files2) = parse_deck(&[("top.sp", &first)]).await;
    assert_eq!(diagnostics(&parsed2), []);
    assert_eq!(files2.build(parsed2).0.to_string(), first);
}
