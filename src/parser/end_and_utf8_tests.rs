use super::recover_tests::{TempDir, error_msgs, errors, names, opts};
use super::*;
use crate::{TitleLine, ast::Value};
use std::path::Path;

async fn parse_path(path: &Path, title_line: TitleLine) -> (Parsed, Files) {
    parse_top_multi_with(
        [FileId::Include { path: path.into() }].into_iter(),
        &opts(title_line),
    )
    .await
}

fn top(parsed: &Parsed) -> &ASTBuilder {
    &parsed.inner[parsed.top_ids[0].0].1
}

#[tokio::test]
async fn after_end() {
    // Golden `after_end`: the lines after `.end` of the top file are
    // not read (r1 uses p=1k), and HSPICE gives no warning.
    let path = PathBuf::from("golden/cases/after_end/deck.sp");
    let (parsed, files) = parse_path(&path, TitleLine::Auto).await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let end = top(&parsed).after_end.expect("after_end");
    assert_eq!((end.line, end.lines), (6, 3));
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["v1", "r1"]);
    assert_eq!(ast.param.len(), 1);
    assert!(matches!(ast.param[0].v, Value::Num(f) if f == 1e3));

    let tmp = TempDir::new("after_end");
    let dir = &tmp.0;
    let main = dir.join("main.sp");
    for (text, want, end) in [
        // only comments and empty lines after `.end`
        (
            "* t\nR1 a b 1\n.END\n* c\n\n  $ d\n",
            &["R1"][..],
            Some((3, 0)),
        ),
        // `.end` with a name, and no newline at the end
        ("* t\nR1 a b 1\n.end x\nR2 a b 2", &["R1"], Some((3, 1))),
        // an open `.subckt` is closed at `.end`
        (
            "* t\n.subckt s a\nR1 a b 1\n.end\nR2 a b 2\n.ends\n",
            &[],
            Some((4, 2)),
        ),
        // no `.end`
        ("* t\nR1 a b 1\n", &["R1"], None),
        // a `.lib` section after `.end` is read by its `.lib` calls,
        // so its lines are not counted (`.LIB` with a `$` comment too)
        (
            "* t\n.lib 'main.sp' tt\n.end\n.lib tt\n.param p=1\n.endl tt\nR2 a b 2\n.LIB ff $ c\nR3 a b 3\n.ENDL\n",
            &[],
            Some((3, 1)),
        ),
    ] {
        std::fs::write(&main, text).unwrap();
        let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
        let got = top(&parsed).after_end.map(|e| (e.line, e.lines));
        assert_eq!(got, end, "{text:?}");
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), want, "{text:?}");
    }
    // An open `.subckt` at `.end` has no `.ends`.
    std::fs::write(&main, "* t\n.subckt s a\nR1 a b 1\n.end\nR2 a b 2\n.ends\n").unwrap();
    let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
    assert_eq!(error_msgs(&parsed), ["`.subckt s` has no `.ends`"]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.subckt["s"].ast.instance), ["R1"]);

    // `.end` in an included file does not end the top file, and the
    // included file is read past it.
    std::fs::write(dir.join("inc.sp"), "R1 a b 1\n.end\nR2 a b 2\n").unwrap();
    std::fs::write(&main, "* t\n.inc 'inc.sp'\nR3 a b 3\n.end\nR4 a b 4\n").unwrap();
    let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let inc = parsed
        .inner
        .iter()
        .find(|(id, _)| id.path().ends_with("inc.sp"))
        .unwrap();
    assert!(inc.1.after_end.is_none());
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R1", "R2", "R3"]);
}

