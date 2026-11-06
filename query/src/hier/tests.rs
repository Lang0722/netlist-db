use super::{Copies, SHORT_CTX, analyze, analyze_with, distance, glob, suggest};
use crate::{LoadOptions, Netlist, Page, QueryError, SubcktSort};

/// Loads `text` as the deck of test `name` (in a temporary directory,
/// removed after the load).
fn load_text(name: &str, text: &str) -> Netlist {
    let dir =
        std::env::temp_dir().join(format!("netlist-query-hier-{}-{name}", std::process::id()));
    _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let main = dir.join("main.sp");
    std::fs::write(&main, text).unwrap();
    let net = Netlist::load(&LoadOptions::new(main));
    _ = std::fs::remove_dir_all(&dir);
    net.unwrap()
}

/// Levels of two definitions `a<l>`, `b<l>`, each with a nested
/// definition `h` that it calls (so each is a lookup scope), each
/// calling both definitions of the next level: the definitions of level
/// `l` are reached in 2^l contexts.
fn layered(levels: usize) -> String {
    let mut text = String::from("* t\n");
    for l in 0..levels {
        for x in ["a", "b"] {
            text.push_str(&format!(
                    ".subckt {x}{l} p\n.subckt h p\nr1 p 0 1\n.ends\nxh p h\nxa p a{n}\nxb p b{n}\n.ends\n",
                    n = l + 1
                ));
        }
    }
    text.push_str(&format!(
        ".subckt a{levels} p\nr1 p 0 1\n.ends\n.subckt b{levels} p\nr1 p 0 1\n.ends\nx0 1 a0\n"
    ));
    text
}

#[test]
fn one_node_per_context() {
    let net = load_text("layered", &layered(6));
    // every path to level `l` (1 <= l <= 5) has its own context here:
    // the 2^(l-1) choices of `a`/`b` above it. One node per
    // (definition, context): the top level, `a0`, and 2 * (1 + 2 + ...
    // + 16) = 62 nodes of levels 1 to 5, one copy each. Level 6 and the
    // `h` definitions call no nested name (they are not hot), so each
    // has one node, with the copies of all its callers: 2 + 11 nodes
    // (`b0>h` is not reached). 77 nodes on the instance paths. `b0` (no
    // instances) is checked in its own context: `b0`, `b0>h` and 62
    // new nodes of levels 1 to 5, 0 copies. 141 in all
    let row = |name: &str| {
        net.list_subckts(Some(name), SubcktSort::Name, &Page::default())
            .unwrap()
            .items
            .into_iter()
            .find(|r| r.name == name)
            .unwrap()
    };
    assert_eq!(row("a6").copies, Some(32));
    assert_eq!(row("b6").copies, Some(32));
    assert_eq!(row("a6").x_lines, Some(2));
    assert!(!net.hier.too_large);
    assert_eq!(net.hier.nodes.len(), 141);
    assert_eq!(net.hier.n_reached, 77);
    assert_eq!(row("a5>h").copies, Some(16));
    assert_eq!(row("b0>h").copies, Some(0));
    let (again, _) = analyze(&net);
    assert_eq!(again.nodes.len(), 141);
}

#[test]
fn too_many_contexts_stop() {
    let net = load_text("too_large", &layered(6));
    let (hier, diags) = analyze_with(&net, Some(50));
    assert!(hier.too_large);
    assert!(hier.defs[1..].iter().all(|d| d.copies == Copies::TooLarge));
    assert!(hier.expanded_devices.is_none());
    assert!(
        hier.expanded_note
            .as_deref()
            .is_some_and(|n| n.contains("too many")),
        "{:?}",
        hier.expanded_note
    );
    let codes: Vec<&str> = diags.iter().map(|d| d.code).collect();
    assert_eq!(codes, ["hierarchy_too_large"]);
    // the queries say so
    let net = Netlist { hier, ..net };
    assert!(matches!(
        net.instances_of("a6", &Page::default()),
        Err(QueryError::BadArgument(_))
    ));
    let info = net.subckt_info("a5", &Page::default()).unwrap();
    assert_eq!(info.copies, None);
    assert_eq!(
        info.copies_note.as_deref(),
        Some("too many lookup contexts")
    );
    assert!(
        info.children
            .iter()
            .all(|c| c.binds_to.is_none() && c.unresolved.is_none())
    );
    assert_eq!(info.x_lines, None);
    let rows = net
        .list_subckts(None, SubcktSort::Name, &Page::default())
        .unwrap();
    assert!(rows.items.iter().all(|r| r.x_lines.is_none()));
}

