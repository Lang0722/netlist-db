use super::super::{blocks::Blocks, statement::local_ast};
use super::instance;
use crate::{
    ast::{KeyValue, Value},
    builder::Builder as _,
    instance::{Instance, InstanceCtx, InstanceCtxBuilder},
    span::span,
};

/// Parses one element line (the whole text) and builds it.
fn inst(text: &str) -> Instance<'_> {
    let (rest, x) = instance(span(text, 0)).unwrap_or_else(|e| panic!("{text:?}: {e}"));
    assert!(rest.is_empty(), "{text:?}: left {:?}", rest.fragment());
    x.build(text)
}

/// Parses `text` as file-level statements. Returns the built elements and
/// the number of recorded errors.
fn statements(text: &str) -> (Vec<Instance<'_>>, usize) {
    let i = span(text, 0);
    let blocks = Blocks::new(i);
    let mut ast = Default::default();
    local_ast(i, None, &blocks, &mut ast);
    let elements = ast.instance.iter().map(|x| x.build(text)).collect();
    (elements, ast.errors.len())
}

/// The value and the parameters of an R, C or L element.
fn rcl<'a, 's>(x: &'a Instance<'s>) -> (Option<&'a Value<'s>>, &'a [KeyValue<'s>]) {
    match &x.ctx {
        InstanceCtx::Resistor(r) => (r.value.as_ref(), &r.params),
        InstanceCtx::Capacitor(c) => (c.value.as_ref(), &c.params),
        InstanceCtx::Inductor(l) => (l.value.as_ref(), &l.params),
        other => panic!("not R/C/L: {other:?}"),
    }
}

fn num(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Num(f)) => *f,
        other => panic!("want a number, got {other:?}"),
    }
}

fn expr<'a>(v: Option<&'a Value>) -> &'a str {
    match v {
        Some(Value::Expr(e)) => e,
        other => panic!("want an expression, got {other:?}"),
    }
}

fn keys(params: &[KeyValue]) -> Vec<String> {
    params.iter().map(|kv| kv.k.to_string()).collect()
}

fn close(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-12 * a.abs().max(b.abs())
}

/// The element as netlist text.
fn spice(x: &Instance) -> String {
    use crate::display::{DefaultFmt, SpiceFmt as _};
    let mut s = String::new();
    DefaultFmt.instance(x, &mut s).unwrap();
    s
}

#[test]
fn units_via_expr() {
    // The unit rules of `netlist_expr::scan_number` (golden `units`,
    // `units2`): `M` is milli, letters after a scale are ignored, and no
    // scale applies after an exponent.
    let cases: &[(&str, f64)] = &[
        ("R1 a b 1m", 1e-3),
        ("R1 a b 1M", 1e-3),
        ("C1 a b 10pF", 1e-11),
        ("R1 a b 1meg", 1e6),
        ("R1 a b 1me", 1e6),
        ("R1 a b 1mil", 25.4e-6),
        ("R1 a b 2.5k", 2500.0),
        ("R1 a b 1e-3k", 1e-3),
        ("R1 a b 1meter", 1e6),
        ("R1 a b 1mohm", 1e-3),
        ("L1 a b 1z", 1e-21),
        ("R1 a b 20db", 10.0),
    ];
    for &(text, want) in cases {
        let x = inst(text);
        let got = num(rcl(&x).0);
        assert!(close(got, want), "{text}: got {got:e}, want {want:e}");
    }
    // A number followed by an operator is still an expression.
    assert_eq!(expr(rcl(&inst("R1 a b 2k*x")).0), "2k*x");
}

fn mosfet<'a, 's>(x: &'a Instance<'s>) -> &'a crate::instance::MOSFET<'s> {
    match &x.ctx {
        InstanceCtx::MOSFET(m) => m,
        other => panic!("not a MOSFET: {other:?}"),
    }
}

#[test]
fn mos_extra_nodes() {
    let x = inst("M1 d g s b e1 nch W=1u");
    let m = mosfet(&x);
    assert_eq!(
        (&*m.nd, &*m.ng, &*m.ns, m.nb.as_deref()),
        ("d", "g", "s", Some("b"))
    );
    assert_eq!(m.extra_nodes, ["e1"]);
    assert_eq!(m.mname, "nch");
    assert_eq!(keys(&m.params), ["W"]);
    // Up to 7 nodes (BSIM-SOI: d g s e p b t).
    let x = inst("M2 d g s e p b t soi L=1u");
    let m = mosfet(&x);
    assert_eq!(m.nb.as_deref(), Some("e"));
    assert_eq!(m.extra_nodes, ["p", "b", "t"]);
    assert_eq!(m.mname, "soi");
    // (the parser does not know the models: `nch` can be the model name
    // with values after it, so the readers of the line count the nodes)
    let x = inst("M3 d g s b nch pl pw pa pb W=1u");
    assert_eq!(mosfet(&x).extra_nodes, ["nch", "pl", "pw", "pa"]);
    assert!(instance(span("M3 d g nch W=1u", 0)).is_err());
}

#[test]
fn mos_three_nodes() {
    let x = inst("M1 d g s nch W=1u L=0.1u");
    let m = mosfet(&x);
    assert_eq!((&*m.nd, &*m.ng, &*m.ns), ("d", "g", "s"));
    assert_eq!(m.nb, None);
    assert!(m.extra_nodes.is_empty());
    assert_eq!(m.mname, "nch");
    assert_eq!(keys(&m.params), ["W", "L"]);
    let x = inst("M1 d g s b nch");
    let m = mosfet(&x);
    assert_eq!(m.nb.as_deref(), Some("b"));
    assert!(m.extra_nodes.is_empty());
    assert!(m.params.is_empty());
}

#[test]
fn rcl_keyword_and_params() {
    let x = inst("R1 a b r='2*x' tc1=1e-3 m=2");
    let (v, params) = rcl(&x);
    assert_eq!(expr(v), "2*x");
    assert_eq!(keys(params), ["tc1", "m"]);
    assert!(close(num(Some(&params[0].v)), 1e-3));
    assert!(close(num(Some(&params[1].v)), 2.0));

    let x = inst("C1 a b c=1p");
    let (v, params) = rcl(&x);
    assert!(close(num(v), 1e-12));
    assert!(params.is_empty());

    let x = inst("L1 a b l=1n");
    assert!(close(num(rcl(&x).0), 1e-9));

    // The keyword is case-insensitive and may have spaces around `=`.
    let x = inst("R2 a b R = 2k M=3");
    let (v, params) = rcl(&x);
    assert!(close(num(v), 2000.0));
    assert_eq!(keys(params), ["M"]);

    // A positional value with parameters.
    let x = inst("R3 a b 1k tc1=1e-3 tc2=0");
    let (v, params) = rcl(&x);
    assert!(close(num(v), 1000.0));
    assert_eq!(keys(params), ["tc1", "tc2"]);

    // Only the element's own letter is its value: `l` and `w` of a
    // resistor are parameters.
    let x = inst("R4 a b r=1k l=10u w=1u");
    let (v, params) = rcl(&x);
    assert!(close(num(v), 1000.0));
    assert_eq!(keys(params), ["l", "w"]);

    // A name before `r=` is the model (golden `res_model`), and `r=` is
    // the value.
    let x = inst("R5 a b rmod r=1k");
    let (v, params) = rcl(&x);
    assert!(close(num(v), 1000.0));
    assert!(params.is_empty());
    assert_eq!(rcl_model(&x), Some("rmod"));

    // No value at all, only parameters.
    let x = inst("C2 a b m=2");
    let (v, params) = rcl(&x);
    assert!(v.is_none());
    assert_eq!(keys(params), ["m"]);

    // Continuation lines.
    let x = inst("R6 a b\n+ r='w*2'\n+ m=2");
    let (v, params) = rcl(&x);
    assert_eq!(expr(v), "w*2");
    assert_eq!(keys(params), ["m"]);

    // Nothing after the nodes is still an error.
    let (elements, errors) = statements("R7 a b\nR8 a b 1\n");
    assert_eq!(errors, 1);
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0].name, "R8");
}

#[test]
fn rcl_params_on_continuation_lines() {
    // After a value, parameters may follow on `+` lines, also after
    // comment lines and empty lines. Other next lines end the element.
    for (text, want) in [
        ("R1 a b 1k\n+ m=2\n", 1),
        ("R1 a b 1k \t\r\n  + m=2\r\n", 1),
        ("R1 a b 1k\n* c\n+ m=2\n", 1),
        ("R1 a b 1k\n$ c\n+ m=2\n", 1),
        ("R1 a b 1k\n\n+ m=2\n", 1),
        ("R1 a b 1k\n\r\n+ m=2\n", 1),
        ("R1 a b 1k $ c\n+ m=2\n", 1),
        ("R1 a b 1k $c\n+ $d\n+ m=2\n", 1),
        ("R1 a b 1k $c\n+ $d\n* e\n+ m=2\n", 1),
        ("R1 a b 1k $c\n+ $d\nR2 a b 2\n", 0),
        ("R1 a b 1k\n+\nR2 a b 2\n", 0),
        ("R1 a b 1k $c\r\n+ * d\r\nR2 a b 2\n", 0),
        ("R1 a b 1k\n* c\nR2 a b 2\n", 0),
        ("R1 a b 1k\nR2 a b 2\n", 0),
        ("R1 a b 1k\n", 0),
        ("R1 a b 1k", 0),
    ] {
        let (elements, errors) = statements(text);
        assert_eq!(errors, 0, "{text:?}");
        let (v, params) = rcl(&elements[0]);
        assert!(close(num(v), 1000.0), "{text:?}");
        assert_eq!(params.len(), want, "{text:?}");
    }
}

