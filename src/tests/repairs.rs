use super::*;

// repairs of wrong results, crashes and very long runs
/// A deck whose instance lines are all in an `.if` block is a circuit, not a
/// library: the tool that looks for a library must not skip these lines.
#[tokio::test]
async fn lines_in_a_top_level_if_are_not_a_library() {
    let is_library = async |text: &str| {
        let (_dir, parsed, files) = parse_deck(&[("deck.sp", text)]).await;
        files.build(parsed).0.is_library()
    };
    let cell = ".subckt s d\nM1 d d 0 0 n l=1u w=1u\n.ends\n";
    assert!(is_library(&format!("* t\n{cell}")).await);
    assert!(is_library(&format!("* t\n.if (1)\n.param a=1\n.endif\n{cell}")).await);
    assert!(!is_library(&format!("* t\n{cell}X1 d s\n")).await);
    assert!(!is_library(&format!("* t\n{cell}.if (1)\nX1 d s\n.endif\n")).await);
    let nested = ".if (0)\n.else\n.if (1)\nM1 d d 0 0 n l=1u w=1u\n.endif\n.endif\n";
    assert!(!is_library(&format!("* t\n{nested}")).await);
}

/// `.unprot` has short forms (see `canonical_cmd`): each one ends an encrypted
/// block, and the rest of the file is read.
#[tokio::test]
async fn encrypted_block_ends_at_a_short_form() {
    for end in [".unprot", ".UNPR", ".unp", ".unprotect"] {
        let text = format!("R1 a b 1k\n.prot freelib\nH]=3g\\#\n{end}\nR2 b c 1k\n");
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        assert_eq!(diagnostics(&parsed), [diag("Encrypted", 2)], "{end}");
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err, "{end}");
        assert_eq!(names(&ast), ["R1", "R2"], "{end}");
    }
}

/// With no `.unprot`, an encrypted block takes the rest of the file. The lines
/// of the rest are lost, so this is an error, as for a plain `.prot` block.
#[tokio::test]
async fn encrypted_block_with_no_end_is_an_error() {
    let text = "R1 a b 1k\n.prot freelib\nH]=3g\\#\nR2 b c 1k\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(names(&ast), ["R1"]);
}

/// A `\` continuation after a `$` that is in a name: the `$` starts no comment,
/// so the next line goes on with the statement.
#[tokio::test]
async fn continuation_after_a_dollar_in_a_name() {
    let rows =
        async |lines: &str| deck_rows(&format!("* t\n.model n nmos level=54\n{lines}")).await;
    let got = rows("M$1 d g 0 0 n l=1u \\\nw=2u\n").await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].0, "M$1");
    assert_eq!((got[0].2, got[0].3), (1.0, 2.0));
    // in a value of a parameter too
    let got = rows(".param a$b=3u\nM1 d g 0 0 n l=a$b \\\nw=2u\n").await;
    assert_eq!((got[0].2, got[0].3), (3.0, 2.0));
    // at the start of a word and after a number, a `$` still starts a comment,
    // and a `\` in the comment continues nothing
    for line in ["w=2u $ note", "w=2u$ note"] {
        let got = rows(&format!(
            "M1 d g 0 0 n l=1u {line} \\\nM2 d g 0 0 n l=1u w=2u\n"
        ))
        .await;
        assert_eq!(got.len(), 2, "{got:?}");
    }
}

/// Two names that differ in a byte that is not UTF-8 are two names: `X1` must
/// call the subckt that it names.
#[tokio::test]
async fn names_with_other_bytes_stay_apart() {
    let dir = TempDir::new("bytes");
    let deck = dir.0.join("deck.sp");
    std::fs::write(
        &deck,
        b"* t\n.subckt \xe9 d\nM1 d d 0 0 one l=1u w=1u\n.ends\n\
          .subckt \xe8 d\nM1 d d 0 0 two l=1u w=1u\n.ends\n\
          X1 a \xe9\nX2 a \xe8\n\
          .model one nmos level=54\n.model two nmos level=54\n",
    )
    .unwrap();
    let (parsed, files) = parse_files(&[deck]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.subckt.len(), 2);
    let rows = ast.mosfet_models();
    let model = |row: usize| rows[row].selection.as_ref().unwrap().model.name.to_string();
    assert_eq!((model(0).as_str(), model(1).as_str()), ("one", "two"));
}

