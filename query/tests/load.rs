//! Load, summary and diagnostics (plan Task 6).

use netlist_query::{
    Diagnostic, Level, LibOverride, LoadError, LoadOptions, Netlist, Page, QueryError, TitleLine,
};
use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// A deck written by `write_deck`. It derefs to the path of its first
/// file. Its directory belongs to this process, so two test runs at the
/// same time (in one target directory) do not share it, and it is removed
/// on drop.
struct Deck {
    dir: PathBuf,
    main: PathBuf,
}

impl Drop for Deck {
    fn drop(&mut self) {
        _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl std::ops::Deref for Deck {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.main
    }
}

impl AsRef<Path> for Deck {
    fn as_ref(&self) -> &Path {
        &self.main
    }
}

/// Writes `files` (name, text) into a new directory for test `name`.
fn write_deck(name: &str, files: &[(&str, &str)]) -> Deck {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("load")
        .join(format!("{name}-{}", std::process::id()));
    _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        std::fs::write(dir.join(file), text).unwrap();
    }
    let main = dir.join(files[0].0);
    Deck { dir, main }
}

fn load(path: impl AsRef<Path>) -> Netlist {
    Netlist::load(&LoadOptions::new(path.as_ref().to_path_buf())).unwrap()
}

fn all_diags(net: &Netlist, level: Option<Level>) -> Vec<Diagnostic> {
    let page = Page {
        limit: 500,
        cursor: None,
    };
    let p = net.diagnostics(level, &page).unwrap();
    assert!(p.next_cursor.is_none(), "more than 500 diagnostics");
    p.items
}

fn with_code<'a>(diags: &'a [Diagnostic], code: &str) -> Vec<&'a Diagnostic> {
    diags.iter().filter(|d| d.code == code).collect()
}

#[test]
fn load_counts_tests_top_sp() {
    let net = load(repo("tests/top.sp"));
    let s = net.summary();
    // DEMO and UNITS; the included files define no subcircuit
    assert_eq!(s.definitions, 2);
    assert_eq!(s.root, "top-level");
    // X0.AAA, and X0.BBB from inc1.sp
    assert_eq!(s.top_level_instances, 2);
    let diags = all_diags(&net, Some(Level::Error));
    let io = with_code(&diags, "file_unreadable");
    assert_eq!(io.len(), 1, "{diags:#?}");
    assert!(io[0].message.contains("inc0.sp"), "{}", io[0].message);
    let loc = io[0].loc.as_ref().unwrap();
    assert!(loc.file.ends_with("top.sp"), "{loc:?}");
    assert_eq!(loc.line, 5);
    // inc2.sp -> lib.sp (section pre_layout) -> cycle_ref0.sp ->
    // cycle_ref1.sp -> cycle_ref0.sp
    let cycle = with_code(&diags, "include_cycle");
    assert_eq!(cycle.len(), 1, "{diags:#?}");
    assert!(
        cycle[0].message.contains("cycle_ref0.sp") && cycle[0].message.contains("cycle_ref1.sp"),
        "{}",
        cycle[0].message
    );
    let loc = cycle[0].loc.as_ref().unwrap();
    assert!(loc.file.ends_with("cycle_ref1.sp"), "{loc:?}");
    assert_eq!(loc.line, 2);
    // `.lib 'lib.sp' tt`: lib.sp has no section tt
    let missing = with_code(&diags, "lib_section_missing");
    assert_eq!(missing.len(), 1, "{diags:#?}");
    assert!(missing[0].message.contains("tt"), "{}", missing[0].message);
    assert_eq!(s.diagnostics_count, all_diags(&net, None).len());
}

#[test]
fn diagnostic_has_file_and_line() {
    let main = write_deck(
        "file_and_line",
        &[
            ("main.sp", "* title\nR1 a b 1k\n.inc 'sub.sp'\nR3 c d 3k\n"),
            ("sub.sp", "R2 a b 2k\nM1 d g s b x y z w nch W=1u\n"),
        ],
    );
    let net = load(main);
    let diags = all_diags(&net, Some(Level::Error));
    assert_eq!(diags.len(), 1, "{diags:#?}");
    let d = &diags[0];
    let loc = d.loc.as_ref().unwrap();
    assert!(loc.file.ends_with("sub.sp"), "{loc:?}");
    assert_eq!(loc.line, 2);
    // plain English, no parser internals
    assert!(!d.message.contains("Nom") && !d.message.contains("MapRes"));
    assert!(d.message.contains("M1 d g s b"), "{}", d.message);
}

