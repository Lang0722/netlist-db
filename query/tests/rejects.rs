//! Plan Task 13a: each golden deck that HSPICE rejects gives a load Error,
//! except the cases that `golden/NOTES.md` lists under "Known
//! differences", and the load checks of forms that HSPICE rejects give no
//! Error on a deck that HSPICE runs.

use netlist_query::{Diagnostic, Level, LoadOptions, Netlist, Page};
use std::path::{Path, PathBuf};

/// The codes of the load checks of `query/src/rejects.rs`.
const CODES: [&str; 5] = [
    "undefined_param",
    "override_self",
    "statistical_in_element",
    "name_then_model",
    "inductor_model_level",
];

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

fn errors_of(net: &Netlist) -> Vec<Diagnostic> {
    let page = Page {
        limit: 500,
        cursor: None,
    };
    net.diagnostics(Some(Level::Error), &page).unwrap().items
}

fn errors(deck: &Path) -> Vec<Diagnostic> {
    errors_of(&Netlist::load(&LoadOptions::new(deck.to_path_buf())).unwrap())
}

/// Each golden case: its name, its deck, and whether HSPICE rejects it.
fn cases() -> Vec<(String, PathBuf, bool)> {
    let mut out: Vec<_> = std::fs::read_dir(repo("golden/cases"))
        .unwrap()
        .map(|d| {
            let dir = d.unwrap().path();
            let want: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(dir.join("expected.json")).unwrap())
                    .unwrap();
            let name = dir.file_name().unwrap().to_str().unwrap().to_owned();
            (
                name,
                dir.join("deck.sp"),
                want["extra"].get("rejected").is_some(),
            )
        })
        .collect();
    out.sort();
    out
}

/// The case names that the bullets of the NOTES section "Known
/// differences" start with (before the first `:`).
fn known_differences() -> Vec<String> {
    let notes = std::fs::read_to_string(repo("golden/NOTES.md")).unwrap();
    let start = notes.find("\n## Known differences\n").expect("the section") + 1;
    let body = &notes[start..];
    let body = &body[..body[3..].find("\n## ").map_or(body.len(), |e| e + 3)];
    let mut out: Vec<String> = body
        .lines()
        .filter_map(|l| l.strip_prefix("- "))
        .filter_map(|l| l.split_once(": ").map(|(head, _)| head))
        .flat_map(|head| head.split('`').skip(1).step_by(2))
        .map(str::to_owned)
        .collect();
    out.sort();
    out
}

#[test]
fn rejected_decks_have_errors() {
    let cases = cases();
    let known = known_differences();
    assert_eq!(
        known,
        [
            "func_body_undef_global",
            "func_shadow_more",
            "func_shadow_more_local",
            "m_expr_zero",
            "pulse_r_noparen"
        ]
    );
    let mut silent = Vec::new();
    for (name, deck, rejected) in &cases {
        let quiet = errors(deck).is_empty();
        if known.contains(name) {
            // the section lists rejected decks with no Error only
            assert!(
                *rejected && quiet,
                "{name}: leave NOTES \"Known differences\""
            );
        } else if *rejected && quiet {
            silent.push(name.as_str());
        }
    }
    assert!(
        silent.is_empty(),
        "HSPICE rejects these decks, and they load with no Error (and are not in NOTES \"Known differences\"): {silent:?}"
    );
}

#[test]
fn load_checks_name_their_form() {
    for (case, code, line) in [
        ("unquoted_space", "undefined_param", 10),
        ("pe_scope_global", "undefined_param", 3),
        ("param_undef_unused", "undefined_param", 2),
        ("res_name_model", "name_then_model", 4),
        ("ind_model_nolevel", "inductor_model_level", 5),
        ("stat_direct", "statistical_in_element", 3),
        ("ovr_self_global", "override_self", 5),
        ("stat_in_mos", "statistical_in_element", 3),
        ("ind_level_twice", "inductor_model_level", 4),
        ("func_zero_args", "func_no_args", 2),
    ] {
        let errs = errors(&repo(&format!("golden/cases/{case}/deck.sp")));
        assert_eq!(errs.len(), 1, "{case}: {errs:#?}");
        let e = &errs[0];
        assert_eq!(
            (e.code, e.loc.as_ref().map(|l| l.line)),
            (code, Some(line)),
            "{case}"
        );
        assert!(e.message.contains(case), "{case}: {}", e.message);
    }
    // the summary note of the expanded totals says so
    let net = Netlist::load(&LoadOptions::new(repo("golden/cases/stat_direct/deck.sp"))).unwrap();
    let note = net.summary().expanded_note.unwrap();
    assert!(
        note.contains("1 line has a form that HSPICE rejects"),
        "{note}"
    );
}