/// A definition that an instance path re-enters: the counts of a subckt depend
/// on the path above it, so a count kept from another path is wrong. `usage_agrees`
/// (in `deck_rows`) compares the summary with the list.
#[tokio::test]
async fn usage_of_a_definition_that_a_path_reenters() {
    let defs = ".subckt a p q\nxs p q s\n.ends\n\
                .subckt s p q\n.subckt s p q\nM1 p q p q nch l=1u w=1u\n.ends\nxa p q a\n.ends\n\
                .model nch nmos level=54\n";
    for lines in ["X1 a b s\nx2 1 0 a\n", "x2 1 0 a\nX1 a b s\n"] {
        let deck = format!("* t\n{lines}{defs}");
        let rows = deck_rows(&deck).await;
        let bad = rows.iter().filter(|r| r.1.starts_with("error")).count();
        assert_eq!((rows.len(), bad), (2, 1), "{deck}: {rows:?}");
    }
    // a re-entry with other lines: a sibling reaches the definition from the
    // other side, at the same context
    let deck = "* t\nX1 a\nX2 b\n\
                .subckt a p\nM1 p p p p nch l=1u w=1u\nxb p b\n.ends\n\
                .subckt b p\nM2 p p p p nch l=1u w=1u\nxa p a\n.ends\n\
                .model nch nmos level=54\n";
    deck_rows(deck).await;
}

/// A definition under a re-entry keeps its memo when no path can re-enter it.
#[tokio::test]
async fn usage_keeps_counts_of_a_definition_outside_a_cycle() {
    let (_dir, parsed, files) = parse_deck(&[(
        "deck.sp",
        "* t\nX1 a\nX2 a\n\
         .subckt a p\nxl p leaf\nxb p b\n.ends\n\
         .subckt b p\nxl p leaf\nxa p a\n.ends\n\
         .subckt leaf p\nM1 p p p p nch l=1u w=1u\n.ends\n\
         .model nch nmos level=54\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    let names: HashSet<String> = ast
        .reentrant_defs()
        .into_iter()
        .map(|d| d.name.to_lowercase())
        .collect();
    assert_eq!(names, HashSet::from(["a".to_owned(), "b".to_owned()]));
}

/// A chain of parameters across levels must not end the process, and must give
/// the value: each level's chain reads the one of the level above.
#[tokio::test]
async fn long_chain_of_parameters_across_levels() {
    use core::fmt::Write as _;
    let (levels, links) = (100, 70);
    let mut deck = String::from("* t\n.model n nmos level=54\n.param p0_0=3u\nX0 d s1\n");
    for level in 1..levels {
        let _ = writeln!(deck, ".subckt s{level} d");
        for k in 0..links - 1 {
            let _ = writeln!(deck, ".param p{level}_{k}='p{level}_{}'", k + 1);
        }
        let last = links - 1;
        let _ = writeln!(deck, ".param p{level}_{last}='p{}_0'", level - 1);
        if level + 1 < levels {
            let _ = writeln!(deck, "X0 d s{}", level + 1);
        } else {
            let _ = writeln!(deck, "M1 d d 0 0 n l=p{level}_0 w=1u");
        }
        let _ = writeln!(deck, ".ends");
    }
    let rows = deck_rows(&deck).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].2, 3.0, "{:?}", rows[0]);
}

/// A chain of function calls that fans out repeats the same calls: it gives the
/// value in short time. HSPICE gives the value too (decks `fn_fan_*`).
#[tokio::test]
async fn function_calls_that_fan_out_give_the_value() {
    use core::fmt::Write as _;
    let mut deck = String::from("* t\n.model n nmos level=54\n");
    for i in 0..40 {
        let _ = writeln!(deck, ".param f{i}(x)='f{}(x)+f{}(x)'", i + 1, i + 1);
    }
    deck.push_str(".param f40(x)='x'\nM1 d d 0 0 n l='f0(1f)' w=1u\n");
    let rows = deck_rows(&deck).await;
    assert_eq!(rows.len(), 1);
    assert!((rows[0].2 - 1.099_511_627_776e3).abs() < 1e-3, "{rows:?}");
}

/// Calls that fan out with other values each time cannot repeat: they stop with
/// an error, not with a run that never ends.
#[tokio::test]
async fn function_calls_with_new_values_stop() {
    use core::fmt::Write as _;
    let mut deck = String::from("* t\n.model n nmos level=54\n");
    for i in 0..60 {
        let _ = writeln!(deck, ".param f{i}(x)='f{}(x*2+1)+f{}(x*3+1)'", i + 1, i + 1);
    }
    deck.push_str(".param f60(x)='x'\nM1 d d 0 0 n l='f0(1f)' w=1u\n");
    let rows = deck_rows(&deck).await;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].1.contains("too many"), "{rows:?}");
}

/// A name of a parameter with a byte that is not UTF-8 can be read in an
/// expression, and two such names stay two names.
#[tokio::test]
async fn expressions_read_names_with_other_bytes() {
    let dir = TempDir::new("bytes");
    let deck = dir.0.join("deck.sp");
    std::fs::write(
        &deck,
        b"* t\n.model n nmos level=54\n.param p\xe9=1u p\xe8=2u\n\
          M1 d g 0 0 n l=p\xe9 w='p\xe8'\n",
    )
    .unwrap();
    let (parsed, files) = parse_files(&[deck]).await;
    let (ast, _) = files.build(parsed);
    let rows = ast.mosfet_models();
    let chosen = rows[0].selection.as_ref().unwrap();
    assert_eq!((chosen.l, chosen.w), (1e-6, 2e-6));
}