#[test]
fn every_error_message_is_plain_english() {
    let main = write_deck(
        "plain_english",
        &[(
            "main.sp",
            "* title\n.param f(x=\nR1 a b\n.foo 1 2\n.ends\nR2 a b {x}\n.subckt open a\n",
        )],
    );
    let net = load(main);
    let diags = all_diags(&net, Some(Level::Error));
    assert!(diags.len() >= 5, "{diags:#?}");
    for d in &diags {
        assert!(d.loc.is_some(), "{d:?}");
        for bad in ["Nom(", "MapRes", "Verify", "Span {", "ErrorKind"] {
            assert!(!d.message.contains(bad), "{d:?}");
        }
    }
    let unknown = with_code(&diags, "unknown_command");
    assert_eq!(unknown.len(), 1, "{diags:#?}");
    assert!(
        unknown[0].message.contains(".foo"),
        "{}",
        unknown[0].message
    );
}

#[test]
fn duplicate_subckt_diagnosed() {
    let main = write_deck(
        "dup",
        &[(
            "main.sp",
            "* dup\n\
             .subckt inv a b\nr1 a b 1\n.ends\n\
             .SUBCKT INV a b\nr1 a b 2\n.ends\n\
             .subckt cell a b\nrc a b 1\n.ends\n\
             .subckt outer a b\n.subckt cell a b\nrc a b 2\n.ends\nxc a b cell\n.ends\n\
             x1 1 0 inv\n",
        )],
    );
    let net = load(main);
    // both `inv`, `cell`, `outer` and `outer>cell`
    assert_eq!(net.summary().definitions, 5);
    let diags = all_diags(&net, None);
    let dups = with_code(&diags, "duplicate_subckt");
    // the nested `cell` is not a duplicate of the top-level `cell`
    assert_eq!(dups.len(), 1, "{diags:#?}");
    let d = dups[0];
    assert_eq!(d.level, Level::Error);
    assert!(
        d.message.contains("HSPICE rejects this deck"),
        "{}",
        d.message
    );
    assert!(d.message.contains("first"), "{}", d.message);
    assert_eq!(d.loc.as_ref().unwrap().line, 5);
}

#[test]
fn golden_dup_subckt_diagnosed() {
    let net = load(repo("golden/cases/dup_subckt/deck.sp"));
    let diags = all_diags(&net, Some(Level::Error));
    let dups = with_code(&diags, "duplicate_subckt");
    assert_eq!(dups.len(), 1, "{diags:#?}");
    assert_eq!(dups[0].loc.as_ref().unwrap().line, 5);
}

#[test]
fn nested_same_name_twice_is_duplicate() {
    // two nested `cell` in the same parent: the same scope
    let main = write_deck(
        "dup_nested",
        &[(
            "main.sp",
            "* t\n.subckt outer a b\n.subckt cell a b\n.ends\n.subckt cell a b\n.ends\n.ends\n",
        )],
    );
    let net = load(main);
    let diags = all_diags(&net, None);
    assert_eq!(with_code(&diags, "duplicate_subckt").len(), 1, "{diags:#?}");
}

#[test]
fn parhier_option_read() {
    let parhier = |case: &str| {
        load(repo(&format!("golden/cases/{case}/deck.sp")))
            .summary()
            .parhier
    };
    assert_eq!(parhier("parhier_local"), "local");
    assert_eq!(parhier("parhier_global"), "global");
    assert_eq!(parhier("parhier_default"), "global");
    // line 1 `.option parhier=local` is the title line (golden `title_dot`)
    assert_eq!(parhier("title_dot"), "global");
    let main = write_deck(
        "parhier_case",
        &[("main.sp", "* t\n.OPTIONS PARHIER='LOCAL' post\nR1 a b 1\n")],
    );
    assert_eq!(load(main).summary().parhier, "local");
}

#[test]
fn parhier_in_alter_block_is_not_the_first_run() {
    let main = write_deck(
        "parhier_alter",
        &[("main.sp", "* t\nR1 a b 1\n.alter\n.option parhier=local\n")],
    );
    assert_eq!(load(main).summary().parhier, "global");
}