#[test]
fn stopped_analysis_keeps_a_cycle_on_the_root_path_an_error() {
    // the top level calls `loop` (which calls itself) before the
    // layers that exceed the budget: the search reported the cycle on
    // an instance path from the root before it stopped
    let text = layered(6).replace("x0 1 a0\n", "xc 1 loop\nx0 1 a0\n")
        + ".subckt loop p\nxl p loop\n.ends\n";
    let net = load_text("too_large_cycle", &text);
    let (hier, diags) = analyze_with(&net, Some(50));
    assert!(hier.too_large);
    let cycles: Vec<_> = diags.iter().filter(|d| d.code == "subckt_cycle").collect();
    assert_eq!(cycles.len(), 1, "{diags:#?}");
    assert_eq!(
        cycles[0].level,
        crate::Level::Error,
        "{}",
        cycles[0].message
    );
    assert!(
        cycles[0].message.contains("HSPICE rejects"),
        "{}",
        cycles[0].message
    );
}

#[test]
fn stopped_analysis_checks_context_free_port_counts() {
    // the analysis stops, but these `X` lines bind the same way in
    // every context: a top-level line whose callee no definition nests,
    // a line that calls a definition nested in its own definition, and
    // a line of a definition that is not hot. Each has one node too
    // many: an Error, as `m_not_positive` and `duplicate_element` give
    let text = layered(6)
        .replacen("xh p h\n", "xh p p h\n", 1)
        .replace(
            ".subckt a6 p\nr1 p 0 1\n.ends\n",
            ".subckt a6 p\nr1 p 0 1\nxc p p cell\n.ends\n",
        )
        .replace("x0 1 a0\n", "x0 1 a0\nx9 1 2 cell\n")
        + ".subckt cell p\nr1 p 0 1\n.ends\n";
    let net = load_text("too_large_ports", &text);
    let names = |diags: &[crate::model::Diag]| {
        let mut v: Vec<(crate::Level, String)> = diags
            .iter()
            .filter(|d| d.code == "port_count_mismatch")
            .map(|d| {
                (
                    d.level,
                    d.message.split('`').nth(1).unwrap_or("").to_owned(),
                )
            })
            .collect();
        v.sort_by(|a, b| a.1.cmp(&b.1));
        v
    };
    // sorted by name
    let want = [
        (crate::Level::Error, "x9".to_owned()),
        (crate::Level::Error, "xc".to_owned()),
        (crate::Level::Error, "xh".to_owned()),
    ];
    let (full, diags) = analyze(&net);
    assert!(!full.too_large);
    assert_eq!(names(&diags), want, "{diags:#?}");
    let (hier, diags) = analyze_with(&net, Some(50));
    assert!(hier.too_large);
    assert_eq!(names(&diags), want, "{diags:#?}");
}

