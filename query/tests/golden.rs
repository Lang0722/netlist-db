//! Parameter resolution, checked against the HSPICE golden
//! corpus (`golden/cases`) and pinned on small decks.

use netlist_query::{
    ElementParams, Level, LoadOptions, Netlist, Page, QueryError, Resolved, StepKind,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// Every case directory of the golden corpus, by name.
fn cases() -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(repo("golden/cases"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), p))
        .collect();
    out.sort();
    out
}

fn expected(dir: &Path) -> Value {
    let text = std::fs::read_to_string(dir.join("expected.json"))
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    serde_json::from_str(&text).unwrap()
}

fn load(path: impl AsRef<Path>) -> Netlist {
    Netlist::load(&LoadOptions::new(path.as_ref().to_path_buf()))
        .unwrap_or_else(|e| panic!("{}: {e}", path.as_ref().display()))
}

fn golden(case: &str) -> Netlist {
    load(repo(&format!("golden/cases/{case}/deck.sp")))
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
        .join("golden")
        .join(format!("{name}-{}", std::process::id()));
    _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        std::fs::write(dir.join(file), text).unwrap();
    }
    let main = dir.join(files[0].0);
    Deck { dir, main }
}

fn load_text(name: &str, text: &str) -> (Deck, Netlist) {
    let deck = write_deck(name, &[("main.sp", text)]);
    let net = load(&deck.main);
    (deck, net)
}

/// The parameter `name` (any letter case) of `ep`.
fn param<'a>(ep: &'a ElementParams, name: &str) -> &'a Resolved {
    ep.params
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| panic!("{}: no parameter `{name}` in {ep:?}", ep.path))
}

fn has_param(ep: &ElementParams, name: &str) -> bool {
    ep.params.iter().any(|p| p.name.eq_ignore_ascii_case(name))
}

/// The value of parameter `name` of the element at `path`.
fn value(net: &Netlist, path: &str, name: &str) -> f64 {
    let ep = net.resolve_element(path).unwrap();
    let p = param(&ep, name);
    p.value
        .unwrap_or_else(|| panic!("{path}.{name}: unresolved: {:?}", p.unresolved))
}

/// The golden tolerance: |got - want| <= 2e-4 * max(|want|, 1e-30).
fn close(got: f64, want: f64) -> bool {
    (got - want).abs() <= 2e-4 * want.abs().max(1e-30)
}

fn approx(got: f64, want: f64) {
    assert!(close(got, want), "got {got}, want {want}");
}

fn unresolved(r: &Resolved) -> &str {
    assert!(r.value.is_none(), "{} has a value: {r:?}", r.name);
    r.unresolved
        .as_deref()
        .unwrap_or_else(|| panic!("{}: no reason: {r:?}", r.name))
}

fn caveats_hold(r: &Resolved, part: &str) -> bool {
    r.caveats.iter().any(|c| c.contains(part))
}

/// The lines (`file:line`, the file name only) of the Errors `code` of the
/// load of `net`.
fn error_lines(net: &Netlist, code: &str) -> Vec<String> {
    let d = net.diagnostics(None, &Page::default()).unwrap();
    d.items
        .iter()
        .filter(|x| x.code == code && x.level == Level::Error)
        .map(|x| {
            let loc = x.loc.as_ref().expect("a line");
            let file = Path::new(&loc.file).file_name().unwrap().to_string_lossy();
            format!("{file}:{}", loc.line)
        })
        .collect()
}

/// The R value of every resolved resistor of every golden case equals the
/// `.op` value (times the instance `m`: `.op` prints R/m). A case is
/// skipped only when HSPICE rejects it (`extra.rejected`, with a NOTES.md
/// line that says REJECTED), and a resistor
/// only when its `.op` value is temperature-adjusted
/// (`extra.skip_value_check`, with a NOTES.md line for the case).
#[test]
fn golden_all_resistors() {
    let notes = std::fs::read_to_string(repo("golden/NOTES.md")).unwrap();
    let mut failures: Vec<String> = Vec::new();
    let (mut loaded, mut rejected, mut checked, mut skipped) = (0, 0, 0, 0);
    let all = cases();
    for (case, dir) in &all {
        let exp = expected(dir);
        let extra = &exp["extra"];
        if !extra["rejected"].is_null() {
            assert!(
                notes
                    .lines()
                    .any(|l| l.contains(&format!("`{case}`")) && l.contains("REJECTED")),
                "{case}: `extra.rejected` with no NOTES.md line that says REJECTED"
            );
            rejected += 1;
            continue;
        }
        let resistors = exp["resistors"].as_array().unwrap();
        let skip: Vec<&str> = extra["skip_value_check"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_str().unwrap()).collect())
            .unwrap_or_default();
        if !skip.is_empty() {
            assert!(
                notes
                    .lines()
                    .any(|l| l.contains(&format!("`{case}`")) && l.contains("skip_value_check")),
                "{case}: `skip_value_check` with no NOTES.md line that gives the reason"
            );
        }
        for s in &skip {
            assert!(
                resistors.iter().any(|r| r["path"] == *s),
                "{case}: skipped path `{s}` is not a resistor of the case"
            );
        }
        let net = match Netlist::load(&LoadOptions::new(dir.join("deck.sp"))) {
            Ok(net) => net,
            Err(e) => {
                failures.push(format!("{case}: load: {e}"));
                continue;
            }
        };
        loaded += 1;
        for r in resistors {
            let path = r["path"].as_str().unwrap();
            if skip.contains(&path) {
                skipped += 1;
                continue;
            }
            let want = r["value"].as_f64().unwrap() * r["m"].as_f64().unwrap_or(1.0);
            let got = net.resolve_element(path).map(|ep| {
                ep.params
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case("r"))
                    .map(|p| (p.value, p.unresolved.clone()))
            });
            match got {
                Ok(Some((Some(v), _))) if close(v, want) => checked += 1,
                other => failures.push(format!("{case}: {path}: want {want}, got {other:?}")),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!(loaded + rejected, all.len(), "every case dir is covered");
    // 738 after the 8b-G probes; new cases only add to it
    assert!(checked >= 738, "only {checked} resistors checked");
    assert_eq!(
        skipped, 20,
        "only the temperature-adjusted res_tc_positional r1-r4, rcl_param_tc r1, r3, rcl_tc_kw_after r4, r5, rcl_kw_after_more r7-r9 and tc_tnom_dtemp r1-r5, the output variables of rcl_output_var r1, r2 and the `.prot` names of prot_param r1, r2 are skipped"
    );
}

/// The `dc`, `ac` and `acphase` of every source in `extra.sources` equal
/// HSPICE's (`op_value` where `.op` differs from `.option list`): with no
/// DC value, a function gives its value at t=0 (golden `src_fn_nodc`,
/// `src_sin_t0`, `pwl_*`), and an AC magnitude with no phase gives phase 0
/// (`src_ac_default_phase`).
#[test]
fn golden_sources() {
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0;
    for (case, dir) in cases() {
        let exp = expected(&dir);
        let extra = &exp["extra"];
        let Some(sources) = extra["sources"].as_object() else {
            continue;
        };
        if !extra["rejected"].is_null() {
            continue;
        }
        let net = load(dir.join("deck.sp"));
        for (name, s) in sources {
            let ep = match net.resolve_element(name) {
                Ok(ep) => ep,
                Err(e) => {
                    failures.push(format!("{case}: {name}: {e}"));
                    continue;
                }
            };
            let want_dc = s["op_value"].as_f64().or(s["dc"].as_f64()).unwrap();
            let mut check = |key: &str, want: f64| {
                let got = ep
                    .params
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(key))
                    .and_then(|p| p.value);
                match got {
                    Some(v) if close(v, want) || v == want => checked += 1,
                    _ => failures.push(format!("{case}: {name}.{key}: want {want}, got {got:?}")),
                }
            };
            check("dc", want_dc);
            if let Some(m) = s["ac_mag"].as_f64() {
                check("ac", m);
            }
            if let Some(p) = s["ac_phase"].as_f64() {
                check("acphase", p);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(checked >= 40, "only {checked} source values checked");
}

/// The bare numbers after an R, C or L value are TC1, TC2 (golden
/// `res_tc_positional`, `cl_tc_positional`). The value is the element's
/// own value; at a temperature that is not TNOM, a caveat gives the
/// temperature-adjusted value, which `.op` prints.
#[test]
fn golden_rcl_tc() {
    // `rcl_name_first_cl`, `ind_name_model`: a parameter name in the C or
    // L value place, then numbers or a model name
    for (case, (group, key, at_temp)) in ["cl_tc_positional", "rcl_name_first_cl", "ind_name_model"]
        .into_iter()
        .flat_map(|case| {
            [
                ("capacitors", "c", "c_at_temp"),
                ("inductors", "l", "l_at_temp"),
            ]
            .map(|g| (case, g))
        })
    {
        let net = golden(case);
        let exp = expected(&repo(&format!("golden/cases/{case}")));
        let Some(group) = exp["extra"][group].as_object() else {
            continue;
        };
        for (name, want) in group {
            let ep = net.resolve_element(name).unwrap();
            approx(value(&net, name, key), want[key].as_f64().unwrap());
            for tc in ["tc1", "tc2"] {
                let w = want[tc].as_f64().unwrap();
                if w == 0.0 {
                    assert!(!has_param(&ep, tc), "{name} has {tc}");
                } else {
                    approx(param(&ep, tc).value.unwrap(), w);
                }
            }
            assert_eq!(ep.model.as_deref(), want["model"].as_str(), "{name}: model");
            let v = param(&ep, key);
            if want["tc1"].as_f64() != Some(0.0) {
                let adjusted = want[at_temp].as_f64().unwrap();
                let shown = format!("{adjusted:e}");
                assert!(
                    v.caveats
                        .iter()
                        .any(|c| c.contains("125") && c.contains(&shown)),
                    "{name}: no caveat with the value {shown} at 125: {:?}",
                    v.caveats
                );
            } else if case == "cl_tc_positional" {
                assert!(v.caveats.is_empty(), "{name}: {:?}", v.caveats);
            } else {
                assert!(!caveats_hold(v, "TC1/TC2"), "{name}: {:?}", v.caveats);
            }
        }
    }
    // the caveat of a name in the value place names the probe
    for (case, name, key) in [
        ("rcl_name_first_cl", "c1", "c"),
        ("rcl_name_first_cl", "c2", "c"),
        ("rcl_name_first_cl", "l1", "l"),
        ("rcl_name_first_cl", "l4", "l"),
        ("ind_name_model", "l2", "l"),
        ("ind_name_model", "l3", "l"),
    ] {
        let ep = golden(case).resolve_element(name).unwrap();
        let v = param(&ep, key);
        assert!(
            caveats_hold(v, &format!("golden `{case}`")) || caveats_hold(v, &format!("`{case}`)")),
            "{case} {name}: {:?}",
            v.caveats
        );
    }
    // the value at TNOM, and the `.op` value (adjusted to 125) in the
    // caveat; `rcl_param_tc` has a parameter name as the value, and
    // `rcl_tc_kw_after`, `rcl_kw_after_more` an `r=` after it (the value
    // at TNOM of each is `nominal_value`, or `r` in `tc`)
    for case in [
        "res_tc_positional",
        "rcl_param_tc",
        "rcl_tc_kw_after",
        "rcl_kw_after_more",
    ] {
        let net = golden(case);
        let exp = expected(&repo(&format!("golden/cases/{case}")));
        let nominal = exp["extra"]["nominal_value"].as_f64();
        for (name, tc) in exp["extra"]["tc"].as_object().unwrap() {
            let ep = net.resolve_element(name).unwrap();
            if let Some(nominal) = nominal {
                approx(param(&ep, "r").value.unwrap(), nominal);
            }
            assert_eq!(
                ep.params
                    .iter()
                    .filter(|p| p.name.starts_with("tc"))
                    .count(),
                tc.as_object()
                    .unwrap()
                    .keys()
                    .filter(|k| k.starts_with("tc"))
                    .count(),
                "{case} {name}: {:?}",
                ep.params
            );
            for (k, w) in tc.as_object().unwrap() {
                approx(param(&ep, k).value.unwrap(), w.as_f64().unwrap());
            }
            let op = exp["resistors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["path"] == name.as_str())
                .unwrap()["value"]
                .as_f64()
                .unwrap();
            let shown = format!("{op}");
            let r = param(&ep, "r");
            assert!(
                r.caveats
                    .iter()
                    .any(|c| c.contains("125") && c.contains(&shown)),
                "{case} {name}: no caveat with the value {shown} at 125: {:?}",
                r.caveats
            );
        }
    }
    // no TC at 125: no caveat
    let net = golden("res_tc_positional");
    let ep = net.resolve_element("r5").unwrap();
    assert!(param(&ep, "r").caveats.is_empty());
    // TNOM from `.option tnom`: dT = 125 - 50
    let (_d, net) = load_text(
        "tnom",
        "* t\n.temp 125\n.option tnom=50\nv1 1 0 1\nr1 1 0 1k tc1=0.004\nr2 1 0 1k\n.end\n",
    );
    let ep = net.resolve_element("r1").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(1000.0));
    assert!(
        caveats_hold(r, "TNOM 50") && caveats_hold(r, "= 1300 "),
        "{:?}",
        r.caveats
    );
}

/// The chain lists the definitions that gave the value; the winner of
/// the name the element uses comes last (golden `parhier_*`).
#[test]
fn chain_shows_winner() {
    for (case, kind, want) in [
        ("parhier_global", StepKind::GlobalParam, 4.0),
        ("parhier_default", StepKind::GlobalParam, 4.0),
        ("parhier_local", StepKind::InstanceOverride, 3.0),
    ] {
        let net = golden(case);
        let ep = net.resolve_element("x1.r1").unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(want), "{case}");
        assert_eq!(r.chain[0].kind, StepKind::ElementParam, "{case}");
        let last = r.chain.last().unwrap();
        assert_eq!((last.kind, last.value), (kind, Some(want)), "{case}");
        assert!(last.text.contains(&format!("{want}")), "{case}: {last:?}");
        assert!(last.loc.is_some());
        // resolve_param in the instance scope gives the same winner
        let p = net.resolve_param("x1", "WhoWins").unwrap();
        assert_eq!(p.value, Some(want), "{case}");
        assert_eq!(p.chain.last().unwrap().kind, kind);
    }
    // a scope chain: x10.xi.r10 of nested_global is u = w*5 with the
    // top-level w = 9 (golden: 45)
    let net = golden("nested_global");
    let ep = net.resolve_element("x10.xi.r10").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(45.0));
    let kinds: Vec<StepKind> = r.chain.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            StepKind::ElementParam,
            StepKind::InstanceOverride,
            StepKind::GlobalParam
        ],
        "{:?}",
        r.chain
    );
    assert_eq!(r.chain[1].scope, "x10.xi");
    assert_eq!(r.chain[2].scope, "");
}