#[test]
fn comment_only_plus_lines() {
    // Extraction tools write `$` annotations on `+` lines with nothing
    // else. Any number of them continue the statement, which is kept
    // (two of them gave a `TrailingText` error and dropped it before).
    for text in [
        "R1 a b 1k $X=1\n+ $Y=2\n+ $Z=3\nR2 a b 2\n",
        "R1 a b 1k $X=1\n+ $Y=2\n  + $Z=3\r\n+\n+ * c\nR2 a b 2\n",
        "R1 a b rmod 1k $X=1\n+ $Y=2\n+ $Z=3\nR2 a b 2",
        "M1 d g s b nch W=1u $X=1\n+ $Y=2\n+ $Z=3\nR2 a b 2\n",
        "V1 a 0 DC 1 $X=1\n+ $Y=2\n+ $Z=3\nR2 a b 2\n",
        "X1 a b inv $X=1\n+ $Y=2\n+ $Z=3\nR2 a b 2\n",
    ] {
        let (elements, errors) = statements(text);
        assert_eq!(errors, 0, "{text:?}");
        let names: Vec<&str> = elements.iter().map(|x| &*x.name).collect();
        assert_eq!(names, [&text[..2], "R2"], "{text:?}");
    }
    // Parameters on a `+` line after them still belong to the element.
    let (elements, errors) = statements("R1 a b 1k $X=1\n+ $Y=2\n+ $Z=3\n+ m=2\nR2 a b 2\n");
    assert_eq!(errors, 0);
    assert_eq!(keys(rcl(&elements[0]).1), ["m"]);
    let (elements, errors) =
        statements("M1 d g s b nch W=1u $X=1\n+ $Y=2\n+ $Z=3\n+ L=2u\nR2 a b 2\n");
    assert_eq!(errors, 0);
    assert_eq!(keys(&mosfet(&elements[0]).params), ["W", "L"]);
    // `.param` lines too.
    let text = ".param a=1 $c\n+ $d\n+ $e\n+ b=2\nR2 a b 2\n";
    let i = span(text, 0);
    let mut ast = crate::ast::LocalAST::default();
    local_ast(i, None, &Blocks::new(i), &mut ast);
    assert!(ast.errors.is_empty(), "{:?}", ast.errors);
    assert_eq!(ast.param.len(), 2);
    assert_eq!(ast.instance.len(), 1);
    // The last comment-only `+` line can end the file with no line end
    // after it: the statement is kept, as with the line end.
    for text in [
        "R1 a b 1k $X=1\n+ $Y=2",
        "R1 a b 1k $c\n+",
        "R1 a b 1k\n+ $d",
        "R1 a b rmod 1k $c\n+ $d\n+ $e",
        "X1 a b inv $c\n+ $d",
        "V1 a 0 DC 1 $c\n+ $d",
        "M1 d g s b nch W=1u $c\n+ * d",
    ] {
        let (elements, errors) = statements(text);
        assert_eq!(errors, 0, "{text:?}");
        let names: Vec<&str> = elements.iter().map(|x| &*x.name).collect();
        assert_eq!(names, [&text[..2]], "{text:?}");
    }
    let text = ".param a=1 $c\n+ $d";
    let i = span(text, 0);
    let mut ast = crate::ast::LocalAST::default();
    local_ast(i, None, &Blocks::new(i), &mut ast);
    assert!(ast.errors.is_empty(), "{:?}", ast.errors);
    assert_eq!(ast.param.len(), 1);
}

#[test]
fn no_continuation_agrees_with_parser() {
    // `no_continuation` is a quick test. When it is true, no token and
    // no `key=value` parameter follows, and the statement ends there.
    use super::super::{
        statement::statement_end,
        tokens::{key_value_str, multiline_sep},
    };
    use super::{no_continuation, rcl_item};
    use nom::Parser as _;
    for (rest, want) in [
        ("", true),
        (" ", true),
        (" $X=1", true),
        (" $X=1\n", true),
        ("\nR2 a b 1", true),
        (" $X=1\n+ $Y=2\nR2 a b 1", true),
        (" $X=1\n+ $Y=2\n", true),
        ("\n+\nR2 a b 1", true),
        (" \r\n+ *c\r\nR2 a b 1", true),
        // any number of comment-only `+` lines
        (" $c\n+ $d\n+ $e\n", true),
        (" $X=1\n+ $Y=2\n  + $Z=3\r\n+\nR2 a b 1", true),
        // the input ends after a comment-only `+` line
        ("\n+ $d", true),
        (" $c\n+ $d", true),
        (" $c\n+", true),
        (" $c\n+ $d\n+ * e", true),
        // it cannot tell: the parser reads on
        (" $c\n+ $d\n+ m=2", false),
        (" $c\n+ $d\n+ $e\n+ m=2", false),
        (" $c\n+ $d\n\n+ $e\n", false),
        (" 2k", false),
        (" m=2", false),
        ("\n+ m=2", false),
        ("\n* c\n+ m=2", false),
        ("\n\n+ m=2", false),
        ("\n+ $d\n\n", false),
        (" *c", false),
    ] {
        assert_eq!(no_continuation(rest), want, "{rest:?}");
        if want {
            let i = span(rest, 0);
            assert!(
                multiline_sep(rcl_item).parse_complete(i).is_err(),
                "{rest:?}"
            );
            assert!(
                multiline_sep(key_value_str).parse_complete(i).is_err(),
                "{rest:?}"
            );
            assert!(statement_end(i).is_ok(), "{rest:?}");
        }
    }
}

#[test]
fn bang_in_node_name() {
    // `gnd!` is ground and `vdd!` a global net (golden `ground_alias`).
    let (elements, errors) = statements("r2 1 gnd! 4\nM1 d g vss! vdd! pch\n");
    assert_eq!(errors, 0);
    match &elements[0].ctx {
        InstanceCtx::Resistor(r) => assert_eq!((&*r.n1, &*r.n2), ("1", "gnd!")),
        other => panic!("{other:?}"),
    }
    let m = mosfet(&elements[1]);
    assert_eq!((&*m.ns, m.nb.as_deref()), ("vss!", Some("vdd!")));
}

/// `(name, value)` of the numbers after the model name of an `M` line.
fn positional(m: &crate::instance::MOSFET) -> Vec<(&'static str, f64)> {
    m.positional_params()
        .map(|(k, v)| (k, num(Some(v))))
        .collect()
}

fn nodes<'a>(m: &'a crate::instance::MOSFET) -> Vec<&'a str> {
    let mut nodes = vec![&*m.nd, &*m.ng, &*m.ns];
    nodes.extend(m.nb.as_deref());
    nodes
}

#[test]
fn mos_positional_lw() {
    // Golden `mos_positional`: the two numbers after the model name are
    // L, then W.
    let x = inst("M1 d g s b nch 2u 1u");
    let m = mosfet(&x);
    assert_eq!(nodes(m), ["d", "g", "s", "b"]);
    assert!(m.extra_nodes.is_empty());
    assert_eq!(m.mname, "nch");
    assert_eq!(positional(m), [("l", 2e-6), ("w", 1e-6)]);
    assert!(m.params.is_empty());
    // Golden `mos_positional_all`: 8 numbers are `l w ad as pd ps nrd nrs`.
    let x = inst("M1 d g s b nja 2u 1u 3p 4p 5u 6u 7 8");
    let m = mosfet(&x);
    assert_eq!(m.mname, "nja");
    assert_eq!(
        positional(m),
        [
            ("l", 2e-6),
            ("w", 1e-6),
            ("ad", 3e-12),
            ("as", 4e-12),
            ("pd", 5e-6),
            ("ps", 6e-6),
            ("nrd", 7.0),
            ("nrs", 8.0)
        ]
    );
    // Numeric node names are nodes: the model is the last token that is
    // not a number.
    let x = inst("M1 1 2 3 4 nch");
    let m = mosfet(&x);
    assert_eq!(nodes(m), ["1", "2", "3", "4"]);
    assert_eq!(m.mname, "nch");
    assert!(m.positional.is_empty());
    let x = inst("M1 1 2 3 4 5 nch 2u W=3u");
    let m = mosfet(&x);
    assert_eq!(nodes(m), ["1", "2", "3", "4"]);
    assert_eq!(m.extra_nodes, ["5"]);
    assert_eq!(positional(m), [("l", 2e-6)]);
    assert_eq!(keys(&m.params), ["W"]);
    // A model name that starts with a digit is an error (see
    // `digit_first_model`).
    assert!(instance(span("M1 d g s b 2n7002 1u", 0)).is_err());
    // Numbers on continuation lines.
    let x = inst("M1 d g s b nch\n+ 2u\n+ 1u L=3u");
    let m = mosfet(&x);
    assert_eq!(positional(m), [("l", 2e-6), ("w", 1e-6)]);
    assert_eq!(keys(&m.params), ["L"]);
    // The text form writes them back after the model name.
    let x = inst("M1 d g s b nch 2u 1u W=3u");
    let again = spice(&x);
    assert!(again.contains(" nch 2."), "{again}");
    let y = inst(&again);
    assert_eq!(positional(mosfet(&y)), [("l", 2e-6), ("w", 1e-6)]);
    assert_eq!(keys(&mosfet(&y).params), ["W"]);
    // HSPICE also runs a line with more than 8 numbers (deck
    // `q_positional_numbers_9`).
    let (elements, errors) = statements("M1 d g s b nch 1 2 3 4 5 6 7 8 9\nR1 a b 1\n");
    assert_eq!(errors, 0);
    assert_eq!(elements.len(), 2);
    // A bare `OFF` (an initial-condition flag) after the model name is
    // not the model name, and it gives no value.
    let x = inst("M1 d g s b nch 2u 1u OFF");
    let m = mosfet(&x);
    assert_eq!(nodes(m), ["d", "g", "s", "b"]);
    assert_eq!(m.mname, "nch");
    assert_eq!(positional(m), [("l", 2e-6), ("w", 1e-6)]);
    let x = inst("M1 d g s b nch off W=1u");
    let m = mosfet(&x);
    assert_eq!(m.mname, "nch");
    assert!(positional(m).is_empty());
    assert_eq!(keys(&m.params), ["W"]);
    // ... but it takes a place
    let x = inst("M1 d g s b nch off 1u 2u");
    assert_eq!(positional(mosfet(&x)), [("w", 1e-6), ("ad", 2e-6)]);
    // (`(` only separates words: the second word stands on the `+` line)
    let x = inst("M1 d g 0 0 nch l=(1u\n+ +2u)");
    let m = mosfet(&x);
    assert_eq!(keys(&m.params), ["l"]);
    assert_eq!(positional(m), [("w", 2e-6)]);
    assert!(instance(span("M1 d g 0 0 nch l='(1u\n+ +2u)' w=1u", 0)).is_ok());
    assert!(instance(span("M1 d g 0 0 nch\n+ l=(1u+2u)\n+ w=1u", 0)).is_ok());
}