#[tokio::test]
async fn non_utf8() {
    // Golden `nonutf8_comment`: a Latin-1 byte in a comment; HSPICE
    // reads the deck with no warning.
    let path = PathBuf::from("golden/cases/nonutf8_comment/deck.sp");
    let (parsed, files) = parse_path(&path, TitleLine::Auto).await;
    // (the parser gives one warning, and no error)
    assert!(errors(&parsed).iter().all(|e| e.is_warning()));
    assert_eq!(top(&parsed).non_utf8, Some(2));
    assert!(files.inner[parsed.top_ids[0].0].contains("caf\u{F7E9} "));
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["v1", "r1"]);

    // A bad byte in a statement: the statement is kept.
    let tmp = TempDir::new("non_utf8");
    let dir = &tmp.0;
    let main = dir.join("main.sp");
    std::fs::write(&main, b"* t\nR1 a b 1\nR\xe92 a b 2\n.inc 'inc.sp'\n").unwrap();
    std::fs::write(dir.join("inc.sp"), b"R3 a b 3\n\n* \xff\xfe\nR4 a b 4\n").unwrap();
    let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
    assert!(errors(&parsed).iter().all(|e| e.is_warning()));
    assert_eq!(top(&parsed).non_utf8, Some(3));
    let inc = parsed
        .inner
        .iter()
        .find(|(id, _)| id.path().ends_with("inc.sp"))
        .unwrap();
    assert_eq!(inc.1.non_utf8, Some(3));
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R1", "R\u{F7E9}2", "R3", "R4"]);

    // A `.lib` section: the first bad line in the section, and none
    // when the bad bytes are outside it.
    std::fs::write(
        dir.join("m.lib"),
        b"* \xe9\n.lib tt\nR5 a b 5\n.endl tt\n.lib ff\nR6 a b 6\n* \xe9\nR\xe97 a b 7\n.endl ff\n",
    )
    .unwrap();
    std::fs::write(&main, "* t\n.lib 'm.lib' tt\n.lib 'm.lib' ff\n").unwrap();
    let (parsed, _) = parse_path(&main, TitleLine::Auto).await;
    let section = |name: &str| {
        parsed
            .inner
            .iter()
            .find(|(id, _)| matches!(id, FileId::Section { section, .. } if section == name))
            .map(|(_, ast)| ast.non_utf8)
            .unwrap()
    };
    assert_eq!(section("tt"), None);
    assert_eq!(section("ff"), Some(7));

    // Valid UTF-8 has no mark.
    std::fs::write(&main, "* t\nR1 a b 1 $ caf\u{e9}\n").unwrap();
    let (parsed, _) = parse_path(&main, TitleLine::Auto).await;
    assert_eq!(top(&parsed).non_utf8, None);
}

#[tokio::test]
async fn include_cycles() {
    // a -> b -> c -> a, and the top file includes `a` and `c`. The parse
    // reads each file once, so which include line closed the cycle (a
    // parse error) depended on the order in which the files were read.
    // Now no include is a parse error, and `Files::build` finds the
    // cycle in document order: the same result in every run.
    let tmp = TempDir::new("include_cycles");
    let dir = &tmp.0;
    for (name, text) in [
        ("main.sp", "* t\n.inc 'a.sp'\n.inc 'c.sp'\n"),
        ("a.sp", ".inc 'b.sp'\nra 1 0 1\n"),
        ("b.sp", ".inc 'c.sp'\nrb 1 0 1\n"),
        ("c.sp", ".inc 'a.sp'\nrc 1 0 1\n"),
    ] {
        std::fs::write(dir.join(name), text).unwrap();
    }
    for _ in 0..20 {
        let (parsed, files) = parse_path(&dir.join("main.sp"), TitleLine::Auto).await;
        assert_eq!(parsed.inner.len(), 4);
        let include_errors = parsed
            .inner
            .iter()
            .flat_map(|(_, ast)| &ast.segments)
            .filter(|seg| {
                matches!(seg, Segment::Include { res, .. }
                        if res.get().is_none_or(Result::is_err))
            })
            .count();
        assert_eq!(include_errors, 0);
        let (ast, has_err) = files.build(parsed);
        assert!(has_err);
        // main -> a -> b -> c (its include of `a` closes a cycle), then
        // main -> c -> a -> b (its include of `c` closes one)
        assert_eq!(names(&ast.instance), ["rc", "rb", "ra", "rb", "ra", "rc"]);
    }
    // A file that includes itself.
    let main = dir.join("self.sp");
    std::fs::write(&main, "* t\nR1 a b 1\n.inc 'self.sp'\nR2 a b 2\n").unwrap();
    let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
    assert!(error_msgs(&parsed).is_empty());
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(names(&ast.instance), ["R1", "R2"]);
}