#[test]
fn paging_limits() {
    let mut text = String::from("* paging\n");
    for n in 0..75 {
        text.push_str(&format!(".bad{n} 1\n"));
    }
    let net = load(write_deck("paging", &[("main.sp", &text)]));
    let first = net
        .diagnostics(Some(Level::Error), &Page::default())
        .unwrap();
    assert_eq!(first.total, 75);
    assert_eq!(first.items.len(), 50);
    assert_eq!(first.next_cursor.as_deref(), Some("50"));
    let second = net
        .diagnostics(
            Some(Level::Error),
            &Page {
                limit: 50,
                cursor: first.next_cursor.clone(),
            },
        )
        .unwrap();
    assert_eq!(second.total, 75);
    assert_eq!(second.items.len(), 25);
    assert_eq!(second.next_cursor, None);
    assert!(second.items[0].message.contains(".bad50"));
    // the limit is at most 500
    let big = net
        .diagnostics(
            Some(Level::Error),
            &Page {
                limit: 10_000,
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(big.items.len(), 75);
    // a cursor that is not a decimal offset is an error, not a guess
    let bad = net.diagnostics(
        None,
        &Page {
            limit: 50,
            cursor: Some("abc".to_owned()),
        },
    );
    assert!(matches!(bad, Err(QueryError::BadArgument(_))), "{bad:?}");
}

#[test]
fn analysis_commands_counted_once_per_name() {
    let main = write_deck(
        "analysis",
        &[(
            "main.sp",
            "* t\nR1 a 0 1\n.op\n.meas tran a1 avg v(a)\n.meas tran a2 avg v(a)\n.meas tran a3 avg v(a)\n.tran 1n 10n\n",
        )],
    );
    let net = load(main);
    let diags = all_diags(&net, Some(Level::Info));
    let cmds = with_code(&diags, "analysis_commands");
    assert_eq!(cmds.len(), 3, "{diags:#?}");
    let meas = cmds.iter().find(|d| d.message.contains(".meas")).unwrap();
    assert!(meas.message.contains('3'), "{}", meas.message);
    assert_eq!(meas.loc.as_ref().unwrap().line, 4);
    assert!(all_diags(&net, Some(Level::Error)).is_empty());
}

#[test]
fn protected_region_warned() {
    let main = write_deck(
        "prot",
        &[(
            "main.sp",
            "* t\nR1 a 0 1\n.prot\nsecret text\n.unprot\nR2 a 0 2\n",
        )],
    );
    let net = load(main);
    let diags = all_diags(&net, Some(Level::Warning));
    let prot = with_code(&diags, "protected_region");
    assert_eq!(prot.len(), 1, "{diags:#?}");
    assert!(prot[0].message.contains("lines 3-5"), "{}", prot[0].message);
    assert!(prot[0].message.contains("main.sp"), "{}", prot[0].message);
    assert!(prot[0].message.contains("missing"), "{}", prot[0].message);
}

#[test]
fn lib_override_unused_warned() {
    let dir_deck = repo("golden/cases/lib_corner/deck.sp");
    let opts = |from: &str| LoadOptions {
        lib_override: vec![LibOverride {
            file: "models.lib".into(),
            from: from.to_owned(),
            to: "ff".to_owned(),
        }],
        ..LoadOptions::new(dir_deck.clone())
    };
    let net = Netlist::load(&opts("tt")).unwrap();
    let diags = all_diags(&net, None);
    assert!(
        with_code(&diags, "lib_override_unused").is_empty(),
        "{diags:#?}"
    );
    let net = Netlist::load(&opts("ss")).unwrap();
    let diags = all_diags(&net, Some(Level::Warning));
    let unused = with_code(&diags, "lib_override_unused");
    assert_eq!(unused.len(), 1, "{diags:#?}");
    assert!(unused[0].message.contains("ss"), "{}", unused[0].message);
}

#[test]
fn title_line_info() {
    let net = load(repo("golden/cases/title_line/deck.sp"));
    let diags = all_diags(&net, Some(Level::Info));
    let title = with_code(&diags, "title_line");
    assert_eq!(title.len(), 1, "{diags:#?}");
    assert_eq!(title[0].loc.as_ref().unwrap().line, 1);
    assert!(
        title[0].message.contains("not parsed"),
        "{}",
        title[0].message
    );
    // title_line = no: line 1 is parsed, no title diagnostic
    let net = Netlist::load(&LoadOptions {
        title_line: TitleLine::No,
        ..LoadOptions::new(repo("golden/cases/title_line/deck.sp"))
    })
    .unwrap();
    assert!(with_code(&all_diags(&net, None), "title_line").is_empty());
}

#[test]
fn alter_blocks_renumbered_across_files() {
    let main = write_deck(
        "alter_files",
        &[
            (
                "main.sp",
                "* t\n.param a=1\n.inc 'b.sp'\n.alter\n.param a=3\n",
            ),
            ("b.sp", ".param b=1\n.alter\n.param b=2\n"),
        ],
    );
    let net = load(main);
    assert_eq!(net.summary().alter_runs, 2);
    let diags = all_diags(&net, Some(Level::Info));
    let alters = with_code(&diags, "alter_block");
    assert_eq!(alters.len(), 2, "{diags:#?}");
    assert!(alters[0].message.contains("#1"), "{}", alters[0].message);
    assert!(alters[0].loc.as_ref().unwrap().file.ends_with("b.sp"));
    assert!(alters[1].message.contains("#2"), "{}", alters[1].message);
    assert!(alters[1].loc.as_ref().unwrap().file.ends_with("main.sp"));
    assert_eq!(alters[1].loc.as_ref().unwrap().line, 4);
}

#[test]
fn golden_alter_runs() {
    let net = load(repo("golden/cases/alter/deck.sp"));
    assert_eq!(net.summary().alter_runs, 2);
}

#[test]
fn roots() {
    assert_eq!(
        load(repo("tests/asap7_extracted.sp")).summary().root,
        "none"
    );
    let net = Netlist::load(&LoadOptions {
        top: Some("demo".to_owned()),
        ..LoadOptions::new(repo("tests/top.sp"))
    })
    .unwrap();
    assert_eq!(net.summary().root, "subckt:DEMO");
    // a nested definition by its qualified name
    let net = Netlist::load(&LoadOptions {
        top: Some("OUTER>cell".to_owned()),
        ..LoadOptions::new(repo("golden/cases/nested_def/deck.sp"))
    })
    .unwrap();
    assert_eq!(net.summary().root, "subckt:outer>cell");
}

#[test]
fn load_errors() {
    let r = Netlist::load(&LoadOptions {
        top: Some("nosuch".to_owned()),
        ..LoadOptions::new(repo("tests/top.sp"))
    });
    assert!(matches!(r, Err(LoadError::NoTopLevel(_))), "{:?}", r.err());
    let r = Netlist::load(&LoadOptions::new(repo("tests/no_such_file.sp")));
    match r {
        Err(LoadError::Io(msg)) => assert!(msg.contains("no_such_file.sp"), "{msg}"),
        other => panic!("{:?}", other.err()),
    }
    // `cell` is defined twice (top level and in `outer`): ambiguous
    let r = Netlist::load(&LoadOptions {
        top: Some("deep".to_owned()),
        ..LoadOptions::new(repo("golden/cases/nested_def/deck.sp"))
    });
    assert_eq!(
        r.as_ref().map(|n| n.summary().root).ok(),
        Some("subckt:outer>deep".to_owned()),
        "a unique nested name is found"
    );
    let r = Netlist::load(&LoadOptions {
        top: Some("cell".to_owned()),
        ..LoadOptions::new(repo("golden/cases/nested_def/deck.sp"))
    });
    assert_eq!(
        r.as_ref().map(|n| n.summary().root).ok(),
        Some("subckt:cell".to_owned()),
        "the top-level definition wins over a nested one"
    );
}

#[test]
fn loads_inside_a_tokio_runtime() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let net = rt.block_on(async { load(repo("golden/cases/parhier_local/deck.sp")) });
    assert_eq!(net.summary().parhier, "local");
}

#[test]
fn summary_and_diagnostics_serialize() {
    let net = load(repo("tests/top.sp"));
    let s = serde_json::to_value(net.summary()).unwrap();
    assert_eq!(s["root"], "top-level");
    assert_eq!(s["parhier"], "global");
    let d = serde_json::to_value(net.diagnostics(None, &Page::default()).unwrap()).unwrap();
    assert!(d["total"].as_u64().unwrap() > 0);
    assert!(d["items"][0]["level"].as_str().is_some());
}

#[test]
fn netlist_is_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Netlist>();
}