fn diode<'a, 's>(x: &'a Instance<'s>) -> &'a crate::instance::Diode<'s> {
    match &x.ctx {
        InstanceCtx::Diode(d) => d,
        other => panic!("not a diode: {other:?}"),
    }
}

fn bjt<'a, 's>(x: &'a Instance<'s>) -> &'a crate::instance::BJT<'s> {
    match &x.ctx {
        InstanceCtx::BJT(q) => q,
        other => panic!("not a BJT: {other:?}"),
    }
}

#[test]
fn diode_bjt_positional() {
    // As for a MOSFET, the model name is the last token that is not a
    // number and not `OFF`, and numbers after it are positional
    // parameters: `area`, `pj` for D (golden `diode_area`), `area` for Q
    // (golden `bjt_area`).
    let pos = |p: Vec<(&'static str, &crate::ast::Value)>| -> Vec<(&'static str, f64)> {
        p.into_iter().map(|(k, v)| (k, num(Some(v)))).collect()
    };
    let x = inst("D1 a k dmod 2");
    let d = diode(&x);
    assert_eq!((&*d.nplus, &*d.nminus, &*d.mname), ("a", "k", "dmod"));
    assert_eq!(pos(d.positional_params().collect()), [("area", 2.0)]);
    let x = inst("D1 1 0 dmod 2p 4u OFF m=2");
    let d = diode(&x);
    assert_eq!((&*d.nplus, &*d.nminus, &*d.mname), ("1", "0", "dmod"));
    assert_eq!(
        pos(d.positional_params().collect()),
        [("area", 2e-12), ("pj", 4e-6)]
    );
    assert_eq!(keys(&d.params), ["m"]);
    let x = inst("D1 a k dmod");
    assert!(diode(&x).positional.is_empty());

    let x = inst("Q1 c b e qmod 2");
    let q = bjt(&x);
    assert_eq!(
        (&*q.nc, &*q.nb, &*q.ne, q.ns.as_deref()),
        ("c", "b", "e", None)
    );
    assert_eq!(q.mname, "qmod");
    assert_eq!(pos(q.positional_params().collect()), [("area", 2.0)]);
    let x = inst("Q1 c b e s qmod");
    let q = bjt(&x);
    assert_eq!(q.ns.as_deref(), Some("s"));
    assert_eq!(q.mname, "qmod");
    assert!(q.positional.is_empty());
    // Numeric node names and `OFF`.
    let x = inst("Q1 1 2 3 4 q2n3904 3 OFF");
    let q = bjt(&x);
    assert_eq!(q.ns.as_deref(), Some("4"));
    assert_eq!(q.mname, "q2n3904");
    assert_eq!(pos(q.positional_params().collect()), [("area", 3.0)]);
    // A model name that starts with a digit is an error (see
    // `digit_first_model`).
    assert!(instance(span("Q1 1 2 3 4 2n3904 3 OFF", 0)).is_err());

    // The text form writes the numbers back after the model name.
    for text in ["D1 a k dmod 2 3 m=2", "Q1 c b e s qmod 2 m=2"] {
        let again = spice(&inst(text));
        let y = inst(&again);
        assert_eq!(spice(&y), again, "{text:?}");
        assert!(
            again.contains("mod 2.") && again.ends_with("m=2.0000000e0"),
            "{again}"
        );
    }
}

/// The model, the value and the parameters of an R, C or L element.
fn rcl_model<'a, 's>(x: &'a Instance<'s>) -> Option<&'a str> {
    match &x.ctx {
        InstanceCtx::Resistor(r) => r.model.as_deref(),
        InstanceCtx::Capacitor(c) => c.model.as_deref(),
        InstanceCtx::Inductor(l) => l.model.as_deref(),
        other => panic!("not R/C/L: {other:?}"),
    }
}

#[test]
fn rcl_model_and_value() {
    // Golden `res_model`: a model name and a value.
    for (text, model, value) in [
        ("R1 a b rmod 2k", "rmod", 2e3),
        ("C1 a b cmod 1p", "cmod", 1e-12),
        ("L1 a b lmod 1n", "lmod", 1e-9),
        ("R1 a b\n+ rmod\n+ 2k", "rmod", 2e3),
        ("R1 a b nch.1 2k", "nch.1", 2e3),
    ] {
        let x = inst(text);
        assert_eq!(rcl_model(&x), Some(model), "{text:?}");
        let (v, params) = rcl(&x);
        assert!(close(num(v), value), "{text:?}");
        assert!(params.is_empty(), "{text:?}");
    }
    let x = inst("R1 a b rmod 'w*2' m=2");
    assert_eq!(rcl_model(&x), Some("rmod"));
    assert_eq!(expr(rcl(&x).0), "w*2");
    assert_eq!(keys(rcl(&x).1), ["m"]);

    // One bare name stays the value: a parameter or a model, which the
    // query layer decides.
    let x = inst("R1 a b rval");
    assert_eq!(rcl_model(&x), None);
    assert_eq!(expr(rcl(&x).0), "rval");

    // Golden `res_model`: a model name and a keyword value.
    let x = inst("R2 2 0 rmod r=3k");
    assert_eq!(rcl_model(&x), Some("rmod"));
    assert!(close(num(rcl(&x).0), 3e3));
    assert!(rcl(&x).1.is_empty());

    // Golden `elem_dup_kw`: the last keyword value wins, and a keyword
    // value wins over a value after the nodes.
    for (text, value) in [
        ("R1 1 0 r=1k r=2k", 2e3),
        ("R2 2 0 1k r=3k", 3e3),
        ("C1 1 0 c=1p C=2p", 2e-12),
        ("L1 1 0 1n l=2n l=3n", 3e-9),
    ] {
        let x = inst(text);
        assert_eq!(rcl_model(&x), None, "{text:?}");
        let (v, params) = rcl(&x);
        assert!(close(num(v), value), "{text:?}");
        assert!(params.is_empty(), "{text:?}");
    }
    let x = inst("R3 a b rmod 1k tc1=1 r=3k m=2");
    assert_eq!(rcl_model(&x), Some("rmod"));
    assert!(close(num(rcl(&x).0), 3e3));
    assert_eq!(keys(rcl(&x).1), ["tc1", "m"]);

    // The text form keeps the model.
    let x = inst("R1 a b rmod 2k m=2");
    let again = spice(&x);
    assert!(again.starts_with("R1 a b rmod "), "{again}");
    let y = inst(&again);
    assert_eq!(rcl_model(&y), Some("rmod"));
    assert!(close(num(rcl(&y).0), 2e3));

    // A third token after the nodes is `tc1` (see `rcl_order`); more
    // than `tc1` and `tc2` is an error.
    let (elements, errors) = statements("R5 a b rmod 2k 3k 4 5\nR6 a b 1\n");
    assert_eq!(errors, 1);
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0].name, "R6");

    // The model is a plain word. A word after a number or an expression
    // is a model name after the value: L reads it, R and C do not (see
    // `rcl_order`). A number after a quoted word is `tc1`.
    let (elements, errors) =
        statements("L1 a b 1n lmod\nR1 a b 2k rmod\nR2 a b 'rmod' 2k\nC1 a b x*2 cmod\nR3 a b 1\n");
    assert_eq!(errors, 2);
    let names: Vec<&str> = elements.iter().map(|x| &*x.name).collect();
    assert_eq!(names, ["L1", "R2", "R3"]);
    assert_eq!(rcl_model(&elements[0]), Some("lmod"));
    assert_eq!(rcl_model(&elements[1]), None);
    assert_eq!(expr(rcl(&elements[1]).0), "rmod");
    assert_eq!(rcl_positional(&elements[1]), [("tc1", 2e3)]);
    // Before `r=`, a number, an expression or a quoted word is a value
    // that `r=` replaces, not a model.
    for text in ["R3 a b x*2 r=1k", "R3 a b 'rmod' r=1k", "R3 a b 2k r=1k"] {
        let x = inst(text);
        assert_eq!(rcl_model(&x), None, "{text:?}");
        assert!(close(num(rcl(&x).0), 1e3), "{text:?}");
    }

    // Extracted netlists: a value and `$` annotations, also on `+`
    // lines, and a name with annotations.
    let (elements, errors) = statements(
        "r1 6 8 10.3626 $w=1.39e-08 $X=0.027\n+ $Y=0.234 $X2=0.027\nc1 1 vss 0.0093f $X=0.072 $Y=0.135\nrv a b rval $X=1\n+ m=2 $Y=2\n",
    );
    assert_eq!(errors, 0);
    assert_eq!(elements.len(), 3);
    let (v, params) = rcl(&elements[0]);
    assert!(close(num(v), 10.3626));
    assert!(params.is_empty());
    assert_eq!(rcl_model(&elements[0]), None);
    assert!(close(num(rcl(&elements[1]).0), 0.0093e-15));
    assert_eq!(rcl_model(&elements[2]), None);
    assert_eq!(expr(rcl(&elements[2]).0), "rval");
    assert_eq!(keys(rcl(&elements[2]).1), ["m"]);
}

/// The source of a V or I element.
fn source<'a, 's>(x: &'a Instance<'s>) -> &'a crate::instance::Source<'s> {
    match &x.ctx {
        InstanceCtx::Voltage(v) => &v.source,
        InstanceCtx::Current(c) => &c.source,
        other => panic!("not V/I: {other:?}"),
    }
}