#[test]
fn long_contexts_bind_by_the_scopes_of_the_name() {
    // `c0` .. `c{depth-1}` each nest and call `h`, so the context of
    // `c{depth}` holds `depth` lookup scopes. `c{depth}` calls
    // `names` distinct names that `holder` nests (`holder` is on
    // another instance path, `x9`): each bind reads the one scope of
    // its name (`holder`, not in the context), not the whole context
    let (depth, names) = (200, 500);
    let mut text = String::from("* t\n");
    for i in 0..depth {
        text.push_str(&format!(
            ".subckt c{i} a\n.subckt h a\nrh a 0 1\n.ends\nxh a h\nxn a c{}\n.ends\n",
            i + 1
        ));
    }
    text.push_str(&format!(".subckt c{depth} a\n"));
    for j in 0..names {
        text.push_str(&format!("xb{j} a n{j}\n"));
    }
    text.push_str(".ends\n.subckt holder a\n");
    for j in 0..names {
        text.push_str(&format!(".subckt n{j} a\nr1 a 0 2\n.ends\n"));
    }
    text.push_str(".ends\n");
    for j in 0..names {
        text.push_str(&format!(".subckt n{j} a\nr1 a 0 1\n.ends\n"));
    }
    text.push_str("x0 1 c0\n");
    let net = load_text("long_contexts", &format!("{text}x9 1 holder\n"));
    assert!(!net.hier.too_large);
    // two lookups for each name (the nearest scope of the context, then
    // `holder`, the one scope of the name), and one for the `h` of each
    // level whose context is long (`h` is in the nearest scope)
    assert_eq!(net.hier.bind_steps, 2 * names + depth - SHORT_CTX);
    let row = |net: &Netlist, name: &str| {
        net.list_subckts(Some(name), SubcktSort::Name, &Page::default())
            .unwrap()
            .items
            .into_iter()
            .find(|r| r.name == name)
            .unwrap()
    };
    assert_eq!(row(&net, "n7").copies, Some(1));
    assert_eq!(row(&net, "holder>n7").copies, Some(0));
    // the counts: `h` of each level, the chain, the leaves and `x9`
    assert_eq!(
        net.hier.expanded_instances,
        Some((depth + depth + 1 + names + 1) as u64)
    );
    // with no instance of `holder`, no instance path reaches it, so the
    // names it nests are looked up at the top level only: no lookups
    // for them
    let net = load_text("long_contexts_unused", &text);
    assert!(!net.hier.too_large);
    assert_eq!(net.hier.bind_steps, depth - SHORT_CTX);
    assert_eq!(row(&net, "n7").copies, Some(1));
    assert_eq!(row(&net, "holder>n7").copies, Some(0));
    // the same deck where `holder` is above `c0` on the instance path:
    // the names bind to its nested definitions, the farthest scope of
    // the context
    let text = text
        .replace(".subckt holder a\n", ".subckt holder a\nxc a c0\n")
        .replace("x0 1 c0\n", "x0 1 holder\n");
    let net = load_text("long_contexts_holder", &text);
    assert!(!net.hier.too_large);
    assert_eq!(net.hier.bind_steps, 2 * names + depth - SHORT_CTX + 1);
    assert_eq!(row(&net, "n7").copies, Some(0));
    assert_eq!(row(&net, "holder>n7").copies, Some(1));
}

#[test]
fn definitions_that_call_no_nested_name_have_one_node() {
    // 50 wrappers each nest and call `h` (so each is a lookup scope)
    // and call one `dut` with 100 `X` lines to `leaf`. `dut` calls no
    // nested name: one node, so 50 * 100 edges are not needed
    let mut text = String::from("* t\n.subckt leaf a\nr1 a 0 1\n.ends\n.subckt dut a\n");
    for j in 0..100 {
        text.push_str(&format!("xl{j} a leaf\n"));
    }
    text.push_str(".ends\n");
    for i in 0..50 {
        text.push_str(&format!(
            ".subckt w{i} a\n.subckt h a\nrh a 0 1\n.ends\nxh a h\nxd a dut\n.ends\nxw{i} 1 w{i}\n"
        ));
    }
    let net = load_text("cold_dut", &text);
    let (hier, diags) = analyze_with(&net, Some(1000));
    assert!(!hier.too_large, "{diags:#?}");
    let dut = net.find_def("dut");
    let crate::DefLookup::Found(dut) = dut else {
        panic!("{dut:?}")
    };
    assert_eq!(hier.def_nodes(dut).count(), 1);
    assert_eq!(hier.defs[dut.idx()].copies, Copies::Count(50));
    assert_eq!(hier.expanded_instances, Some(50 + 50 + 50 + 5000));
    assert_eq!(
        hier.expanded_devices
            .as_ref()
            .and_then(|d| d.get("resistor").copied()),
        Some(5050)
    );
}

