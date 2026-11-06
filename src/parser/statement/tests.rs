use super::local_ast;
use crate::parser::{
    blocks::Blocks,
    tokens::{float_unit, option, value},
};
use crate::{
    Files, Parsed,
    ast::{Value, ValueBuilder},
    builder::Builder as _,
    parser::{ParseOptions, TitleLine, parse_top_multi_with},
    span::{FileId, span},
};
use std::path::PathBuf;

fn close(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-12 * a.abs().max(b.abs())
}

#[test]
fn skip_nested_moves_forward() {
    use nom::Input as _;
    // The skip of a `.subckt` with no `.ends` goes to the `.end` of the
    // top file, or to the end of input. It never goes back: a `.end`
    // before the `.subckt` (which the main parse did not stop at) is
    // passed over, so the parse cannot read the same lines again.
    let text = "* t\n.end\n.subckt s a\nR1 a b 1\n";
    let file = span(text, 0);
    let at = |line: &str| file.take_from(text.find(line).unwrap());
    let blocks = Blocks::new_top(file);
    let next = blocks.skip_nested(at(".subckt"));
    assert_eq!(next.location_offset(), text.len());
    // The `.end` after it is the stop.
    let text = "* t\n.subckt s a\nR1 a b 1\n.end\nR2 a b 2\n";
    let file = span(text, 0);
    let blocks = Blocks::new_top(file);
    let next = blocks.skip_nested(file.take_from(text.find(".subckt").unwrap()));
    assert_eq!(next.location_offset(), text.find(".end").unwrap());
    // In an included file, `.end` is no stop.
    let blocks = Blocks::new(file);
    let next = blocks.skip_nested(file.take_from(text.find(".subckt").unwrap()));
    assert_eq!(next.location_offset(), text.len());
}

#[test]
fn comment_space_newline_matches_combinators() {
    // The byte scan gives the result of the nom parser it replaces.
    use super::{comment_space_newline, space};
    use crate::span::LocatedSpan;
    use nom::{
        IResult, Parser as _,
        branch::alt,
        bytes::complete::{tag, take_till},
        character::char,
        combinator::{eof, opt, recognize, verify},
        multi::many0,
    };
    fn comment(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
        recognize((alt((tag("*"), tag("$"))), take_till(|c| c == '\n'))).parse_complete(i)
    }
    fn reference(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
        recognize(many0(verify(
            recognize((
                space,
                opt(comment),
                opt((
                    char('\n'),
                    many0((
                        space,
                        char('+'),
                        space,
                        opt(comment),
                        alt((recognize(char('\n')), eof)),
                    )),
                )),
            )),
            |s| !s.is_empty(),
        )))
        .parse_complete(i)
    }
    let mut inputs: Vec<String> = [
        "",
        " ",
        "x",
        " $c",
        " $c\n",
        " $c\nR1",
        "\n+ m=2",
        " $X=1\n+ $Y=2\n+ $Z=3\nR2 a b 1",
        " $X=1\n+ $Y=2\n  + * c\r\n+\n+ $d",
        "\n\n* c\n  \t\r\n$ d\n+ $e\n+ x",
        " *c\n+",
        "\r\n+\t$\n+ \n",
        // a comment-only `+` line at the end of the input
        " $c\n+ $d",
        "\n+",
        " $c\n+ $d\n+ * e",
        " $c\n+ $d\n+ x",
    ]
    .map(str::to_owned)
    .to_vec();
    // every string of these pieces, up to 5 long
    let pieces = [" ", "\t", "\r", "\n", "+", "*c", "$d", "x", "m=2"];
    let mut level = vec![String::new()];
    for _ in 0..5 {
        level = level
            .iter()
            .flat_map(|s| pieces.iter().map(move |p| format!("{s}{p}")))
            .collect();
        inputs.extend(level.iter().cloned());
    }
    for text in &inputs {
        let i = span(text, 0);
        let (rest, got) = comment_space_newline(i).unwrap();
        let (want_rest, want) = reference(i).unwrap();
        assert_eq!(
            (got.fragment(), rest.location_line()),
            (want.fragment(), want_rest.location_line()),
            "{text:?}"
        );
        assert_eq!(
            rest.location_offset(),
            want_rest.location_offset(),
            "{text:?}"
        );
    }
}