/// The function of a V or I element: its name and its arguments as
/// written.
fn source_func<'a, 's>(x: &'a Instance<'s>) -> Option<(&'a str, &'a str)> {
    source(x).func.as_ref().map(|f| (&*f.name, &*f.args))
}

/// The numbers after the value of an R, C or L element, by name.
fn rcl_positional(x: &Instance) -> Vec<(&'static str, f64)> {
    let p: Vec<_> = match &x.ctx {
        InstanceCtx::Resistor(r) => r.positional_params().collect(),
        InstanceCtx::Capacitor(c) => c.positional_params().collect(),
        InstanceCtx::Inductor(l) => l.positional_params().collect(),
        other => panic!("not R/C/L: {other:?}"),
    };
    p.into_iter().map(|(k, v)| (k, num(Some(v)))).collect()
}

/// The error of the element line `text`.
fn inst_err(text: &str) -> String {
    match instance(span(text, 0)) {
        Ok(x) => panic!("{text:?}: parsed {x:?}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn source_function_not_dc() {
    for (text, name, args) in [
        (
            "V1 a 0 PULSE(0,1,0,1n,1n,5n,10n)",
            "PULSE",
            "0,1,0,1n,1n,5n,10n",
        ),
        ("V1 a 0 SIN(0 1 1meg)", "SIN", "0 1 1meg"),
        ("V1 a 0 sin (0 1 1meg)", "sin", "0 1 1meg"),
        ("V1 a 0 EXP(0 1 1n 2n 3n 4n)", "EXP", "0 1 1n 2n 3n 4n"),
        ("V1 a 0 PWL(0,0,1n,1)", "PWL", "0,0,1n,1"),
        ("V1 a 0 pwl(0 0, 1n 1)", "pwl", "0 0, 1n 1"),
        ("V1 a 0 SFFM(0 1 1meg 5 10k)", "SFFM", "0 1 1meg 5 10k"),
        ("V1 a 0 AM(1 0.5 1k 10k 1n)", "AM", "1 0.5 1k 10k 1n"),
        (
            "V1 a 0 PAT(1 0 0 1n 1n 5n b1010)",
            "PAT",
            "1 0 0 1n 1n 5n b1010",
        ),
        ("V1 a 0 PULSE(0 'vdd/2' 0 1n)", "PULSE", "0 'vdd/2' 0 1n"),
        ("V1 a 0 PULSE(0 1\n+ 0 1n)", "PULSE", "0 1\n+ 0 1n"),
        (
            "I1 a 0 PULSE(0,1m,0,1n,1n,5n,10n)",
            "PULSE",
            "0,1m,0,1n,1n,5n,10n",
        ),
        ("I1 a 0 sin(0 1m 1meg)", "sin", "0 1m 1meg"),
    ] {
        let x = inst(text);
        assert_eq!(source_func(&x), Some((name, args)), "{text:?}");
    }
    // A space-separated PWL is a function like the others.
    let x = inst("V1 a 0 PWL(0 0 1n 1)");
    assert_eq!(source_func(&x), Some(("PWL", "0 0 1n 1")));
    // A DC value stays a value.
    for text in ["V1 a 0 1.2", "V1 a 0 vdd", "V1 a 0 'vdd*2'", "I1 a 0 1m"] {
        let x = inst(text);
        assert_eq!(source_func(&x), None, "{text:?}");
        assert!(source(&x).dc.is_some(), "{text:?}");
    }
    // A function that does not close on its logical line is an error.
    let (elements, errors) = statements("V1 a 0 SIN(0 1 1meg\nR1 a b 1\n");
    assert_eq!(errors, 1);
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0].name, "R1");
}

#[test]
fn rcl_order() {
    // Golden `ind_value_model`, `ind_model_value`: L reads its value
    // before or after its model name.
    for text in ["L1 1 2 1n lmod", "L1 1 2 lmod 1n", "L1 1 2\n+ 1n\n+ lmod"] {
        let x = inst(text);
        assert_eq!(rcl_model(&x), Some("lmod"), "{text:?}");
        assert!(close(num(rcl(&x).0), 1e-9), "{text:?}");
        assert!(rcl_positional(&x).is_empty(), "{text:?}");
    }
    // Golden `res_value_model` (HSPICE reads the value as a third node
    // and aborts) and `cap_value_model` (HSPICE does not read the value
    // and uses 0): for R and C, only "model, then value" is read.
    for (text, msg) in [
        (
            "R1 1 0 2k rmod",
            "a resistor line has a value and then a model name (`rmod`): HSPICE reads the value as a third node and rejects the line. Write the model name first",
        ),
        (
            "R1 a b x*2 rmod",
            "a resistor line has a value and then a model name (`rmod`): HSPICE reads the value as a third node and rejects the line. Write the model name first",
        ),
        (
            "C1 1 2 1p cmod",
            "a capacitor line has a value and then a model name (`cmod`): HSPICE does not read the value in this order and uses a capacitance of 0. Write the model name first",
        ),
    ] {
        assert_eq!(inst_err(text), msg, "{text:?}");
    }
    // Golden `res_tc_positional`: the numbers after the value are tc1,
    // then tc2. C and L read them the same way (HSPICE syntax
    // `[[TC1=]val] [[TC2=]val]`; not probed).
    for (text, model, value, tcs) in [
        ("R1 1 0 1k 0.004", None, 1e3, &[("tc1", 0.004)][..]),
        (
            "R2 2 0 1k 0.004 1e-5",
            None,
            1e3,
            &[("tc1", 0.004), ("tc2", 1e-5)],
        ),
        ("R3 a b rmod 2k 0.004", Some("rmod"), 2e3, &[("tc1", 0.004)]),
        (
            "C1 a b 1p 0.004 1e-5",
            None,
            1e-12,
            &[("tc1", 0.004), ("tc2", 1e-5)],
        ),
        (
            "C1 a b cmod 1p 0.004",
            Some("cmod"),
            1e-12,
            &[("tc1", 0.004)],
        ),
        (
            "L1 a b 1n lmod 0.004 1e-5",
            Some("lmod"),
            1e-9,
            &[("tc1", 0.004), ("tc2", 1e-5)],
        ),
        (
            "L1 a b lmod 1n 0.004",
            Some("lmod"),
            1e-9,
            &[("tc1", 0.004)],
        ),
        (
            "R4 a b 1k\n+ 0.004 $ c\n+ 1e-5 m=2",
            None,
            1e3,
            &[("tc1", 0.004), ("tc2", 1e-5)],
        ),
    ] {
        let x = inst(text);
        assert_eq!(rcl_model(&x), model, "{text:?}");
        assert!(close(num(rcl(&x).0), value), "{text:?}");
        assert_eq!(rcl_positional(&x), tcs, "{text:?}");
    }
    // `r=` wins over the value after the nodes (golden `elem_dup_kw`);
    // the numbers after that value stay tc1 and tc2.
    let x = inst("R5 a b 1k 0.004 r=2k m=2");
    assert!(close(num(rcl(&x).0), 2e3));
    assert_eq!(rcl_positional(&x), [("tc1", 0.004)]);
    assert_eq!(keys(rcl(&x).1), ["m"]);
    // A word, then a number: the model and the value, as `rmod 2k`. The
    // query layer decides whether `rval` is a model or (with no
    // `.model rval` visible) a parameter value with tc1 = 0.004. Not
    // probed.
    let x = inst("R1 a 0 rval 0.004");
    assert_eq!(rcl_model(&x), Some("rval"));
    assert!(close(num(rcl(&x).0), 0.004));
    assert!(rcl_positional(&x).is_empty());
    // The same for two words: model `lval`, value `lmod` as written.
    // With no `.model lval` visible, the query layer reads l=lval and
    // model `lmod` (golden `ind_value_model`). Not probed.
    let x = inst("L6 a b lval lmod");
    assert_eq!(rcl_model(&x), Some("lval"));
    assert_eq!(expr(rcl(&x).0), "lmod");
    assert!(rcl_positional(&x).is_empty());
    // A first token with a character that model names do not have, and
    // then a value: an error, as before plan Task 6a-P (it gave the
    // expression `rmod-1` as the value and tc1=2k, with no error). An
    // expression value before tc1 must be in quotes.
    let bad = |element: &str, token: &str| {
        format!(
            "{element} line has `{token}` before its value: R and C take \"model, then value\", so `{token}` is in the place of a model name, but a model name has only letters, digits, `_`, `.` and `:`. If it is the value, write it in quotes (`'...'`)"
        )
    };
    for (text, element, token) in [
        ("R1 a b rmod-1 2k", "a resistor", "rmod-1"),
        ("R1 a b rmod/2 2k", "a resistor", "rmod/2"),
        ("C1 a b x*2 0.004", "a capacitor", "x*2"),
        ("R1 a b rmod+a 'w*2'", "a resistor", "rmod+a"),
    ] {
        assert_eq!(inst_err(text), bad(element, token), "{text:?}");
    }
    let x = inst("R1 a b 'x*2' 0.004");
    assert_eq!(expr(rcl(&x).0), "x*2");
    assert_eq!(rcl_positional(&x), [("tc1", 0.004)]);
    // Alone, or before `r=`, it is still a value.
    assert_eq!(expr(rcl(&inst("R1 a b rmod-1")).0), "rmod-1");
    assert!(close(num(rcl(&inst("R1 a b rmod-1 r=2k")).0), 2e3));
    // The numbers after the value keep their tokens as written, also on
    // `+` lines and when `r=` gives the value.
    let tokens = |text: &str| -> Vec<String> {
        let (_, x) = instance(span(text, 0)).unwrap();
        let InstanceCtxBuilder::Resistor(r) = x.ctx else {
            panic!("{text:?}: not R");
        };
        r.positional_params()
            .map(|(name, sp, _)| format!("{name}={}", &text[sp.start..sp.end]))
            .collect()
    };
    assert_eq!(
        tokens("R9 a 0 1k\n+ 4m $ c\n+ 1e-5"),
        ["tc1=4m", "tc2=1e-5"]
    );
    assert_eq!(tokens("R20 a 0 1k 0.004 r=2k"), ["tc1=0.004"]);
    // More than two numbers, or a word where a number goes.
    for (text, msg) in [
        (
            "R1 a b 1k 1 2 3",
            "a resistor line has 3 numbers after its value, but at most 2 are read (tc1 tc2)",
        ),
        (
            "R1 a b rmod 1k 1 x",
            "a resistor line has `x` after its value, where only numbers or quoted expressions (tc1 tc2) and `key=value` parameters may follow",
        ),
        (
            "L1 a b 1n lmod x",
            "an inductor line has `x` after its value, where only numbers or quoted expressions (tc1 tc2) and `key=value` parameters may follow",
        ),
    ] {
        assert_eq!(inst_err(text), msg, "{text:?}");
    }
    // The text form writes tc1 and tc2 after the value.
    let again = spice(&inst("R1 a b rmod 1k 0.004 1e-5 m=2"));
    let y = inst(&again);
    assert_eq!(rcl_model(&y), Some("rmod"));
    assert_eq!(rcl_positional(&y), [("tc1", 0.004), ("tc2", 1e-5)]);
    assert_eq!(keys(rcl(&y).1), ["m"]);
    // Neighbors of a rejected line are kept.
    let (elements, errors) = statements("R1 1 0 2k rmod\nC1 1 2 1p cmod\nL1 1 2 1n lmod\n");
    assert_eq!(errors, 2);
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0].name, "L1");
}