/// A name that refers to itself through other names is unresolved, with
/// the cycle in the reason; no number is given.
#[test]
fn cycle_unresolved() {
    let (_d, net) = load_text(
        "cycle",
        "* t\n.param a='b+1' b='a*2' c=3\nv1 1 0 1\nr1 1 0 a\nr2 1 0 c\n\
         .subckt s p q\n.param z='z+1'\nrz p q z\n.ends\nx1 1 0 s\n.end\n",
    );
    let ep = net.resolve_element("r1").unwrap();
    let why = unresolved(param(&ep, "r"));
    assert!(
        why.contains("cycle") && why.contains("a -> b -> a"),
        "{why}"
    );
    assert_eq!(value(&net, "r2", "r"), 3.0);
    let ep = net.resolve_element("x1.rz").unwrap();
    let why = unresolved(param(&ep, "r"));
    assert!(why.contains("z -> z"), "{why}");
    let p = net.resolve_param("", "b").unwrap();
    assert!(unresolved(&p).contains("cycle"));
}

/// A statistical function in a `.param` gives its nominal value with a
/// caveat (golden `stat_nominal`). Directly in an element expression,
/// HSPICE rejects it (golden `stat_direct`): unresolved, naming the
/// function.
#[test]
fn stat_caveat() {
    let net = golden("stat_nominal");
    for (path, f, want) in [
        ("ra", "agauss", 1e3),
        ("rg", "gauss", 2e3),
        ("ru", "aunif", 3e3),
        ("rn", "unif", 4e3),
    ] {
        let ep = net.resolve_element(path).unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(want));
        assert!(
            r.caveats
                .iter()
                .any(|c| c.contains(&format!("`{f}`")) && c.contains("nominal")),
            "{path}: {:?}",
            r.caveats
        );
    }
    let net = golden("stat_direct");
    let ep = net.resolve_element("rd").unwrap();
    let why = unresolved(param(&ep, "r"));
    assert!(why.contains("agauss") && why.contains("directly"), "{why}");
}

/// A parameter or element that an `.alter` block changes gets a caveat;
/// the value is the first run's (golden `alter`).
#[test]
fn alter_caveat() {
    let net = golden("alter");
    let ep = net.resolve_element("r1").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(1000.0));
    assert!(caveats_hold(r, ".alter #1"), "{:?}", r.caveats);
    let ep = net.resolve_element("r2").unwrap();
    assert_eq!(param(&ep, "r").value, Some(2000.0));
    assert!(
        ep.caveats.iter().any(|c| c.contains(".alter #2")),
        "{:?}",
        ep.caveats
    );
    let p = net.resolve_param("", "p").unwrap();
    assert_eq!(p.value, Some(1000.0));
    assert!(caveats_hold(&p, ".alter #1"));
}

/// `temper` is the first `.temp` value, else 25, with a caveat (golden
/// `temper`, `temper_default`).
#[test]
fn temper_value_and_caveat() {
    for (case, t) in [("temper", 50.0), ("temper_default", 25.0)] {
        let net = golden(case);
        for path in ["r1", "r2"] {
            let ep = net.resolve_element(path).unwrap();
            let r = param(&ep, "r");
            assert_eq!(r.value, Some(100.0 + t), "{case} {path}");
            assert!(
                r.caveats
                    .iter()
                    .any(|c| c.contains("temper") && c.contains(&format!("{t}"))),
                "{case} {path}: {:?}",
                r.caveats
            );
            assert!(r.chain.iter().any(|s| s.kind == StepKind::Builtin));
        }
    }
}

/// `.param f(a,b)` functions: GLOBAL looks at the top level first, then
/// down the instance path (golden `func_scope`, `func_two_levels_global`);
/// LOCAL looks at the scope of the call, then up the path, nearest first,
/// then at the top level (golden `func_scope_local`, `ovr_self_combo`,
/// `func_levels_local`; `function_creation_order` pins the exception). A
/// function defined only in a subckt is visible there and below. A free name in the body is
/// resolved where the function is defined, also for a function defined in
/// a subckt (golden `func_free_local`, `func_nested_local`).
#[test]
fn param_function_scope() {
    // the argument list can go on on a `+` line (golden `func_args_plus`)
    let net = golden("func_args_plus");
    assert_eq!(net.summary().errors, 0);
    assert_eq!(value(&net, "r1", "r"), 3000.0);
    for (case, own) in [("func_scope", 2000.0), ("func_scope_local", 3000.0)] {
        let net = golden(case);
        assert_eq!(value(&net, "x1.r1", "r"), own, "{case}");
        assert_eq!(value(&net, "x1.xl.rl", "r"), 2000.0, "{case}");
        assert_eq!(value(&net, "x2.xl.rg", "r"), 5000.0, "{case}");
        let ep = net.resolve_element("x2.xl.rg").unwrap();
        let g: Vec<&str> = param(&ep, "r")
            .chain
            .iter()
            .filter(|s| s.kind == StepKind::ParamFunction)
            .map(|s| s.scope.as_str())
            .collect();
        assert_eq!(g, ["x2"], "{case}: g is defined in x2's subckt");
    }
    let ep = golden("func_free_local").resolve_element("x1.r1").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(2000.0));
    assert!(r.caveats.is_empty(), "{:?}", r.caveats);
    let (_d, net) = load_text(
        "funcs",
        "* t\n.param f(x)='x*2' k=10 h(x)='x*k'\nv1 1 0 1\n\
         .subckt a n1 n2\n.param g(x)='x+100' k=1\nra n1 n2 'f(3)+g(1)'\nrh n1 n2 'h(2)'\nxb n1 n2 b\n.ends\n\
         .subckt b n1 n2\nrb n1 n2 'g(2)'\n.ends\n\
         .subckt c n1 n2\n.param f(x)='x*3'\nrc n1 n2 'f(2)'\nrg n1 n2 'g(2)'\n.ends\n\
         x1 1 0 a\nx2 1 0 c\nr0 1 0 'f(5)'\nr9 1 0 'g(1)'\n.end\n",
    );
    let ep = net.resolve_element("x1.ra").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(107.0));
    let funcs: Vec<(&str, &str)> = r
        .chain
        .iter()
        .filter(|s| s.kind == StepKind::ParamFunction)
        .map(|s| (s.name.as_str(), s.scope.as_str()))
        .collect();
    assert_eq!(funcs, [("f", ""), ("g", "x1")]);
    assert_eq!(value(&net, "x1.xb.rb", "r"), 102.0);
    assert_eq!(value(&net, "x2.rc", "r"), 4.0, "GLOBAL: the top-level f");
    assert_eq!(value(&net, "r0", "r"), 10.0);
    for path in ["x2.rg", "r9"] {
        let ep = net.resolve_element(path).unwrap();
        let why = unresolved(param(&ep, "r"));
        assert!(why.contains('g'), "{path}: {why}");
    }
    // `k` in the body of the top-level `h` is the top-level k = 10, not
    // the k = 1 of the calling subckt (golden `func_free_local`)
    let ep = net.resolve_element("x1.rh").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(20.0));
    assert!(r.caveats.is_empty(), "{:?}", r.caveats);
    // under LOCAL, `k` in the body of `a`'s `q` is `a`'s k = 1, also when
    // `q` is called from `b` below it, whose default is k = 7 (golden
    // `func_nested_local`: x3.xk.r3)
    let (_d, net) = load_text(
        "funcs_local",
        "* t\n.option parhier=local\n.param k=10\nv1 1 0 1\n\
         .subckt a n1 n2\n.param k=1 q(x)='x*k'\nrq n1 n2 'q(2)'\nxb n1 n2 b\n.ends\n\
         .subckt b n1 n2 k=7\nrq n1 n2 'q(2)'\n.ends\nx1 1 0 a\n.end\n",
    );
    for path in ["x1.rq", "x1.xb.rq"] {
        let ep = net.resolve_element(path).unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(2.0), "{path}");
        assert!(r.caveats.is_empty(), "{path}: {:?}", r.caveats);
    }
    let net = golden("func_nested_local");
    for (path, want) in [
        ("x1.r1", 2000.0),
        ("x2.xk.r2", 5000.0),
        ("x3.xk.r3", 2000.0),
    ] {
        let ep = net.resolve_element(path).unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(want), "{path}");
        assert!(r.caveats.is_empty(), "{path}: {:?}", r.caveats);
    }
    // two instance levels above the call define `g`, the top level does
    // not: GLOBAL takes the outer one (golden `func_two_levels_global`),
    // LOCAL the nearer one (golden `ovr_self_combo`); a call in an `X`-line
    // override uses the new instance's scope (LOCAL: `cf`'s `f`, golden
    // `func_three_local`)
    for mode in ["global", "local"] {
        let (_d, net) = load_text(
            &format!("funcs_levels_{mode}"),
            &format!(
                "* t\n.option parhier={mode}\n.param f(x)='x*2'\nv1 1 0 1\n\
                 .subckt a n1 n2\n.param g(x)='x+1'\nxb n1 n2 b\n.ends\n\
                 .subckt b n1 n2\n.param g(x)='x+2'\nxc n1 n2 c\n.ends\n\
                 .subckt c n1 n2\nrg n1 n2 'g(10)'\n.ends\n\
                 .subckt cf n1 n2 w=1\n.param f(x)='x*3'\nrw n1 n2 w\n.ends\n\
                 x1 1 0 a\nx2 1 0 cf w='f(1)'\n.end\n"
            ),
        );
        let want = if mode == "local" { 12.0 } else { 11.0 };
        assert_eq!(value(&net, "x1.xb.xc.rg", "r"), want, "{mode}");
        let want = if mode == "local" { 3.0 } else { 2.0 };
        assert_eq!(value(&net, "x2", "w"), want, "{mode}");
    }
    // the funcs golden case: a two-argument function
    assert_eq!(value(&golden("funcs"), "rf", "r"), 7.0);
}

