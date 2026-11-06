use super::recover_tests::{TempDir, error_lines, error_msgs, errors, names, opts, parse_text};
use super::*;
use crate::{
    ParseOptions, Span, TitleLine,
    ast::{Token, Value},
};

/// Writes `text` to `deck.sp` in a temp dir and parses it as a top file
/// with `opts`.
async fn parse_with(tag: &str, text: &str, opts: &ParseOptions) -> (Parsed, Files) {
    let tmp = TempDir::new(tag);
    let path = tmp.0.join("deck.sp");
    std::fs::write(&path, text).unwrap();
    parse_top_multi_with([FileId::Include { path }].into_iter(), opts).await
}

/// The builder of the (first) top file.
fn top(parsed: &Parsed) -> &ASTBuilder {
    &parsed.inner[parsed.top_ids[0].0].1
}

/// The text of the (first) top file.
fn top_text<'a>(parsed: &Parsed, files: &'a Files) -> &'a str {
    &files.inner[parsed.top_ids[0].0]
}

/// The `LocalAST`s of the top file, in order.
fn top_locals(parsed: &Parsed) -> Vec<&LocalAST> {
    top(parsed)
        .segments
        .iter()
        .filter_map(|s| match s {
            Segment::Local(l) => Some(&**l),
            _ => None,
        })
        .collect()
}

fn texts<'a>(file: &'a str, spans: &[Span]) -> Vec<&'a str> {
    spans.iter().map(|s| &file[s]).collect()
}

#[tokio::test]
async fn param_function_parsed() {
    // The plan's example: one function and one parameter.
    let (parsed, files) = parse_text("param_func_one", ".param f(a,b)='a*b' g=2\n").await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let file = top_text(&parsed, &files);
    let locals = top_locals(&parsed);
    assert_eq!(locals.len(), 1);
    assert_eq!(locals[0].param_func.len(), 1);
    assert_eq!(locals[0].param.len(), 1);
    let f = &locals[0].param_func[0];
    assert_eq!(&file[&f.name], "f");
    assert_eq!(texts(file, &f.args), ["a", "b"]);
    assert_eq!(&file[&f.body], "a*b");
    assert_eq!(&file[&locals[0].param[0].k], "g");

    // Spaces in the argument list and around `=`, an unquoted body,
    // continuation lines, mixed case, and a function in a subckt.
    let (parsed, files) = parse_text(
            "param_func_more",
            ".PARAM h( x , y ) = x*2+y\n+ k='f(1,2)' one(z)=1\n.subckt s p q\n.param sq(v)='v*v'\nR1 p q 'sq(2)'\n.ends s\n",
        )
        .await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let file = top_text(&parsed, &files);
    let funcs: Vec<(&str, Vec<&str>, &str)> = top_locals(&parsed)
        .iter()
        .flat_map(|l| &l.param_func)
        .map(|f| (&file[&f.name], texts(file, &f.args), &file[&f.body]))
        .collect();
    assert_eq!(
        funcs,
        [("h", vec!["x", "y"], "x*2+y"), ("one", vec!["z"], "1")]
    );
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(ast.param_func.len(), 2);
    assert_eq!(ast.param_func[0].name, "h");
    assert_eq!(ast.param_func[0].args, ["x", "y"]);
    assert_eq!(ast.param_func[0].body, "x*2+y");
    assert_eq!(ast.param.len(), 1);
    assert_eq!(ast.param[0].k, "k");
    assert!(
        ast.to_string()
            .contains("\n.PARAM h(x,y)='x*2+y'\n.PARAM one(z)='1'\n"),
        "{ast}"
    );
    let s = ast.subckt.get("s").expect("subckt kept");
    assert_eq!(s.ast.param_func.len(), 1);
    assert_eq!(s.ast.param_func[0].name, "sq");
    assert_eq!(s.ast.param_func[0].args, ["v"]);
    assert_eq!(s.ast.param_func[0].body, "v*v");
    assert_eq!(names(&s.ast.instance), ["R1"]);
}

#[tokio::test]
async fn global_parsed() {
    let (parsed, files) = parse_text(
        "global",
        ".global vdd gnd!\n+ VSS\n.GLOBAL x1\nR1 vdd 0 1\n",
    )
    .await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let file = top_text(&parsed, &files);
    let global: Vec<&str> = top_locals(&parsed)
        .iter()
        .flat_map(|l| texts(file, &l.global))
        .collect();
    assert_eq!(global, ["vdd", "gnd!", "VSS", "x1"]);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(ast.global, ["vdd", "gnd!", "VSS", "x1"]);
    assert_eq!(names(&ast.instance), ["R1"]);
    assert!(
        ast.to_string().contains("\n.GLOBAL vdd gnd! VSS x1\n"),
        "{ast}"
    );

    // A `.global` with no name is an error.
    let (parsed, files) = parse_text("global_empty", ".global\nR1 a b 1\n").await;
    assert_eq!(error_lines(&parsed), [1]);
    let (ast, _) = files.build(parsed);
    assert!(ast.global.is_empty());
    assert_eq!(names(&ast.instance), ["R1"]);
}

