use super::recover_tests::{TempDir, error_lines, error_msgs, errors, names, parse_text};
use super::*;
use crate::{AST, LibOverride, ParseOptions, TitleLine, ast::Value};
use std::{collections::BTreeMap, path::Path};

/// The top-level `.param` values of `ast`, by name.
fn params(ast: &AST) -> BTreeMap<String, f64> {
    ast.param
        .iter()
        .map(|kv| match kv.v {
            Value::Num(f) => (kv.k.to_string(), f),
            Value::Expr(ref e) => panic!("{}: {e}", kv.k),
        })
        .collect()
}

/// Parses the top file `path` with `title_line` and `lib_override`
/// (`(file, from, to)`).
async fn parse_path(
    path: &Path,
    title_line: TitleLine,
    lib_override: &[(&str, &str, &str)],
) -> (Parsed, Files) {
    let opts = ParseOptions {
        title_line,
        skip_prot: true,
        lib_override: lib_override
            .iter()
            .map(|(file, from, to)| LibOverride {
                file: PathBuf::from(file),
                from: from.to_string(),
                to: to.to_string(),
            })
            .collect(),
    };
    parse_top_multi_with([FileId::Include { path: path.into() }].into_iter(), &opts).await
}

#[tokio::test]
async fn lib_override_triples() {
    let tmp = TempDir::new("lib_triples");
    let dir = &tmp.0;
    std::fs::write(
        dir.join("models.lib"),
        "\
* corners
.lib tt
.param rcorner=1k
.lib 'models.lib' common
.endl tt
.lib ff
.param rcorner=2k
.lib 'models.lib' common
.endl ff
.lib res_tt
.param rres=10
.endl res_tt
.lib res_ff
.param rres=20
.endl res_ff
.lib common
.param rcommon=1
.endl common
.lib common2
.param rcommon=2
.endl common2
",
    )
    .unwrap();
    let deck = dir.join("deck.sp");
    std::fs::write(
        &deck,
        "* title\n.lib 'models.lib' tt\n.lib 'models.lib' res_tt\n",
    )
    .unwrap();
    let p = |rcorner: f64, rres: f64, rcommon: f64| {
        BTreeMap::from([
            ("rcommon".to_owned(), rcommon),
            ("rcorner".to_owned(), rcorner),
            ("rres".to_owned(), rres),
        ])
    };
    for (overrides, want, matched) in [
        // the deck's own sections
        (&[][..], p(1e3, 10.0, 1.0), &[][..]),
        // only the `tt` call changes: `ff` and `res_tt` load
        (&[("models.lib", "tt", "ff")], p(2e3, 10.0, 1.0), &[true]),
        // each call is switched on its own; names in any letter case,
        // the file without its directory
        (
            &[
                ("/elsewhere/MODELS.LIB", "TT", "FF"),
                ("models.lib", "res_tt", "res_ff"),
            ],
            p(2e3, 20.0, 1.0),
            &[true, true],
        ),
        // no call to `res_ff` or to `other.lib`: unused
        (
            &[
                ("models.lib", "res_ff", "res_tt"),
                ("other.lib", "tt", "ff"),
            ],
            p(1e3, 10.0, 1.0),
            &[false, false],
        ),
        // a call made inside the library itself changes too
        (
            &[("models.lib", "common", "common2")],
            p(1e3, 10.0, 2.0),
            &[true],
        ),
    ] {
        let (parsed, files) = parse_path(&deck, TitleLine::Auto, overrides).await;
        assert!(
            error_msgs(&parsed).is_empty(),
            "{overrides:?}: {:?}",
            error_msgs(&parsed)
        );
        assert_eq!(parsed.lib_override_matched, matched, "{overrides:?}");
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err, "{overrides:?}");
        assert_eq!(params(&ast), want, "{overrides:?}");
    }
}