/// Two tokens after the R/C/L nodes: model and value when a `.model` of
/// the first word is visible, else value and TC1 (R, C, and L with a
/// number second) or value and model (L with a name second). One bare
/// name is a model when a `.model` of that name is visible.
#[test]
fn rcl_two_tokens() {
    let (_d, net) = load_text(
        "rcl",
        "* t\n.param rval=2k lval=3n\n.model rmod r\n.model lmod l level=3\nv1 1 0 1\n\
         r1 1 0 rval 0.004\nr2 1 0 rmod 5k\nr3 1 0 rmod\nr4 1 0 rval 0.004 1e-5\n\
         r5 1 0 rval 0.004 tc1=1e-3\nr6 1 0 nomodel r=3k\nr7 1 0 rval\nr8 1 0 rval r=3k\n\
         r9 1 0 rval 0.004 1e-5 3e-6 tc2=2e-5 r=3k\nl5 1 6 lval lmod2 0.004 l=5n\nl6 1 7 1n lmod l=5n\n\
         l1 1 2 lval lmod\nl2 1 3 1n lmod\nl3 1 4 lval 0.004\nl4 1 5 lmod 2n\n\
         c1 1 0 rval 0.004\n.end\n",
    );
    let ep = net.resolve_element("r1").unwrap();
    assert_eq!(param(&ep, "r").value, Some(2000.0));
    assert_eq!(param(&ep, "tc1").value, Some(0.004));
    assert_eq!(ep.model, None);
    assert!(caveats_hold(param(&ep, "r"), "rval"));
    let ep = net.resolve_element("r2").unwrap();
    assert_eq!(
        (param(&ep, "r").value, ep.model.as_deref()),
        (Some(5000.0), Some("rmod"))
    );
    let ep = net.resolve_element("r3").unwrap();
    assert!(unresolved(param(&ep, "r")).contains("rmod"));
    assert_eq!(ep.model.as_deref(), Some("rmod"));
    let ep = net.resolve_element("r4").unwrap();
    assert_eq!(param(&ep, "r").value, Some(2000.0));
    assert_eq!(param(&ep, "tc1").value, Some(0.004));
    assert_eq!(param(&ep, "tc2").value, Some(1e-5));
    let ep = net.resolve_element("r5").unwrap();
    assert_eq!(param(&ep, "tc1").value, Some(1e-3), "the keyword wins");
    assert!(!has_param(&ep, "tc2"));
    // a name in the value place, then `r=`: HSPICE uses the name and not
    // the `r=` (golden `rcl_kw_after_more`); an undefined name has no value
    let ep = net.resolve_element("r6").unwrap();
    assert!(unresolved(param(&ep, "r")).contains("nomodel"));
    assert_eq!(value(&net, "r7", "r"), 2000.0);
    let ep = net.resolve_element("r8").unwrap();
    assert_eq!(param(&ep, "r").value, Some(2000.0));
    assert!(caveats_hold(param(&ep, "r"), "`r=3k`"));
    // the numbers after the name are TC1, TC2 (golden `rcl_tc_kw_after`), a
    // keyword wins over the number in its place, a third has no place
    let ep = net.resolve_element("r9").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(2000.0));
    assert!(
        caveats_hold(r, "`3e-6`") && caveats_hold(r, "TC1 and TC2"),
        "{:?}",
        r.caveats
    );
    assert_eq!(param(&ep, "tc1").value, Some(0.004));
    assert_eq!(param(&ep, "tc2").value, Some(2e-5));
    assert_eq!(ep.params.len(), 3, "{:?}", ep.params);
    // L: a name, then a model name, then TC1, then `l=` (not probed);
    // a number before the model is replaced by `l=` (golden `elem_dup_kw`)
    let ep = net.resolve_element("l5").unwrap();
    assert_eq!(param(&ep, "l").value, Some(3e-9));
    assert_eq!(param(&ep, "tc1").value, Some(0.004));
    assert!(caveats_hold(param(&ep, "l"), "was probed for R only"));
    assert!(
        ep.caveats.iter().any(|c| c.contains("lmod2")),
        "{:?}",
        ep.caveats
    );
    let ep = net.resolve_element("l6").unwrap();
    assert_eq!(
        (param(&ep, "l").value, ep.model.as_deref()),
        (Some(5e-9), Some("lmod"))
    );
    let ep = net.resolve_element("l1").unwrap();
    assert_eq!(
        (param(&ep, "l").value, ep.model.as_deref()),
        (Some(3e-9), Some("lmod"))
    );
    let ep = net.resolve_element("l2").unwrap();
    assert_eq!(
        (param(&ep, "l").value, ep.model.as_deref()),
        (Some(1e-9), Some("lmod"))
    );
    let ep = net.resolve_element("l3").unwrap();
    assert_eq!(param(&ep, "l").value, Some(3e-9));
    assert_eq!(param(&ep, "tc1").value, Some(0.004));
    let ep = net.resolve_element("l4").unwrap();
    assert_eq!(
        (param(&ep, "l").value, ep.model.as_deref()),
        (Some(2e-9), Some("lmod"))
    );
    let ep = net.resolve_element("c1").unwrap();
    assert_eq!(param(&ep, "c").value, Some(2000.0));
    assert_eq!(param(&ep, "tc1").value, Some(0.004));
}

/// The `dc` of a source with no DC value is its function's value at t=0
/// for the probed forms, with a caveat; other forms are unresolved.
#[test]
fn source_dc_rules() {
    let (_d, net) = load_text(
        "src",
        "* t\n.param v0=0.7\n\
         v1 1 0 PWL(0 'v0' 1n 1)\nv2 2 0 PWL(-1n 0 1n 1)\nv3 3 0 SIN(0.5 1 1meg 0 0 30)\n\
         v4 4 0 AM(1 0.5 1k 1meg 1n)\nv5 5 0 AC 1\nv6 6 0 PULSE('v0*2' 1 0 1n)\n\
         v7 7 0 DC 0.25 SIN(0 1 1meg)\n.end\n",
    );
    let dc = |path: &str| {
        let ep = net.resolve_element(path).unwrap();
        param(&ep, "dc").clone()
    };
    let r = dc("v1");
    assert_eq!(r.value, Some(0.7));
    assert!(caveats_hold(&r, "t=0"), "{:?}", r.caveats);
    approx(dc("v2").value.unwrap(), 0.5);
    approx(dc("v3").value.unwrap(), 1.0);
    // AM before its delay: 0 (golden `src_delay_scale`)
    let r = dc("v4");
    assert_eq!(r.value, Some(0.0));
    assert!(caveats_hold(&r, "t=0"), "{:?}", r.caveats);
    let ep = net.resolve_element("v5").unwrap();
    assert_eq!(param(&ep, "dc").value, Some(0.0));
    assert_eq!(param(&ep, "acphase").value, Some(0.0));
    assert_eq!(dc("v6").value, Some(1.4));
    let r = dc("v7");
    assert_eq!(r.value, Some(0.25));
    assert!(r.caveats.is_empty(), "{:?}", r.caveats);
    // the function itself has no single value
    let ep = net.resolve_element("v7").unwrap();
    assert!(param(&ep, "sin").value.is_none());
}

/// `resolve_param`: a scope path names an instance (or `""`, the top
/// level, also in a bare netlist); an unknown name gives close names.
#[test]
fn resolve_param_scopes() {
    let net = golden("inherit");
    let ep = net.resolve_element("x1.xc.r2").unwrap();
    let want = param(&ep, "r").value.unwrap();
    assert_eq!(want, 4.0);
    // a device path is not a scope
    assert!(matches!(
        net.resolve_param("x1.xc.r2", "k"),
        Err(QueryError::BadArgument(_))
    ));
    let p = net.resolve_param("x1.xc", "k").unwrap();
    assert_eq!(p.value, Some(want));
    assert_eq!(net.resolve_param("X1", "kk").unwrap().value, Some(3.0));
    match net.resolve_param("x1.xc", "kkk") {
        Err(QueryError::NotFound { suggestions, .. }) => {
            assert!(suggestions.iter().any(|s| s == "kk"), "{suggestions:?}")
        }
        other => panic!("{other:?}"),
    }
    // a bare netlist: the top-level `.param` lines still resolve
    let (_d, net) = load_text(
        "bare",
        ".subckt cell a b w=1\nr1 a b w\n.ends\n.param top=5\n",
    );
    assert_eq!(net.resolve_param("", "top").unwrap().value, Some(5.0));
    assert!(net.resolve_element("x1.r1").is_err());
    // `top=` makes the subckt the root: its defaults are its values
    let net = Netlist::load(&LoadOptions {
        top: Some("cell".to_owned()),
        ..LoadOptions::new(_d.main.clone())
    })
    .unwrap();
    let ep = net.resolve_element("r1").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(1.0));
    assert_eq!(r.chain.last().unwrap().kind, StepKind::SubcktDefault);
}

/// An unparsed element letter has no resolved parameters.
#[test]
fn unparsed_letter_unresolved() {
    let (_d, net) = load_text(
        "letters",
        "* t\n.param g=2\nv1 1 0 1\ne1 2 0 1 0 2 max=g\nr1 2 0 1k\n.end\n",
    );
    let ep = net.resolve_element("e1").unwrap();
    assert!(ep.caveats.iter().any(|c| c.contains("not parsed")));
    for p in &ep.params {
        assert!(unresolved(p).contains("element type not parsed"));
    }
}

/// A reference chain deeper than the limit is unresolved, not a stack
/// overflow; a long chain within the limit resolves.
#[test]
fn deep_chain_no_overflow() {
    let mut text = String::from("* t\n.param p0=1\n");
    for k in 1..5000 {
        text.push_str(&format!(".param p{k}='p{}+1'\n", k - 1));
    }
    text.push_str("v1 1 0 1\nr1 1 0 p4999\nr2 1 0 p50\n.end\n");
    let (_d, net) = load_text("deep", &text);
    let ep = net.resolve_element("r1").unwrap();
    let why = unresolved(param(&ep, "r"));
    assert!(why.contains("deep"), "{why}");
    assert_eq!(value(&net, "r2", "r"), 51.0);
}

/// `m` <= 0 through an expression: HSPICE rejects the deck (golden
/// `elem_m_zero`), so the value has a caveat.
#[test]
fn m_expression_not_positive() {
    let (_d, net) = load_text(
        "mexpr",
        "* t\n.param mz=0\nv1 1 0 1\nr1 1 0 1k m=mz\n.end\n",
    );
    let ep = net.resolve_element("r1").unwrap();
    let m = param(&ep, "m");
    assert_eq!(m.value, Some(0.0));
    assert!(caveats_hold(m, "HSPICE rejects"), "{:?}", m.caveats);
}

/// `.option scale` gives a caveat on the geometric parameters; `dtemp=`
/// on a line with TC1 gives the TC caveat at TNOM too; a duplicate
/// `.subckt` on the path gives a caveat (HSPICE rejects the deck, golden
/// `dup_subckt`).
#[test]
fn scale_dtemp_and_duplicate_caveats() {
    let (_d, net) = load_text(
        "scale",
        "* t\n.option scale=1e-6\n.model nch nmos level=1\nv1 1 0 1\n\
         m1 1 1 0 0 nch w=2 l=1 m=2\nr1 1 0 1k tc1=1e-3 dtemp=10\nr2 1 0 1k tc1=1e-3\n.end\n",
    );
    let ep = net.resolve_element("m1").unwrap();
    for name in ["w", "l"] {
        assert!(caveats_hold(param(&ep, name), "scale"), "{name}");
    }
    assert!(param(&ep, "m").caveats.is_empty());
    let ep = net.resolve_element("r1").unwrap();
    let r = param(&ep, "r");
    assert!(
        caveats_hold(r, "DTEMP 10") && caveats_hold(r, "= 1010 "),
        "{:?}",
        r.caveats
    );
    let ep = net.resolve_element("r2").unwrap();
    assert!(param(&ep, "r").caveats.is_empty());
    let (_d, net) = load_text(
        "dup",
        "* t\n.subckt inv a b\nr1 a b 1k\n.ends\n.subckt inv a b\nr1 a b 2k\n.ends\nx1 1 0 inv\n.end\n",
    );
    let ep = net.resolve_element("x1.r1").unwrap();
    assert_eq!(param(&ep, "r").value, Some(1000.0), "the first definition");
    assert!(
        ep.caveats.iter().any(|c| c.contains("more than once")),
        "{:?}",
        ep.caveats
    );
}

/// The parameter forms that HSPICE was not run on (golden/NOTES.md,
/// "Choices not probed"): a function that calls itself, a statistical
/// function in an `X` line override, a `.model` card in a subckt, a third
/// number after a name value, several temperatures, a chain of more than
/// 500 steps.
#[test]
fn unprobed_choices() {
    let mut text = String::from(
        "* t\n.temp 30\n.temp 60\n.param f(x)='x<1 ? 1 : x*f(x-1)' rval=2k\nv1 1 0 1\n\
         .subckt s a b w=1\n.model rm r\nr1 a b rm\nr2 a b 'w*2'\n.ends\n\
         x1 1 0 s w='agauss(1,0.1,3)'\nx2 1 0 s w=3\nrf 1 0 'f(3)'\nr9 1 0 rm 0.004\n\
         r8 1 0 rval 0.004 1e-5 3\nrt 1 0 'temper'\n.param p0=1\n",
    );
    for k in 1..600 {
        text.push_str(&format!(".param p{k}=1\n"));
    }
    for i in 0..30 {
        let terms: Vec<String> = (0..20).map(|j| format!("p{}", 20 * i + j)).collect();
        text.push_str(&format!(".param q{i}='{}'\n", terms.join("+")));
    }
    let terms: Vec<String> = (0..30).map(|i| format!("q{i}")).collect();
    text.push_str(&format!(
        ".param sum='{}'\nrs 1 0 sum\n.end\n",
        terms.join("+")
    ));
    let (_d, net) = load_text("unprobed", &text);
    let ep = net.resolve_element("rf").unwrap();
    assert!(unresolved(param(&ep, "r")).contains("calls itself"));
    let ep = net.resolve_element("x1.r2").unwrap();
    let why = unresolved(param(&ep, "r"));
    assert!(why.contains("agauss") && why.contains("directly"), "{why}");
    assert_eq!(value(&net, "x2.r2", "r"), 6.0);
    // the card in the subckt is visible from its lines only
    let ep = net.resolve_element("x2.r1").unwrap();
    assert_eq!(ep.model.as_deref(), Some("rm"));
    let ep = net.resolve_element("r9").unwrap();
    assert_eq!(ep.model, None);
    assert!(unresolved(param(&ep, "r")).contains("rm"));
    let ep = net.resolve_element("r8").unwrap();
    assert_eq!(param(&ep, "r").value, Some(2000.0));
    assert_eq!(param(&ep, "tc1").value, Some(0.004));
    assert_eq!(param(&ep, "tc2").value, Some(1e-5));
    assert!(
        caveats_hold(param(&ep, "r"), "no place"),
        "{:?}",
        param(&ep, "r").caveats
    );
    let ep = net.resolve_element("rt").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(30.0));
    assert!(
        caveats_hold(r, "more than one temperature"),
        "{:?}",
        r.caveats
    );
    let ep = net.resolve_element("rs").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(600.0));
    assert_eq!(r.chain.len(), 500);
    assert!(caveats_hold(r, "more than 500 steps"), "{:?}", r.caveats);
}