/// (code, file name, line) of each diagnostic.
fn code_file_line(diags: &[Diagnostic]) -> Vec<(&'static str, String, u32)> {
    diags
        .iter()
        .map(|d| {
            let loc = d.loc.as_ref();
            let file = loc.map_or(String::new(), |l| {
                Path::new(&l.file)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            });
            (d.code, file, loc.map_or(0, |l| l.line))
        })
        .collect()
}

#[test]
fn interner_single_copy() {
    // answers show the first spelling: here the `X` line's `INV`
    let main = write_deck(
        "spelling",
        &[(
            "main.sp",
            "* t\nX1 1 0 INV\n.subckt Inv a b\nr1 a b 1\n.ends\n.SUBCKT inv a b\nr1 a b 2\n.ends\n",
        )],
    );
    let net = Netlist::load(&LoadOptions {
        top: Some("inv".to_owned()),
        ..LoadOptions::new(main.to_path_buf())
    })
    .unwrap();
    assert_eq!(net.summary().root, "subckt:INV");
    let diags = all_diags(&net, Some(Level::Error));
    let dup = with_code(&diags, "duplicate_subckt");
    assert_eq!(dup.len(), 1, "{diags:#?}");
    assert!(dup[0].message.contains("`INV`"), "{}", dup[0].message);
}