#[tokio::test]
async fn prot_region_skipped() {
    let text = "\
R1 a b 1
.prot
R9 a b 9
#$%^ garbage ((( '
.ends
+ junk
.UNPROT
R2 a b 2
.subckt s a b
R3 a b 3
  .Protect extra text
.subckt hidden x y
R8 x y 8
.unprotect
R4 a b 4
.ends s
";
    let (parsed, files) = parse_text("prot", text).await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let protected: Vec<(u32, u32)> = top_locals(&parsed)
        .iter()
        .flat_map(|l| l.protected.iter().copied())
        .collect();
    assert_eq!(protected, [(2, 7)]);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(names(&ast.instance), ["R1", "R2"]);
    assert_eq!(ast.protected, [(2, 7)]);
    let s = ast.subckt.get("s").expect("subckt kept");
    assert_eq!(names(&s.ast.instance), ["R3", "R4"]);
    assert!(s.ast.subckt.is_empty());
    assert_eq!(s.ast.protected, [(11, 14)]);

    // No `.unprot`: one error, and the region goes to the end.
    let (parsed, files) = parse_text("prot_open", "R1 a b 1\n.prot\nR9 a b 9\n").await;
    assert_eq!(error_lines(&parsed), [2]);
    assert_eq!(error_msgs(&parsed), ["`.prot` has no `.unprot`"]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R1"]);
    assert_eq!(ast.protected, [(2, 3)]);
}

#[tokio::test]
async fn alter_segments() {
    let (parsed, files) = parse_text(
        "alter",
        ".param x=1\n.alter\n.param x=2\n.ALTER second run\nR1 a b 1\n",
    )
    .await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let kinds: Vec<String> = top(&parsed)
        .segments
        .iter()
        .map(|s| match s {
            Segment::Local(l) => format!("local:{}", l.param.len() + l.instance.len()),
            Segment::Alter { index, pos, .. } => {
                format!("alter#{index}@{}", pos.map_or(0, |p| p.line_num))
            }
            Segment::Include { .. } => "include".to_owned(),
            Segment::Cond { .. } => "cond".to_owned(),
        })
        .collect();
    assert_eq!(
        kinds,
        ["local:1", "alter#1@2", "local:1", "alter#2@4", "local:1"]
    );
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(ast.alter_runs, 2);
    // the netlist is the first run: the part before the first `.alter`
    assert_eq!(ast.param.len(), 1);

    // `.alter` inside a `.subckt` is an error; the body goes on.
    let (parsed, files) = parse_text(
        "alter_in_subckt",
        ".subckt s a b\n.alter\nR1 a b 1\n.ends s\n",
    )
    .await;
    assert_eq!(error_lines(&parsed), [2]);
    assert_eq!(error_msgs(&parsed), ["`.alter` inside a `.subckt`"]);
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.alter_runs, 0);
    let s = ast.subckt.get("s").expect("subckt kept");
    assert_eq!(names(&s.ast.instance), ["R1"]);

    // Golden `alter`: two `.alter` blocks after the first run.
    let (parsed, files) = parse_top_multi_with(
        [FileId::Include {
            path: PathBuf::from("golden/cases/alter/deck.sp"),
        }]
        .into_iter(),
        &opts(TitleLine::Auto),
    )
    .await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.alter_runs, 2);
}

/// What a parse with `title_line` did with line 1 of `text`.
#[derive(Debug, PartialEq)]
struct Titled {
    /// `(skipped, line 1)` of the title info
    title: Option<(bool, String)>,
    elements: Vec<String>,
    subckts: Vec<String>,
    options: usize,
    models: usize,
    errors: usize,
}

async fn titled(text: &str, title_line: TitleLine) -> Titled {
    let (parsed, files) = parse_with("title", text, &opts(title_line)).await;
    let file = top_text(&parsed, &files);
    let title = top(&parsed).title.as_ref().map(|t| {
        assert_eq!(t.line.line_num, 1);
        assert!(!t.message.is_empty());
        (t.skipped, file[&t.line].to_owned())
    });
    let errors = errors(&parsed).len();
    let (ast, _) = files.build(parsed);
    Titled {
        title,
        elements: names(&ast.instance),
        subckts: ast.subckt.keys().cloned().collect(),
        options: ast.option.len(),
        models: ast.model.len(),
        errors,
    }
}