/// The text of an `X`-line override is evaluated in the scope of the
/// instance that the line makes (golden `ovr_scope_local`,
/// `ovr_scope_child_global`, `ovr_scope_more_local`); the overridden name
/// itself comes from the scope above (golden `ovr_self_local`), and with
/// none there HSPICE rejects the deck (golden `ovr_self_global`); `m` is
/// evaluated in the line's scope (golden `ovr_scope_more_local`).
#[test]
fn override_scope() {
    let net = golden("ovr_scope_local");
    let p = net.resolve_param("x1", "l").unwrap();
    assert_eq!(p.value, Some(6.0));
    let steps: Vec<(StepKind, &str)> = p.chain.iter().map(|s| (s.kind, s.scope.as_str())).collect();
    assert_eq!(
        steps,
        [
            (StepKind::InstanceOverride, "x1"),
            (StepKind::SubcktDefault, "x1")
        ],
        "{:?}",
        p.chain
    );
    // the `X` line itself: its overrides in the new instance's scope
    let ep = net.resolve_element("x2").unwrap();
    assert_eq!(param(&ep, "l").value, Some(15.0));
    assert_eq!(param(&ep, "w").value, Some(5.0));
    let net = golden("ovr_scope_child_global");
    assert_eq!(net.resolve_param("x1", "l").unwrap().value, Some(6.0));
    let net = golden("ovr_self_local");
    let p = net.resolve_param("x1", "w").unwrap();
    assert_eq!(p.value, Some(12.0));
    assert_eq!(p.chain.last().unwrap().kind, StepKind::GlobalParam);
    assert_eq!(value(&net, "x1", "w"), 12.0);
    let net = golden("ovr_self_global");
    for r in [
        param(&net.resolve_element("x1.r1").unwrap(), "r").clone(),
        param(&net.resolve_element("x1").unwrap(), "w").clone(),
    ] {
        let why = unresolved(&r);
        assert!(why.contains("ovr_self_global"), "{why}");
    }
    let net = golden("ovr_scope_more_local");
    assert_eq!(value(&net, "x2", "m"), 2.0, "m in the line's scope");
    assert_eq!(value(&net, "x1", "l"), 6.0);
    // an `X` line with no visible definition: the scope of an override
    // expression is not known; a number has its value
    let (_d, net) = load_text(
        "ovr_unbound",
        "* t\n.param k=2\nv1 1 0 1\nx1 1 0 nosuch w=3 l='k*2'\n.end\n",
    );
    let ep = net.resolve_element("x1").unwrap();
    assert_eq!(param(&ep, "w").value, Some(3.0));
    assert!(unresolved(param(&ep, "l")).contains("no visible definition"));
}

/// `m` <= 0 from an expression on an `X` line of the path: HSPICE rejects
/// the deck (golden `m_expr_zero`), so the answers below it have a caveat.
#[test]
fn m_expression_on_path() {
    let net = golden("m_expr_zero");
    let ep = net.resolve_element("x1.xq.r2").unwrap();
    assert_eq!(param(&ep, "r").value, Some(1000.0));
    assert!(
        ep.caveats.iter().any(|c| c.contains("m_expr_zero")),
        "{:?}",
        ep.caveats
    );
    let (_d, net) = load_text(
        "m_path",
        "* t\n.param k=3\nv1 1 0 1\n.subckt s a b k=1\nr1 a b 1k\n.ends\n\
         x1 1 0 s m='1-2'\nx2 1 0 s m='k-1'\n.end\n",
    );
    let ep = net.resolve_element("x1.r1").unwrap();
    assert!(
        ep.caveats.iter().any(|c| c.contains("HSPICE rejects")),
        "{:?}",
        ep.caveats
    );
    let p = net.resolve_param("x1", "k").unwrap();
    assert!(p.caveats.iter().any(|c| c.contains("HSPICE rejects")));
    // `m='k-1'` in the line's scope: 3 - 1 (not the instance's 1 - 1)
    let ep = net.resolve_element("x2.r1").unwrap();
    assert!(ep.caveats.is_empty(), "{:?}", ep.caveats);
}

/// The element `scale=` multiplies the R, C and L value (golden
/// `src_delay_scale`, `elem_scale_cl`); `.option scale` gives a caveat on
/// geometric parameters only, not on an inductor's inductance.
#[test]
fn element_scale() {
    let net = golden("elem_scale_cl");
    let exp = expected(&repo("golden/cases/elem_scale_cl"));
    for (group, key, eff) in [
        ("capacitors", "c", "cap_eff"),
        ("inductors", "l", "ind_eff"),
    ] {
        for (name, want) in exp["extra"][group].as_object().unwrap() {
            let ep = net.resolve_element(name).unwrap();
            let v = param(&ep, key);
            approx(v.value.unwrap(), want[eff].as_f64().unwrap());
            assert!(caveats_hold(v, "scale=3"), "{name}: {:?}", v.caveats);
        }
    }
    let ep = golden("src_delay_scale").resolve_element("r6").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(2000.0));
    assert!(r.chain.iter().any(|s| s.name.eq_ignore_ascii_case("scale")));
    // with TC1 at 125, the TC caveat uses the scaled value; with a model
    // card, the value is still multiplied (neither probed)
    let (_d, net) = load_text(
        "scale_tc",
        "* t\n.temp 125\n.model rm r\nv1 1 0 1\nr1 1 0 1k scale=2 tc1=0.004\nr2 1 0 rm 1k scale=2\n.end\n",
    );
    let ep = net.resolve_element("r1").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(2000.0));
    assert!(caveats_hold(r, "= 2800 "), "{:?}", r.caveats);
    let ep = net.resolve_element("r2").unwrap();
    let r = param(&ep, "r");
    assert_eq!((r.value, ep.model.as_deref()), (Some(2000.0), Some("rm")));
    assert!(caveats_hold(r, "model"), "{:?}", r.caveats);
    let (_d, net) = load_text(
        "scale_more",
        "* t\n.option scale=1e-6\nv1 1 0 1\nr1 1 0 1k scale=sx\nl1 1 2 3n\nr2 2 0 1k l=2 w=1\n.end\n",
    );
    let ep = net.resolve_element("r1").unwrap();
    assert!(unresolved(param(&ep, "r")).contains("scale"));
    let ep = net.resolve_element("l1").unwrap();
    assert!(param(&ep, "l").caveats.is_empty(), "{:?}", ep);
    let ep = net.resolve_element("r2").unwrap();
    for name in ["l", "w"] {
        assert!(caveats_hold(param(&ep, name), "option scale"), "{name}");
    }
}

/// A parameter name as the R or C value, then the name of a visible model
/// card: HSPICE rejects the R line (golden `res_name_model`) and uses a
/// capacitance of 0 for C (golden `cap_name_model`).
#[test]
fn rc_name_then_model() {
    let ep = golden("res_name_model").resolve_element("r1").unwrap();
    assert!(unresolved(param(&ep, "r")).contains("third node"));
    assert_eq!(ep.model.as_deref(), Some("rmod"));
    assert!(!has_param(&ep, "tc1"), "{ep:?}");
    let ep = golden("cap_name_model").resolve_element("c1").unwrap();
    assert!(unresolved(param(&ep, "c")).contains("capacitance of 0"));
    assert_eq!(ep.model.as_deref(), Some("cmod"));
}

/// The numbers after a name value move one place on (golden
/// `rcl_param_tc`); a keyword `tc1=` or `tc2=` wins over the number in its
/// place, and params stay unique by name.
#[test]
fn rcl_name_value_tc_keywords() {
    let (_d, net) = load_text(
        "tc_kw",
        "* t\n.param rval=2k\nv1 1 0 1\nr1 1 0 rval 0.004 1e-5 tc2=2e-5\n\
         r2 1 0 rval 0.004 1e-5 tc1=3e-3\nR3 1 0 R=1k\nr4 1 0 2k\n.end\n",
    );
    let names = |ep: &ElementParams| -> Vec<String> {
        ep.params
            .iter()
            .map(|p| p.name.to_ascii_lowercase())
            .collect()
    };
    let ep = net.resolve_element("r1").unwrap();
    assert_eq!(names(&ep), ["r", "tc1", "tc2"]);
    assert_eq!(param(&ep, "tc1").value, Some(0.004));
    assert_eq!(param(&ep, "tc2").value, Some(2e-5));
    let ep = net.resolve_element("r2").unwrap();
    assert_eq!(names(&ep), ["r", "tc1", "tc2"]);
    assert_eq!(param(&ep, "tc1").value, Some(3e-3));
    assert_eq!(param(&ep, "tc2").value, Some(1e-5));
    // the value is named `r`, whatever the deck's spelling of `R=`
    for path in ["r3", "r4"] {
        let ep = net.resolve_element(path).unwrap();
        assert_eq!(ep.params[0].name, "r", "{path}");
    }
}

/// The limits of one call: a value that needs more than 100000
/// evaluations has none (no hang, no unbounded memory); a failure at a
/// limit is not reused for a shallower use of the same name; a number out
/// of range has no value.
#[test]
fn limits_of_one_call() {
    let mut text = String::from("* t\n.param f0(x)='x'\n");
    for i in 1..=18 {
        text.push_str(&format!(
            ".param f{i}(x)='f{p}(2*x)+f{p}(2*x+1)'\n",
            p = i - 1
        ));
    }
    text.push_str("v1 1 0 1\nr1 1 0 'f18(1)'\nr2 1 0 'f10(1)'\n.end\n");
    let (_d, net) = load_text("budget", &text);
    let start = std::time::Instant::now();
    let ep = net.resolve_element("r1").unwrap();
    let why = unresolved(param(&ep, "r"));
    assert!(why.contains("100000"), "{why}");
    assert!(start.elapsed().as_secs() < 30, "{:?}", start.elapsed());
    // f10(1) = f0(1024) + … + f0(2047)
    assert_eq!(value(&net, "r2", "r"), f64::from((1024..2048).sum::<i32>()));
    let mut text = String::from("* t\n.param p0=1\n");
    for k in 1..=300 {
        text.push_str(&format!(".param p{k}='p{}+1'\n", k - 1));
    }
    // h1 = 1+1+…+1 (200 ones, height 200), h{k} = h{k-1}+1+…+1 (199
    // ones, height 200)
    let ones = |n: usize| vec!["1"; n].join("+");
    text.push_str(&format!(".param h1='{}'\n", ones(200)));
    for k in 2..=12 {
        text.push_str(&format!(".param h{k}='h{}+{}'\n", k - 1, ones(199)));
    }
    text.push_str(
        ".param big=1e400\nv1 1 0 1\nr1 1 0 p300 tc1=p100\n\
         r2 1 0 h8\nr3 1 0 h12\nr4 1 0 1e400\nr5 1 0 big\n.end\n",
    );
    let (_d, net) = load_text("limits", &text);
    let ep = net.resolve_element("r1").unwrap();
    assert!(unresolved(param(&ep, "r")).contains("deep"));
    assert_eq!(param(&ep, "tc1").value, Some(101.0), "not poisoned by r");
    // heights 8 × 200 = 1600: a value; 12 × 200 = 2400 > 2048: none
    assert_eq!(value(&net, "r2", "r"), f64::from(200 + 199 * 7));
    let ep = net.resolve_element("r3").unwrap();
    assert!(unresolved(param(&ep, "r")).contains("deep"));
    for path in ["r4", "r5"] {
        let ep = net.resolve_element(path).unwrap();
        let r = param(&ep, "r");
        assert!(unresolved(r).contains("out of range"), "{path}: {r:?}");
    }
}

/// `.alter` blocks: a caveat for an `X` line of the path that a block
/// replaces, for a `.temp` of a block, and for a `.param` function that a
/// block defines again; the values are the first run's.
#[test]
fn alter_more_caveats() {
    let (_d, net) = load_text(
        "alter_more",
        "* t\n.param f(x)='x*2'\nv1 1 0 1\n.subckt s a b w=1\nr1 a b w\nr2 a b 'temper'\nr3 a b 'f(1)'\n.ends\n\
         x1 1 0 s w=5\n.op\n.alter\nx1 1 0 s w=9\n.temp 100\n.param f(x)='x*3'\n.end\n",
    );
    let ep = net.resolve_element("x1.r1").unwrap();
    assert_eq!(param(&ep, "r").value, Some(5.0));
    assert!(
        ep.caveats.iter().any(|c| c.contains(".alter #1")),
        "{:?}",
        ep.caveats
    );
    let p = net.resolve_param("x1", "w").unwrap();
    assert!(p.caveats.iter().any(|c| c.contains(".alter #1")));
    let ep = net.resolve_element("x1.r2").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(25.0));
    assert!(
        caveats_hold(r, "first run has no `.temp`") && caveats_hold(r, "set again by .alter #1"),
        "{:?}",
        r.caveats
    );
    let ep = net.resolve_element("x1.r3").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(2.0));
    assert!(
        caveats_hold(r, "defined again by .alter #1"),
        "{:?}",
        r.caveats
    );
    // `.option tnom`, `.option scale` and a `.model` card of a block
    let (_d, net) = load_text(
        "alter_opts",
        "* t\n.temp 125\n.option scale=1e-6\n.model rm r\n.model nch nmos level=1\nv1 1 0 1\n\
         rr 1 0 rm 1k\nr1 1 0 1k tc1=0.004\nm1 1 1 0 0 nch w=2 l=1\n.op\n\
         .alter\n.option tnom=50 scale=2e-6\n.model rm r tc1=1\n.end\n",
    );
    for (path, name, part) in [
        ("rr", "r", "defines the model again"),
        ("r1", "r", "`.option tnom` is set again by .alter #1"),
        ("m1", "w", "`.option scale` is set again by .alter #1"),
    ] {
        let ep = net.resolve_element(path).unwrap();
        let r = param(&ep, name);
        assert!(caveats_hold(r, part), "{path}: {:?}", r.caveats);
    }
    // `.option parhier` in a block: the first run's value, with a caveat
    let (_d, net) = load_text(
        "alter_parhier",
        "* t\n.param w=4\nv1 1 0 1\n.subckt cc a b w=2\nr1 a b w\n.ends\nx1 1 0 cc w=3\n.op\n\
         .alter\n.option parhier=local\n.end\n",
    );
    let ep = net.resolve_element("x1.r1").unwrap();
    assert_eq!(param(&ep, "r").value, Some(4.0));
    let part = "`.option parhier` is set again by .alter #1";
    assert!(
        ep.caveats.iter().any(|c| c.contains(part)),
        "{:?}",
        ep.caveats
    );
    let p = net.resolve_param("x1", "w").unwrap();
    assert!(
        p.caveats.iter().any(|c| c.contains(part)),
        "{:?}",
        p.caveats
    );
}