#[test]
fn digit_first_model() {
    // Golden `mos_digit_model`, `mos_digit_model_33`,
    // `mos_digit_model_ref`: HSPICE rejects a model name that starts
    // with a digit, on the `.model` line and in the model place of an
    // element line. Such a token is never read as a number there (it
    // gave model `b` and l=5 before).
    for (text, msg) in [
        (
            "M1 d g s b 5vnmos w=1u l=1u",
            "a MOSFET line has `5vnmos` in the place of its model name: HSPICE rejects a model name that starts with a digit (a number in this place has no letters after its scale: `2u`, not `2umos`)",
        ),
        (
            "M1 d g s b 33nmos w=1u l=1u",
            "a MOSFET line has `33nmos` in the place of its model name: HSPICE rejects a model name that starts with a digit (a number in this place has no letters after its scale: `2u`, not `2umos`)",
        ),
        (
            "M1 d g s b 2n7002 1u",
            "a MOSFET line has `2n7002` in the place of its model name: HSPICE rejects a model name that starts with a digit (a number in this place has no letters after its scale: `2u`, not `2umos`)",
        ),
        (
            "M1 1 2 3 4 5vnmos",
            "a MOSFET line has `5vnmos` in the place of its model name: HSPICE rejects a model name that starts with a digit (a number in this place has no letters after its scale: `2u`, not `2umos`)",
        ),
        (
            "D1 a k 1n4148",
            "a diode line has `1n4148` in the place of its model name: HSPICE rejects a model name that starts with a digit (a number in this place has no letters after its scale: `2u`, not `2umos`)",
        ),
        (
            "Q1 c b e 2n3904",
            "a BJT line has `2n3904` in the place of its model name: HSPICE rejects a model name that starts with a digit (a number in this place has no letters after its scale: `2u`, not `2umos`)",
        ),
        // Letters after a number that may be a model name: a 3-node
        // MOSFET (no full node list before `nch`). HSPICE probably
        // reads L=2u here (`mos_suffix_letters` probed 4 nodes only).
        (
            "M1 d g s nch 2umos",
            "a MOSFET line has `2umos` in the place of its model name: HSPICE rejects a model name that starts with a digit (a number in this place has no letters after its scale: `2u`, not `2umos`)",
        ),
        (
            "M1 d g s nch 2umos 1u",
            "a MOSFET line has `2umos` in the place of its model name: HSPICE rejects a model name that starts with a digit (a number in this place has no letters after its scale: `2u`, not `2umos`)",
        ),
    ] {
        assert_eq!(inst_err(text), msg, "{text:?}");
    }
    // R, C and L: a number with other letters after it is an error
    // right after a value, when no model name came before it (it
    // dropped the model before). See `digit_first_after_value`.
    let tc1 = "R and C take \"model, then value\", so it is not a model name. Right after a value with no model name before it, the place of `tc1`, a number has no letters after its scale (`2u`, not `2umos`). After a model name and a value, the letters after a number are ignored";
    for (text, msg) in [
            (
                "L1 a b 1n 33nmos",
                "an inductor line has `33nmos` right after its value, with no model name before it: L takes \"value, then model\" there, and HSPICE rejects a model name that starts with a digit (a number in this place has no letters after its scale: `2u`, not `2umos`)".to_owned(),
            ),
            (
                "C1 a b 1p 5vnmos",
                format!("a capacitor line has `5vnmos` right after its value, with no model name before it: {tc1}"),
            ),
            // R reads no model name after its value (golden
            // `res_value_model`): the place of `tc1`.
            (
                "R1 a 0 1k 2umos",
                format!("a resistor line has `2umos` right after its value, with no model name before it: {tc1}"),
            ),
            (
                "R1 a b 1k\n+ 2n7002",
                format!("a resistor line has `2n7002` right after its value, with no model name before it: {tc1}"),
            ),
            (
                "R1 a b 2k 4mx",
                format!("a resistor line has `4mx` right after its value, with no model name before it: {tc1}"),
            ),
        ] {
            assert_eq!(inst_err(text), msg, "{text:?}");
        }
    // R, C and L: before a value or `r=`, where a model name goes, a
    // token that starts with a digit is an error that names it and says
    // which order R and C take (it gave r=5 and tc1=2k before). It can
    // be a model name (`2n7002`) or a value with a unit (`10pF`).
    let rc = "R and C take \"model, then value\"";
    let l = "L takes \"model, then value\" or \"value, then model\"";
    let digit = "and HSPICE rejects a model name that starts with a digit. If it is the value, write it as one number with no letters after its scale (`10p`, not `10pF`)";
    for (text, element, token, order) in [
        ("R1 a b 2n7002 2k", "a resistor", "2n7002", rc),
        ("R1 a b 5vnmos 2k", "a resistor", "5vnmos", rc),
        ("R1 a 0 5vrmod 1k", "a resistor", "5vrmod", rc),
        ("C1 a b 33nmos 1p", "a capacitor", "33nmos", rc),
        ("R2 a b 5vmod r=2k", "a resistor", "5vmod", rc),
        ("C1 a b 10pF 0.004", "a capacitor", "10pF", rc),
        ("R1 a b 1kohm r=2k", "a resistor", "1kohm", rc),
        ("C1 a b 1pF c=2p", "a capacitor", "1pF", rc),
        ("L1 a b 1nH 0.004", "an inductor", "1nH", l),
    ] {
        assert_eq!(
            inst_err(text),
            format!(
                "{element} line has `{token}` before its value: {order}, so `{token}` is in the place of a model name, {digit}"
            ),
            "{text:?}"
        );
    }
    // After the model name and the value, no model name can follow, so
    // a number with letters after it is `tc1` or `tc2`, with the letters
    // ignored (as `mos_suffix_letters` for M).
    for (text, model, value, tcs) in [
        (
            "R3 a b rmod 2k 2umos",
            Some("rmod"),
            2e3,
            &[("tc1", 2e-6)][..],
        ),
        (
            "L1 a b 1n lmod 33nmos",
            Some("lmod"),
            1e-9,
            &[("tc1", 33e-9)],
        ),
        (
            "R1 a b 1k 0.004 5vnmos",
            None,
            1e3,
            &[("tc1", 0.004), ("tc2", 5.0)],
        ),
    ] {
        let x = inst(text);
        assert_eq!(rcl_model(&x), model, "{text:?}");
        assert!(close(num(rcl(&x).0), value), "{text:?}");
        assert_eq!(rcl_positional(&x), tcs, "{text:?}");
    }
    // One token alone is the value, read as HSPICE reads a number
    // (golden `units2`: `1mohm` = 1e-3), also before `key=value`
    // parameters other than `r=`.
    for (text, value) in [
        ("R1 a b 5vnmos", 5.0),
        ("R1 a b 1mohm m=2", 1e-3),
        ("C1 a b 10pF", 1e-11),
    ] {
        let x = inst(text);
        assert_eq!(rcl_model(&x), None, "{text:?}");
        assert!(close(num(rcl(&x).0), value), "{text:?}");
        assert!(rcl_positional(&x).is_empty(), "{text:?}");
    }
    // Golden `units`: HSPICE reads `1e-3k` as the number 0.001 (with a
    // warning), so it is a number also where a model name can be.
    let x = inst("R1 a b 1e-3k 0.004");
    assert_eq!(rcl_model(&x), None);
    assert!(close(num(rcl(&x).0), 1e-3));
    assert_eq!(rcl_positional(&x), [("tc1", 0.004)]);
    let x = inst("M1 d g s nch 1e-3k 1e3MEG");
    assert_eq!(mosfet(&x).mname, "nch");
    assert_eq!(positional(mosfet(&x)), [("l", 1e-3), ("w", 1e3)]);
    // Golden `mos_suffix_letters`: after a full node list and the model
    // name, HSPICE ignores the letters after a positional number.
    let x = inst("M1 d g s b nch 2umos 1u");
    let m = mosfet(&x);
    assert_eq!(nodes(m), ["d", "g", "s", "b"]);
    assert_eq!(m.mname, "nch");
    assert_eq!(positional(m), [("l", 2e-6), ("w", 1e-6)]);
    // Numbers with one scale are numbers; numeric node names are nodes.
    let x = inst("M1 1 2 3 4 nch 2u 1e-6");
    let m = mosfet(&x);
    assert_eq!(nodes(m), ["1", "2", "3", "4"]);
    assert_eq!(positional(m), [("l", 2e-6), ("w", 1e-6)]);
    // A model name that starts with a letter and has digits is a name.
    assert_eq!(mosfet(&inst("M1 d g s b n33 w=1u")).mname, "n33");
    // A `.model` card whose name starts with a digit is an error.
    let text = ".model 5vnmos nmos level=1\n.model n5v nmos level=1\n";
    let i = span(text, 0);
    let mut ast = crate::ast::LocalAST::default();
    super::super::statement::local_ast(i, None, &Blocks::new(i), &mut ast);
    assert_eq!(ast.errors.len(), 1);
    assert_eq!(
        ast.errors[0].err.to_string(),
        "a `.model` line has `5vnmos` in the place of its model name: HSPICE rejects a model name that starts with a digit"
    );
    assert_eq!(ast.model.len(), 1);
}

