use super::*;

/// A temp path that no other call in any test of this process uses: the
/// tests run at the same time, and two of them can use one `tag`.
pub(super) fn unique_temp_path(tag: &str, suffix: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("netlist_db_{tag}_{pid}_{n}{suffix}"))
}

/// The options of the tests here, which are those of `netlist-query`
pub(super) fn opts(title_line: TitleLine) -> ParseOptions {
    ParseOptions {
        title_line,
        lib_override: Vec::new(),
        skip_prot: true,
    }
}

/// Writes `text` to a temp file, parses it as a top file.
pub(super) async fn parse_text(tag: &str, text: &str) -> (Parsed, Files) {
    let path = unique_temp_path(tag, ".sp");
    std::fs::write(&path, text).unwrap();
    let opts = opts(TitleLine::No);
    let res =
        parse_top_multi_with([FileId::Include { path: path.clone() }].into_iter(), &opts).await;
    _ = std::fs::remove_file(&path);
    res
}

/// Every recorded error, including subckt bodies.
pub(super) fn errors(parsed: &Parsed) -> Vec<&ParseError> {
    fn walk<'p>(ast: &'p ASTBuilder, out: &mut Vec<&'p ParseError>) {
        for seg in &ast.segments {
            if let Segment::Local(local) = seg {
                out.extend(local.errors.iter());
                for s in &local.subckt {
                    walk(&s.ast, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    for (_, ast) in &parsed.inner {
        walk(ast, &mut out);
    }
    out
}

/// `(line, byte offset)` of every recorded error, including subckt bodies.
pub(super) fn error_pos(parsed: &Parsed) -> Vec<(u32, usize)> {
    errors(parsed)
        .into_iter()
        .map(|e| e.pos.map_or((0, usize::MAX), |p| (p.line_num, p.start)))
        .collect()
}

/// The message of every recorded error, including subckt bodies.
pub(super) fn error_msgs(parsed: &Parsed) -> Vec<String> {
    errors(parsed)
        .into_iter()
        .map(|e| e.err.to_string())
        .collect()
}

/// Line numbers of every recorded error, including subckt bodies.
pub(super) fn error_lines(parsed: &Parsed) -> Vec<u32> {
    error_pos(parsed)
        .into_iter()
        .map(|(line, _)| line)
        .collect()
}

pub(super) fn names(instances: &[crate::instance::Instance]) -> Vec<String> {
    instances.iter().map(|x| x.name.to_string()).collect()
}

#[tokio::test]
async fn recover_bad_mosfet_keeps_neighbors() {
    let (parsed, files) = parse_text(
        "bad_mosfet",
        "R1 a b 1k\nM1 d g nch W=1u\nR2 c d 2k\n.end\n",
    )
    .await;
    assert_eq!(error_lines(&parsed), vec![2]);
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(names(&ast.instance), ["R1", "R2"]);
}

#[tokio::test]
async fn recover_bad_dot_line() {
    let (parsed, files) = parse_text("bad_dot_line", ".param f(x=\nR2 a b 1\n").await;
    // at the start of the statement: line 1, column 1
    assert_eq!(error_pos(&parsed), vec![(1, 0)]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R2"]);
}

#[tokio::test]
async fn recover_inside_subckt() {
    let (parsed, files) = parse_text(
        "inside_subckt",
        ".subckt sub a b\nR1 a b 1k\nM1 d g nch W=1u\nR2 a b 2k\n.ends sub\nR3 a b 3k\n.end\n",
    )
    .await;
    assert_eq!(error_lines(&parsed), vec![3]);
    let (ast, _) = files.build(parsed);
    let sub = ast.subckt.get("sub").expect("subckt kept");
    assert_eq!(names(&sub.ast.instance), ["R1", "R2"]);
    // `.ends` kept: R3 is top-level, not part of `sub`.
    assert_eq!(names(&ast.instance), ["R3"]);
}

#[tokio::test]
async fn recover_skips_continuation_lines() {
    let (parsed, files) = parse_text(
            "continuation",
            "R1 a b 1k\nM1 d g nch\n* comment between continuation lines\n+ W=1u\n\n  + L=0.1u\nR2 c d 2k\n",
        )
        .await;
    assert_eq!(error_lines(&parsed), vec![2]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R1", "R2"]);
}

#[tokio::test]
async fn recover_bad_subckt_header() {
    // A bad `.subckt` header drops that body (nested defs too), but its
    // `.ends` must not end the file.
    let (parsed, files) = parse_text(
            "bad_subckt_header",
            "R0 a b 1\n.subckt\n.subckt inner a b\nR1 a b 1k\n.ends inner\nR2 a b 2k\n.ends\nR3 a b 3k\n.end\n",
        )
        .await;
    assert_eq!(error_lines(&parsed), vec![2]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R0", "R3"]);
    assert!(ast.subckt.is_empty());
}

#[tokio::test]
async fn recover_failure_on_continuation_line() {
    // The statement parses in part and stops at a bad `+` line. The
    // error is at the start of the statement, and neither the partial
    // record nor a record made from the `+` lines is kept.
    for text in [
        "M1 d g s b nch W=1u\n+ =bad\n+ M=2\nR2 a b 1\n",
        "X1 a b sub\n+ w=max(1,'2')\n+ l=2\nR2 a b 1\n",
    ] {
        let (parsed, files) = parse_text("continuation_fail", text).await;
        assert_eq!(error_pos(&parsed), vec![(1, 0)], "{text:?}");
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R2"], "{text:?}");
    }
}

#[tokio::test]
async fn recover_orphan_continuation_line() {
    // `+` lines with no statement before them are one error.
    let (parsed, files) = parse_text("orphan_plus", "+ a b\n+ c\nR2 a b 1\n").await;
    assert_eq!(error_lines(&parsed), vec![1]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R2"]);
}

#[tokio::test]
async fn recover_text_left_on_line() {
    // Text left on a `.subckt` header line fails the whole statement,
    // so the ports do not change and the body is not kept.
    let (parsed, files) = parse_text(
        "header_left",
        "R1 a b 1k\n.subckt sub a b w=max(1,'2')\nR2 a b 2k\n.ends sub\nR3 a b 3k\n",
    )
    .await;
    assert_eq!(error_lines(&parsed), vec![2]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R1", "R3"]);
    assert!(ast.subckt.is_empty());

    // Text left on a `.ends` line is an error, but the `.ends` still
    // ends the body.
    let (parsed, files) = parse_text(
        "ends_left",
        ".subckt sub a b\nR1 a b 1k\n.ends sub (x\n+ y\nR2 a b 2k\n",
    )
    .await;
    assert_eq!(error_lines(&parsed), vec![3]);
    let (ast, _) = files.build(parsed);
    let sub = ast.subckt.get("sub").expect("subckt kept");
    assert_eq!(names(&sub.ast.instance), ["R1"]);
    assert_eq!(names(&ast.instance), ["R2"]);
}

#[tokio::test]
async fn recover_stray_ends() {
    // A `.ends` with no open `.subckt` is one error at its line. It does
    // not end the file.
    let (parsed, files) = parse_text(
        "stray_ends",
        "R0 a b 1\n*.subckt old a b\n*R1 a b 1\n.ends\nR3 a b 3\nR9 a b 9\n",
    )
    .await;
    assert_eq!(error_pos(&parsed), vec![(4, 36)]);
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(names(&ast.instance), ["R0", "R3", "R9"]);

    // A duplicated `.ends` (with a continuation line) after a good body.
    let (parsed, files) = parse_text(
        "dup_ends",
        ".subckt inv a b\nR1 a b 1\n.ends inv\n.ends inv\n+ x\nR2 a b 2\n",
    )
    .await;
    assert_eq!(error_lines(&parsed), vec![4]);
    let (ast, _) = files.build(parsed);
    let inv = ast.subckt.get("inv").expect("subckt kept");
    assert_eq!(names(&inv.ast.instance), ["R1"]);
    assert_eq!(names(&ast.instance), ["R2"]);
}

#[tokio::test]
async fn recover_missing_ends() {
    // A body that reaches the end of input without `.ends` keeps the
    // rest of the input, with one error at its `.subckt` line.
    let (parsed, files) = parse_text(
        "missing_ends",
        "R0 a b 1\n.subckt inv a b\nR1 a b 1\nR9 a b 9",
    )
    .await;
    assert_eq!(error_pos(&parsed), vec![(2, 9)]);
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    let inv = ast.subckt.get("inv").expect("subckt kept");
    assert_eq!(names(&inv.ast.instance), ["R1", "R9"]);
    assert_eq!(names(&ast.instance), ["R0"]);
}

#[tokio::test]
async fn ends_name_chars() {
    // `.ends` reads its name with the same characters as `.subckt`.
    for name in ["inv-x1", "pm#1", "cell.a", "bus[0]"] {
        let text = format!(".subckt {name} a b\nR1 a b 1\n.ends {name}\nR2 a b 2\n");
        let (parsed, files) = parse_text("ends_name", &text).await;
        assert!(
            error_msgs(&parsed).is_empty(),
            "{name}: {:?}",
            error_msgs(&parsed)
        );
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err, "{name}");
        let sub = ast.subckt.get(name).expect("subckt kept");
        assert_eq!(names(&sub.ast.instance), ["R1"], "{name}");
        assert_eq!(names(&ast.instance), ["R2"], "{name}");
    }
}

#[tokio::test]
async fn binned_model_name() {
    let (parsed, files) = parse_text("binned_model", ".model nch.1 nmos level=54\n").await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.model.len(), 1);
    assert_eq!(ast.model[0].name, "nch.1");
}

#[tokio::test]
async fn skip_subckt_eof_fallback() {
    // A failed `.subckt` header with no `.ends` skips only its line.
    let (parsed, files) = parse_text("subckt_eof", "R0 a b 1\n.subckt\nR1 a b 1\nR9 a b 9\n").await;
    assert_eq!(error_lines(&parsed), vec![2]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R0", "R1", "R9"]);
    assert!(ast.subckt.is_empty());

    // A failed nested header with no `.ends` does not take the parent's
    // `.ends`.
    let (parsed, files) = parse_text(
        "nested_no_ends",
        ".subckt outer a b\nR1 a b 1\n.subckt\nR2 a b 2\n.ends outer\nR3 a b 3\n",
    )
    .await;
    assert_eq!(error_lines(&parsed), vec![3]);
    let (ast, _) = files.build(parsed);
    let outer = ast.subckt.get("outer").expect("subckt kept");
    assert_eq!(names(&outer.ast.instance), ["R1", "R2"]);
    assert_eq!(names(&ast.instance), ["R3"]);

    // A failed nested header with its own `.ends` skips its body only.
    let (parsed, files) = parse_text(
            "nested_own_ends",
            ".subckt outer a b\nR1 a b 1\n.subckt bad a b w=max(1,'2')\nR2 a b 2\n.ends bad\nR4 a b 4\n.ends outer\nR3 a b 3\n",
        )
        .await;
    assert_eq!(error_lines(&parsed), vec![3]);
    let (ast, _) = files.build(parsed);
    let outer = ast.subckt.get("outer").expect("subckt kept");
    assert_eq!(names(&outer.ast.instance), ["R1", "R4"]);
    assert!(outer.ast.subckt.is_empty());
    assert_eq!(names(&ast.instance), ["R3"]);

    // Two levels down: the grandparent's `.ends` does not make the
    // parent's `.ends` look like the failed header's.
    let (parsed, files) = parse_text(
        "nested_two_levels",
        ".subckt g a b\n.subckt p a b\n.subckt\nR2 a b 2\n.ends p\nR5 a b 5\n.ends g\nR3 a b 3\n",
    )
    .await;
    assert_eq!(error_lines(&parsed), vec![3]);
    let (ast, _) = files.build(parsed);
    let g = ast.subckt.get("g").expect("subckt kept");
    assert_eq!(names(&g.ast.instance), ["R5"]);
    let p = g.ast.subckt.get("p").expect("nested subckt kept");
    assert_eq!(names(&p.ast.instance), ["R2"]);
    assert_eq!(names(&ast.instance), ["R3"]);
}

#[tokio::test]
async fn skip_subckt_ends_name() {
    // `.ends bad` names the failed header: it closes the failed block,
    // also when the parent has no `.ends` after it.
    let (parsed, files) = parse_text(
        "ends_names_failed",
        ".subckt outer a b\n.subckt bad a b w=max(1,'2')\nR2 a b 2\n.ends bad\nR4 a b 4\n",
    )
    .await;
    let mut lines = error_lines(&parsed);
    lines.sort_unstable();
    assert_eq!(lines, vec![1, 2]);
    assert!(error_msgs(&parsed).contains(&"`.subckt outer` has no `.ends`".to_owned()));
    let (ast, _) = files.build(parsed);
    let outer = ast.subckt.get("outer").expect("subckt kept");
    assert_eq!(names(&outer.ast.instance), ["R4"]);
    assert!(ast.instance.is_empty());

    // `.ends outer` names the parent: it is not the failed block's, so
    // the later `.ends` is a stray one.
    let (parsed, files) = parse_text(
        "ends_names_parent",
        ".subckt outer a b\n.subckt\nR2 a b 2\n.ends outer\nR3 a b 3\n.ends\nR4 a b 4\n",
    )
    .await;
    let mut lines = error_lines(&parsed);
    lines.sort_unstable();
    assert_eq!(lines, vec![2, 6]);
    assert!(
        error_msgs(&parsed).contains(&"`.ends` without an open `.subckt` or `.macro`".to_owned())
    );
    let (ast, _) = files.build(parsed);
    let outer = ast.subckt.get("outer").expect("subckt kept");
    assert_eq!(names(&outer.ast.instance), ["R2"]);
    assert_eq!(names(&ast.instance), ["R3", "R4"]);
}

/// Skipping failed blocks stays linear in the file size: a failure does
/// not scan the rest of the file again. The lines of each kind of block
/// are found in one pass, and each skip after that is a look-up.
/// The test counts the passes, not the time, so a busy machine cannot
/// make it fail. (`#[tokio::test]` runs the parse on this thread.)
#[tokio::test]
async fn failed_blocks_linear() {
    const K: usize = 2000;
    let filler = "R1 a b 1\nR2 a b 2\nR3 a b 3\n";
    // failed nested headers, each with its own `.ends` (no name, so the
    // `.ends` that follow it decide)
    let nested = format!(
        ".subckt outer a b\n{}.ends outer\n",
        format!(".subckt bad a b w=max(1,'2')\n{filler}.ends\n").repeat(K)
    );
    // failed headers with no `.ends`
    let no_ends = format!(".subckt\n{filler}").repeat(K);
    // `.data` with no `.enddata`
    let no_enddata = format!(".data d\n+ a b\n+ 1 x(\n{filler}").repeat(K);
    // failed `.data` with `.enddata` in a body (the skip also finds the
    // `.subckt` and `.ends` lines, to keep the body's `.ends`)
    let data_in_body = format!(
        ".subckt outer a b\n{}.ends outer\n",
        format!(".data d\n+ a b\n+ 1 x(\n{filler}.enddata\n").repeat(K)
    );
    for (tag, text, scans) in [
        ("linear_nested", nested, 1),
        ("linear_no_ends", no_ends, 1),
        ("linear_no_enddata", no_enddata, 1),
        ("linear_data_in_body", data_in_body, 2),
    ] {
        let before = utils::BLOCK_SCANS.with(|n| n.get());
        let (parsed, _) = parse_text(tag, &text).await;
        let after = utils::BLOCK_SCANS.with(|n| n.get());
        assert_eq!(errors(&parsed).len(), K, "{tag}");
        // A skip that scans the rest of the file again for each failure
        // makes `K` passes (in a debug build it took 18 s here).
        assert_eq!(after - before, scans, "{tag}");
    }
}

#[tokio::test]
async fn failed_block_keeps_parent_ends() {
    // A failed `.data` in a body does not take a stray `.enddata` after
    // the body's `.ends`: only its line is skipped, and `.ends inv`
    // still ends `inv`.
    let (parsed, files) = parse_text(
        "data_past_ends",
        ".subckt inv a b\n.data d\n+ p q\n+ 1 x(\n.ends inv\nR9 a b 9\n.enddata\nR10 a b 10\n",
    )
    .await;
    let mut lines = error_lines(&parsed);
    lines.sort_unstable();
    assert_eq!(lines, vec![2, 7]);
    let (ast, _) = files.build(parsed);
    let inv = ast.subckt.get("inv").expect("subckt kept");
    assert!(inv.ast.instance.is_empty());
    assert_eq!(names(&ast.instance), ["R9", "R10"]);

    // The same `.data` at file level has no `.ends` to keep: its skip
    // goes to the `.enddata`.
    let (parsed, files) = parse_text(
        "data_top_enddata",
        ".data d\n+ p q\n+ 1 x(\nR1 a b 1\n.enddata\nR10 a b 10\n",
    )
    .await;
    assert_eq!(error_lines(&parsed), vec![1]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R10"]);
}

#[tokio::test]
async fn failed_block_one_error() {
    for (tag, text) in [
        // `.data` with a bad value
        (
            "data_bad",
            "R0 a b 1\n.data d\n+ a b\n+ 1 x(\n.enddata\nR9 a b 9\n",
        ),
        // `.lib` section whose `.endl` line has extra text
        (
            "lib_endl",
            "R0 a b 1\n.lib tt\nR1 a b 1\n.endl tt junk\nR9 a b 9\n",
        ),
    ] {
        let (parsed, files) = parse_text(tag, text).await;
        assert_eq!(error_lines(&parsed), vec![2], "{tag}");
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R0", "R9"], "{tag}");
        assert!(ast.data.is_empty(), "{tag}");
        assert!(ast.unknwon.is_empty(), "{tag}");
    }

    // A `.lib` section with no `.endl` skips only its line.
    let (parsed, files) = parse_text("lib_no_endl", "R0 a b 1\n.lib tt\nR1 a b 1\n").await;
    assert_eq!(error_lines(&parsed), vec![2]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R0", "R1"]);
}

#[tokio::test]
async fn clear_error_text() {
    for (tag, text, msg) in [
        (
            "clear_stray_ends",
            "R0 a b 1\n.ends\n",
            "`.ends` without an open `.subckt` or `.macro`",
        ),
        (
            "clear_missing_ends",
            ".subckt inv a b\nR1 a b 1\n",
            "`.subckt inv` has no `.ends`",
        ),
        // `.macro`/`.eom` errors name their own keywords
        (
            "clear_stray_eom",
            "R0 a b 1\n.EOM\n",
            "`.eom` without an open `.subckt` or `.macro`",
        ),
        (
            "clear_missing_eom",
            ".Macro inv a b\nR1 a b 1\n",
            "`.macro inv` has no `.eom`",
        ),
        (
            "trailing_text",
            ".option a=1 (\n",
            "text after the end of the statement",
        ),
        (
            "trailing_ends",
            ".subckt inv a b\nR1 a b 1\n.ends inv x\n",
            "text after the end of the statement",
        ),
    ] {
        let (parsed, _) = parse_text(tag, text).await;
        assert_eq!(error_msgs(&parsed), [msg], "{tag}");
    }
}

/// A temp dir that is removed when it is dropped, also when a test fails.
pub(super) struct TempDir(pub(super) PathBuf);

impl TempDir {
    pub(super) fn new(tag: &str) -> Self {
        let dir = unique_temp_path(tag, "");
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn include_quotes_stripped() {
    let tmp = TempDir::new("incq");
    let dir = &tmp.0;
    std::fs::write(dir.join("f.sp"), "R1 a b 1\n").unwrap();
    std::fs::write(dir.join("l.sp"), ".lib tt\nR1 a b 1\n.endl tt\n").unwrap();
    for (k, line) in [
        ".include \"f.sp\"",
        ".include 'f.sp'",
        ".inc \"f.sp\"",
        ".lib \"l.sp\" tt",
        ".lib 'l.sp' tt",
    ]
    .into_iter()
    .enumerate()
    {
        let top = dir.join(format!("top{k}.sp"));
        std::fs::write(&top, format!("{line}\n")).unwrap();
        let (parsed, files) = parse_top_multi_with(
            [FileId::Include { path: top }].into_iter(),
            &opts(TitleLine::No),
        )
        .await;
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err, "{line}");
        assert_eq!(names(&ast.instance), ["R1"], "{line}");
    }
}

#[tokio::test]
async fn data_file_quotes_stripped() {
    for quote in ['\'', '"'] {
        let text = format!(
            ".data d mer\n+ file={quote}a.txt{quote} p1=1\n+ out={quote}o.txt{quote}\n.enddata\n"
        );
        let (parsed, files) = parse_text("data_quotes", &text).await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let (ast, _) = files.build(parsed);
        let crate::ast::DataValues::MER(data_files) = &ast.data[0].values else {
            panic!("{quote}: {:?}", ast.data[0].values);
        };
        assert_eq!(data_files.files[0].file, "a.txt", "{quote}");
        assert_eq!(data_files.out.as_deref(), Some("o.txt"), "{quote}");
    }
}

#[test]
fn io_error_text() {
    let e = ParseErrorInner::IO(std::io::Error::other("cannot read deck.sp"));
    assert_eq!(e.to_string(), "cannot read deck.sp");
}

#[tokio::test]
async fn bom_skipped() {
    let (parsed, files) = parse_text(
        "bom",
        "\u{FEFF}R1 a b 1\n.subckt inv a b\nR2 a b 2\n.ends inv\n",
    )
    .await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    assert!(!files.inner[0].starts_with('\u{FEFF}'));
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R1"]);
    assert!(ast.subckt.contains_key("inv"));
}

#[tokio::test]
async fn ic_only_segment_kept() {
    let (parsed, files) = parse_text("ic_only", ".ic v(a)=1\n").await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.init_condition.len(), 1);

    let (parsed, files) = parse_text("nodeset_only", ".nodeset a=1\n").await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.nodeset.len(), 1);
}