/// A source function other than PULSE, EXP, SFFM, SIN, PWL and AM has no
/// `dc` value at t=0 (golden/NOTES.md, "Choices not probed"); SFFM with
/// 7 arguments gives its offset (golden `sffm_phase`).
#[test]
fn source_other_functions() {
    let (_d, net) = load_text(
        "pat",
        "* t\nv1 1 0 PAT(1 0 0 1n 1n 5n b1010)\nr1 1 0 1k\n.end\n",
    );
    let ep = net.resolve_element("v1").unwrap();
    assert!(unresolved(param(&ep, "dc")).contains("not resolved"));
    let ep = golden("sffm_phase").resolve_element("v1").unwrap();
    assert_eq!(param(&ep, "dc").value, Some(0.1));
}

/// `top` set to a nested definition: the definitions it is written inside
/// are scopes too (golden/NOTES.md, "Choices not probed").
#[test]
fn top_nested_scopes() {
    let (d, _) = load_text(
        "top_nested",
        "* t\n.subckt outer a b k=5\n.param q=7\n.subckt deep c d\nr1 c d 'k+q'\n.ends\nxd a b deep\n.ends\nx1 1 0 outer\n.end\n",
    );
    let net = Netlist::load(&LoadOptions {
        top: Some("outer>deep".to_owned()),
        ..LoadOptions::new(d.main.clone())
    })
    .unwrap();
    assert_eq!(value(&net, "r1", "r"), 12.0);
    let k = net.resolve_param("", "k").unwrap();
    assert_eq!(k.value, Some(5.0));
    // the scopes of `top=` and the definitions it is written inside have
    // no instance path
    assert_eq!(k.chain[0].scope, "(outer)");
}

/// The 1,000-cell × 102-device × 10-instance deck: one
/// `resolve_element` answers in < 10 ms (release build).
#[test]
#[ignore = "timing: run with --release -- --ignored"]
fn resolve_element_big_deck_fast() {
    let mut s = String::from(
        "* big deck\n.param l=0.1u wn=1u\n.model nch nmos level=1\n.model pch pmos level=1\n",
    );
    for k in 0..1000 {
        s.push_str(&format!(".subckt cell{k} a y vdd vss wn=1u\n"));
        for j in 0..34 {
            s.push_str(&format!("M{j} n{j} a vss vss nch W=wn L=l\n"));
            s.push_str(&format!("R{j} n{j} y {}k\n", j + 1));
            s.push_str(&format!("C{j} n{j} vss {}p\n", j + 1));
        }
        s.push_str(&format!(".ends cell{k}\n"));
    }
    for k in 0..1000 {
        for t in 0..10 {
            s.push_str(&format!("X{k}_{t} a{k} y{t} vdd 0 cell{k} wn=2u\n"));
        }
    }
    s.push_str(".op\n.end\n");
    let (_d, net) = load_text("big", &s);
    let start = std::time::Instant::now();
    let ep = net.resolve_element("x5_3.m7").unwrap();
    let took = start.elapsed();
    eprintln!("resolve_element(\"x5_3.m7\"): {took:?}");
    // GLOBAL: the top-level `.param wn=1u` beats the `X` override
    assert_eq!(param(&ep, "w").value, Some(1e-6));
    assert!(took.as_millis() < 10, "{took:?}");
}

/// Under GLOBAL a definition above a new instance beats the override on
/// its `X` line (golden `nested_global`, `parhier_global`):
/// `resolve_element` on the `X` line gives the override's own value, with a
/// caveat that names the winning definition and the value the instance
/// uses (which `resolve_param` gives). Under LOCAL the override wins.
#[test]
fn shadowed_override_caveat() {
    let net = golden("nested_global");
    for (path, name, own, used, winner) in [
        ("x7.xi", "q", 5.0, 2.0, "the `.subckt` default of `x7`"),
        ("x6.xi", "p", 5.0, 3.0, "the `X`-line override of `x6`"),
        ("x8.xi", "s", 7.0, 6.0, "the body `.param` of `x8`"),
    ] {
        let ep = net.resolve_element(path).unwrap();
        let p = param(&ep, name);
        assert_eq!(p.value, Some(own), "{path}");
        let shown = format!("the instance uses {name} = {used} (");
        assert!(
            caveats_hold(p, &shown) && caveats_hold(p, winner),
            "{path}: {:?}",
            p.caveats
        );
        let scope = path.to_owned();
        assert_eq!(net.resolve_param(&scope, name).unwrap().value, Some(used));
    }
    // x9.xm.xi: `t=8` loses to x9's `t=4`; x10.xi: `u` is defined nowhere
    // above, so the override wins and has no caveat
    let ep = net.resolve_element("x9.xm.xi").unwrap();
    assert!(caveats_hold(param(&ep, "t"), "t = 4"));
    let ep = net.resolve_element("x10.xi").unwrap();
    assert!(param(&ep, "u").caveats.is_empty(), "{:?}", ep.params);
    let ep = golden("parhier_global").resolve_element("x1").unwrap();
    let p = param(&ep, "whowins");
    assert_eq!(p.value, Some(3.0));
    assert!(
        caveats_hold(p, "the top-level `.param`") && caveats_hold(p, "whowins = 4"),
        "{:?}",
        p.caveats
    );
    // LOCAL: the override wins, no caveat
    let ep = golden("parhier_local").resolve_element("x1").unwrap();
    assert!(param(&ep, "whowins").caveats.is_empty());
    // a winner of 0 is written `0`
    let (_d, net) = load_text(
        "shadow_zero",
        "* t\n.param z=0\n.subckt c a b z=1\nr1 a b 'z+1'\n.ends\nx1 1 0 c z=5\nv1 1 0 1\n.end\n",
    );
    let ep = net.resolve_element("x1").unwrap();
    assert!(caveats_hold(param(&ep, "z"), "z = 0 "), "{:?}", ep.params);
    // the override text and the winning value share the caveats of the
    // `.alter` and of the sweep of `p`: each is listed once
    let (_d, net) = load_text(
        "shadow_dup",
        "* t\n.param p=1k\n.subckt cell a b p=5\nrc a b p\n.ends\nv1 1 0 1\n\
         x2 1 0 cell p='p*2'\n.dc p 1k 2k 100\n.alter\n.param p=3k\n.end\n",
    );
    let ep = net.resolve_element("x2").unwrap();
    let p = param(&ep, "p");
    assert!(caveats_hold(p, "p = 1000 ("), "{:?}", p.caveats);
    for part in ["changed by .alter #1", "swept by .dc"] {
        assert_eq!(
            p.caveats.iter().filter(|c| c.contains(part)).count(),
            1,
            "{part}: {:?}",
            p.caveats
        );
    }
}

/// Under LOCAL a call takes the nearest function definition up its
/// instance path, except when an instance that HSPICE creates before it
/// evaluated the same text and took the top-level one (golden
/// `func_scope_local`, `func_sticky_local`, `func_sticky_ovr_local`): then
/// the answer gives the top-level value, with a caveat that names that
/// instance.
#[test]
fn function_creation_order() {
    for (case, path, first) in [
        ("func_scope_local", "x1.xl.rl", "`xt`"),
        ("func_sticky_local", "x1.xl.rc", "`x0.xl`"),
    ] {
        let ep = golden(case).resolve_element(path).unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(2000.0), "{case}");
        assert!(caveats_hold(r, first), "{case}: {:?}", r.caveats);
        let f: Vec<&str> = r
            .chain
            .iter()
            .filter(|s| s.kind == StepKind::ParamFunction)
            .map(|s| s.scope.as_str())
            .collect();
        assert_eq!(f, [""], "{case}: the top-level f");
    }
    // the instance created first takes its parent's f, with no caveat
    let net = golden("func_sticky_local");
    let ep = net.resolve_element("x3.xl.rd").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(3000.0));
    assert!(r.caveats.is_empty(), "{:?}", r.caveats);
    // a deeper instance created later: `x2.xm.xl` (no f above) comes after
    // `x1.xl` (under cf), so x1.xl keeps cf's f
    let (_d, net) = load_text(
        "func_order",
        "* t\n.option parhier=local\n.param f(x)='x*2'\nv1 1 0 1\n\
         .subckt leaf a b\nrl a b 'f(1)'\n.ends\n\
         .subckt cf a b\n.param f(x)='x*3'\nxl a b leaf\n.ends\n\
         .subckt m a b\nxl a b leaf\n.ends\n.subckt w a b\nxm a b m\n.ends\n\
         x2 1 0 w\nx1 1 0 cf\n.end\n",
    );
    assert_eq!(value(&net, "x1.xl.rl", "r"), 3.0);
    assert_eq!(value(&net, "x2.xm.xl.rl", "r"), 2.0);
    // the unit is the text (golden `func_sticky_ovr_local`): a `.subckt`
    // default that the `X` line of the first instance overrides does not
    // count (x1.xl.rd); the override text of one `X` line counts for the
    // instances it makes (x3.xm.xl.ro, also on the `X` line itself); any
    // earlier instance counts, not only the first (cf6, then w6, then cf6)
    let net = golden("func_sticky_ovr_local");
    for (path, want, first) in [
        ("x1.xl.rd", 3000.0, None),
        ("x3.xm.xl.ro", 2000.0, Some("`x2.xm.xl`")),
        ("x4.xl.rb", 3000.0, None),
        ("x6.xl.rb", 2000.0, Some("`x5.xl`")),
    ] {
        let ep = net.resolve_element(path).unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(want), "{path}");
        match first {
            Some(f) => assert!(caveats_hold(r, f), "{path}: {:?}", r.caveats),
            None => assert!(r.caveats.is_empty(), "{path}: {:?}", r.caveats),
        }
    }
    assert_eq!(value(&net, "x3.xm.xl", "v"), 2000.0);
    // two `X` lines with one override text count separately (x5.xl.ro); a
    // body `.param` and a `.subckt` default count (golden
    // `rcl_quoted_sticky_local`)
    let net = golden("rcl_quoted_sticky_local");
    let ep = net.resolve_element("x5.xl.ro").unwrap();
    assert!(param(&ep, "r").caveats.is_empty(), "{:?}", ep.params);
    for (path, first) in [("x1.xl.rp", "`x0.xl`"), ("x3.xl.rh", "`x2.xl`")] {
        let ep = net.resolve_element(path).unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(2000.0), "{path}");
        assert!(caveats_hold(r, first), "{path}: {:?}", r.caveats);
    }
}

/// The creation-order check of `.param` functions reads at most 4000000
/// `X` lines in one call, over all its walks; after that, a call takes the
/// nearest definition with a caveat. Functions defined in the same
/// subckts share one walk.
#[test]
fn function_creation_order_bounded() {
    let deck = |distinct: bool| {
        let mut s =
            String::from("* t\n.option parhier=local\nv1 1 0 1\n.subckt w a b\nrw a b 1\n.ends\n");
        let calls: Vec<String> = (0..200).map(|i| format!("f{i}(1)")).collect();
        for i in 0..200 {
            s.push_str(&format!(".param f{i}(x)='x*2'\n"));
        }
        s.push_str(&format!(
            ".subckt leaf a b\nrl a b '{}'\n.ends\n.subckt mid a b\nxl a b leaf\n.ends\n.subckt cf a b\n",
            calls.join("+")
        ));
        for i in 0..200 {
            s.push_str(&format!(".param f{i}(x)='x*3'\n"));
        }
        s.push_str("xm a b mid\n.ends\n");
        if distinct {
            for i in 0..200 {
                s.push_str(&format!(".subckt u{i} a b\n.param f{i}(x)='x*5'\n.ends\n"));
            }
        }
        for k in 0..25_000 {
            s.push_str(&format!("xw{k} 1 0 w\n"));
        }
        s.push_str("xc 1 0 cf\n.end\n");
        s
    };
    for distinct in [false, true] {
        let (_d, net) = load_text(&format!("bounded_{distinct}"), &deck(distinct));
        let start = std::time::Instant::now();
        let ep = net.resolve_element("xc.xm.xl.rl").unwrap();
        let took = start.elapsed();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(600.0), "{distinct}");
        assert_eq!(
            caveats_hold(r, "more than 4000000"),
            distinct,
            "{distinct}: {:?}",
            r.caveats
        );
        assert!(took.as_secs() < 10, "{took:?}");
    }
}