/// A deep chain of includes must not end the process: past the limit it is an
/// error. (A big stack: the frames of a debug build are big.)
#[test]
fn include_chain_has_a_limit() {
    let chain = |n: usize| {
        (0..n)
            .map(|i| {
                (
                    format!("f{i}.sp"),
                    format!("R{i} a b 1\n.inc 'f{}.sp'\n", i + 1),
                )
            })
            .chain([(format!("f{n}.sp"), String::from("R a b 1\n"))])
            .collect::<Vec<_>>()
    };
    let build = async |files: Vec<(String, String)>| {
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(n, c)| (n.as_str(), c.as_str()))
            .collect();
        let (_dir, parsed, files) = parse_text(&refs).await;
        let (ast, has_err) = files.build(parsed);
        (ast.instance.len(), has_err)
    };
    let test = async {
        // (HSPICE reads a chain of 1000 files: deck `i_chain_1000`)
        assert_eq!(build(chain(1000)).await, (1001, false));
        let n = crate::parser::MAX_INCLUDE_DEPTH + 10;
        let (lines, has_err) = build(chain(n)).await;
        assert!(has_err);
        assert!(lines < n, "{lines}");
    };
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn_scoped(scope, || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(test);
            })
            .unwrap()
            .join()
            .unwrap();
    });
}

/// Files that include the next one twice expand to 2^n copies. Past the limit,
/// the build stops following includes, with an error.
#[tokio::test]
async fn include_expansion_has_a_limit() {
    let mut files: Vec<(String, String)> = (0..40)
        .map(|i| {
            let next = i + 1;
            (
                format!("f{i}.sp"),
                format!(".inc 'f{next}.sp'\n.inc 'f{next}.sp'\n"),
            )
        })
        .collect();
    files.push(("f40.sp".to_owned(), "R a b 1\n".to_owned()));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, c)| (n.as_str(), c.as_str()))
        .collect();
    let (_dir, parsed, built) = parse_text(&refs).await;
    let (ast, has_err) = built.build(parsed);
    assert!(has_err);
    assert!(ast.instance.len() < 1 << 30);
}

/// A file included many times, within the limit, stays correct.
#[tokio::test]
async fn include_repeated_within_the_limit() {
    let top = ".inc 'a.sp'\n".repeat(1000);
    let (_dir, parsed, files) = parse_text(&[("top.sp", &top), ("a.sp", "R1 a b 1\n")]).await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(ast.instance.len(), 1000);
}

/// A `$` in a name must not take quadratic time.
#[tokio::test]
async fn many_dollars_in_one_word_are_read_in_linear_time() {
    let word = "a$".repeat(200_000);
    let deck = format!("* t\n.param {word}=2u\n.param b={word}\n.param c={word}*2\n");
    let start = std::time::Instant::now();
    let (_dir, parsed, files) = parse_deck(&[("deck.sp", &deck)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.param.len(), 3);
    assert!(start.elapsed().as_secs() < 5, "{:?}", start.elapsed());
}

/// An include of a thing that is not a file (a device, a FIFO) is an error:
/// reading `/dev/zero` never ends.
#[cfg(unix)]
#[tokio::test]
async fn include_of_a_device_is_an_error() {
    let (_dir, parsed, files) = parse_text(&[("top.sp", "R1 a b 1\n.inc '/dev/zero'\n")]).await;
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(ast.instance.len(), 1);
}

/// A row of a table that has fewer values than the header has no value there.
#[test]
fn short_row_of_a_measure_table() {
    use crate::parser::MeasCols;
    let table = vec![vec![Some(1.0), Some(2.0)], vec![Some(3.0)]];
    let mut cols = MeasCols::new(vec!["a", "b"], &table, None);
    let (_, a) = cols.next().unwrap();
    assert_eq!(a.collect::<Vec<_>>(), [Some(1.0), Some(3.0)]);
    let (_, b) = cols.next().unwrap();
    assert_eq!(b.collect::<Vec<_>>(), [Some(2.0), None]);
}

/// A file that two lines read keeps its statements for the second read, and so
/// does a file that it reads.
#[tokio::test]
async fn file_read_twice_keeps_its_statements() {
    let (_dir, parsed, files) = parse_text(&[
        (
            "top.sp",
            "R1 a b 1\n.inc 'twice.sp'\n.inc 'once.sp'\n.inc 'twice.sp'\n",
        ),
        ("twice.sp", "R2 a b 1\n.inc 'leaf.sp'\n"),
        ("leaf.sp", "R3 a b 1\n"),
        ("once.sp", "R4 a b 1\n"),
    ])
    .await;
    let (ast, has_err) = files.build(parsed);
    let names: Vec<&str> = ast.instance.iter().map(|i| &*i.name).collect();
    assert!(!has_err);
    assert_eq!(names, ["R1", "R2", "R3", "R4", "R2", "R3"]);
}
