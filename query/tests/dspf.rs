//! DSPF files: the included form and `.option sim_dspf`.

mod common;
use common::*;

use netlist_query::{Diagnostic, NetInfo, NetView, Netlist, Page, Parasitics, TraceDir};

fn all() -> Page {
    Page {
        limit: 500,
        cursor: None,
    }
}

fn info(net: &Netlist, scope: &str, name: &str, view: NetView) -> NetInfo {
    net.net_info(scope, name, view, &all())
        .unwrap_or_else(|e| panic!("net_info({scope:?}, {name:?}): {e}"))
}

fn diags(net: &Netlist) -> Vec<Diagnostic> {
    net.diagnostics(None, &all()).unwrap().items
}

/// The `extra.nets` of a golden case.
fn golden_nets(case: &str) -> serde_json::Value {
    let text =
        std::fs::read_to_string(repo(&format!("golden/cases/{case}/expected.json"))).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["extra"]["nets"].clone()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs())
}

/// `p` matches the golden numbers of one net.
fn check_sums(p: &Parasitics, want: &serde_json::Value) {
    assert!(
        close(p.total_cap.unwrap(), want["total_cap"].as_f64().unwrap()),
        "{p:?}"
    );
    assert_eq!(p.r_count, want["r_count"].as_u64().unwrap(), "{p:?}");
    assert!(
        close(p.r_sum.unwrap(), want["r_sum"].as_f64().unwrap()),
        "{p:?}"
    );
    assert_eq!(p.c_count, want["c_count"].as_u64().unwrap(), "{p:?}");
    assert!(
        close(p.c_sum.unwrap(), want["c_sum"].as_f64().unwrap()),
        "{p:?}"
    );
    assert_eq!(p.pins as u64, want["pins"].as_u64().unwrap(), "{p:?}");
}

/// The DSPF of the golden cases, as a file of its own.
fn inv_dspf() -> String {
    std::fs::read_to_string(repo("golden/cases/dspf_backannot/inv.dspf")).unwrap()
}

#[test]
fn dspf_logical_net() {
    // golden `dspf_included`: the DSPF is the netlist itself
    let net = load(repo("golden/cases/dspf_included/deck.sp"));
    let a = info(&net, "xi", "A", NetView::Logical);
    // `MN1 MN1:D MN1:G …`: the gate is on `MN1:G`, a pin node of net `A`
    assert_eq!(pins(&a), owned(&[("mn1", "g"), ("mp1", "g")]));
    assert!(a.is_port);
    assert_eq!(a.total, 2);
    assert!(a.connections.items.iter().all(|c| c.node.is_none()));
    let y = info(&net, "xi", "y", NetView::Logical);
    assert_eq!(pins(&y), owned(&[("mn1", "d"), ("mp1", "d")]));
    // a sub-node or a pin node answers for its net
    for node in ["A:1", "mn1:g", "MP1:G"] {
        let n = info(&net, "xi", node, NetView::Logical);
        assert_eq!(n.net, "A", "{node}");
        assert_eq!(pins(&n), pins(&a), "{node}");
        assert!(
            n.caveats.iter().any(|c| c.contains("DSPF")),
            "{:?}",
            n.caveats
        );
    }
    // the parasitic capacitors are not pins of `VSS` in the logical view
    let vss = info(&net, "xi", "vss", NetView::Logical);
    assert_eq!(pins(&vss), owned(&[("mn1", "s"), ("mn1", "b")]));
    // the top level does not change
    let top = info(&net, "", "in", NetView::Logical);
    assert_eq!(pins(&top), owned(&[("xi", "A"), ("vin", "1")]));
    assert!(top.parasitics.is_none());
}

#[test]
fn dspf_parasitic_sums() {
    let net = load(repo("golden/cases/dspf_included/deck.sp"));
    let want = golden_nets("dspf_included");
    for (name, key) in [("A", "a"), ("Y", "y")] {
        let i = info(&net, "xi", name, NetView::Logical);
        check_sums(i.parasitics.as_ref().expect("parasitics"), &want[key]);
    }
    // a net with no `*|NET` block has none
    assert!(
        info(&net, "xi", "vdd", NetView::Logical)
            .parasitics
            .is_none()
    );
}