#[tokio::test]
async fn endl_in_comment() {
    // A `.lib` section in a parsed file ends at the line whose first
    // token is `.endl`, not at `.endl` in a comment or in a longer word.
    let (parsed, files) = parse_text(
            "endl_comment",
            "R0 a b 1\n.lib tt\nR1 a b 1\n* the section ends at .endl below\nR2 a b 2 $ not .endl here\n.endlx\n  .ENDL tt\nR3 a b 3\n",
        )
        .await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R0", "R3"]);

    // A section that `.lib '<file>' <section>` loads (`match_lib`). The
    // `.endlx` line does not end it: it is an unknown command, one error
    // at its line, and `R3` after it is in the section.
    let tmp = TempDir::new("endl_comment_lib");
    let dir = &tmp.0;
    std::fs::write(
            dir.join("l.sp"),
            ".lib tt\nR1 a b 1\n* old .endl tt\nR2 a b 2 $ .endl\n.endlx\nR3 a b 3\n.endl tt\n.lib ff\nR9 a b 9\n.endl ff\n",
        )
        .unwrap();
    let top = dir.join("top.sp");
    std::fs::write(&top, ".lib 'l.sp' tt\n").unwrap();
    let (parsed, files) = parse_path(&top, TitleLine::No, &[]).await;
    assert_eq!(error_lines(&parsed), [5], "{:?}", error_msgs(&parsed));
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R1", "R2", "R3"]);
}

/// `depth` nested `.subckt s<k>` headers, each with a resistor `R<k>`,
/// then (with `ends`) their `.ends` and a top-level `R_top`.
fn nested_deck(depth: usize, ends: bool) -> String {
    let mut s = String::new();
    for k in 0..depth {
        s.push_str(&format!(".subckt s{k} a b\nR{k} a b 1\n"));
    }
    if ends {
        for k in (0..depth).rev() {
            s.push_str(&format!(".ends s{k}\n"));
        }
        s.push_str("R_top a b 1\n");
    }
    s
}

/// The number of `NestingTooDeep` errors, and their lines.
fn too_deep(parsed: &Parsed) -> Vec<u32> {
    errors(parsed)
        .into_iter()
        .filter(|e| matches!(e.err, ParseErrorInner::NestingTooDeep { .. }))
        .map(|e| e.pos.map_or(0, |p| p.line_num))
        .collect()
}

/// The depth of the `.subckt` chain `s0 > s1 > …` in `ast`.
fn chain_depth(ast: &AST) -> usize {
    let mut depth = 0;
    let mut ast = ast;
    while let Some(s) = ast.subckt.get(&format!("s{depth}")) {
        assert_eq!(names(&s.ast.instance), [format!("R{depth}")]);
        ast = &s.ast;
        depth += 1;
    }
    depth
}

/// Runs on the default test thread (`#[tokio::test]`), so its stack
/// is the default one.
#[tokio::test]
async fn deep_nesting_no_crash() {
    // 256 levels are allowed.
    let (parsed, files) = parse_text("nest_256", &nested_deck(256, true)).await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(chain_depth(&ast), 256);
    assert_eq!(names(&ast.instance), ["R_top"]);

    // Level 257 is one error at its header, and its body is skipped up
    // to its `.ends`. The levels above it keep the rest.
    let (parsed, files) = parse_text("nest_257", &nested_deck(257, true)).await;
    assert_eq!(too_deep(&parsed), [2 * 256 + 1]);
    assert_eq!(
        error_msgs(&parsed),
        ["`.subckt s256` is nested deeper than 256 levels: its definition is skipped"]
    );
    let (ast, _) = files.build(parsed);
    assert_eq!(chain_depth(&ast), 256);
    assert_eq!(names(&ast.instance), ["R_top"]);

    // 5,000 headers without `.ends`: one `NestingTooDeep`, and the parse
    // ends.
    let (parsed, _) = parse_text("nest_5000", &nested_deck(5000, false)).await;
    assert_eq!(too_deep(&parsed), [2 * 256 + 1]);
    let missing = errors(&parsed)
        .into_iter()
        .filter(|e| matches!(e.err, ParseErrorInner::MissingEnds { .. }))
        .count();
    assert_eq!(missing, 256);
}

/// Writes `dir/<tag>.sp`: `levels` nested `.subckt <tag><k>` headers,
/// each with a resistor `R<tag><k>`, the line `inner` in the innermost
/// body, the `.ends` lines, and a top-level `R<tag>_top`.
fn nested_file(dir: &Path, tag: &str, levels: usize, inner: &str) {
    let mut s = String::new();
    for k in 0..levels {
        s.push_str(&format!(".subckt {tag}{k} a b\nR{tag}{k} a b 1\n"));
    }
    s.push_str(inner);
    for k in (0..levels).rev() {
        s.push_str(&format!(".ends {tag}{k}\n"));
    }
    s.push_str(&format!("R{tag}_top a b 1\n"));
    std::fs::write(dir.join(format!("{tag}.sp")), s).unwrap();
}

