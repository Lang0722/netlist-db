use crate::{LoadOptions, Netlist};

/// Loads `text` as the deck of test `name` (in a temporary directory,
/// removed after the load).
fn load_text(name: &str, text: &str) -> Netlist {
    let dir = std::env::temp_dir().join(format!(
        "netlist-query-params-{}-{name}",
        std::process::id()
    ));
    _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let main = dir.join("main.sp");
    std::fs::write(&main, text).unwrap();
    let net = Netlist::load(&LoadOptions::new(main));
    _ = std::fs::remove_dir_all(&dir);
    net.unwrap()
}

/// The sum of `terms`, as a balanced tree of `+` (so that the
/// expression stays shallow).
fn sum(terms: &[String]) -> String {
    match terms {
        [] => "0".to_owned(),
        [one] => one.clone(),
        _ => {
            let (a, b) = terms.split_at(terms.len() / 2);
            format!("({}+{})", sum(a), sum(b))
        }
    }
}

/// The value and the work of `resolve_element(path)` on `text`.
fn work(name: &str, text: &str, path: &str) -> (f64, u64) {
    let net = load_text(name, text);
    let (ep, work) = net.resolve_element_work(path).unwrap();
    let r = ep.params.iter().find(|p| p.name == "r").unwrap();
    (r.value.unwrap_or_else(|| panic!("{name}: {r:?}")), work)
}

/// `n` top-level functions, each called once in one element (the
/// review's `funcs_*`).
fn funcs(n: usize) -> String {
    let mut s = String::from("* t\nv1 1 0 1\n");
    for i in 0..n {
        s.push_str(&format!(".param f{i}(x)='x*2'\n"));
    }
    let calls: Vec<String> = (0..n).map(|i| format!("f{i}(1)")).collect();
    s.push_str(&format!("r1 1 0 '{}'\n.end\n", sum(&calls)));
    s
}

/// `n` top-level functions, and `f0` called `n` times with different
/// arguments (the review's `funcs_same_*`).
fn funcs_same(n: usize) -> String {
    let mut s = String::from("* t\nv1 1 0 1\n");
    for i in 0..n {
        s.push_str(&format!(".param f{i}(x)='x*2'\n"));
    }
    let calls: Vec<String> = (0..n).map(|i| format!("f0({i})")).collect();
    s.push_str(&format!("r1 1 0 '{}'\n.end\n", sum(&calls)));
    s
}

/// Under LOCAL, a top-level `f`, `d` unused subckts that define `f`,
/// and `cf` (which defines `f`) holding `leaf`, whose element calls
/// `f(1)` `c` times (the review's `adv_*`), or whose `c` body `.param`
/// lines each call `f` (the review's `advmem_*`, `body`).
fn adv(d: usize, c: usize, body: bool) -> String {
    let mut s = String::from("* t\n.option parhier=local\n.param f(x)='x*2'\nv1 1 0 1\n");
    for i in 0..d {
        s.push_str(&format!(".subckt u{i} a b\n.param f(x)='x*5'\n.ends\n"));
    }
    s.push_str(".subckt leaf a b\n");
    let terms: Vec<String> = if body {
        for i in 0..c {
            s.push_str(&format!(".param p{i}='f({i})'\n"));
        }
        (0..c).map(|i| format!("p{i}")).collect()
    } else {
        vec!["f(1)".to_owned(); c]
    };
    s.push_str(&format!(
            "r1 a b '{}'\n.ends\n.subckt cf a b\n.param f(x)='x*3'\nxl a b leaf\n.ends\nx1 1 0 cf\n.end\n",
            sum(&terms)
        ));
    s
}

/// One `resolve_element` call costs work linear in the number of
/// `.param` functions, calls and definitions:
/// doubling the deck at most about doubles the work, where a scan of
/// all functions at each call, or a copy of the list of the subckts
/// that define a function at each call, would make it 4 times as much.
#[test]
fn work_is_linear() {
    for (name, shape) in shapes() {
        let mut works = Vec::new();
        for n in [1000, 2000] {
            let (text, path, want) = shape(n);
            let (v, w) = work(&format!("{name}_{n}"), &text, path);
            assert_eq!(v, want, "{name} {n}");
            works.push(w);
        }
        let ratio = works[1] as f64 / works[0] as f64;
        assert!(ratio < 2.5, "{name}: work {works:?} (ratio {ratio})");
    }
}

type Shape = fn(usize) -> (String, &'static str, f64);

/// The decks of the cost tests: a name, and the deck of size `n` with
/// the path to resolve and its value.
fn shapes() -> [(&'static str, Shape); 4] {
    [
        ("funcs", |n| (funcs(n), "r1", 2.0 * n as f64)),
        ("funcs_same", |n| {
            let want = (0..n).map(|i| 2.0 * i as f64).sum();
            (funcs_same(n), "r1", want)
        }),
        ("adv", |n| (adv(n, n, false), "x1.xl.r1", 3.0 * n as f64)),
        ("advmem", |n| {
            let want = (0..n).map(|i| 3.0 * i as f64).sum();
            (adv(n, n, true), "x1.xl.r1", want)
        }),
    ]
}

/// The wall-clock companion of `work_is_linear`, for the costs that the
/// work counter does not count (building, hashing and copying memo
/// keys): 4 times the deck takes at most about 4 times as long, where
/// a memo key that holds the list of the subckts that define a function
/// would make it 16 times as long.
#[test]
#[ignore = "timing: run with --release -- --ignored"]
fn time_is_linear() {
    for (name, shape) in shapes() {
        let mut times = Vec::new();
        for n in [4000, 16000] {
            let (text, path, want) = shape(n);
            let net = load_text(&format!("time_{name}_{n}"), &text);
            let mut best = f64::MAX;
            for _ in 0..3 {
                let t = std::time::Instant::now();
                let ep = net.resolve_element(path).unwrap();
                best = best.min(t.elapsed().as_secs_f64());
                let r = ep.params.iter().find(|p| p.name == "r").unwrap();
                assert_eq!(r.value, Some(want), "{name} {n}");
            }
            times.push(best);
        }
        let ratio = times[1] / times[0];
        assert!(ratio < 8.0, "{name}: times {times:?} s (ratio {ratio})");
    }
}
