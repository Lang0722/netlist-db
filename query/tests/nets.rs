//! Connectivity: `net_info` and `trace_net` (plan Task 9).

use netlist_query::{LoadOptions, NetInfo, NetView, Netlist, Page, QueryError, TraceDir};
use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// A deck written by `load_text`; its directory is removed on drop.
struct Deck {
    dir: PathBuf,
}

impl Drop for Deck {
    fn drop(&mut self) {
        _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn load(path: impl AsRef<Path>) -> Netlist {
    Netlist::load(&LoadOptions::new(path.as_ref().to_path_buf())).unwrap()
}

fn load_text(name: &str, text: &str) -> (Deck, Netlist) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("nets")
        .join(format!("{name}-{}", std::process::id()));
    _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let main = dir.join("main.sp");
    std::fs::write(&main, text).unwrap();
    let net = load(&main);
    (Deck { dir }, net)
}

fn all() -> Page {
    Page {
        limit: 500,
        cursor: None,
    }
}

fn info(net: &Netlist, scope: &str, name: &str) -> NetInfo {
    net.net_info(scope, name, NetView::Logical, &all())
        .unwrap_or_else(|e| panic!("net_info({scope:?}, {name:?}): {e}"))
}

/// (element, pin role) of each connection.
fn pins(i: &NetInfo) -> Vec<(String, String)> {
    i.connections
        .items
        .iter()
        .map(|c| (c.element.to_ascii_lowercase(), c.pin_role.clone()))
        .collect()
}

fn owned(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

const BUF: &str = "* buffers\n\
.subckt inv in out vdd vss\n\
m1 out in vdd vdd pch w=2u l=0.1u\n\
m2 out in vss vss nch w=1u l=0.1u\n\
.ends\n\
.subckt buf a y vdd vss\n\
x1 a mid vdd vss inv\n\
x2 mid y vdd vss inv\n\
.ends\n\
vdd vdd 0 1\n\
xb1 n1 n2 vdd 0 buf\n\
xb2 n2 n3 vdd 0 buf\n\
rin n1 0 1k\n";

#[test]
fn net_info_inverter() {
    let (_deck, net) = load_text("inverter", BUF);
    let out = info(&net, "xb1.x1", "out");
    assert_eq!(pins(&out), owned(&[("m1", "d"), ("m2", "d")]));
    assert_eq!((out.total, out.is_port), (2, true));
    assert_eq!((out.scope.as_str(), out.def.as_str()), ("xb1.x1", "inv"));
    assert!(!out.connectivity_incomplete);
    // the source and the bulk of each MOSFET; `vdd` is a port here
    let vdd = info(&net, "xb1.x1", "VDD");
    assert_eq!(pins(&vdd), owned(&[("m1", "s"), ("m1", "b")]));
    // an `X` line pin has the port name of the definition it binds to
    let mid = info(&net, "xb1", "mid");
    assert_eq!(pins(&mid), owned(&[("x1", "out"), ("x2", "in")]));
    assert!(!mid.is_port);
    assert_eq!(mid.connections.items[0].callee.as_deref(), Some("inv"));
    // the top level
    let n2 = info(&net, "", "n2");
    assert_eq!(pins(&n2), owned(&[("xb1", "y"), ("xb2", "a")]));
    let n1 = info(&net, "", "n1");
    assert_eq!(pins(&n1), owned(&[("xb1", "a"), ("rin", "1")]));
}

#[test]
fn trace_down_through_ports() {
    let (_deck, net) = load_text("trace_down", BUF);
    let t = net.trace_net("", "n1", TraceDir::Down, 5, &all()).unwrap();
    let hops: Vec<(String, String, String, String, u32)> = t
        .hops
        .items
        .iter()
        .map(|h| {
            assert_eq!(h.direction, "down");
            (
                h.from_scope.clone(),
                h.from_net.clone(),
                h.to_scope.clone(),
                h.to_net.clone(),
                h.depth,
            )
        })
        .collect();
    let s = |a: &str, b: &str, c: &str, d: &str, n: u32| {
        (a.to_owned(), b.to_owned(), c.to_owned(), d.to_owned(), n)
    };
    assert_eq!(
        hops,
        [s("", "n1", "xb1", "a", 1), s("xb1", "a", "xb1.x1", "in", 2)]
    );
    assert_eq!(t.hops.items[0].via_instance, "xb1");
    assert!(!t.truncated);
    // the depth bounds the trace
    let one = net.trace_net("", "n1", TraceDir::Down, 1, &all()).unwrap();
    assert_eq!(one.hops.total, 1);
    // a supply goes into every instance, one level per step
    let vdd = net.trace_net("", "vdd", TraceDir::Down, 2, &all()).unwrap();
    let to: Vec<&str> = vdd.hops.items.iter().map(|h| h.to_scope.as_str()).collect();
    assert_eq!(to, ["xb1", "xb2", "xb1.x1", "xb1.x2", "xb2.x1", "xb2.x2"]);
}

#[test]
fn trace_up_from_port() {
    let (_deck, net) = load_text("trace_up", BUF);
    let t = net
        .trace_net("xb1.x1", "in", TraceDir::Up, 5, &all())
        .unwrap();
    let hops: Vec<(String, String, String, String)> = t
        .hops
        .items
        .iter()
        .map(|h| {
            assert_eq!(h.direction, "up");
            (
                h.from_scope.clone(),
                h.from_net.clone(),
                h.to_scope.clone(),
                h.to_net.clone(),
            )
        })
        .collect();
    let s = |a: &str, b: &str, c: &str, d: &str| {
        (a.to_owned(), b.to_owned(), c.to_owned(), d.to_owned())
    };
    assert_eq!(
        hops,
        [s("xb1.x1", "in", "xb1", "a"), s("xb1", "a", "", "n1")]
    );
    // a net that is not a port does not go up
    let mid = net
        .trace_net("xb1", "mid", TraceDir::Up, 5, &all())
        .unwrap();
    assert_eq!(mid.hops.total, 0);
    // both ways: up to `n2` at the top, then down into `xb2`, never back
    let both = net
        .trace_net("xb1.x2", "out", TraceDir::Both, 3, &all())
        .unwrap();
    let got: Vec<(String, String, String)> = both
        .hops
        .items
        .iter()
        .map(|h| (h.direction.clone(), h.to_scope.clone(), h.to_net.clone()))
        .collect();
    let s = |a: &str, b: &str, c: &str| (a.to_owned(), b.to_owned(), c.to_owned());
    assert_eq!(
        got,
        [
            s("up", "xb1", "y"),
            s("up", "", "n2"),
            s("down", "xb2", "a"),
        ]
    );
}

#[test]
fn global_and_ground() {
    // golden `ground_alias`: `0`, `gnd`, `gnd!`, `ground`, `GND` are one net
    let net = load(repo("golden/cases/ground_alias/deck.sp"));
    for name in ["0", "gnd", "GND", "gnd!", "ground"] {
        let g = info(&net, "", name);
        assert!(g.is_ground, "{name}");
        assert_eq!(g.total, 6, "{name}: {:?}", pins(&g));
        assert!(g.caveats.iter().any(|c| c.contains("every level")));
    }
    let sub = info(&net, "xg", "0");
    assert_eq!(pins(&sub), owned(&[("rs", "2")]));
    // ground is not traced through ports
    let t = net.trace_net("", "0", TraceDir::Both, 4, &all()).unwrap();
    assert_eq!(t.hops.total, 0);
    assert!(
        t.caveats.iter().any(|c| c.contains("ground")),
        "{:?}",
        t.caveats
    );
    // golden `global_net`: `.global vdd` is the same net inside `load`,
    // which has no `vdd` port; `vss` is not global
    let net = load(repo("golden/cases/global_net/deck.sp"));
    let inside = info(&net, "x1", "vdd");
    assert!(inside.is_global && !inside.is_port);
    assert_eq!(pins(&inside), owned(&[("rl", "1")]));
    assert!(inside.caveats.iter().any(|c| c.contains("global_net")));
    let top = info(&net, "", "vdd");
    assert_eq!(pins(&top), owned(&[("vsup", "1")]));
    let vss = info(&net, "x1", "vss");
    assert!(!vss.is_global);
    assert_eq!(pins(&vss), owned(&[("rv", "1")]));
    let t = net.trace_net("", "vdd", TraceDir::Down, 3, &all()).unwrap();
    assert_eq!(t.hops.total, 0);
    assert!(t.caveats.iter().any(|c| c.contains(".global")));
    // `a` of `x1` goes up to `n1`
    let up = net.trace_net("x1", "a", TraceDir::Up, 1, &all()).unwrap();
    assert_eq!(up.hops.items[0].to_net, "n1");
}

#[test]
fn unknown_letter_incomplete() {
    let (_deck, net) = load_text(
        "unknown_letter",
        "* t\nv1 in 0 1\nE1 out 0 in 0 2\nr1 out 0 1k\nr2 x 0 1k\n.subckt amp p q\nE2 q 0 p 0 3\n.ends\nxa x out amp\n",
    );
    let out = info(&net, "", "out");
    assert!(out.connectivity_incomplete);
    assert_eq!(out.incomplete_due_to, ["E1"]);
    assert!(info(&net, "", "in").connectivity_incomplete);
    let x = info(&net, "", "x");
    assert!(!x.connectivity_incomplete && x.incomplete_due_to.is_empty());
    // the pins of the unparsed type are `n1`, `n2`, …
    assert!(pins(&out).contains(&("e1".to_owned(), "n1".to_owned())));
    // a trace that reaches it says so
    let t = net.trace_net("", "x", TraceDir::Down, 2, &all()).unwrap();
    assert!(t.connectivity_incomplete);
    assert_eq!(t.incomplete_due_to, ["xa.e2"]);
    assert!(t.hops.items[0].connectivity_incomplete);
}

#[test]
fn net_info_pages_high_fanout() {
    let mut text = String::from("* fanout\nv1 hub 0 1\n");
    for k in 0..999 {
        text.push_str(&format!("r{k} hub n{k} 1k\n"));
    }
    let (_deck, net) = load_text("fanout", &text);
    let p1 = net
        .net_info("", "hub", NetView::Logical, &Page::default())
        .unwrap();
    assert_eq!(p1.total, 1000);
    assert_eq!(p1.connections.total, 1000);
    assert_eq!(p1.connections.items.len(), 50);
    assert_eq!(p1.connections.next_cursor.as_deref(), Some("50"));
    let p2 = net
        .net_info(
            "",
            "hub",
            NetView::Logical,
            &Page {
                limit: 50,
                cursor: p1.connections.next_cursor.clone(),
            },
        )
        .unwrap();
    assert_eq!(p2.connections.items[0].element, "r49");
    // a page of 0 gives the counts only
    let none = net
        .net_info(
            "",
            "hub",
            NetView::Logical,
            &Page {
                limit: 0,
                cursor: None,
            },
        )
        .unwrap();
    assert!(none.connections.items.is_empty());
    assert_eq!(none.total, 1000);
}

#[test]
fn port_names_follow_the_binding() {
    // one `X` line binds to two nested definitions with other port names
    let (_deck, net) = load_text(
        "dynamic_ports",
        "* t\n.subckt t a b\nxn a b only\n.ends\n\
         .subckt outer a b\n.subckt only p q\nro p q 5\n.ends\nxt a b t\n.ends\n\
         .subckt outer2 a b\n.subckt only u v\nro u v 6\n.ends\nxt a b t\n.ends\n\
         v1 1 0 1\nx1 1 0 outer\nx2 1 0 outer2\n",
    );
    assert_eq!(pins(&info(&net, "x1.xt", "a")), owned(&[("xn", "p")]));
    assert_eq!(pins(&info(&net, "x2.xt", "a")), owned(&[("xn", "u")]));
    let t = net.trace_net("x2", "a", TraceDir::Down, 2, &all()).unwrap();
    let to: Vec<(String, String)> = t
        .hops
        .items
        .iter()
        .map(|h| (h.to_scope.clone(), h.to_net.clone()))
        .collect();
    assert_eq!(to, owned(&[("x2.xt", "a"), ("x2.xt.xn", "u")]));
}

#[test]
fn unbound_and_mismatched_x_lines() {
    let (_deck, net) = load_text(
        "bad_x",
        "* t\n.subckt cell a b\nr1 a b 1\n.ends\nv1 n 0 1\nx1 n 0 m cell\nx2 n 0 nosuch\n",
    );
    let n = info(&net, "", "n");
    assert_eq!(pins(&n), owned(&[("v1", "1"), ("x1", "n1"), ("x2", "n1")]));
    assert!(
        n.caveats.iter().any(|c| c.contains("port_count_mismatch")),
        "{:?}",
        n.caveats
    );
    assert!(
        n.caveats.iter().any(|c| c.contains("unresolved_subckt")),
        "{:?}",
        n.caveats
    );
    let t = net.trace_net("", "n", TraceDir::Down, 2, &all()).unwrap();
    assert_eq!(t.hops.total, 0);
    assert!(t.caveats.iter().any(|c| c.contains("port_count_mismatch")));
    assert!(t.caveats.iter().any(|c| c.contains("unresolved_subckt")));
}

#[test]
fn net_names_and_errors() {
    let (_deck, net) = load_text(
        "names",
        "* t\n.subckt cell a b\nr1 a b 1\n.ends\nv1 net<3> 0 1\nr2 net<3> a:b 1\nx1.n a:b 0 cell\nr3 x1.n 0 1\n",
    );
    assert_eq!(
        pins(&info(&net, "", "NET<3>")),
        owned(&[("v1", "1"), ("r2", "1")])
    );
    assert_eq!(
        pins(&info(&net, "", "a:b")),
        owned(&[("r2", "2"), ("x1.n", "a")])
    );
    // a net name with a `.` is a plain name
    assert_eq!(pins(&info(&net, "", "x1.n")), owned(&[("r3", "1")]));
    match net.net_info("", "net<4>", NetView::Logical, &all()) {
        Err(QueryError::NotFound { suggestions, .. }) => {
            assert!(suggestions.iter().any(|s| s == "net<3>"), "{suggestions:?}")
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        net.net_info("", " ", NetView::Logical, &all()),
        Err(QueryError::BadArgument(_))
    ));
    // a device is not a scope
    match net.net_info("r2", "a", NetView::Logical, &all()) {
        Err(QueryError::BadArgument(msg)) => {
            assert!(msg.contains("not a subckt instance"), "{msg}")
        }
        other => panic!("{other:?}"),
    }
    assert!(net.net_info("x9", "a", NetView::Logical, &all()).is_err());
    // the instance `x1.n` is a scope
    assert_eq!(pins(&info(&net, "x1.n", "a")), owned(&[("r1", "1")]));
}

#[test]
fn bare_netlist_top_level() {
    let (_deck, net) = load_text(
        "bare",
        ".subckt cell a b\nr1 a b 1\n.ends\n.subckt top p\nx1 p 0 cell\n.ends\n",
    );
    match net.net_info("", "p", NetView::Logical, &all()) {
        Err(QueryError::NotFound { .. }) => {}
        other => panic!("{other:?}"),
    }
    let t = net.trace_net("", "0", TraceDir::Down, 1, &all()).unwrap();
    assert_eq!(t.hops.total, 0);
}

#[test]
fn trace_stops_at_its_step_limit() {
    let n = 100_005;
    let mut text = String::from("* t\n.subckt leaf a\nr1 a 0 1\n.ends\nv1 hub 0 1\n");
    for k in 0..n {
        text.push_str(&format!("x{k} hub leaf\n"));
    }
    let (_deck, net) = load_text("step_limit", &text);
    let t = net
        .trace_net("", "hub", TraceDir::Down, 3, &Page::default())
        .unwrap();
    assert!(t.truncated);
    assert_eq!(t.hops.total, 100_000);
    assert_eq!(t.hops.items.len(), 50);
    assert!(t.caveats.iter().any(|c| c.contains("stopped")));
}

#[test]
fn golden_angle_names() {
    // golden `node_angle_names`: HSPICE accepts `<` and `>` in node names
    // and in `.subckt` ports
    let net = load(repo("golden/cases/node_angle_names/deck.sp"));
    assert_eq!(
        pins(&info(&net, "", "a<0>")),
        owned(&[("r1", "2"), ("r2", "1"), ("r3", "1"), ("x1", "d<1>")])
    );
    assert_eq!(
        pins(&info(&net, "", "b>c")),
        owned(&[("r3", "2"), ("r4", "1")])
    );
    assert_eq!(pins(&info(&net, "x1", "D<1>")), owned(&[("rc", "1")]));
    let up = net
        .trace_net("x1", "d<1>", TraceDir::Up, 1, &all())
        .unwrap();
    assert_eq!(up.hops.items[0].to_net, "a<0>");
}

#[test]
fn skipped_lines_make_nets_incomplete() {
    // `POLY(n)` lines are skipped with a load Error: the nets of their
    // definition can have pins that are not listed (review of Task 9)
    let (_d, net) = load_text(
        "poly",
        "* poly\ng1 o1 0 poly(2) in 0 n2 0 0 1 1\nv1 in 0 1\nvsense n2 0 0\nr1 o1 0 1\n.end\n",
    );
    let o1 = info(&net, "", "o1");
    assert!(o1.connectivity_incomplete, "{o1:?}");
    assert_eq!(o1.incomplete_due_to, ["g1"]);
    assert!(
        o1.caveats
            .iter()
            .any(|c| c.contains("skipped by a load error"))
    );
    let t = net.trace_net("", "in", TraceDir::Down, 3, &all()).unwrap();
    assert!(t.connectivity_incomplete);
    // a definition with no skipped line is not marked
    let (_d, net) = load_text(
        "poly_sub",
        "* t\n.subckt s a b\ng1 a 0 poly(1) b 0 0 1\n.ends\nx1 1 2 s\nr1 1 2 1\nv1 1 0 1\n.end\n",
    );
    assert!(!info(&net, "", "1").connectivity_incomplete);
    assert!(info(&net, "x1", "a").connectivity_incomplete);
}

#[test]
fn trace_up_stops_at_a_mismatched_x_line() {
    let (_d, net) = load_text(
        "mismatch_up",
        "* t\n.subckt cell a b\nr1 a b 1\n.ends\nx2 n1 cell\nv1 n1 0 1\n.end\n",
    );
    let up = net.trace_net("x2", "a", TraceDir::Up, 3, &all()).unwrap();
    assert!(up.hops.items.is_empty(), "{:?}", up.hops.items);
    assert!(
        up.caveats
            .iter()
            .any(|c| c.contains("the trace stops there"))
    );
}

#[test]
fn port_that_is_global() {
    let (_d, net) = load_text(
        "port_global",
        "* t\n.global vdd\n.subckt mid a b vdd\nr2 a vdd 1k\n.ends\nx1 n1 n2 n3 mid\nvdd vdd 0 1\n.end\n",
    );
    let i = info(&net, "x1", "vdd");
    assert!(i.is_port && i.is_global);
    assert!(
        i.caveats.iter().any(|c| c.contains("not probed")),
        "{:?}",
        i.caveats
    );
}