#[tokio::test]
async fn include_cycle_canonical_paths() {
    // `main.sp` -> `sub/b.sp` -> `../main.sp`: the path is spelled
    // another way, but it is the same file, so it is the same parse and
    // the include closes a cycle (each spelling was a new file before,
    // and the paths grew until "File name too long").
    let tmp = TempDir::new("cycle_canonical");
    let dir = &tmp.0;
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("main.sp"), "* t\n.inc 'sub/b.sp'\nR1 a b 1\n").unwrap();
    std::fs::write(dir.join("sub/b.sp"), "R2 a b 2\n.inc '../main.sp'\n").unwrap();
    let (parsed, files) = parse_path(&dir.join("main.sp"), TitleLine::Auto).await;
    assert_eq!(parsed.inner.len(), 2);
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(names(&ast.instance), ["R2", "R1"]);
    // Two spellings of one file that is not in a cycle: one parse,
    // read at each include (HSPICE reads a file again at each
    // `.include`).
    std::fs::write(dir.join("x.sp"), "R3 a b 3\n").unwrap();
    std::fs::write(
        dir.join("two.sp"),
        "* t\n.inc 'x.sp'\n.inc './sub/../x.sp'\n",
    )
    .unwrap();
    let (parsed, files) = parse_path(&dir.join("two.sp"), TitleLine::Auto).await;
    assert_eq!(parsed.inner.len(), 2);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(names(&ast.instance), ["R3", "R3"]);
}

/// The lines of the `.end` statements that `ast` (a file) ignored,
/// also in its `.subckt` bodies.
fn ignored_ends(ast: &ASTBuilder) -> Vec<u32> {
    let mut out = Vec::new();
    for seg in &ast.segments {
        if let Segment::Local(local) = seg {
            out.extend(&local.ignored_end);
            for s in &local.subckt {
                out.extend(ignored_ends(&s.ast));
            }
        }
    }
    out.sort_unstable();
    out
}

#[tokio::test]
async fn end_in_include() {
    // Golden `end_in_include`: HSPICE ignores a `.end` in an included
    // file, with a warning, and reads the lines after it. The parse
    // records the line, so the loader can warn too.
    let path = PathBuf::from("golden/cases/end_in_include/deck.sp");
    let (parsed, files) = parse_path(&path, TitleLine::Auto).await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let inc = parsed
        .inner
        .iter()
        .find(|(id, _)| id.path().ends_with("inc.sp"))
        .unwrap();
    assert_eq!(ignored_ends(&inc.1), [2]);
    assert!(ignored_ends(top(&parsed)).is_empty());
    assert_eq!(
        top(&parsed).after_end.map(|e| (e.line, e.lines)),
        Some((6, 0))
    );
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["v1", "r1", "r2", "r3"]);
    // In a `.subckt` body of an included file, and a file with only
    // `.end` (its segment is kept for the note).
    let tmp = TempDir::new("end_in_include");
    let dir = &tmp.0;
    std::fs::write(
        dir.join("inc.sp"),
        ".subckt s a\nR1 a b 1\n.END $ c\nR2 a b 2\n.ends\n",
    )
    .unwrap();
    std::fs::write(dir.join("only.sp"), "* c\n.end\n").unwrap();
    let main = dir.join("main.sp");
    std::fs::write(&main, "* t\n.inc 'inc.sp'\n.inc 'only.sp'\n").unwrap();
    let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let file = |name: &str| {
        &parsed
            .inner
            .iter()
            .find(|(id, _)| id.path().ends_with(name))
            .unwrap()
            .1
    };
    assert_eq!(ignored_ends(file("inc.sp")), [3]);
    assert_eq!(ignored_ends(file("only.sp")), [2]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.subckt["s"].ast.instance), ["R1", "R2"]);
}