#[test]
fn alter_in_included_subckt() {
    let main = write_deck(
        "alter_in_sub",
        &[
            (
                "main.sp",
                "* t\n.subckt holder a b\n.inc 'alt.sp'\nr9 a b 9\n.ends\nx1 1 0 holder\n",
            ),
            ("alt.sp", "r1 a b 1\n.alter\nr2 a b 2\n"),
        ],
    );
    let net = load(main);
    let s = net.summary();
    assert_eq!(s.alter_runs, 0);
    let diags = all_diags(&net, None);
    assert!(with_code(&diags, "alter_block").is_empty(), "{diags:#?}");
    let errs = with_code(&diags, "alter_in_subckt");
    assert_eq!(errs.len(), 1, "{diags:#?}");
    assert_eq!(errs[0].level, Level::Error);
    let loc = errs[0].loc.as_ref().unwrap();
    assert!(loc.file.ends_with("alt.sp"), "{loc:?}");
    assert_eq!(loc.line, 2);
    assert!(errs[0].message.contains("holder"), "{}", errs[0].message);
}

#[test]
fn include_cycle_guard() {
    // a -> b -> c -> a, and the top file includes `a` and `c`. The parser
    // reads each file once, so it gave a cycle error at an include line
    // that depended on the order in which the files were read (and so did
    // the elements). Now lowering finds the cycles in document order: every
    // load gives the same diagnostics and elements. One code, with the
    // include line.
    let main = write_deck(
        "include_cycle",
        &[
            ("main.sp", "* t\n.inc 'a.sp'\n.inc 'c.sp'\n"),
            ("a.sp", ".inc 'b.sp'\nra 1 0 1\n"),
            ("b.sp", ".inc 'c.sp'\nrb 1 0 1\n"),
            ("c.sp", ".inc 'a.sp'\nrc 1 0 1\n"),
        ],
    );
    let dir = main.parent().unwrap().display().to_string();
    let run = || {
        let net = load(&main);
        let diags: Vec<String> = all_diags(&net, None)
            .iter()
            .map(|d| {
                let at = d
                    .loc
                    .as_ref()
                    .map_or(String::new(), |l| format!("{}:{}", l.file, l.line));
                format!("{:?} {} {at} {}", d.level, d.code, d.message).replace(&dir, "")
            })
            .collect();
        let s = net.summary();
        (diags, s.top_level_instances, s.diagnostics_count)
    };
    let first = run();
    for _ in 0..19 {
        assert_eq!(run(), first);
    }
    let (diags, ..) = &first;
    let cycles: Vec<&String> = diags.iter().filter(|d| d.contains("_cycle")).collect();
    // main -> a -> b -> c: the include of `a` in c.sp closes the cycle. A
    // later read of a file reports nothing again.
    assert_eq!(cycles.len(), 1, "{diags:#?}");
    assert!(
        cycles[0].starts_with(
            r#"Error include_cycle /c.sp:1 circular include: "/a.sp" -> "/b.sp" -> "/c.sp" -> "/a.sp". The include of "/a.sp" in "/c.sp" was skipped"#
        ),
        "{}",
        cycles[0]
    );
    assert!(diags.iter().all(|d| !d.contains("circular_include")));
}