#[test]
fn mos_digit_after_model() {
    // After 4 or more nodes, the last word that does not start as a
    // number is the model (`e`), and `5vnmos` is a positional L of 5,
    // letters ignored (golden `mos_suffix_letters`). HSPICE rejects a
    // model name that starts with a digit (`mos_digit_model`), so
    // `5vnmos` cannot be the model of a deck it accepts. Not probed:
    // whether HSPICE reads this line so (golden/NOTES.md, "Choices not
    // probed").
    let x = inst("M1 d g s b e 5vnmos");
    let m = mosfet(&x);
    assert_eq!(nodes(m), ["d", "g", "s", "b"]);
    assert_eq!(m.mname, "e");
    assert_eq!(positional(m), [("l", 5.0)]);
    // With 3 nodes, `5vnmos` can be the model: an Error.
    assert_eq!(
        inst_err("M1 d g s 5vnmos"),
        "a MOSFET line has `5vnmos` in the place of its model name: HSPICE rejects a model name that starts with a digit (a number in this place has no letters after its scale: `2u`, not `2umos`)"
    );
}

#[test]
fn source_forms() {
    use crate::instance::source_args;
    // (line, DC text, AC magnitude, AC phase, function, arguments).
    // Golden `src_forms`, `src_ac_phase`, `src_order_ac_first`.
    type Want<'a> = (
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
        Option<(&'a str, &'a str)>,
    );
    let cases: &[Want] = &[
        ("V1 1 0 DC 1.2", Some("1.2"), None, None, None),
        (
            "V2 2 0 DC 1 PULSE(0 1 0 1n 1n 5n 10n)",
            Some("1"),
            None,
            None,
            Some(("PULSE", "0 1 0 1n 1n 5n 10n")),
        ),
        ("V3 3 0 1.2 AC 1 0", Some("1.2"), Some("1"), Some("0"), None),
        (
            "V4 4 0 PULSE 0 1 0 1n 1n 5n 10n",
            None,
            None,
            None,
            Some(("PULSE", "0 1 0 1n 1n 5n 10n")),
        ),
        ("V5 5 0 DC=2", Some("2"), None, None, None),
        ("I1 0 6 DC 1m", Some("1m"), None, None, None),
        (
            "V1 1 0 1.2 AC 1 45",
            Some("1.2"),
            Some("1"),
            Some("45"),
            None,
        ),
        (
            "V2 2 0 DC 0.5 AC 2 90",
            Some("0.5"),
            Some("2"),
            Some("90"),
            None,
        ),
        (
            "V1 1 0 AC 1 45 DC 0.7",
            Some("0.7"),
            Some("1"),
            Some("45"),
            None,
        ),
        // Not probed: `AC=`, the AC part around a function.
        ("V1 a 0 dc = 2 AC=1", Some("2"), Some("1"), None, None),
        ("V1 a 0 'vdd*2'", Some("'vdd*2'"), None, None, None),
        (
            "V1 a 0 0 PULSE(0 1 0 1n) AC 1",
            Some("0"),
            Some("1"),
            None,
            Some(("PULSE", "0 1 0 1n")),
        ),
        (
            "V1 a 0 AC 1 SIN(0 1 1k)",
            None,
            Some("1"),
            None,
            Some(("SIN", "0 1 1k")),
        ),
        (
            "V1 a 0 PULSE 0 1\n+ 0 1n AC 1",
            None,
            Some("1"),
            None,
            Some(("PULSE", "0 1\n+ 0 1n")),
        ),
        (
            "I1 a 0 pwl 0 0 1n 'i1*2'",
            None,
            None,
            None,
            Some(("pwl", "0 0 1n 'i1*2'")),
        ),
        // HSPICE manual `AC=acmag, acphase`: a comma before the phase.
        // Not probed.
        ("V1 a 0 AC=1,90", None, Some("1"), Some("90"), None),
        (
            "V1 a 0 DC 1 AC 1, 90",
            Some("1"),
            Some("1"),
            Some("90"),
            None,
        ),
        ("V1 a 0 AC=1 , 'p'", None, Some("1"), Some("'p'"), None),
        // The arguments without the spaces and `+` markers around them.
        (
            "V1 a 0 pulse ( 0 1 0 1n )",
            None,
            None,
            None,
            Some(("pulse", "0 1 0 1n")),
        ),
        (
            "V1 a 0 PWL(\n+ 0 0 1n 1)",
            None,
            None,
            None,
            Some(("PWL", "0 0 1n 1")),
        ),
    ];
    for &(text, dc, ac, phase, func) in cases {
        let x = inst(text);
        let s = source(&x);
        let tok = |v: &Option<(std::borrow::Cow<str>, Value)>| v.as_ref().map(|t| t.0.to_string());
        assert_eq!(tok(&s.dc).as_deref(), dc, "{text:?}");
        assert_eq!(tok(&s.ac_mag).as_deref(), ac, "{text:?}");
        assert_eq!(tok(&s.ac_phase).as_deref(), phase, "{text:?}");
        assert_eq!(source_func(&x), func, "{text:?}");
        assert!(s.params.is_empty(), "{text:?}");
    }
    // The values: numbers and expressions.
    let x = inst("V1 1 0 1.2 AC 1 45");
    assert!(close(num(source(&x).dc.as_ref().map(|t| &t.1)), 1.2));
    assert!(close(num(source(&x).ac_phase.as_ref().map(|t| &t.1)), 45.0));
    let x = inst("V1 a 0 'vdd*2'");
    assert_eq!(expr(source(&x).dc.as_ref().map(|t| &t.1)), "vdd*2");
    // Other `key=value` parameters.
    let x = inst("V1 a 0 DC 1 foo=2");
    assert_eq!(keys(&source(&x).params), ["foo"]);

    // Golden `src_order_fn_first`: a function before `DC` is rejected.
    for (text, msg) in [
        (
            "V1 1 0 PULSE(0 1 0 1n 1n 5n 10n) DC 0.7",
            "a voltage source line: a transient function comes before `DC`, so HSPICE reads that `dc` as a parameter name and rejects the line. Write `DC` first",
        ),
        (
            "I1 1 0 SIN(0 1 1k) DC=1m",
            "a current source line: a transient function comes before `DC`, so HSPICE reads that `dc` as a parameter name and rejects the line. Write `DC` first",
        ),
        (
            "V1 a 0 PULSE(0 1) SIN(0 1 1k)",
            "a voltage source line: two transient functions. Write one",
        ),
        (
            "V1 a 0 SIN(0 1 1meg",
            "a voltage source line: the `(` of its transient function does not close on its logical line",
        ),
        (
            "V1 a 0 DC",
            "a voltage source line: `DC` has no value after it",
        ),
        (
            "I1 a 0 DC AC 1",
            "a current source line: `DC` has no value after it",
        ),
        (
            "V1 a 0 1 AC",
            "a voltage source line: `AC` has no magnitude after it",
        ),
    ] {
        assert_eq!(inst_err(text), msg, "{text:?}");
    }
    // No source part: a 0 V (0 A) source, as the HSPICE manual allows
    // (`Vxxx n+ n- <<DC=> dcval> ...`, all parts optional). Not probed.
    let (elements, errors) = statements(
        "V1 a 0
I1 b 0 $ c
R1 a b 1
",
    );
    assert_eq!(errors, 0);
    assert_eq!(elements.len(), 3);
    for x in &elements[..2] {
        let s = source(x);
        assert!(s.dc.is_none() && s.ac_mag.is_none() && s.func.is_none());
        assert!(s.params.is_empty());
    }
    assert_eq!(spice(&elements[0]), "V1 a 0");
    // A token that is not a value is still an error.
    assert_eq!(
        inst_err("V1 a 0 {x}"),
        "a voltage source line has text after its two nodes that is not a value, a model name or a `key=value` parameter"
    );
    // A value after the AC part is text left over.
    let (elements, errors) = statements("V1 a 0 AC 1 0 1.2\nR1 a b 1\n");
    assert_eq!(errors, 1);
    assert_eq!(elements.len(), 1);
    // `{...}` in the arguments of a function: HSPICE rejects `{...}`
    // (golden `param_braces`), as outside the parentheses.
    for text in [
        "V1 a 0 PULSE({x} 1 0 1n)",
        "V1 a 0 PULSE(0,1\n+ {x})",
        "I1 a 0 sin(0 1m {f})",
    ] {
        let element = if text.starts_with('V') {
            "a voltage"
        } else {
            "a current"
        };
        assert_eq!(
            inst_err(text),
            format!(
                "{element} source line: `{{...}}` in the arguments of its transient function: HSPICE rejects `{{` and `}}`, also inside quotes. Write the expression in quotes without braces, `'...'`"
            ),
            "{text:?}"
        );
    }
    // Not in a `$` comment.
    let x = inst("V1 a 0 PULSE(0 1 $ {x}\n+ 0 1n)");
    assert_eq!(source_func(&x), Some(("PULSE", "0 1 $ {x}\n+ 0 1n")));
    // Also in quotes: HSPICE reads `{a}` there as the name `[a]` and
    // rejects the deck (golden `brace_quoted_fn`), as it rejects `{a}`
    // without quotes (golden `brace_fn_unquoted`). See
    // `braces_in_quotes` for the other places.
    assert!(inst_err("V5 a 0 PWL(0 0 1n '{x}')").contains("also inside quotes"));
    for text in ["V5 a 0 PWL 0 0 1n '{x}'\n", "V5 a 0 PWL(0 '{a}' 1n 1)\n"] {
        let (elements, errors) = statements(text);
        assert_eq!((elements.len(), errors), (0, 1), "{text:?}");
    }

    // The arguments without `+` markers, `$` comments and `*` lines.
    assert_eq!(source_args("0 1\n+ 0 1n"), "0 1 0 1n");
    assert_eq!(
        source_args("0 1 $ c\n* a comment line\n  + 0 1n\r\n+ 5n"),
        "0 1 0 1n 5n"
    );
    assert!(matches!(
        source_args("0,1,0"),
        std::borrow::Cow::Borrowed("0,1,0")
    ));
    // `$` starts a comment also right after a number, as in
    // `func_args_len` and the statement parser.
    assert_eq!(source_args("0 0 1n 1$c\n+ 2n 0"), "0 0 1n 1 2n 0");
    // The text form writes the function on one line, with parentheses.
    let again = spice(&inst("V1 a 0 DC 1 PULSE 0 1\n+ 0 1n AC 1 90"));
    assert!(again.contains("PULSE(0 1 0 1n)"), "{again}");
    let y = inst(&again);
    assert_eq!(source_func(&y), Some(("PULSE", "0 1 0 1n")));
    assert!(close(num(source(&y).ac_phase.as_ref().map(|t| &t.1)), 90.0));
    assert!(close(num(source(&y).dc.as_ref().map(|t| &t.1)), 1.0));
}