#[tokio::test]
async fn block_recovery_stops_at_end() {
    // The skip of a failed block statement does not pass the `.end`
    // of the top file: HSPICE reads no line after it (golden
    // `after_end`, `after_end_second_deck`). The skip went on to the
    // `.ends` after `.end` before, so R3 was loaded and the `.end` was
    // lost. Now the block has no closing line, so only its first line is
    // skipped and R1 is read in the enclosing scope.
    let tmp = TempDir::new("block_end");
    let dir = &tmp.0;
    let main = dir.join("main.sp");
    for (text, want, end) in [
        (
            "* t\n.subckt\nR1 a b 1\n.end\nR2 a b 2\n.ends\nR3 a b 3\n",
            &["R1"][..],
            (4, 3),
        ),
        (
            "* t\n.SUBCKT\nR1 a b 1\n.END $ c\n.ENDS\nR3 a b 3\n",
            &["R1"],
            (4, 2),
        ),
        (
            "* t\n.data\nR1 a b 1\n.end\nR2 a b 2\n.enddata\nR3 a b 3\n",
            &["R1"],
            (4, 3),
        ),
        // a `.lib` section whose `.endl` line has extra text
        (
            "* t\n.lib tt\nR1 a b 1\n.end\nR2 a b 2\n.endl tt junk\nR3 a b 3\n",
            &["R1"],
            (4, 3),
        ),
        // a failed `.subckt` inside a body
        (
            "* t\n.subckt s a\n.subckt\nR1 a b 1\n.end\n.ends\n.ends\nR3 a b 3\n",
            &[],
            (5, 3),
        ),
        // a `.ends` before the `.end` still closes the failed block
        (
            "* t\n.subckt\nR1 a b 1\n.ends\nR2 a b 2\n.end\nR3 a b 3\n",
            &["R2"],
            (6, 1),
        ),
    ] {
        std::fs::write(&main, text).unwrap();
        let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
        let got = top(&parsed).after_end.map(|e| (e.line, e.lines));
        assert_eq!(got, Some(end), "{text:?}");
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), want, "{text:?}");
    }
    // A `.subckt` nested too deep, with no `.ends`: the skip stops at
    // `.end`, not at the end of the input.
    let mut text = String::from("* t\n");
    for k in 0..=MAX_NESTING {
        text += &format!(".subckt s{k} a\n");
    }
    text += ".end\nR9 a b 9\n";
    std::fs::write(&main, &text).unwrap();
    let (parsed, _) = parse_path(&main, TitleLine::Auto).await;
    let end_line = MAX_NESTING as u32 + 3;
    assert_eq!(
        top(&parsed).after_end.map(|e| (e.line, e.lines)),
        Some((end_line, 1))
    );
    // In an included file, `.end` is ignored, so the skip goes on to
    // the `.ends`.
    std::fs::write(
        dir.join("inc.sp"),
        ".subckt\nR1 a b 1\n.end\nR2 a b 2\n.ends\nR3 a b 3\n",
    )
    .unwrap();
    std::fs::write(&main, "* t\n.inc 'inc.sp'\nR4 a b 4\n").unwrap();
    let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R3", "R4"]);
}

#[tokio::test]
async fn block_scan_reads_as_main_parse() {
    // The main parse does not read the text of a `.prot` region or of
    // a `.lib <section>` definition, so a `.end` there does not end the
    // top file. The pass that finds the closing line of a failed block
    // reads the file the same way (it stopped at such a `.end`, so the
    // skip of the failed `.subckt` after it found no `.ends`: R3 joined
    // the top level, and its `.ends` was a `stray_ends`). Not probed.
    let tmp = TempDir::new("block_scan_unparsed");
    let main = tmp.0.join("main.sp");
    for (text, want, errors) in [
        (
            "* t\n.lib tt\nR1 a b 1\n.end\n.endl tt\n.subckt\nR3 a b 3\n.ends\nR4 a b 4\n",
            &["R4"][..],
            vec![6],
        ),
        (
            "* t\n.prot\nR1 a b 1\n.end\n.unprot\n.subckt\nR3 a b 3\n.ends\nR4 a b 4\n",
            &["R4"],
            vec![6],
        ),
        // `.ends` lines in such text do not close the failed block
        (
            "* t\n.subckt\nR1 a b 1\n.prot\n.ends\n.unprot\n.ends\nR3 a b 3\n",
            &["R3"],
            vec![2],
        ),
        (
            "* t\n.subckt\nR1 a b 1\n.lib tt\n.ends\n.endl\n.ends\nR3 a b 3\n",
            &["R3"],
            vec![2],
        ),
        // A `.lib` section that does not parse (text after its `.endl`)
        // is read line by line: its `.end` stops the skip (as in
        // `block_recovery_stops_at_end`).
        (
            "* t\n.lib tt\nR1 a b 1\n.end\n.endl tt junk\n.subckt\nR3 a b 3\n.ends\n",
            &["R1"],
            vec![2],
        ),
    ] {
        std::fs::write(&main, text).unwrap();
        let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
        assert_eq!(
            super::recover_tests::error_lines(&parsed),
            errors,
            "{text:?}"
        );
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), want, "{text:?}");
    }
}

