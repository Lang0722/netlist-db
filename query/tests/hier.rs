//! Hierarchy, counts, subcircuit queries and instance paths (plan Task 7).

use netlist_query::{
    ChildCall, Diagnostic, ElementInfo, Level, LoadOptions, Netlist, Page, QueryError, SubcktRow,
    SubcktSort,
};
use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// A deck written by `write_deck`; its directory is removed on drop.
struct Deck {
    dir: PathBuf,
    main: PathBuf,
}

impl Drop for Deck {
    fn drop(&mut self) {
        _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Writes `files` (name, text) into a new directory for test `name`.
fn write_deck(name: &str, files: &[(&str, &str)]) -> Deck {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("hier")
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

fn load_top(path: impl AsRef<Path>, top: &str) -> Netlist {
    Netlist::load(&LoadOptions {
        top: Some(top.to_owned()),
        ..LoadOptions::new(path.as_ref().to_path_buf())
    })
    .unwrap()
}

fn load_text(name: &str, text: &str) -> (Deck, Netlist) {
    let deck = write_deck(name, &[("main.sp", text)]);
    let net = load(&deck.main);
    (deck, net)
}

fn all_page() -> Page {
    Page {
        limit: 500,
        cursor: None,
    }
}

fn all_diags(net: &Netlist) -> Vec<Diagnostic> {
    let p = net.diagnostics(None, &all_page()).unwrap();
    assert!(p.next_cursor.is_none(), "more than 500 diagnostics");
    p.items
}

/// The diagnostics of level Error or Warning.
fn problems(net: &Netlist) -> Vec<Diagnostic> {
    all_diags(net)
        .into_iter()
        .filter(|d| d.level != Level::Info)
        .collect()
}

fn with_code(net: &Netlist, code: &str) -> Vec<Diagnostic> {
    all_diags(net)
        .into_iter()
        .filter(|d| d.code == code)
        .collect()
}

/// Every row of `list_subckts`, by name.
fn rows(net: &Netlist) -> Vec<SubcktRow> {
    let p = net
        .list_subckts(None, SubcktSort::Name, &all_page())
        .unwrap();
    assert!(p.next_cursor.is_none());
    p.items
}

/// The row of `list_subckts` named `name`.
fn row(net: &Netlist, name: &str) -> SubcktRow {
    let p = net
        .list_subckts(Some(name), SubcktSort::Name, &all_page())
        .unwrap();
    p.items
        .iter()
        .find(|r| r.name == name)
        .cloned()
        .unwrap_or_else(|| panic!("no row {name}: {:#?}", p.items))
}

fn copies(net: &Netlist, name: &str) -> Option<u64> {
    row(net, name).copies
}

fn info(net: &Netlist, path: &str) -> ElementInfo {
    net.element_info(path)
        .unwrap_or_else(|e| panic!("element_info({path}): {e}"))
}

/// The `binds_to` of `c` (the hierarchy was analysed).
fn binds(c: &ChildCall) -> Vec<&str> {
    c.binds_to
        .as_deref()
        .expect("analysed")
        .iter()
        .map(String::as_str)
        .collect()
}

fn expected(case: &str) -> serde_json::Value {
    let text =
        std::fs::read_to_string(repo(&format!("golden/cases/{case}/expected.json"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn expanded(net: &Netlist, kind: &str) -> u64 {
    let s = net.summary();
    let map = s.expanded_devices.expect("expanded device totals");
    map.get(kind).copied().unwrap_or(0)
}

#[test]
fn golden_hier_counts() {
    let net = load(repo("golden/cases/hier_counts/deck.sp"));
    let want = expected("hier_counts");
    for (def, n) in want["extra"]["marker_counts"].as_object().unwrap() {
        assert_eq!(copies(&net, def), n.as_u64(), "copies of {def}");
    }
    assert_eq!(copies(&net, "unused"), Some(0));
    assert_eq!(row(&net, "unused").x_lines, Some(0));
    // `leaf` is called by xl1, xl2, xl3 in `mid` and xl0 in `blk`
    assert_eq!(row(&net, "leaf").x_lines, Some(4));
    assert_eq!(row(&net, "mid").x_lines, Some(3));
    assert_eq!(row(&net, "blk").x_lines, Some(2));
    let totals = &want["totals"];
    assert_eq!(
        expanded(&net, "resistor"),
        totals["resistors"].as_u64().unwrap()
    );
    assert_eq!(
        expanded(&net, "capacitor"),
        totals["capacitors"].as_u64().unwrap()
    );
    assert_eq!(
        expanded(&net, "mosfet"),
        totals["mosfets"].as_u64().unwrap()
    );
    assert_eq!(expanded(&net, "vsource"), 1);
    let s = net.summary();
    assert_eq!(s.expanded_instances, totals["subckt_instances"].as_u64());
    assert_eq!(s.expanded_note, None);
    // `m=2` is reported, never multiplied into the copies
    let xm2 = info(&net, "x1.xm2");
    assert!(
        xm2.params.iter().any(|p| p.name == "m" && p.text == "2"),
        "{xm2:?}"
    );
    let refs = net.instances_of("mid", &all_page()).unwrap();
    let xm2 = refs.items.iter().find(|r| r.instance == "xm2").unwrap();
    assert_eq!(xm2.m.as_deref(), Some("2"));
    assert_eq!(xm2.copies, Some(2), "two `blk` copies, m not multiplied");
}

/// The element counts of the `Circuit Statistics` of the first job in the
/// listing of `case`, by the loader's type name (`k` for `# mutual_inds`,
/// `e` for `# vcvs`, ...); `None` when the listing has none (the `sim_dspf`
/// flow of `dspf_backannot`).
fn listing_counts(case: &str) -> Option<Vec<(&'static str, u64)>> {
    const TYPES: [(&str, &str); 21] = [
        ("resistors", "resistor"),
        ("capacitors", "capacitor"),
        ("inductors", "inductor"),
        ("mutual_inds", "k"),
        ("vccs", "g"),
        ("vcvs", "e"),
        ("cccs", "f"),
        ("ccvs", "h"),
        ("volt_srcs", "vsource"),
        ("curr_srcs", "isource"),
        ("diodes", "diode"),
        ("bjts", "bjt"),
        ("jfets", "j"),
        ("mosfets", "mosfet"),
        ("U elements", "u"),
        ("T elements", "t"),
        ("W elements", "w"),
        ("B elements", "b"),
        ("S elements", "s"),
        ("P elements", "p"),
        ("N elements", "n"),
    ];
    let text = std::fs::read_to_string(repo(&format!("golden/cases/{case}/hspice.lis"))).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains("Circuit Statistics"))?;
    let mut counts = Vec::new();
    for line in &lines[start + 1..(start + 12).min(lines.len())] {
        for part in line.split('#').skip(1) {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            if let Some(&(_, kind)) = TYPES.iter().find(|t| t.0 == key.trim()) {
                counts.push((kind, value.trim().parse().unwrap()));
            }
        }
    }
    assert_eq!(counts.len(), TYPES.len(), "{case}: {counts:?}");
    Some(counts)
}

#[test]
fn golden_totals_all_cases() {
    // every case HSPICE ran: the expanded totals of the first run equal the
    // `Circuit Statistics` of the listing, for every element type (the
    // letters the parser does not read by their letter name). The loader
    // skips a statement that HSPICE counts in these cases, with an error
    // diagnostic that the summary notes.
    let skipped_with_error = ["cap_value_model"];
    let mut checked = 0;
    for entry in std::fs::read_dir(repo("golden/cases")).unwrap() {
        let dir = entry.unwrap().path();
        let case = dir.file_name().unwrap().to_str().unwrap().to_owned();
        let want = expected(&case);
        if want["extra"].get("rejected").is_some() {
            continue;
        }
        let net = Netlist::load(&LoadOptions::new(dir.join("deck.sp"))).unwrap();
        let s = net.summary();
        if skipped_with_error.contains(&case.as_str()) {
            assert!(s.errors > 0, "{case}");
            assert!(
                s.expanded_note
                    .as_deref()
                    .is_some_and(|n| n.contains("error")),
                "{case}: {s:?}"
            );
            continue;
        }
        let mut got = s
            .expanded_devices
            .clone()
            .unwrap_or_else(|| panic!("{case}: no totals: {:?}", s.expanded_note));
        // HSPICE counts the parasitic R and C of an included DSPF
        // (`dspf_included`); the summary gives them apart (Task 10)
        for (kind, n) in s.expanded_parasitics.clone().unwrap_or_default() {
            *got.entry(kind).or_default() += n;
        }
        let totals = &want["totals"];
        for (key, kind) in [
            ("resistors", "resistor"),
            ("capacitors", "capacitor"),
            ("mosfets", "mosfet"),
        ] {
            let Some(n) = totals[key].as_u64() else {
                continue;
            };
            // `dspf_backannot`: the totals are those of the pre-layout deck
            assert_eq!(
                got.get(kind).copied().unwrap_or(0),
                n,
                "{case}: {kind} ({got:?})"
            );
        }
        if let Some(n) = totals["subckt_instances"].as_u64() {
            assert_eq!(s.expanded_instances, Some(n), "{case}: instances");
        }
        if let Some(counts) = listing_counts(&case) {
            for (kind, n) in counts {
                assert_eq!(
                    got.get(kind).copied().unwrap_or(0),
                    n,
                    "{case}: {kind} ({got:?})"
                );
            }
        }
        checked += 1;
    }
    assert!(checked > 75, "{checked} cases");
}

#[test]
fn golden_hier_empty_kinds() {
    // empty subckts, a subckt with only a `.param`, and every element
    // type that HSPICE counts, under an `m=2` instance too
    let net = load(repo("golden/cases/hier_empty_kinds/deck.sp"));
    let counts = listing_counts("hier_empty_kinds").unwrap();
    for (kind, n) in counts {
        assert_eq!(expanded(&net, kind), n, "{kind}");
    }
    assert_eq!(expanded(&net, "k"), 2);
    assert_eq!(expanded(&net, "e"), 2);
    assert_eq!(expanded(&net, "vsource"), 3);
    assert_eq!(copies(&net, "empty"), Some(2));
    assert_eq!(copies(&net, "kinds"), Some(2));
    let want = expected("hier_empty_kinds");
    assert_eq!(
        net.summary().expanded_instances,
        want["totals"]["subckt_instances"].as_u64()
    );
    assert_eq!(net.summary().expanded_note, None);
}

#[test]
fn bare_netlist_no_root() {
    let path = repo("tests/asap7_extracted.sp");
    let net = load(&path);
    let all = rows(&net);
    assert_eq!(all.len(), 3);
    for r in &all {
        assert_eq!(r.copies, None, "{r:?}");
        assert_eq!(r.copies_note.as_deref(), Some("no top-level instances"));
    }
    // x_lines work without a root
    assert_eq!(row(&net, "PM_INVx1_ASAP7_75t_R%A").x_lines, Some(1));
    assert_eq!(row(&net, "PM_INVx1_ASAP7_75t_R%Y").x_lines, Some(1));
    let s = net.summary();
    assert_eq!(s.expanded_devices, None);
    assert_eq!(s.expanded_instances, None);
    assert_eq!(s.expanded_note.as_deref(), Some("no top-level instances"));
    // instance paths need a root
    assert!(matches!(
        net.element_info("m0"),
        Err(QueryError::BadArgument(_))
    ));

    // `top=` the first subckt
    let net = load_top(&path, "PM_INVx1_ASAP7_75t_R%A");
    assert_eq!(copies(&net, "PM_INVx1_ASAP7_75t_R%A"), Some(1));
    assert_eq!(copies(&net, "PM_INVx1_ASAP7_75t_R%Y"), Some(0));
    assert_eq!(copies(&net, "INVx1_ASAP7_75t_R"), Some(0));
    assert_eq!(expanded(&net, "capacitor"), 8);
    assert_eq!(net.summary().expanded_instances, Some(0));
    assert_eq!(info(&net, "c1").def, "PM_INVx1_ASAP7_75t_R%A");
    // `top=` the cell
    let net = load_top(&path, "invx1_asap7_75t_r");
    assert_eq!(copies(&net, "PM_INVx1_ASAP7_75t_R%A"), Some(1));
    assert_eq!(copies(&net, "PM_INVx1_ASAP7_75t_R%Y"), Some(1));
    assert_eq!(expanded(&net, "mosfet"), 2);
    assert_eq!(net.summary().expanded_instances, Some(2));
    let x = info(&net, "x_pm_invx1_asap7_75t_r%y");
    assert_eq!(x.binds_to.as_deref(), Some("PM_INVx1_ASAP7_75t_R%Y"));
    let pins: Vec<(&str, &str)> = x
        .pins
        .iter()
        .map(|p| (p.role.as_str(), p.net.as_str()))
        .collect();
    assert_eq!(
        pins,
        [
            ("vss", "vss"),
            ("17", "Y"),
            ("13", "N_M0_d"),
            ("25", "N_M1_d"),
            ("11", "N_Y_11"),
            ("8", "N_Y_8"),
            ("7", "N_Y_7"),
        ]
    );
    assert_eq!(
        info(&net, "x_pm_invx1_asap7_75t_r%y.r1").def,
        "PM_INVx1_ASAP7_75t_R%Y"
    );
}

#[test]
fn golden_nested_def() {
    let net = load(repo("golden/cases/nested_def/deck.sp"));
    assert_eq!(copies(&net, "cell"), Some(1));
    assert_eq!(copies(&net, "outer>cell"), Some(2));
    assert_eq!(copies(&net, "outer>deep"), Some(1));
    assert_eq!(copies(&net, "outer"), Some(1));
    assert_eq!(info(&net, "x1.xc.rc").def, "outer>cell");
    assert_eq!(info(&net, "x1.xd.xc.rc").def, "outer>cell");
    assert_eq!(info(&net, "x2.rc").def, "cell");
    assert_eq!(info(&net, "x1.xc").binds_to.as_deref(), Some("outer>cell"));
    assert_eq!(info(&net, "x1.xc").callee.as_deref(), Some("cell"));
    assert_eq!(info(&net, "x2").binds_to.as_deref(), Some("cell"));
}

#[test]
fn golden_nested_def_dynamic() {
    let net = load(repo("golden/cases/nested_def_dynamic/deck.sp"));
    // the one `xc` line of `deep2` gives 1 copy of the top-level `cell`
    // (under x2) and 2 copies of `outer`'s `cell` (under x1)
    assert_eq!(copies(&net, "cell"), Some(1));
    assert_eq!(copies(&net, "outer>cell"), Some(2));
    assert_eq!(copies(&net, "deep2"), Some(3));
    assert_eq!(copies(&net, "mid"), Some(1));
    assert_eq!(info(&net, "x1.xd.xc.rc").def, "outer>cell");
    assert_eq!(info(&net, "x1.xm.xd.xc.rc").def, "outer>cell");
    assert_eq!(info(&net, "x2.xc.rc").def, "cell");
    assert_eq!(
        info(&net, "x1.xd.xc").binds_to.as_deref(),
        Some("outer>cell")
    );
    assert_eq!(info(&net, "x2.xc").binds_to.as_deref(), Some("cell"));
    // `xc` in `deep2` calls both definitions, one X line each
    assert_eq!(row(&net, "cell").x_lines, Some(1));
    assert_eq!(row(&net, "outer>cell").x_lines, Some(1));
    let top = net.instances_of("cell", &all_page()).unwrap();
    assert_eq!(top.total, 1);
    assert_eq!(top.items[0].parent, "deep2");
    assert_eq!(top.items[0].copies, Some(1));
    let nested = net.instances_of("outer>cell", &all_page()).unwrap();
    assert_eq!(nested.total, 1);
    assert_eq!(nested.items[0].copies, Some(2));
    let s = net.summary();
    assert_eq!(expanded(&net, "resistor"), 3);
    assert_eq!(s.expanded_instances, Some(8));
    // `nested_def_reach`: the nearest instance ancestor wins over the text
    // parent, and a name with no top-level definition resolves under an
    // instance of the definition that nests it (more in
    // `golden_nested_def_reach`)
    let reach = load(repo("golden/cases/nested_def_reach/deck.sp"));
    assert_eq!(info(&reach, "x1.xu.xd.xc.rc").def, "u>cell");
    assert_eq!(
        info(&reach, "x1.xt.xn").binds_to.as_deref(),
        Some("outer>only")
    );
    // the children of `deep2` name both bindings
    let deep2 = net.subckt_info("deep2", &all_page()).unwrap();
    assert_eq!(deep2.children.len(), 1);
    assert_eq!(deep2.children[0].subckt, "cell");
    assert_eq!(deep2.children[0].x_lines, 1);
    assert_eq!(binds(&deep2.children[0]), ["cell", "outer>cell"]);
    assert!(with_code(&net, "unresolved_subckt").is_empty());
}

#[test]
fn golden_nested_def_reach() {
    let net = load(repo("golden/cases/nested_def_reach/deck.sp"));
    // the nearest instance ancestor (`u`) wins over the text parent
    // (`outer`) of `deep`
    assert_eq!(info(&net, "x1.xu.xd.xc.rc").def, "u>cell");
    assert_eq!(
        info(&net, "x1.xu.xd").binds_to.as_deref(),
        Some("outer>deep")
    );
    // `only` exists only nested in `outer`: it resolves under x1.xt
    assert_eq!(
        info(&net, "x1.xt.xn").binds_to.as_deref(),
        Some("outer>only")
    );
    assert_eq!(info(&net, "x1.xt.xn.ro").def, "outer>only");
    assert_eq!(copies(&net, "u>cell"), Some(1));
    assert_eq!(copies(&net, "outer>cell"), Some(0));
    assert_eq!(copies(&net, "outer>only"), Some(1));
    assert_eq!(copies(&net, "t"), Some(1));
    // the deck has no path where `only` is missing
    assert!(
        with_code(&net, "unresolved_subckt").is_empty(),
        "{:#?}",
        all_diags(&net)
    );
    assert_eq!(net.summary().expanded_instances, Some(6));
}

#[test]
fn top_nested_definition_sees_its_text_parents() {
    // a choice (golden/NOTES.md, "Choices not probed"): `top=outer>deep`
    // looks up names as if `deep` were instantiated inside an `outer`
    let net = load_top(repo("golden/cases/nested_def/deck.sp"), "outer>deep");
    assert_eq!(info(&net, "xc").binds_to.as_deref(), Some("outer>cell"));
    assert_eq!(info(&net, "xc.rc").def, "outer>cell");
    assert_eq!(copies(&net, "outer>cell"), Some(1));
    assert_eq!(copies(&net, "cell"), Some(0));
    assert_eq!(copies(&net, "outer>deep"), Some(1));
}

#[test]
fn cycle_reported() {
    let (_deck, net) = load_text(
        "cycle",
        "* t\n\
         .subckt a p\n\
         xb p b\n\
         .ends\n\
         .subckt b p\n\
         xa p a\n\
         xl p leaf\n\
         .ends\n\
         .subckt leaf p\n\
         r1 p 0 1\n\
         .ends\n\
         .subckt fine p\n\
         r1 p 0 1\n\
         .ends\n\
         x1 1 a\n\
         x2 1 fine\n",
    );
    let cycles = with_code(&net, "subckt_cycle");
    assert_eq!(cycles.len(), 1, "{:#?}", all_diags(&net));
    assert_eq!(cycles[0].level, Level::Error);
    assert!(
        cycles[0].message.contains("`a`") && cycles[0].message.contains("`b`"),
        "{}",
        cycles[0].message
    );
    assert!(
        cycles[0].message.contains("HSPICE rejects"),
        "{}",
        cycles[0].message
    );
    assert!(cycles[0].loc.is_some());
    for def in ["a", "b", "leaf"] {
        let r = row(&net, def);
        assert_eq!(r.copies, None, "{def}");
        assert!(
            r.copies_note
                .as_deref()
                .is_some_and(|n| n.contains("cycle")),
            "{r:?}"
        );
    }
    assert_eq!(copies(&net, "fine"), Some(1));
    let s = net.summary();
    assert_eq!(s.expanded_devices, None);
    assert!(
        s.expanded_note
            .as_deref()
            .is_some_and(|n| n.contains("cycle"))
    );
    // a self call
    let (_deck, net) = load_text("self_cycle", "* t\n.subckt s p\nxs p s\n.ends\nx1 1 s\n");
    assert_eq!(with_code(&net, "subckt_cycle").len(), 1);
    assert_eq!(copies(&net, "s"), None);
}

#[test]
fn unused_cycle_has_no_copies() {
    // a cycle that no instance reaches: reported as a Warning (HSPICE runs
    // such a deck: golden `hier_unused_cycle_unres`), and nothing expands
    let (_deck, net) = load_text(
        "unused_cycle",
        "* t\n.subckt a p\nxb p b\n.ends\n.subckt b p\nxa p a\n.ends\nr1 1 0 1\n",
    );
    let cycles = with_code(&net, "subckt_cycle");
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0].level, Level::Warning);
    assert_eq!(net.summary().errors, 0);
    assert_eq!(copies(&net, "a"), Some(0));
    assert_eq!(copies(&net, "b"), Some(0));
    assert_eq!(expanded(&net, "resistor"), 1);
}

#[test]
fn golden_hier_unused_cycle_unres() {
    // HSPICE runs the deck with no warning: an unused `loop` that calls
    // itself, and an unused `orphan` that calls an undefined name
    let net = load(repo("golden/cases/hier_unused_cycle_unres/deck.sp"));
    let s = net.summary();
    assert_eq!(s.errors, 0, "{:#?}", all_diags(&net));
    let cycles = with_code(&net, "subckt_cycle");
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0].level, Level::Warning);
    assert!(
        cycles[0].message.contains("HSPICE runs the deck"),
        "{}",
        cycles[0].message
    );
    let unres = with_code(&net, "unresolved_subckt");
    assert_eq!(unres.len(), 1);
    assert_eq!(unres[0].level, Level::Warning);
    assert!(
        unres[0].message.contains("`xq`") && unres[0].message.contains("`nosuch`"),
        "{}",
        unres[0].message
    );
    assert_eq!(copies(&net, "loop"), Some(0));
    assert_eq!(copies(&net, "orphan"), Some(0));
    assert_eq!(copies(&net, "used"), Some(1));
    assert_eq!(expanded(&net, "resistor"), 1);
    assert_eq!(s.expanded_instances, Some(1));
    assert_eq!(s.expanded_note, None);
}

#[test]
fn golden_hier_reentry() {
    // HSPICE rejects a definition instantiated inside an instance of
    // itself, also when the inner instance binds its `X` lines elsewhere
    // so that the expansion ends: `aa` -> `bb` -> `aa` -> `bb>bb` (`aa`
    // nests `zz`, which no line calls), and the same with `a` and `s`
    for (case, def) in [("hier_reentry", "aa"), ("hier_reentry_unscoped", "a")] {
        let want = expected(case);
        let rejected = want["extra"]["rejected"].as_str().unwrap();
        assert!(rejected.contains("defined recursively"), "{rejected}");
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let cycles = with_code(&net, "subckt_cycle");
        assert_eq!(cycles.len(), 1, "{case}: {:#?}", all_diags(&net));
        assert_eq!(cycles[0].level, Level::Error);
        let inside = format!("an instance of `{def}` is inside an instance of `{def}`");
        assert!(cycles[0].message.contains(&inside), "{}", cycles[0].message);
        assert_eq!(copies(&net, def), None, "{case}");
        assert!(
            row(&net, def)
                .copies_note
                .as_deref()
                .is_some_and(|n| n.contains("subckt_cycle")),
            "{case}"
        );
        let s = net.summary();
        assert!(s.errors >= 1, "{case}");
        assert_eq!(s.expanded_devices, None, "{case}");
        assert_eq!(s.expanded_instances, None, "{case}");
        assert!(
            s.expanded_note
                .as_deref()
                .is_some_and(|n| n.contains("subckt_cycle")),
            "{case}: {s:?}"
        );
    }
    let net = load(repo("golden/cases/hier_reentry/deck.sp"));
    let cycle = &with_code(&net, "subckt_cycle")[0];
    assert!(
        cycle.message.contains("`aa` (`xb`) -> `bb` (`xa`) -> `aa`"),
        "{}",
        cycle.message
    );
    assert_eq!(cycle.loc.as_ref().unwrap().line, 14);
    // the definitions in and below the cycle have no number
    assert_eq!(copies(&net, "bb"), None);
    assert_eq!(copies(&net, "bb>bb"), None);
    assert_eq!(copies(&net, "aa>zz"), Some(0));
    // the path still answers, with a caveat
    let rn = info(&net, "x1.xb.xa.xb.rn");
    assert_eq!(rn.def, "bb>bb");
    assert!(
        rn.caveats.iter().any(|c| c.contains("subckt_cycle")),
        "{:?}",
        rn.caveats
    );
}

#[test]
fn reentry_in_unused_definition_is_a_warning() {
    // `a` -> `s` -> `a` -> `s>s` in a definition with no instances
    let (_deck, net) = load_text(
        "reentry_unused",
        "* t\n\
         .subckt a p\nxs p s\n.ends\n\
         .subckt s p\n.subckt s p\nr1 p 0 1\n.ends\nxa p a\n.ends\n\
         .subckt fine p\nr1 p 0 1\n.ends\n\
         x1 1 fine\n",
    );
    let cycles = with_code(&net, "subckt_cycle");
    assert_eq!(cycles.len(), 1, "{:#?}", all_diags(&net));
    assert_eq!(cycles[0].level, Level::Warning);
    assert_eq!(copies(&net, "a"), Some(0));
    assert_eq!(expanded(&net, "resistor"), 1);
    assert_eq!(net.summary().errors, 0);
}

#[test]
fn golden_hier_same_name_nested() {
    // an `X` line that calls the name of a definition on its own instance
    // path binds to a nested definition: no recursion, HSPICE runs it
    let net = load(repo("golden/cases/hier_same_name_nested/deck.sp"));
    assert!(with_code(&net, "subckt_cycle").is_empty());
    assert_eq!(net.summary().errors, 0, "{:#?}", all_diags(&net));
    assert_eq!(info(&net, "x1.xc.rin").def, "cell>cell");
    assert_eq!(info(&net, "x1.rout").def, "cell");
    assert_eq!(info(&net, "x2.xw.xt.rt2").def, "top2>top2");
    for def in ["cell", "cell>cell", "top2", "wrap", "top2>top2"] {
        assert_eq!(copies(&net, def), Some(1), "{def}");
    }
}

#[test]
fn golden_hier_shadow_own() {
    // a name nested in P's own definition and in an ancestor's: P's own
    // wins; the nearest instance ancestor wins over a farther one
    let net = load(repo("golden/cases/hier_shadow_own/deck.sp"));
    let want = expected("hier_shadow_own");
    let defs = [
        ("x2.xc.rc", "mid>cell"),
        ("x1.xm.xc.rc", "mid>cell"),
        ("x1.xd.xc.rc", "outer>cell"),
        ("x1.xn.xd.xc.rc", "mid2>cell"),
    ];
    for (path, def) in defs {
        assert_eq!(info(&net, path).def, def, "{path}");
        let r = want["resistors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["path"] == path)
            .unwrap();
        // `rc` is 1, 2, 3, 4 in `cell`, `outer>cell`, `mid>cell`,
        // `mid2>cell`
        let value = ["cell", "outer>cell", "mid>cell", "mid2>cell"]
            .iter()
            .position(|d| *d == def)
            .unwrap() as f64
            + 1.0;
        assert_eq!(r["value"].as_f64(), Some(value), "{path}");
    }
    assert_eq!(copies(&net, "cell"), Some(0));
    assert_eq!(copies(&net, "mid>cell"), Some(2));
}

#[test]
fn golden_hier_top_nested_only() {
    // a top-level `X` line calls a name defined only nested in `outer`:
    // HSPICE rejects the deck
    let net = load(repo("golden/cases/hier_top_nested_only/deck.sp"));
    let want = expected("hier_top_nested_only");
    assert!(
        want["extra"]["rejected"]
            .as_str()
            .unwrap()
            .contains("is not found for the element \"x2\"")
    );
    let unres = with_code(&net, "unresolved_subckt");
    assert_eq!(unres.len(), 1, "{unres:#?}");
    assert_eq!(unres[0].level, Level::Error);
    let m = &unres[0].message;
    assert!(m.contains("`x2`") && m.contains("`only`"), "{m}");
    assert!(m.contains("HSPICE rejects"), "{m}");
    assert_eq!(unres[0].loc.as_ref().unwrap().line, 11);
    let s = net.summary();
    assert_eq!(s.errors, 1);
    assert!(
        s.expanded_note
            .as_deref()
            .is_some_and(|n| n.contains("lower bound")),
        "{s:?}"
    );
}

#[test]
fn golden_hier_unres_in_subckt() {
    // `xn` in `t` resolves under x1 (an `outer`), not under x2: HSPICE
    // rejects the deck
    let net = load(repo("golden/cases/hier_unres_in_subckt/deck.sp"));
    let want = expected("hier_unres_in_subckt");
    assert!(
        want["extra"]["rejected"]
            .as_str()
            .unwrap()
            .contains("for the element \"x2.xn\"")
    );
    let unres = with_code(&net, "unresolved_subckt");
    assert_eq!(unres.len(), 1, "{unres:#?}");
    assert_eq!(unres[0].level, Level::Error);
    let m = &unres[0].message;
    assert!(m.contains("`xn` in subckt `t`"), "{m}");
    assert!(m.contains("on 1 of the 2 instance paths of `t`"), "{m}");
    assert_eq!(unres[0].loc.as_ref().unwrap().line, 4);
    assert_eq!(info(&net, "x1.xt.xn.ro").def, "outer>only");
    assert!(net.element_info("x2.xn.ro").is_err());
    let t = net.subckt_info("t", &all_page()).unwrap();
    assert_eq!(t.children[0].unresolved, Some(true));
    assert_eq!(binds(&t.children[0]), ["outer>only"]);
}

#[test]
fn golden_hier_unused_caller_reach() {
    // `spare` (no instances) calls `t`, whose `xn` resolves only under an
    // `outer`. On the instance paths of `t` it resolves, and HSPICE runs
    // the deck: no diagnostic, and the bindings of `t` are those of its
    // instance paths
    let net = load(repo("golden/cases/hier_unused_caller_reach/deck.sp"));
    assert!(problems(&net).is_empty(), "{:#?}", problems(&net));
    assert_eq!(
        info(&net, "x1.xt.xn").binds_to.as_deref(),
        Some("outer>only")
    );
    let t = net.subckt_info("t", &all_page()).unwrap();
    assert_eq!(binds(&t.children[0]), ["outer>only"]);
    assert_eq!(t.children[0].unresolved, Some(false));
    assert_eq!(copies(&net, "t"), Some(1));
    // `xt` in `outer`, and `xs` in `spare` (as if at the top level)
    assert_eq!(row(&net, "t").x_lines, Some(2));
    assert_eq!(row(&net, "outer>only").x_lines, Some(1));
    // three `outer` instances: still no diagnostic
    let text = std::fs::read_to_string(repo("golden/cases/hier_unused_caller_reach/deck.sp"))
        .unwrap()
        .replace(
            "x1 1 0 outer\n",
            "x1 1 0 outer\nx2 1 0 outer\nx3 1 0 outer\n",
        );
    let (_deck, net) = load_text("unused_caller_reach3", &text);
    assert!(problems(&net).is_empty(), "{:#?}", problems(&net));
    assert_eq!(copies(&net, "t"), Some(3));
}

#[test]
fn golden_hier_unused_nested() {
    // a used `outer` nests an unused `helper` that calls an undefined name,
    // or an unused `loop` that calls itself: HSPICE runs both decks with no
    // warning. The loader checks the nested definition inside an imagined
    // instance of `outer` and gives a Warning
    let net = load(repo("golden/cases/hier_unused_nested_unres/deck.sp"));
    assert_eq!(net.summary().errors, 0, "{:#?}", all_diags(&net));
    let unres = with_code(&net, "unresolved_subckt");
    assert_eq!(unres.len(), 1, "{unres:#?}");
    assert_eq!(unres[0].level, Level::Warning);
    let m = &unres[0].message;
    assert!(
        m.contains("`xq` in subckt `outer>helper`") && m.contains("`nosuch`"),
        "{m}"
    );
    assert!(
        m.contains("as if instantiated inside an instance of `outer`"),
        "{m}"
    );
    assert!(!m.contains("at the top level"), "{m}");
    assert_eq!(unres[0].loc.as_ref().unwrap().line, 4);
    assert_eq!(copies(&net, "outer>helper"), Some(0));
    let want = expected("hier_unused_nested_unres");
    assert_eq!(
        expanded(&net, "resistor"),
        want["totals"]["resistors"].as_u64().unwrap()
    );
    assert_eq!(
        net.summary().expanded_instances,
        want["totals"]["subckt_instances"].as_u64()
    );

    let net = load(repo("golden/cases/hier_unused_nested_cycle/deck.sp"));
    assert_eq!(net.summary().errors, 0, "{:#?}", all_diags(&net));
    let cycles = with_code(&net, "subckt_cycle");
    assert_eq!(cycles.len(), 1, "{cycles:#?}");
    assert_eq!(cycles[0].level, Level::Warning);
    assert!(
        cycles[0]
            .message
            .contains("`outer>loop` (`xl`) -> `outer>loop`"),
        "{}",
        cycles[0].message
    );
    assert_eq!(copies(&net, "outer>loop"), Some(0));
    let want = expected("hier_unused_nested_cycle");
    assert_eq!(
        expanded(&net, "resistor"),
        want["totals"]["resistors"].as_u64().unwrap()
    );
    assert_eq!(net.summary().expanded_note, None);
}

#[test]
fn golden_hier_dup_nested() {
    // two nested definitions with one name in one parent: HSPICE rejects
    // the deck at the second one
    let want = expected("hier_dup_nested");
    let rejected = want["extra"]["rejected"].as_str().unwrap();
    assert!(
        rejected.contains("(deck.sp:6) duplicated subckt definition 'cell'"),
        "{rejected}"
    );
    let net = load(repo("golden/cases/hier_dup_nested/deck.sp"));
    let dups = with_code(&net, "duplicate_subckt");
    assert_eq!(dups.len(), 1, "{dups:#?}");
    assert_eq!(dups[0].level, Level::Error);
    assert_eq!(dups[0].loc.as_ref().unwrap().line, 6);
    let m = &dups[0].message;
    assert!(m.contains("HSPICE rejects this deck"), "{m}");
    assert!(m.contains("hier_dup_nested"), "{m}");
    assert!(!m.contains("not tested"), "{m}");
    // the first definition is used
    assert_eq!(info(&net, "x1.xc.rc").def, "outer>cell");
}

#[test]
fn golden_m_not_positive() {
    // HSPICE rejects an element or `X` line with `m` <= 0 on an instance
    // path: "Multiplication factor(M) for the element x1.x1 cannot be < or
    // = zero"
    for (case, lines) in [
        ("hier_m_zero", &[6, 9][..]),
        ("hier_m_negative", &[6][..]),
        // the top level first, as HSPICE lists them
        ("elem_m_zero", &[8, 3][..]),
        ("elem_m_negative", &[8, 3][..]),
    ] {
        let want = expected(case);
        let rejected = want["extra"]["rejected"].as_str().unwrap();
        assert!(
            rejected.contains("Multiplication factor(M)"),
            "{case}: {rejected}"
        );
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let ms = with_code(&net, "m_not_positive");
        let got: Vec<u32> = ms.iter().map(|d| d.loc.as_ref().unwrap().line).collect();
        assert_eq!(got, lines, "{case}: {ms:#?}");
        assert!(ms.iter().all(|d| d.level == Level::Error), "{case}");
        assert!(
            ms.iter().all(|d| d.message.contains("HSPICE rejects")),
            "{case}: {ms:#?}"
        );
        let s = net.summary();
        assert_eq!(s.errors, lines.len(), "{case}: {:#?}", all_diags(&net));
        assert!(
            s.expanded_note
                .as_deref()
                .is_some_and(|n| n.contains("m_not_positive")),
            "{case}: {s:?}"
        );
    }
    let net = load(repo("golden/cases/hier_m_zero/deck.sp"));
    let refs = net.instances_of("cell", &all_page()).unwrap();
    let x1 = refs.items.iter().find(|r| r.instance == "x1").unwrap();
    assert_eq!(x1.m.as_deref(), Some("0"));
    assert!(
        x1.caveats.iter().any(|c| c.contains("m_not_positive")),
        "{x1:?}"
    );
    let x2 = refs.items.iter().find(|r| r.instance == "x2").unwrap();
    assert!(x2.caveats.is_empty(), "{x2:?}");
    assert!(
        info(&net, "x1.rc")
            .caveats
            .iter()
            .any(|c| c.contains("`x1` has `m=0`")),
        "{:?}",
        info(&net, "x1.rc").caveats
    );
    // the sign stays in the message
    let net = load(repo("golden/cases/elem_m_negative/deck.sp"));
    let ms = with_code(&net, "m_not_positive");
    assert!(ms[0].message.contains("`m=-1`"), "{}", ms[0].message);
    assert!(ms[1].message.contains("`m=-0.5`"), "{}", ms[1].message);
}

#[test]
fn golden_elem_m_zero_unused() {
    // `m=0` on lines of a subckt with no instances: HSPICE runs the deck,
    // so the loader gives Warnings only
    let net = load(repo("golden/cases/elem_m_zero_unused/deck.sp"));
    let s = net.summary();
    assert_eq!(s.errors, 0, "{:#?}", all_diags(&net));
    let ms = with_code(&net, "m_not_positive");
    let got: Vec<(Level, u32)> = ms
        .iter()
        .map(|d| (d.level, d.loc.as_ref().unwrap().line))
        .collect();
    assert_eq!(got, [(Level::Warning, 6), (Level::Warning, 7)]);
    assert!(
        ms[0].message.contains("elem_m_zero_unused"),
        "{}",
        ms[0].message
    );
    let want = expected("elem_m_zero_unused");
    assert_eq!(
        expanded(&net, "resistor"),
        want["totals"]["resistors"].as_u64().unwrap()
    );
    assert_eq!(s.expanded_note, None);
    // a positive `m`, and an `m` expression (not evaluated here), give none
    let (_deck, net) = load_text(
        "m_fine",
        "* t\n.param mz=0\n.subckt c a\nr1 a 0 1 m=2\n.ends\nx1 1 c m=mz\nx2 1 c m=0.5\nr2 1 0 1 m='1-1'\n",
    );
    assert!(with_code(&net, "m_not_positive").is_empty());
    // a number in quotes, with a sign, is checked
    let (_deck, net) = load_text(
        "m_quoted",
        "* t\n.subckt c a\nr1 a 0 1\n.ends\nx1 1 c m='0'\nc1 1 0 1p m=+0\n",
    );
    let lines: Vec<u32> = with_code(&net, "m_not_positive")
        .iter()
        .map(|d| d.loc.as_ref().unwrap().line)
        .collect();
    assert_eq!(lines, [5, 6]);
}

#[test]
fn imagined_cycle_through_a_reached_definition_is_a_warning() {
    // the used `a` calls `b` (the top-level one on its instance path). The
    // unused `spare` nests a `b` that calls `a`, and calls `a`: in the
    // imagined instance of `spare`, `a` binds `b` to `spare>b`, a cycle.
    // HSPICE does not check it (no instance path reaches `spare`): a
    // Warning, and the answers about `a` keep its instance paths
    let (_deck, net) = load_text(
        "imagined_cycle",
        "* t\n\
         .subckt a p\nxb p b\n.ends\n\
         .subckt b p\nr1 p 0 1\n.ends\n\
         .subckt spare p\n.subckt b p\nxa p a\n.ends\nxs p a\n.ends\n\
         x1 1 a\n",
    );
    assert_eq!(net.summary().errors, 0, "{:#?}", all_diags(&net));
    let cycles = with_code(&net, "subckt_cycle");
    assert_eq!(cycles.len(), 1, "{cycles:#?}");
    assert_eq!(cycles[0].level, Level::Warning);
    assert!(
        cycles[0]
            .message
            .contains("`a` (`xb`) -> `spare>b` (`xa`) -> `a`"),
        "{}",
        cycles[0].message
    );
    let a = net.subckt_info("a", &all_page()).unwrap();
    assert_eq!(binds(&a.children[0]), ["b"]);
    assert_eq!(copies(&net, "a"), Some(1));
    assert_eq!(copies(&net, "b"), Some(1));
    // `x1`, and `xs` in `spare` and `xa` in `spare>b` (no instances)
    assert_eq!(row(&net, "a").x_lines, Some(3));
    assert_eq!(expanded(&net, "resistor"), 1);
}

#[test]
fn bindings_of_unused_callers_stay_out_of_reached_definitions() {
    // `t` binds `cell` to `outer>cell` on its one instance path. The
    // unused `spare` calls `t` too: checked as if at the top level, where
    // `cell` would be the top-level one. That imagined binding is not on
    // an instance path of `t`, so the answers about `t` and `cell` leave
    // it out
    let (_deck, net) = load_text(
        "imagined",
        "* t\n\
         .subckt cell a\nrc a 0 1\n.ends\n\
         .subckt t a\nxc a cell\n.ends\n\
         .subckt outer a\n.subckt cell a\nrc a 0 2\n.ends\nxt a t\n.ends\n\
         .subckt spare a\nxs a t\n.ends\n\
         x1 1 outer\n",
    );
    assert!(problems(&net).is_empty(), "{:#?}", problems(&net));
    assert_eq!(row(&net, "cell").x_lines, Some(0));
    assert_eq!(copies(&net, "cell"), Some(0));
    assert_eq!(row(&net, "outer>cell").x_lines, Some(1));
    let t = net.subckt_info("t", &all_page()).unwrap();
    assert_eq!(binds(&t.children[0]), ["outer>cell"]);
    assert_eq!(net.instances_of("cell", &all_page()).unwrap().total, 0);
    let nested = net.instances_of("outer>cell", &all_page()).unwrap();
    assert_eq!(nested.total, 1);
    assert_eq!(nested.items[0].copies, Some(1));
    // `t` is called by `outer` (1 copy) and by `spare` (no instances)
    let callers: Vec<(String, Option<u64>)> = net
        .instances_of("t", &all_page())
        .unwrap()
        .items
        .into_iter()
        .map(|r| (r.parent, r.copies))
        .collect();
    assert_eq!(
        callers,
        [("outer".to_owned(), Some(1)), ("spare".to_owned(), Some(0))]
    );
    assert_eq!(row(&net, "t").x_lines, Some(2));
}

#[test]
fn uncalled_nested_definitions_add_no_contexts() {
    // 26 levels of two definitions, each with a nested helper that no line
    // calls, each calling both definitions of the next level: the helpers
    // change no lookup, so the definitions keep one context each
    let levels = 26;
    let mut text = String::from("* t\n");
    for l in 0..levels {
        for x in ["a", "b"] {
            text.push_str(&format!(
                ".subckt {x}{l} p\n.subckt help p\n.ends\nxa p a{n}\nxb p b{n}\n.ends\n",
                n = l + 1
            ));
        }
    }
    text.push_str(&format!(
        ".subckt a{levels} p\nr1 p 0 1\n.ends\n.subckt b{levels} p\nr1 p 0 1\n.ends\nx0 1 a0\n"
    ));
    let (_deck, net) = load_text("uncalled_nested", &text);
    assert!(
        with_code(&net, "hierarchy_too_large").is_empty(),
        "{:#?}",
        all_diags(&net)
    );
    assert_eq!(copies(&net, &format!("a{levels}")), Some(1 << (levels - 1)));
    assert_eq!(copies(&net, "a0>help"), Some(0));
    assert_eq!(expanded(&net, "resistor"), 1 << levels);
}

#[test]
fn protected_region_noted_in_totals() {
    // the elements inside `.prot`/`.unprot` are not parsed, so the totals
    // miss them: the note says so
    let (_deck, net) = load_text(
        "prot",
        "* t\n.subckt cell a\nr1 a 0 1\n.prot\nr2 a 0 2\nr3 a 0 3\n.unprot\n.ends\nx1 1 cell\n",
    );
    assert_eq!(with_code(&net, "protected_region").len(), 1);
    assert_eq!(expanded(&net, "resistor"), 1);
    let note = net.summary().expanded_note.unwrap_or_default();
    assert!(note.contains("protected_region"), "{note}");
}

#[test]
fn unresolved_callee_one_diagnostic_per_x_line() {
    // `t` calls `only`, which exists only nested in `outer`: it resolves
    // under x1 (an `outer`), not under x2 and x3
    let (_deck, net) = load_text(
        "unresolved",
        "* t\n\
         .subckt t a\n\
         xn a only\n\
         rt a 0 1\n\
         .ends\n\
         .subckt outer a\n\
         .subckt only a\n\
         ro a 0 1\n\
         .ends\n\
         xt a t\n\
         .ends\n\
         x1 1 outer\n\
         x2 2 t\n\
         x3 3 t\n\
         xm 4 missing\n",
    );
    // on instance paths from the root: HSPICE rejects the deck (golden
    // `hier_top_nested_only`, `hier_unres_in_subckt`)
    let warns = with_code(&net, "unresolved_subckt");
    assert_eq!(warns.len(), 2, "{warns:#?}");
    assert!(warns.iter().all(|w| w.level == Level::Error));
    let xn = warns.iter().find(|w| w.message.contains("`xn`")).unwrap();
    assert!(xn.message.contains("`only`"), "{}", xn.message);
    assert!(xn.message.contains("2 of the 3"), "{}", xn.message);
    assert!(xn.message.contains("HSPICE rejects"), "{}", xn.message);
    assert_eq!(xn.loc.as_ref().unwrap().line, 3);
    let xm = warns.iter().find(|w| w.message.contains("`xm`")).unwrap();
    assert!(xm.message.contains("`missing`"), "{}", xm.message);
    assert_eq!(copies(&net, "outer>only"), Some(1));
    assert_eq!(copies(&net, "t"), Some(3));
    // the instances with no definition are not counted, and the summary
    // says so
    assert_eq!(expanded(&net, "resistor"), 4);
    let s = net.summary();
    assert_eq!(s.expanded_instances, Some(5));
    assert!(
        s.expanded_note
            .as_deref()
            .is_some_and(|n| n.contains("unresolved_subckt")),
        "{s:?}"
    );
    let x2 = info(&net, "x2.xn");
    assert_eq!(x2.binds_to, None);
    assert!(!x2.caveats.is_empty());
    assert_eq!(
        info(&net, "x1.xt.xn").binds_to.as_deref(),
        Some("outer>only")
    );
    // a path through an instance with no definition
    assert!(net.element_info("x2.xn.ro").is_err());
}

#[test]
fn overflow_note() {
    // l0 calls l1 twice, l1 calls l2 twice, ...: l64 has 2^64 copies
    let mut text = String::from("* t\n");
    for k in 0..64 {
        text.push_str(&format!(
            ".subckt l{k} p\nxa p l{n}\nxb p l{n}\n.ends\n",
            n = k + 1
        ));
    }
    text.push_str(".subckt l64 p\nr1 p 0 1\n.ends\nx0 1 l0\n");
    let (_deck, net) = load_text("overflow", &text);
    assert_eq!(copies(&net, "l63"), Some(1 << 63));
    let r = row(&net, "l64");
    assert_eq!(r.copies, None);
    assert_eq!(r.copies_note.as_deref(), Some("overflow"));
    let s = net.summary();
    assert_eq!(s.expanded_devices, None);
    assert!(
        s.expanded_note
            .as_deref()
            .is_some_and(|n| n.contains("overflow"))
    );
    assert!(with_code(&net, "subckt_cycle").is_empty());
}

#[test]
fn path_with_dotted_instance_name() {
    let net = load(repo("tests/top.sp"));
    let x = info(&net, "x0.aaa");
    assert_eq!(x.path, "x0.aaa");
    assert_eq!(x.kind, "instance");
    assert_eq!(x.def, "(top level)");
    assert_eq!(x.callee.as_deref(), Some("NHVT11LL_CKT"));
    assert_eq!(x.binds_to, None);
    assert!(x.loc.file.ends_with("top.sp"), "{:?}", x.loc);
    assert_eq!(x.loc.line, 3);
    let w: Vec<(&str, &str)> = x
        .params
        .iter()
        .map(|p| (p.name.as_str(), p.text.as_str()))
        .collect();
    assert_eq!(w, [("W", "0.135u"), ("L", "40.00n")]);
    // any letter case
    assert_eq!(info(&net, "X0.AAA").path, "x0.aaa");
    // from an included file
    assert_eq!(info(&net, "x0.bbb").loc.line, 1);
    // no definition of NHVT11LL_CKT: one diagnostic per X line
    assert_eq!(with_code(&net, "unresolved_subckt").len(), 3);
}

#[test]
fn path_first_created_match() {
    let (_deck, net) = load_text(
        "longest",
        "* t\n\
         .subckt cell a\n\
         r2 a 0 1\n\
         r9 a 0 9\n\
         .ends\n\
         .subckt other a\n\
         r5 a 0 5\n\
         .ends\n\
         x1 1 cell\n\
         x1.r2 1 other\n",
    );
    // `x1.r2` names the top-level instance `x1.r2` and the `r2` of `x1`:
    // HSPICE finds an element by splitting its path at the last `.` (golden
    // `elem_dotted_name_collision`), so the device `r2` of `x1` wins, with
    // a caveat
    let x = info(&net, "x1.r2");
    assert_eq!((x.def.as_str(), x.kind.as_str()), ("cell", "resistor"));
    assert!(
        x.caveats.iter().any(|c| c.contains("names 2 elements")),
        "{:?}",
        x.caveats
    );
    // as a scope, the path is the instance
    if let Err(QueryError::BadArgument(msg)) = net.resolve_param("x1.r2", "nosuch") {
        assert!(!msg.contains("not a subckt instance"), "{msg}");
    }
    assert_eq!(info(&net, "x1.r2.r5").def, "other");
    // `x1.r9` names no top-level element: x1, then r9 in `cell`
    assert_eq!(info(&net, "x1.r9").def, "cell");
    // not found: the suggestions are close names in that scope
    match net.element_info("x1.r3") {
        Err(QueryError::NotFound {
            what, suggestions, ..
        }) => {
            assert_eq!(what, "element");
            assert!(suggestions.contains(&"x1.r2".to_owned()), "{suggestions:?}");
        }
        other => panic!("{other:?}"),
    }
    // a path cannot go on below a device
    assert!(matches!(
        net.element_info("x1.r9.foo"),
        Err(QueryError::BadArgument(_))
    ));
    // the root is not an element
    assert!(matches!(
        net.element_info(""),
        Err(QueryError::BadArgument(_))
    ));
}

#[test]
fn element_info_pins_and_params() {
    let (_deck, net) = load_text(
        "einfo",
        "* t\n\
         .model nch nmos level=1\n\
         .subckt inv in out vdd vss wn=1u\n\
         mn out in vss vss nch w=wn l=0.1u\n\
         e1 out 0 in 0 2\n\
         .ends\n\
         xi a y vdd 0 inv wn=2u\n",
    );
    let m = info(&net, "xi.mn");
    assert_eq!(m.kind, "mosfet");
    assert_eq!(m.def, "inv");
    assert_eq!(m.model.as_deref(), Some("nch"));
    let pins: Vec<(&str, &str)> = m
        .pins
        .iter()
        .map(|p| (p.role.as_str(), p.net.as_str()))
        .collect();
    assert_eq!(
        pins,
        [("d", "out"), ("g", "in"), ("s", "vss"), ("b", "vss")]
    );
    let params: Vec<(&str, &str)> = m
        .params
        .iter()
        .map(|p| (p.name.as_str(), p.text.as_str()))
        .collect();
    assert_eq!(params, [("w", "wn"), ("l", "0.1u")]);
    assert!(m.caveats.is_empty(), "{:?}", m.caveats);
    let x = info(&net, "xi");
    let ports: Vec<&str> = x.pins.iter().map(|p| p.role.as_str()).collect();
    assert_eq!(ports, ["in", "out", "vdd", "vss"]);
    assert_eq!(x.binds_to.as_deref(), Some("inv"));
    // an element letter the parser does not read
    let e = info(&net, "xi.e1");
    assert_eq!(e.kind, "e");
    assert!(
        e.caveats.iter().any(|c| c.contains("not parsed")),
        "{:?}",
        e.caveats
    );
    // unparsed letters count by their letter name
    assert_eq!(expanded(&net, "e"), 1);
    assert_eq!(expanded(&net, "mosfet"), 1);
    let v = serde_json::to_value(&m).unwrap();
    assert_eq!(v["pins"][0]["role"], "d");
}

#[test]
fn instances_of_lists_overrides() {
    let (_deck, net) = load_text(
        "instances",
        "* t\n\
         .param w=1u\n\
         .subckt INV a b wn=1u\n\
         r1 a b 1\n\
         .ends\n\
         .subckt buf a b\n\
         Xi a b inv WN='w*2'\n\
         .ends\n\
         X1 a b inv wn=2u M=3\n\
         x2 c d Inv\n\
         x3 c d buf\n",
    );
    let refs = net.instances_of("inv", &all_page()).unwrap();
    assert_eq!(refs.total, 3);
    // `parent | instance | overrides | m`
    let got: Vec<String> = refs
        .items
        .iter()
        .map(|r| {
            let overrides: Vec<String> = r
                .overrides
                .iter()
                .map(|o| format!("{}={}", o.name, o.text))
                .collect();
            format!(
                "{} | {} | {} | {}",
                r.parent,
                r.instance,
                overrides.join(" "),
                r.m.as_deref().unwrap_or("-")
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            "(top level) | X1 | wn=2u | 3",
            "(top level) | x2 |  | -",
            "buf | Xi | wn=w*2 | -",
        ]
    );
    assert_eq!(refs.items[0].loc.line, 9);
    assert_eq!(refs.items[0].copies, Some(1));
    // paged
    let first = net
        .instances_of(
            "INV",
            &Page {
                limit: 2,
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.next_cursor.as_deref(), Some("2"));
    assert_eq!(row(&net, "INV").x_lines, Some(3));
    assert_eq!(copies(&net, "INV"), Some(3));
}

#[test]
fn unknown_name_suggests() {
    let (_deck, net) = load_text(
        "suggest",
        "* t\n\
         .subckt inv a\n.ends\n\
         .subckt nand2 a\n.ends\n\
         .subckt p1 a\n.subckt cell a\n.ends\n.ends\n\
         .subckt p2 a\n.subckt cell a\n.ends\n.ends\n\
         x1 1 inv\n",
    );
    match net.subckt_info("invv", &Page::default()) {
        Err(QueryError::NotFound {
            what,
            name,
            suggestions,
        }) => {
            assert_eq!(what, "subckt");
            assert_eq!(name, "invv");
            assert_eq!(suggestions.first().map(String::as_str), Some("inv"));
        }
        other => panic!("{other:?}"),
    }
    // two nested `cell`: the qualified candidates
    match net.instances_of("cell", &Page::default()) {
        Err(QueryError::NotFound { suggestions, .. }) => {
            assert_eq!(suggestions, ["p1>cell", "p2>cell"]);
        }
        other => panic!("{other:?}"),
    }
    assert!(net.subckt_info("p2>cell", &Page::default()).is_ok());
    assert!(net.subckt_info("INV", &Page::default()).is_ok());
}

#[test]
fn subckt_info_contents() {
    let (_deck, net) = load_text(
        "sinfo",
        "* t\n\
         .model nch nmos level=1\n\
         .model pch pmos level=1\n\
         .subckt leaf a b\n\
         r1 a b 1\n\
         .ends\n\
         .subckt top a b vdd wn=1u wp='wn*2'\n\
         .param k=3\n\
         mn a b 0 0 nch w=wn\n\
         mp a b vdd vdd pch w=wp\n\
         mn2 a b 0 0 nch w=wn\n\
         r1 a b 1k\n\
         x1 a b leaf\n\
         x2 a b LEAF\n\
         x3 a b nosuch\n\
         .ends\n\
         xt 1 2 3 top\n",
    );
    let s = net
        .subckt_info(
            "top",
            &Page {
                limit: 3,
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(s.name, "top");
    assert_eq!(s.ports, ["a", "b", "vdd"]);
    let defaults: Vec<(&str, &str)> = s
        .defaults
        .iter()
        .map(|p| (p.name.as_str(), p.text.as_str()))
        .collect();
    assert_eq!(defaults, [("wn", "1u"), ("wp", "wn*2")]);
    assert_eq!(s.params.len(), 1);
    assert_eq!(
        (s.params[0].name.as_str(), s.params[0].text.as_str()),
        ("k", "3")
    );
    let devs: Vec<(&str, usize, Vec<&str>)> = s
        .devices_by_type
        .iter()
        .map(|d| {
            (
                d.r#type.as_str(),
                d.count,
                d.models.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        devs,
        [("mosfet", 3, vec!["nch", "pch"]), ("resistor", 1, vec![])]
    );
    let kids: Vec<(&str, usize, Vec<&str>)> = s
        .children
        .iter()
        .map(|c| (c.subckt.as_str(), c.x_lines, binds(c)))
        .collect();
    assert_eq!(kids, [("leaf", 2, vec!["leaf"]), ("nosuch", 1, vec![])]);
    assert_eq!(s.children[1].unresolved, Some(true));
    assert_eq!(s.copies, Some(1));
    assert_eq!(s.x_lines, Some(1));
    assert_eq!(s.loc.line, 7);
    // the elements are paged
    assert_eq!(s.elements.total, 7);
    assert_eq!(s.elements.items.len(), 3);
    assert_eq!(s.elements.next_cursor.as_deref(), Some("3"));
    assert_eq!(s.elements.items[0].name, "mn");
    assert_eq!(s.elements.items[0].model.as_deref(), Some("nch"));
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["devices_by_type"][0]["type"], "mosfet");
    assert_eq!(row(&net, "top").devices, 4);
    assert_eq!(row(&net, "top").ports, 3);
}

#[test]
fn list_subckts_sort_filter_page() {
    let (_deck, net) = load_text(
        "list",
        "* t\n\
         .subckt Inv a\nr1 a 0 1\n.ends\n\
         .subckt nand a\nr1 a 0 1\nr2 a 0 1\nr3 a 0 1\n.ends\n\
         .subckt big a\nxi1 a inv\nxi2 a inv\nxn a nand\n.ends\n\
         .subckt unused a\nr1 a 0 1\nr2 a 0 1\n.ends\n\
         x1 1 big\nx2 1 big\n\
         .alter\n\
         .subckt late a\n.ends\n\
         x9 1 late\n",
    );
    let names = |sort: SubcktSort, filter: Option<&str>| -> Vec<String> {
        net.list_subckts(filter, sort, &all_page())
            .unwrap()
            .items
            .into_iter()
            .map(|r| r.name)
            .collect()
    };
    // `.alter` definitions are not listed; names in any letter case
    assert_eq!(
        names(SubcktSort::Name, None),
        ["big", "Inv", "nand", "unused"]
    );
    assert_eq!(
        names(SubcktSort::Copies, None),
        ["Inv", "big", "nand", "unused"]
    );
    assert_eq!(
        names(SubcktSort::Devices, None),
        ["nand", "unused", "Inv", "big"]
    );
    // a substring, or a glob with `*` and `?`
    assert_eq!(
        names(SubcktSort::Name, Some("N")),
        ["Inv", "nand", "unused"]
    );
    assert_eq!(names(SubcktSort::Name, Some("n*")), ["nand"]);
    assert_eq!(names(SubcktSort::Name, Some("?nv")), ["Inv"]);
    let p = net
        .list_subckts(
            None,
            SubcktSort::Name,
            &Page {
                limit: 3,
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(p.total, 4);
    assert_eq!(p.next_cursor.as_deref(), Some("3"));
    assert_eq!(copies(&net, "Inv"), Some(4));
    assert_eq!(copies(&net, "unused"), Some(0));
    assert!(matches!(
        net.list_subckts(
            None,
            SubcktSort::Name,
            &Page {
                limit: 3,
                cursor: Some("x".to_owned())
            }
        ),
        Err(QueryError::BadArgument(_))
    ));
    assert_eq!("copies".parse::<SubcktSort>(), Ok(SubcktSort::Copies));
    assert!("size".parse::<SubcktSort>().is_err());
    // the X line of the `.alter` block is not counted
    assert_eq!(net.summary().expanded_instances, Some(8));
}

#[test]
fn duplicate_subckt_caveats() {
    let net = load(repo("golden/cases/dup_subckt/deck.sp"));
    let all = rows(&net);
    let invs: Vec<&SubcktRow> = all.iter().filter(|r| r.name == "inv").collect();
    assert_eq!(invs.len(), 2, "{all:#?}");
    // the first is used, with a caveat; the second is not used
    assert!(invs[0].copies.is_some());
    assert!(
        invs[0]
            .copies_note
            .as_deref()
            .is_some_and(|n| n.contains("HSPICE rejects")),
        "{:?}",
        invs[0]
    );
    assert_eq!(invs[1].copies, None);
    assert!(
        invs[1]
            .copies_note
            .as_deref()
            .is_some_and(|n| n.contains("duplicate")),
        "{:?}",
        invs[1]
    );
    // the row of the duplicate counts its own devices
    assert_eq!(invs[1].devices, 1);
    let info = net.subckt_info("inv", &Page::default()).unwrap();
    assert!(
        info.caveats.iter().any(|c| c.contains("HSPICE rejects")),
        "{:?}",
        info.caveats
    );
    // the X lines and copies come from the first definition
    let refs = net.instances_of("inv", &Page::default()).unwrap();
    assert_eq!(refs.total, 1);
    assert!(
        refs.items[0]
            .caveats
            .iter()
            .any(|c| c.contains("HSPICE rejects") && c.contains("first definition")),
        "{:?}",
        refs.items[0].caveats
    );
}

#[test]
fn deep_and_wide_hierarchies_are_linear() {
    // a chain of 20k definitions (no recursion in the analysis) and a
    // definition with 20k X lines
    let n = 20_000;
    let mut text = String::from("* t\n");
    for k in 0..n {
        text.push_str(&format!(
            ".subckt c{k} p\nxn p c{}\nr1 p 0 1\n.ends\n",
            k + 1
        ));
    }
    text.push_str(&format!(".subckt c{n} p\nr1 p 0 1\n.ends\n"));
    text.push_str(".subckt wide p\n");
    for k in 0..n {
        text.push_str(&format!("x{k} p c{n}\n"));
    }
    text.push_str(".ends\nx0 1 c0\nxw 1 wide\n");
    let t = std::time::Instant::now();
    let (_deck, net) = load_text("deep_wide", &text);
    let dt = t.elapsed();
    assert_eq!(copies(&net, &format!("c{n}")), Some(n as u64 + 1));
    assert_eq!(expanded(&net, "resistor"), 2 * n as u64 + 1);
    let deep = format!("x0{}.r1", ".xn".repeat(n));
    assert_eq!(info(&net, &deep).def, format!("c{n}"));
    assert!(dt.as_secs() < 20, "{dt:?}");
}

#[test]
fn golden_hier_dotted_names() {
    // instance names with `.`: the top level has `x0.xm` (cell3) and `x0`
    // (wrap), and wrap holds `xm.n` (cell3). HSPICE lists `x0.xm.n`
    // (cell3, x0.xm.n.rc=3): the instance `x0.xm.n` is wrap's `xm.n` under
    // `x0`, as `x0.xm` (cell3) has no `n`
    let net = load(repo("golden/cases/hier_dotted_names/deck.sp"));
    let want = expected("hier_dotted_names");
    let resistors = want["resistors"].as_array().unwrap();
    assert_eq!(resistors.len(), 10);
    for r in resistors {
        let path = r["path"].as_str().unwrap();
        let e = info(&net, path);
        assert_eq!(e.kind, "resistor", "{path}");
        assert_eq!(e.path, path);
        let value = e
            .params
            .iter()
            .find(|p| p.name == "r")
            .map(|p| p.text.parse::<f64>().unwrap());
        assert_eq!(value, r["value"].as_f64(), "{path}");
    }
    let rc = info(&net, "x0.xm.n.rc");
    assert_eq!(rc.def, "cell3");
    let xmn = info(&net, "X0.XM.N");
    assert_eq!(xmn.path, "x0.xm.n");
    assert_eq!(xmn.def, "wrap");
    assert_eq!(xmn.binds_to.as_deref(), Some("cell3"));
    assert_eq!(info(&net, "x0.xm").def, "(top level)");
    assert_eq!(info(&net, "x0.xm.rc").def, "cell3");
    assert_eq!(info(&net, "x0.xq.rc").def, "cell");
    // no element: the error of the first instance with the longest full
    // name on the path
    match net.element_info("x0.xm.n.rq") {
        Err(QueryError::NotFound { suggestions, .. }) => {
            assert_eq!(suggestions, ["x0.xm.n.rc"]);
        }
        other => panic!("{other:?}"),
    }
    assert!(problems(&net).is_empty(), "{:#?}", problems(&net));
}

/// The `r` text of a resistor answer, as a number.
fn r_value(e: &ElementInfo) -> Option<f64> {
    e.params
        .iter()
        .find(|p| p.name == "r")
        .map(|p| p.text.parse::<f64>().unwrap())
}

#[test]
fn golden_hier_dotted_collision() {
    // two instances get one full name (instance names with a `.`). HSPICE
    // creates the instances breadth-first (level by level, then by parent,
    // then by line) and gives the full name to the first one; it lists the
    // later one as circuit `.` (golden/NOTES.md)
    for (case, first_def, shared) in [
        // the top-level `x0.xq` (level 1) before wrap's `xq` under `x0`
        ("hier_dotted_collision", "(top level)", "x0.xq"),
        // `x0.xa` (mid) comes first, but wrap's `xa.xb.xc` (level 2) is
        // created before mid2's `xc` (level 3)
        ("hier_dotted_collision_deep", "wrap", "x0.xa.xb.xc"),
        // level 2: the parent `x0` (wrap) comes before `x0.xa` (mid)
        ("hier_dotted_collision_order", "wrap", "x0.xa.xb"),
        // the same with the parents in the other order
        ("hier_dotted_collision_order_rev", "mid", "x0.xa.xb"),
        // level 2: the parent first, then the `X` line: wrap's second line
        // `xa.xb` comes before mid's first line `xb`
        ("hier_dotted_collision_xline_order", "wrap", "x0.xa.xb"),
    ] {
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let want = expected(case);
        for r in want["resistors"].as_array().unwrap() {
            let path = r["path"].as_str().unwrap();
            let e = info(&net, path);
            assert_eq!(e.path, path, "{case}");
            assert_eq!(e.kind, "resistor", "{case}: {path}");
            assert_eq!(r_value(&e), r["value"].as_f64(), "{case}: {path}");
        }
        // the resistor of the later instance (circuit `.`) has no path:
        // its path names the resistor of the first instance
        let unnamed = want["extra"]["unnamed_circuit_resistors"]
            .as_array()
            .unwrap();
        assert_eq!(unnamed.len(), 1, "{case}");
        let e = info(
            &net,
            &format!("{shared}.{}", unnamed[0]["element"].as_str().unwrap()),
        );
        assert_ne!(r_value(&e), unnamed[0]["value"].as_f64(), "{case}");
        assert!(
            e.caveats
                .iter()
                .any(|c| c.contains("2 instances have the full name")),
            "{case}: {:?}",
            e.caveats
        );
        // the instance path names the first instance
        let x = info(&net, shared);
        assert_eq!(x.def, first_def, "{case}");
        assert!(
            x.caveats.iter().any(|c| c.contains(&format!("`{shared}`"))),
            "{case}: {:?}",
            x.caveats
        );
        assert!(problems(&net).is_empty(), "{case}: {:#?}", problems(&net));
        assert_eq!(
            Some(net.summary().expanded_instances.unwrap()),
            want["totals"]["subckt_instances"].as_u64(),
            "{case}"
        );
    }
    let net = load(repo("golden/cases/hier_dotted_collision_deep/deck.sp"));
    let rl = info(&net, "x0.xa.xb.xc.rl");
    assert_eq!(rl.def, "leafb");
    let xc = info(&net, "x0.xa.xb.xc");
    assert_eq!(
        (xc.def.as_str(), xc.binds_to.as_deref()),
        ("wrap", Some("leafb"))
    );
    assert_eq!(info(&net, "x0.xa.xb").binds_to.as_deref(), Some("mid2"));

    // below the later instance, the instances still get full names from
    // the shared one: `.print i(x0.xa.xb.xk.rk)` finds the `rk` of mid's
    // `xb` (cella) -> `xk` (leafk), `i(x0.xa.xb.rc)` the `rc` of wrap's
    // `xa.xb` (cellb), and `i(x0.xa.xb.xk.rc)` nothing
    let case = "hier_dotted_collision_below";
    let lis = std::fs::read_to_string(repo(&format!("golden/cases/{case}/hspice.lis"))).unwrap();
    assert!(lis.contains("Could not find branch element .rc"));
    let printed: Vec<&str> = lis
        .lines()
        .skip_while(|l| !l.contains("dc transfer curves"))
        .take(6)
        .collect();
    assert!(
        printed.iter().any(|l| l.contains("117.6471m  250.0000m")),
        "{printed:#?}"
    );
    let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
    let rk = info(&net, "x0.xa.xb.xk.rk");
    assert_eq!((rk.def.as_str(), r_value(&rk)), ("leafk", Some(5.0)));
    assert!(
        rk.caveats.iter().any(|c| c.contains("`x0.xa.xb`")),
        "{:?}",
        rk.caveats
    );
    let xk = info(&net, "x0.xa.xb.xk");
    assert_eq!(
        (xk.def.as_str(), xk.binds_to.as_deref()),
        ("cella", Some("leafk"))
    );
    let rc = info(&net, "x0.xa.xb.rc");
    assert_eq!((rc.def.as_str(), r_value(&rc)), ("cellb", Some(3.0)));
    assert!(matches!(
        net.element_info("x0.xa.xb.xk.rc"),
        Err(QueryError::NotFound { .. })
    ));
    assert!(problems(&net).is_empty(), "{:#?}", problems(&net));
    // a device that only the later instance holds has no path
    let (_deck, net) = load_text(
        "dotted_hidden",
        "* t\n.subckt cella a b\nrc a b 2\n.ends\n.subckt cellb a b\nrd a b 3\n.ends\n\
         .subckt mid a b\nxb a b cella\n.ends\n.subckt wrap a b\nxa.xb a b cellb\n.ends\n\
         x0 1 0 wrap\nx0.xa 1 0 mid\n",
    );
    match net.element_info("x0.xa.xb.rc") {
        Err(QueryError::BadArgument(msg)) => {
            assert!(msg.contains("hier_dotted_collision_hidden"), "{msg}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(info(&net, "x0.xa.xb.rd").def, "cellb");
}

/// The `.print dc` columns of the listing of `case`: (header, value), in
/// order, over every printed block.
fn printed(case: &str) -> Vec<(String, String)> {
    let lis = std::fs::read_to_string(repo(&format!("golden/cases/{case}/hspice.lis"))).unwrap();
    let lines: Vec<&str> = lis.lines().collect();
    let mut out = Vec::new();
    for (k, l) in lines.iter().enumerate() {
        if l.trim_start().starts_with("volt ") && k + 2 < lines.len() {
            let heads = lines[k + 1].split_whitespace();
            let values = lines[k + 2].split_whitespace().skip(1);
            out.extend(heads.zip(values).map(|(h, v)| (h.to_owned(), v.to_owned())));
        }
    }
    out
}

/// The printed value of the column `header` of `case`.
fn printed_value(case: &str, header: &str) -> String {
    let p = printed(case);
    p.iter()
        .find(|(h, _)| h == header)
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| panic!("{case}: no printed column {header}: {p:?}"))
}

#[test]
fn golden_elem_dotted_name_collision() {
    // two resistors have the full path `x0.ra.rb`: the device `rb` of the
    // instance `x0.ra` (cell) and the device `ra.rb` of `x0` (wrap). In
    // both line orders, `.print i(x0.ra.rb)` gives 333.3333m, the current
    // of cell's `rb` (2 ohm, with rt1 = 1 ohm): HSPICE splits the path at
    // its last `.`, takes the first circuit with the full name `x0.ra`,
    // then its `rb`
    for case in [
        "elem_dotted_name_collision",
        "elem_dotted_name_collision_rev",
    ] {
        assert_eq!(printed_value(case, "x0.ra.rb"), "333.3333m", "{case}");
        let want = expected(case);
        let shared = want["extra"]["shared_path_resistors"].as_array().unwrap();
        assert_eq!(shared.len(), 2, "{case}");
        let rb = shared.iter().find(|r| r["element"] == "rb").unwrap();
        assert_eq!(rb["definition"], "cell", "{case}");
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        assert!(problems(&net).is_empty(), "{case}: {:#?}", problems(&net));
        let e = info(&net, "x0.ra.rb");
        assert_eq!(
            (e.def.as_str(), e.kind.as_str()),
            ("cell", "resistor"),
            "{case}"
        );
        assert_eq!(r_value(&e), rb["value"].as_f64(), "{case}");
        // the caveat names the rule and the other element
        assert!(
            e.caveats
                .iter()
                .any(|c| c.contains("last `.`") && c.contains("elem_dotted_name_collision")),
            "{case}: {:?}",
            e.caveats
        );
        let r = net.resolve_element("x0.ra.rb").unwrap();
        let v = r.params.iter().find(|p| p.name.eq_ignore_ascii_case("r"));
        assert_eq!(v.and_then(|p| p.value), Some(2.0), "{case}: {r:?}");
        // the instance `x0.ra` is cell's
        assert_eq!(info(&net, "x0.ra").binds_to.as_deref(), Some("cell"));
    }
}

#[test]
fn golden_hier_dotted_collision_hidden() {
    // cellb (wrap's `xa.xb` under `x0`) gets `x0.xa.xb`; cella (mid's `xb`
    // under `x0.xa`) is the later instance with that name and the only one
    // with an `rz`: HSPICE does not find `x0.xa.xb.rz`
    let case = "hier_dotted_collision_hidden";
    let lis = std::fs::read_to_string(repo(&format!("golden/cases/{case}/hspice.lis"))).unwrap();
    assert!(lis.contains("Could not find branch element x0.xa.xb.rz"));
    assert_eq!(printed_value(case, "x0.xa.xb.rc"), "250.0000m");
    let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
    assert!(problems(&net).is_empty(), "{:#?}", problems(&net));
    match net.element_info("x0.xa.xb.rz") {
        Err(QueryError::BadArgument(msg)) => {
            assert!(msg.contains("hier_dotted_collision_hidden"), "{msg}");
            assert!(msg.contains("cella") && msg.contains("cellb"), "{msg}");
        }
        other => panic!("{other:?}"),
    }
    let rc = info(&net, "x0.xa.xb.rc");
    assert_eq!((rc.def.as_str(), r_value(&rc)), ("cellb", Some(3.0)));
}

#[test]
fn golden_hier_dotted_collision_shift() {
    // the later `x0.xa.xb` (cella, circuit 5) is not the last circuit: the
    // directory shifts the names of circuits 5 to 7, but `.print` finds
    // the elements by their own paths
    let case = "hier_dotted_collision_shift";
    let lis = std::fs::read_to_string(repo(&format!("golden/cases/{case}/hspice.lis"))).unwrap();
    assert!(lis.contains("        5 x9.xp.                          cella"));
    assert!(lis.contains("        7 .                               leafk"));
    // `i(x0.xa.xb.xk.rk)` (header `.rk`) is leafk's `rk` (7:rk in the
    // `.op` listing), `i(x9.xp.rp)` (header `x0.xa.xb.xk.rp`) leafp's `rp`
    // (6:rp)
    assert_eq!(printed_value(case, ".rk"), "117.6471m");
    assert_eq!(printed_value(case, "x0.xa.xb.xk.rp"), "115.7895m");
    assert_eq!(printed_value(case, "x0.xa.xb.rc"), "250.0000m");
    let want = expected(case);
    assert_eq!(want["extra"]["collided_circuits"], serde_json::json!([5]));
    let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
    assert!(problems(&net).is_empty(), "{:#?}", problems(&net));
    for (path, def, value) in [
        ("x0.xa.xb.xk.rk", "leafk", 5.0),
        ("x9.xp.rp", "leafp", 7.0),
        ("x0.xa.xb.rc", "cellb", 3.0),
        ("x9.ro", "other", 11.0),
    ] {
        let e = info(&net, path);
        assert_eq!((e.def.as_str(), r_value(&e)), (def, Some(value)), "{path}");
        let listed = want["resistors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["path"] == path)
            .unwrap_or_else(|| panic!("{path} not in expected.json"));
        assert_eq!(listed["value"].as_f64(), Some(value), "{path}");
    }
    // the shifted directory files cella's `rc` under `x9.xp`, which has no
    // `rc`
    assert!(matches!(
        net.element_info("x9.xp.rc"),
        Err(QueryError::NotFound { .. })
    ));
}

#[test]
fn golden_hier_port_forms() {
    // node counts equal to the port counts in other written forms: HSPICE
    // runs the deck. `params:` in a `.subckt` header is a port: `cell a b
    // params: w=1` has 3 ports (with 2 nodes, `hier_port_params_colon` is
    // rejected)
    let case = "hier_port_forms";
    let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
    assert!(problems(&net).is_empty(), "{:#?}", problems(&net));
    assert!(with_code(&net, "port_count_mismatch").is_empty());
    let cell = net.subckt_info("cell", &Page::default()).unwrap();
    assert_eq!(cell.ports.len(), 3, "{:?}", cell.ports);
    assert_eq!(cell.ports[2].to_ascii_lowercase(), "params:");
    assert_eq!(
        net.subckt_info("cell3", &Page::default())
            .unwrap()
            .ports
            .len(),
        3
    );
    assert_eq!(info(&net, "x4").pins.len(), 3);
    assert_eq!(info(&net, "x5.rm").def, "mac");
}

#[test]
fn walk_contexts_are_pairs() {
    // the reviewer chains: `s_i` nests and calls `z`, and calls `s_{i+1}`
    // with `xa` and `s_{i+2}` with `xa.xa`; `p_i` calls `d_{i+1}` with
    // `xa`, `p_{i+1}` with `xa.xb` and `q` with `xq`, and `d_i` nests a `q`
    // and calls `p_i`. The paths have many readings, but the walk answers
    // on every deck that the hierarchy analysis handles
    let expo = |n: usize| {
        let mut t = String::from("* t\n");
        for i in 0..n + 3 {
            t.push_str(&format!(
                ".subckt s{i} p\n.subckt z p\nrz p 0 {}\n.ends\nxz p z\n",
                i + 1
            ));
            if i < n + 2 {
                t.push_str(&format!("xa p s{}\n", i + 1));
            }
            if i < n + 1 {
                t.push_str(&format!("xa.xa p s{}\n", i + 2));
            }
            t.push_str("r1 p 0 1\n.ends\n");
        }
        t + "x0 1 s0\n"
    };
    let pairs = |n: usize| {
        let mut t = String::from("* t\n.subckt q n\nrq n 0 1\n.ends\n");
        for i in 1..n + 2 {
            t.push_str(&format!(".subckt p{i} n\n"));
            if i <= n {
                t.push_str(&format!("xa n d{}\nxa.xb n p{}\n", i + 1, i + 1));
            }
            t.push_str(&format!(
                "xq n q\nr1 n 0 1\n.ends\n.subckt d{i} n\n.subckt q n\nrq n 0 2\n.ends\nrq n 0 1\nxb n p{i}\n.ends\n"
            ));
        }
        t + "xa 1 d1\nxa.xb 1 p1\n"
    };
    let mut answered = 0;
    for n in 12..=18 {
        for (kind, text, path, def) in [
            (
                "expo",
                expo(n),
                format!("x0.{}r1", "xa.".repeat(n)),
                format!("s{n}"),
            ),
            (
                "pairs",
                pairs(n),
                // each `xa.xb` reaches the next `p`: `p{n}`
                format!("{}.r1", vec!["xa.xb"; n].join(".")),
                format!("p{n}"),
            ),
        ] {
            let (_deck, net) = load_text(&format!("walk_{kind}{n}"), &text);
            let too_large = with_code(&net, "hierarchy_too_large");
            if !too_large.is_empty() {
                // HSPICE expands it, but the analysis stopped: the walk may
                // refuse the path, with the reason
                assert!(kind == "pairs" && n > 15, "{kind}{n}");
                match net.element_info(&path) {
                    Ok(e) => assert_eq!(e.def, def),
                    Err(QueryError::BadArgument(msg)) => {
                        assert!(msg.contains("too many readings"), "{msg}")
                    }
                    Err(other) => panic!("{kind}{n}: {other:?}"),
                }
                continue;
            }
            assert_eq!(net.summary().errors, 0, "{kind}{n}");
            let e = info(&net, &path);
            assert_eq!(e.def, def, "{kind}{n}");
            assert_eq!(e.path, path, "{kind}{n}");
            answered += 1;
        }
    }
    // every expo deck and the pairs decks to 15
    assert!(answered >= 11, "{answered}");
}

#[test]
fn walk_is_linear_on_a_deep_scoped_chain() {
    // each `s_i` nests `q` and calls it, then calls `s_{i+1}`: each level
    // is a lookup scope, so the context of each instance on the path is
    // one more than its parent's. The analysis stops (hierarchy_too_large)
    // but the walk still answers, in time and memory linear in the depth
    let chain = |n: usize| {
        let mut t = String::from("* t\n");
        for i in 0..n {
            t.push_str(&format!(
                ".subckt s{i} a\n.subckt q a\nrq a 0 1\n.ends\nxq a q\nx1 a s{}\n.ends\n",
                i + 1
            ));
        }
        t + &format!(".subckt s{n} a\nr1 a 0 1\n.ends\nx0 1 s0\n")
    };
    let n = 12_000;
    let (_deck, net) = load_text("walk_deep_chain", &chain(n));
    let path = format!("x0{}.r1", ".x1".repeat(n));
    let t = std::time::Instant::now();
    let e = info(&net, &path);
    let dt = t.elapsed();
    assert_eq!(e.def, format!("s{n}"));
    // the `q` of the deepest level
    let q = info(&net, &format!("x0{}.xq.rq", ".x1".repeat(n - 1)));
    assert_eq!(q.def, format!("s{}>q", n - 1));
    assert!(dt.as_millis() < 2000, "{dt:?}");
}

#[test]
fn shared_name_counts_skipped_instances() {
    // `x0.xa` (mid) at the top level, and wrap's `xa` (mid) under `x0`:
    // the walk keeps one of these two instances, which have one definition
    // and one context, but both have a `xb` below them, so `x0.xa.xb` is
    // shared by 2 instances too
    let (_deck, net) = load_text(
        "shared_skipped",
        "* t\n.subckt cell a\nrc a 0 1\n.ends\n.subckt mid a\nxb a cell\n.ends\n\
         .subckt wrap a\nxa a mid\n.ends\nx0.xa 1 mid\nx0 1 wrap\n",
    );
    let e = info(&net, "x0.xa.xb.rc");
    let shared: Vec<&String> = e
        .caveats
        .iter()
        .filter(|c| c.contains("instances have the full name"))
        .collect();
    assert_eq!(shared.len(), 1, "{:?}", e.caveats);
    assert!(
        shared[0].contains("2 instances have the full name `x0.xa`")
            && shared[0].contains("1 more full name"),
        "{}",
        shared[0]
    );
    let xb = info(&net, "x0.xa.xb");
    assert!(
        xb.caveats
            .iter()
            .any(|c| c.contains("2 instances have the full name `x0.xa`")),
        "{:?}",
        xb.caveats
    );
}

#[test]
fn path_caveats_are_capped() {
    // every `X` line of a 30-level path has `m=0` and one node too many:
    // one caveat of each kind, with a count
    let n = 30;
    let mut text = String::from("* t\n");
    for i in 0..n {
        text.push_str(&format!(".subckt c{i} a\nx1 a 0 c{} m=0\n.ends\n", i + 1));
    }
    text.push_str(&format!(".subckt c{n} a\nr1 a 0 1\n.ends\nx0 1 0 c0 m=0\n"));
    let (_deck, net) = load_text("capped_caveats", &text);
    let path = format!("x0{}.r1", ".x1".repeat(n));
    for caveats in [
        info(&net, &path).caveats,
        net.resolve_element(&path).unwrap().caveats,
    ] {
        for code in ["port_count_mismatch", "m_not_positive"] {
            let of: Vec<&String> = caveats.iter().filter(|c| c.contains(code)).collect();
            assert_eq!(of.len(), 1, "{code}: {caveats:#?}");
            assert!(of[0].contains(&format!("{} more", n)), "{}", of[0]);
        }
        assert!(caveats.len() < 10, "{caveats:#?}");
    }
}

#[test]
fn element_lookup_uses_a_name_index() {
    // a top level with 100k elements: each query looks up the names of
    // the path, not every element
    let n = 100_000;
    let mut text = String::from("* t\n.subckt cell a\nrc a 0 1\n.ends\n");
    for k in 0..n {
        text.push_str(&format!("r{k} a{k} 0 1\n"));
    }
    text.push_str("x1 1 cell\n");
    let (_deck, net) = load_text("name_index", &text);
    // the first query builds the index of the top level
    assert_eq!(info(&net, "x1.rc").def, "cell");
    let t = std::time::Instant::now();
    for k in 0..2000 {
        assert_eq!(info(&net, &format!("r{}", k * 37 % n)).kind, "resistor");
        assert_eq!(info(&net, "x1.rc").def, "cell");
    }
    // a scan of the 100k elements for each of the 4000 queries takes
    // seconds even in release
    assert!(t.elapsed().as_millis() < 1500, "{:?}", t.elapsed());
}

/// The deck of the first review repro: `s0` .. `s{n+2}` each nest a `z`
/// that they do not call (so each is a lookup scope, as the unused `u`
/// calls `z`) and call the next two with `xa` and `xa.xa`.
fn scoped_chain(n: usize) -> String {
    let mut text = String::from("* t\n.subckt z p\nrz p 0 1\n.ends\n.subckt u p\nxq p z\n.ends\n");
    for i in 0..=n + 2 {
        text.push_str(&format!(".subckt s{i} p\n.subckt z p\nrz p 0 2\n.ends\n"));
        if i < n + 2 {
            text.push_str(&format!("xa p s{}\n", i + 1));
        }
        if i + 2 <= n + 2 {
            text.push_str(&format!("xa.xa p s{}\n", i + 2));
        }
        text.push_str("r1 p 0 1\n.ends\n");
    }
    text + "x0 1 s0\nxu 1 u\n"
}

/// The deck of the second review repro: `p{i}` calls `d{i+1}` with `xa` and
/// `p{i+1}` with `xa.xb`; `d{i}` nests a `q` (a lookup scope, as the unused
/// `spare` calls `q`) and calls `p{i}` with `xb`.
fn scoped_pairs(n: usize) -> String {
    let mut text =
        String::from("* t\n.subckt q n\nrq n 0 1\n.ends\n.subckt spare n\nxs n q\n.ends\n");
    for i in 1..=n + 1 {
        text.push_str(&format!(".subckt p{i} n\n"));
        if i <= n {
            text.push_str(&format!("xa n d{}\nxa.xb n p{}\n", i + 1, i + 1));
        }
        text.push_str(&format!(
            "r1 n 0 1\n.ends\n.subckt d{i} n\n.subckt q n\nrq n 0 2\n.ends\nrq n 0 1\nxb n p{i}\n.ends\n"
        ));
    }
    text + "xa 1 d1\nxa.xb 1 p1\n"
}

#[test]
fn walk_is_polynomial() {
    // instance names with a `.` give a path many readings. The walk keeps
    // one instance for each (place in the path, definition, lookup
    // context), so the readings do not multiply
    let n = 60;
    let (_deck, net) = load_text("walk_scoped_chain", &scoped_chain(n));
    assert!(problems(&net).is_empty(), "{:#?}", problems(&net));
    let typo = format!("x0.{}nosuch", "xa.".repeat(n));
    let t = std::time::Instant::now();
    let got = net.element_info(&typo);
    let dt = t.elapsed();
    assert!(matches!(got, Err(QueryError::NotFound { .. })), "{got:?}");
    assert!(dt.as_millis() < 50, "{dt:?}");
    let r1 = info(&net, &format!("x0.{}r1", "xa.".repeat(n)));
    assert_eq!(r1.def, format!("s{n}"));

    let (_deck, net) = load_text("walk_scoped_pairs", &scoped_pairs(n));
    assert!(problems(&net).is_empty(), "{:#?}", problems(&net));
    let typo = format!("{}.zz", ["xa.xb"; 60].join("."));
    let t = std::time::Instant::now();
    let got = net.element_info(&typo);
    let dt = t.elapsed();
    assert!(matches!(got, Err(QueryError::NotFound { .. })), "{got:?}");
    assert!(dt.as_millis() < 50, "{dt:?}");
    let r1 = info(&net, &format!("{}.r1", ["xa.xb"; 60].join(".")));
    assert_eq!(r1.def, "p60");

    // each level has `xa` and `xa.xa`, both calling `s`: a path of 60 `xa`
    // has Fibonacci(60) readings. HSPICE rejects the deck (a call cycle),
    // but the walk still answers
    let (_deck, net) = load_text(
        "dotted_states",
        "* t\n.subckt s p\nxa p s\nxa.xa p s\nr1 p 0 1\n.ends\nxa 1 s\nxa.xa 1 s\n",
    );
    let path = format!("{}.zz", ["xa"; 60].join("."));
    let t = std::time::Instant::now();
    let got = net.element_info(&path);
    let dt = t.elapsed();
    assert!(matches!(got, Err(QueryError::NotFound { .. })), "{got:?}");
    assert!(dt.as_millis() < 50, "{dt:?}");
    for n in [1999, 2000] {
        let path = format!("{}.r1", ["xa"; 2000][..n].join("."));
        let r1 = info(&net, &path);
        assert_eq!(r1.def, "s");
        assert_eq!(r1.path, path);
        // every level is shared by two instances: one caveat
        let shared: Vec<_> = r1
            .caveats
            .iter()
            .filter(|c| c.contains("instances have the full name"))
            .collect();
        assert_eq!(shared.len(), 1, "{:?}", r1.caveats);
    }
    // a dead end at every level (`xa.xa` calls an undefined name): the
    // error is built once, at the end
    let (_deck, net) = load_text(
        "dotted_dead_ends",
        "* t\n.subckt s p\nxa p s\nxa.xa p dead\nr1 p 0 1\n.ends\nxa 1 s\n",
    );
    let path = format!("{}.r1", ["xa"; 20_000].join("."));
    let t = std::time::Instant::now();
    assert_eq!(info(&net, &path).def, "s");
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
}

#[test]
fn golden_duplicate_element() {
    // two element or `X` lines with one name in one definition on an
    // instance path: HSPICE rejects the deck ("attempts to redefine")
    for (case, lines) in [
        ("hier_dup_instance", &[17, 11][..]),
        ("hier_dup_instance_sub", &[11][..]),
        ("elem_dup_name", &[4][..]),
        ("elem_dup_name_sub", &[4][..]),
    ] {
        let want = expected(case);
        let rejected = want["extra"]["rejected"].as_str().unwrap();
        assert!(
            rejected.contains("attempts to redefine"),
            "{case}: {rejected}"
        );
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let ds = with_code(&net, "duplicate_element");
        let got: Vec<u32> = ds.iter().map(|d| d.loc.as_ref().unwrap().line).collect();
        assert_eq!(got, lines, "{case}: {ds:#?}");
        assert!(ds.iter().all(|d| d.level == Level::Error), "{case}");
        assert!(
            ds.iter().all(|d| d.message.contains("HSPICE rejects")),
            "{case}: {ds:#?}"
        );
        let s = net.summary();
        assert_eq!(s.errors, lines.len(), "{case}: {:#?}", all_diags(&net));
        assert!(
            s.expanded_note
                .as_deref()
                .is_some_and(|n| n.contains("duplicate_element")),
            "{case}: {s:?}"
        );
    }
    // the first line is used, with a caveat
    let net = load(repo("golden/cases/hier_dup_instance/deck.sp"));
    let x1 = info(&net, "x1");
    assert_eq!(x1.binds_to.as_deref(), Some("wrap"));
    assert!(
        x1.caveats.iter().any(|c| c.contains("duplicate_element")),
        "{x1:?}"
    );
    let rc = info(&net, "x1.xq.rc");
    assert_eq!(rc.def, "cell");
    // `x1` and `xq` both name two lines: one caveat, with the count
    // (plan Task 7a: one caveat for each kind)
    let dup_caveats: Vec<&String> = rc
        .caveats
        .iter()
        .filter(|c| c.contains("duplicate_element"))
        .collect();
    assert_eq!(dup_caveats.len(), 1, "{:?}", rc.caveats);
    assert!(dup_caveats[0].contains("1 more name"), "{}", dup_caveats[0]);
    assert!(info(&net, "rt1").caveats.is_empty());
    // short messages (one per line of a file included twice)
    let ds = with_code(&net, "duplicate_element");
    assert!(ds[0].message.contains("line 15"), "{}", ds[0].message);
    assert!(ds.iter().all(|d| d.message.len() <= 100), "{ds:#?}");
    // `instances_of` and `subckt_info` say so too: `wrap` has two `xq`
    // (the second calls `cell3`), and the top level two `x1`
    let lines = net.instances_of("cell3", &all_page()).unwrap().items;
    let at: Vec<(&str, &str, u32)> = lines
        .iter()
        .map(|l| (l.parent.as_str(), l.instance.as_str(), l.loc.line))
        .collect();
    assert_eq!(at, [("(top level)", "x1", 17), ("wrap", "xq", 11)]);
    assert!(
        lines
            .iter()
            .all(|l| l.caveats.iter().any(|c| c.contains("duplicate_element"))),
        "{lines:#?}"
    );
    let wrap = net.subckt_info("wrap", &Page::default()).unwrap();
    assert!(
        wrap.caveats.iter().any(|c| c.contains("duplicate_element")),
        "{:?}",
        wrap.caveats
    );
    let cell = net.subckt_info("cell", &Page::default()).unwrap();
    assert!(cell.caveats.is_empty(), "{:?}", cell.caveats);
}

#[test]
fn golden_duplicate_element_unused() {
    // the same lines in a subckt with no instances: HSPICE runs the deck,
    // so the loader gives Warnings only
    for case in ["hier_dup_instance_unused", "elem_dup_name_unused"] {
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let s = net.summary();
        assert_eq!(s.errors, 0, "{case}: {:#?}", all_diags(&net));
        let ds = with_code(&net, "duplicate_element");
        let got: Vec<(Level, u32)> = ds
            .iter()
            .map(|d| (d.level, d.loc.as_ref().unwrap().line))
            .collect();
        assert_eq!(got, [(Level::Warning, 7)], "{case}");
        assert!(
            ds[0]
                .message
                .contains("no instance path reaches it, so HSPICE runs the deck"),
            "{}",
            ds[0].message
        );
        assert!(ds[0].message.len() <= 100, "{}", ds[0].message);
        assert_eq!(s.expanded_note, None, "{case}");
        let want = expected(case);
        assert_eq!(
            expanded(&net, "resistor"),
            want["totals"]["resistors"].as_u64().unwrap()
        );
    }
}

#[test]
fn verilog_a_callee_is_a_warning() {
    // not probed (hspice.exe runs no Verilog-A, golden/NOTES.md "Choices
    // not probed"): with a `.hdl` line in the first run, or a visible
    // `.model` card of the callee's name whose type is not built in, an
    // `X` line with no subckt definition can call a Verilog-A module: a
    // Warning, not the Error of `hier_top_nested_only`
    for (name, text) in [
        ("hdl", "* t\n.hdl \"res.va\"\nv1 1 0 1\nx1 1 0 myres r=1k\n"),
        (
            "va_model",
            "* t\n.model vres myres\nv1 1 0 1\nx2 1 0 vres\n",
        ),
        (
            "va_model_outer",
            "* t\n.model vres myres\n.subckt w a\nx3 a 0 vres\n.ends\nv1 1 0 1\nxw 1 w\n",
        ),
    ] {
        let (_deck, net) = load_text(name, text);
        let u = with_code(&net, "unresolved_subckt");
        assert_eq!(u.len(), 1, "{name}: {u:#?}");
        assert_eq!(u[0].level, Level::Warning, "{name}");
        assert!(u[0].message.contains("Verilog-A"), "{}", u[0].message);
        // short: a deck loaded without its cell library has one per line
        assert!(u[0].message.len() <= 100, "{}", u[0].message);
        let s = net.summary();
        assert_eq!(s.errors, 0, "{name}: {:#?}", all_diags(&net));
        assert!(
            s.expanded_note
                .as_deref()
                .is_some_and(|n| n.contains("Verilog-A") && n.contains("neither")),
            "{name}: {s:?}"
        );
    }
    // an Error: no `.hdl` and no model card; a `.hdl` line in an `.alter`
    // block only; a model card of a built-in type (`hier_model_callee`);
    // a model card that is not visible from the `X` line (in another
    // subckt)
    for (name, text) in [
        ("no_va", "* t\nv1 1 0 1\nx2 1 0 vres\n.op\n"),
        (
            "hdl_in_alter",
            "* t\nv1 1 0 1\nx2 1 0 vres\n.op\n.alter\n.hdl \"res.va\"\n",
        ),
        (
            "builtin_model",
            "* t\n.model vres nmos level=1\nv1 1 0 1\nx2 1 0 vres\n",
        ),
        (
            "model_elsewhere",
            "* t\n.subckt other a\n.model vres myres\nr1 a 0 1\n.ends\nv1 1 0 1\nx2 1 0 vres\nxo 1 other\n",
        ),
        // a visible model card of a built-in type is never a Verilog-A
        // module, also with a `.hdl` line (`hier_model_callee`)
        (
            "hdl_builtin_model",
            "* t\n.hdl \"res.va\"\n.model rmod r\nv1 1 0 1\nx1 1 0 rmod\n",
        ),
        (
            "hdl_builtin_model_outer",
            "* t\n.hdl \"res.va\"\n.model rmod r\n.subckt w a\nx3 a 0 rmod\n.ends\nv1 1 0 1\nxw 1 w\n",
        ),
        // `s` (S-parameter) is a built-in type
        ("s_model", "* t\n.model smod s\nv1 1 0 1\nx1 1 0 smod\n"),
    ] {
        let (_deck, net) = load_text(name, text);
        let u = with_code(&net, "unresolved_subckt");
        assert_eq!(u.len(), 1, "{name}: {u:#?}");
        assert_eq!(u[0].level, Level::Error, "{name}: {}", u[0].message);
    }
}

#[test]
fn golden_hier_model_callee() {
    // an `X` line that calls the name of an R model card, with no `.hdl`
    // line: HSPICE rejects the deck ("Definition of model/subckt "rmod" is
    // not found for the element "x1"")
    let want = expected("hier_model_callee");
    let rejected = want["extra"]["rejected"].as_str().unwrap();
    assert!(rejected.contains("(deck.sp:4)"), "{rejected}");
    let net = load(repo("golden/cases/hier_model_callee/deck.sp"));
    let u = with_code(&net, "unresolved_subckt");
    let got: Vec<(Level, u32)> = u
        .iter()
        .map(|d| (d.level, d.loc.as_ref().unwrap().line))
        .collect();
    assert_eq!(got, [(Level::Error, 4)], "{u:#?}");
    assert_eq!(net.summary().errors, 1, "{:#?}", all_diags(&net));
}

#[test]
fn golden_port_count_mismatch() {
    // an `X` line whose node count is not the port count of its subckt:
    // HSPICE rejects the deck on an instance path ("Number of nodes
    // mismatch between instance "x1" and subcircuit "cell"")
    for (case, line, path) in [
        ("hier_port_more", 6, "x1.rc"),
        ("hier_port_fewer", 7, "x1.xc.rc"),
        // one `X` line, two bindings (dynamic lookup): only `outer2>only`
        // (3 ports) under `x2` mismatches
        ("hier_port_dynamic", 3, "x2.xt.xn.ro"),
        // `params:` is a port: `cell a b params: w=1` has 3
        ("hier_port_params_colon", 17, "x1.rc"),
    ] {
        let want = expected(case);
        let rejected = want["extra"]["rejected"].as_str().unwrap();
        assert!(
            rejected.contains(&format!("(deck.sp:{line}) Number of nodes mismatch")),
            "{case}: {rejected}"
        );
        let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
        let ds = with_code(&net, "port_count_mismatch");
        let got: Vec<(Level, u32)> = ds
            .iter()
            .map(|d| (d.level, d.loc.as_ref().unwrap().line))
            .collect();
        assert_eq!(got, [(Level::Error, line)], "{case}: {ds:#?}");
        assert!(
            ds[0].message.contains("HSPICE rejects"),
            "{}",
            ds[0].message
        );
        if case == "hier_port_dynamic" {
            assert!(ds[0].message.contains("`outer2>only`"), "{}", ds[0].message);
            assert!(
                info(&net, "x1.xt.xn.ro")
                    .caveats
                    .iter()
                    .all(|c| !c.contains("port_count_mismatch")),
                "x1's binding has 2 ports"
            );
        }
        let s = net.summary();
        assert_eq!(s.errors, 1, "{case}: {:#?}", all_diags(&net));
        assert!(
            s.expanded_note
                .as_deref()
                .is_some_and(|n| n.contains("port_count_mismatch")),
            "{case}: {s:?}"
        );
        // the paths below the line say so
        let e = info(&net, path);
        assert!(
            e.caveats.iter().any(|c| c.contains("port_count_mismatch")),
            "{case}: {:?}",
            e.caveats
        );
    }
    // in a subckt that no instance path reaches, HSPICE runs the deck with
    // no warning (`hier_port_unused`): a Warning for each line
    let case = "hier_port_unused";
    let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
    let ds = with_code(&net, "port_count_mismatch");
    let got: Vec<(Level, u32)> = ds
        .iter()
        .map(|d| (d.level, d.loc.as_ref().unwrap().line))
        .collect();
    assert_eq!(got, [(Level::Warning, 6), (Level::Warning, 7)], "{ds:#?}");
    let s = net.summary();
    assert_eq!(
        (s.errors, s.expanded_note),
        (0, None),
        "{:#?}",
        all_diags(&net)
    );
    assert_eq!(
        expanded(&net, "resistor"),
        expected(case)["totals"]["resistors"].as_u64().unwrap()
    );
}

#[test]
fn answer_lists_are_capped() {
    // every list in an answer holds at most MAX_LIMIT items, with its total
    let n = 600;
    let names: Vec<String> = (0..n).map(|k| format!("p{k}")).collect();
    let kv = |prefix: &str| -> String {
        names
            .iter()
            .map(|p| format!("{prefix}{p}=1"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut text = format!("* t\n.subckt big a {}\n", kv(""));
    for k in 0..n {
        text.push_str(&format!(".param q{k}=1\n.param f{k}(x)='x+{k}'\n"));
    }
    text.push_str(&format!("r1 a 0 1 {}\n.ends\n", kv("e")));
    text.push_str(&format!("x1 1 big {}\n", kv("")));
    let (_deck, net) = load_text("capped", &text);
    let big = net.subckt_info("big", &Page::default()).unwrap();
    assert_eq!((big.defaults.len(), big.defaults_total), (500, n));
    assert_eq!((big.params.len(), big.params_total), (500, n));
    assert_eq!((big.funcs.len(), big.funcs_total), (500, n));
    let x = net.instances_of("big", &Page::default()).unwrap().items;
    assert_eq!((x[0].overrides.len(), x[0].overrides_total), (500, n));
    let r1 = info(&net, "x1.r1");
    assert_eq!((r1.params.len(), r1.params_total), (500, n + 1));
}

/// The lines, the first line number and the caveats of the whole text of
/// subckt `name`.
fn text_of(net: &Netlist, name: &str) -> (u32, Vec<String>, Vec<String>) {
    let t = net.subckt_text(name, &all_page()).unwrap();
    assert_eq!(t.lines.total, t.lines.items.len());
    assert_eq!(t.first_line, t.loc.line);
    (t.first_line, t.lines.items, t.caveats)
}

#[test]
fn subckt_text_is_the_text_as_written() {
    let (_deck, net) = load_text(
        "subckt_text_written",
        "* title\r\n.param p=1\r\n* a comment before\r\n.SUBCKT Inv a b wn=1u   $ header\r\n+ wp=2u\r\nr1 a b 1k\r\n* body comment\r\n.ENDS Inv $ end comment\r\n\r\nx1 1 0 inv\r\n.end\r\n",
    );
    let t = net.subckt_text("INV", &all_page()).unwrap();
    assert_eq!(t.name, net.subckt_info("inv", &all_page()).unwrap().name);
    assert!(t.loc.file.ends_with("main.sp"), "{}", t.loc.file);
    let (line, lines, caveats) = text_of(&net, "inv");
    assert_eq!(line, 4);
    assert_eq!(
        lines,
        [
            ".SUBCKT Inv a b wn=1u   $ header",
            "+ wp=2u",
            "r1 a b 1k",
            "* body comment",
            ".ENDS Inv $ end comment",
        ]
    );
    assert!(caveats.is_empty(), "{caveats:?}");
}

#[test]
fn subckt_text_nested_and_paged() {
    let net = load(repo("golden/cases/nested_def/deck.sp"));
    let (line, lines, _) = text_of(&net, "outer>cell");
    assert_eq!(line, 6);
    assert_eq!(lines, [".subckt cell a b", "rc a b 2", ".ends"]);
    let (line, lines, _) = text_of(&net, "outer");
    assert_eq!(line, 5);
    assert_eq!(lines.len(), 10);
    assert_eq!(lines[0], ".subckt outer a b");
    assert_eq!(lines[9], ".ends");
    // the nested definitions are part of the text
    assert_eq!(lines[1], ".subckt cell a b");
    let page = |cursor: Option<&str>| Page {
        limit: 4,
        cursor: cursor.map(str::to_owned),
    };
    let p1 = net.subckt_text("outer", &page(None)).unwrap();
    assert_eq!((p1.first_line, p1.lines.items.len()), (5, 4));
    assert_eq!(p1.lines.next_cursor.as_deref(), Some("4"));
    let p3 = net.subckt_text("outer", &page(Some("8"))).unwrap();
    assert_eq!(p3.first_line, 13);
    assert_eq!(p3.lines.items, ["xd a b deep", ".ends"]);
    assert_eq!(p3.lines.next_cursor, None);
    assert_eq!(p3.loc.line, 5);
}

#[test]
fn subckt_text_macro_eom() {
    let net = load(repo("golden/cases/macro_eom/deck.sp"));
    let (line, lines, caveats) = text_of(&net, "inv");
    assert_eq!(line, 2);
    assert_eq!(lines, [".macro inv a b rv=3", "r1 a b rv", ".eom"]);
    assert!(caveats.is_empty());
}

#[test]
fn subckt_text_without_ends() {
    let (_deck, net) = load_text(
        "subckt_text_no_ends",
        "* title\n.subckt open a b\nr1 a b 1\n* trailing comment\n\n   \n.end\nafter the end\n",
    );
    let (line, lines, caveats) = text_of(&net, "open");
    assert_eq!(line, 2);
    assert_eq!(
        lines,
        [".subckt open a b", "r1 a b 1", "* trailing comment"]
    );
    assert!(
        caveats.iter().any(|c| c.contains("missing_ends")),
        "{caveats:?}"
    );
    // no `.end` either: the text goes to the end of the file
    let (_deck, net) = load_text("subckt_text_eof", "* title\n.subckt open a b\nr1 a b 1\n\n");
    let (_, lines, caveats) = text_of(&net, "open");
    assert_eq!(lines, [".subckt open a b", "r1 a b 1"]);
    assert_eq!(caveats.len(), 1);
}

#[test]
fn subckt_text_include_lines() {
    let net = load(repo("golden/cases/include_in_subckt/deck.sp"));
    let (_, lines, caveats) = text_of(&net, "holder");
    assert_eq!(
        lines,
        [
            ".subckt holder a b pv=3",
            ".include 'inc.sp'",
            "r0 a b 1",
            ".ends"
        ]
    );
    assert_eq!(caveats.len(), 1);
    assert!(
        caveats[0].contains("1 `.include` or `.lib` line:"),
        "{caveats:?}"
    );
}

#[test]
fn subckt_text_in_a_lib_section() {
    let deck = write_deck(
        "subckt_text_lib",
        &[
            ("main.sp", "* title\n.lib 'lib.sp' tt\nx1 1 0 cell\n.end\n"),
            (
                "lib.sp",
                "* cells\n.lib ff\n.subckt cell a b\nr1 a b 2\n.ends\n.endl ff\n.lib tt\n.subckt cell a b\nr1 a b 1\n.ends cell\n.endl tt\n",
            ),
        ],
    );
    let net = load(&deck.main);
    let t = net.subckt_text("cell", &all_page()).unwrap();
    assert!(t.loc.file.ends_with("lib.sp"), "{}", t.loc.file);
    assert_eq!(t.first_line, 8);
    assert_eq!(
        t.lines.items,
        [".subckt cell a b", "r1 a b 1", ".ends cell"]
    );
}

#[test]
fn subckt_text_not_found() {
    let net = load(repo("golden/cases/hier_counts/deck.sp"));
    match net.subckt_text("lef", &all_page()) {
        Err(QueryError::NotFound { suggestions, .. }) => {
            assert!(suggestions.iter().any(|s| s == "leaf"), "{suggestions:?}")
        }
        other => panic!("{other:?}"),
    }
    let bad = Page {
        limit: 5,
        cursor: Some("x".to_owned()),
    };
    assert!(matches!(
        net.subckt_text("leaf", &bad),
        Err(QueryError::BadArgument(_))
    ));
}