#[test]
fn dspf_not_in_device_totals() {
    let path = repo("golden/cases/dspf_included/expected.json");
    let golden: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let net = load(repo("golden/cases/dspf_included/deck.sp"));
    let s = net.summary();
    let devs = s.expanded_devices.clone().unwrap();
    let parasitic_r = golden["extra"]["parasitic_resistors"].as_u64().unwrap();
    let parasitic_c = golden["extra"]["parasitic_capacitors"].as_u64().unwrap();
    assert_eq!(
        devs.get("resistor").copied(),
        Some(golden["totals"]["resistors"].as_u64().unwrap() - parasitic_r)
    );
    assert_eq!(devs.get("capacitor"), None);
    assert_eq!(devs.get("mosfet").copied(), Some(2));
    let par = s.expanded_parasitics.clone().unwrap();
    assert_eq!(par.get("resistor").copied(), Some(parasitic_r));
    assert_eq!(par.get("capacitor").copied(), Some(parasitic_c));
    assert_eq!(s.dspf.len(), 1);
    let f = &s.dspf[0];
    assert_eq!(
        (f.mode.as_str(), f.subckt.as_deref()),
        ("included", Some("inv"))
    );
    assert_eq!((f.nets, f.resistors, f.capacitors), (2, 5, 3));
    // the subckt lists its devices without the parasitics
    let inv = net.subckt_info("inv", &all()).unwrap();
    let types: Vec<(&str, usize)> = inv
        .devices_by_type
        .iter()
        .map(|g| (g.r#type.as_str(), g.count))
        .collect();
    assert_eq!(types, [("mosfet", 2)]);
    let p = inv.parasitics.unwrap();
    assert_eq!((p.nets, p.resistors, p.capacitors), (2, 5, 3));
    assert_eq!(inv.elements.total, 2);
    let rows = net.list_subckts(None, Default::default(), &all()).unwrap();
    assert_eq!(rows.items[0].devices, 2);
}

#[test]
fn dspf_through_include() {
    let deck = write_deck(
        "include",
        &[
            (
                "main.sp",
                "* t\n.include 'inv.dspf'\n.model nch nmos level=1\n.model pch pmos level=1\nxi in out vdd 0 inv\nvdd vdd 0 1\nvin in 0 0.5\n.end\n",
            ),
            ("inv.dspf", &inv_dspf()),
        ],
    );
    let net = load(&deck.main);
    let want = golden_nets("dspf_included");
    let a = info(&net, "xi", "a", NetView::Logical);
    assert_eq!(pins(&a), owned(&[("mn1", "g"), ("mp1", "g")]));
    check_sums(a.parasitics.as_ref().unwrap(), &want["a"]);
    assert_eq!(
        net.summary().expanded_devices.unwrap().get("resistor"),
        None
    );
}

#[test]
fn dspf_header_after_first_line() {
    // a `*|DSPF` line after line 1 still makes the file a DSPF
    let dspf = format!("* extracted\n{}", inv_dspf());
    let deck = write_deck(
        "header_line2",
        &[
            (
                "main.sp",
                "* t\n.include 'inv.dspf'\n.model nch nmos level=1\n.model pch pmos level=1\nxi in out vdd 0 inv\nvdd vdd 0 1\nvin in 0 0.5\n.end\n",
            ),
            ("inv.dspf", &dspf),
        ],
    );
    assert_eq!(load(&deck.main).summary().dspf.len(), 1);
}

#[test]
fn dspf_backannot() {
    // golden `dspf_backannot`: `.option sim_dspf="inv inv.dspf"`
    let net = load(repo("golden/cases/dspf_backannot/deck.sp"));
    let want = golden_nets("dspf_included");
    let a = info(&net, "xi", "A", NetView::Logical);
    assert_eq!(pins(&a), owned(&[("mn1", "g"), ("mp1", "g")]));
    check_sums(a.parasitics.as_ref().expect("parasitics"), &want["a"]);
    check_sums(
        info(&net, "xi", "y", NetView::Logical)
            .parasitics
            .as_ref()
            .unwrap(),
        &want["y"],
    );
    assert!(info(&net, "", "in", NetView::Logical).parasitics.is_none());
    let s = net.summary();
    assert_eq!(s.dspf.len(), 1);
    let f = &s.dspf[0];
    assert_eq!(
        (f.mode.as_str(), f.scope.as_deref()),
        ("sim_dspf", Some("inv"))
    );
    assert_eq!(
        (f.nets, f.resistors, f.capacitors, f.unmatched_nets),
        (2, 5, 3, 0)
    );
    // the DSPF devices are not devices of the netlist
    let devs = s.expanded_devices.unwrap();
    assert_eq!(devs.get("resistor").copied(), Some(2));
    assert_eq!(devs.get("capacitor"), None);
    assert!(s.expanded_parasitics.is_none());
    assert!(
        diags(&net).iter().all(|d| !d.code.starts_with("dspf")),
        "{:?}",
        diags(&net)
    );
}

const PRE: &str = "* pre-layout\n{option}\n.model nch nmos level=1\n.model pch pmos level=1\n\
.subckt inv A Y VDD VSS\nMN1 Y A VSS VSS nch W=1u L=0.1u\nMP1 Y A VDD VDD pch W=2u L=0.1u\n.ends\n\
xi in out vdd 0 inv\nxj out o2 vdd 0 inv\nvdd vdd 0 1\nvin in 0 0.5\n.end\n";

fn backannot(name: &str, option: &str) -> (Deck, Netlist) {
    let main = PRE.replace("{option}", option);
    let deck = write_deck(name, &[("main.sp", &main), ("inv.dspf", &inv_dspf())]);
    let net = load(&deck.main);
    (deck, net)
}

#[test]
fn dspf_backannot_scopes() {
    // a subckt name: every instance of it
    let (_d, net) = backannot("by_subckt", ".option sim_dspf=\"inv inv.dspf\"");
    assert!(info(&net, "xj", "a", NetView::Logical).parasitics.is_some());
    // without `.option sim_dspf_rail=on`, a caveat (golden
    // `dspf_backannot`); with it, none
    let rail = |net: &Netlist| {
        let p = info(net, "xj", "a", NetView::Logical).parasitics.unwrap();
        p.caveats.iter().any(|c| c.contains("sim_dspf_rail"))
    };
    assert!(rail(&net));
    let (_d, net) = backannot(
        "by_subckt_rail",
        ".option sim_dspf=\"inv inv.dspf\" sim_dspf_rail=on",
    );
    assert!(!rail(&net));
    // an instance name: that instance only
    let (_d, net) = backannot("by_instance", ".option sim_dspf=\"XI inv.dspf\"");
    assert!(info(&net, "xi", "a", NetView::Logical).parasitics.is_some());
    assert!(info(&net, "xj", "a", NetView::Logical).parasitics.is_none());
    assert_eq!(net.summary().dspf[0].subckt.as_deref(), Some("inv"));
    // no scope: the top level, which has no net `A` or `Y`
    let (_d, net) = backannot("no_scope", ".option sim_dspf=\"inv.dspf\"");
    assert!(info(&net, "xi", "a", NetView::Logical).parasitics.is_none());
    assert_eq!(net.summary().dspf[0].unmatched_nets, 2);
    assert!(diags(&net).iter().any(|d| d.code == "dspf_net_unmatched"));
    // a scope that names nothing (HSPICE: "Wrong instance or subckt name")
    let (_d, net) = backannot("bad_scope", ".option sim_dspf=\"@inv inv.dspf\"");
    assert!(
        diags(&net).iter().any(|d| d.code == "dspf_scope_unknown"),
        "{:?}",
        diags(&net)
    );
    // a file that is not there
    let (_d, net) = backannot("missing", ".option sim_dspf=\"inv nosuch.dspf\"");
    assert!(
        diags(&net).iter().any(|d| d.code == "dspf_file_missing"),
        "{:?}",
        diags(&net)
    );
    assert!(net.summary().dspf.is_empty());
}

#[test]
fn dspf_physical_view_pages() {
    let net = load(repo("golden/cases/dspf_included/deck.sp"));
    let p1 = net
        .net_info(
            "xi",
            "A",
            NetView::Physical,
            &Page {
                limit: 4,
                cursor: None,
            },
        )
        .unwrap();
    // R1 R2 R3 C1 C2 and the gates, on `A`, `A:1`, `MN1:G`, `MP1:G`
    assert_eq!(p1.total, 10);
    assert_eq!(p1.connections.next_cursor.as_deref(), Some("4"));
    let got: Vec<(String, String, String)> = p1
        .connections
        .items
        .iter()
        .map(|c| {
            (
                c.element.to_ascii_lowercase(),
                c.pin_role.clone(),
                c.node.clone().unwrap(),
            )
        })
        .collect();
    let s = |a: &str, b: &str, c: &str| (a.to_owned(), b.to_owned(), c.to_owned());
    assert_eq!(
        got,
        [
            s("r1", "1", "A"),
            s("r1", "2", "A:1"),
            s("r2", "1", "A:1"),
            s("r2", "2", "MN1:G")
        ]
    );
    let rest = net
        .net_info(
            "xi",
            "A",
            NetView::Physical,
            &Page {
                limit: 50,
                cursor: p1.connections.next_cursor.clone(),
            },
        )
        .unwrap();
    let last: Vec<(String, String)> = rest
        .connections
        .items
        .iter()
        .map(|c| (c.element.to_ascii_lowercase(), c.node.clone().unwrap()))
        .collect();
    assert_eq!(
        last,
        owned(&[
            ("r3", "A:1"),
            ("r3", "MP1:G"),
            ("c1", "A:1"),
            ("c2", "MN1:G"),
            ("mn1", "MN1:G"),
            ("mp1", "MP1:G")
        ])
    );
    // a sub-node: its own pins only
    let sub = info(&net, "xi", "a:1", NetView::Physical);
    assert_eq!(
        pins(&sub),
        owned(&[("r1", "2"), ("r2", "1"), ("r3", "1"), ("c1", "1")])
    );
    // no DSPF: the node is the net
    let top = info(&net, "", "in", NetView::Physical);
    assert!(
        top.connections
            .items
            .iter()
            .all(|c| c.node.as_deref() == Some("in"))
    );
}

#[test]
fn dspf_trace_uses_logical_nets() {
    let net = load(repo("golden/cases/dspf_included/deck.sp"));
    let up = net
        .trace_net("xi", "MN1:G", TraceDir::Up, 2, &all())
        .unwrap();
    let to: Vec<(String, String)> = up
        .hops
        .items
        .iter()
        .map(|h| (h.from_net.clone(), h.to_net.clone()))
        .collect();
    assert_eq!(to, owned(&[("A", "in")]));
    let down = net.trace_net("", "out", TraceDir::Down, 2, &all()).unwrap();
    assert_eq!(down.hops.items[0].to_net, "Y");
}

#[test]
fn dspf_header_and_blocks() {
    // `<>` bus bits, a `*|NET` cap with a unit, `*+` continuation, an
    // instance section after the blocks, and a resistor value as `r=`
    let dspf = "*|DSPF 1.0\n*|DIVIDER /\n*|DELIMITER :\n*|BUSBIT <>\n*|GROUND_NET 0\n\
.SUBCKT blk D<0> Q\n\
*|NET D<0> 0.02PF\n*|P (D<0> I 0 0 0)\n*|I (X1:A X1 A I 1e-16 0 0)\n*+ \n*|S (D<0>:1 0 0)\n\
R1 D<0> D<0>:1 r=4\nR2 D<0>:1 X1:A 6\nC1 D<0>:1 0 1f\n\
*|NET Q 0.01PF\n*|P (Q O 0 0 0)\n*|I (X1:Z X1 Z O 0 0 0)\nR3 X1:Z Q 2\n\
X1 X1:A X1:Z buf\n.ENDS\n";
    let main = "* t\n.subckt buf a z\nrb a z 1k\n.ends\n.include 'blk.dspf'\nx0 n1 n2 blk\nv1 n1 0 1\n.end\n";
    let deck = write_deck("header", &[("main.sp", main), ("blk.dspf", dspf)]);
    let net = load(&deck.main);
    let d = info(&net, "x0", "d<0>", NetView::Logical);
    assert_eq!(pins(&d), owned(&[("x1", "a")]));
    let p = d.parasitics.unwrap();
    assert!(close(p.total_cap.unwrap(), 0.02e-12), "{p:?}");
    assert_eq!((p.r_count, p.c_count, p.pins), (2, 1, 2));
    assert!(
        close(p.r_sum.unwrap(), 10.0) && close(p.c_sum.unwrap(), 1e-15),
        "{p:?}"
    );
    // the `X` line of the instance section is a device of the subckt
    let t = net.trace_net("", "n1", TraceDir::Down, 3, &all()).unwrap();
    let to: Vec<(String, String)> = t
        .hops
        .items
        .iter()
        .map(|h| (h.to_scope.clone(), h.to_net.clone()))
        .collect();
    assert_eq!(to, owned(&[("x0", "D<0>"), ("x0.x1", "a")]));
    let devs = net.summary().expanded_devices.unwrap();
    assert_eq!(devs.get("resistor").copied(), Some(1));
}

#[test]
fn dspf_net_named_only_in_its_block() {
    // an internal net: no node is named `mid`, only `mid:1`, `mid:2` and
    // pin nodes
    let dspf = "*|DSPF 1.0\n.SUBCKT two A Y VSS\n\
*|NET mid 3e-15\n*|I (M1:D M1 D B 0 0 0)\n*|I (M2:G M2 G I 0 0 0)\n*|S (mid:1 0 0)\n*|S (mid:2 0 0)\n\
R1 M1:D mid:1 1\nR2 mid:1 mid:2 2\nR3 mid:2 M2:G 3\nC1 mid:1 VSS 1f\n\
M1 M1:D A VSS VSS n\nM2 Y M2:G VSS VSS n\n.ENDS\n";
    let main = "* t\n.model n nmos level=1\n.include 'two.dspf'\nx1 a y 0 two\nv1 a 0 1\n.end\n";
    let deck = write_deck("name_only", &[("main.sp", main), ("two.dspf", dspf)]);
    let net = load(&deck.main);
    let mid = info(&net, "x1", "mid", NetView::Logical);
    assert_eq!(mid.net, "mid");
    assert_eq!(pins(&mid), owned(&[("m1", "d"), ("m2", "g")]));
    let p = mid.parasitics.unwrap();
    assert_eq!((p.r_count, p.c_count, p.pins), (3, 1, 2));
    assert!(close(p.r_sum.unwrap(), 6.0), "{p:?}");
    // a sub-node answers for `mid`, with its parasitics
    let sub = info(&net, "x1", "MID:2", NetView::Logical);
    assert_eq!(sub.net, "mid");
    assert!(sub.parasitics.is_some());
    // the physical view of the net: every node of the block
    assert_eq!(info(&net, "x1", "mid", NetView::Physical).total, 9);
}

#[test]
fn dspf_ground_block() {
    // a hierarchical DSPF with a block named `gnd` (review: a trace up
    // from it panicked, and `gnd` and its sub-nodes gave other pins)
    let d = write_deck(
        "ground_block",
        &[
            (
                "hgnd.sp",
                r#"* t
.subckt buf in vss
r9 in vss 1k
.ends
.include 'hgnd.dspf'
x0 a 0 top
va a 0 1
.end
"#,
            ),
            (
                "hgnd.dspf",
                r#"*|DSPF 1.0
*|DIVIDER /
*|DELIMITER :
.SUBCKT top A gnd
*|NET A 1e-15
*|P (A I 0 0 0)
*|I (X1:IN X1 IN I 0 0 0)
R1 A X1:IN 10
*|NET gnd 2e-15
*|P (gnd B 0 0 0)
*|I (X1:VSS X1 VSS B 0 0 0)
R2 gnd X1:VSS 0.5
* Instance Section
X1 X1:IN X1:VSS buf
.ENDS
"#,
            ),
        ],
    );
    let net = load(&d.main);
    let up = net
        .trace_net("x0.x1", "vss", TraceDir::Up, 3, &all())
        .unwrap();
    let hops: Vec<_> = up
        .hops
        .items
        .iter()
        .map(|h| (h.to_scope.as_str(), h.to_net.as_str()))
        .collect();
    assert_eq!(hops, [("x0", "0")]);
    let a = info(&net, "x0", "gnd", NetView::Logical);
    let b = info(&net, "x0", "X1:VSS", NetView::Logical);
    assert!(a.is_ground && b.is_ground);
    assert_eq!(pins(&a), pins(&b));
    assert_eq!(pins(&a), owned(&[("x1", "vss")]));
    // a flat DSPF as the deck: the block and its sub-node are one net
    let d = write_deck(
        "ground_flat",
        &[(
            "flat.sp",
            r#"*|DSPF 1.0
*|DIVIDER /
*|DELIMITER :
*|NET in 1e-15
*|P (in I 0 0 0)
*|I (M1:G M1 G I 1e-16 0 0)
R1 in M1:G 10
C1 M1:G gnd:1 1e-16
*|NET gnd 5e-15
*|P (gnd B 0 0 0)
*|I (M1:S M1 S B 0 0 0)
*|S (gnd:1 0 0)
R2 gnd gnd:1 1
R3 gnd:1 M1:S 2
*|NET out 1e-15
*|I (M1:D M1 D B 0 0 0)
R4 M1:D out 3
* Instance Section
M1 M1:D M1:G M1:S M1:S nch W=1u L=0.1u
.model nch nmos level=1
vin in 0 1
rl out vdd 1k
vdd vdd 0 1
.end
"#,
        )],
    );
    let net = load(&d.main);
    let a = info(&net, "", "gnd", NetView::Logical);
    assert_eq!(pins(&a), pins(&info(&net, "", "gnd:1", NetView::Logical)));
    assert_eq!(a.total, 4, "{:?}", pins(&a));
}

#[test]
fn dspf_instance_section_devices() {
    // R and C devices at the start of the instance section are devices,
    // and do not join `VDD` and `OUT`
    let text = r#"*|DSPF 1.0
*|DIVIDER /
*|DELIMITER :
.SUBCKT amp IN OUT VDD VSS
*|NET IN 1e-15
*|P (IN I 0 0 0)
*|I (M1:G M1 G I 1e-16 0 0)
R1 IN M1:G 10
C1 M1:G VSS 1e-16
*|NET OUT 2e-15
*|P (OUT O 0 0 0)
*|I (M1:D M1 D B 0 0 0)
R2 M1:D OUT 20
C2 OUT VSS 5e-16
*
* Instance Section
*
CMIM0 OUT VSS 1p
RPOLY0 VDD M1:D 10k
M1 M1:D M1:G VSS VSS nch W=1u L=0.1u
.ENDS
"#;
    for (name, dspf) in [
        ("marked", text.to_owned()),
        ("unmarked", text.replace("* Instance Section\n", "")),
    ] {
        let d = write_deck(
            name,
            &[
                (
                    "inst_rc.sp",
                    r#"* t
.include 'inst_rc.dspf'
.model nch nmos level=1
xa in out vdd 0 amp
vdd vdd 0 1
vin in 0 0.5
.end
"#,
                ),
                ("inst_rc.dspf", &dspf),
            ],
        );
        let net = load(&d.main);
        let s = net.summary();
        let devs = s.expanded_devices.as_ref().unwrap();
        assert_eq!(
            (devs["resistor"], devs["mosfet"]),
            (1, 1),
            "{name}: {devs:?}"
        );
        let vdd = info(&net, "xa", "vdd", NetView::Logical);
        assert_eq!(vdd.net.to_ascii_lowercase(), "vdd", "{name}");
        assert_eq!(pins(&vdd), owned(&[("rpoly0", "1")]), "{name}");
        let out = info(&net, "xa", "out", NetView::Logical);
        let p = out.parasitics.as_ref().unwrap();
        assert_eq!((p.r_count, p.r_sum), (1, Some(20.0)), "{name}");
        let warned = diags(&net).iter().any(|d| d.code == "dspf_device_in_block");
        // without the marker line, `CMIM0` (it touches `OUT`) is read as a
        // parasitic of `OUT`, and `RPOLY0` (it touches `VDD`) as a device
        assert_eq!(warned, name == "unmarked", "{name}");
    }
}

#[test]
fn dspf_unread_values() {
    let d = write_deck(
        "unread",
        &[
            (
                "main.sp",
                "* t\n.param rpar=3\n.include 'blk.dspf'\nxi in out blk\nv1 in 0 1\n.end\n",
            ),
            (
                "blk.dspf",
                "*|DSPF 1.0\n.SUBCKT blk A B\n*|NET A 1e-15\n*|P (A I 0 0 0)\nR1 A A:1 10\nR2 A:1 A:2\n+ 5\nR3 A:2 A:3 rpar\nC5 A:5 B 1e-16\nR4 A:3 B 1\nC7 A:1 zz 1f\n.ENDS\n",
            ),
        ],
    );
    let net = load(&d.main);
    let a = info(&net, "xi", "a", NetView::Logical);
    let p = a.parasitics.as_ref().unwrap();
    assert_eq!((p.r_count, p.r_sum, p.c_count), (3, None, 2), "{p:?}");
    assert!(
        p.caveats[0].contains("2 parasitic values"),
        "{:?}",
        p.caveats
    );
    // a node only on a capacitor (`A:5`, `<net>:` name) is a node of `A`
    let c = info(&net, "xi", "A:5", NetView::Logical);
    assert_eq!(c.net.to_ascii_lowercase(), "a");
    // a node that only a parasitic C touches: no pin, and a caveat
    let z = info(&net, "xi", "zz", NetView::Logical);
    assert_eq!(z.total, 0);
    assert!(
        z.caveats.iter().any(|c| c.contains("only parasitic")),
        "{:?}",
        z.caveats
    );
    assert_eq!(info(&net, "xi", "zz", NetView::Physical).total, 1);
}