/// A quoted expression in the place of the R, C or L value is the value,
/// and a later `r=`, `c=` or `l=` is not used (golden
/// `rcl_quoted_sticky_local`, `rcl_quoted_more`); a number there loses to
/// the keyword (golden `elem_dup_kw`). Not probed: the numbers after a
/// quoted value (TC1, TC2, as after a name), an L model after it, and a
/// quoted value after a model name.
#[test]
fn rcl_quoted_value() {
    let net = golden("rcl_quoted_more");
    for (path, key, want) in [
        ("r14", "r", 2000.0),
        ("r15", "r", 2000.0),
        ("c1", "c", 2e-12),
        ("c2", "c", 2e-12),
        ("l1", "l", 2e-9),
        ("l2", "l", 2e-9),
    ] {
        let ep = net.resolve_element(path).unwrap();
        let p = param(&ep, key);
        approx(p.value.unwrap(), want);
        assert!(
            caveats_hold(p, "rcl_quoted_more"),
            "{path}: {:?}",
            p.caveats
        );
    }
    assert_eq!(value(&net, "r15", "tc1"), 0.004);
    // a quoted C or L value that goes on on a `+` line (golden
    // `rcl_quoted_plus`: `cap eff` 6.0000p, `ind eff` 6.0000n)
    let net = golden("rcl_quoted_plus");
    approx(value(&net, "c1", "c"), 6e-12);
    approx(value(&net, "l1", "l"), 6e-9);
    let net = golden("rcl_quoted_sticky_local");
    assert_eq!(value(&net, "r10", "r"), 2000.0);
    assert_eq!(value(&net, "r13", "r"), 5000.0);
    let (_d, net) = load_text(
        "quoted_more",
        "* t\n.param rval=2k lval=2n cval=2p\n.model lmod l level=3\n.model cmod c\nv1 1 0 1\n\
         r1 1 0 'rval' 0.004 1e-5 r=5k\nl1 1 2 'lval' lmod 0.004 l=5n\nc1 1 0 cmod 'cval' c=5p\n.end\n",
    );
    let ep = net.resolve_element("r1").unwrap();
    assert_eq!(param(&ep, "r").value, Some(2000.0));
    assert_eq!(param(&ep, "tc1").value, Some(0.004));
    assert_eq!(param(&ep, "tc2").value, Some(1e-5));
    assert!(caveats_hold(
        param(&ep, "r"),
        "not probed after a quoted expression"
    ));
    let ep = net.resolve_element("l1").unwrap();
    assert_eq!(param(&ep, "l").value, Some(2e-9));
    assert_eq!(param(&ep, "tc1").value, Some(0.004));
    assert_eq!(ep.model.as_deref(), Some("lmod"));
    let ep = net.resolve_element("c1").unwrap();
    let c = param(&ep, "c");
    assert_eq!(c.value, Some(2e-12));
    assert!(caveats_hold(c, "after a model name"), "{:?}", c.caveats);
    // a quoted value that goes on on a `+` line, as `param_text` reads it
    // (golden `rcl_quoted_plus` pins the forms that HSPICE accepts); a `$`
    // inside the quotes is not a comment, and HSPICE rejects the deck
    // (golden `rcl_quoted_dollar`)
    let (_d, net) = load_text(
        "quoted_plus",
        "* t
.param rval=2k
v1 1 0 1
r3 1 0 'rval\n+ *3' r=5k
r4 1 0 'rval $ c\n+ *2' tc1=0.004 r=5k
.end
",
    );
    assert_eq!(value(&net, "r3", "r"), 6000.0);
    let ep = net.resolve_element("r4").unwrap();
    assert!(
        unresolved(param(&ep, "r")).contains("`$` inside quotes"),
        "{ep:?}"
    );
    assert_eq!(value(&net, "r4", "tc1"), 0.004);
}

/// A `$` inside quotes is not the start of a comment: HSPICE rejects the
/// deck (golden `rcl_quoted_dollar`: a quoted R value that goes on on a `+`
/// line; `rcl_quoted_dollar_line`: a quoted `.param` value;
/// `dollar_elem_line`: a quoted R value on one line; `dollar_func_body`,
/// `dollar_override`). The value has no number, and the reason says so;
/// the text keeps the `$`. A `$` outside quotes still starts a comment.
#[test]
fn dollar_in_quotes() {
    let why = |net: &Netlist, path: &str| {
        let ep = net.resolve_element(path).unwrap();
        let r = param(&ep, "r");
        let why = unresolved(r).to_owned();
        assert!(why.contains("`$` inside quotes"), "{path}: {why}");
        assert!(why.contains("rejects the deck"), "{path}: {why}");
        r.chain.iter().map(|s| s.text.clone()).collect::<Vec<_>>()
    };
    let net = golden("rcl_quoted_dollar");
    why(&net, "r1");
    assert_eq!(value(&net, "r2", "r"), 3000.0);
    // the loader gives an Error for each such text, as HSPICE rejects the
    // deck
    assert_eq!(error_lines(&net, "quoted_dollar"), ["deck.sp:4"]);
    for (case, lines) in [
        ("rcl_quoted_dollar_line", &["deck.sp:3", "deck.sp:5"][..]),
        ("dollar_elem_line", &["deck.sp:4"]),
        ("dollar_func_body", &["deck.sp:2"]),
        ("dollar_override", &["deck.sp:7"]),
    ] {
        assert_eq!(error_lines(&golden(case), "quoted_dollar"), lines, "{case}");
    }
    let net = golden("rcl_quoted_dollar_line");
    why(&net, "r1");
    let chain = why(&net, "r2");
    assert!(chain.iter().any(|t| t == "rval*2 $ c"), "{chain:?}");
    let q = net.resolve_param("", "q").unwrap();
    assert!(unresolved(&q).contains("`$` inside quotes"), "{q:?}");
    // a quoted R value on one line, a function body, an `X`-line override
    why(&golden("dollar_elem_line"), "r1");
    why(&golden("dollar_func_body"), "r1");
    why(&golden("dollar_override"), "x1.rc");
    let ep = golden("dollar_override").resolve_element("x1").unwrap();
    assert!(unresolved(param(&ep, "w")).contains("`$` inside quotes"));
    // not probed: a `key=value` of an element and a source-function
    // argument follow the same rule
    let (_d, net) = load_text(
        "dollar_more",
        "* t
.param rval=2k f(x)='x*2 $ c'
.subckt cell a b w=1
rc a b w
.ends
v1 1 0 1
r1 1 0 'f(1)*1k'
x2 2 0 cell w='rval $ c'
r3 3 0 1k tc1='0.004 $ c'
v4 4 0 pwl(0 '0.3 $ c' 1n 1)
r5 5 0 rval $ c 'x'
r6 6 0 'rval*2' $ it's a comment
+ tc1=0.004
.end
",
    );
    why(&net, "r1");
    why(&net, "x2.rc");
    let ep = net.resolve_element("r3").unwrap();
    assert!(unresolved(param(&ep, "tc1")).contains("`$` inside quotes"));
    let ep = net.resolve_element("v4").unwrap();
    assert!(
        unresolved(param(&ep, "dc")).contains("`$` inside quotes"),
        "{ep:?}"
    );
    assert_eq!(value(&net, "r5", "r"), 2000.0);
    assert_eq!(value(&net, "r6", "r"), 4000.0);
    assert_eq!(value(&net, "r6", "tc1"), 0.004);
    // one Error for each text with a `$` inside quotes, and none for a
    // `$` comment
    assert_eq!(
        error_lines(&net, "quoted_dollar"),
        ["main.sp:2", "main.sp:8", "main.sp:9", "main.sp:10"]
    );
    // `$` comments that hold a `'` (golden `dollar_quote_comment`)
    assert!(error_lines(&golden("dollar_quote_comment"), "quoted_dollar").is_empty());
}

/// The reason of a text with a line that HSPICE does not join to it: a
/// line inside quotes that is not a `+` line, or a `*` comment line inside
/// open parentheses (see `line_breaks_in_quotes`).
fn line_break_why(net: &Netlist, path: &str, name: &str) {
    let ep = net.resolve_element(path).unwrap();
    let why = unresolved(param(&ep, name)).to_owned();
    assert!(
        why.contains("a line inside quotes that is not a `+` line")
            && why.contains("`*` comment line inside open parentheses"),
        "{path}: {why}"
    );
    assert!(why.contains("rejects the deck"), "{path}: {why}");
}