#[test]
fn units_via_expr() {
    // `float_unit` and `value` use `netlist_expr::scan_number`.
    let cases: &[(&str, f64)] = &[
        ("1m", 1e-3),
        ("1M", 1e-3),
        ("10pF", 1e-11),
        ("1meg", 1e6),
        ("1x", 1e6),
        ("1mi", 25.4e-6),
        ("1e-3k", 1e-3),
        ("1min", 60.0),
        ("20db", 10.0),
        ("1z", 1e-21),
        ("1V", 1.0),
    ];
    for &(s, want) in cases {
        let (rest, got) = float_unit(span(s, 0)).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert!(rest.is_empty(), "{s}: left {:?}", rest.fragment());
        assert!(close(got, want), "{s}: got {got:e}, want {want:e}");
        match value(span(s, 0)) {
            Ok((rest, ValueBuilder::Num(got))) if rest.is_empty() => {
                assert!(close(got, want), "{s}: value got {got:e}, want {want:e}")
            }
            other => panic!("{s}: value gave {other:?}"),
        }
    }
    // Not numbers: names and expressions.
    assert!(float_unit(span("inf", 0)).is_err());
    assert!(float_unit(span("nan", 0)).is_err());
    assert!(matches!(
        value(span("1n*2", 0)),
        Ok((_, ValueBuilder::Expr(_)))
    ));
}