#[test]
fn load_checks_quiet_on_accepted_decks() {
    for (name, deck, rejected) in cases() {
        if rejected {
            continue;
        }
        let new: Vec<_> = errors(&deck)
            .into_iter()
            .filter(|d| CODES.contains(&d.code))
            .collect();
        assert!(new.is_empty(), "{name}: {new:#?}");
    }
}

fn load_text(name: &str, text: &str) -> Netlist {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("rejects")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.sp");
    std::fs::write(&path, text).unwrap();
    Netlist::load(&LoadOptions::new(path)).unwrap()
}

fn codes(net: &Netlist) -> Vec<&'static str> {
    errors_of(net).into_iter().map(|d| d.code).collect()
}

#[test]
fn load_checks_scope() {
    // only on instance paths: an unused subckt gets nothing
    let body = "rs a b 'agauss(1k,0.1k,3)'\nru a b nosuch\nl1 a b 1n lmod\n";
    let deck = |x: &str| {
        format!(
            "* t\n.model lmod l\n.subckt s a b\n{body}.ends\n{x}v1 1 0 1\nr0 1 0 1k\n.op\n.end\n"
        )
    };
    assert_eq!(codes(&load_text("unused", &deck(""))), Vec::<&str>::new());
    let mut used = codes(&load_text("used", &deck("x1 1 0 s\n")));
    used.sort_unstable();
    assert_eq!(
        used,
        [
            "inductor_model_level",
            "statistical_in_element",
            "undefined_param"
        ]
    );
    // names that HSPICE knows, a name that an X line or a subckt defines,
    // an inductor card with level=3 or a level expression
    let net = load_text(
        "fine",
        "* t\n.param lv=3 a='temper*2'\n.model l3 l level=3\n.model lx l level=lv\n.subckt s a b w=1\nrw a b w\nrq a b q\n.ends\nx1 1 0 s q=2\nrt 1 0 'hertz+time+1'\nla 1 0 1n l3\nlb 1 0 1n lx\nv1 1 0 1\n.op\n.end\n",
    );
    assert_eq!(codes(&net), Vec::<&str>::new());
    // a self-naming override in a subckt: no load check (NOTES "Known
    // differences"); at the top level with a top-level `.param`: fine
    let net = load_text(
        "ovr",
        "* t\n.param w=1\n.subckt c a b w=2\nr1 a b w\n.ends\n.subckt p a b\nxi a b c w='w*3'\n.ends\nx1 1 0 c w='w*3'\nx2 1 0 p\nv1 1 0 1\n.op\n.end\n",
    );
    assert_eq!(codes(&net), Vec::<&str>::new());
    // the card of an L line can be the word after a name in the value
    // place (`ind_name_model`)
    let net = load_text(
        "l_name_model",
        "* t\n.param lval=1n\n.model lmod l level=1\nl1 1 0 lval lmod\nv1 1 0 1\n.op\n.end\n",
    );
    assert_eq!(codes(&net), ["inductor_model_level"]);
    // after a load Error the checks of undefined names are off
    let net = load_text(
        "after_error",
        "* t\n.param p='{a}*2'\n.param q='z*2'\nr1 1 0 p\nv1 1 0 1\n.op\n.end\n",
    );
    let c = codes(&net);
    assert!(!c.is_empty() && !c.contains(&"undefined_param"), "{c:?}");
}

/// The reasons of parameter resolution for the forms of the review probes
/// that HSPICE runs: none says that HSPICE rejects the deck.
#[test]
fn resolver_reasons_of_accepted_forms() {
    let reason = |case: &str, path: &str| {
        let net = Netlist::load(&LoadOptions::new(repo(&format!(
            "golden/cases/{case}/deck.sp"
        ))))
        .unwrap();
        let e = net.resolve_element(path).unwrap();
        let p = e
            .params
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case("r"))
            .unwrap();
        (p.value, p.unresolved.clone().unwrap_or_default())
    };
    let (v, why) = reason("rcl_output_var", "r1");
    assert!(
        v.is_none() && why.contains("no `.param` function `v`"),
        "{why}"
    );
    let (v, why) = reason("prot_param", "r1");
    assert!(v.is_none() && why.contains("`.prot` region"), "{why}");
    assert!(!why.contains("rejects"), "{why}");
    assert_eq!(reason("stat_user_func", "r1").0, Some(2000.0));
}