/// A `*` comment line inside quotes: HSPICE rejects the deck (golden
/// `quoted_cmtline`, `elem_unquoted_cmtline`: "string misses dexter
/// quotation mark"). The value has no number, the reason says so, and the
/// loader gives the Error `quoted_comment_line`. Outside quotes a `+` line
/// after a `*` comment line goes on with the statement (golden
/// `plus_after_cmtline`).
#[test]
fn comment_line_in_quotes() {
    let why = line_break_why;
    let net = golden("quoted_cmtline");
    why(&net, "r7", "r");
    assert_eq!(error_lines(&net, "quoted_comment_line"), ["deck.sp:4"]);
    // the text keeps the comment line on a line of its own
    let ep = net.resolve_element("r7").unwrap();
    let chain: Vec<&str> = param(&ep, "r")
        .chain
        .iter()
        .map(|s| s.text.as_str())
        .collect();
    assert_eq!(chain, ["a\n* a comment line\n*5"]);
    assert_eq!(value(&net, "rc", "r"), 1000.0);
    // a `'` in the comment line: the parser rejects the line
    let net = golden("elem_unquoted_cmtline");
    assert_eq!(error_lines(&net, "trailing_text"), ["deck.sp:5"]);
    assert_eq!(value(&net, "re", "r"), 4000.0);
    // not probed: a `.param` value, a function body, a `.subckt` default,
    // an `X`-line override, a `key=value`, a quoted value that a later
    // `r=` replaces, a quoted TC1 and a quoted source argument
    let (_d, net) = load_text(
        "cmtline_more",
        "* t
.param a=2k
.param q='a
* c
+ *3'
.param f(x)='x
* c
+ *4k'
.subckt cell n1 n2 w='1k
* c
+ *6'
rc n1 n2 w
.ends
v1 1 0 1
r1 1 0 q
r2 1 0 'f(1)'
x1 1 0 cell
x2 1 0 cell w='a
* c
+ *7'
r3 1 0 1k tc1='0.004
* c
+ *1'
r5 1 0 'a
* c
+ *5' r=9k
r6 1 0 a '0.004
* c
+ *1' r=5k
r7 1 0 a '0.004
+ *1' r=5k
r8 1 0 'a
* it is
+ *2' $ it's
v9 9 0 PWL(0 '0.3
* c
+ *1' 1n 1)
.end
",
    );
    why(&net, "r1", "r");
    why(&net, "r2", "r");
    why(&net, "x1.rc", "r");
    why(&net, "x2.rc", "r");
    why(&net, "x2", "w");
    why(&net, "r3", "tc1");
    why(&net, "r5", "r");
    why(&net, "r6", "tc1");
    why(&net, "r8", "r");
    why(&net, "v9", "dc");
    assert_eq!(value(&net, "r3", "r"), 1000.0);
    assert_eq!(value(&net, "r6", "r"), 2000.0);
    // a quoted TC1 that goes on on a `+` line, with no comment line
    assert_eq!(value(&net, "r7", "tc1"), 0.004);
    assert_eq!(
        error_lines(&net, "quoted_comment_line"),
        [
            "main.sp:3",
            "main.sp:6",
            "main.sp:9",
            "main.sp:18",
            "main.sp:21",
            "main.sp:24",
            "main.sp:27",
            "main.sp:32",
            "main.sp:35"
        ]
    );
    assert_eq!(net.summary().errors, 9);
    // outside quotes, the `+` line after a comment line goes on with the
    // statement (golden `plus_after_cmtline`); a ` *` in an unquoted
    // `.param` value ends it (golden `unquoted_space`,
    // `unquoted_space_val`)
    let net = golden("plus_after_cmtline");
    assert_eq!(net.summary().errors, 0);
    assert_eq!(value(&net, "rp", "r"), 4000.0);
    let net = golden("unquoted_space");
    // HSPICE rejects the deck: `rc2` names `c2`
    assert_eq!(error_lines(&net, "undefined_param"), ["deck.sp:10"]);
    assert_eq!(net.summary().errors, 1);
    assert_eq!(value(&net, "rb2", "r"), 2000.0);
    let ep = net.resolve_element("rc2").unwrap();
    assert!(unresolved(param(&ep, "r")).contains("`c2`"), "{ep:?}");
}

/// Line breaks inside quotes and parentheses (the 8b-G probes). Inside
/// quotes, a line that is not a `+` line makes HSPICE reject the deck: a
/// blank line, a line with only spaces and a tab, and a `$` comment line
/// (golden `quoted_blank_line`, `quoted_blank_param`,
/// `quoted_spaces_param`, `quoted_spaces_elem`, `quoted_dollarline_param`,
/// `quoted_dollarline_elem`), as a `*` comment line does
/// (`quoted_cmtline`). The value has no number, the reason says so, and the
/// loader gives the Error `quoted_comment_line`. A `+` line with no text
/// there is passed over (`quoted_plus_empty_*`). Inside open parentheses
/// with no quotes, a blank line is passed over and a `*` comment line makes
/// HSPICE reject the deck (`paren_blank_call`, `paren_cmtline_call`: the
/// Error `paren_comment_line`), except in the argument list of a `.param`
/// function and in the parentheses of a source function
/// (`func_args_cmtline`, `paren_cmtline_pwl`).
#[test]
fn line_breaks_in_quotes() {
    for (case, path, line) in [
        ("quoted_blank_line", "r7", "deck.sp:4"),
        ("quoted_blank_param", "r1", "deck.sp:3"),
        ("quoted_spaces_param", "r1", "deck.sp:3"),
        ("quoted_spaces_elem", "r1", "deck.sp:4"),
        ("quoted_dollarline_param", "r1", "deck.sp:3"),
        ("quoted_dollarline_elem", "r1", "deck.sp:4"),
    ] {
        let net = golden(case);
        line_break_why(&net, path, "r");
        assert_eq!(error_lines(&net, "quoted_comment_line"), [line], "{case}");
        assert_eq!(net.summary().errors, 1, "{case}");
        assert_eq!(value(&net, "rc", "r"), 1000.0, "{case}");
    }
    let net = golden("paren_cmtline_call");
    line_break_why(&net, "rp", "r");
    assert_eq!(error_lines(&net, "paren_comment_line"), ["deck.sp:2"]);
    assert_eq!(net.summary().errors, 1);
    // the forms that HSPICE accepts load with no Error (their values are
    // in `golden_all_resistors` and `golden_sources`)
    for case in [
        "quoted_plus_empty_param",
        "quoted_plus_empty_elem",
        "quote_tail_param",
        "quote_tail_elem",
        "quoted_plus_pwl",
        "quoted_plus_tc",
        "kw_value_next_line",
        "paren_plus_call",
        "paren_blank_call",
        "paren_blank_args",
        "func_args_plus",
        "func_args_cmtline",
        "paren_plus_pwl",
        "paren_blank_pwl",
        "paren_cmtline_pwl",
        "plus_after_blank",
        "plus_after_cmtline",
    ] {
        let net = golden(case);
        let d = net.diagnostics(None, &Page::default()).unwrap();
        assert!(
            d.items.iter().all(|x| x.level != Level::Error),
            "{case}: {:?}",
            d.items
        );
    }
    // the PWL points of a quoted argument on a `+` line (golden
    // `quoted_plus_pwl`): the text on one line, with the quoted value whole
    let net = golden("quoted_plus_pwl");
    for (name, want) in [("v1", "0 '0.3 *1' 1n 1"), ("v2", "0 '0.3 *2' 1n 1")] {
        let ep = net.resolve_element(name).unwrap();
        let f = param(&ep, "pwl");
        assert_eq!(f.chain[0].text, want, "{name}");
    }
    // a quoted TC2 on `+` lines (golden `quoted_plus_tc`)
    let net = golden("quoted_plus_tc");
    for (name, tc2) in [("r1", 1e-5), ("r2", 2e-5)] {
        approx(value(&net, name, "r"), 2000.0);
        approx(value(&net, name, "tc1"), 0.004);
        approx(value(&net, name, "tc2"), tc2);
    }
    // the text after the closing quote is not used, and it is not TC1
    // (golden `quote_tail_elem`: `tc1` 0)
    let ep = golden("quote_tail_elem").resolve_element("r1").unwrap();
    assert!(!has_param(&ep, "tc1") && !has_param(&ep, "tc2"), "{ep:?}");
    // the text of `r=` at the end of a line, with its value on the next
    // `+` line (golden `kw_value_next_line`)
    let ep = golden("kw_value_next_line").resolve_element("r1").unwrap();
    let r = param(&ep, "r");
    assert_eq!((r.chain[0].text.as_str(), r.value), ("5k", Some(5000.0)));
    // not probed: a blank line or a line with only spaces inside quotes in
    // a function body, a `.subckt` default, an `X`-line override, an `r=`
    // value, a quoted TC1 and a quoted source argument (the reviewer deck
    // of the seventh review); a `*` comment line inside parentheses in an
    // element value and a function body; a `$` comment line and a blank
    // line inside parentheses, which are passed over
    let (_d, net) = load_text(
        "blank_more",
        "* t
.param a=2k
.param f(x)='x*

+ 3k'
.subckt cell n1 n2 w='1k

+ *3'
rc n1 n2 w
.ends
v1 1 0 1
r2 1 0 'f(1)'
x1 1 0 cell
x2 1 0 cell w='a

+ *4'
r4 1 0 r='a

+ *5'
r5 1 0 'a
   \t
+ *6'
r6 1 0 a '0.004

+ *1' r=5k
v2 2 0 PWL(0 '0.3

+ *1' 1n 1)
r7 1 0 max(1k,
* c
+ 3k)
.param g(x)=max(x,
* c
+ 1)
r8 1 0 'g(1)*1k'
.param p=max(1k,
$ c

+ 3k)
r9 1 0 p
r10 1 0 max(1k,
+ 3k) tc1=
+ 0.004
.end
",
    );
    for (path, name) in [
        ("r2", "r"),
        ("x1.rc", "r"),
        ("x2.rc", "r"),
        ("x2", "w"),
        ("r4", "r"),
        ("r5", "r"),
        ("r6", "tc1"),
        ("v2", "dc"),
        ("r7", "r"),
        ("r8", "r"),
    ] {
        line_break_why(&net, path, name);
    }
    assert_eq!(value(&net, "r6", "r"), 2000.0);
    assert_eq!(value(&net, "r9", "r"), 3000.0);
    assert_eq!(value(&net, "r10", "r"), 3000.0);
    assert_eq!(value(&net, "r10", "tc1"), 0.004);
    assert_eq!(
        error_lines(&net, "quoted_comment_line"),
        [
            "main.sp:3",
            "main.sp:6",
            "main.sp:14",
            "main.sp:17",
            "main.sp:20",
            "main.sp:23",
            "main.sp:26"
        ]
    );
    assert_eq!(
        error_lines(&net, "paren_comment_line"),
        ["main.sp:29", "main.sp:32"]
    );
    assert_eq!(net.summary().errors, 9);
}

/// The other 8b-G probes and the probes of the seventh review that agree
/// with the tool: TNOM, `dtemp=` and `scale=` with a TC (golden
/// `tc_tnom_dtemp`), the `.op` value of a swept parameter (`sweep_op`),
/// and a body `.param` that names itself under GLOBAL
/// (`body_self_ref_global`).
#[test]
fn golden_tc_and_sweep_probes() {
    let net = golden("tc_tnom_dtemp");
    let exp = expected(&repo("golden/cases/tc_tnom_dtemp"));
    for (name, tc) in exp["extra"]["tc"].as_object().unwrap() {
        let ep = net.resolve_element(name).unwrap();
        for (k, w) in tc.as_object().unwrap() {
            approx(param(&ep, k).value.unwrap(), w.as_f64().unwrap());
        }
        let op = exp["resistors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["path"] == name.as_str())
            .unwrap()["value"]
            .as_f64()
            .unwrap();
        let r = param(&ep, "r");
        if name == "r6" {
            // `dtemp=-75` puts the element at TNOM: the `.op` value is the
            // value as written
            approx(op, 1000.0);
            assert!(!caveats_hold(r, "TC1/TC2"), "{:?}", r.caveats);
            continue;
        }
        // the `.op` value in the caveat (the listing rounds it to 5 digits)
        let adjusted = r.caveats.iter().find_map(|c| {
            let c = c.strip_prefix("TC1/TC2 are set")?;
            assert!(
                c.contains("TNOM 50") && c.contains("`tc_tnom_dtemp`"),
                "{c}"
            );
            let (_, rest) = c.split_once("dT^2) = ")?;
            rest.split(' ').next()?.parse::<f64>().ok()
        });
        match adjusted {
            Some(v) => approx(v, op),
            None => panic!("{name}: no caveat with the value {op}: {:?}", r.caveats),
        }
    }
    let ep = net.resolve_element("r2").unwrap();
    assert!(caveats_hold(param(&ep, "r"), "125 + DTEMP 20"));
    // the `.op` value of a swept parameter is the deck value
    let net = golden("sweep_op");
    for (name, want, via) in [
        ("r1", 2000.0, "`pv` is swept by .dc"),
        ("r2", 3000.0, "`pd` is swept by .tran"),
        ("r3", 4000.0, "`pv` is swept by .dc"),
    ] {
        let ep = net.resolve_element(name).unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(want), "{name}");
        assert!(
            r.caveats.iter().any(|c| c.contains(via)
                && c.contains("which is the `.op` value (golden `sweep_op`")
                && !c.contains("not probed")
                && !c.contains("before the sweep")),
            "{name}: {:?}",
            r.caveats
        );
    }
    let net = golden("body_self_ref_global");
    assert_eq!(net.summary().errors, 0);
    assert_eq!(value(&net, "x1.r1", "r"), 4.0);
    assert_eq!(value(&net, "x2.r1", "r"), 4.0);
}

/// Small rules of the answers: a value out of range after the element
/// `scale=` or in a PWL has no number; a long expression that does not
/// parse is quoted in part; the function of a source with a DC value says
/// that `.op` uses the DC value; an `.alter` `.param` at the top level
/// gives no caveat on a definition below the top level that wins under
/// LOCAL; an undefined name says that HSPICE rejects the deck (golden
/// `pe_scope_global`).
#[test]
fn answer_details() {
    let long = vec!["q"; 400].join("+(");
    let (_d, net) = load_text(
        "details",
        &format!(
            "* t\nv1 1 0 1\nr1 1 0 1e300 scale=1e10\nv2 2 0 PWL(0 -1e308 1n 1e308) TD=-0.5n\n\
             r3 1 0 '{long}'\nv4 4 0 DC 1 SIN(0 1 1meg)\n.end\n"
        ),
    );
    let ep = net.resolve_element("r1").unwrap();
    assert!(unresolved(param(&ep, "r")).contains("out of the range"));
    let ep = net.resolve_element("v2").unwrap();
    assert!(unresolved(param(&ep, "dc")).contains("out of the range"));
    let ep = net.resolve_element("r3").unwrap();
    let why = unresolved(param(&ep, "r"));
    assert!(why.len() < 400 && why.contains('…'), "{why}");
    let ep = net.resolve_element("v4").unwrap();
    assert_eq!(param(&ep, "dc").value, Some(1.0));
    assert!(unresolved(param(&ep, "sin")).contains("DC value of the line"));
    for (mode, alter) in [("local", false), ("global", true)] {
        let (_d, net) = load_text(
            &format!("alter_{mode}"),
            &format!(
                "* t\n.option parhier={mode}\nv1 1 0 1\n\
                 .subckt c a b\n.param a=2\nr1 a b a\n.ends\nx1 1 0 c\n.op\n\
                 .alter\n.param a=3\n.end\n"
            ),
        );
        let ep = net.resolve_element("x1.r1").unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(2.0));
        assert_eq!(
            caveats_hold(r, ".alter #1"),
            alter,
            "{mode}: {:?}",
            r.caveats
        );
    }
    let ep = golden("pe_scope_global").resolve_element("x1.r1").unwrap();
    let why = unresolved(param(&ep, "r"));
    assert!(why.contains("`b`") && why.contains("rejects"), "{why}");
    // a function that only the calling subckt defines, called in the body
    // of a top-level function (golden `func_body_undef_global`)
    let ep = golden("func_body_undef_global")
        .resolve_element("x1.r1")
        .unwrap();
    let why = unresolved(param(&ep, "r"));
    assert!(why.contains("`hq`") && why.contains("rejects"), "{why}");
    // the name `h` (golden `func_nested_global`): HSPICE reads `h(...)` as
    // an output variable, so the reason hedges
    let ep = golden("func_nested_global")
        .resolve_element("x1.r1")
        .unwrap();
    let why = unresolved(param(&ep, "r"));
    assert!(
        why.contains("`h`")
            && why.contains("rejects this deck or reads the name as an output variable"),
        "{why}"
    );
    // an overflow names the numbers in short form
    let (_d, net) = load_text(
        "overflow",
        "* t\n.param big=1e308\nv1 1 0 1\nr1 1 0 'big*big'\n.end\n",
    );
    let ep = net.resolve_element("r1").unwrap();
    let why = unresolved(param(&ep, "r"));
    assert!(why.len() < 100 && why.contains("1e308"), "{why}");
}

/// A call in a function body takes the nearest definition, with no
/// creation-order rule (golden `sticky_lazy_local`: x5.xc.rh, x7.xc.rk).
/// The other forms of the probe: a body `.param` that the `X` line of the
/// first instance overrides does not count (x1.xl.rp), a body `.param` or
/// a ternary branch that the first instance does not use counts (x9.xl.ru,
/// x3.xl.rq), and an argument hides a parameter of the same name (r11,
/// x10.ry, x12.rz; golden `func_global_more` under GLOBAL).
#[test]
fn function_body_nearest() {
    let net = golden("sticky_lazy_local");
    for (path, want, first) in [
        ("x5.xc.rh", 3000.0, None),
        ("x7.xc.rk", 3000.0, None),
        ("x1.xl.rp", 3000.0, None),
        ("x3.xl.rq", 2000.0, Some("`x2.xl`")),
        ("x9.xl.ru", 2000.0, Some("`x8.xl`")),
        ("r11", 3000.0, None),
        ("x10.ry", 3000.0, None),
        ("x12.rz", 5000.0, None),
    ] {
        let ep = net.resolve_element(path).unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(want), "{path}");
        match first {
            Some(f) => assert!(caveats_hold(r, f), "{path}: {:?}", r.caveats),
            None => assert!(r.caveats.is_empty(), "{path}: {:?}", r.caveats),
        }
    }
    // the body of `h` uses `cf3`'s `f`: a chain step of `f` in `x5`
    let ep = net.resolve_element("x5.xc.rh").unwrap();
    let f: Vec<&str> = param(&ep, "r")
        .chain
        .iter()
        .filter(|s| s.kind == StepKind::ParamFunction && s.name == "f")
        .map(|s| s.scope.as_str())
        .collect();
    assert_eq!(f, ["x5"]);
    let net = golden("func_global_more");
    for (path, want) in [
        ("x1.rk", 9000.0),
        ("r2", 3000.0),
        ("x3.ry", 5000.0),
        ("x4.xc.rc", 3000.0),
        ("x5.xh.xk.rh", 3000.0),
        ("x6.xm.xk.rq", 4000.0),
    ] {
        assert_eq!(value(&net, path, "r"), want, "{path}");
    }
}

