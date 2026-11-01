//! The speed and memory gate (plan Task 13). Release only, run on purpose:
//!
//! ```sh
//! timeout 180 cargo test --release -p netlist-query --test perf -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! The decks come from `query/benches/gen_big.py` (python3). Peak memory is
//! measured in a child process for each deck (`perf_child`), so that the two
//! decks do not share one peak.

use netlist_query::{LoadOptions, NetView, Netlist, Page, SubcktSort};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const CHILD_ENV: &str = "NETLIST_PERF_DECK";

/// The deck of `gen_big.py` with `levels` more levels of groups (`unused`:
/// the top calls the cells, not the groups). The file
/// name has a hash of the script, so that an edit of the script makes a
/// new deck, and the deck is written under a temporary name first, so that
/// a run stopped while it writes leaves no cut deck.
fn deck(levels: u32, unused: bool) -> PathBuf {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/gen_big.py");
    let mut h = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(&std::fs::read(&script).unwrap(), &mut h);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("perf");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!(
        "big_l{levels}{}_{:016x}.sp",
        if unused { "u" } else { "" },
        std::hash::Hasher::finish(&h)
    ));
    if !path.exists() {
        let part = path.with_extension("part");
        let ok = Command::new("python3")
            .arg(&script)
            .arg(&part)
            .args(["--levels", &levels.to_string()])
            .args(unused.then_some("--unused"))
            .status()
            .unwrap()
            .success();
        assert!(ok, "gen_big.py failed");
        std::fs::rename(&part, &path).unwrap();
    }
    path
}

/// The peak resident memory of this process, in bytes.
fn peak_rss() -> u64 {
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `u` is a valid `rusage` for `getrusage` to fill.
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) }, 0);
    let max = u.ru_maxrss as u64;
    // bytes on macOS, KiB on Linux
    if cfg!(target_os = "macos") {
        max
    } else {
        max * 1024
    }
}

/// Loads `path` in a child process; its peak RSS in bytes.
fn child_rss(path: &Path) -> u64 {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "perf_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, path)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|l| {
            l.split("PERF_RSS ")
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .unwrap_or_else(|| panic!("no PERF_RSS line: {text}"))
}

#[test]
#[ignore]
fn perf_child() {
    let Ok(path) = std::env::var(CHILD_ENV) else {
        return;
    };
    let net = Netlist::load(&LoadOptions::new(PathBuf::from(path))).unwrap();
    assert!(net.summary().expanded_devices.is_some());
    println!("PERF_RSS {}", peak_rss());
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let t = Instant::now();
    let v = f();
    (v, t.elapsed())
}

#[test]
#[ignore]
fn perf_gate() {
    if cfg!(debug_assertions) {
        panic!("the perf gate runs in release: cargo test --release ...");
    }
    let base = deck(0, false);
    let ten = deck(1, false);
    let deep = deck(2, false);
    let control = deck(2, true);
    let (net, load) = timed(|| Netlist::load(&LoadOptions::new(base.clone())).unwrap());
    let s = net.summary();
    assert_eq!(s.expanded_instances, Some(10_000));
    assert_eq!(
        s.expanded_devices.as_ref().unwrap().values().sum::<u64>(),
        1_020_000
    );
    let page = Page::default();
    let (rows, t_list) = timed(|| net.list_subckts(None, SubcktSort::Copies, &page).unwrap());
    assert_eq!(rows.total, 1000);
    let (ep, t_resolve) = timed(|| net.resolve_element("x5_3.m7").unwrap());
    // parhier GLOBAL (the default): the top-level `.param wn=1u` wins over
    // the `X` line's `wn=2u`
    let w = ep.params.iter().find(|p| p.name.eq_ignore_ascii_case("w"));
    assert!(
        w.and_then(|p| p.value)
            .is_some_and(|v| (v - 1e-6).abs() < 1e-12),
        "{w:?}"
    );
    let (ni, t_net) = timed(|| net.net_info("x5_3", "n4", NetView::Logical, &page).unwrap());
    assert_eq!(ni.total, 3);
    let base_rss = child_rss(&base);
    let ten_rss = child_rss(&ten);
    let deep_rss = child_rss(&deep);
    let control_rss = child_rss(&control);
    for (path, instances) in [(&ten, 110_000), (&deep, 1_110_000), (&control, 10_000)] {
        let net = Netlist::load(&LoadOptions::new(path.clone())).unwrap();
        assert_eq!(
            net.summary().expanded_instances,
            Some(instances),
            "{path:?}"
        );
    }
    let mb = |b: u64| b as f64 / 1e6;
    println!(
        "PERF load {:.3} s; list_subckts {:.3} ms; resolve_element {:.3} ms; net_info {:.3} ms; peak RSS base {:.0} MB, 10x instances {:.0} MB ({:.2}x); 111x instances {:.0} MB vs the same lines unused {:.0} MB ({:.2}x)",
        load.as_secs_f64(),
        t_list.as_secs_f64() * 1e3,
        t_resolve.as_secs_f64() * 1e3,
        t_net.as_secs_f64() * 1e3,
        mb(base_rss),
        mb(ten_rss),
        ten_rss as f64 / base_rss as f64,
        mb(deep_rss),
        mb(control_rss),
        deep_rss as f64 / control_rss as f64
    );
    assert!(load < Duration::from_secs(1), "load {load:?}");
    for (what, t) in [
        ("list_subckts", t_list),
        ("resolve_element", t_resolve),
        ("net_info", t_net),
    ] {
        assert!(t < Duration::from_millis(10), "{what} {t:?}");
    }
    // the plan's gate: one more level (10x the instances, about 10% more
    // lines) needs less than 1.3x the memory
    assert!(
        (ten_rss as f64) < 1.3 * base_rss as f64,
        "memory scales with the expanded count: {base_rss} -> {ten_rss}"
    );
    // the same lines with 1.1M expanded instances and with 10,000: the
    // memory is the memory of the lines
    assert!(
        (deep_rss as f64) < 1.1 * control_rss as f64,
        "memory scales with the expanded count: {control_rss} -> {deep_rss}"
    );
}