#[tokio::test]
async fn end_line_with_text_ends_file() {
    // Golden `end_quoted_arg`, `end_kv_arg`: HSPICE ends the deck at a
    // `.end` line also when other text follows the keyword (`.end 'x'`,
    // `.end x=1`), with no warning. Such a line failed before
    // (`trailing_text`), and the lines after it were read. The pass
    // that finds the closing lines of blocks stopped at it, so the skip
    // of a `.subckt` nested too deep after it jumped back to it, and
    // the parse read the same lines again and again (a hang with
    // unbounded memory).
    for case in ["end_quoted_arg", "end_kv_arg"] {
        let path = PathBuf::from(format!("golden/cases/{case}/deck.sp"));
        let (parsed, files) = parse_path(&path, TitleLine::Auto).await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        // the second deck: 4 lines with statements
        let got = top(&parsed).after_end.map(|e| (e.line, e.lines));
        assert_eq!(got, Some((5, 4)), "{case}");
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["v1", "r1"], "{case}");
    }
    let tmp = TempDir::new("end_text");
    let main = tmp.0.join("main.sp");
    let deck = |before: usize, end: &str, after: usize| {
        let mut text = String::from("* t\n");
        for k in 0..before {
            text += &format!(".subckt p{k} a\n");
        }
        text += end;
        text += "\n";
        for k in 0..after {
            text += &format!(".subckt s{k} a\n");
        }
        text + "R1 a b 1\n"
    };
    let deep = MAX_NESTING + 2;
    for (text, end, missing) in [
        (deck(0, ".end 'x'", deep), (2, deep as u32 + 1), 0),
        (deck(0, ".end x=1", deep), (2, deep as u32 + 1), 0),
        (deck(100, ".END 'x' $ c", 200), (102, 201), 100),
        // the text after `.end` can go on on `+` lines
        (deck(0, ".end x\n+ y", 1), (2, 2), 0),
    ] {
        std::fs::write(&main, &text).unwrap();
        let (parsed, _) = parse_path(&main, TitleLine::Auto).await;
        let errors = super::recover_tests::errors(&parsed);
        assert_eq!(errors.len(), missing, "{text:?}");
        assert!(
            errors
                .iter()
                .all(|e| matches!(e.err, ParseErrorInner::MissingEnds { .. }))
        );
        let got = top(&parsed).after_end.map(|e| (e.line, e.lines));
        assert_eq!(got, Some(end), "{text:?}");
    }
    // The `.end 'x'` line is the end of the file for the main parse and
    // for the skip of a failed block: the failed `.subckt` has no
    // closing line before it, so only its line is skipped (as in
    // `block_recovery_stops_at_end`). The main parse read on before,
    // so R1, R2 and R3 joined the top level, with a `stray_ends`.
    std::fs::write(
        &main,
        "* t\n.subckt\nR1 a b 1\n.end 'x'\nR2 a b 2\n.ends\nR3 a b 3\n",
    )
    .unwrap();
    let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
    assert_eq!(super::recover_tests::error_lines(&parsed), [2]);
    let got = top(&parsed).after_end.map(|e| (e.line, e.lines));
    assert_eq!(got, Some((4, 3)));
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R1"]);
    // In an included file, such a line is ignored as `.end` is
    // (golden `end_in_include`), and the lines after it are read.
    std::fs::write(tmp.0.join("inc.sp"), "R1 a b 1\n.end 'x'\nR2 a b 2\n").unwrap();
    std::fs::write(&main, "* t\n.inc 'inc.sp'\nR3 a b 3\n").unwrap();
    let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R1", "R2", "R3"]);
}

#[tokio::test]
async fn braces_in_quoted_param() {
    // Golden `brace_quoted_param`: HSPICE reads `{a}` inside quotes as
    // the name `[a]` and rejects the deck. So a `.param` statement with
    // `{` or `}` in a quoted value is an error, as without quotes
    // (golden `param_braces_param`), and gives no parameter.
    let path = PathBuf::from("golden/cases/brace_quoted_param/deck.sp");
    let (parsed, files) = parse_path(&path, TitleLine::Auto).await;
    assert_eq!(super::recover_tests::error_lines(&parsed), [2]);
    let (ast, _) = files.build(parsed);
    assert!(ast.param.is_empty());
    let tmp = TempDir::new("brace_param");
    let main = tmp.0.join("main.sp");
    for text in [
        "* t\n.param p='{a}'\nR1 a b 1\n",
        "* t\n.param f(x)='{x}+1'\nR1 a b 1\n",
        "* t\n.param p='a}'\nR1 a b 1\n",
        "* t\n.subckt s a w='{x}'\n.ends\nR1 a b 1\n",
        "* t\n.option x='{a}'\nR1 a b 1\n",
    ] {
        std::fs::write(&main, text).unwrap();
        let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
        assert_eq!(super::recover_tests::error_lines(&parsed), [2], "{text:?}");
        let (ast, _) = files.build(parsed);
        assert!(
            ast.param.is_empty() && ast.param_func.is_empty(),
            "{text:?}"
        );
        assert!(ast.subckt.is_empty(), "{text:?}");
        assert_eq!(names(&ast.instance), ["R1"], "{text:?}");
    }
}
