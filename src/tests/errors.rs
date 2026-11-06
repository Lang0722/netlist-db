use super::*;

// error recovery

/// One unparseable statement used to erase the whole file's AST, including
/// everything parsed before it.
#[tokio::test]
async fn bad_statement_keeps_rest_of_file() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "M1 out in vss vss nch W=1u L=0.1u\n\
         M2 d g\n\
         M3 x y z z nch W=1u L=0.1u\n\
         .subckt INV a y\n\
         M4 y a vss vss nch\n\
         .ends INV\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("NodeCount", 2)]);
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(names(&ast), ["M1", "M3"]);
    assert!(ast.subckt.contains_key("inv"));
}

/// Left-over text used to be parsed as a new statement: `PWL 0 0 1n 1.8`
/// produced a fake instance named `0`, and errors pointed at the next line.
#[tokio::test]
async fn leftover_text_is_reported_not_reparsed() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "M1 d g s b nch W=1u L=0.1u =junk\n\
         + W2=2u\n\
         M2 d g s b nch W=1u L=0.1u\n\
         X1\n\
         M3 d g s b nch W=1u L=0.1u\n",
    )])
    .await;
    // `X1` fails while looking at the next line: still reported on line 4
    assert_eq!(
        diagnostics(&parsed),
        [diag("Trailing", 1), diag("Syntax", 4)]
    );
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["M2", "M3"]);
}

/// A group can start a value without a key too, and a `.endl` needs the end
/// of a word: `.endlx` is an unknown command, as on HSPICE (probes on
/// P-2019.06).
#[tokio::test]
async fn group_first_value_and_endl_word() {
    let (_dir, parsed, files) = parse_text(&[
        ("top.sp", "R1 a b (3k)\nV1 a 0 (1)\n.lib 'm.l' tt\n"),
        ("m.l", ".lib tt\nR2 a b 1\n.endlx\nR3 a b 3\n.endl tt\n"),
    ])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("UnknownCommand", 3)]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R1", "V1", "R2", "R3"]);
    assert_eq!(ast.unknown.len(), 1);
    // HSPICE rejects a group in a `.temp` line, and reads a value in quotes
    for (text, errors) in [
        (".temp (50)\n", 1),
        (".temp (25+25)\n", 1),
        (".temp 25 (50)\n", 1),
        (".temp '50'\n", 0),
        (".temp 50 $ (a comment)\n", 0),
    ] {
        let (_dir, parsed, _) = parse_text(&[("top.sp", text)]).await;
        assert_eq!(diagnostics(&parsed).len(), errors, "{text}");
    }
}

/// A line inside quotes that is not a `+` line: HSPICE rejects the deck
/// (golden `quoted_cmtline`, `quoted_blank_line`, `quoted_spaces_elem`,
/// `quoted_dollarline_param`). Outside quotes these lines are passed over.
#[tokio::test]
async fn line_inside_quotes_is_an_error() {
    for (text, line) in [
        (".param b='2\n\n+ *3'\n", 1),
        ("R0 a b 1\n.param b='2\n* c\n+ *3'\n", 2),
        (".param b='2\n$ c\n+ *3'\n", 1),
        ("R1 a b '2\n  \t\n+ *3'\n", 1),
        ("M1 d g 0 0 a l='2\n\n+ *1u'\n", 1),
        (".subckt s a w='2\n\n+ *3'\n.ends\n", 1),
        (".model a nmos level='2\n\n+ *3'\n", 1),
    ] {
        let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
        assert_eq!(diagnostics(&parsed), [diag("LineInQuotes", line)], "{text}");
        assert!(files.build(parsed).1, "{text}");
    }
    for text in [
        ".param b='2\n+ *3'\n",
        ".param b=2\n\n+ c=3\n",
        ".param b=2\n* it's\n+ c=3\n",
        ".param b=2 $ it's\n\n+ c=3\n",
        ".param b=2 * it's\n\n.param c=3\n",
        "R1 a b 1k $ it's\n\nR2 a b 1k\n",
        ".param p=max(1,\n\n+ 3)\n",
    ] {
        let (_dir, parsed, _) = parse_text(&[("top.sp", text)]).await;
        assert_eq!(diagnostics(&parsed), [], "{text}");
    }
}

/// A `.ends` without `.subckt` used to end the file, silently.
#[tokio::test]
async fn stray_ends_does_not_truncate_the_file() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "M1 d g s b nch\n.ends\nM2 d g s b nch\n.subckt INV a y\nM3 y a 0 0 nch\n.ends INV\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("StrayEnds", 2)]);
    let (ast, has_err) = files.build(parsed);
    // (HSPICE stops at it with a syntax error)
    assert!(has_err);
    assert_eq!(names(&ast), ["M1", "M2"]);
    assert!(ast.subckt.contains_key("inv"));
}

#[tokio::test]
async fn unterminated_subckt_is_reported() {
    let (_dir, parsed, files) =
        parse_text(&[("top.sp", "R0 a b 1k\n.subckt OPEN a b\nR1 a b 1k\n")]).await;
    assert_eq!(diagnostics(&parsed), [diag("UnterminatedSubckt", 2)]);
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(ast.subckt["open"].ast.instance.len(), 1);
}

/// A second definition used to replace the first one silently.
#[tokio::test]
async fn duplicate_subckt_warns() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".subckt INV a y\nM1 y a 0 0 nch\n.ends\n.subckt inv a y\nR1 a y 1k\n.ends\n",
    )])
    .await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err, "a warning is not an error");
    assert_eq!(ast.subckt.len(), 1);
    // unchanged behavior: the later definition wins
    assert!(matches!(
        ast.subckt["inv"].ast.instance[0].ctx,
        InstanceCtx::Resistor(_)
    ));
}

/// Blocks holding only `.ic`/`.nodeset` used to be dropped as "empty".
#[tokio::test]
async fn ic_and_nodeset_are_kept() {
    for text in [
        ".ic V(a)=1\n.nodeset b=0.5\n",
        ".ic V(a)=1\n.nodeset b=0.5\n.inc 'small.sp'\n",
    ] {
        let (_dir, parsed, files) =
            parse_text(&[("top.sp", text), ("small.sp", "R0 a b 1k\n")]).await;
        assert_eq!(diagnostics(&parsed), []);
        let (ast, _) = files.build(parsed);
        assert_eq!(ast.init_condition.len(), 1, "{text}");
        assert_eq!(ast.nodeset.len(), 1, "{text}");
    }
}

#[tokio::test]
async fn missing_top_file_keeps_the_others() {
    let dir = TempDir::new("missing");
    let good = dir.write("good.sp", "* good\nR0 a b 1k\n");
    let (parsed, files) = parse_files(&[dir.0.join("missing.sp"), good]).await;
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(names(&ast), ["R0"]);
}