#[test]
fn ac_then_function() {
    // A function written with no space in it (`SIN(0,1,1meg)`) is one
    // token. After the AC magnitude it is the function, never the AC
    // phase (it gave acphase `SIN(0,1,1meg)` and no function before,
    // while `AC 1 SIN(0 1 1k)` kept the function).
    type Want<'a> = (
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        (&'a str, &'a str),
    );
    let cases: &[Want] = &[
        ("V1 a 0 AC 1 SIN(0,1,1meg)", None, None, ("SIN", "0,1,1meg")),
        (
            "V2 b 0 DC 0 AC 1 PULSE(0,1.8,0,1n,1n,5n,10n)",
            Some("0"),
            None,
            ("PULSE", "0,1.8,0,1n,1n,5n,10n"),
        ),
        ("V3 c 0 AC=1 PWL(0,0,1n,1)", None, None, ("PWL", "0,0,1n,1")),
        (
            "V4 c 0 AC 1 90 SIN(0,1,1meg)",
            None,
            Some("90"),
            ("SIN", "0,1,1meg"),
        ),
        (
            "V5 c 0 AC=1, 90 sffm(0,1,1meg)",
            None,
            Some("90"),
            ("sffm", "0,1,1meg"),
        ),
        ("I1 c 0 AC 1m exp(0,1m)", None, None, ("exp", "0,1m")),
    ];
    for &(text, dc, phase, func) in cases {
        let x = inst(text);
        let s = source(&x);
        let tok = |v: &Option<(std::borrow::Cow<str>, Value)>| v.as_ref().map(|t| t.0.to_string());
        assert_eq!(tok(&s.dc).as_deref(), dc, "{text:?}");
        assert!(s.ac_mag.is_some(), "{text:?}");
        assert_eq!(tok(&s.ac_phase).as_deref(), phase, "{text:?}");
        assert_eq!(source_func(&x), Some(func), "{text:?}");
    }
    // A second function after AC: the "two transient functions" error.
    for text in [
        "V1 a 0 PULSE(0 1) AC 1 SIN(0,1,1k)",
        "V1 a 0 PULSE(0,1) AC 1 SIN(0,1,1k)",
        "V1 a 0 SIN(0,1,1k) AC=1 90 PWL(0,0,1n,1)",
    ] {
        assert_eq!(
            inst_err(text),
            "a voltage source line: two transient functions. Write one",
            "{text:?}"
        );
    }
    // After a comma, a function is not the phase either.
    assert_eq!(
        inst_err("V1 a 0 SIN(0,1,1k) AC=1,PWL(0,0,1n,1)"),
        "a voltage source line: the `,` after the AC magnitude has no AC phase after it"
    );
    // A function is not a DC value either.
    assert_eq!(
        inst_err("V1 a 0 DC SIN(0,1,1k)"),
        "a voltage source line: `DC` has no value after it"
    );
    // An expression that is not a function call stays a value.
    let x = inst("V1 a 0 AC 1 'sin(p)'");
    assert_eq!(
        source(&x)
            .ac_phase
            .as_ref()
            .map(|t| t.0.to_string())
            .as_deref(),
        Some("'sin(p)'")
    );
}

#[test]
fn clear_element_errors() {
    for (text, msg) in [
        (
            "M1 d g nch",
            "a MOSFET needs 3 to 7 nodes before its model name, but this line has 2",
        ),
        (
            "R7 a b",
            "a resistor line has no value: write it after the two nodes, or as `r=`",
        ),
        (
            "C7 a b",
            "a capacitor line has no value: write it after the two nodes, or as `c=`",
        ),
        (
            "L7 a b",
            "an inductor line has no value: write it after the two nodes, or as `l=`",
        ),
        (
            "R7 a b $ no value",
            "a resistor line has no value: write it after the two nodes, or as `r=`",
        ),
        (
            "R5 a b {x} r=1k",
            "a resistor line has text after its two nodes that is not a value, a model name or a `key=value` parameter",
        ),
        (
            "D1 a dmod",
            "a diode needs 2 nodes before its model name, but this line has 1",
        ),
        ("D1 1 0 2", "a diode line has no model name"),
        (
            "D1 a k dmod 1 2 3",
            "a diode line has 3 numbers after its model name, but at most 2 are read (area pj)",
        ),
        (
            "Q1 c b qmod",
            "a BJT needs 3 or 4 nodes before its model name, but this line has 2",
        ),
        (
            "Q1 c b e qmod 1 2",
            "a BJT line has 2 numbers after its model name, but at most 1 is read (area)",
        ),
    ] {
        let err = match instance(span(text, 0)) {
            Ok(x) => panic!("{text:?}: parsed {x:?}"),
            Err(e) => e.to_string(),
        };
        assert_eq!(err, msg, "{text:?}");
        assert!(!err.contains("MapRes") && !err.contains("Verify"), "{err}");
    }
}

#[test]
fn braces_in_quotes() {
    // Golden `brace_quoted_elem`, `brace_quoted_fn`: HSPICE reads `{`
    // and `}` inside quotes as `[` and `]` (`'{a}'` is the name `[a]`)
    // and rejects the deck, as it rejects `{...}` without quotes
    // (golden `param_braces`). So a `{` or `}` in a quoted value is a
    // statement error in each place where a value goes: the line is
    // skipped and gives no value.
    for text in [
        "R1 1 0 '{a}'\n",
        "R1 a b rmod '{a}*2'\n",
        "C1 a b c='{a}'\n",
        "L1 a b 1n lmod '{a}'\n",
        "R1 a b 'a}'\n",
        "V1 1 0 '{a}'\n",
        "V1 1 0 DC '{a}'\n",
        "V1 1 0 DC 1 AC '{a}'\n",
        "V1 1 0 PWL(0 '{a}' 1n 1)\n",
        "V1 1 0 PWL 0 '{a}' 1n 1\n",
        "I1 1 0 PULSE(0 1m 0 1n)\n+ td='{a}'\n",
        "M1 d g s b nch w='{a}'\n",
        "M1 d g s b nch\n+ l='{a}*2'\n",
        "Q1 c b e qmod area='{a}'\n",
        "D1 a k dmod area='{a}'\n",
        "X1 a b sub w='{a}'\n",
    ] {
        let (elements, errors) = statements(text);
        assert_eq!((elements.len(), errors), (0, 1), "{text:?}");
    }
    // A `$` comment is not part of the value.
    for text in ["R1 a b '2*x' $ '{a}'\n", "V1 a 0 PWL(0 0 1n 1) $ '{a}'\n"] {
        let (elements, errors) = statements(text);
        assert_eq!((elements.len(), errors), (1, 0), "{text:?}");
    }
}