fn titled_want(
    title: Option<(bool, &str)>,
    elements: &[&str],
    subckts: &[&str],
    options: usize,
    models: usize,
    errors: usize,
) -> Titled {
    Titled {
        title: title.map(|(s, t)| (s, t.to_owned())),
        elements: elements.iter().map(|s| s.to_string()).collect(),
        subckts: subckts.iter().map(|s| s.to_string()).collect(),
        options,
        models,
        errors,
    }
}

#[tokio::test]
async fn title_auto() {
    use TitleLine::{Auto, No, Yes};
    // An element on line 1 is the title (golden `title_line`).
    assert_eq!(
        titled("R1 a b 1\nR2 a b 2\n", Auto).await,
        titled_want(Some((true, "R1 a b 1")), &["R2"], &[], 0, 0, 0)
    );
    // A dot command on line 1 is the title too (golden `title_dot`).
    assert_eq!(
        titled(".option parhier=local\nR2 a b 2\n", Auto).await,
        titled_want(Some((true, ".option parhier=local")), &["R2"], &[], 0, 0, 0)
    );
    // A library loaded directly: line 1 is parsed.
    assert_eq!(
        titled(".subckt inv a b\nR1 a b 1\n.ends inv\nR2 a b 2\n", Auto).await,
        titled_want(Some((false, ".subckt inv a b")), &["R2"], &["inv"], 0, 0, 0)
    );
    assert_eq!(
        titled("  .SUBCKT inv a b\r\nR1 a b 1\r\n.ends\r\n", Auto).await,
        titled_want(Some((false, "  .SUBCKT inv a b")), &[], &["inv"], 0, 0, 0)
    );
    assert_eq!(
        titled(".Model nch nmos level=1\n", Auto).await,
        titled_want(Some((false, ".Model nch nmos level=1")), &[], &[], 0, 1, 0)
    );
    // `.macro` keeps line 1 too.
    let t = titled(".macro inv a b\n.eom\n", Auto).await;
    assert_eq!(t.title, Some((false, ".macro inv a b".to_owned())));
    // Only the whole keyword counts: `.models` is a title.
    let t = titled(".models x\nR1 a b 1\n", Auto).await;
    assert_eq!(t.title, Some((true, ".models x".to_owned())));
    assert_eq!(t.errors, 0);
    // A comment on line 1 is skipped either way.
    assert_eq!(
        titled("* t\nR2 a b 2\n", Auto).await,
        titled_want(Some((true, "* t")), &["R2"], &[], 0, 0, 0)
    );
    assert_eq!(
        titled("* t\nR2 a b 2\n", No).await,
        titled_want(None, &["R2"], &[], 0, 0, 0)
    );
    // A blank line 1 has no title.
    assert_eq!(
        titled("  \nR1 a b 1\n", Auto).await,
        titled_want(None, &["R1"], &[], 0, 0, 0)
    );
    // A file of one line without a newline.
    assert_eq!(
        titled("R1 a b 1", Auto).await,
        titled_want(Some((true, "R1 a b 1")), &[], &[], 0, 0, 0)
    );
    // `yes` always skips line 1, `no` never does.
    assert_eq!(
        titled(".model nch nmos level=1\nR1 a b 1\n", Yes).await,
        titled_want(
            Some((true, ".model nch nmos level=1")),
            &["R1"],
            &[],
            0,
            0,
            0
        )
    );
    assert_eq!(
        titled("R1 a b 1\nR2 a b 2\n", No).await,
        titled_want(None, &["R1", "R2"], &[], 0, 0, 0)
    );
    // the mode of `parse_text`
    let (parsed, files) = parse_text("title_default", "R1 a b 1\nR2 a b 2\n").await;
    assert!(top(&parsed).title.is_none());
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R1", "R2"]);
}

#[tokio::test]
async fn title_auto_include_and_lib_lines() {
    let tmp = TempDir::new("title_inc");
    let dir = &tmp.0;
    std::fs::write(dir.join("f.sp"), "R1 a b 1\nR2 a b 2\n").unwrap();
    std::fs::write(dir.join("l.sp"), ".lib tt\nR3 a b 3\n.endl tt\n").unwrap();
    for (k, (line1, want)) in [
        (".inc 'f.sp'", &["R1", "R2"][..]),
        (".INCLUDE \"f.sp\"", &["R1", "R2"]),
        (".lib 'l.sp' tt", &["R3"]),
        // included files never have a title line
        ("* top\n.inc 'f.sp'", &["R1", "R2"]),
    ]
    .into_iter()
    .enumerate()
    {
        let top_path = dir.join(format!("top{k}.sp"));
        std::fs::write(&top_path, format!("{line1}\n")).unwrap();
        let (parsed, files) = parse_top_multi_with(
            [FileId::Include { path: top_path }].into_iter(),
            &opts(TitleLine::Auto),
        )
        .await;
        assert!(error_msgs(&parsed).is_empty(), "{line1}");
        let skipped = top(&parsed).title.as_ref().map(|t| t.skipped);
        assert_eq!(skipped, Some(line1.starts_with('*')), "{line1}");
        for (id, (_, ast)) in parsed.inner.iter().enumerate() {
            if id != parsed.top_ids[0].0 {
                assert!(ast.title.is_none(), "{line1}");
            }
        }
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err, "{line1}");
        assert_eq!(names(&ast.instance), want, "{line1}");
    }
}