/// The depth of the first `.subckt` chain in `ast`, and the instances of
/// its innermost body.
fn first_chain(ast: &AST) -> (usize, Vec<String>) {
    let mut depth = 0;
    let mut ast = ast;
    while let Some((_, s)) = ast.subckt.first() {
        ast = &s.ast;
        depth += 1;
    }
    (depth, names(&ast.instance))
}

/// Runs on the default test thread (`#[tokio::test]`).
#[tokio::test]
async fn nesting_counts_across_includes() {
    // An `.include` in a body puts the file in that body (golden
    // `include_in_subckt`), so the depth adds up along the include
    // chain. Each file is within the limit, so the parse has no error.
    let tmp = TempDir::new("nest_include");
    let dir = &tmp.0;

    // 100 + 100 levels: within the limit.
    nested_file(dir, "a", 100, ".include 'b.sp'\n");
    nested_file(dir, "b", 100, "");
    let (parsed, files) = parse_path(&dir.join("a.sp"), TitleLine::No, &[]).await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(first_chain(&ast), (200, vec!["Rb99".to_owned()]));

    // 256 + 256 + 256 levels (a includes b, b includes c): `b0` is level
    // 257, one `NestingTooDeep` error at build time, and its definition
    // is skipped. The statements of `b.sp` outside its definitions stay
    // in the body of `a255`, and `c.sp` is not reached.
    nested_file(dir, "a", 256, ".include 'b.sp'\n");
    nested_file(dir, "b", 256, ".include 'c.sp'\n");
    nested_file(dir, "c", 256, "");
    let (parsed, files) = parse_path(&dir.join("a.sp"), TitleLine::No, &[]).await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(
        first_chain(&ast),
        (256, vec!["Ra255".to_owned(), "Rb_top".to_owned()])
    );
    assert_eq!(names(&ast.instance), ["Ra_top"]);
}

#[tokio::test]
async fn macro_eom_synonyms() {
    // Golden `macro_eom`.
    let (parsed, files) = parse_path(
        Path::new("golden/cases/macro_eom/deck.sp"),
        TitleLine::Auto,
        &[],
    )
    .await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    let inv = ast.subckt.get("inv").expect("macro kept");
    assert_eq!(inv.ports, ["a", "b"]);
    assert_eq!(inv.params.len(), 1);
    assert_eq!(names(&inv.ast.instance), ["r1"]);
    assert_eq!(names(&ast.instance), ["v1", "v2", "x1", "x2"]);

    // The keywords mix and nest, in any letter case.
    let (parsed, files) = parse_text(
        "macro_mix",
        ".MACRO m1 a b\nR1 a b 1\n.subckt inner a b\nR2 a b 2\n.EOM inner\n.ends m1\nR3 a b 3\n",
    )
    .await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, _) = files.build(parsed);
    let m1 = ast.subckt.get("m1").expect("macro kept");
    assert_eq!(names(&m1.ast.instance), ["R1"]);
    let inner = m1.ast.subckt.get("inner").expect("nested kept");
    assert_eq!(names(&inner.ast.instance), ["R2"]);
    assert_eq!(names(&ast.instance), ["R3"]);

    // A stray `.eom` is an error, and a failed `.macro` header skips its
    // body up to its `.eom`.
    let (parsed, files) = parse_text(
        "macro_errors",
        "R0 a b 1\n.eom\n.macro bad a b w=max(1,'2')\nR1 a b 1\n.eom\nR2 a b 2\n",
    )
    .await;
    assert_eq!(error_lines(&parsed), [2, 3]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R0", "R2"]);
    assert!(ast.subckt.is_empty());
}