#[test]
fn unquoted_call_in_value() {
    // HSPICE accepts function calls without quotes (golden `stat_nominal`).
    let text = ".param pa=agauss(1k,0.1k,3) pg=gauss(2k,0.1,3)\n+ px=2*max(a,(b+1))-1 py=3*(1+2)\n";
    let i = span(text, 0);
    let blocks = Blocks::new(i);
    let mut ast = Default::default();
    local_ast(i, None, &blocks, &mut ast);
    assert!(ast.errors.is_empty(), "{:?}", ast.errors);
    let params: Vec<(String, String)> = ast
        .param
        .iter()
        .map(|kv| match kv.build(text).v {
            Value::Expr(e) => (kv.k.build(text).to_string(), e.to_string()),
            Value::Num(f) => panic!("{f}"),
        })
        .collect();
    let s = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(
        params,
        [
            s("pa", "agauss(1k,0.1k,3)"),
            s("pg", "gauss(2k,0.1,3)"),
            s("px", "2*max(a,(b+1))-1"),
            s("py", "3*(1+2)"),
        ]
    );
    // A group with a space, a quote or no `)` on its line is not part
    // of the value: the value ends before its `(`.
    for (src, want) in [("sin(0 1)", "sin"), ("f('a')", "f"), ("g(1\n)", "g")] {
        match value(span(src, 0)) {
            Ok((_, ValueBuilder::Expr(e))) => assert_eq!(&src[&e], want, "{src:?}"),
            other => panic!("{src:?}: {other:?}"),
        }
    }
    // A group can start a value (HSPICE reads `r1 1 0 (3k)`).
    match value(span("(1)", 0)) {
        Ok((_, ValueBuilder::Expr(e))) => assert_eq!(&"(1)"[&e], "(1)"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn param_function_args_on_plus_lines() {
    // The argument list of a `.param` function can go on on a `+`
    // line (golden `func_args_plus`), after a blank line (golden
    // `paren_blank_args`) and after a `*` comment line or a `$` comment
    // (golden `func_args_cmtline`); spaces may stand around each
    // argument, and none are needed (`f(a,b)`).
    let text = ".param g(x,\n+ y)='x+y'\n.param g2(x,\n\n+ y)='x*y'\n\
                    .param g3(x,\n* c\n+ y)='x-y'\n.param h(x, $ c\n+ y)='x*y*1k'\n\
                    .param f( a , b )='a*b' f2(a,b)='a+b'\n";
    let i = span(text, 0);
    let blocks = Blocks::new(i);
    let mut ast = Default::default();
    local_ast(i, None, &blocks, &mut ast);
    assert!(ast.errors.is_empty(), "{:?}", ast.errors);
    let funcs: Vec<(String, Vec<String>, String)> = ast
        .param_func
        .iter()
        .map(|f| {
            (
                text[&f.name].to_string(),
                f.args.iter().map(|a| text[a].to_string()).collect(),
                text[&f.body].to_string(),
            )
        })
        .collect();
    let f = |name: &str, args: &[&str], body: &str| {
        (
            name.to_string(),
            args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            body.to_string(),
        )
    };
    assert_eq!(
        funcs,
        [
            f("g", &["x", "y"], "x+y"),
            f("g2", &["x", "y"], "x*y"),
            f("g3", &["x", "y"], "x-y"),
            f("h", &["x", "y"], "x*y*1k"),
            f("f", &["a", "b"], "a*b"),
            f("f2", &["a", "b"], "a+b"),
        ]
    );
    // a line that is not a `+` line ends the statement
    let text = ".param g(x,\ny)='x+y'\n";
    let i = span(text, 0);
    let blocks = Blocks::new(i);
    let mut ast = Default::default();
    local_ast(i, None, &blocks, &mut ast);
    assert!(ast.param_func.is_empty());
    assert!(!ast.errors.is_empty());
}

#[test]
fn values_on_plus_lines() {
    // A group in parentheses of an unquoted value goes on on a `+`
    // line, also after a blank line, a `$` comment and a comment line
    // (golden `paren_plus_call`, `paren_blank_call`; HSPICE rejects
    // the comment line, `paren_cmtline_call`, which the query crate
    // reports). The text keeps the line breaks.
    for (src, want) in [
        ("max(1k,\n+ 3k) x", "max(1k,\n+ 3k)"),
        ("max(1k,\n\n+ 3k) x", "max(1k,\n\n+ 3k)"),
        ("max(1k, $ c\n+ 3k)", "max(1k, $ c\n+ 3k)"),
        ("max(1k,\n* c\n+ 3k)", "max(1k,\n* c\n+ 3k)"),
        ("2*(a+\n+ (b\n+ +1))-1", "2*(a+\n+ (b\n+ +1))-1"),
        // the statement ends first: the value ends before the `(`
        ("g(1\n)", "g"),
        ("g(1\nr2 1 0 1k)", "g"),
        ("g(1, 2)", "g"),
        ("g(1,\n+ 2 3)", "g"),
    ] {
        match value(span(src, 0)) {
            Ok((_, ValueBuilder::Expr(e))) => assert_eq!(&src[&e], want, "{src:?}"),
            other => panic!("{src:?}: {other:?}"),
        }
    }
    // The value of `key=` can be on the next `+` line (golden
    // `kw_value_next_line`), also after a `$` comment, blank lines and
    // comment lines.
    let text = ".param a=\n+ 5k b= $ c\n* c\n\n+ 'a*2' c=\n+\n+ 3\n";
    let i = span(text, 0);
    let blocks = Blocks::new(i);
    let mut ast = Default::default();
    local_ast(i, None, &blocks, &mut ast);
    assert!(ast.errors.is_empty(), "{:?}", ast.errors);
    let params: Vec<(String, String)> = ast
        .param
        .iter()
        .map(|kv| {
            let v = match kv.build(text).v {
                Value::Expr(e) => e.to_string(),
                Value::Num(f) => f.to_string(),
            };
            (kv.k.build(text).to_string(), v)
        })
        .collect();
    let s = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(params, [s("a", "5000"), s("b", "a*2"), s("c", "3")]);
    // a `=` with no value on its statement is still an error
    for text in [
        ".param a=\n",
        ".param a=\nr1 1 0 1k\n",
        ".param a= $ c\n* c\n",
    ] {
        let i = span(text, 0);
        let blocks = Blocks::new(i);
        let mut ast = Default::default();
        local_ast(i, None, &blocks, &mut ast);
        assert!(!ast.errors.is_empty(), "{text:?}");
    }
}

#[test]
fn float_unit_fast_path_matches_scan_number() {
    // Plain numbers take a fast path; the result must be the one of
    // `scan_number` (value and length), also where a letter follows.
    for s in [
        "0",
        "3",
        "42",
        "0.071",
        ".5",
        "1.",
        "1.e5",
        "8.1e-08",
        "1.2E+15",
        "-2",
        "+.5",
        "-1.5e-3",
        "123456789012345678901234567890",
        "1e999999",
        "1e-999999",
        "1.5.3",
        "2u*w",
        "1e-3k",
        "1e",
        "3e+",
        "1meg",
        "10pF",
        "-20db",
        "5)",
        "7,",
        "1e5 x",
    ] {
        // (a number too large for `f64` is kept as an expression)
        let want = netlist_expr::scan_number(s)
            .filter(|(f, _)| f.is_finite())
            .map(|(f, n)| (f.to_bits(), n));
        let got = float_unit(span(s, 0))
            .ok()
            .map(|(rest, f)| (f.to_bits(), s.len() - rest.len()));
        assert_eq!(got, want, "{s:?}");
    }
    for s in ["inf", "-inf", "nan", "infinity", "+nan", "e5", ".", "-", ""] {
        assert!(float_unit(span(s, 0)).is_err(), "{s:?}");
    }
}

/// Parses `text` as file-level statements. Returns the built options and
/// the number of recorded errors.
fn options(text: &str) -> (Vec<(String, Option<String>)>, usize) {
    let i = span(text, 0);
    let blocks = Blocks::new(i);
    let mut ast = Default::default();
    local_ast(i, None, &blocks, &mut ast);
    let opts = ast
        .option
        .iter()
        .map(|(k, v)| {
            let v = v.map(|v| match v.build(text) {
                Value::Num(f) => format!("num:{f}"),
                Value::Expr(e) => format!("expr:{e}"),
            });
            (k.build(text).to_string(), v)
        })
        .collect();
    (opts, ast.errors.len())
}

#[test]
fn option_quoted_string() {
    let text = r#"sim_dspf="inv inv.dspf""#;
    let (rest, (k, v)) = option(span(text, 0)).unwrap();
    assert!(rest.is_empty(), "left {:?}", rest.fragment());
    assert_eq!(&text[&k], "sim_dspf");
    match v {
        Some(ValueBuilder::Expr(e)) => assert_eq!(&text[&e], "inv inv.dspf"),
        other => panic!("{other:?}"),
    }

    let (opts, errors) = options(
        ".option sim_dspf=\"inv inv.dspf\"\n.OPTION post=2 sim_dspf = \"a.dspf\" probe\n+ x='1'\n",
    );
    assert_eq!(errors, 0);
    let s = |s: &str| s.to_string();
    assert_eq!(
        opts,
        [
            (s("sim_dspf"), Some(s("expr:inv inv.dspf"))),
            (s("post"), Some(s("num:2"))),
            (s("sim_dspf"), Some(s("expr:a.dspf"))),
            (s("probe"), None),
            (s("x"), Some(s("expr:1"))),
        ]
    );
}

#[test]
fn model_without_params() {
    // HSPICE accepts a model card with only a name and a type (golden
    // `res_model`: `.model rmod r`).
    let text = ".model rmod r\n.MODEL dmod D\n.model nch nmos $ c\n.model n2 nmos level=1\n.model n3 nmos ()\n.model n4 nmos (level=1\n+ vth0=0.4)\n";
    let i = span(text, 0);
    let blocks = Blocks::new(i);
    let mut ast = Default::default();
    local_ast(i, None, &blocks, &mut ast);
    assert!(ast.errors.is_empty(), "{:?}", ast.errors);
    let models: Vec<(String, String, usize)> = ast
        .model
        .iter()
        .map(|m| {
            let m = m.build(text);
            (
                m.name.to_string(),
                format!("{:?}", m.model_type),
                m.params.len(),
            )
        })
        .collect();
    let m = |n: &str, t: &str, p: usize| (n.to_string(), t.to_string(), p);
    assert_eq!(
        models,
        [
            m("rmod", "R", 0),
            m("dmod", "D", 0),
            m("nch", "NMOS", 0),
            m("n2", "NMOS", 1),
            m("n3", "NMOS", 0),
            m("n4", "NMOS", 2),
        ]
    );
    // Text after the type that is not a parameter list is still an error.
    let text = ".model rmod r junk\n";
    let i = span(text, 0);
    let mut ast = Default::default();
    local_ast(i, None, &Blocks::new(i), &mut ast);
    assert_eq!(ast.errors.len(), 1);
    assert!(ast.model.is_empty());
}

#[tokio::test]
async fn golden_res_model_parses() {
    // The golden `res_model` deck (HSPICE accepts it) parses with no
    // error: the model card, and R1/R2 with model `rmod` and their
    // values.
    use crate::{ParseOptions, TitleLine, instance::InstanceCtx, parser::parse_top_multi_with};
    let opts = ParseOptions {
        title_line: TitleLine::Auto,
        ..Default::default()
    };
    let path = PathBuf::from("golden/cases/res_model/deck.sp");
    let (parsed, files) = parse_top_multi_with([FileId::Include { path }].into_iter(), &opts).await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(ast.model.len(), 1);
    assert_eq!(ast.model[0].name, "rmod");
    assert!(ast.model[0].params.is_empty());
    let rs: Vec<(String, Option<String>, f64)> = ast
        .instance
        .iter()
        .filter_map(|x| match &x.ctx {
            InstanceCtx::Resistor(r) => Some((
                x.name.to_string(),
                r.model.as_ref().map(|m| m.to_string()),
                match r.value {
                    Some(Value::Num(f)) => f,
                    ref other => panic!("{other:?}"),
                },
            )),
            _ => None,
        })
        .collect();
    let r = |n: &str, f: f64| (n.to_string(), Some("rmod".to_string()), f);
    assert_eq!(rs, [r("R1", 2e3), r("R2", 3e3)]);
}

/// A temp path that no other call in any test of this process uses.
fn unique_temp_path(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("netlist_db_utils_{tag}_{pid}_{n}.sp"))
}

/// Parses `text` as a top file. Returns the built AST as text and the
/// number of recorded errors.
async fn parse_display(tag: &str, text: &str) -> (String, bool) {
    let path = unique_temp_path(tag);
    std::fs::write(&path, text).unwrap();
    let opts = ParseOptions {
        title_line: TitleLine::No,
        lib_override: Vec::new(),
        skip_prot: true,
    };
    let (parsed, files): (Parsed, Files) =
        parse_top_multi_with([FileId::Include { path: path.clone() }].into_iter(), &opts).await;
    _ = std::fs::remove_file(&path);
    let (ast, has_err) = files.build(parsed);
    (ast.to_string(), has_err)
}

#[tokio::test]
async fn messy_whitespace_equivalent() {
    let clean = "\
* clean deck
.option post=2
.param w0=1u
.subckt inv a y vdd vss wn=1u
M1 y a vss vss nch W=wn L=0.1u
M2 y a vdd vdd pch W='2*wn' L=0.1u
R1 a y 1k tc1=1e-3
.ends inv
X1 in out vdd 0 inv wn=2u
C1 out 0 10f
";
    // CRLF line ends, tabs, mixed-case keywords, comments and empty
    // lines between `+` lines, and no newline at the end.
    let messy = "* messy deck\r\n\
.OPTION\tpost=2\r\n\
.Param w0=1u \t\r\n\
.SUBCKT inv a y\r\n\
* a comment between + lines\r\n\
+ vdd vss\r\n\
+\twn=1u\r\n\
M1\ty a vss vss nch\r\n\
* comment\r\n\
+ W=wn\r\n\
\r\n\
+ L=0.1u\r\n\
\tM2 y a vdd vdd pch W='2*wn' L=0.1u \t\r\n\
R1 a y 1k\r\n\
$ dollar comment\r\n\
+ tc1=1e-3\r\n\
.Ends inv\r\n\
X1\tin out vdd 0 inv\twn=2u\r\n\
C1 out 0 10f";
    let (want, err_clean) = parse_display("messy_clean", clean).await;
    let (got, err_messy) = parse_display("messy_messy", messy).await;
    assert!(!err_clean, "clean deck has errors");
    assert!(!err_messy, "messy deck has errors");
    assert!(!got.contains('\r'), "CR in a name or value: {got:?}");
    assert!(!got.contains('\t'), "tab in a name or value: {got:?}");
    assert_eq!(got, want);
    // The deck parsed, it is not two empty ASTs.
    for part in [
        "M1 y a vss vss nch",
        "R1 a y",
        "X1 in out vdd 0 inv",
        "C1 out 0",
    ] {
        assert!(want.contains(part), "{part} missing in {want}");
    }
}