/// A `.param` function with the name of a built-in function hides it where
/// it is visible, and the built-in is used elsewhere (golden
/// `func_shadow_top`, `func_shadow_sub`, `func_shadow_scope`). The instance
/// that HSPICE creates first among those that evaluate a text decides the
/// kind of function for the later ones: x2.xk.rk uses the built-in after
/// x4.rk did, and x8.xk.rk2, which finds no `.param` `sgn` after x7.xk.rk2
/// used one, makes HSPICE reject the deck (golden `func_shadow_more`,
/// `func_shadow_more_local`; x7.xk.rk2 gets a caveat). A call in a function
/// body takes the nearest `.param` function, else the built-in, with no
/// such rule (golden `builtin_body_global`, `builtin_body_global_rev`,
/// `builtin_local`: (4)), and with a top-level `.param` function of the
/// name, the rule of `func_sticky_local` applies under LOCAL
/// (`builtin_local`: (1), (2)).
#[test]
fn builtin_function_names() {
    assert_eq!(value(&golden("func_shadow_top"), "r1", "r"), 3000.0);
    assert_eq!(value(&golden("func_shadow_sub"), "x1.rs", "r"), 7000.0);
    for case in [
        "func_shadow_scope",
        "func_shadow_more",
        "func_shadow_more_local",
    ] {
        let net = golden(case);
        for (path, want) in [
            ("x1.rm", 16000.0),
            ("x2.rs", 7000.0),
            ("r3", 1000.0),
            ("x4.rk", 1000.0),
            ("r5", 4000.0),
            ("x6.rd", 8000.0),
        ] {
            let ep = net.resolve_element(path).unwrap();
            let r = param(&ep, "r");
            assert_eq!(r.value, Some(want), "{case}: {path}");
            assert!(r.caveats.is_empty(), "{case}: {path}: {:?}", r.caveats);
        }
        let ep = net.resolve_element("x2.xk.rk").unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(1000.0), "{case}");
        assert!(caveats_hold(r, "`x4`"), "{case}: {:?}", r.caveats);
        if case == "func_shadow_scope" {
            continue;
        }
        // x7.xk.rk2 keeps its value, with a caveat: a later instance of
        // the text makes HSPICE reject the deck
        let ep = net.resolve_element("x7.xk.rk2").unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(7000.0), "{case}");
        assert!(
            caveats_hold(r, "HSPICE rejects this deck") && caveats_hold(r, "`x8.xk`"),
            "{case}: {:?}",
            r.caveats
        );
        let ep = net.resolve_element("x8.xk.rk2").unwrap();
        let why = unresolved(param(&ep, "r"));
        assert!(
            why.contains("rejects") && why.contains("`x7.xk`"),
            "{case}: {why}"
        );
    }
    // with no `.param` function of the name anywhere, the built-in; in a
    // function body, the nearest `.param` function, else the built-in
    let (_d, net) = load_text(
        "builtin_body",
        "* t\n.param g(x)='max(x,3)*1k'\nv1 1 0 1\nr1 1 0 'max(1,2)*1k'\n\
         .subckt cm a b\n.param max(a,b)='a+b'\nrm a b 'g(1)'\n.ends\nx1 1 0 cm\n.end\n",
    );
    assert_eq!(value(&net, "r1", "r"), 2000.0);
    assert_eq!(value(&net, "x1.rm", "r"), 3000.0);
    for (case, path, want, caveat) in [
        ("builtin_body_global", "x1.xc.rh", 1000.0, None),
        ("builtin_body_global", "x2.xc.rh", 7000.0, None),
        ("builtin_body_global", "x3.r3", 1000.0, None),
        ("builtin_body_global", "r4", 4000.0, None),
        ("builtin_body_global", "r5", 3000.0, None),
        ("builtin_body_global", "x7.rd", 12000.0, None),
        ("builtin_body_global_rev", "x1.xc.rh", 7000.0, None),
        ("builtin_body_global_rev", "x2.xc.rh", 1000.0, None),
        ("builtin_local", "x2.xk.rk", 2000.0, Some("`x1.xk`")),
        ("builtin_local", "x3.xk.rk2", 7000.0, None),
        ("builtin_local", "x4.xk.rk2", 2000.0, None),
        ("builtin_local", "x6.xk.ra", 1000.0, Some("`x5`")),
        ("builtin_local", "x7.xc.rh", 1000.0, None),
        ("builtin_local", "x8.xc.rh", 6000.0, None),
    ] {
        let ep = golden(case).resolve_element(path).unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(want), "{case}: {path}");
        match caveat {
            Some(c) => assert!(caveats_hold(r, c), "{case}: {path}: {:?}", r.caveats),
            None => assert!(r.caveats.is_empty(), "{case}: {path}: {:?}", r.caveats),
        }
    }
}

/// A name that a first-run analysis sweeps (`.dc`, a `sweep` of `.tran`,
/// `.ac` or `.noise`, a `.data` block that they name) gives a caveat on an
/// answer that uses it: the value is the nominal one (spec 6.2a). Not
/// probed. HSPICE P-2019.06 ignores `.step` with a warning (golden
/// `step_param`, `step_plain`, `step_lin`), so it sweeps nothing.
#[test]
fn sweep_caveat() {
    let (_d, net) = load_text(
        "sweep",
        "* t\n.param rval=2k rb=1k rc=3k rd=4k\nv1 1 0 1\nv2 2 0 2\n\
         r1 1 0 'rval*2'\nr2 1 0 rb\nr3 1 0 rc\nr4 1 0 rd\nr5 1 0 1k tc1=0.001\n\
         .subckt s a b rval=7\nrs a b rval\n.ends\nx1 1 0 s\n\
         .dc rval 1k 3k 1k v2 0 1 0.5\n\
         .data d1 rb\n+ 1k\n+ 2k\n.enddata\n\
         .tran 1n 10n sweep data=d1\n.ac dec 10 1 1g sweep temp -40 125 5\n.op\n\
         .step param re 1 2 1\nr6 1 0 re\n.param re=5\n\
         .alter\n.dc rd 1 2 1\n.end\n",
    );
    let ep = net.resolve_element("r6").unwrap();
    let r = param(&ep, "r");
    assert!(!caveats_hold(r, "swept"), "{:?}", r.caveats);
    for case in ["step_param", "step_plain", "step_lin"] {
        let net = golden(case);
        let ep = net.resolve_element("r1").unwrap();
        let r = param(&ep, "r");
        assert_eq!(r.value, Some(2000.0));
        assert!(!caveats_hold(r, "swept"), "{case}: {:?}", r.caveats);
        let s = net.summary();
        assert_eq!(s.errors, 0, "{case}");
        let d = net.diagnostics(None, &Page::default()).unwrap();
        assert!(
            d.items.iter().any(|x| x.code == "unknown_command"
                && x.level == Level::Warning
                && x.message.contains("ignores it with a warning")),
            "{case}: {:?}",
            d.items
        );
    }
    // the second variable of `.dc` after a typed first sweep (`lin`,
    // `dec`, `oct`, `poi`), after `start=`/`stop=`/`step=`, and after
    // values that are names
    let (_d, two) = load_text(
        "sweep_two",
        "* t\n.param p1=1 p2=2 p3=3 p4=4 p5=5 p6=6 p7=7 vmax=1\nv1 1 0 1\n\
         r1 1 0 p1\nr2 1 0 p2\nr3 1 0 p3\nr4 1 0 p4\nr5 1 0 p5\nr6 1 0 p6\nr7 1 0 p7\n\
         .dc v1 lin 11 0 1 p1 lin 3 1k 3k\n.dc v1 dec 10 1 100 p2 0 1 0.5\n\
         .dc v1 oct 2 1 4 p3 oct 2 1 4\n.dc v1 poi 3 0 0.5 1 p4 poi 2 1 2\n\
         .dc v1 start=0 stop=1 step=0.1 p5 0 1 0.5\n.dc v1 0 vmax 0.1 p6 0 1 0.5\n\
         .end\n",
    );
    for path in ["r1", "r2", "r3", "r4", "r5", "r6"] {
        let ep = two.resolve_element(path).unwrap();
        let r = param(&ep, "r");
        let p = format!("`p{}` is swept by .dc", &path[1..]);
        assert!(caveats_hold(r, &p), "{path}: {:?}", r.caveats);
    }
    let ep = two.resolve_element("r7").unwrap();
    assert!(!caveats_hold(param(&ep, "r"), "swept"));
    // the values of a sweep are not swept names
    let ep = two.resolve_element("v1").unwrap();
    assert!(caveats_hold(param(&ep, "dc"), "`v1` is swept by .dc"));
    let r = two.resolve_param("", "vmax").unwrap();
    assert!(!caveats_hold(&r, "swept"), "{:?}", r.caveats);
    let ep = net.resolve_element("r1").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(4000.0));
    assert!(
        caveats_hold(r, "`rval` is swept by .dc at") && caveats_hold(r, "nominal value"),
        "{:?}",
        r.caveats
    );
    let ep = net.resolve_element("r2").unwrap();
    let r = param(&ep, "r");
    assert!(
        caveats_hold(r, "swept by .tran") && caveats_hold(r, "`.data` block `d1`"),
        "{:?}",
        r.caveats
    );
    // not swept: `rc`; `rd` only in an `.alter` block
    for path in ["r3", "r4"] {
        let ep = net.resolve_element(path).unwrap();
        let r = param(&ep, "r");
        assert!(!caveats_hold(r, "swept"), "{path}: {:?}", r.caveats);
    }
    // under GLOBAL the swept top-level `rval` beats the subckt default;
    // under LOCAL the default wins, and it is not swept
    let ep = net.resolve_element("x1.rs").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(2000.0));
    assert!(caveats_hold(r, "`rval` is swept"), "{:?}", r.caveats);
    let (_d, local) = load_text(
        "sweep_local",
        "* t\n.option parhier=local\n.param rval=2k\nv1 1 0 1\n\
         .subckt s a b rval=7\nrs a b rval\n.ends\nx1 1 0 s\n.dc rval 1k 3k 1k\n.end\n",
    );
    let ep = local.resolve_element("x1.rs").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some(7.0));
    assert!(!caveats_hold(r, "swept"), "{:?}", r.caveats);
    // a swept source and a swept temperature
    let ep = net.resolve_element("v2").unwrap();
    assert!(caveats_hold(param(&ep, "dc"), "`v2` is swept by .dc"));
    let ep = net.resolve_element("v1").unwrap();
    assert!(!caveats_hold(param(&ep, "dc"), "swept"));
    let ep = net.resolve_element("r5").unwrap();
    assert!(
        caveats_hold(param(&ep, "r"), "`temp` is swept by .ac"),
        "{:?}",
        param(&ep, "r").caveats
    );
    let r = net.resolve_param("", "rval").unwrap();
    assert!(caveats_hold(&r, "swept by .dc"), "{:?}", r.caveats);
    // under GLOBAL a swept top-level `.param` beats an `X`-line override:
    // the override's caveat gives the value that the instance uses, and
    // the sweep of that value
    let (_d, ovr) = load_text(
        "sweep_ovr",
        "* t\n.param p=1k\nv1 1 0 1\n.subckt cell a b p=1\nrc a b p\n.ends\n\
         x2 1 0 cell p=9\n.dc p 1k 2k 100\n.end\n",
    );
    let ep = ovr.resolve_element("x2").unwrap();
    let p = param(&ep, "p");
    assert_eq!(p.value, Some(9.0));
    assert!(
        caveats_hold(p, "the instance uses p = 1000") && caveats_hold(p, "`p` is swept by .dc"),
        "{:?}",
        p.caveats
    );
}

/// The caveats of one answer are capped at 50; the last one counts the
/// others.
#[test]
fn caveats_are_capped() {
    let n = 60;
    let mut text = String::from("* t\nv1 1 0 1\n");
    for i in 1..=n {
        text.push_str(&format!(".param p{i}={i}\n.dc p{i} 0 1 0.5\n"));
    }
    let sum: Vec<String> = (1..=n).map(|i| format!("p{i}")).collect();
    text.push_str(&format!("r1 1 0 '{}'\n.end\n", sum.join("+")));
    let (_d, net) = load_text("caveats_capped", &text);
    let ep = net.resolve_element("r1").unwrap();
    let r = param(&ep, "r");
    assert_eq!(r.value, Some((n * (n + 1) / 2) as f64));
    assert_eq!(r.caveats.len(), 50, "{:?}", r.caveats);
    assert!(
        r.caveats[49].starts_with("11 more caveats are not listed"),
        "{:?}",
        r.caveats[49]
    );
    // one name: one caveat
    assert_eq!(net.resolve_param("", "p1").unwrap().caveats.len(), 1);
}