#[tokio::test]
async fn title_auto_prot() {
    // `.prot`/`.protect` on line 1: an encrypted library, so line 1 is
    // parsed.
    for line1 in [".prot", ".PROTECT", "  .protect x"] {
        let tmp = TempDir::new("title_prot");
        let path = tmp.0.join("deck.sp");
        std::fs::write(&path, format!("{line1}\njunk ((\n.unprot\nR1 a b 1\n")).unwrap();
        let (parsed, files) = parse_path(&path, TitleLine::Auto, &[]).await;
        assert!(error_msgs(&parsed).is_empty(), "{line1}");
        let top = &parsed.inner[parsed.top_ids[0].0].1;
        let title = top.title.as_ref().expect("title info");
        assert!(!title.skipped, "{line1}");
        let (ast, _) = files.build(parsed);
        assert_eq!(ast.protected, [(1, 3)], "{line1}");
        assert_eq!(names(&ast.instance), ["R1"], "{line1}");
    }

    // Golden `title_plus`, `title_plus_words`, `title_plus_comment`: a
    // `+` line right after the title line is one error at line 2, and
    // the rest of the deck is parsed.
    for case in ["title_plus", "title_plus_words", "title_plus_comment"] {
        for mode in [TitleLine::Auto, TitleLine::Yes] {
            let path = PathBuf::from(format!("golden/cases/{case}/deck.sp"));
            let (parsed, files) = parse_path(&path, mode, &[]).await;
            assert_eq!(error_lines(&parsed), [2], "{case}");
            assert_eq!(
                error_msgs(&parsed),
                ["a `+` line right after the title line: HSPICE rejects the deck"],
                "{case}"
            );
            let (ast, _) = files.build(parsed);
            assert_eq!(names(&ast.instance), ["v1", "r2"], "{case}");
        }
    }
    // When line 1 is parsed, a `+` line continues it.
    let tmp = TempDir::new("title_plus_parsed");
    let path = tmp.0.join("deck.sp");
    for (text, mode) in [
        ("R1 a b 1\n+ m=2\nR2 a b 2\n", TitleLine::No),
        (
            ".subckt inv a b\n+ c\nR1 a b 1\n.ends\nR2 a b 2\n",
            TitleLine::Auto,
        ),
    ] {
        std::fs::write(&path, text).unwrap();
        let (parsed, files) = parse_path(&path, mode, &[]).await;
        assert!(error_msgs(&parsed).is_empty(), "{text:?}");
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance).last().map(String::as_str), Some("R2"));
    }
}

#[tokio::test]
async fn title_plus_gap() {
    // Golden `title_plus_gap`: a `*` line between line 1 and the `+`
    // line does not change it (HSPICE gives the error at line 3).
    let path = PathBuf::from("golden/cases/title_plus_gap/deck.sp");
    let (parsed, files) = parse_path(&path, TitleLine::Auto, &[]).await;
    assert_eq!(error_lines(&parsed), [3]);
    assert_eq!(
        error_msgs(&parsed),
        ["a `+` line right after the title line: HSPICE rejects the deck"]
    );
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["v1", "r2"]);
    // Also after a comment or an empty line.
    let tmp = TempDir::new("title_plus_gap");
    let path = tmp.0.join("deck.sp");
    std::fs::write(&path, "title\n* c\n\n+ r9 1 0 9\n+ x\nR1 a b 1\n").unwrap();
    let (parsed, files) = parse_path(&path, TitleLine::Auto, &[]).await;
    assert_eq!(error_lines(&parsed), [4]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R1"]);
    // Also a `+` line with only a comment, after one or more `*` and
    // `$` lines, or right after the title line (one error each time).
    for (text, line) in [
        ("title\n* gap\n+ $ c\nR1 a b 1\n", 3),
        ("title\n* gap\n$ gap\n  + $ c\n+ $ d\nR1 a b 1\n", 4),
        ("title\n+ $ c\nR1 a b 1\n", 2),
        ("title\n* gap\n+", 3),
    ] {
        std::fs::write(&path, text).unwrap();
        let (parsed, files) = parse_path(&path, TitleLine::Auto, &[]).await;
        assert_eq!(error_lines(&parsed), [line], "{text:?}");
        let (ast, _) = files.build(parsed);
        let want: &[&str] = if text.contains("R1") { &["R1"] } else { &[] };
        assert_eq!(names(&ast.instance), want, "{text:?}");
    }
}