#[test]
fn diagnostics_document_order() {
    let main = write_deck(
        "doc_order",
        &[
            (
                "main.sp",
                "* t\n\
                 .option parhier=bad1\n\
                 .subckt s a\n\
                 R2 a b\n\
                 .ends\n\
                 .op\n\
                 .option parhier=bad2\n\
                 R3 a b\n\
                 .inc 'twice.sp'\n\
                 .inc 'twice.sp'\n\
                 .op\n",
            ),
            ("twice.sp", ".option parhier=bad3\nR5 a b\n"),
        ],
    );
    let net = load(main);
    let got = code_file_line(&all_diags(&net, None));
    let want: Vec<(&str, String, u32)> = [
        ("title_line", "main.sp", 1),
        ("bad_option_value", "main.sp", 2),
        ("no_value", "main.sp", 4),
        ("analysis_commands", "main.sp", 6),
        ("bad_option_value", "main.sp", 7),
        ("no_value", "main.sp", 8),
        ("bad_option_value", "twice.sp", 1),
        ("no_value", "twice.sp", 2),
    ]
    .into_iter()
    .map(|(c, f, l)| (c, f.to_owned(), l))
    .collect();
    assert_eq!(got, want);
    let op = all_diags(&net, Some(Level::Info))
        .into_iter()
        .find(|d| d.code == "analysis_commands")
        .unwrap();
    assert!(op.message.starts_with("2 `.op` lines"), "{}", op.message);
}

#[test]
fn summary_first_run() {
    let main = write_deck(
        "first_run",
        &[(
            "main.sp",
            "* t\n.subckt a x\n.ends\nR1 1 0 1\n.alter\n.subckt b x\n.ends\n.subckt c x\n.ends\n",
        )],
    );
    let s = load(main).summary();
    assert_eq!(s.definitions, 1);
    assert_eq!(s.alter_definitions, 2);
    assert_eq!(s.alter_runs, 1);
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["alter_definitions"], 2);
}

#[test]
fn load_error_serializes() {
    let e = Netlist::load(&LoadOptions::new(repo("tests/no_such_file.sp"))).unwrap_err();
    let v = serde_json::to_value(&e).unwrap();
    assert!(
        v["Io"]
            .as_str()
            .is_some_and(|m| m.contains("no_such_file.sp")),
        "{v}"
    );
}

#[test]
fn find_top_rule() {
    let top = |name: &str, text: &str| {
        let main = write_deck(&format!("find_top_{name}"), &[("main.sp", text)]);
        Netlist::load(&LoadOptions {
            top: Some(name.to_owned()),
            ..LoadOptions::new(main.to_path_buf())
        })
        .map(|n| n.summary().root)
    };
    // two top-level `inv` (the second is a duplicate), each nests `cell`:
    // only the first `inv`'s `cell` is active
    let dup = "* t\n.subckt inv a\n.subckt cell a\n.ends\n.ends\n.subckt inv a\n.subckt cell a\n.ends\n.ends\n";
    assert_eq!(top("cell", dup), Ok("subckt:inv>cell".to_owned()));
    // two active nested `cell`: ambiguous, each qualified name once
    let two = "* t\n.subckt p1 a\n.subckt cell a\n.ends\n.ends\n.subckt p2 a\n.subckt cell a\n.ends\n.ends\n";
    match top("cell", two) {
        Err(LoadError::NoTopLevel(msg)) => {
            assert!(msg.contains("p1>cell, p2>cell"), "{msg}")
        }
        other => panic!("{other:?}"),
    }
    // the top-level definition wins over nested ones
    let both = format!("{two}.subckt cell a\n.ends\n");
    assert_eq!(top("CELL", &both), Ok("subckt:cell".to_owned()));
    // a definition in an `.alter` block, and those nested in it, are not
    // first-run definitions
    let alter = "* t\nR1 1 0 1\n.alter\n.subckt late a\n.subckt inner a\n.ends\n.ends\n";
    assert!(matches!(top("inner", alter), Err(LoadError::NoTopLevel(_))));
    assert!(matches!(top("late", alter), Err(LoadError::NoTopLevel(_))));
}