#[test]
fn scopes_on_no_instance_path_make_no_definition_hot() {
    // the deck of `definitions_that_call_no_nested_name_have_one_node`,
    // plus an unused `lib` that nests its own `leaf`. No instance path
    // from the root reaches `lib`, so `lib>leaf` binds in no context
    // from the root: `dut` keeps one node on those paths, and the
    // analysis stays inside the budget
    let mut text = String::from("* t\n.subckt leaf a\nr1 a 0 1\n.ends\n.subckt dut a\n");
    for j in 0..100 {
        text.push_str(&format!("xl{j} a leaf\n"));
    }
    text.push_str(".ends\n.subckt lib a\n.subckt leaf a\nr1 a 0 2\n.ends\n.ends\n");
    for i in 0..50 {
        text.push_str(&format!(
            ".subckt w{i} a\n.subckt h a\nrh a 0 1\n.ends\nxh a h\nxd a dut\n.ends\nxw{i} 1 w{i}\n"
        ));
    }
    let net = load_text("cold_dut_unused_scope", &text);
    let (hier, diags) = analyze_with(&net, Some(1000));
    assert!(!hier.too_large, "{diags:#?}");
    let def = |name: &str| match net.find_def(name) {
        crate::DefLookup::Found(id) => id,
        other => panic!("{other:?}"),
    };
    assert_eq!(hier.def_nodes(def("dut")).count(), 1);
    assert_eq!(hier.defs[def("dut").idx()].copies, Copies::Count(50));
    assert_eq!(hier.defs[def("leaf").idx()].copies, Copies::Count(5000));
    assert_eq!(hier.defs[def("lib>leaf").idx()].copies, Copies::Count(0));
    assert_eq!(hier.expanded_instances, Some(50 + 50 + 50 + 5000));
    // the unused `lib` calling `dut` checks `dut` in `lib`'s context,
    // where its `X` lines bind to `lib>leaf`
    let text = text.replace(".ends\n.ends\n", ".ends\nxd a dut\n.ends\n");
    let net = load_text("cold_dut_unused_scope_call", &text);
    assert!(!net.hier.too_large);
    let nodes: Vec<_> = net.hier.def_nodes(def("dut")).collect();
    assert_eq!(nodes.len(), 2);
    let imagined = nodes[1].1;
    let e = net.hier.edges[imagined.edges.start as usize];
    assert_eq!(net.hier.nodes[e.target as usize].def, def("lib>leaf"));
    assert_eq!(imagined.copies, Copies::Count(0));
    assert_eq!(net.hier.defs[def("leaf").idx()].copies, Copies::Count(5000));
    assert!(net.diags.is_empty() || net.diags.iter().all(|d| d.code == "title_line"));
}

#[test]
fn unused_definitions_are_checked_in_any_document_order() {
    // the unused `u1` nests `only` and calls `u2`, whose `xq` calls
    // `only`: `u2` is checked inside `u1`'s imagined instance, where
    // `only` is visible, in both orders of the two definitions
    let u1 = ".subckt u1 p\n.subckt only p\nro p 0 1\n.ends\nxo p only\nxu p u2\n.ends\n";
    let u2 = ".subckt u2 p\nxq p only\n.ends\n";
    let rest = ".subckt used p\nr1 p 0 1\n.ends\nx1 1 used\n";
    let mut seen = Vec::new();
    for (k, body) in [format!("{u1}{u2}"), format!("{u2}{u1}")]
        .iter()
        .enumerate()
    {
        let net = load_text(&format!("unused_order_{k}"), &format!("* t\n{body}{rest}"));
        let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
        assert!(!codes.contains(&"unresolved_subckt"), "{:#?}", net.diags);
        let info = net.subckt_info("u2", &Page::default()).unwrap();
        assert_eq!(
            info.children[0].binds_to.as_deref(),
            Some(&["u1>only".to_owned()][..])
        );
        seen.push(codes.len());
    }
    assert_eq!(seen[0], seen[1]);
}

#[test]
fn glob_matches() {
    assert!(glob(b"in*", b"inv"));
    assert!(glob(b"*", b""));
    assert!(glob(b"?nv", b"Inv"));
    assert!(glob(b"*a*b", b"xaxxb"));
    assert!(!glob(b"n*", b"inv"));
    assert!(!glob(b"inv?", b"inv"));
    assert!(glob(b"IN*X", b"inv_x"));
}

#[test]
fn edit_distance() {
    assert_eq!(distance(b"invv", b"inv"), 1);
    assert_eq!(distance(b"INV", b"inv"), 0);
    assert_eq!(distance(b"", b"abc"), 3);
    assert_eq!(distance(b"kitten", b"sitting"), 3);
}

#[test]
fn suggestions_closest_first() {
    let names = ["inv", "nand2", "inv2", "buf"];
    let got = suggest("invv", names.iter().map(|&n| (n, n.to_owned())));
    assert_eq!(got, ["inv", "inv2"]);
    assert!(suggest("zzzzzz", names.iter().map(|&n| (n, n.to_owned()))).is_empty());
}