#[test]
fn pwl_repeat() {
    // Golden `pwl_repeat`, `pwl_repeat_td`, `pwl_td`: HSPICE accepts
    // `R`, `R=0 TD=1n` and `TD=1n` after the `)` of a PWL, and
    // `.option list` gives the repeat time 0 for `R` and for `R=0`. So
    // a bare `R` after a PWL function is `R=0`, and the PWL keeps its
    // points.
    for (text, want) in [
        ("V1 1 0 PWL(0 0.3 1n 1) R", &[("R", 0.0)][..]),
        (
            "V1 1 0 PWL(0 0.3 1n 1) R=0 TD=1n",
            &[("R", 0.0), ("TD", 1e-9)],
        ),
        ("V1 1 0 PWL(0 0.3 1n 1) TD=1n", &[("TD", 1e-9)]),
        (
            "V1 1 0 PWL(0 0.3 1n 1) R TD=1n",
            &[("R", 0.0), ("TD", 1e-9)],
        ),
        ("V1 1 0 PWL(0 0.3 1n 1)\n+ r", &[("r", 0.0)]),
        ("I1 1 0 pwl(0 0 1n 1m) R", &[("R", 0.0)]),
    ] {
        let x = inst(text);
        let (name, args) = source_func(&x).unwrap();
        assert_eq!(
            (name.to_ascii_lowercase().as_str(), args),
            (
                "pwl",
                text.split_once('(').unwrap().1.split_once(')').unwrap().0
            ),
            "{text:?}"
        );
        let got: Vec<(String, f64)> = source(&x)
            .params
            .iter()
            .map(|kv| (kv.k.to_string(), num(Some(&kv.v))))
            .collect();
        let want: Vec<(String, f64)> = want.iter().map(|&(k, v)| (k.to_owned(), v)).collect();
        assert_eq!(got, want, "{text:?}");
    }
    // Only after a PWL function, and once. Not probed: HSPICE manual
    // `PWL (t1 v1 ...) <R <=repeat>> <TD=delay>`.
    for text in [
        "V1 a 0 PULSE(0 1 0 1n) R\n",
        "V1 a 0 PWL(0 0 1n 1) R R\n",
        "V1 a 0 DC 1 R\n",
        "V1 a 0 PWL(0 0 1n 1) R 1n\n",
    ] {
        let (elements, errors) = statements(text);
        assert_eq!((elements.len(), errors), (0, 1), "{text:?}");
    }
    let (elements, errors) = statements("V1 1 0 PWL(0 0.3 1n 1) R $ c\n+ TD=1n\n");
    assert_eq!((elements.len(), errors), (1, 0));
    assert_eq!(keys(&source(&elements[0]).params), ["R", "TD"]);
    // Golden `pwl_repeat_noparen`: without parentheses, `R` is the
    // repeat flag (repeat delay 0), not a fifth PWL argument (it was an
    // argument before). Golden `pwl_ac_r`: after the AC magnitude, `R`
    // is the repeat flag (AC phase 0), not the AC phase (it was the
    // phase before). Golden `pwl_repeat_key_bare`: `R=1n R` is accepted
    // and the bare `R` wins (repeat delay 0); both stay, and the last
    // one wins in the lowering (as in golden `elem_dup_kw`).
    for (text, args, want, ac) in [
        (
            "V1 1 0 PWL 0 0.3 1n 1 R",
            "0 0.3 1n 1",
            &[("R", 0.0)][..],
            None,
        ),
        (
            "V1 1 0 pwl 0 0.3 1n 1 r TD=1n",
            "0 0.3 1n 1",
            &[("r", 0.0), ("TD", 1e-9)],
            None,
        ),
        (
            "V1 1 0 PWL(0 0.3 1n 1) R=1n R",
            "0 0.3 1n 1",
            &[("R", 1e-9), ("R", 0.0)],
            None,
        ),
        (
            "V1 1 0 PWL(0 0.3 1n 1) AC 1 R",
            "0 0.3 1n 1",
            &[("R", 0.0)],
            Some(1.0),
        ),
        (
            "V1 1 0 PWL(0 0.3 1n 1) AC=1 R",
            "0 0.3 1n 1",
            &[("R", 0.0)],
            Some(1.0),
        ),
    ] {
        let x = inst(text);
        assert_eq!(source_func(&x).unwrap().1, args, "{text:?}");
        let got: Vec<(String, f64)> = source(&x)
            .params
            .iter()
            .map(|kv| (kv.k.to_string(), num(Some(&kv.v))))
            .collect();
        let want: Vec<(String, f64)> = want.iter().map(|&(k, v)| (k.to_owned(), v)).collect();
        assert_eq!(got, want, "{text:?}");
        assert_eq!(
            source(&x).ac_mag.as_ref().map(|(_, v)| num(Some(v))),
            ac,
            "{text:?}"
        );
        assert!(source(&x).ac_phase.is_none(), "{text:?}");
    }
    // With no PWL before it, `R` after the AC magnitude is still the
    // phase (a parameter name; not probed).
    assert!(source(&inst("V1 1 0 AC 1 R")).ac_phase.is_some());
    // After the repeat flag, `R` is the phase again: one flag only.
    let (elements, errors) = statements("V1 1 0 PWL(0 0.3 1n 1) R AC 1 R\n");
    assert_eq!((elements.len(), errors), (1, 0));
    assert!(source(&elements[0]).ac_phase.is_some());
    // A bare value named `r` as the first item is still the DC value.
    assert!(source(&inst("V1 a 0 r")).dc.is_some());
    // The text form writes it as `R=0`, which reads back the same.
    let again = spice(&inst("V1 1 0 PWL(0 0.3 1n 1) R"));
    assert!(again.starts_with("V1 1 0 PWL(0 0.3 1n 1) R="), "{again}");
    let y = inst(&again);
    assert_eq!(keys(&source(&y).params), ["R"]);
    assert_eq!(num(Some(&source(&y).params[0].v)), 0.0);
}

#[test]
fn digit_first_after_value() {
    // The error fires only for a word right after a value with no
    // model name before it. For R and C, that is the place of `tc1`
    // (golden `res_value_model`, `cap_value_model`: no model name
    // after an R or C value; `res_tc_positional`,
    // `cl_tc_positional`): the message says so, and that the letters
    // after a number are ignored after a model name and a value.
    let tail = "R and C take \"model, then value\", so it is not a model name. Right after a value with no model name before it, the place of `tc1`, a number has no letters after its scale (`2u`, not `2umos`). After a model name and a value, the letters after a number are ignored";
    for (text, element, word) in [
        ("R1 a b 2k 4mx", "a resistor", "4mx"),
        ("R1 a b 1k\n+ 2n7002", "a resistor", "2n7002"),
        ("C1 a b 1p 4mx", "a capacitor", "4mx"),
        ("C1 a b 1p 5vnmos", "a capacitor", "5vnmos"),
    ] {
        assert_eq!(
            inst_err(text),
            format!(
                "{element} line has `{word}` right after its value, with no model name before it: {tail}"
            ),
            "{text:?}"
        );
    }
    // For L, the place right after the value with no model name before
    // it is the place of the model name (golden `ind_value_model`).
    assert_eq!(
        inst_err("L1 a b 1n 4mx"),
        "an inductor line has `4mx` right after its value, with no model name before it: L takes \"value, then model\" there, and HSPICE rejects a model name that starts with a digit (a number in this place has no letters after its scale: `2u`, not `2umos`)"
    );
    // After a model name and a value: `tc1` with the letters ignored.
    for text in [
        "R1 a b rmod 2k 4mx",
        "C1 a b cmod 1p 4mx",
        "L1 a b 1n lmod 4mx",
    ] {
        let x = inst(text);
        assert_eq!(rcl_positional(&x), [("tc1", 4e-3)], "{text:?}");
    }
    // The place of `tc2`: the letters are ignored too.
    let x = inst("R1 a b 2k 0.004 4mx");
    assert_eq!(rcl_positional(&x), [("tc1", 4e-3), ("tc2", 4e-3)]);
}

#[test]
fn dollar_annotations() {
    // `$` starts an inline comment: extraction tools write `$X=… $Y=…`.
    let (elements, errors) =
        statements("M1 d g s b nch W=1u $X=1.2 $Y=3.4\nR1 a b 1k $[rpoly] $X=0\n");
    assert_eq!(errors, 0);
    assert_eq!(elements.len(), 2);
    assert_eq!(keys(&mosfet(&elements[0]).params), ["W"]);
    let (v, params) = rcl(&elements[1]);
    assert!(close(num(v), 1000.0));
    assert!(params.is_empty());
    // Also on a continuation line, and with no newline at the end.
    let (elements, errors) = statements("M1 d g s b nch W=1u $X=1\n+ L=0.1u $Y=2");
    assert_eq!(errors, 0);
    assert_eq!(keys(&mosfet(&elements[0]).params), ["W", "L"]);
}

#[test]
fn quoted_texts_on_plus_lines() {
    // A quoted source argument goes on on a `+` line (golden
    // `quoted_plus_pwl`), with its line breaks as written; a blank
    // line or a comment line inside the quotes stays in the text,
    // where the query crate reports it (HSPICE rejects it, golden
    // `quoted_blank_line`, `quoted_cmtline`).
    for text in [
        "v1 1 0 PWL(0 '0.3\n+ *1' 1n 1)",
        "v1 1 0 PWL(0 '0.3\n\n+ *1' 1n 1)",
        "v1 1 0 PWL(0 '0.3\n* it's\n+ *1' 1n 1)",
        "v1 1 0 PWL(0 '0.3 $ c\n+ *1' 1n 1)",
        "v1 1 0 PWL(0 'a' $ it's\n+ 1n 1)",
        "v1 1 0 PWL(0 0.3\n\n+ 1n 1)",
        "v1 1 0 PWL(0 0.3\n* c\n+ 1n 1)",
    ] {
        let x = inst(text);
        let (_, args) = source_func(&x).unwrap();
        let want = &text[text.find('(').unwrap() + 1..text.rfind(')').unwrap()];
        assert_eq!(args, want, "{text:?}");
    }
    // a quote that does not close in the statement
    assert!(inst_err("v1 1 0 PWL(0 '0.3 1n 1)").contains("does not close"));
    // A quoted TC1 or TC2 goes on on a `+` line (golden
    // `quoted_plus_tc`): it is kept as an expression.
    let x = inst("R1 1 0 1k '0.004'\n+ '1e-5\n+ *2'");
    let tcs: Vec<(&str, &str)> = match &x.ctx {
        InstanceCtx::Resistor(r) => r
            .positional_params()
            .map(|(k, v)| match v {
                Value::Expr(e) => (k, &**e),
                Value::Num(_) => panic!("{v:?}"),
            })
            .collect(),
        other => panic!("{other:?}"),
    };
    assert_eq!(tcs, [("tc1", "0.004"), ("tc2", "1e-5\n+ *2")]);
    // an unquoted expression there is still an error
    assert!(inst_err("R1 1 0 1k 0.004 a*2").contains("tc1 tc2"));
    // The value of `r=` can be on the next `+` line (golden
    // `kw_value_next_line`).
    for text in [
        "r1 1 0 r=\n+ 5k",
        "r1 1 0 r= $ c\n* c\n\n+ 5k",
        "r1 1 0 r =\n+\n+ 5k",
    ] {
        let x = inst(text);
        assert!(close(num(rcl(&x).0), 5000.0), "{text:?}");
    }
}