/// The top-level `.param` values of the golden `lib_corner` deck,
/// parsed with `lib_override` (`(file, from, to)`).
async fn lib_corner_params(lib_override: &[(&str, &str, &str)]) -> Vec<(String, f64)> {
    let opts = ParseOptions {
        title_line: TitleLine::Auto,
        skip_prot: true,
        lib_override: lib_override
            .iter()
            .map(|(file, from, to)| crate::LibOverride {
                file: PathBuf::from(file),
                from: from.to_string(),
                to: to.to_string(),
            })
            .collect(),
    };
    let (parsed, files) = parse_top_multi_with(
        [FileId::Include {
            path: PathBuf::from("golden/cases/lib_corner/deck.sp"),
        }]
        .into_iter(),
        &opts,
    )
    .await;
    assert!(
        error_msgs(&parsed).is_empty(),
        "{lib_override:?}: {:?}",
        error_msgs(&parsed)
    );
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err, "{lib_override:?}");
    ast.param
        .iter()
        .map(|kv| match kv.v {
            Value::Num(f) => (kv.k.to_string(), f),
            Value::Expr(ref e) => panic!("{}: {e}", kv.k),
        })
        .collect()
}

#[tokio::test]
async fn lib_override_selects_section() {
    let p = |a: f64, b: f64| vec![("rbase".to_owned(), a), ("rcorner".to_owned(), b)];
    // The deck's own selection: `tt`.
    assert_eq!(lib_corner_params(&[]).await, p(100.0, 1000.0));
    // The override loads `ff`. The `.lib 'models.lib' base` call in the
    // `ff` section calls another section, so it still loads `base`.
    assert_eq!(
        lib_corner_params(&[("models.lib", "tt", "ff")]).await,
        p(100.0, 2000.0)
    );
    // The file name is matched without its directory, and the names in
    // any letter case.
    assert_eq!(
        lib_corner_params(&[("/elsewhere/MODELS.LIB", "TT", "FF")]).await,
        p(100.0, 2000.0)
    );
    // An override for another file or another section changes nothing.
    assert_eq!(
        lib_corner_params(&[("other.lib", "tt", "ff")]).await,
        p(100.0, 1000.0)
    );
    assert_eq!(
        lib_corner_params(&[("models.lib", "ss", "ff")]).await,
        p(100.0, 1000.0)
    );
}

#[tokio::test]
async fn analysis_cmds_not_errors() {
    let text = "\
.tran 1n 10n
.op
.dc v1 0 1 0.1
.ac dec 10 1 1g
.temp 50
.print tran v(a) i(r1)
.probe tran v(a)
.meas tran t1 trig v(a) val=0.5 rise=1 targ v(b) val=0.5 rise=1
.measure tran x find v(out) when v(in)=0.5
.options post=2
.option probe
.hdl \"model.va\"
.save level=all
.tf v(out) v1
.noise v(out) v1 10
.sens v(out)
.four 1meg v(out)
.ic v(a)=1
.nodeset v(a)=1
.data d
+ p
+ 1
.enddata
.subckt s a b
.OP
R2 a b 2
.ends s
R1 a b 1
.end
";
    let (parsed, files) = parse_text("analysis", text).await;
    assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(names(&ast.instance), ["R1"]);
    let options: Vec<&str> = ast.option.iter().map(|(k, _)| k.as_ref()).collect();
    assert_eq!(options, ["post", "probe"]);
    assert_eq!(ast.init_condition.len(), 1);
    assert_eq!(ast.nodeset.len(), 1);
    assert_eq!(ast.data.len(), 1);
    // The commands are kept with their tokens (`.temp 50`).
    let temp = ast
        .unknown
        .iter()
        .find(|u| u.cmd.eq_ignore_ascii_case("temp"))
        .expect(".temp kept");
    assert!(
        matches!(temp.tokens[..], [Token::Value(Value::Num(t))] if t == 50.0),
        "{:?}",
        temp.tokens
    );

    // An unknown command is still an error.
    let (parsed, files) = parse_text("unknown_cmd", ".foo 1 2\nR1 a b 1\n").await;
    assert_eq!(error_lines(&parsed), [1]);
    assert!(matches!(
        errors(&parsed)[0].err,
        crate::err::ParseErrorInner::Unknown(_)
    ));
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast.instance), ["R1"]);
}
