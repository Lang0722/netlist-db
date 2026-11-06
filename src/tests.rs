//! Regression tests for parser robustness: each test pins down one bug that
//! used to drop, corrupt, or crash on valid input.
use crate::{AST, FileId, Files, Parsed, parser::parse_top_multi};
use core::fmt::Write as _;
use std::path::PathBuf;

/// A unique temporary directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "netlist-db-test-{}-{}-{name}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn write(&self, name: &str, content: &str) -> PathBuf {
        self.write_bytes(name, content.as_bytes())
    }
    fn write_bytes(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn parse_files(paths: &[PathBuf]) -> (Parsed, Files) {
    crate::utlis::test::init_logger();
    parse_top_multi(
        paths
            .iter()
            .map(|path| FileId::Include { path: path.clone() }),
    )
    .await
}

fn count_instances(ast: &AST<'_>) -> usize {
    ast.instance.len()
        + ast
            .subckt
            .values()
            .map(|s| count_instances(&s.ast))
            .sum::<usize>()
}

/// With several top-level files, one file's includes could all finish while
/// another top-level file was still being parsed. The "all done" signal then
/// fired early, and `Files::build` panicked on an include that had not been
/// resolved yet (`src/lib.rs`: `OnceLock::get().unwrap()`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multiple_top_files_do_not_race() {
    const TOP2_LINES: usize = 20_000;
    const BIG_LINES: usize = 60_000;
    let dir = TempDir::new("race");
    dir.write("small.sp", "R0 a b 1k\n");
    let top1 = dir.write("top1.sp", ".inc 'small.sp'\n");
    let mut top2 = String::from("* top2\n");
    for i in 0..TOP2_LINES {
        writeln!(top2, "M{i} d{i} g{i} s{i} b nch W=1u L=0.1u").unwrap();
    }
    top2.push_str(".inc 'big.sp'\n");
    let top2 = dir.write("top2.sp", &top2);
    let mut big = String::new();
    for i in 0..BIG_LINES {
        writeln!(big, "MB{i} d{i} g{i} s{i} b nch W=1u L=0.1u").unwrap();
    }
    dir.write("big.sp", &big);

    let (parsed, files) = parse_files(&[top1, top2]).await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(count_instances(&ast), 1 + TOP2_LINES + BIG_LINES);
}

// ---------------------------------------------------------------------------
// helpers

use crate::{
    ast::{ASTBuilder, Segment, Value},
    err::ParseErrorInner,
    instance::{ControlledKind, Instance, InstanceCtx, Source, source_args},
};
use std::collections::{HashMap, HashSet};

/// A diagnostic recorded while parsing
#[derive(Debug, PartialEq)]
struct Diag {
    kind: &'static str,
    line: Option<u32>,
    warning: bool,
}

fn kind(e: &ParseErrorInner) -> &'static str {
    match e {
        ParseErrorInner::IO(_) => "IO",
        ParseErrorInner::NoLibSection { .. } => "NoLibSection",
        ParseErrorInner::Nom(_) => "Syntax",
        ParseErrorInner::Unknown(_) => "UnknownCommand",
        ParseErrorInner::CircularDefinition(..) => "CircularDefinition",
        ParseErrorInner::TrailingText => "Trailing",
        ParseErrorInner::OrphanContinuation => "OrphanContinuation",
        ParseErrorInner::StrayEnds { .. } => "StrayEnds",
        ParseErrorInner::MissingEnds { .. } => "UnterminatedSubckt",
        ParseErrorInner::StrayCond { .. } => "StrayCond",
        ParseErrorInner::AfterElse { .. } => "AfterElse",
        ParseErrorInner::MissingEndif => "MissingEndif",
        ParseErrorInner::DuplicateSubckt(_) => "DuplicateSubckt",
        ParseErrorInner::Unsupported(_) => "Unsupported",
        ParseErrorInner::NonHspice(_) => "NonHspice",
        ParseErrorInner::Internal(_) => "Internal",
        ParseErrorInner::NestingTooDeep { .. } => "TooDeep",
        ParseErrorInner::Encrypted { .. } => "Encrypted",
        ParseErrorInner::Encoding(_) => "Encoding",
        ParseErrorInner::TitleIsElement => "TitleIsElement",
        ParseErrorInner::AlterInSubckt => "AlterInSubckt",
        ParseErrorInner::MissingUnprot => "MissingUnprot",
        ParseErrorInner::PlusAfterTitle => "PlusAfterTitle",
        ParseErrorInner::NodeCount { .. } => "NodeCount",
        ParseErrorInner::LineInQuotes => "LineInQuotes",
        ParseErrorInner::NoModel { .. } => "NoModel",
        ParseErrorInner::NoSubcktName => "NoSubcktName",
        ParseErrorInner::TooManyValues { .. } => "TooManyValues",
        ParseErrorInner::ValueThenModel { .. } => "ValueThenModel",
        ParseErrorInner::BadPositional { .. } => "BadPositional",
        ParseErrorInner::DigitFirstModel { .. } => "DigitFirstModel",
        ParseErrorInner::NotModelName { .. } => "NotModelName",
        ParseErrorInner::BadSource { .. } => "BadSource",
        ParseErrorInner::NoValue { .. } => "NoValue",
        ParseErrorInner::BadValue { .. } => "BadValue",
    }
}

/// Every diagnostic recorded by the parser (duplicate subckts are found by
/// `Files::build`, so they are not in this list).
fn diagnostics(parsed: &Parsed) -> Vec<Diag> {
    fn walk(ast: &ASTBuilder, out: &mut Vec<Diag>) {
        for seg in &ast.segments {
            match seg {
                Segment::Local(local) => {
                    out.extend(local.errors.iter().map(|e| Diag {
                        kind: kind(&e.err),
                        line: e.pos.map(|p| p.line_num),
                        warning: e.is_warning(),
                    }));
                    for subckt in &local.subckt {
                        walk(&subckt.ast, out);
                    }
                }
                Segment::Alter { .. } | Segment::Cond { .. } => {}
                Segment::Include { res, .. } => {
                    if let Some(Err(e)) = res.get() {
                        out.push(Diag {
                            kind: kind(&e.err),
                            line: e.pos.map(|p| p.line_num),
                            warning: e.is_warning(),
                        });
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for (_, ast) in &parsed.inner {
        walk(ast, &mut out);
    }
    out
}

fn diag(kind: &'static str, line: u32) -> Diag {
    Diag {
        kind,
        line: Some(line),
        warning: matches!(
            kind,
            "UnknownCommand"
                | "DuplicateSubckt"
                | "NonHspice"
                | "Encrypted"
                | "Encoding"
                | "TitleIsElement"
        ),
    }
}

/// Parse `files` (name, content). The first one is the netlist under test,
/// included from a top-level deck: its first line is not the deck's title
/// (HSPICE reads the first line of the deck as its title; see `parse_deck`).
async fn parse_text(files: &[(&str, &str)]) -> (TempDir, Parsed, Files) {
    let dir = TempDir::new("netlist");
    for (name, content) in files {
        dir.write(name, content);
    }
    let deck = dir.write("deck.sp", &format!("* deck\n.inc '{}'\n", files[0].0));
    let (parsed, files) = parse_files(&[deck]).await;
    (dir, parsed, files)
}

/// Parse `files` (name, content); the first one is the top-level deck.
async fn parse_deck(files: &[(&str, &str)]) -> (TempDir, Parsed, Files) {
    let dir = TempDir::new("deck");
    let paths: Vec<_> = files.iter().map(|(n, c)| dir.write(n, c)).collect();
    let (parsed, files) = parse_files(&paths[..1]).await;
    (dir, parsed, files)
}

fn inst<'a, 's>(ast: &'a AST<'s>, name: &str) -> &'a Instance<'s> {
    ast.instance
        .iter()
        .find(|i| i.name.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| panic!("no instance {name} in:\n{ast}"))
}

fn names(ast: &AST<'_>) -> Vec<String> {
    ast.instance.iter().map(|i| i.name.to_string()).collect()
}

fn num(v: &Value<'_>) -> f64 {
    match v {
        Value::Num(f) => *f,
        Value::Expr(e) => panic!("expected a number, got expression '{e}'"),
    }
}

/// The value of an R, C or L element, or the like
fn value_of(v: &Option<Value<'_>>) -> f64 {
    num(v.as_ref().expect("a value"))
}

/// The arguments of a source function, each on its own
fn func_args(s: &Source<'_>) -> Vec<String> {
    let f = s.func.as_ref().expect("a function");
    source_args(&f.args)
        .split([' ', ','])
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * b.abs()
}

fn nets(i: &Instance<'_>) -> Vec<String> {
    let mut map = HashMap::new();
    i.ctx.append_nodes(&mut map);
    let mut v: Vec<_> = map.into_keys().collect();
    v.sort();
    v
}

// ---------------------------------------------------------------------------
// error recovery

/// One unparseable statement used to erase the whole file's AST, including
/// everything parsed before it.
#[tokio::test]
async fn bad_statement_keeps_rest_of_file() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "M1 out in vss vss nch W=1u L=0.1u\n\
         M2 d g\n\
         M3 x y z z nch W=1u L=0.1u\n\
         .subckt INV a y\n\
         M4 y a vss vss nch\n\
         .ends INV\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("NodeCount", 2)]);
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(names(&ast), ["M1", "M3"]);
    assert!(ast.subckt.contains_key("inv"));
}

/// Left-over text used to be parsed as a new statement: `PWL 0 0 1n 1.8`
/// produced a fake instance named `0`, and errors pointed at the next line.
#[tokio::test]
async fn leftover_text_is_reported_not_reparsed() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "M1 d g s b nch W=1u L=0.1u =junk\n\
         + W2=2u\n\
         M2 d g s b nch W=1u L=0.1u\n\
         X1\n\
         M3 d g s b nch W=1u L=0.1u\n",
    )])
    .await;
    // `X1` fails while looking at the next line: still reported on line 4
    assert_eq!(
        diagnostics(&parsed),
        [diag("Trailing", 1), diag("Syntax", 4)]
    );
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["M2", "M3"]);
}

/// A group can start a value without a key too, and a `.endl` needs the end
/// of a word: `.endlx` is an unknown command, as on HSPICE (probes on
/// P-2019.06).
#[tokio::test]
async fn group_first_value_and_endl_word() {
    let (_dir, parsed, files) = parse_text(&[
        ("top.sp", "R1 a b (3k)\nV1 a 0 (1)\n.lib 'm.l' tt\n"),
        ("m.l", ".lib tt\nR2 a b 1\n.endlx\nR3 a b 3\n.endl tt\n"),
    ])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("UnknownCommand", 3)]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R1", "V1", "R2", "R3"]);
    assert_eq!(ast.unknwon.len(), 1);
    // HSPICE rejects a group in a `.temp` line, and reads a value in quotes
    for (text, errors) in [
        (".temp (50)\n", 1),
        (".temp (25+25)\n", 1),
        (".temp 25 (50)\n", 1),
        (".temp '50'\n", 0),
        (".temp 50 $ (a comment)\n", 0),
    ] {
        let (_dir, parsed, _) = parse_text(&[("top.sp", text)]).await;
        assert_eq!(diagnostics(&parsed).len(), errors, "{text}");
    }
}

/// A line inside quotes that is not a `+` line: HSPICE rejects the deck
/// (golden `quoted_cmtline`, `quoted_blank_line`, `quoted_spaces_elem`,
/// `quoted_dollarline_param`). Outside quotes these lines are passed over.
#[tokio::test]
async fn line_inside_quotes_is_an_error() {
    for (text, line) in [
        (".param b='2\n\n+ *3'\n", 1),
        ("R0 a b 1\n.param b='2\n* c\n+ *3'\n", 2),
        (".param b='2\n$ c\n+ *3'\n", 1),
        ("R1 a b '2\n  \t\n+ *3'\n", 1),
        ("M1 d g 0 0 a l='2\n\n+ *1u'\n", 1),
        (".subckt s a w='2\n\n+ *3'\n.ends\n", 1),
        (".model a nmos level='2\n\n+ *3'\n", 1),
    ] {
        let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
        assert_eq!(diagnostics(&parsed), [diag("LineInQuotes", line)], "{text}");
        assert!(files.build(parsed).1, "{text}");
    }
    for text in [
        ".param b='2\n+ *3'\n",
        ".param b=2\n\n+ c=3\n",
        ".param b=2\n* it's\n+ c=3\n",
        ".param b=2 $ it's\n\n+ c=3\n",
        ".param b=2 * it's\n\n.param c=3\n",
        "R1 a b 1k $ it's\n\nR2 a b 1k\n",
        ".param p=max(1,\n\n+ 3)\n",
    ] {
        let (_dir, parsed, _) = parse_text(&[("top.sp", text)]).await;
        assert_eq!(diagnostics(&parsed), [], "{text}");
    }
}

/// A `.ends` without `.subckt` used to end the file, silently.
#[tokio::test]
async fn stray_ends_does_not_truncate_the_file() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "M1 d g s b nch\n.ends\nM2 d g s b nch\n.subckt INV a y\nM3 y a 0 0 nch\n.ends INV\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("StrayEnds", 2)]);
    let (ast, has_err) = files.build(parsed);
    // (HSPICE stops at it with a syntax error)
    assert!(has_err);
    assert_eq!(names(&ast), ["M1", "M2"]);
    assert!(ast.subckt.contains_key("inv"));
}

#[tokio::test]
async fn unterminated_subckt_is_reported() {
    let (_dir, parsed, files) =
        parse_text(&[("top.sp", "R0 a b 1k\n.subckt OPEN a b\nR1 a b 1k\n")]).await;
    assert_eq!(diagnostics(&parsed), [diag("UnterminatedSubckt", 2)]);
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(ast.subckt["open"].ast.instance.len(), 1);
}

/// A second definition used to replace the first one silently.
#[tokio::test]
async fn duplicate_subckt_warns() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".subckt INV a y\nM1 y a 0 0 nch\n.ends\n.subckt inv a y\nR1 a y 1k\n.ends\n",
    )])
    .await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err, "a warning is not an error");
    assert_eq!(ast.subckt.len(), 1);
    // unchanged behavior: the later definition wins
    assert!(matches!(
        ast.subckt["inv"].ast.instance[0].ctx,
        InstanceCtx::Resistor(_)
    ));
}

/// Blocks holding only `.ic`/`.nodeset` used to be dropped as "empty".
#[tokio::test]
async fn ic_and_nodeset_are_kept() {
    for text in [
        ".ic V(a)=1\n.nodeset b=0.5\n",
        ".ic V(a)=1\n.nodeset b=0.5\n.inc 'small.sp'\n",
    ] {
        let (_dir, parsed, files) =
            parse_text(&[("top.sp", text), ("small.sp", "R0 a b 1k\n")]).await;
        assert_eq!(diagnostics(&parsed), []);
        let (ast, _) = files.build(parsed);
        assert_eq!(ast.init_condition.len(), 1, "{text}");
        assert_eq!(ast.nodeset.len(), 1, "{text}");
    }
}

#[tokio::test]
async fn missing_top_file_keeps_the_others() {
    let dir = TempDir::new("missing");
    let good = dir.write("good.sp", "* good\nR0 a b 1k\n");
    let (parsed, files) = parse_files(&[dir.0.join("missing.sp"), good]).await;
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(names(&ast), ["R0"]);
}

// ---------------------------------------------------------------------------
// values and names

#[tokio::test]
async fn scale_factors_follow_hspice() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".param milli=1m milli2=1M mega=1meg mega2=1Megohm kilo=10kohm\n\
         + pf=10pF ff=0.5fF volt=1.8V ns=5ns mil=1mil db=20db minute=1min dot=.5p\n\
         + e=nand i=info\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let param = |k: &str| &ast.param.iter().find(|kv| kv.k == k).unwrap().v;
    for (k, v) in [
        ("milli", 1e-3),
        ("milli2", 1e-3),
        ("mega", 1e6),
        ("mega2", 1e6),
        ("kilo", 1e4),
        ("pf", 1e-11),
        ("ff", 5e-16),
        ("volt", 1.8),
        ("ns", 5e-9),
        ("mil", 25.4e-6),
        ("db", 10.0),
        ("minute", 60.0),
        ("dot", 5e-13),
    ] {
        assert!(close(num(param(k)), v), "{k}: {:?} != {v}", param(k));
    }
    // names are not NaN or infinity
    assert!(matches!(param("e"), Value::Expr(e) if e == "nand"));
    assert!(matches!(param("i"), Value::Expr(e) if e == "info"));
}

#[tokio::test]
async fn unquoted_function_calls_are_expressions() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".param a=agauss(0,1,3) b=2*(x+y)\nX1 n1 n2 sub w=max(1u,2u)\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert!(matches!(&ast.param[0].v, Value::Expr(e) if e == "agauss(0,1,3)"));
    assert!(matches!(&ast.param[1].v, Value::Expr(e) if e == "2*(x+y)"));
    let InstanceCtx::Subckt(x) = &inst(&ast, "X1").ctx else {
        panic!()
    };
    assert!(matches!(&x.params[0].v, Value::Expr(e) if e == "max(1u,2u)"));
}

/// CDL bus bits (`A<0>`), global nets (`VDD!`) and `/` before the subckt
/// name used to wipe the file.
#[tokio::test]
async fn cdl_names() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "*.PININFO A<0>:I A<1>:I Y:O\n\
         .SUBCKT NAND2 A<0> A<1> Y VDD! VSS!\n\
         MM0 Y A<0> VDD! VDD! pch W=1u L=0.1u\n\
         MM1 Y A<1> net1 VSS! nch W=1u L=0.1u\n\
         .ENDS\n\
         X1 a<0> a<1> y VDD! VSS! / NAND2\n\
         X2 a<0> a<1> y VDD! VSS! /NAND2 $PINS A<0>=a<0>\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(
        ast.subckt["nand2"].ports,
        ["A<0>", "A<1>", "Y", "VDD!", "VSS!"]
    );
    for name in ["X1", "X2"] {
        let InstanceCtx::Subckt(x) = &inst(&ast, name).ctx else {
            panic!()
        };
        assert_eq!(x.cktname, "NAND2");
        assert_eq!(x.nodes, ["a<0>", "a<1>", "y", "VDD!", "VSS!"]);
    }
}

#[tokio::test]
async fn binned_models_and_models_without_params() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nch.1 nmos level=54 lmin=1e-8\n.model dmod d\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.model[0].name, "nch.1");
    assert_eq!(ast.model[0].params.len(), 2);
    assert_eq!(ast.model[1].name, "dmod");
    assert!(ast.model[1].params.is_empty());
}

#[tokio::test]
async fn ic_and_nodeset_forms() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".nodeset V(out)=0.9\n.nodeset a 1 b=2\n.ic out=1 V(x) = 0 subckt=INV\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let nodes: Vec<_> = ast.nodeset.iter().map(|n| n.0.to_string()).collect();
    assert_eq!(nodes, ["out", "a", "b"]);
    assert_eq!(ast.init_condition.len(), 2);
    assert!(
        ast.init_condition
            .iter()
            .all(|ic| ic.2.as_deref() == Some("INV"))
    );
}

#[tokio::test]
async fn double_quoted_include() {
    let (_dir, parsed, files) = parse_text(&[
        ("top.sp", ".include \"small.sp\"\n"),
        ("small.sp", "R0 a b 1k\n"),
    ])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(names(&ast), ["R0"]);
}

// ---------------------------------------------------------------------------
// elements

#[tokio::test]
async fn resistor_capacitor_inductor_params_and_models() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "R1 a b 1k m=2 tc1=1e-3\nR2 a b r=2k\nR3 a b rmod r=3k\nR4 a b rmod 4k w=1u\n\
         C1 a b cmod 1f\nC2 a b c=2f m=4\nL1 a b 1n lmod\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let r = |name| match &inst(&ast, name).ctx {
        InstanceCtx::Resistor(r) => r,
        _ => panic!(),
    };
    assert!(close(value_of(&r("R1").value), 1e3));
    assert_eq!(r("R1").params.len(), 2);
    assert!(close(value_of(&r("R2").value), 2e3) && r("R2").params.is_empty());
    assert_eq!(r("R3").model.as_deref(), Some("rmod"));
    assert!(close(value_of(&r("R3").value), 3e3));
    assert_eq!(r("R4").model.as_deref(), Some("rmod"));
    assert!(close(value_of(&r("R4").value), 4e3) && r("R4").params.len() == 1);
    let InstanceCtx::Capacitor(c1) = &inst(&ast, "C1").ctx else {
        panic!()
    };
    assert_eq!(c1.model.as_deref(), Some("cmod"));
    assert!(close(value_of(&c1.value), 1e-15));
    let InstanceCtx::Capacitor(c2) = &inst(&ast, "C2").ctx else {
        panic!()
    };
    assert!(close(value_of(&c2.value), 2e-15) && c2.params.len() == 1);
    let InstanceCtx::Inductor(l1) = &inst(&ast, "L1").ctx else {
        panic!()
    };
    assert_eq!(l1.model.as_deref(), Some("lmod"));
    assert!(close(value_of(&l1.value), 1e-9));
}

/// `DC`, `AC`, transient functions and PWL without parentheses used to be
/// misread or wipe the file.
#[tokio::test]
async fn independent_sources() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "V1 vdd 0 DC 1.8\n\
         V2 in 0 PULSE(0 1.8 0 10p 10p 1n 2n)\n\
         V3 in 0 PULSE 0 1.8 0 10p 10p 1n 2n\n\
         V4 in 0 DC 0 AC 1 90 SIN(0, 1, 1meg)\n\
         V5 in 0 PWL 0 0 1n 1.8 R TD=1n\n\
         V6 in 0 PWL(0 0\n+ 1n 1.8)\n\
         I1 0 a 1m\n\
         I2 0 a AC=1\n\
         V7 vdd 0 'vsupply' M=2\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.instance.len(), 9, "no fake instances:\n{ast}");
    let v = |name| match &inst(&ast, name).ctx {
        InstanceCtx::Voltage(v) => &v.source,
        _ => panic!(),
    };
    let dc = |s: &Source<'_>| num(&s.dc.as_ref().expect("a DC value").1);
    assert!(close(dc(v("V1")), 1.8));
    for name in ["V2", "V3"] {
        let f = v(name).func.as_ref().expect(name);
        assert_eq!(f.name.to_uppercase(), "PULSE");
        let args = func_args(v(name));
        assert_eq!(args.len(), 7);
        assert_eq!(args[1], "1.8");
    }
    let v4 = v("V4");
    assert_eq!(dc(v4), 0.0);
    let (mag, phase) = (v4.ac_mag.as_ref().unwrap(), v4.ac_phase.as_ref().unwrap());
    assert!(close(num(&mag.1), 1.0) && close(num(&phase.1), 90.0));
    assert_eq!(func_args(v4).len(), 3);
    let v5 = v("V5");
    assert_eq!(func_args(v5).len(), 4);
    let keys: Vec<_> = v5.params.iter().map(|kv| kv.k.to_string()).collect();
    assert_eq!(keys, ["R", "TD"]);
    assert_eq!(func_args(v("V6")).len(), 4);
    let i = |name| match &inst(&ast, name).ctx {
        InstanceCtx::Current(i) => &i.source,
        _ => panic!(),
    };
    assert!(close(dc(i("I1")), 1e-3));
    assert!(i("I2").ac_mag.is_some());
    let v7 = v("V7");
    assert!(matches!(&v7.dc, Some((_, Value::Expr(e))) if e == "vsupply"));
    assert_eq!(v7.params.len(), 1);
}

/// E/F/G/H/K used to be generic elements whose gains (and coupled inductor
/// names) showed up as nodes.
#[tokio::test]
async fn controlled_sources_and_mutual_inductors() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "E1 out 0 in 0 10\n\
         G1 out 0 VCCS in 0 1m MAX=1\n\
         E2 out 0 POLY(2) a 0 b 0 0 1 1\n\
         F1 out 0 vsense 2\n\
         H1 out 0 POLY(2) v1 v2 0 1k 1k\n\
         E3 out 0 VOL='V(a)*2'\n\
         K1 L1 L2 0.95\n\
         K2 L1 L3 K=0.5\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let c = |name| match &inst(&ast, name).ctx {
        InstanceCtx::ControlledSource(c) => c,
        _ => panic!("{name}"),
    };
    assert_eq!(c("E1").kind, ControlledKind::E);
    assert!(close(num(&c("E1").coeffs[0]), 10.0));
    assert_eq!(nets(inst(&ast, "E1")), ["0", "in", "out"]);
    assert_eq!(c("G1").keyword.as_deref(), Some("VCCS"));
    assert!(close(num(&c("G1").coeffs[0]), 1e-3) && c("G1").params.len() == 1);
    assert_eq!(c("E2").poly, Some(2));
    assert_eq!(c("E2").ctrl_nodes.len(), 2);
    assert_eq!(c("E2").coeffs.len(), 3);
    assert_eq!(nets(inst(&ast, "E2")), ["0", "a", "b", "out"]);
    assert_eq!(c("F1").ctrl_sources, ["vsense"]);
    // the controlling source is an element, not a node
    assert_eq!(nets(inst(&ast, "F1")), ["0", "out"]);
    assert_eq!(c("H1").ctrl_sources, ["v1", "v2"]);
    assert!(c("E3").coeffs.is_empty() && c("E3").params.len() == 1);
    for name in ["K1", "K2"] {
        let InstanceCtx::MutualInductor(k) = &inst(&ast, name).ctx else {
            panic!()
        };
        assert_eq!(k.l1, "L1");
        assert!(nets(inst(&ast, name)).is_empty());
    }
}

/// Forms that are not supported yet are reported instead of misread.
#[tokio::test]
async fn unsupported_controlled_form_is_reported() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "* title\nE1 out 0 DELAY in 0 TD=1n\nH1 out 0 LAPLACE vin 1 0 / 1 1\nR1 a b 1k\n",
    )])
    .await;
    assert_eq!(
        diagnostics(&parsed),
        [diag("Unsupported", 2), diag("Unsupported", 3)]
    );
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R1"]);
}

// ---------------------------------------------------------------------------
// commands and output

/// Valid HSPICE control cards used to be reported as errors, and were written
/// back with quoted tokens (`.print 'tran'`).
#[tokio::test]
async fn control_cards_are_kept_verbatim() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".global vdd vss\n.temp 25\n.tran 1p 10n SWEEP data=d1\n.op\n.options post=2 probe\n\
         .print tran v(out) i(V1)\n.probe tran v(out) par('v(out)*2')\n\
         .meas tran tdel trig v(in) val=0.9 rise=1\n+ targ v(out) val=0.9 fall=1\n\
         .foo bar\n",
    )])
    .await;
    // only the truly unknown `.foo` is reported, as a warning
    assert_eq!(diagnostics(&parsed), [diag("UnknownCommand", 10)]);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(ast.option.len(), 2);
    let text = ast.to_string();
    for line in [
        ".GLOBAL vdd vss",
        ".temp 25",
        ".tran 1p 10n SWEEP data=d1",
        ".op",
        ".print tran v(out) i(V1)",
        ".probe tran v(out) par('v(out)*2')",
        ".meas tran tdel trig v(in) val=0.9 rise=1\n+ targ v(out) val=0.9 fall=1",
        ".foo bar",
    ] {
        assert!(text.contains(line), "missing `{line}` in:\n{text}");
    }
}

/// Printing used to produce invalid SPICE (`w=1e-6.OPTION`, `suckt=`). The
/// output must parse back without diagnostics, to the same output.
#[tokio::test]
async fn display_round_trips() {
    const DECK: &str = "\
.option post=2
.param vsupply=1.8 w0='2*lmin'
.model nch.1 nmos level=54
.subckt INV a y vdd vss w=1u
.option scale=1
M1 y a vss vss nch W=w L=0.1u
M2 y a vdd vdd pch W='2*w' L=0.1u
.ends INV
X1 in out vdd 0 INV w=2u
Xcdl a<0> b VDD! 0 / INV
R1 a b rmod 1k m=2
L1 a b 1n lmod
C1 a 0 10fF
V1 vdd 0 DC 1.8
V2 in 0 DC 0 PULSE(0 1.8 0 10p 10p 1n 2n) AC 1 90 M=1
V3 in 0 PWL(0 0 1n 1.8 R TD=1n)
I1 0 a SIN(0 1m 1meg)
I2 0 a PAT(1 0 0 1n 1n 5n b1010 r=1)
E1 out 0 POLY(2) a 0 b 0 0 1 1
F1 out 0 vsense 2
E2 out 0 VOL='V(a)*2'
K1 L1 L2 0.95
Q1 c b e npn area=2
D1 a 0 dmod
J1 d g s jmod
.ic V(a)=1 subckt=INV
.nodeset b=0.5
.data d1
+ p1 p2
+ 1 2
+ 3 4
.enddata
.global vdd
.tran 1p 10n
.meas tran t1 trig v(in) val=0.9 rise=1
+ targ v(out) val=0.9 fall=1
";
    let (_dir, parsed, files) = parse_text(&[("top.sp", DECK)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    let first = ast.to_string();
    assert!(!first.contains("suckt"), "{first}");
    let (_dir2, parsed2, files2) = parse_text(&[("top.sp", &first)]).await;
    assert_eq!(diagnostics(&parsed2), [], "printed netlist:\n{first}");
    let (ast2, has_err2) = files2.build(parsed2);
    assert!(!has_err2);
    assert_eq!(count_instances(&ast2), count_instances(&ast));
    assert_eq!(ast2.to_string(), first);
}

/// Text after `.ends` / `.end` is ignored, as in HSPICE.
#[tokio::test]
async fn text_after_ends_and_end_is_ignored() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".subckt PM_INV%A a y\nR1 a y 1k\n.ends PM_INV%A * note\nR2 a b 1k\n.end 'title' 42\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert!(ast.subckt.contains_key("pm_inv%a"));
    assert_eq!(names(&ast), ["R2"]);
}

// ---------------------------------------------------------------------------
// dialect details found in real netlists (Infineon adsbenchmark)

/// Infineon's model files start with `simulator lang = spice`, which used to
/// become a fake element named `simulator`. It is a Spectre directive, not
/// HSPICE syntax: it is ignored, with a warning.
#[tokio::test]
async fn simulator_lang_spice_is_ignored_with_a_warning() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        // header of adsbenchmark/*/src/model/bip_nom.scs
        "simulator lang = spice\n\
         .model latpnp pnp \n\
         + is=1.8e-15 bf=120 rb=500 re=50 vaf=5  var=3 nf=1.25 ikf=3e-5 tf=100e-12\n\
         + cje=2.5e-15 vje=0.9 mje=0.55 cjc=25e-15 vjc=0.9 mjc=0.55\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("NonHspice", 1)]);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err, "a warning is not an error");
    assert!(
        ast.instance.is_empty(),
        "no element named `simulator`:\n{ast}"
    );
    assert_eq!(ast.model.len(), 1);
    assert_eq!(ast.model[0].params.len(), 15);
}

/// A Spectre section can't be read as SPICE: it is reported and skipped up to
/// `simulator lang=spice`, or to the end of the file.
#[tokio::test]
async fn simulator_lang_spectre_section_is_reported_and_skipped() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "R1 a b 1k\n\
         simulator lang=spectre\n\
         r2 (a b) resistor r=2k\n\
         parameters x=1\n\
         Simulator Lang=SPICE\n\
         R3 a b 3k\n\
         simulator lang=spectre\n\
         r4 (a b) resistor r=4k\n",
    )])
    .await;
    assert_eq!(
        diagnostics(&parsed),
        [
            diag("Unsupported", 2),
            diag("NonHspice", 5),
            diag("Unsupported", 7)
        ]
    );
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(names(&ast), ["R1", "R3"]);
}

/// HSPICE continues a statement with a trailing `\` (the line break counts as
/// white space). Infineon's PHY testbench continues a `.PARAM` line this way,
/// and the statement used to be dropped.
#[tokio::test]
async fn backslash_continues_a_line() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        // adsbenchmark/PHY/src/top.hsp, lines 17-18
        ".PARAM pll_wait_time=100n rbulkpanalog=1m rbulkpcore=1m rhsupanalog=1m rhsupcore=1m \\\n\
         \t\ttd_ps=1n tr_ps=1n vsup1v5=1.5 vsup2v5=2.5 phy_test=1\n\
         M1 d g s b nch W=1u \\\n\
         L=0.1u\n\
         .tran 1n \\\n\
         10n\n\
         .param bad=1 ( \\\n\
         c=2\n\
         R1 a b 1k $ note \\\n\
         R2 a b 2k\n",
    )])
    .await;
    // the bad statement is skipped together with its continuation line, and a
    // `\` inside a `$` comment continues nothing
    assert_eq!(diagnostics(&parsed), [diag("Trailing", 7)]);
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.param.len(), 10);
    assert_eq!(names(&ast), ["M1", "R1", "R2"]);
    let InstanceCtx::MOSFET(m) = &inst(&ast, "M1").ctx else {
        panic!()
    };
    assert_eq!(m.params.len(), 2);
    // the lines are joined when the file is read
    assert!(ast.to_string().contains(".tran 1n  10n"), "{ast}");
}

/// HSPICE: a trailing `\\` joins the lines with no white space (e.g. to split
/// a long path), and both forms also work inside quotes. Line numbers after
/// the joined lines stay right.
#[tokio::test]
async fn double_backslash_joins_lines_without_white_space() {
    let (_dir, parsed, files) = parse_text(&[
        (
            "top.sp",
            ".inc 'sm\\\\\n\
             \x20   all.sp'\n\
             .param y=12\\\\\n\
             34 z='1 + \\\n\
             2'\n\
             M9 d g\n",
        ),
        ("small.sp", "R0 a b 1k\n"),
    ])
    .await;
    // `M9 d g` (too few nodes) is on line 6 of the file
    assert_eq!(diagnostics(&parsed), [diag("NodeCount", 6)]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R0"], "the split include path resolves");
    let param = |k: &str| &ast.param.iter().find(|kv| kv.k == k).unwrap().v;
    assert!(close(num(param("y")), 1234.0));
    assert!(
        matches!(param("z"), Value::Expr(e) if !e.contains(['\\', '\n'])),
        "{:?}",
        param("z")
    );
}

/// After a `\`, a `+` starting the next line is a continuation mark, except
/// inside quotes, where it is an operator.
#[tokio::test]
async fn backslash_then_plus() {
    let (_dir, parsed, files) = parse_text(&[("top.sp", ".param a=1 \\\n+ b='1 \\\n+ 2'\n")]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.param.len(), 2);
    assert!(
        matches!(&ast.param[1].v, Value::Expr(e) if e.contains("+ 2")),
        "{ast}"
    );
}

// ---------------------------------------------------------------------------
// stability, and HSPICE syntax found in the HSPICE corpus

/// A `.subckt` nested in the one before it, 400 deep (as in a cell library
/// whose `.ends` lines are missing), used to overflow the stack and abort.
#[tokio::test]
async fn deep_subckt_nesting_does_not_overflow_the_stack() {
    use crate::parser::MAX_NESTING;
    const DEPTH: usize = 400;
    let mut text = String::new();
    for i in 0..DEPTH {
        writeln!(text, ".subckt s{i} a").unwrap();
    }
    text.push_str("R1 a 0 1k\n");
    text.push_str(&".ends\n".repeat(DEPTH));
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    let diags = diagnostics(&parsed);
    let too_deep: Vec<_> = diags.iter().filter(|d| d.kind == "TooDeep").collect();
    // one error, for the first `.subckt` too deep, on its own line; the part
    // below it is skipped
    assert_eq!(too_deep, [&diag("TooDeep", MAX_NESTING as u32 + 1)]);
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(count_instances(&ast), 0);
    // building and printing it do not overflow either
    assert!(ast.to_string().contains(".SUBCKT s0 a"), "{ast}");

    // a cell library without `.ends`: the cells up to the limit are kept, and
    // the error names the limit
    let mut lib = String::new();
    for i in 0..500 {
        writeln!(lib, ".subckt cell{i} a b\nM1 a b 0 0 nch w=1u l=1u").unwrap();
    }
    let (_dir, parsed, files) = parse_text(&[("lib.sp", &lib)]).await;
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(count_instances(&ast), MAX_NESTING);
}

/// `Subckt::append` on a subckt that instantiates itself used to recurse
/// until the stack overflowed.
#[tokio::test]
async fn recursive_subckt_instantiation_is_an_error() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".subckt a x\nX1 x b\n.ends\n.subckt b x\nX2 x a\n.ends\n\
         .subckt c x\nX3 x c\n.ends\n.subckt d x\nX4 x b\n.ends\n",
    )])
    .await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    let append = |name: &str| {
        crate::Subckt::append(
            name,
            |_| false,
            &mut Vec::new(),
            &mut vec![&ast.subckt],
            &mut Vec::new(),
            &mut Vec::new(),
        )
    };
    assert_eq!(
        append("a"),
        Err("X1.X2.a: subckt `a` instantiates itself (a -> b -> a)".to_owned())
    );
    assert_eq!(
        append("c"),
        Err("X3.c: subckt `c` instantiates itself (c -> c)".to_owned())
    );
    assert_eq!(
        append("d"),
        Err("X4.X2.X1.b: subckt `b` instantiates itself (b -> a -> b)".to_owned())
    );
}

/// Line ends of classic Mac OS (CR only) used to make the whole file one
/// line, a UTF-8 byte order mark broke the first statement, and one byte that
/// is not UTF-8 (a Latin-1 `µ` in a comment) made the whole file unreadable.
#[tokio::test]
async fn cr_line_ends_bom_and_latin1_are_read() {
    let dir = TempDir::new("encoding");
    dir.write_bytes("cr.sp", b"R1 a b 1k\rC1 b 0 1p\rV1 a 0 1.8\r");
    // (and one inside the file, as when files are joined)
    dir.write_bytes("bom.sp", b"\xef\xbb\xbfR2 a b 1k\n\xef\xbb\xbfC2 b 0 1p\n");
    dir.write_bytes("latin1.sp", b"* bias 10 \xb5A\nR3 a b 1k\n");
    // a deck whose title line follows a byte order mark
    let deck = dir.write_bytes(
        "deck.sp",
        b"\xef\xbb\xbf* deck\n.inc 'cr.sp'\n.inc 'bom.sp'\n.inc 'latin1.sp'\n",
    );
    let (parsed, files) = parse_files(&[deck]).await;
    assert_eq!(
        diagnostics(&parsed),
        [Diag {
            kind: "Encoding",
            line: None,
            warning: true
        }]
    );
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(names(&ast), ["R1", "C1", "V1", "R2", "C2", "R3"]);
}

/// The sections of a library file are read from one reading of it (a PDK
/// model file of tens of MB used to be read and kept in memory once per
/// section, all at once): sections loaded from a corner, a missing section,
/// a library that is not UTF-8, and one that does not exist.
#[tokio::test]
async fn library_sections_share_one_reading() {
    let dir = TempDir::new("lib");
    dir.write_bytes(
        "models.l",
        b"* library\n.lib tt\n* \xb5\n.param d=0\n.lib 'models.l' core\n.lib 'models.l' io\n.endl tt\n\
          .lib core\n.model nch.1 nmos lmin=1e-8 lmax=1e-6 wmin=1e-8 wmax=1\n\
          .model nch.2 nmos lmin=1e-6 lmax=1e-4 wmin=1e-8 wmax=1\n.endl core\n\
          .lib io\n.model nio nmos level=54\n.endl io\n",
    );
    let deck = dir.write_bytes(
        "deck.sp",
        b"* deck\n.lib 'models.l' tt\n.lib 'models.l' rf\n.lib 'nothing.l' tt\n.lib 'nothing.l' ss\n\
          M1 d g s b nch l=2u w=1u\nM2 d g s b nio l=1u w=1u\n",
    );
    let (parsed, files) = parse_files(&[deck]).await;
    let mut kinds: Vec<_> = diagnostics(&parsed).iter().map(|d| d.kind).collect();
    kinds.sort();
    // the encoding warning of the section with the bad byte (tt), the missing
    // section, and each section of the missing file
    assert_eq!(kinds, ["Encoding", "IO", "IO", "NoLibSection"]);
    let (ast, _) = files.build(parsed);
    let models: Vec<_> = ast.model.iter().map(|m| m.name.to_string()).collect();
    assert_eq!(models, ["nch.1", "nch.2", "nio"]);
    let chosen: Vec<_> = ast
        .mosfet_models()
        .into_iter()
        .map(|m| m.selection.unwrap().model.name.to_string())
        .collect();
    assert_eq!(chosen, ["nch.2", "nio"]);
}

/// A quote that was not closed used to take in every line up to the next
/// quote in the file (here, two resistors), and the error pointed there.
#[tokio::test]
async fn unterminated_quote_stays_in_its_statement() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "R0 a b 1k\n.param x='1+2\nR1 a b 1k\nR2 a b 1k\n\
         .param y='3' z='1+\n+ 2'\nR3 a b 1k\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("Trailing", 2)]);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R0", "R1", "R2", "R3"]);
    // a quoted expression may still go on over a continuation line
    assert_eq!(ast.param.len(), 2);
}

/// A source with no value (an ammeter) used to be an error when a comment or
/// a space followed its nodes.
#[tokio::test]
async fn source_without_value() {
    let (_dir, parsed, files) =
        parse_text(&[("top.sp", "V1 a 0 $ ammeter\nVm a b \nI1 c d\n")]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["V1", "Vm", "I1"]);
    let InstanceCtx::Voltage(v) = &inst(&ast, "V1").ctx else {
        panic!()
    };
    let s = &v.source;
    assert!(s.dc.is_none() && s.ac_mag.is_none() && s.func.is_none() && s.params.is_empty());
    assert!(
        ast.to_string().contains("\nV1 a 0\nVm a b\nI1 c d"),
        "{ast}"
    );
}

/// Values that used to print as text that parses back differently: a number
/// too large for `f64` (printed `inf`), a `.data` file name with a `'`, a
/// parameter named `*e` (a comment), and a function call with a quoted
/// argument (printed in nested quotes).
#[tokio::test]
async fn awkward_values_print_back_the_same() {
    const DECK: &str = "\
R1 a b 1e999
R2 a b 50 TC=0,0
.param a=1 b=2 c=3 d=4 *e=5
.param val2=agauss(0,0,'2*p1')
.param delay = OPT1 ( 0n, 0n, 5n )
.data d MER
+ FILE=\"it's.dat\" p1=1
.enddata
";
    let (_dir, parsed, files) = parse_text(&[("top.sp", DECK)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    // `*e=5` is a comment; `1e999` is kept as written
    assert_eq!(ast.param.len(), 6);
    let InstanceCtx::Resistor(r) = &inst(&ast, "R1").ctx else {
        panic!()
    };
    assert!(matches!(&r.value, Some(Value::Expr(e)) if e == "1e999"));
    let first = ast.to_string();
    assert!(first.contains("TC=0,0"), "{first}");
    assert!(first.contains("delay=OPT1 ( 0n, 0n, 5n )"), "{first}");
    assert!(first.contains("val2=agauss(0,0,'2*p1')"), "{first}");
    let (_dir2, parsed2, files2) = parse_deck(&[("top.sp", &first)]).await;
    assert_eq!(diagnostics(&parsed2), [], "printed netlist:\n{first}");
    let (ast2, _) = files2.build(parsed2);
    assert_eq!(ast2.to_string(), first);

    // HSPICE rejects a quote in a function call without quotes around it
    // ("cannot use quotation marks"), but not in a statistical function or in
    // `str`, when the call is the whole value. It also reads a group at the
    // start of a value (probes on P-2019.06).
    for text in [
        ".param v=max(1,'2')\n",
        ".param v=sin('p')\n",
        ".param v=(1+'2')\n",
        ".param v=2*agauss(0,0,'2')\n",
        ".param v=agauss(0,0,'2')*2\n",
        ".param v=max(1,agauss(0,0,'2'))\n",
        ".param v=agauss(0,0,max(1,'2'))\n",
        ".param v='max'(1,2)\n",
    ] {
        let (_dir, parsed, _) = parse_text(&[("top.sp", text)]).await;
        assert_eq!(diagnostics(&parsed), [diag("Trailing", 1)], "{text}");
    }
    for text in [
        ".param v=AGAUSS('1', 0, '2') w=2\n",
        ".param v=agauss(0,0,\n+ '2')\n",
        ".param v=gauss(1,0.1,'2')\n.param w=aunif(1,0.1,'2')\n",
        ".param v=unif(1,0.1,'2')\n.param w=limit(1,0.1,'2')\n",
        ".param v=str('x')\n.param w=str(\"x\")\n",
        ".subckt s a w=agauss(1u,0,'2') f=str('x')\n.ends\nX1 a s w=agauss(1u,0,'2') f=str('x')\n",
        "V1 a 0 PWL PWLFILE=str('x.pwl')\n",
    ] {
        let (_dir, parsed, _) = parse_text(&[("top.sp", text)]).await;
        assert_eq!(diagnostics(&parsed), [], "{text}");
    }
    // ... and such a value has its nominal value
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model a nmos level=54\n.param q=agauss(1u,0,'2')\nM1 d g 0 0 a l=q w=1u\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    assert_sizes(&files.build(parsed).0, &[("M1", 1.0, 1.0)], "agauss");
    // HSPICE takes commas in `.global`
    let (_dir, parsed, files) = parse_text(&[("top.sp", ".global a,b, c\n+ d,\n+ e\n")]).await;
    assert_eq!(diagnostics(&parsed), []);
    assert_eq!(files.build(parsed).0.global.len(), 5);
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".param p=(1+2)*1k q=max(1k, 3k)\nX1 a b s w=(3k)\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.param.len(), 2);
    assert!(matches!(&ast.param[1].v, Value::Expr(e) if e == "max(1k, 3k)"));
}

/// HSPICE also delimits expressions with double quotes (2,641 `X` lines in
/// the HSPICE corpus were rejected).
#[tokio::test]
async fn double_quoted_values() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "XNAND2_0 NA1 NA0 Y vdd gnd NAND2 size = \"2\" Lg = '20n'\n\
         X2 a b y vdd gnd NAND2 SN=\"SN_NAND2\" SP=\"1\"\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let InstanceCtx::Subckt(x) = &inst(&ast, "XNAND2_0").ctx else {
        panic!()
    };
    assert_eq!((x.cktname.as_ref(), x.nodes.len()), ("NAND2", 5));
    assert!(matches!(&x.params[0].v, Value::Expr(e) if e == "2"));
    let InstanceCtx::Subckt(x) = &inst(&ast, "X2").ctx else {
        panic!()
    };
    assert!(matches!(&x.params[0].v, Value::Expr(e) if e == "SN_NAND2"));
}

/// The rows of a `.data` block may be written without `+` (about 400 rows in
/// the HSPICE corpus were rejected); without `.enddata`, only `+` lines
/// belong to the block.
#[tokio::test]
async fn data_rows_without_plus() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".data vectorload\nload\n0.004f\n0.012f\n.enddata\n\
         .data truth bit0 bit1\n0 0 $ first row\n* a comment\n+ 1 0\n.enddata\n\
         .data open p\n+ 1\nR1 a b 1k\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R1"]);
    let shape: Vec<_> = ast
        .data
        .iter()
        .map(|d| match &d.values {
            crate::ast::DataValues::InlineNum { params, values } => (params.len(), values.len()),
            _ => panic!(),
        })
        .collect();
    assert_eq!(shape, [(1, 2), (2, 4), (1, 1)]);
}

/// `PWL(1)` controlled sources (166 statements in the HSPICE corpus) used to
/// be reported as not supported.
#[tokio::test]
async fn pwl_controlled_sources() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "G_Switch BE 0 vcr PWL(1) ctrl 0 2.6v,1m 2.4v,100g\n\
         e1 s_clip 0 pwl(1) s 0 DELTA=0.01 -0.1,-0.1 0.1,0.1\n\
         G1 IOUT+ IOUT- VCR PWL(1) VC+ VC-\n\
         +(0,     2.39E-05)\n\
         +(40.33, 1.05E-03)\n\
         F1 a b NAND(2) v1 v2 0,0 1,1 SCALE=2\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let controlled = |name: &str| match &inst(&ast, name).ctx {
        InstanceCtx::ControlledSource(c) => c.clone(),
        _ => panic!(),
    };
    let g = controlled("G_Switch");
    let pwl = g.pwl.as_ref().unwrap();
    assert_eq!(
        (pwl.function.as_ref(), pwl.ndim, pwl.points.len()),
        ("PWL", 1, 2)
    );
    assert!(close(num(&pwl.points[1].1), 100e9));
    assert_eq!(nets(inst(&ast, "G_Switch")), ["0", "be", "ctrl"]);
    let e = controlled("e1");
    assert_eq!(e.params.len(), 1);
    assert!(close(num(&e.pwl.as_ref().unwrap().points[0].0), -0.1));
    assert_eq!(controlled("G1").pwl.unwrap().points.len(), 2);
    let f = controlled("F1");
    assert_eq!((f.ctrl_sources.len(), f.params.len()), (2, 1));
    // printed, they parse back the same
    let first = ast.to_string();
    let (_dir2, parsed2, files2) = parse_text(&[("top.sp", &first)]).await;
    assert_eq!(diagnostics(&parsed2), [], "printed netlist:\n{first}");
    assert_eq!(files2.build(parsed2).0.to_string(), first);
}

/// HSPICE reads the first line of the netlist as its title (62 decks in the
/// HSPICE corpus had a title reported as a syntax error, and titles such as
/// `DIC Lab 7: ring` were read as elements). An included file has no title.
#[tokio::test]
async fn first_line_is_the_title() {
    let (_dir, parsed, files) = parse_deck(&[
        ("top.sp", "DIC Lab 7: ring\nR1 a b 1k\n.inc 'lib.sp'\n"),
        ("lib.sp", "R2 a b 1k\n"),
    ])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("TitleIsElement", 1)]);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(ast.title.as_deref(), Some("DIC Lab 7: ring"));
    assert_eq!(names(&ast), ["R1", "R2"]);
    // the title is printed on the first line, and read back as the title
    let printed = ast.to_string();
    assert!(printed.starts_with("DIC Lab 7: ring\n"), "{printed}");

    // a comment, a dot-command or a blank first line is not a title
    for text in [
        "* deck\nR1 a b 1k\n",
        ".param p=1\nR1 a b 1k\n",
        "\nR1 a b 1k\n",
    ] {
        let (_dir, parsed, files) = parse_deck(&[("top.sp", text)]).await;
        assert_eq!(diagnostics(&parsed), []);
        let (ast, _) = files.build(parsed);
        assert_eq!(
            (ast.title.as_deref(), names(&ast)),
            (None, vec!["R1".to_owned()])
        );
    }
    // a title that is not an element: no warning
    let (_dir, parsed, files) =
        parse_deck(&[("top.sp", "mux 2 pra 1 com transmission gate\nR1 a b 1k\n")]).await;
    assert_eq!(diagnostics(&parsed), []);
    assert_eq!(names(&files.build(parsed).0), ["R1"]);
}

/// HSPICE also reads `(`, `)` and `,` as delimiters (about 150 `X` lines and
/// some `E` and `.SUBCKT` lines in the HSPICE corpus were rejected).
#[tokio::test]
async fn parentheses_and_commas_are_delimiters() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".SUBCKT (via) in out zvia=45 tpropvia=15p\nR1 in out 1\n.ENDS (via)\n\
         Xvn7    jn13  jn14  (via)\t\t* Package via\n\
         X_U1          ESDP N24207 OPA (FLW=10 GLF=0.0565  RNV=9.617)\n\
         xpmos0 (vout vin vvdd vvdd) DGPMOS\n\
         E1 vdiff 0 (out1,out2) 1\n\
         W1 a 0 b 0\n+ N=1 LAYERSTACK=stack_1, FSOPTIONS=opt1\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert!(ast.subckt.contains_key("via"));
    let subckt = |name: &str| match &inst(&ast, name).ctx {
        InstanceCtx::Subckt(x) => (x.cktname.to_string(), x.nodes.len(), x.params.len()),
        _ => panic!(),
    };
    assert_eq!(subckt("Xvn7"), ("via".to_owned(), 2, 0));
    assert_eq!(subckt("X_U1"), ("OPA".to_owned(), 2, 3));
    assert_eq!(subckt("xpmos0"), ("DGPMOS".to_owned(), 4, 0));
    assert_eq!(nets(inst(&ast, "E1")), ["0", "out1", "out2", "vdiff"]);
    let InstanceCtx::Unknown { params, .. } = &inst(&ast, "W1").ctx else {
        panic!()
    };
    assert_eq!(params.len(), 3);
}

/// Statements found in the HSPICE corpus: `.lib "file"tt` without a space;
/// functions and optimization parameters in `.param`; a `PWL` source read
/// from a file; a `*` comment after a statement; command short forms and
/// variation blocks; a Windows path in `.inc`.
#[tokio::test]
async fn more_hspice_statements() {
    let (_dir, parsed, files) = parse_text(&[
        (
            "top.sp",
            ".lib \"models.l\"tt\n\
             .PARAM a1=11 square(X) = 'X*X' a2=3\n\
             .param fs(x) = 'x*2*pi' DelayTime = Opt1 ( 0n, 0n, 5n )\n\
             Vsig1 sig1 0 PWL PWLFILE=str(wav_left)  TD=1n\n\
             cload out gnd 1000f *output loading\n\
             .Local_Variation\n.End_Local_Variation\n\
             .mea tran t1 trig v(a) val=0.5 rise=1 targ v(b) val=0.5 rise=1\n\
             .inc \".\\sub.sp\"\n",
        ),
        ("models.l", ".lib tt\nR1 a b 1k\n.endl tt\n"),
        ("sub.sp", "R2 a b 1k\n"),
    ])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R1", "Vsig1", "cload", "R2"]);
    let keys: Vec<_> = ast.param.iter().map(|kv| kv.k.to_string()).collect();
    assert_eq!(keys, ["a1", "a2", "DelayTime"]);
    let funcs: Vec<_> = ast.param_func.iter().map(|f| f.name.to_string()).collect();
    assert_eq!(funcs, ["square", "fs"]);
    let InstanceCtx::Voltage(v) = &inst(&ast, "Vsig1").ctx else {
        panic!()
    };
    assert_eq!(v.source.func.as_ref().map(|f| &*f.name), Some("PWL"));
    assert!(func_args(&v.source).is_empty());
    assert_eq!(v.source.params[0].k, "PWLFILE");
    assert!(matches!(&v.source.params[0].v, Value::Expr(e) if e == "str(wav_left)"));
    assert!(
        ast.to_string()
            .contains("Vsig1 sig1 0 PWL PWLFILE=str(wav_left) TD="),
        "{ast}"
    );
    let InstanceCtx::Capacitor(c) = &inst(&ast, "cload").ctx else {
        panic!()
    };
    assert!(close(value_of(&c.value), 1e-12));
}

/// A block encrypted for HSPICE (`.prot freelib`) used to give one error per
/// line (over a million in the HSPICE corpus); a plain `.prot` block is
/// parsed as usual.
#[tokio::test]
async fn encrypted_block_is_skipped() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "R1 a b 1k\n.PROT freelib\nH]=3g\\#/%M_lgOq+y\neGLW-[=:\\>al\n.UNPROT\n\
         .protect\nR2 b c 1k\n.unprotect\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), [diag("Encrypted", 2)]);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(names(&ast), ["R1", "R2"]);
}

/// Every element is stored in the AST: keep them small (they were 272 and
/// 232 bytes before the `PWL` sources were boxed).
#[test]
fn element_sizes() {
    use crate::instance::{Instance, InstanceBuilder};
    println!(
        "InstanceBuilder: {}, Instance: {}",
        std::mem::size_of::<InstanceBuilder>(),
        std::mem::size_of::<Instance<'_>>()
    );
    assert!(std::mem::size_of::<InstanceBuilder>() <= 232);
    assert!(std::mem::size_of::<Instance<'_>>() <= 232);
}

/// A file included from several places at once used to be read and parsed
/// once per include.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_included_twice_is_parsed_once() {
    let (_dir, parsed, files) = parse_text(&[
        ("top.sp", ".inc 'leaf.sp'\n.inc 'leaf.sp'\n.inc 'leaf.sp'\n"),
        ("leaf.sp", "R1 a b 1k\n"),
    ])
    .await;
    assert_eq!(parsed.inner.len(), 3);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R1", "R1", "R1"]);
}

/// At most `MAX_DIAGNOSTICS_PER_FILE` diagnostics are logged per file (a
/// Liberty file read as a netlist used to log hundreds of thousands); all of
/// them still count.
#[test]
fn diagnostics_are_capped_per_file() {
    use crate::{MAX_DIAGNOSTICS_PER_FILE, Reporter};
    let file_id: FileId = PathBuf::from("x.sp").into();
    let mut reporter = Reporter::default();
    for _ in 0..MAX_DIAGNOSTICS_PER_FILE + 50 {
        reporter.report(&ParseErrorInner::TrailingText.with(None), &file_id, "");
    }
    assert_eq!(reporter.counts[&file_id], (MAX_DIAGNOSTICS_PER_FILE, 50, 0));
    assert!(reporter.finish());
}

/// More forms from the HSPICE corpus: `~` in node names (schematic exports),
/// `,` between the points of a `PWL` source without parentheses, the nodes
/// of a source or resistor in parentheses, `,` between `.model` parameters,
/// and a value on the continuation line after `key=`.
#[tokio::test]
async fn more_delimiters_and_continuations() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "XI24 temp1 ~s0 ~s1 net90 vdd vss / nand3 n_width=29.44u\n\
         VC clk gnd pwl 0n 0.0, 4n 0.0, 4.05n 5.0\n\
         vdd (vdd 0) 1\n\
         R9 (a b) 1k\n\
         .model sample_brd W modeltype=RLGC N=1\n\
         + Lo=\n\
         +   3.381601808482054e-007\n\
         + Co=1p\n\
         .model diff_stripline W MODELTYPE=FieldSolver, LAYERSTACK=stack_1, FSOPTIONS=opt1\n\
         .model nch nmos (level=54\n+ vth0=0.3)\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["XI24", "VC", "vdd", "R9"]);
    assert_eq!(
        nets(inst(&ast, "XI24")),
        ["net90", "temp1", "vdd", "vss", "~s0", "~s1"]
    );
    let InstanceCtx::Voltage(v) = &inst(&ast, "VC").ctx else {
        panic!()
    };
    assert_eq!(func_args(&v.source).len(), 6);
    assert_eq!(nets(inst(&ast, "vdd")), ["0", "vdd"]);
    assert_eq!(nets(inst(&ast, "R9")), ["a", "b"]);
    let params: Vec<_> = ast.model.iter().map(|m| m.params.len()).collect();
    assert_eq!(params, [4, 3, 2]);
    assert!(close(num(&ast.model[0].params[2].v), 3.381601808482054e-7));
}

/// Found by fuzzing: both CDL forms of `/` at once, and a lone `+` on the
/// last line (an empty continuation line, as it is on any other line).
#[tokio::test]
async fn fuzzed_round_trip_cases() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "Xcdl a<0> b VDD! 0 / /INV\n.meas tran t1 trig v(a) val=1\n+ targ v(b) val=1\n+",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let InstanceCtx::Subckt(x) = &inst(&ast, "Xcdl").ctx else {
        panic!()
    };
    assert_eq!((x.nodes.len(), x.cktname.as_ref()), (4, "INV"));
    let first = ast.to_string();
    let (_dir2, parsed2, files2) = parse_deck(&[("top.sp", &first)]).await;
    assert_eq!(diagnostics(&parsed2), []);
    assert_eq!(files2.build(parsed2).0.to_string(), first);
}

// ---------------------------------------------------------------------------
// model selection

/// `(path, chosen model or error)` for every MOSFET
fn chosen(ast: &AST<'_>) -> Vec<(String, String)> {
    ast.mosfet_models()
        .into_iter()
        .map(|m| {
            let model = match &m.selection {
                Ok(s) => s.model.name.to_string(),
                Err(e) => format!("error: {e}"),
            };
            (m.path, model)
        })
        .collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// The path, and L and W in um, of every MOSFET, against `expected`
fn assert_sizes(ast: &AST<'_>, expected: &[(&str, f64, f64)], what: &str) {
    let got: Vec<_> = ast
        .mosfet_models()
        .into_iter()
        .map(|m| match &m.selection {
            Ok(s) => (m.path, s.l * 1e6, s.w * 1e6),
            Err(e) => panic!("{what}: {}: {e}", m.path),
        })
        .collect();
    assert_eq!(got.len(), expected.len(), "{what}: {got:?}");
    for ((path, l, w), (epath, el, ew)) in got.iter().zip(expected) {
        assert_eq!(path, epath, "{what}");
        assert!(
            close(*l, *el) && close(*w, *ew),
            "{what}: {path}: L={l} W={w}"
        );
    }
}

/// HSPICE's model selector: the model of that name, or the bin `name.N`
/// with LMIN <= L < LMAX and WMIN <= W < WMAX.
#[tokio::test]
async fn model_bins_by_length_and_width() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nch.1 nmos level=54 lmin=1e-6 lmax=1e-5 wmin=1e-6 wmax=1e-4\n\
         .model nch.2 nmos level=54 lmin=1.8e-7 lmax=1e-6 wmin=1e-6 wmax=1e-4\n\
         .model nch.3 nmos level=54 lmin=1.8e-7 lmax=1e-6 wmin=2.2e-7 wmax=1e-6\n\
         .model nch.4 nmos level=54 lmin=5e-8 lmax=1e-7 wmin=1e-7 wmax=1e-4\n\
         .model nch.5 nmos level=54 lmin=1e-7 lmax=1.8e-7 wmin=1e-7 wmax=1e-4\n\
         .model nmod nmos level=1\n\
         .model nmod.1 nmos level=1\n\
         .param lg=0.18u wn='2*0.5u'\n\
         M1 d g s b nch l=lg w=wn\n\
         M2 d g s b nch l=0.18u w=0.5u\n\
         M3 d g s b nch l=2u w=10u\n\
         M4 d g s b nch l=0.1u w=1u\n\
         M5 d g s b NCH l=1u w=0.5u\n\
         M6 d g s b nmod l=1u w=1u\n\
         M7 d g s b pch l=1u w=1u\n\
         M8 d g s b nch l=lx w=1u\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        pairs(&[
            // W = WMAX of nch.3 = WMIN of nch.2: the upper bin
            ("M1", "nch.2"),
            ("M2", "nch.3"),
            ("M3", "nch.1"),
            // 0.1u is 1.0000000000000001e-7: equal to LMAX of nch.4
            ("M4", "nch.5"),
            (
                "M5",
                "error: none of the 5 bins `NCH.N` holds L=1e-6, W=5e-7"
            ),
            // a model of that name: no selection
            ("M6", "nmod"),
            ("M7", "error: no model `pch`, nor bins `pch.N`"),
            ("M8", "error: L: undefined parameter `lx`"),
        ])
    );
    let s = ast.mosfet_models().remove(0).selection.unwrap();
    assert!(close(s.l, 1.8e-7) && close(s.w, 1e-6) && s.nf == 1.0);
    assert_eq!(ast.select_model("nch", 2e-6, 1e-5).unwrap().name, "nch.1");
}

/// Models defined in a subckt are local to it and hide the ones outside;
/// L, W and the limits of the bins are evaluated where the transistor is,
/// with the parameters of the subckt instances above it (as in the TSMC
/// `nch_mac` macro models, whose limits use mismatch parameters).
#[tokio::test]
async fn model_selection_through_the_hierarchy() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".param dxln=0\n\
         .subckt nch_mac d g s b l=1u w=1u nf=1 mismatch=0\n\
         .param dlmis='1e-9*mismatch'\n\
         MAIN d g s b nch l=l w=w nf=nf\n\
         .model nch.1 nmos lmin='1e-7-(dxln+dlmis)' lmax='5e-7-(dxln+dlmis)' wmin=1e-7 wmax=1e-5\n\
         .model nch.2 nmos lmin='5e-7-(dxln+dlmis)' lmax=1e-5 wmin=1e-7 wmax=1e-5\n\
         .ends\n\
         .subckt inv in out vdd vss wn=1u\n\
         Xn out in vss vss nch_mac l=200n w='wn/2'\n\
         Mp out in vdd vdd nch l=1u w='2*wn'\n\
         .ends\n\
         .model nch.1 nmos lmin=1e-8 lmax=1 wmin=1e-8 wmax=1\n\
         X1 a b c d nch_mac l=200n w=2u\n\
         X2 a b c d nch_mac l=800n w=2u\n\
         X3 a y vdd 0 inv wn=4u\n\
         M9 a b c d nch l=1u w=1u\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    let models = ast.mosfet_models();
    let local = &ast.subckt["nch_mac"].ast.model;
    let global = &ast.model[0];
    let which = |path: &str| {
        let m = models.iter().find(|m| m.path == path).unwrap();
        let s = m.selection.as_ref().unwrap();
        let place = if local.iter().any(|l| std::ptr::eq(l, s.model)) {
            "local"
        } else if std::ptr::eq(global, s.model) {
            "global"
        } else {
            "?"
        };
        (
            s.model.name.to_string(),
            place,
            (s.l * 1e9).round(),
            (s.w * 1e9).round(),
        )
    };
    assert_eq!(
        models.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(),
        ["X1.MAIN", "X2.MAIN", "X3.Xn.MAIN", "X3.Mp", "M9"]
    );
    // (L and W in nm)
    assert_eq!(
        which("X1.MAIN"),
        ("nch.1".to_owned(), "local", 200.0, 2000.0)
    );
    assert_eq!(
        which("X2.MAIN"),
        ("nch.2".to_owned(), "local", 800.0, 2000.0)
    );
    // `w='wn/2'` with `wn=4u` from the instance of `inv` above
    assert_eq!(
        which("X3.Xn.MAIN"),
        ("nch.1".to_owned(), "local", 200.0, 2000.0)
    );
    assert_eq!(
        which("X3.Mp"),
        ("nch.1".to_owned(), "global", 1000.0, 8000.0)
    );
    assert_eq!(which("M9"), ("nch.1".to_owned(), "global", 1000.0, 1000.0));
}

/// `.option parhier`: with GLOBAL (HSPICE's default), the definition closest
/// to the top level wins, also over a value passed on an instance line; with
/// LOCAL, the definition closest to the transistor wins.
#[tokio::test]
async fn parameter_hierarchy_rules() {
    let deck = |option: &str| {
        format!(
            "{option}\n.param w=1u\n.subckt inv a b w=2u\nM1 a b 0 0 nmod w=w l=1u\n.ends\n\
             .model nmod nmos level=1\nX1 a b inv\nX2 a b inv w=3u\n"
        )
    };
    let widths = |ast: &AST<'_>| -> Vec<f64> {
        ast.mosfet_models()
            .iter()
            .map(|m| m.selection.as_ref().unwrap().w)
            .collect()
    };
    let (_dir, parsed, files) = parse_text(&[("top.sp", &deck("* default"))]).await;
    assert_eq!(widths(&files.build(parsed).0), [1e-6, 1e-6]);
    let (_dir, parsed, files) = parse_text(&[("top.sp", &deck(".option parhier=local"))]).await;
    assert_eq!(widths(&files.build(parsed).0), [2e-6, 3e-6]);
}

/// `.option scale` multiplies L and W; `defl`/`defw` give L and W when the
/// instance has none; bins hold the width per finger.
#[tokio::test]
async fn model_selection_options_and_fingers() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".option scale=1e-6 defl=0.5 defw=2\n\
         .model nch.1 nmos wnflag=1 lmin=1e-7 lmax=1e-6 wmin=1e-7 wmax=1e-6\n\
         .model nch.2 nmos wnflag=1 lmin=1e-7 lmax=1e-6 wmin=1e-6 wmax=1e-5\n\
         .model pch.1 nmos lmin=1e-7 lmax=1e-6 wmin=1e-7 wmax=1e-6\n\
         .model pch.2 nmos lmin=1e-7 lmax=1e-6 wmin=1e-6 wmax=1e-5\n\
         M1 d g s b nch l=0.18 w=4 nf=8\n\
         M2 d g s b nch l=0.18 w=4\n\
         M3 d g s b pch l=0.18 w=4 nf=8\n\
         M4 d g s b pch\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        pairs(&[
            ("M1", "nch.1"),
            ("M2", "nch.2"),
            ("M3", "pch.1"),
            ("M4", "pch.2"),
        ])
    );
    let m4 = ast.mosfet_models().remove(3).selection.unwrap();
    assert!(close(m4.l, 5e-7) && close(m4.w, 2e-6));
}

/// The ranges of a bin are evaluated where the model is defined: those of a
/// top-level model with the top-level parameters, even inside a subckt that
/// defines (or is passed) a parameter of the same name; those of a model in a
/// subckt with that subckt instance's. (The top-level `M1` is the first
/// MOSFET HSPICE makes, on purpose: see the known gap in `models.rs`.)
#[tokio::test]
async fn bin_ranges_are_evaluated_where_the_model_is() {
    for option in ["", ".option parhier=local\n"] {
        let text = format!(
            "{option}.param lb=2e-7\n\
             .model nch.1 nmos lmin=1e-8 lmax='lb' wmin=1e-8 wmax=1e-4\n\
             .model nch.2 nmos lmin='lb' lmax=1e-5 wmin=1e-8 wmax=1e-4\n\
             .subckt shadow d lb=1e-6\nM1 d d 0 0 nch w=1u l=5e-7\n.ends\n\
             .subckt inner d\n.param lb=1e-6\nM1 d d 0 0 nch w=1u l=5e-7\n.ends\n\
             .subckt mac d ll=1e-7 dx=0\n\
             .model lm.1 nmos lmin=1e-8 lmax='2e-7-dx' wmin=1e-8 wmax=1e-4\n\
             .model lm.2 nmos lmin='2e-7-dx' lmax=1e-5 wmin=1e-8 wmax=1e-4\n\
             M1 d d 0 0 lm w=1u l=ll\n.ends\n\
             X1 d shadow\nX2 d shadow lb=1e-6\nX3 d inner\n\
             X4 d mac ll=1.95e-7\nX5 d mac ll=1.95e-7 dx=1e-8\nM1 d d 0 0 nch w=1u l=5e-7\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        assert_eq!(
            chosen(&ast),
            pairs(&[
                ("X1.M1", "nch.2"),
                ("X2.M1", "nch.2"),
                ("X3.M1", "nch.2"),
                ("X4.M1", "lm.1"),
                ("X5.M1", "lm.2"),
                ("M1", "nch.2"),
            ]),
            "{option}"
        );
        let usage = ast.model_usage();
        let mut counts: Vec<_> = usage
            .models
            .iter()
            .map(|(m, n)| (m.name.to_string(), *n))
            .collect();
        counts.sort();
        assert_eq!(
            counts,
            [
                ("lm.1".to_owned(), 1),
                ("lm.2".to_owned(), 1),
                ("nch.2".to_owned(), 4)
            ]
        );
    }
}

/// A value passed on an instance line is computed in the new instance: with
/// LOCAL, the subckt's own `k=100` and `.param q=200` are used; with GLOBAL,
/// the definitions closest to the top, the parent's `k=7` and `q=8`. (HSPICE,
/// probe case 4.)
#[tokio::test]
async fn passed_values_are_computed_in_the_new_instance() {
    for (option, expected) in [("", 15e-6), (".option parhier=local\n", 300e-6)] {
        let text = format!(
            "{option}.model nmod nmos level=1\n\
             .subckt child d s=0 k=100\n.param q=200\nM1 d d 0 0 nmod w='s*1u' l=1u\n.ends\n\
             .subckt parent d k=7 q=8\nX1 d child s='k+q'\n.ends\n\
             X1 d parent\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let w = ast.mosfet_models().remove(0).selection.unwrap().w;
        assert!(close(w, expected), "{option}: {w}");
    }
}

/// Parameters, as measured on HSPICE (deck `q_override_*` of
/// `scripts/hspice_kit`). The text of a value passed is evaluated in the new
/// instance; its own name is looked up only above that instance; with GLOBAL,
/// a definition closer to the top wins over any value passed.
#[tokio::test]
async fn parameters_as_measured_on_hspice() {
    let deck = ".param w=4 q=2\n.model nv nmos level=54\n\
        .subckt cc d g s w=2 l=1\nM1 d g s s nv w='w*1u' l='l*1u'\n.ends cc\n\
        .subckt cb d g s\n.param v=2 w=2\nM1 d g s s nv w='w*1u' l='v*1u'\n.ends cb\n\
        .subckt cu d g s\nM1 d g s s nv w='q*1u' l=1u\n.ends cu\n\
        .subckt cm d g s w=3\nX1 d g s cc w='w*2'\n.ends cm\n\
        X1 d g 0 cc l='w*3'\nX2 d g 0 cc w=5 l='w*3'\nX3 d g 0 cc w='w*3'\n\
        X4 d g 0 cc w='w*2' l='w*3'\nX5 d g 0 cu q=5\nX6 d g 0 cb w=7\n\
        X7 d g 0 cb w='w*3'\nX8 d g 0 cm\n";
    let local = [
        ("X1.M1", 6.0, 2.0),
        ("X2.M1", 15.0, 5.0),
        ("X3.M1", 1.0, 12.0),
        ("X4.M1", 24.0, 8.0),
        ("X5.M1", 1.0, 5.0),
        ("X6.M1", 2.0, 7.0),
        ("X7.M1", 2.0, 12.0),
        ("X8.X1.M1", 1.0, 6.0),
    ];
    let global = [
        ("X1.M1", 12.0, 4.0),
        ("X2.M1", 12.0, 4.0),
        ("X3.M1", 1.0, 4.0),
        ("X4.M1", 12.0, 4.0),
        ("X5.M1", 1.0, 2.0),
        ("X6.M1", 2.0, 4.0),
        ("X7.M1", 2.0, 4.0),
        ("X8.X1.M1", 1.0, 4.0),
    ];
    let text = format!(".option parhier=local\n{deck}");
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    assert_sizes(&files.build(parsed).0, &local, "local");
    let (_dir, parsed, files) = parse_text(&[("top.sp", deck)]).await;
    assert_sizes(&files.build(parsed).0, &global, "global");

    // the probe (scripts/parhier_probe.sp), cases 1 to 6: W in um, by parhier
    let probe = ".param p=1\n.model nv nmos level=54\n\
        .subckt show d v=0\nM1 d d 0 0 nv w='v*1u' l=1u\n.ends show\n\
        .subckt c1 d p=3\nX1 d show v=p\n.ends c1\n\
        .subckt c2 d p=3\nX1 d show v=p\n.ends c2\n\
        .subckt c3leaf d\nX1 d show v=p\n.ends c3leaf\n\
        .subckt c3 d p=4\nX1 d c3leaf\n.ends c3\n\
        .subckt c4child d s=0 k=100\n.param q=200\nX1 d show v='s'\n.ends c4child\n\
        .subckt c4 d k=7 q=8\nX1 d c4child s='k+q'\n.ends c4\n\
        .subckt c5 d p=3\n.param p=5\nX1 d show v=p\n.ends c5\n\
        .subckt c6 d a=1 b='a*2'\nX1 d show v=b\n.ends c6\n\
        Xc1 d c1 p=2\nXc2 d c2\nXc3 d c3 p=5\nXc4 d c4\nXc5 d c5 p=2\nXc6 d c6 a=6\n";
    let row = |path, w| (path, 1.0, w);
    for (option, values) in [
        ("", [1.0, 1.0, 1.0, 15.0, 1.0, 12.0]),
        (".option parhier=local\n", [2.0, 3.0, 5.0, 300.0, 2.0, 12.0]),
    ] {
        let text = format!("{option}{probe}");
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        assert_sizes(
            &files.build(parsed).0,
            &[
                row("Xc1.X1.M1", values[0]),
                row("Xc2.X1.M1", values[1]),
                row("Xc3.X1.X1.M1", values[2]),
                row("Xc4.X1.X1.M1", values[3]),
                row("Xc5.X1.M1", values[4]),
                row("Xc6.X1.M1", values[5]),
            ],
            option,
        );
    }

    // a value passed that reads its own name, with nothing above the new
    // instance: HSPICE stops ("cannot redefine parameter to itself")
    for option in ["", ".option parhier=local\n"] {
        let text = format!(
            "{option}.model nv nmos level=54\n\
             .subckt cc d g s w=2\nM1 d g s s nv w='w*1u' l=1u\n.ends cc\n\
             X1 d g 0 cc w='w*3'\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        assert_eq!(
            chosen(&ast),
            pairs(&[("X1.M1", "error: W: parameter `w` depends on itself")]),
            "{option}"
        );
    }
}

/// Functions defined with `.param f(x)=...`, as measured on HSPICE (deck
/// `q_func_*` of `scripts/hspice_kit`): the definition closest to the top
/// with GLOBAL, the nearest with LOCAL; the names and the calls in a body are
/// resolved where the function is defined, not where it is called.
#[tokio::test]
async fn functions_as_measured_on_hspice() {
    let deck = ".param f(x)='x*2' k=2 h(x)='x*k'\n.model nv nmos level=54\n\
        .subckt cf d g s\n.param f(x)='x*3'\nM1 d g s s nv l=1u w='f(1)*1u'\n.ends cf\n\
        .subckt ck d g s k=7\nM1 d g s s nv l=1u w='h(1)*1u'\n.ends ck\n\
        .subckt cl d g s\n.param g(x)='x*kk' kk=2\nX1 d g s cn\n.ends cl\n\
        .subckt cn d g s kk=9\nM1 d g s s nv l=1u w='g(1)*1u'\n.ends cn\n\
        X1 d g 0 cf\nX2 d g 0 ck\nX3 d g 0 cl\n";
    for (option, x1) in [("", 2.0), (".option parhier=local\n", 3.0)] {
        let text = format!("{option}{deck}");
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        assert_sizes(
            &files.build(parsed).0,
            &[
                ("X1.M1", 1.0, x1),
                ("X2.M1", 1.0, 2.0),
                ("X3.X1.M1", 1.0, 2.0),
            ],
            option,
        );
    }
    // a function called in a body: where the outer function is defined
    // (`func_nested_local`)
    let text = ".option parhier=local\n.param f(x)='x*2' g(x)='f(x)'\n.model nv nmos level=54\n\
        .subckt cf d\n.param f(x)='x*3'\nM1 d d 0 0 nv l=1u w='g(1)*1u'\n.ends cf\nX1 d cf\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_sizes(&files.build(parsed).0, &[("X1.M1", 1.0, 2.0)], "nested");
}

/// Models and subckts are looked up along the instance path, not along the
/// nesting of the text: a definition in the subckt of any instance above is
/// seen, the nearest first (HSPICE, decks `q_model_dynamic` and
/// `q_subckt_dynamic`). One line can then bind to different definitions.
#[tokio::test]
async fn lookup_along_the_instance_path() {
    let limits = "lmin=1e-10 lmax=1 wmin=1e-10 wmax=1";
    let text = format!(
        ".model dy.1 nmos {limits}\n.subckt leaf d g s\nM1 d g s s dy l=1u w=1u\n.ends leaf\n\
         .subckt host d g s\n.model dy.2 nmos {limits}\nX1 d g s leaf\n.ends host\n\
         X1 d g 0 leaf\nX2 d g 0 host\n"
    );
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        pairs(&[("X1.M1", "dy.1"), ("X2.X1.M1", "dy.2")])
    );
    let usage = ast.model_usage();
    let mut counts: Vec<_> = usage
        .models
        .iter()
        .map(|(m, n)| (m.name.to_string(), *n))
        .collect();
    counts.sort();
    assert_eq!(counts, [("dy.1".to_owned(), 1), ("dy.2".to_owned(), 1)]);

    let text = ".model nv nmos level=54\n\
        .subckt cell d g s\nM1 d g s s nv l=1u w=1u\n.ends cell\n\
        .subckt leaf d g s\nX1 d g s cell\n.ends leaf\n\
        .subckt host d g s\n.subckt cell d g s\nM1 d g s s nv l=1u w=2u\n.ends cell\n\
        X1 d g s leaf\n.ends host\nX1 d g 0 leaf\nX2 d g 0 host\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_sizes(
        &files.build(parsed).0,
        &[("X1.X1.M1", 1.0, 1.0), ("X2.X1.X1.M1", 1.0, 2.0)],
        "subckt",
    );
}

/// `name.suffix` is a bin of `name`, whatever the suffix; a name with a dot
/// cannot be named on an instance line; a name with two dots is of no use
/// (HSPICE, decks `q_mc_as_bin`, `q_nonnumeric`, `q_1a_as_bin`,
/// `q_bin_direct_in`, `q_two_dots_*`).
#[tokio::test]
async fn bin_names_as_measured_on_hspice() {
    let limits = "lmin=1e-10 lmax=1 wmin=1e-10 wmax=1";
    let text = format!(
        ".model mc.mc nmos {limits}\n.model a1.1a nmos {limits}\n\
         .model nm.1 nmos lmin=1e-7 lmax=1e-6 wmin=1e-10 wmax=1\n.model nm.mc nmos {limits}\n\
         .model nm.1a nmos {limits}\n.model e.4 nmos {limits}\n.model n.ch.1 nmos {limits}\n\
         M1 d g 0 0 mc l=1u w=1u\nM2 d g 0 0 a1 l=1u w=1u\nM3 d g 0 0 nm l=5u w=1u\n\
         M4 d g 0 0 e.4 l=5u w=5u\nM5 d g 0 0 n l=1u w=1u\nM6 d g 0 0 n.ch l=1u w=1u\n\
         M7 d g 0 0 n.ch.1 l=1u w=1u\nM8 d g 0 0 mc.mc l=1u w=1u\n"
    );
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        pairs(&[
            ("M1", "mc.mc"),
            ("M2", "a1.1a"),
            // the first that holds it, in netlist order
            ("M3", "nm.mc"),
            ("M4", "error: no model `e.4`, nor bins `e.4.N`"),
            ("M5", "error: no model `n`, nor bins `n.N`"),
            ("M6", "error: no model `n.ch`, nor bins `n.ch.N`"),
            ("M7", "error: no model `n.ch.1`, nor bins `n.ch.1.N`"),
            ("M8", "error: no model `mc.mc`, nor bins `mc.mc.N`"),
        ])
    );
}

/// A bin needs its four limits when it is used, not otherwise (HSPICE, decks
/// `q_incomplete_*`, `q_mc_unused`, `q_bin_nolimits_unused`): "you must set
/// all four parameters (wmin,wmax,lmin,lmax)".
#[tokio::test]
async fn bins_need_their_four_limits() {
    let limits = "lmin=1e-10 lmax=1 wmin=1e-10 wmax=1";
    for cards in [
        format!(".model nm.1 nmos {limits}\n.model nm.mc nmos level=54\n"),
        format!(".model nm.mc nmos level=54\n.model nm.1 nmos {limits}\n"),
    ] {
        let text = format!(
            "{cards}.model ok nmos level=54\nM1 d g 0 0 nm l=1u w=1u\nM2 d g 0 0 ok l=1u w=1u\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        assert_eq!(
            chosen(&ast),
            pairs(&[
                (
                    "M1",
                    "error: model `nm.mc` has a `.` in its name: it must set LMIN, LMAX, WMIN and WMAX \
                     (at level 72: LMIN, LMAX, and NFINMIN and NFINMAX, or WSHEETMIN and WSHEETMAX)"
                ),
                ("M2", "ok"),
            ])
        );
    }
}

/// At each level from the transistor's up: a model of that name, else the
/// first bin that holds L and W; a level whose bins do not hold them hides
/// nothing (HSPICE, decks `q_localhide`, `q_local_exact`,
/// `q_localbin_globalexact`, `q_localbin_out_globalexact`, `q_exact_and_bin`,
/// `q_order`).
#[tokio::test]
async fn levels_of_models_as_measured_on_hspice() {
    let wide = "lmin=1e-10 lmax=1 wmin=1e-10 wmax=1";
    let small = "lmin=1e-10 lmax=5e-7 wmin=1e-10 wmax=1";
    let cases = [
        // (global cards, local cards, whether the global one is chosen)
        (
            format!(".model loc.1 nmos {wide}\n"),
            format!(".model loc.1 nmos {small}\n"),
            true,
        ),
        (
            format!(".model loc.1 nmos {small}\n"),
            format!(".model loc.1 nmos {wide}\n"),
            false,
        ),
        (
            format!(".model loc.1 nmos {wide}\n"),
            ".model loc nmos level=54\n".to_owned(),
            false,
        ),
        (
            ".model loc nmos level=54\n".to_owned(),
            format!(".model loc.1 nmos {wide}\n"),
            false,
        ),
        (
            ".model loc nmos level=54\n".to_owned(),
            format!(".model loc.1 nmos {small}\n"),
            true,
        ),
    ];
    for (global, local, global_chosen) in cases {
        let text = format!(
            "{global}.subckt h d g s\n{local}M1 d g s s loc l=1u w=1u\n.ends h\nX1 d g 0 h\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let m = ast.mosfet_models().remove(0);
        let s = m
            .selection
            .as_ref()
            .unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(
            std::ptr::eq(s.model, &ast.model[0]),
            global_chosen,
            "{text}"
        );
    }
    // no level holds them: an error that counts the bins seen
    let text = format!(
        ".model loc.1 nmos {small}\n.subckt h d g s\n.model loc.1 nmos {small}\n\
         M1 d g s s loc l=1u w=1u\n.ends h\nX1 d g 0 h\n"
    );
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        pairs(&[(
            "X1.M1",
            "error: none of the 2 bins `loc.N` holds L=1e-6, W=1e-6"
        )])
    );
    // a model and a bin of that name: the model; two bins hold: the first in
    // the text, not the lowest number
    let text = format!(
        ".model eb nmos level=54\n.model eb.1 nmos {wide}\n\
         .model ov.2 nmos {wide}\n.model ov.1 nmos {wide}\n\
         M1 d g 0 0 eb l=1u w=1u\nM2 d g 0 0 ov l=1u w=1u\n"
    );
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(chosen(&ast), pairs(&[("M1", "eb"), ("M2", "ov.2")]));
}

/// A value at most about 2e-15 (relative) below a limit counts as on it, at
/// every size of limit (HSPICE, decks `q_edge_tol_*`): 3e-15 below it does
/// not, 1e-15 below it does.
#[tokio::test]
async fn bin_edges_as_measured_on_hspice() {
    for limit in [1e-8, 1e-7, 1e-6, 1e-5, 1e-4_f64] {
        let (lo, hi) = (limit / 10.0, limit * 10.0);
        let text = format!(
            ".model tl.1 nmos lmin={lo:e} lmax={limit:e} wmin=1e-10 wmax=1\n\
             .model tl.2 nmos lmin={limit:e} lmax={hi:e} wmin=1e-10 wmax=1\n\
             .model tw.1 nmos lmin=1e-10 lmax=1 wmin={lo:e} wmax={limit:e}\n\
             .model tw.2 nmos lmin=1e-10 lmax=1 wmin={limit:e} wmax={hi:e}\n\
             M1 d g 0 0 tl l='{limit:e}*(1-3e-15)' w=1u\n\
             M2 d g 0 0 tl l='{limit:e}*(1-1e-15)' w=1u\n\
             M3 d g 0 0 tl l={limit:e} w=1u\n\
             M4 d g 0 0 tl l='{limit:e}*(1+1e-15)' w=1u\n\
             M5 d g 0 0 tw l=1u w='{limit:e}*(1-1e-12)'\n\
             M6 d g 0 0 tw l=1u w='{limit:e}*(1-1e-15)'\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        assert_eq!(
            chosen(&ast),
            pairs(&[
                ("M1", "tl.1"),
                ("M2", "tl.2"),
                ("M3", "tl.2"),
                ("M4", "tl.2"),
                ("M5", "tw.1"),
                ("M6", "tw.2"),
            ]),
            "{limit}"
        );
    }
}

/// The numbers after the model name are L, then W; a keyword after them wins
/// (`mos_positional`, `mos_kw_and_positional`, deck `q_positional`).
#[tokio::test]
async fn positional_length_and_width() {
    let text = ".model a nmos level=54\n.param ll=7u\n\
        M1 d g 0 0 a 2u 1u\nM2 d g 0 a 2u 1u\nM3 d g 0 0 a 2u 1u l=3u\nM4 d g 0 0 a 2u w=5u\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_sizes(
        &ast,
        &[
            ("M1", 2.0, 1.0),
            ("M2", 2.0, 1.0),
            ("M3", 3.0, 1.0),
            ("M4", 2.0, 5.0),
        ],
        "positional",
    );
    // printed back as numbers after the model name
    let printed = ast.to_string();
    assert!(
        printed.contains("\nM1 d g 0 0 a 2.0000000e-6 1.0000000e-6\n"),
        "{printed}"
    );
    let (_dir2, parsed2, files2) = parse_text(&[("top.sp", &printed)]).await;
    let (ast2, _) = files2.build(parsed2);
    assert_sizes(
        &ast2,
        &[
            ("M1", 2.0, 1.0),
            ("M2", 2.0, 1.0),
            ("M3", 3.0, 1.0),
            ("M4", 2.0, 5.0),
        ],
        "printed",
    );
}

/// The model name of a MOSFET line is the first word after the fourth node that
/// names a model, else the fourth word if it names one (`M1 d g 0 a pl pw`: 3
/// nodes, then the names of L and W, deck `q_positional_names_3`). A line with no
/// such word keeps the last word that is not a number, so the error names it.
/// HSPICE reads the other lines below as the test does (decks
/// `q_two_model_words*`, and probes on P-2019.06).
#[tokio::test]
async fn model_name_is_the_first_word_that_names_a_model() {
    let text = ".model a nmos level=54\n.param pl=2u pw=1u\n\
        M1 d g 0 a pl pw\nM2 d g 0 0 a pl pw\nM3 d g 0 a pl 1u\nM4 d g s e p b t a\n\
        M5 d g 0 0 nosuch pl pw\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let models = ast.mosfet_models();
    for (name, l, w) in [
        ("M1", 2.0, 1.0),
        ("M2", 2.0, 1.0),
        ("M3", 2.0, 1.0),
        ("M4", 100.0, 100.0),
    ] {
        let m = models.iter().find(|m| m.path == name).unwrap();
        assert_eq!(m.mname, "a", "{name}");
        let s = m.selection.as_ref().expect(name);
        assert!(close(s.l * 1e6, l) && close(s.w * 1e6, w), "{name}: {s:?}");
    }
    let m = models.iter().find(|m| m.path == "M5").unwrap();
    assert!(matches!(&m.selection, Err(crate::models::SelectError::NoModel(n)) if n == "pw"));

    // two words name a model
    let text = ".model a nmos level=54\n.model b nmos level=54\n.model n nmos level=54\n\
        .param b=2u a=1u p=2u\n\
        M1 d g 0 0 a b w=1u\nM2 d g 0 a b w=1u\nM3 d g 0 0 x a b w=1u\nM4 d g 0 b a w=1u\n\
        M5 d g 0 0 a a b\nM6 d g 0 0 b b a\nM7 d g n n n l=1u w=1u\nM8 d g 0 n a p w=1u\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let models = ast.mosfet_models();
    for (name, model, l, w) in [
        ("M1", "a", 2.0, 1.0),
        ("M2", "b", 100.0, 1.0),
        ("M3", "a", 2.0, 1.0),
        ("M4", "a", 100.0, 1.0),
        ("M5", "a", 1.0, 2.0),
        ("M6", "b", 2.0, 1.0),
        ("M7", "n", 1.0, 1.0),
        ("M8", "a", 2.0, 1.0),
    ] {
        let m = models.iter().find(|m| m.path == name).unwrap();
        assert_eq!(m.mname, model, "{name}");
        let s = m.selection.as_ref().expect(name);
        assert!(close(s.l * 1e6, l) && close(s.w * 1e6, w), "{name}: {s:?}");
    }
}

/// A MOSFET line with many words, as measured on HSPICE: any number of values
/// after the model name (decks `q_positional_names_4`, `q_positional_names_8`,
/// `q_positional_numbers_9`), up to 7 nodes before it (`q_nodes_7`). HSPICE
/// stops at 8 nodes (`q_nodes_8`).
#[tokio::test]
async fn mosfet_line_with_many_words() {
    let text = ".model a nmos level=54\n.param pl=2u pw=1u pa=1p pb=1p\n\
        M1 d g 0 0 a pl pw pa pb\nM2 d g 0 a pl pw pa pb\n\
        M3 d g 0 0 a pl pw pa pb pa pb pa pb\nM4 d g 0 0 a 2u 1u 1p 1p 1u 1u 1 1 1\n\
        M5 d g 0 0 a pl pw pa pb w=3u\nM6 d g 0 0 x y z a pl pw\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    let expected = [
        ("M1", 2.0, 1.0),
        ("M2", 2.0, 1.0),
        ("M3", 2.0, 1.0),
        ("M4", 2.0, 1.0),
        // (`pw` is an expression, so it wins over the plain `w=3u`: deck `mb_*`)
        ("M5", 2.0, 1.0),
        ("M6", 2.0, 1.0),
    ];
    assert_sizes(&ast, &expected, "many words");
    // printed, and read back
    let printed = ast.to_string();
    let (_dir2, parsed2, files2) = parse_text(&[("top.sp", &printed)]).await;
    assert_eq!(diagnostics(&parsed2), [], "{printed}");
    assert_sizes(&files2.build(parsed2).0, &expected, "printed");

    let text = ".model a nmos level=54\nM1 d g 0 0 x y z w a l=1u w=1u\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let too_many = |e: &crate::models::SelectError| matches!(e, crate::models::SelectError::TooManyNodes { name, nodes: 8 } if name == "a");
    let models = ast.mosfet_models();
    assert!(models[0].selection.as_ref().is_err_and(too_many));
    let usage = ast.model_usage();
    assert_eq!(usage.errors.len(), 1);
    assert!(too_many(&usage.errors[0].0));
}

/// `OFF` on a MOSFET line takes a place and gives no value. HSPICE's L and W
/// (probes on P-2019.06).
#[tokio::test]
async fn off_takes_a_place() {
    let lines = [
        ("M1 d g 0 0 a OFF", 100.0, 100.0),
        ("M1 d g 0 0 a OFF 1u", 100.0, 1.0),
        ("M1 d g 0 0 a OFF 1u 2u", 100.0, 1.0),
        ("M1 d g 0 0 a 1u OFF", 1.0, 100.0),
        ("M1 d g 0 0 a 1u OFF 2u", 1.0, 100.0),
        ("M1 d g 0 0 a 1u 2u OFF", 1.0, 2.0),
        ("M1 d g 0 0 a 1u 2u OFF 3p", 1.0, 2.0),
        ("M1 d g 0 0 a OFF OFF 1u 2u", 100.0, 100.0),
        ("M1 d g 0 0 a OFF l=2u w=1u", 2.0, 1.0),
        ("M1 d g 0 0 a OFF 1u 2u l=3u", 3.0, 1.0),
        ("M1 d g 0 0 a OFF 1u 2u w=3u", 100.0, 3.0),
        ("M1 d g 0 0 a OFF pl pw", 100.0, 1.0),
        ("M1 d g 0 0 a pl OFF pw", 1.0, 100.0),
        ("M1 d g 0 0 a pl pw OFF", 1.0, 2.0),
        ("M1 d g 0 0 a off '1u' '2u'", 100.0, 1.0),
        ("M1 d g 0 a OFF 1u 2u", 100.0, 1.0),
    ];
    for (line, l, w) in lines {
        let text = format!(".model a nmos level=54\n.param pl=1u pw=2u\n{line}\n");
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        assert_eq!(diagnostics(&parsed), [], "{line}");
        let (ast, _) = files.build(parsed);
        assert_sizes(&ast, &[("M1", l, w)], line);
        // printed, and read back
        let printed = ast.to_string();
        let (_dir2, parsed2, files2) = parse_text(&[("top.sp", &printed)]).await;
        assert_eq!(diagnostics(&parsed2), [], "{printed}");
        assert_sizes(&files2.build(parsed2).0, &[("M1", l, w)], &printed);
    }
}

/// After four nodes, HSPICE wants the model name: it stops on a word that starts
/// with a digit there ("Model name is missing", decks `q_digit_word_5`). A sixth
/// or seventh node can start with a digit.
#[tokio::test]
async fn mosfet_digit_first_fifth_word() {
    let digit = |e: &crate::models::SelectError| matches!(e, crate::models::SelectError::DigitFifthWord { word, .. } if word == "5a");
    let text = ".model a nmos level=54\nM1 d g 0 0 5a a l=1u w=1u\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert!(ast.mosfet_models()[0].selection.as_ref().is_err_and(digit));
    assert!(digit(&ast.model_usage().errors[0].0));
    let text = ".model a nmos level=54\nM1 d g 0 0 x 6 a l=1u w=1u\n\
        M2 d g 0 0 x y 7a a l=1u w=1u\nM3 d g 0 5 a l=1u w=1u\nM4 d g 5 a l=1u w=1u\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let expected: Vec<_> = ["M1", "M2", "M3", "M4"].map(|n| (n, 1.0, 1.0)).into();
    assert_sizes(&files.build(parsed).0, &expected, "digit-first nodes");
}

/// A value in parentheses on a `.param` or `X` line can go on on a `+` line. On a
/// MOSFET line, the parentheses only separate words: see
/// `parentheses_after_a_key_on_a_mosfet_line`. A comma can follow `.ends` and its
/// name (probes on P-2019.06).
#[tokio::test]
async fn group_on_plus_lines_and_ends_commas() {
    let text = ".model a nmos level=54\n.subckt c d g w=1u\nM1 d g 0 0 a l=1u w=w\n.ends\n\
        .param p=(1u+\n+ 2u)\n.param q=max(1u,\n+ 2u)\nM1 d g 0 0 a l=p w=q\n\
        M2 d g 0 0 a l='(1u\n+ +2u)' w=1u\nX1 d g c w=(1u\n+ +2u)\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let expected = [("M1", 3.0, 2.0), ("M2", 3.0, 1.0), ("X1.M1", 1.0, 3.0)];
    assert_sizes(&files.build(parsed).0, &expected, "groups on + lines");
    for ends in [
        ".ends,",
        ".ends,c1",
        ".ends, c1",
        ".ends ,c1",
        ".ends , c1",
        ".ends c1,",
        ".ends (c1)",
        ".ends (c1),",
        ".ends,$ c",
        ".ends c1 $ c",
        ".ends $ c",
        ".ends * c",
        ".ends c1 * c",
        ".ends\n+ c1",
        ".ends c1",
    ] {
        let text = format!(
            ".model a nmos level=54\n.subckt c1 d g\nM1 d g 0 0 a l=1u w=1u\n{ends}\nX1 d g c1\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        assert_eq!(diagnostics(&parsed), [], "{ends}");
        assert_sizes(&files.build(parsed).0, &[("X1.M1", 1.0, 1.0)], ends);
    }
}

/// Values after the model name may also be quoted expressions and names of
/// parameters (decks `q_positional_expr`, `q_positional_names`,
/// `q_positional_mix`). Words after `key=value` are still an error.
#[tokio::test]
async fn positional_expressions() {
    let text = ".model a nmos level=54\n.param pl=2u pw=1u\n\
        M1 d g 0 0 a 'pl' 'pw'\nM2 d g 0 0 a pl pw\nM3 d g 0 0 a 2u pw\nM4 d g 0 0 a pl 1u\n\
        M5 d g 0 a 'pl' 1u\nM6 d g 0 0 a pl\nM7 d g 0 0 a 'pl' w=3u\nM8 d g 0 0 a\n+ 'pl*2'\n+ 'pw*2'\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let expected = [
        ("M1", 2.0, 1.0),
        ("M2", 2.0, 1.0),
        ("M3", 2.0, 1.0),
        ("M4", 2.0, 1.0),
        ("M5", 2.0, 1.0),
        ("M6", 2.0, 100.0),
        ("M7", 2.0, 3.0),
        ("M8", 4.0, 2.0),
    ];
    assert_sizes(&ast, &expected, "positional expressions");
    // printed, and read back
    let printed = ast.to_string();
    let (_dir2, parsed2, files2) = parse_text(&[("top.sp", &printed)]).await;
    assert_eq!(diagnostics(&parsed2), [], "{printed}");
    let (ast2, _) = files2.build(parsed2);
    assert_sizes(&ast2, &expected, "printed");

    // (a value after a keyword takes the next place: deck `rk_key_quoted_name`)
    let (_dir, parsed, _) = parse_text(&[("top.sp", "M1 d g 0 0 a l=1u 'pl'\n")]).await;
    assert_eq!(diagnostics(&parsed), []);
}

/// Texts that lead back to themselves, as measured on HSPICE (decks
/// `q_cycle_*` of the kit): the L and W of `X1.M1`, in um.
#[tokio::test]
async fn loops_as_measured_on_hspice() {
    let ac = "M1 d g s s nv l='a*1u' w='c*1u'\n.ends s\n";
    let kc = "M1 d g s s nv l='k*1u' w='c*1u'\n.ends s\n";
    let ak = "M1 d g s s nv l='a*1u' w='k*1u'\n.ends s\n";
    for (top, def, line, l, w) in [
        // a value passed, and a `.param` that read each other: as a whole, above
        (
            "a=1 c=2",
            format!(".subckt s d g s\n.param c='a*3'\n{ac}"),
            "a='c'",
            2.0,
            6.0,
        ),
        (
            "a=1 c=2",
            format!(".subckt s d g s a=5\n.param c='a*3'\n{ac}"),
            "a='c'",
            2.0,
            6.0,
        ),
        (
            "c=2",
            format!(".subckt s d g s\n.param c='a*3'\n{ac}"),
            "a='c'",
            2.0,
            6.0,
        ),
        (
            "a=1 b=2 c=3",
            ".subckt s d g s\n.param b='c*2' c='a*3'\nM1 d g s s nv l='a*1u' w='b*1u'\n.ends s\n"
                .to_owned(),
            "a='b'",
            2.0,
            12.0,
        ),
        // ... with the order of the line
        (
            "a=1 c=2",
            format!(".subckt s d g s\n{ac}"),
            "a='c' c='a'",
            2.0,
            2.0,
        ),
        (
            "a=1 c=2",
            format!(".subckt s d g s\n{ac}"),
            "c='a' a='c'",
            1.0,
            1.0,
        ),
        (
            "a=1 b=2 c=3",
            format!(".subckt s d g s\n{ac}"),
            "a='b' b='c' c='a'",
            2.0,
            2.0,
        ),
        // ... and all of its names
        (
            "a=1 c=2 k=2",
            format!(".subckt s d g s k=3\n.param c='a*3'\n{ak}"),
            "a='c*k'",
            4.0,
            3.0,
        ),
        (
            "a=1 c=2 k=2",
            format!(".subckt s d g s\n.param c='a*3'\n{ak}"),
            "k=7 a='c*k'",
            4.0,
            7.0,
        ),
        // a text that reads its own name: that name above, the others as usual
        (
            "a=1 c=2 k=2",
            format!(".subckt s d g s k=3\n{kc}"),
            "c='c*k'",
            3.0,
            6.0,
        ),
        (
            "a=1 c=2 k=2",
            format!(".subckt s d g s\n.param k=3 c='c*k'\n{kc}"),
            "",
            3.0,
            6.0,
        ),
        (
            "a=1 c=2 k=2",
            format!(".subckt s d g s k=3 c='c*k'\n{kc}"),
            "",
            3.0,
            6.0,
        ),
        (
            "a=1 c=2 k=2",
            format!(".subckt s d g s\n.param c='c*k'\n{kc}"),
            "k=7",
            7.0,
            14.0,
        ),
        (
            "a=1 c=2",
            format!(".subckt s d g s\n.param c='c*2'\n{ac}"),
            "",
            1.0,
            4.0,
        ),
        (
            "a=1 c=2",
            format!(".subckt s d g s c='c*2'\n{ac}"),
            "",
            1.0,
            4.0,
        ),
        // `c` reads a value later on the line, which reads its own name
        (
            "a=4",
            ".subckt s d g s c=1 a=2\nM1 d g s s nv l='a*1u' w='c*1u'\n.ends s\n".to_owned(),
            "c='a*3' a='a*2'",
            8.0,
            24.0,
        ),
    ] {
        let text = format!(
            ".option parhier=local\n.param {top}\n.model nv nmos level=54\n{def}X1 d g 0 s {line}\n"
        );
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        assert_sizes(&ast, &[("X1.M1", l, w)], &text);
    }
    // the loops that stop HSPICE
    for text in [
        // two `.param` that read each other, also with both names above
        ".param a=1 c=2\n.model nv nmos level=54\n.subckt s d g s\n.param a='c*2' c='a*3'\n\
         M1 d g s s nv l='a*1u' w='c*1u'\n.ends s\nX1 d g 0 s\n",
        // at the top level
        ".param a='c*2' c='a*3'\n.model nv nmos level=54\nM1 d g 0 0 nv l='a*1u' w='c*1u'\n",
        // with no name above the new instance
        ".model nv nmos level=54\n.subckt s d g s\n.param c='a*3'\n\
         M1 d g s s nv l='a*1u' w='c*1u'\n.ends s\nX1 d g 0 s a='c'\n",
    ] {
        let text = format!(".option parhier=local\n{text}");
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let models = ast.mosfet_models();
        assert!(models.iter().all(|m| m.selection.is_err()), "{text}");
    }
    // with parhier=global, the top level wins: no loop
    let text = ".param a=1 c=2\n.model nv nmos level=54\n.subckt s d g s\n.param c='a*3'\n\
        M1 d g s s nv l='a*1u' w='c*1u'\n.ends s\nX1 d g 0 s a='c'\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    let (ast, _) = files.build(parsed);
    assert_sizes(&ast, &[("X1.M1", 1.0, 2.0)], text);
}

/// Whether bins compare the width per finger (`W/NF`): `WNFLAG` on the
/// instance line, else `.option wnflag`, else they do. HSPICE ignores a
/// `WNFLAG` in a model (decks `q_nf_default`, `q_wnflag*`).
#[tokio::test]
async fn width_per_finger_flags() {
    let models = ".model a.1 nmos lmin=1e-8 lmax=1e-4 wmin=1e-8 wmax=1e-6\n\
        .model a.2 nmos lmin=1e-8 lmax=1e-4 wmin=1e-6 wmax=1e-3\n\
        .model b.1 nmos wnflag=1 lmin=1e-8 lmax=1e-4 wmin=1e-8 wmax=1e-6\n\
        .model b.2 nmos wnflag=1 lmin=1e-8 lmax=1e-4 wmin=1e-6 wmax=1e-3\n\
        .model c.1 nmos wnflag=0 lmin=1e-8 lmax=1e-4 wmin=1e-8 wmax=1e-6\n\
        .model c.2 nmos wnflag=0 lmin=1e-8 lmax=1e-4 wmin=1e-6 wmax=1e-3\n";
    // 4u over 8 fingers: 0.5u per finger; M=8 is not fingers
    let devices = "M1 d g s b a w=4u l=1u nf=8\nM2 d g s b b w=4u l=1u nf=8\n\
        M3 d g s b b w=4u l=1u nf=8 wnflag=0\nM4 d g s b a w=4u l=1u nf=8 wnflag=1\n\
        M5 d g s b c w=4u l=1u nf=8\nM6 d g s b a w=4u l=1u m=8\nM7 d g s b a w=4u l=1u nf=8 m=2\n";
    for (option, expected) in [
        ("", ["a.1", "b.1", "b.2", "a.1", "c.1", "a.2", "a.1"]),
        (
            ".option wnflag=1\n",
            ["a.1", "b.1", "b.2", "a.1", "c.1", "a.2", "a.1"],
        ),
        (
            ".option wnflag=0\n",
            ["a.2", "b.2", "b.2", "a.1", "c.2", "a.2", "a.2"],
        ),
    ] {
        let text = format!("{option}{models}{devices}");
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let got: Vec<_> = chosen(&ast).into_iter().map(|(_, m)| m).collect();
        assert_eq!(got, expected, "{option}");
    }
}

/// Counts through a binary tree whose every instance gets a different value
/// (each leaf's `p` is its path in binary): beyond a trial, the instances
/// of a definition that never repeat are not kept (they used to fill the
/// memory); the counts stay those of the listing.
#[tokio::test]
async fn model_usage_of_instances_that_never_repeat() {
    let depth = 13;
    let mut deck = String::from(
        ".option parhier=local\n\
         .model nw.1 nmos lmin=1e-10 lmax=1 wmin=1e-10 wmax=2.5e-7\n\
         .model nw.2 nmos lmin=1e-10 lmax=1 wmin=2.5e-7 wmax=4.5e-7\n\
         .model nw.3 nmos lmin=1e-10 lmax=1 wmin=4.5e-7 wmax=1e-5\n\
         .subckt b0 d p=0\nM0 d d 0 0 nw w='(1+p-7*floor(p/7))*100n' l=1u\n.ends\n",
    );
    for k in 1..=depth {
        deck += &format!(
            ".subckt b{k} d p=0\nX0 d b{} p='2*p'\nX1 d b{} p='2*p+1'\n.ends\n",
            k - 1,
            k - 1
        );
    }
    deck += &format!("X0 d b{depth} p=0\n");
    let (_dir, parsed, files) = parse_text(&[("top.sp", &deck)]).await;
    let (ast, _) = files.build(parsed);
    let mut listed = HashMap::<String, u64>::new();
    for m in ast.mosfet_models() {
        *listed
            .entry(m.selection.unwrap().model.name.to_string())
            .or_default() += 1;
    }
    let usage = ast.model_usage();
    let counted: HashMap<String, u64> = usage
        .models
        .iter()
        .map(|(m, n)| (m.name.to_string(), *n))
        .collect();
    assert_eq!(counted, listed);
    // p mod 7 = 0, 1 (W=100n, 200n): nw.1; 2, 3: nw.2; 4, 5, 6: nw.3
    let n = 1u64 << depth;
    let of = |r: u64| (n - 1 - r) / 7 + 1;
    assert_eq!(counted["nw.1"], of(0) + of(1));
    assert_eq!(counted["nw.2"], of(2) + of(3));
    assert_eq!(counted["nw.3"], of(4) + of(5) + of(6));
}

/// A subckt that instantiates itself does not make the walk loop forever, and
/// the line gets an error row: HSPICE stops there (decks `x_self`, `x_mutual`).
#[tokio::test]
async fn model_selection_stops_at_recursive_subckts() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nmod nmos level=1\n.subckt a x\nM1 x x 0 0 nmod\nX1 x a\n.ends\nX0 n a\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    let recursive = format!(
        "error: {}",
        crate::models::SelectError::Recursive("a".to_owned())
    );
    assert_eq!(
        chosen(&ast),
        pairs(&[("X0.M1", "nmod"), ("X0.X1", &recursive)])
    );
}

/// HSPICE stops at an `X` line with no definition on its instance path (golden
/// cases `hier_top_nested_only`, `hier_unres_in_subckt`, `hier_model_callee`).
/// The MOSFETs below that line are not known, so both walks report the line,
/// and a total counts only the MOSFETs found.
#[tokio::test]
async fn x_line_with_no_definition_is_an_error() {
    use crate::models::SelectError;
    let no_def = |name: &str| format!("error: {}", SelectError::NoSubckt(name.to_owned()));
    let usage_errors = |u: &crate::models::ModelUsage<'_, '_>| -> Vec<(String, u64, String)> {
        u.errors
            .iter()
            .map(|(e, n, path)| (format!("error: {e}"), *n, path.clone()))
            .collect()
    };

    // a top-level line, and a name that only another subckt nests
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nmod nmos level=1\n.subckt outer a b\n.subckt only a b\nM1 a b 0 0 nmod\n.ends\n\
         Xo a b only\n.ends\nX1 1 0 outer\nX2 2 0 only\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        vec![
            ("X1.Xo.M1".to_owned(), "nmod".to_owned()),
            ("X2".to_owned(), no_def("only")),
        ]
    );
    let u = ast.model_usage();
    assert_eq!(u.mosfets, 1);
    assert_eq!(usage_errors(&u), [(no_def("only"), 1, "X2".to_owned())]);

    // one line that resolves on one instance path and not on another
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nmod nmos level=1\n.subckt t a b\nXn a b only\n.ends\n.subckt outer a b\n\
         .subckt only a b\nM1 a b 0 0 nmod\n.ends\nXt a b t\n.ends\nX1 1 0 outer\nX2 2 0 t\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    assert_eq!(
        chosen(&ast),
        vec![
            ("X1.Xt.Xn.M1".to_owned(), "nmod".to_owned()),
            ("X2.Xn".to_owned(), no_def("only")),
        ]
    );
    let u = ast.model_usage();
    assert_eq!(u.mosfets, 1);
    assert_eq!(usage_errors(&u), [(no_def("only"), 1, "X2.Xn".to_owned())]);
    // the same line from an instance of `t` alone
    let of = ast.mosfet_models_of("t").unwrap();
    assert_eq!(of.len(), 1);
    assert_eq!(of[0].path, "Xn");
    let u = ast.model_usage_of("t").unwrap();
    assert_eq!(u.mosfets, 0);
    assert_eq!(usage_errors(&u), [(no_def("only"), 1, "Xn".to_owned())]);

    // the name of a model is not a subckt
    let (_dir, parsed, files) = parse_text(&[("top.sp", ".model rmod r\nX1 1 0 rmod\n")]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(chosen(&ast), vec![("X1".to_owned(), no_def("rmod"))]);
    assert_eq!(ast.model_usage().mosfets, 0);

    // counted through the hierarchy, as the MOSFETs are
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nmod nmos level=1\n.subckt leaf a\nM1 a a 0 0 nmod\nXgone a gone\n.ends\n\
         .subckt mid a\nX1 a leaf\nX2 a leaf\n.ends\nX1 n mid\nX2 n mid\nX3 n mid\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    let u = ast.model_usage();
    assert_eq!(u.mosfets, 6);
    assert_eq!(u.models.len(), 1);
    assert_eq!(u.models[0].1, 6);
    assert_eq!(
        usage_errors(&u),
        [(no_def("gone"), 6, "X1.X1.Xgone".to_owned())]
    );
    let listed = chosen(&ast);
    assert_eq!(listed.len(), 12);
    assert_eq!(
        listed.iter().filter(|(_, m)| *m == no_def("gone")).count(),
        6
    );

    // a definition in a plain `.prot` block is a definition (golden case
    // `prot_subckt`), and a name is found in any case
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nmod nmos level=1\n.prot\n.subckt secret a b\nM1 a b 0 0 nmod\n.ends\n.unprot\n\
         X1 1 0 SECRET\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    assert_eq!(chosen(&ast), pairs(&[("X1.M1", "nmod")]));
    assert!(ast.model_usage().errors.is_empty());
}

/// HSPICE: the netlist ends at the first `.alter`; what follows describes
/// other runs (other corners, other parameters). It used to be parsed into
/// the same netlist, so every corner's `.lib` and `.param` were merged (the
/// last one winning). It is now kept verbatim, not parsed.
#[tokio::test]
async fn alter_blocks_are_not_merged() {
    let (_dir, parsed, files) = parse_text(&[
        (
            "top.sp",
            ".lib 'corners.l' tt\n.param w=1u\nM1 d g s b nmod w=w l=1u\n\
             .alter ss corner\n.del lib 'corners.l' tt\n.lib 'corners.l' ss\n.param w=2u\n\
             .alter\n.param w=3u\n.end\n",
        ),
        (
            "corners.l",
            ".lib tt\n.param dx=0\n.model nmod nmos level=1\n.endl tt\n\
             .lib ss\n.param dx=1n\n.model nmod nmos level=1\n.endl ss\n",
        ),
    ])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    // one corner, one model, one value of each parameter
    assert_eq!(ast.model.len(), 1);
    let scope = ast.scope();
    assert_eq!((scope.get("w"), scope.get("dx")), (Ok(1e-6), Ok(0.0)));
    // the `.alter` blocks are kept, verbatim, and printed back
    let alter = ast
        .unknwon
        .iter()
        .find(|u| u.cmd.eq_ignore_ascii_case("alter"))
        .unwrap();
    assert!(alter.raw.starts_with("ss corner\n.del lib") && alter.raw.ends_with(".end"));
    let printed = ast.to_string();
    let (_dir2, parsed2, files2) = parse_deck(&[("top.sp", &printed)]).await;
    assert_eq!(files2.build(parsed2).0.to_string(), printed);
}

/// A hierarchy 250 subckts deep (the walk stops at `MAX_HIERARCHY_DEPTH`)
/// does not overflow the stack, and parameters pass all the way down.
#[tokio::test]
async fn model_selection_in_a_deep_hierarchy() {
    let mut text = String::from(".option parhier=local\n.model nmod nmos level=1\n");
    for i in 0..250 {
        writeln!(
            text,
            ".subckt s{i} a w=1u\nX{i} a s{} w='w+1n'\n.ends",
            i + 1
        )
        .unwrap();
    }
    text.push_str(".subckt s250 a w=1u\nM1 a a 0 0 nmod w=w l=1u\n.ends\nX a s0\n");
    let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    let mosfets = ast.mosfet_models();
    assert_eq!(mosfets.len(), 1);
    let s = mosfets[0].selection.as_ref().unwrap();
    assert!(close(s.w, 1e-6 + 250e-9), "{}", s.w);
    assert_eq!(
        crate::Subckt::append(
            "s0",
            |_| false,
            &mut Vec::new(),
            &mut vec![&ast.subckt],
            &mut Vec::new(),
            &mut Vec::new()
        ),
        Ok(())
    );
}

/// `model_usage` (counted through the hierarchy, each distinct subckt
/// instance once) gives the same counts as listing every MOSFET.
#[tokio::test]
async fn model_usage_counts_like_the_list() {
    /// MOSFETs; how many use each model; how many have each error
    type Tally = (u64, Vec<(String, u64)>, Vec<(String, u64)>);
    fn tally(ast: &AST<'_>) -> Tally {
        let mut models = HashMap::<String, u64>::new();
        let mut errors = HashMap::<String, u64>::new();
        let list = ast.mosfet_models();
        for m in &list {
            match &m.selection {
                Ok(s) => *models.entry(s.model.name.to_string()).or_default() += 1,
                Err(e) => *errors.entry(e.to_string()).or_default() += 1,
            }
        }
        let mut models: Vec<_> = models.into_iter().collect();
        let mut errors: Vec<_> = errors.into_iter().collect();
        models.sort();
        errors.sort();
        (list.len() as u64, models, errors)
    }
    fn usage(ast: &AST<'_>) -> Tally {
        let u = ast.model_usage();
        let mut models = HashMap::<String, u64>::new();
        for (m, n) in &u.models {
            *models.entry(m.name.to_string()).or_default() += n;
        }
        let mut models: Vec<_> = models.into_iter().collect();
        let mut errors: Vec<_> = u
            .errors
            .iter()
            .map(|(e, n, _)| (e.to_string(), *n))
            .collect();
        models.sort();
        errors.sort();
        (u.mosfets, models, errors)
    }
    // the same subckt under different parameters, levels without parameters,
    // local models, errors, and repeated instances
    let deck = ".param dxln=0 lg=30n\n\
        .model nch.1 nmos lmin=1e-8 lmax=1e-7 wmin=1e-8 wmax=1e-6\n\
        .model nch.2 nmos lmin=1e-8 lmax=1e-7 wmin=1e-6 wmax=1e-4\n\
        .model nch.3 nmos lmin=1e-7 lmax=1e-4 wmin=1e-8 wmax=1e-4\n\
        .subckt cell a b wn=0.5u l=lg\n\
        M1 a b 0 0 nch w=wn l=l\nM2 a b 0 0 nch w='4*wn' l=l\nM3 a b 0 0 pch w=wn l=l\n.ends\n\
        .subckt blk a b\n\
        X1 a b cell\nX2 a b cell wn=1u\nX3 a b cell l=200n\nX4 a b cell\nM9 a b 0 0 nch w=2u l=lx\n.ends\n\
        .subckt tile a b sz=1u\n\
        Xb1 a b blk\nXb2 a b blk\nXc a b cell wn=sz\n.ends\n\
        .subckt mac a b l=1u w=1u\n\
        MAIN a b 0 0 nch l=l w=w\n\
        .model nch.1 nmos lmin=1e-8 lmax=1 wmin=1e-8 wmax=1\n.ends\n\
        Xt1 a b tile\nXt2 a b tile sz=2u\nXt3 a b tile\nXm1 a b mac\nXm2 a b mac l=50n\nM0 a b 0 0 nch w=1u l=1u\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", deck)]).await;
    let (ast, _) = files.build(parsed);
    let expected = tally(&ast);
    assert_eq!(expected.0, 3 * (2 * (4 * 3 + 1) + 3) + 2 + 1);
    assert_eq!(usage(&ast), expected);
    // and the models of one subckt
    let u = ast.model_usage_of("tile").unwrap();
    assert_eq!(u.mosfets, 2 * (4 * 3 + 1) + 3);
    assert!(ast.model_usage_of("nothing").is_none());
    // with `.option parhier=local`, `.option scale`, and fingers
    for deck in [
        ".option parhier=local\n.param w=1u\n.subckt inv a b w=2u\nM1 a b 0 0 nmod w=w l=1u\n.ends\n\
         .model nmod nmos level=1\nX1 a b inv\nX2 a b inv w=3u\n",
        ".option scale=1e-6\n.model nch.1 nmos wnflag=1 lmin=1e-7 lmax=1e-6 wmin=1e-7 wmax=1e-6\n\
         .model nch.2 nmos wnflag=1 lmin=1e-7 lmax=1e-6 wmin=1e-6 wmax=1e-5\n\
         M1 d g s b nch l=0.18 w=4 nf=8\nM2 d g s b nch l=0.18 w=4\n",
    ] {
        let (_dir, parsed, files) = parse_text(&[("top.sp", deck)]).await;
        let (ast, _) = files.build(parsed);
        assert_eq!(usage(&ast), tally(&ast));
    }
    // the counts of a subckt instance are kept for the parameters its
    // contents read: values passed that nothing reads (`id`), values read
    // levels below (`k`, `lsel`), a definition that hides another (`wq`),
    // the ranges of local bins (`dx`), and functions; with each `parhier`
    let models = ".model nch.1 nmos lmin=1e-8 lmax=1e-7 wmin=1e-8 wmax=1e-6\n\
        .model nch.2 nmos lmin=1e-8 lmax=1e-7 wmin=1e-6 wmax=1e-4\n\
        .model nch.3 nmos lmin=1e-7 lmax=1e-4 wmin=1e-8 wmax=1e-6\n\
        .model nch.4 nmos lmin=1e-7 lmax=1e-4 wmin=1e-6 wmax=1e-4\n";
    let decks = [
        ".param lsel=50n k=0.3u\n\
         .subckt leaf a b wl=1u\nM1 a b 0 0 nch w=wl l=lsel\n.ends\n\
         .subckt mid a b id=0\nX1 a b leaf\nX2 a b leaf wl='2*k'\nX3 a b leaf id=id\n.ends\n\
         .subckt top a b id=0 k=1u\nXm1 a b mid id=1\nXm2 a b mid id=2\nXm3 a b mid\n.ends\n\
         Xt1 a b top id=1\nXt2 a b top id=2 k=0.6u\nXt3 a b top k=0.6u\nXt4 a b top id=4\n\
         Xt5 a b mid\nXt6 a b leaf lsel=200n\n",
        ".param wq=0.5u\n\
         .subckt cellq a b\nM1 a b 0 0 nch w=wq l=50n\n.ends\n\
         .subckt blkq a b wq=3u id=0\nX1 a b cellq\nX2 a b cellq wq=0.2u\n.ends\n\
         .subckt topq a b\nXa a b blkq id=1\nXb a b blkq id=2\nXc a b blkq wq=2u\n.ends\n\
         X1 a b blkq\nX2 a b cellq\nX3 a b topq\nX4 a b topq\n",
        ".subckt mac2 a b l=1u w=1u dx=0 tag=0\n\
         .model nmac.1 nmos lmin='1e-7-dx' lmax='5e-7-dx' wmin=1e-8 wmax=1\n\
         .model nmac.2 nmos lmin='5e-7-dx' lmax=1 wmin=1e-8 wmax=1\n\
         M1 a b 0 0 nmac l=l w=w\n.ends\n\
         .subckt pair a b tag=0 dx=0\nX1 a b mac2 l=4.95e-7 tag=tag\nX2 a b mac2 l=4.95e-7 dx=dx\n.ends\n\
         X1 a b mac2 l=4.95e-7 tag=1\nX2 a b mac2 l=4.95e-7 dx=1e-8 tag=2\n\
         X3 a b mac2 l=4.95e-7 dx=1e-8 tag=3\nX4 a b pair tag=4\nX5 a b pair dx=1e-8\nX6 a b pair tag=6\n",
        ".param f(x)='2*x' wf=0.4u\n\
         .subckt fcell a b wf=0.3u\nM1 a b 0 0 nch w='f(wf)' l=50n\n.ends\n\
         .subckt fblk a b id=0\nX1 a b fcell\nX2 a b fcell wf=0.6u\n.ends\n\
         X1 a b fblk id=1\nX2 a b fblk id=2\nX3 a b fcell\n",
        // a subckt that calls a function is counted for its context: what
        // its parent passes (`k`) changes it
        ".param f(x)='2*x'\n\
         .subckt fc a b\nM1 a b 0 0 nch w='f(k)*1u' l=50n\n.ends\n\
         .subckt fp a b k=0.1\nX1 a b fc\nX2 a b fc\n.ends\n\
         X1 a b fp k=0.1\nX2 a b fp k=0.3\nX3 a b fp k=0.6\nX4 a b fp\n",
        ".temp 85\n.subckt tcell a b\nM1 a b 0 0 nch w='temper*1e-8' l=50n\n.ends\n\
         .subckt tblk a b temper=1\nX1 a b tcell\n.ends\n\
         X1 a b tcell\nX2 a b tblk\nX3 a b tblk temper=200\n",
        // a value passed is computed in the new instance: `k` of its subckt,
        // of the line, or from above, by parhier
        ".param k=0.3u\n\
         .subckt oc a b k=0.2u wl=0\nM1 a b 0 0 nch w=wl l=50n\n.ends\n\
         .subckt op a b\nX1 a b oc wl='k'\nX2 a b oc wl='2*k'\nX3 a b oc k=3u wl='k'\n.ends\n\
         X1 a b op\nX2 a b oc wl='k'\nX3 a b oc k=3u wl='k'\nX4 a b op\n",
        // the model of a line depends on the instances above: bins of an
        // instance above, without parameters and with them
        ".subckt leaf a b\nM1 a b 0 0 nch w=1u l=50n\n.ends\n\
         .subckt host a b\n.model nch.1 nmos lmin=1e-8 lmax=1 wmin=1e-8 wmax=1\nX1 a b leaf\n.ends\n\
         X1 a b leaf\nX2 a b host\nX3 a b leaf\nX4 a b host\n",
        ".subckt leafm a b\nM1 a b 0 0 lo w=1u l=5e-7\n.ends\n\
         .subckt hostm a b lb=1e-6\n\
         .model lo.1 nmos lmin=1e-8 lmax='lb' wmin=1e-8 wmax=1\n\
         .model lo.2 nmos lmin='lb' lmax=1 wmin=1e-8 wmax=1\n\
         X1 a b leafm\nX2 a b leafm\n.ends\n\
         X1 a b hostm\nX2 a b hostm lb=1e-7\nX3 a b hostm\n",
        // so does the subckt of a line; here each host also passes a value
        ".subckt sel a b\nX1 a b cell\n.ends\n\
         .subckt cell a b wc=1u\nM1 a b 0 0 nch w=wc l=50n\n.ends\n\
         .subckt host2 a b wc=0.5u\n.subckt cell a b\nM1 a b 0 0 nch w=wc l=200n\n.ends\n\
         X1 a b sel\n.ends\n\
         X1 a b sel\nX2 a b host2\nX3 a b host2 wc=2u\nX4 a b sel\n",
    ];
    for deck in decks {
        for option in ["", ".option parhier=local\n"] {
            let text = format!("{option}{models}{deck}");
            let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
            let (ast, _) = files.build(parsed);
            let expected = tally(&ast);
            // (with GLOBAL, a top-level definition can leave one model)
            assert!(expected.1.len() > 1 || option.is_empty(), "{text}");
            assert_eq!(usage(&ast), expected, "{text}");
        }
    }
}

// ---------------------------------------------------------------------------
// `.if` blocks (each result as measured on HSPICE P-2019.06, decks `if_*`,
// `if2_*` and `ord_*` of `scripts/hspice_kit/forms*.py`)

/// Whether the deck `text` has errors (not warnings), with its MOSFETs as
/// `(path, chosen model or error)`
async fn if_deck(text: &str) -> (bool, Vec<(String, String)>) {
    let (_dir, parsed, files) = parse_deck(&[("deck.sp", text)]).await;
    let (ast, has_err) = files.build(parsed);
    let rows = chosen(&ast);
    (has_err, rows)
}

/// `(path, L in um)` of every MOSFET of the deck `text`
async fn if_lengths(text: &str) -> Vec<(String, f64)> {
    let (_dir, parsed, files) = parse_deck(&[("deck.sp", text)]).await;
    let (ast, _) = files.build(parsed);
    ast.mosfet_models()
        .into_iter()
        .map(|m| match &m.selection {
            Ok(s) => (m.path, (s.l * 1e12).round() / 1e6),
            Err(e) => panic!("{}: {e}\n{text}", m.path),
        })
        .collect()
}

const IF_N: &str = "* t\n.model n nmos level=54\n";
const IF_M1: &str = "M1 d g 0 0 n l=1u w=1u\n";
const IF_M2: &str = "M2 d g 0 0 n l=2u w=2u\n";
const IF_M3: &str = "M3 d g 0 0 n l=3u w=3u\n";

fn if_paths(rows: &[(String, String)]) -> Vec<&str> {
    rows.iter().map(|(p, _)| p.as_str()).collect()
}

#[tokio::test]
async fn if_counts_only_the_active_branch() {
    for (head, want) in [
        (
            ".if (1)\nM1 d g 0 0 n l=1u w=1u\n.else\nM2 d g 0 0 n l=2u w=2u\n.endif\n",
            "M1",
        ),
        (
            ".if (0)\nM1 d g 0 0 n l=1u w=1u\n.else\nM2 d g 0 0 n l=2u w=2u\n.endif\n",
            "M2",
        ),
        (
            ".IF(1)\nM1 d g 0 0 n l=1u w=1u\n.ELSE\nM2 d g 0 0 n l=2u w=2u\n.ENDIF\n",
            "M1",
        ),
        (".if ('1>0')\nM1 d g 0 0 n l=1u w=1u\n.endif\n", "M1"),
        (".if (\"1>0\")\nM1 d g 0 0 n l=1u w=1u\n.endif\n", "M1"),
        (
            ".if ((1>0)&&(2>1)) $ note\nM1 d g 0 0 n l=1u w=1u\n.endif $ note\n",
            "M1",
        ),
        (
            ".if (0)\nM1 d g 0 0 n l=1u w=1u\n.elseif(1)\nM2 d g 0 0 n l=2u w=2u\n.else\nM3 d g 0 0 n l=3u w=3u\n.endif\n",
            "M2",
        ),
        (
            ".if (1)\nM1 d g 0 0 n l=1u w=1u\n.elseif (1)\nM2 d g 0 0 n l=2u w=2u\n.else\nM3 d g 0 0 n l=3u w=3u\n.endif\n",
            "M1",
        ),
        (
            ".if (0)\nM1 d g 0 0 n l=1u w=1u\n.elseif (0)\nM2 d g 0 0 n l=2u w=2u\n.else\nM3 d g 0 0 n l=3u w=3u\n.endif\n",
            "M3",
        ),
    ] {
        let (has_err, rows) = if_deck(&format!("{IF_N}{head}")).await;
        assert!(!has_err, "{head}");
        assert_eq!(if_paths(&rows), [want], "{head}");
    }
}

/// A value is true when it is not 0, however small
#[tokio::test]
async fn if_truth_is_any_value_that_is_not_zero() {
    for value in ["0.5", "-1", "1e-20", "1e-300"] {
        let text = format!("{IF_N}.param t={value}\n.if (t)\n{IF_M1}.else\n{IF_M2}.endif\n");
        assert_eq!(if_paths(&if_deck(&text).await.1), ["M1"], "{value}");
    }
    let text = format!("{IF_N}.if (0)\n{IF_M1}.else\n{IF_M2}.endif\n");
    assert_eq!(if_paths(&if_deck(&text).await.1), ["M2"]);
}

/// Text in a branch that is not active is not read: names need no definition
#[tokio::test]
async fn if_inactive_branch_is_not_evaluated() {
    for body in [
        ".if (0)\n.if (undefined_name)\nM1 d g 0 0 n l=1u w=1u\n.endif\n.endif\nM2 d g 0 0 n l=2u w=2u\n",
        ".if (1)\nM2 d g 0 0 n l=2u w=2u\n.elseif (undefined_name)\nM3 d g 0 0 n l=3u w=3u\n.endif\n",
        ".if (0)\nM1 d g 0 0 nomodel l=1u w=1u\nX1 d g nosubckt\n.param q='undefined_name'\n.endif\nM2 d g 0 0 n l=2u w=2u\n",
    ] {
        let (has_err, rows) = if_deck(&format!("{IF_N}{body}")).await;
        assert!(!has_err, "{body}");
        assert_eq!(if_paths(&rows), ["M2"], "{body}");
        assert!(rows[0].1 == "n", "{body}: {rows:?}");
    }
}

/// A condition is evaluated for each instance, with the values passed to it
#[tokio::test]
async fn if_is_evaluated_for_each_instance() {
    let text = format!(
        "{IF_N}.macro s d g w=1u\n.if (w>2u)\nM1 d g 0 0 n l=1u w=w\n.else\nM2 d g 0 0 n l=2u w=w\n.endif\n.eom\n\
         X1 d g s w=1u\nX2 d g s w=3u\n"
    );
    assert_eq!(if_paths(&if_deck(&text).await.1), ["X1.M2", "X2.M1"]);
    // with parhier: the top level wins over a value passed (global only)
    let body = ".param k=1\n.subckt s d g\n.if (k==1)\nM1 d g 0 0 n l=1u w=1u\n.else\nM2 d g 0 0 n l=2u w=2u\n.endif\n.ends\nX1 d g s k=2\n";
    let global = format!("{IF_N}{body}");
    assert_eq!(if_paths(&if_deck(&global).await.1), ["X1.M1"]);
    let local = format!("{IF_N}.option parhier=local\n{body}");
    assert_eq!(if_paths(&if_deck(&local).await.1), ["X1.M2"]);
}

/// The values of `.param` follow the order of the file: a condition sees all
/// plain `.param` statements of its level, and the branches of earlier blocks
#[tokio::test]
async fn if_params_follow_the_order_of_the_file() {
    let b = |q: &str| format!(".if (1)\n.param q={q}\n.endif\n");
    let use_q = "M1 d g 0 0 n l=q w=1u\n";
    for (body, want) in [
        (format!(".param q=1u\n{}{use_q}", b("2u")), 2.0),
        (format!("{}.param q=1u\n{use_q}", b("2u")), 1.0),
        (format!(".if (0)\n.param q=2u\n.endif\n.param q=1u\n{use_q}"), 1.0),
        (format!("{}{}{use_q}", b("1u"), b("2u")), 2.0),
        // a text read later
        (format!(".if (1)\n.param r='q*2'\n.endif\n.param q=1u\nM1 d g 0 0 n l=r w=1u\n"), 2.0),
        (format!(".param q=1u\n.param r='q*2'\n{}M1 d g 0 0 n l=r w=1u\n", b("2u")), 4.0),
        // nested blocks
        (format!(".if (1)\n{}.param q=1u\n.endif\n{use_q}", b("2u")), 1.0),
        (format!(".if (1)\n.param q=1u\n{}.endif\n{use_q}", b("2u")), 2.0),
        (
            ".if (1)\n.param q=1\n.if (q==1)\n.param q=2\n.endif\n.if (q==2)\n.param q=3\n.endif\n.endif\nM1 d g 0 0 n l='q*1u' w=1u\n".to_owned(),
            3.0,
        ),
    ] {
        let text = format!("{IF_N}{body}");
        assert_eq!(if_lengths(&text).await, [("M1".to_owned(), want)], "{body}");
    }
    let m = format!("{IF_M1}.else\n{IF_M2}.endif\n");
    for (body, want) in [
        (format!(".param q=1\n.if (q>1)\n{m}.param q=2\n"), "M1"),
        (format!(".if (q>1)\n{m}.param q=1\n.param q=2\n"), "M1"),
        (format!(".param q=1\n{}.if (q>1)\n{m}", b("2")), "M1"),
        (format!("{}.param q=1\n.if (q>1)\n{m}", b("2")), "M2"),
        (format!("{}.if (q>1)\n{m}.param q=1\n", b("2")), "M2"),
        (format!(".param q=1\n.if (q>1)\n{m}{}", b("2")), "M2"),
    ] {
        let text = format!("{IF_N}{body}");
        assert_eq!(if_paths(&if_deck(&text).await.1), [want], "{body}");
    }
}

/// A value passed hides a branch's `.param`, which hides a `.subckt` default
#[tokio::test]
async fn if_params_of_an_instance() {
    let text = format!(
        "{IF_N}.option parhier=local\n.subckt s d g k=1\n.if (k==1)\n.param k=2\n.endif\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\n\
         X1 d g s\nX2 d g s k=5\nX3 d g s k=1\n"
    );
    assert_eq!(
        if_lengths(&text).await,
        [
            ("X1.M1".to_owned(), 2.0),
            ("X2.M1".to_owned(), 5.0),
            ("X3.M1".to_owned(), 1.0)
        ]
    );
    // the top level's branches are of the top level
    let body = ".if (1)\n.param k=9\n.endif\n.subckt s d g k=2\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s\n";
    assert_eq!(
        if_lengths(&format!("{IF_N}{body}")).await,
        [("X1.M1".to_owned(), 9.0)]
    );
    let local = format!("{IF_N}.option parhier=local\n{body}");
    assert_eq!(if_lengths(&local).await, [("X1.M1".to_owned(), 2.0)]);
    // a false branch binds nothing
    let hide = |c: u8| {
        format!(
            "{IF_N}.option parhier=local\n.param k=9\n.subckt s d g\n.if ({c})\n.param k=2\n.endif\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s\n"
        )
    };
    assert_eq!(if_lengths(&hide(1)).await, [("X1.M1".to_owned(), 2.0)]);
    assert_eq!(if_lengths(&hide(0)).await, [("X1.M1".to_owned(), 9.0)]);
}

/// Models and subckts in a branch are seen by the whole level, also above the
/// block. Bins keep the order of the file across a branch and the level.
#[tokio::test]
async fn if_models_and_subckts_in_a_branch() {
    let text = format!(
        "* t\nM1 d g 0 0 q l=1u w=1u\n.if (1)\n.model q nmos level=54\n.else\n.model q pmos level=54\n.endif\n"
    );
    let (has_err, rows) = if_deck(&text).await;
    assert!(!has_err);
    assert_eq!(rows, pairs(&[("M1", "q")]));
    let text = format!(
        "{IF_N}.subckt a d g\n{IF_M1}.ends\n.subckt b d g\n{IF_M2}.ends\n.param t=2\n.if (t==1)\nXs d g a\n.elseif (t==2)\nXs d g b\n.else\nXs d g a\n.endif\n"
    );
    assert_eq!(if_paths(&if_deck(&text).await.1), ["Xs.M2"]);
    // a branch defines the same name in each case
    let text = format!(
        "{IF_N}.param t=1\n.if (t==1)\n.subckt c d g\n{IF_M1}.ends\n.else\n.subckt c d g\n{IF_M2}.ends\n.endif\nX1 d g c\n"
    );
    assert_eq!(if_paths(&if_deck(&text).await.1), ["X1.M1"]);
    let bin =
        |s: &str| format!(".model b.{s} nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-3\n");
    let m = "M1 d g 0 0 b l=1u w=1u\n";
    let first_plain = format!(
        "* t\n.model b.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-3\n.if (1)\n{}.endif\n{m}",
        bin("1")
    );
    assert_eq!(if_deck(&first_plain).await.1, pairs(&[("M1", "b.2")]));
    let first_branch = format!("* t\n.if (1)\n{}.endif\n{}{m}", bin("1"), bin("2"));
    assert_eq!(if_deck(&first_branch).await.1, pairs(&[("M1", "b.1")]));
}

/// The lines of a level keep the order of the file, with a branch in its place
#[tokio::test]
async fn if_lines_keep_the_order_of_the_file() {
    let text = format!("{IF_N}{IF_M1}.if (1)\n{IF_M2}.endif\n{IF_M3}");
    assert_eq!(if_paths(&if_deck(&text).await.1), ["M1", "M2", "M3"]);
}

/// `.if` can open in one file and close in another; `.lib` and `.inc` are read
/// in a true branch
#[tokio::test]
async fn if_across_files_and_libraries() {
    let (_dir, parsed, files) = parse_deck(&[
        ("deck.sp", "* t\n.model n nmos level=54\n.if (0)\n.inc 'mid.inc'\nM3 d g 0 0 n l=3u w=3u\n.endif\n"),
        ("mid.inc", "M1 d g 0 0 n l=1u w=1u\n.else\nM2 d g 0 0 n l=2u w=2u\n"),
    ])
    .await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(if_paths(&chosen(&ast)), ["M2", "M3"]);

    let lib = ".lib tt\n.param lc=1u\n.model q nmos level=54\n.endl tt\n.lib ss\n.param lc=2u\n.model q pmos level=54\n.endl ss\n";
    let (_dir, parsed, files) = parse_deck(&[
        ("deck.sp", "* t\n.param corner=2\n.if (corner==1)\n.lib 'c.l' tt\n.else\n.lib 'c.l' ss\n.endif\nM1 d g 0 0 q l=lc w=1u\n"),
        ("c.l", lib),
    ])
    .await;
    let (ast, _) = files.build(parsed);
    let m = ast.mosfet_models().remove(0);
    let s = m.selection.unwrap();
    assert!(close(s.l, 2e-6), "{}", s.l);
    assert!(matches!(s.model.model_type, crate::ast::ModelType::PMOS));
}

/// HSPICE stops at these forms: each one is an error here
#[tokio::test]
async fn if_bad_forms_are_errors() {
    for body in [
        // a block that does not close, or closes what is not open
        ".if (1)\nM1 d g 0 0 n l=1u w=1u\n",
        ".endif\n",
        ".else\n",
        ".elseif (1)\n",
        ".if (1)\n.else\n.else\n.endif\n",
        ".if (1)\n.else\n.elseif (1)\n.endif\n",
        // across a body, or with `.alter` in it
        ".subckt s d g\n.if (1)\n.ends\n.endif\n",
        ".if (1)\n.subckt s d g\n.endif\n.ends\n",
        ".if (1)\n.alter\n.endif\n",
        // the condition
        ".if\n.endif\n",
        ".if ()\n.endif\n",
        ".if 1\n.endif\n",
        ".if {1}\n.endif\n",
        ".if (1) && (1)\n.endif\n",
        ".if (1\n+ )\n.endif\n",
        ".if (0)\n.elseif 1\n.endif\n",
        ".if (1) text\n.endif\n",
        ".if (1)\n+ text\n.endif\n",
        // text after the keywords
        ".if (1)\n.else text\n.endif\n",
        ".if (1)\n.else if (1)\n.endif\n",
        ".if (1)\n.endif text\n",
        ".if (1)\n.endif\n+ w=3u\n",
    ] {
        let (has_err, _) = if_deck(&format!("{IF_N}{body}")).await;
        assert!(has_err, "no error for:\n{body}");
    }
    // `.elsif` is not a keyword: it is an unknown command, and is ignored
    let text = format!("{IF_N}.if (0)\n{IF_M1}.elsif (1)\n{IF_M2}.else\n{IF_M3}.endif\n");
    let (has_err, rows) = if_deck(&text).await;
    assert!(!has_err);
    assert_eq!(if_paths(&rows), ["M3"]);
}

/// A condition that cannot be computed is one row with an error; it is not a
/// MOSFET, and the lines around the block still count
#[tokio::test]
async fn if_condition_that_cannot_be_computed() {
    for cond in ["nothing>1", "1=1", "'a'=='a'", "temper>50"] {
        let text = format!(
            "{IF_N}.subckt s d g\n{IF_M1}.if ({cond})\n{IF_M2}.else\n{IF_M3}.endif\n.ends\nX1 d g s\nX2 d g s\n"
        );
        let (_dir, parsed, files) = parse_deck(&[("deck.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let rows = chosen(&ast);
        assert_eq!(rows.len(), 4, "{cond}: {rows:?}");
        assert_eq!(rows[0].0, "X1.if", "{cond}");
        assert!(rows[0].1.starts_with("error: "), "{cond}: {rows:?}");
        assert_eq!(rows[1], ("X1.M1".to_owned(), "n".to_owned()), "{cond}");
        assert!(matches!(
            ast.mosfet_models()[0].selection,
            Err(crate::models::SelectError::Condition { .. })
        ));
        let usage = ast.model_usage();
        assert_eq!(usage.mosfets, 2, "{cond}");
        assert_eq!(usage.errors.len(), 1, "{cond}");
        assert_eq!(usage.errors[0].1, 2, "{cond}");
    }
}

/// `.option` in an active branch applies; in another branch it does not
#[tokio::test]
async fn if_options() {
    let parhier = |c: u8| {
        format!(
            "{IF_N}.if ({c})\n.option parhier=local\n.endif\n.param k=1u\n.subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n"
        )
    };
    assert_eq!(if_lengths(&parhier(1)).await, [("X1.M1".to_owned(), 2.0)]);
    assert_eq!(if_lengths(&parhier(0)).await, [("X1.M1".to_owned(), 1.0)]);
    let scale =
        |c: u8| format!("{IF_N}.if ({c})\n.option scale=1e-6\n.endif\nM1 d g 0 0 n l=0.5 w=2\n");
    assert_eq!(if_lengths(&scale(1)).await, [("M1".to_owned(), 0.5)]);
    assert_eq!(if_lengths(&scale(0)).await, [("M1".to_owned(), 500000.0)]);
    // the order of the file decides between a branch and the level
    let order = |before: &str, after: &str| {
        format!("{IF_N}{before}.if (1)\n.option scale=2e-6\n.endif\n{after}M1 d g 0 0 n l=1 w=1\n")
    };
    assert_eq!(
        if_lengths(&order(".option scale=1e-6\n", "")).await,
        [("M1".to_owned(), 2.0)]
    );
    assert_eq!(
        if_lengths(&order("", ".option scale=1e-6\n")).await,
        [("M1".to_owned(), 1.0)]
    );
}

/// [`AST::model_usage`] counts the active branches of each instance, as the
/// list does
#[tokio::test]
async fn if_usage_agrees_with_the_list() {
    let leaf = ".subckt leaf d g v=1\n.if (v>1)\n.param w0=1u\nM1 d g 0 0 n l=1u w=w0\n.else\nM2 d g 0 0 p l=2u w=2u\nM3 d g 0 0 p l=2u w=2u\n.endif\n.ends\n";
    let models = "* t\n.model n nmos level=54\n.model p pmos level=54\n";
    let x4 = "X1 d g mid\nX2 d g mid v=2\nX3 d g mid\nX4 d g mid v=2\n";
    let mid = ".subckt mid d g v=1\nX1 d g leaf v=v\nX2 d g leaf\n.ends\n";
    let branch_model = ".subckt inner d g\nM1 d g 0 0 mm l=1u w=1u\n.ends\n\
         .subckt host d g k=1\n.if (k==1)\n.model mm nmos level=54\n.else\n.model mm pmos level=54\n.endif\nX1 d g inner\nX2 d g inner\n.ends\n\
         X1 d g host\nX2 d g host k=2\nX3 d g host\n";
    let branch_subckt = ".subckt host d g k=1\n.if (k==1)\n.subckt inner d g\nM1 d g 0 0 n l=1u w=1u\n.ends\n.else\n.subckt inner d g\nM1 d g 0 0 p l=1u w=1u\nM2 d g 0 0 p l=1u w=1u\n.ends\n.endif\nX1 d g inner\n.ends\n\
         X1 d g host\nX2 d g host k=2\nX3 d g host\n";
    let branch_x = ".subckt host d g k=1\n.if (k==1)\nX1 d g leaf\n.endif\n.ends\nX1 d g host\nX2 d g host k=2\nX3 d g host\n";
    let top_if =
        ".param top=1\n.if (top==1)\n.param v0=2\nX1 d g leaf v=v0\n.else\nX1 d g leaf\n.endif\n";
    let nested = ".subckt host d g k=1 j=1\n.if (k==1)\n.if (j==1)\nM1 d g 0 0 n l=1u w=1u\n.else\nM1 d g 0 0 p l=1u w=1u\n.endif\n.endif\n.ends\nX1 d g host\nX2 d g host j=2\nX3 d g host k=2\nX4 d g host\n";
    let read_above = ".param a=1\n.subckt host d g\n.if (a==1)\nM1 d g 0 0 n l=1u w=1u\n.else\nM1 d g 0 0 p l=1u w=1u\n.endif\n.ends\n.subckt wrap d g a=1\nX1 d g host\n.ends\nX1 d g wrap\nX2 d g wrap a=2\nX3 d g wrap\n";
    for deck in [
        format!("{models}{leaf}{mid}{x4}"),
        format!("{models}{branch_model}"),
        format!("{models}{branch_subckt}"),
        format!("{models}{leaf}{branch_x}"),
        format!("{models}{leaf}{top_if}"),
        format!("{models}{nested}"),
        format!("{models}{read_above}"),
    ] {
        for option in ["", ".option parhier=local\n"] {
            let text = format!("{option}{deck}");
            let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
            let (ast, _) = files.build(parsed);
            let mut expected: HashMap<String, u64> = HashMap::new();
            for m in ast.mosfet_models() {
                let name = m
                    .selection
                    .as_ref()
                    .map_or_else(|e| e.to_string(), |s| s.model.name.to_string());
                *expected.entry(name).or_default() += 1;
            }
            let usage = ast.model_usage();
            let mut got: HashMap<String, u64> = HashMap::new();
            for (m, n) in &usage.models {
                *got.entry(m.name.to_string()).or_default() += n;
            }
            for (e, n, _) in &usage.errors {
                *got.entry(e.to_string()).or_default() += n;
            }
            assert_eq!(got, expected, "{text}");
        }
    }
}

/// A deep nest of blocks is one error, not a stack overflow; a nest within the
/// limit runs
#[tokio::test]
async fn if_nesting_limit() {
    let nest = |n: usize| {
        format!(
            "{IF_N}{}{IF_M1}{}",
            ".if (1)\n".repeat(n),
            ".endif\n".repeat(n)
        )
    };
    let (has_err, rows) = if_deck(&nest(200)).await;
    assert!(!has_err);
    assert_eq!(if_paths(&rows), ["M1"]);
    let (has_err, _) = if_deck(&nest(100_000)).await;
    assert!(has_err);
}

/// The text of a block survives printing the netlist
#[tokio::test]
async fn if_blocks_are_printed() {
    let text = format!("{IF_N}.if (1)\n{IF_M1}.elseif (2)\n{IF_M2}.else\n{IF_M3}.endif\n");
    let (_dir, parsed, files) = parse_deck(&[("deck.sp", &text)]).await;
    let (ast, _) = files.build(parsed);
    let printed = ast.to_string().to_lowercase();
    for word in [
        ".if (1)",
        ".elseif (2)",
        ".else",
        ".endif",
        "m1 ",
        "m2 ",
        "m3 ",
    ] {
        assert!(printed.contains(word), "no `{word}` in:\n{printed}");
    }
}

/// Short forms of the commands, and longer words that are other commands
#[tokio::test]
async fn short_forms_of_commands() {
    for (form, want) in [
        (".par k=2u\nM1 d g 0 0 n l=k w=1u\n", 2.0),
        (".parame k=2u\nM1 d g 0 0 n l=k w=1u\n", 2.0),
        (".parameters k=2u\nM1 d g 0 0 n l=k w=1u\n", 2.0),
        (".su s d g\nM1 d g 0 0 n l=2u w=1u\n.ends\nX1 d g s\n", 2.0),
        (
            ".subc s d g\nM1 d g 0 0 n l=2u w=1u\n.ends\nX1 d g s\n",
            2.0,
        ),
        (".mac s d g\nM1 d g 0 0 n l=2u w=1u\n.eom\nX1 d g s\n", 2.0),
        (".mod q nmos level=54\nM1 d g 0 0 q l=2u w=1u\n", 2.0),
        (".mode q nmos level=54\nM1 d g 0 0 q l=2u w=1u\n", 2.0),
        (".els\n", 0.0),
    ] {
        if want == 0.0 {
            continue;
        }
        let text = format!("{IF_N}{form}");
        let (_dir, parsed, files) = parse_deck(&[("deck.sp", &text)]).await;
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err, "{form}");
        let rows = ast.mosfet_models();
        assert_eq!(rows.len(), 1, "{form}");
        let s = rows[0].selection.as_ref().unwrap();
        assert!(close(s.l * 1e6, want), "{form}: L={}", s.l);
    }
    // `.opt` is `.option`
    let text = format!("{IF_N}.opti scale=1e-6\nM1 d g 0 0 n l=2 w=1\n");
    assert_eq!(if_lengths(&text).await, [("M1".to_owned(), 2.0)]);
    // `.in` and `.li` read files
    let (_dir, parsed, files) = parse_deck(&[
        (
            "deck.sp",
            "* t\n.in 'a.inc'\n.li 'b.l' s\nM1 d g 0 0 q l=k w=1u\n",
        ),
        ("a.inc", ".param k=2u\n"),
        ("b.l", ".lib s\n.model q nmos level=54\n.endl s\n"),
    ])
    .await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(chosen(&ast), pairs(&[("M1", "q")]));
    // the keywords of a block
    let text = format!("{IF_N}.if (0)\n{IF_M1}.elsei (0)\n{IF_M2}.els\n{IF_M3}.endi\n");
    assert_eq!(if_paths(&if_deck(&text).await.1), ["M3"]);
    // `.alt` ends the first run
    let text = format!("{IF_N}{IF_M1}.alt\n{IF_M2}");
    assert_eq!(if_paths(&if_deck(&text).await.1), ["M1"]);
    // longer words: unknown commands, which are ignored
    for word in ["paramx", "params", "alters", "alterx"] {
        let text = format!("{IF_N}.{word} k\n{IF_M1}");
        let (has_err, rows) = if_deck(&text).await;
        assert!(!has_err, "{word}");
        assert_eq!(if_paths(&rows), ["M1"], "{word}");
    }
    // the form of `.temp`: the value of `temper`
    for (head, temper) in [
        (".te 75\n", 75.0),
        (".temper 75\n", 75.0),
        (".temperatures 75\n", 75.0),
        (".tempx 75\n", 25.0),
        (".temperature 75\n", 75.0),
        (".temp=75\n", 75.0),
        (".temp '50+25'\n", 75.0),
        (".param t=75\n.temp t\n", 75.0),
        (".temp 50\n.temp 75\n", 50.0),
        (".temp 75 100\n", 75.0),
        (".option tnom=50\n", 50.0),
        (".option tnom=50\n.option tnom=75\n", 75.0),
        (".option tnom=50\n.temp 75\n", 75.0),
        (".option tnom='50'\n", 25.0),
        (".param tt=50\n.option tnom=tt\n", 25.0),
        (".if (1)\n.temp 75\n.endif\n", 75.0),
        (".if (0)\n.temp 75\n.endif\n", 25.0),
        (".subckt s d g\n.temp 75\n.ends\nX1 d g s\n", 25.0),
    ] {
        let text = format!("{IF_N}{head}M1 d g 0 0 n l='temper*1u' w=1u\n");
        let (_dir, parsed, files) = parse_deck(&[("deck.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let s = ast.mosfet_models().remove(0).selection.unwrap();
        assert!(close(s.l * 1e6, temper), "{head}: L={}", s.l);
    }
}

// ---------------------------------------------------------------------------
// Rules of the selector, as measured on HSPICE (`golden/NOTES.md`)

/// What the deck `text` gives for each row: path, the model (or the error), L
/// and W in um, NF. The counts of [`AST::model_usage`] must agree with the rows.
async fn deck_rows(text: &str) -> Vec<(String, String, f64, f64, f64)> {
    let (_dir, parsed, files) = parse_deck(&[("deck.sp", text)]).await;
    let (ast, _) = files.build(parsed);
    usage_agrees(&ast, text);
    let um = |x: f64| (x * 1e12).round() / 1e6;
    ast.mosfet_models()
        .into_iter()
        .map(|m| match &m.selection {
            Ok(s) => (m.path, s.model.name.to_lowercase(), um(s.l), um(s.w), s.nf),
            Err(e) => (m.path, format!("error: {e}"), 0.0, 0.0, 0.0),
        })
        .collect()
}

/// The counts of [`AST::model_usage`] against the rows of the list
fn usage_agrees(ast: &AST<'_>, text: &str) {
    let mut expected: HashMap<String, u64> = HashMap::new();
    let rows = ast.mosfet_models();
    for m in &rows {
        let name = m
            .selection
            .as_ref()
            .map_or_else(|e| e.to_string(), |s| s.model.name.to_string());
        *expected.entry(name).or_default() += 1;
    }
    let usage = ast.model_usage();
    let mut got: HashMap<String, u64> = HashMap::new();
    for (m, n) in &usage.models {
        *got.entry(m.name.to_string()).or_default() += n;
    }
    for (e, n, _) in &usage.errors {
        *got.entry(e.to_string()).or_default() += n;
    }
    assert_eq!(got, expected, "{text}");
}

/// The L and W of the one row of a deck
async fn one_size(text: &str) -> (f64, f64) {
    let rows = deck_rows(text).await;
    assert_eq!(rows.len(), 1, "{text}: {rows:?}");
    assert!(!rows[0].1.starts_with("error"), "{text}: {rows:?}");
    (rows[0].2, rows[0].3)
}

/// The model of the one row of a deck
async fn one_model(text: &str) -> String {
    let rows = deck_rows(text).await;
    assert_eq!(rows.len(), 1, "{text}: {rows:?}");
    rows[0].1.clone()
}

/// `scale`, `scalm`, `defl` and `defw` take plain numbers only. The last plain
/// number wins, also after the MOSFET line. Other values are ignored (decks
/// `opt_scale_*`, `opd_*`, `os_*`).
#[tokio::test]
async fn option_sizes_are_plain_numbers() {
    let line = "M1 d g 0 0 n w=1u\n";
    for (head, l) in [
        (".option defl=7u\n", 7.0),
        (".option DEFL=7u\n", 7.0),
        (".option defl='7u'\n", 100.0),
        (".param q=7u\n.option defl=q\n", 100.0),
        (".option defl='3u+4u'\n", 100.0),
        (".option defl=7u\n.option defl='9u'\n", 7.0),
        (".param q=9u\n.option defl=7u\n.option defl=q\n", 7.0),
        (".option defl='9u'\n.option defl=7u\n", 7.0),
        (".option defl=9u\n.option defl=7u\n", 7.0),
    ] {
        let text = format!("{IF_N}{head}{line}");
        assert_eq!(one_size(&text).await, (l, 1.0), "{head}");
    }
    let line = "M1 d g 0 0 n l=2 w=3\n";
    for (head, l, w) in [
        (".option scale=1u\n", 2.0, 3.0),
        (".option scale='1u'\n", 2e6, 3e6),
        (".param q=1u\n.option scale=q\n", 2e6, 3e6),
        (".option scale=1u\n.option scale='4u'\n", 2.0, 3.0),
        (".param q=4u\n.option scale=1u\n.option scale=q\n", 2.0, 3.0),
        (".subckt z d g\n.option scale=4u\n.ends\n", 2e6, 3e6),
    ] {
        let text = format!("{IF_N}{head}{line}");
        assert_eq!(one_size(&text).await, (l, w), "{head}");
    }
    // after the line
    assert_eq!(
        one_size(&format!("{IF_N}{line}.option scale=1u\n")).await,
        (2.0, 3.0)
    );
    // `scalm` chooses the bin, `b.2` with these limits times 1000
    let bins = ".model b.1 nmos level=54 lmin=1e-9 lmax=1e-6 wmin=1e-9 wmax=1e-3\n\
                .model b.2 nmos level=54 lmin=1e-6 lmax=1e-3 wmin=1e-9 wmax=1\n";
    for (head, want) in [
        ("", "b.2"),
        (".option scalm=1e3\n", "b.1"),
        (".option scalm='1e3'\n", "b.2"),
        (".param q=1e3\n.option scalm=q\n", "b.2"),
    ] {
        let text = format!("* t\n{head}{bins}M1 d g 0 0 b l=2u w=1u\n");
        assert_eq!(one_model(&text).await, want, "{head}");
    }
}

/// `.option wnflag` takes a number, also in quotes. An expression or a name is
/// ignored. On the line, every value counts (decks `opw_*`).
#[tokio::test]
async fn wnflag_values() {
    let bins = ".model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-6\n\
                .model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-6 wmax=1e-3\n";
    let line = "M1 d g 0 0 a w=4u l=1u nf=8";
    for (head, tail, want) in [
        ("", "", "a.1"),
        (".option wnflag=0\n", "", "a.2"),
        (".option wnflag=1\n", "", "a.1"),
        (".option wnflag\n", "", "a.1"),
        (".option wnflag='1'\n", "", "a.1"),
        (".option wnflag='0'\n", "", "a.2"),
        (".option wnflag='1-1'\n", "", "a.1"),
        (".param wf=0\n.option wnflag=wf\n", "", "a.1"),
        ("", " wnflag=0", "a.2"),
        ("", " wnflag='0'", "a.2"),
        (".param wf=0\n", " wnflag=wf", "a.2"),
        (".option wnflag=0\n", " wnflag=1", "a.1"),
    ] {
        let text = format!("* t\n{head}{bins}{line}{tail}\n");
        assert_eq!(one_model(&text).await, want, "{head}{tail}");
    }
}

/// `.option wl` swaps the two values after the model name. It counts anywhere
/// in the deck: after the line, in a subckt that is not used, in a branch that
/// is not active, in an include. Only `wl` and `wl=1` set it, and the last
/// one wins (decks `opt_wl_*`, `wv_*`).
#[tokio::test]
async fn option_wl_swaps_the_values() {
    let line = "M1 d g 0 0 n 1u 2u\n";
    for (head, swap) in [
        ("", false),
        (".option wl\n", true),
        (".OPTION WL\n", true),
        (".option wl=1\n", true),
        (".option wl=1.0\n", true),
        (".option wl=0\n", false),
        (".option wl=2\n", false),
        (".option wl=0.5\n", false),
        (".option wl='1'\n", false),
        (".param q=1\n.option wl=q\n", false),
        (".option nowl\n", false),
        (".option wl\n.option wl=0\n", false),
        (".option wl=0\n.option wl\n", true),
        (".option wl\n.option wl='1'\n", false),
        (".option wl='1'\n.option wl\n", true),
        (".param q=1\n.option wl\n.option wl=q\n", false),
        (".subckt z d g\n.option wl\n.ends\n", true),
        (".if (0)\n.option wl\n.endif\n", true),
        (".if (1)\n.option wl\n.endif\n", true),
    ] {
        let text = format!("{IF_N}{head}{line}");
        let want = if swap { (2.0, 1.0) } else { (1.0, 2.0) };
        assert_eq!(one_size(&text).await, want, "{head}");
    }
    let wl = ".option wl\n";
    assert_eq!(one_size(&format!("{IF_N}{line}{wl}")).await, (2.0, 1.0));
    // a value given by name wins over its place; the other one is not given
    assert_eq!(
        one_size(&format!("{IF_N}{wl}M1 d g 0 0 n 1u w=2u\n")).await,
        (100.0, 2.0)
    );
    assert_eq!(
        one_size(&format!("{IF_N}{wl}M1 d g 0 0 n 3u\n")).await,
        (100.0, 3.0)
    );
    assert_eq!(
        one_size(&format!("{IF_N}{wl}M1 d g 0 0 n 1u 2u 3u\n")).await,
        (2.0, 1.0)
    );
    // in a subckt
    let text = format!("{IF_N}{wl}.subckt s d g\n{line}.ends\nX1 d g s\n");
    assert_eq!(one_size(&text).await, (2.0, 1.0));
    // in an include
    let (_dir, parsed, files) = parse_deck(&[
        ("deck.sp", &format!("{IF_N}.inc 'wl.inc'\n{line}")),
        ("wl.inc", wl),
    ])
    .await;
    let (ast, _) = files.build(parsed);
    let s = ast.mosfet_models().remove(0).selection.unwrap();
    assert!(close(s.l, 2e-6), "{}", s.l);
    // a bin is chosen with the swapped values
    let text = format!(
        "* t\n{wl}.model b.1 nmos level=54 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n\
         .model b.2 nmos level=54 lmin=1u lmax=1e-3 wmin=1e-8 wmax=1e-3\nM1 d g 0 0 b 1u 500n\n"
    );
    assert_eq!(one_model(&text).await, "b.1");
}

/// NF below 1 counts as 1 in the result, and the bin still divides W by it. NF=0
/// leaves W whole (decks `m_nf_zero`, `mx_nf_*`).
#[tokio::test]
async fn nf_below_one() {
    let bins = |limit: &str| {
        format!(
            "* t\n.model a.1 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax={limit}\n\
             .model a.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin={limit} wmax=1e-3\n"
        )
    };
    for (head, line, model, nf) in [
        (bins("3e-6"), "M1 d g 0 0 a w=2u l=1u nf=0.5\n", "a.2", 1.0),
        (bins("1e-6"), "M1 d g 0 0 a w=4u l=1u nf=0\n", "a.2", 1.0),
        (bins("1e-6"), "M1 d g 0 0 a w=4u l=1u nf=8\n", "a.1", 8.0),
        (
            bins("1e-6"),
            "M1 d g 0 0 a w=2.5u l=1u nf=2.5\n",
            "a.2",
            2.5,
        ),
    ] {
        let rows = deck_rows(&format!("{head}{line}")).await;
        assert_eq!(rows.len(), 1, "{line}");
        assert_eq!((rows[0].1.as_str(), rows[0].4), (model, nf), "{line}");
    }
}

/// HSPICE stops at a size that is not positive; W=0 runs (decks `m_l_neg`,
/// `m_w_neg`, `m_nf_neg`, `m_l_zero`, `m_m_zero`, `m_w_zero`)
#[tokio::test]
async fn sizes_that_are_not_positive() {
    for (tail, stops) in [
        ("l=-1u w=1u", true),
        ("l=1u w=-1u", true),
        ("l=1u w=1u nf=-2", true),
        ("l=0 w=1u", true),
        ("l=1u w=1u m=0", true),
        ("l=1u w=1u m=-1", true),
        ("l=1u w=0", false),
        ("l=1u w=1u m=2", false),
        ("l=1u w=1u M=2", false),
        ("l=1u w=1u m='1+1'", false),
    ] {
        let rows = deck_rows(&format!("{IF_N}M1 d g 0 0 n {tail}\n")).await;
        assert_eq!(rows.len(), 1, "{tail}");
        assert_eq!(rows[0].1.starts_with("error"), stops, "{tail}: {rows:?}");
    }
}

/// With a limit given twice, the first one counts (deck `mdl_dup_keys`)
#[tokio::test]
async fn first_limit_of_two_counts() {
    let text = "* t\n.model b.1 nmos level=54 lmin=1e-8 lmax=1e-6 lmax=1e-3 wmin=1e-8 wmax=1e-3\n\
                .model b.2 nmos level=54 lmin=1e-3 lmax=1 wmin=1e-8 wmax=1e-3\n\
                M1 d g 0 0 b l=2u w=1u\n";
    assert!(one_model(text).await.starts_with("error"), "{text}");
    let text = text.replace("lmax=1e-6 lmax=1e-3", "lmax=1e-3 lmax=1e-6");
    assert_eq!(one_model(&text).await, "b.1");
}

/// Only NMOS and PMOS models serve an M line, and other types of the name are
/// not seen (decks `mt_*`, `mdl_diode_on_m`, `mdl_r_on_m`)
#[tokio::test]
async fn only_mos_models_serve_an_m_line() {
    let line = "M1 d g 0 0 q l=1u w=1u\n";
    for kind in ["nmos", "NMOS", "pmos", "PMos"] {
        assert_eq!(
            one_model(&format!("* t\n.model q {kind} level=54\n{line}")).await,
            "q",
            "{kind}"
        );
    }
    for kind in [
        "d", "r", "c", "nch", "pch", "n", "p", "nfet", "pfet", "nmosfet", "npn",
    ] {
        let text = format!("* t\n.model q {kind}\n{line}");
        assert!(one_model(&text).await.starts_with("error"), "{kind}");
    }
    // the bins of a name of another type
    let text = format!("* t\n.model q.1 nch level=54 lmin=1e-8 lmax=1 wmin=1e-8 wmax=1\n{line}");
    assert!(one_model(&text).await.starts_with("error"), "{text}");
    // the model at the top level serves past a diode model of the name
    let text =
        format!("* t\n.model q nmos level=54\n.subckt s d g\n.model q d\n{line}.ends\nX1 d g s\n");
    assert_eq!(one_model(&text).await, "q");
    assert_eq!(deck_rows(&text).await[0].1, "q");
}

/// Level 72 bins hold a fin count, not a width: `nfinmin <= nfin < nfinmax`.
/// `wsheetmin <= wsheet < wsheetmax` is the other form (decks `f_*`, `k_*`,
/// `w_*`, `p_*`).
#[tokio::test]
async fn finfet_bins() {
    let bins = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\n\
                .model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=4 nfinmax=9\n";
    for (head, tail, want) in [
        ("", "l=500n nfin=1", "f.1"),
        ("", "l=500n nfin=2", "f.1"),
        ("", "l=500n nfin=3.5", "f.1"),
        ("", "l=500n nfin=4", "f.2"),
        ("", "l=500n nfin=8.5", "f.2"),
        ("", "l=500n NFIN=8.5", "f.2"),
        ("", "l=500n", "f.1"),
        ("", "l=500n w=1u", "f.1"),
        ("", "l=500n w=1u nfin=6", "f.2"),
        ("", "l=500n nfin=6 nf=2", "f.2"),
        ("", "l=500n nfin=3 nf=2", "f.1"),
        ("", "l=500n nfin=3 m=2", "f.1"),
        (".param k=6\n", "l=500n nfin=k", "f.2"),
        (".param k=3\n", "l=500n nfin='k*2'", "f.2"),
        ("", "500n 1u nfin=6", "f.2"),
        (".option scale=1e-6\n", "l=0.5 nfin=6", "f.2"),
        (".option scale=2\n", "l=250n nfin=3", "f.1"),
    ] {
        let text = format!("* t\n{head}{bins}M1 d g 0 0 f {tail}\n");
        assert_eq!(one_model(&text).await, want, "{head}{tail}");
    }
    for (head, tail) in [
        ("", "l=500n nfin=0.5"),
        ("", "l=500n nfin=9"),
        ("", "l=500n nfin=10"),
        ("", "l=1u nfin=3"),
        ("", "l=2u nfin=3"),
        (".option scalm=1e-1\n", "l=500n nfin=6"),
    ] {
        let text = format!("* t\n{head}{bins}M1 d g 0 0 f {tail}\n");
        assert!(one_model(&text).await.starts_with("error"), "{head}{tail}");
    }
    // the keys of a bin
    for (model, line, want) in [
        (
            ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3 nfinmin=1 nfinmax=4\n",
            "l=500n w=1u nfin=2",
            "f.1",
        ),
        (
            ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n",
            "l=500n w=1u",
            "error",
        ),
        (
            ".model f.1 nmos level=72 lmax=1u nfinmin=1 nfinmax=4\n",
            "l=500n nfin=2",
            "error",
        ),
        (
            ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1\n",
            "l=500n nfin=2",
            "error",
        ),
        (
            ".model f.1 nmos level=54 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\n",
            "l=500n w=1u nfin=2",
            "error",
        ),
        (
            ".model f.1 nmos level=70 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n",
            "l=500n w=1u",
            "f.1",
        ),
        (".model f nmos level=72\n", "l=500n nfin=6", "f"),
        (".model f nmos level=72\n", "l=500n", "f"),
    ] {
        let got = one_model(&format!("* t\n{model}M1 d g 0 0 f {line}\n")).await;
        assert!(got.starts_with(want), "{model}{line}: {got}");
    }
    // the sheet form
    let sheets = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-9 wsheetmax=1e-8\n\
                  .model f.2 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6\n";
    let text = format!("* t\n{sheets}M1 d g 0 0 f l=500n wsheet=2e-8\n");
    assert_eq!(one_model(&text).await, "f.2");
    let text = format!("* t\n{sheets}M1 d g 0 0 f l=500n w=2e-8\n");
    assert!(one_model(&text).await.starts_with("error"), "{text}");
    // a bin of another level, in the same set
    let text = "* t\n.model f.1 nmos level=54 lmin=1e-8 lmax=1u wmin=1e-8 wmax=1e-3\n\
                .model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\n\
                M1 d g 0 0 f l=500n w=1u nfin=2\n";
    assert_eq!(one_model(text).await, "f.1");
    // a parameter read by the fin count changes the bin in the counts too
    let text = format!(
        "* t\n{bins}.subckt s d g k=2\nM1 d g 0 0 f l=500n nfin=k\n.ends\nX1 d g s\nX2 d g s k=5\nX3 d g s\n"
    );
    let rows = deck_rows(&text).await;
    let models: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
    assert_eq!(models, ["f.1", "f.2", "f.1"]);
}

/// `m` on an X line is not passed to the subckt (decks `x_m_*`)
#[tokio::test]
async fn m_on_an_x_line_is_not_passed() {
    for (text, l) in [
        (
            ".subckt s d g m=3\nM1 d g 0 0 n l='m*1u' w=1u\n.ends\nX1 d g s m=2\n",
            3.0,
        ),
        (
            ".param m=5\n.subckt s d g\nM1 d g 0 0 n l='m*1u' w=1u\n.ends\nX1 d g s m=2\n",
            5.0,
        ),
        (
            ".option parhier=local\n.param m=5\n.subckt s d g\nM1 d g 0 0 n l='m*1u' w=1u\n.ends\nX1 d g s m=2\n",
            5.0,
        ),
        (
            ".subckt s d g m=3\nM1 d g 0 0 n l='m*1u' w=1u\n.ends\nX1 d g s M=2\n",
            3.0,
        ),
        (
            ".subckt s d g k=3\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s k=2 m=2\n",
            2.0,
        ),
    ] {
        let text = format!("{IF_N}{text}");
        assert_eq!(one_size(&text).await, (l, 1.0), "{text}");
    }
    let text = format!("{IF_N}.subckt s d g\nM1 d g 0 0 n l='m*1u' w=1u\n.ends\nX1 d g s m=2\n");
    assert!(one_model(&text).await.starts_with("error"), "{text}");
}

/// A condition stops at `temper` and `time`, also through a parameter (decks
/// `tc_*`). In a value, `time` reads 0 (deck `p_time_in_expr`).
#[tokio::test]
async fn temper_and_time() {
    for head in [
        ".temp 75\n.param t='temper'\n.if (t>50)\n",
        ".temp 75\n.param t='temper*1'\n.if (t)\n",
        ".if (temper>50)\n",
        ".if (time>0)\n",
        ".param t='time'\n.if (t==0)\n",
    ] {
        let text = format!("{IF_N}{head}{IF_M1}.else\n{IF_M2}.endif\n");
        let (_, rows) = if_deck(&text).await;
        assert_eq!(rows.len(), 1, "{head}: {rows:?}");
        assert!(rows[0].1.starts_with("error"), "{head}: {rows:?}");
    }
    // the same parameters read in a value are not a problem
    let text = format!("{IF_N}.temp 75\n.param t='temper'\nM1 d g 0 0 n l='t*1u' w=1u\n");
    assert_eq!(one_size(&text).await, (75.0, 1.0));
    let text = format!("{IF_N}M1 d g 0 0 n l='time+1u' w=1u\n");
    assert_eq!(one_size(&text).await, (1.0, 1.0));
    let text = format!("{IF_N}.param t='time+1u'\nM1 d g 0 0 n l=t w=1u\n");
    assert_eq!(one_size(&text).await, (1.0, 1.0));
}

/// `log(0)` is the logarithm of 1e-28 (decks `g_log0`, `g_log100`)
#[tokio::test]
async fn logarithm_of_zero() {
    let text = format!("{IF_N}.param c='log(0)'\nM1 d g 0 0 n l='(c+1000)*1e-9' w=1u\n");
    let (l, _) = one_size(&text).await;
    assert!((l - 0.9355276).abs() < 1e-6, "{l}");
    let text = format!("{IF_N}.param c='log10(0)'\nM1 d g 0 0 n l='(c+1000)*1e-9' w=1u\n");
    let (l, _) = one_size(&text).await;
    assert!((l - 0.972).abs() < 1e-6, "{l}");
}

/// An X line that HSPICE rejects gives an error row, and no rows below it:
/// node counts (decks `x_too_few_nodes`, `x_too_many_nodes`, `subckt_param_kw`),
/// recursion (`x_self`, `x_mutual`), and a depth over the limit (`dp_loc_*`).
#[tokio::test]
async fn x_lines_that_cannot_be_expanded() {
    let body = "M1 d g 0 0 n l=1u w=1u\n";
    let s = format!(".subckt s d g\n{body}.ends\n");
    for line in ["X1 d s\n", "X1 d g h s\n", "X1 s\n"] {
        let rows = deck_rows(&format!("{IF_N}{s}{line}")).await;
        assert_eq!(rows.len(), 1, "{line}: {rows:?}");
        assert_eq!(rows[0].0, "X1", "{line}");
        assert!(rows[0].1.starts_with("error"), "{line}: {rows:?}");
    }
    // the keyword `param:` is a node there
    let text =
        format!("{IF_N}.subckt s d g param: w=1u\nM1 d g 0 0 n l=1u w=w\n.ends\nX1 d g s w=3u\n");
    assert!(deck_rows(&text).await[0].1.starts_with("error"), "{text}");
    // the right count
    assert_eq!(deck_rows(&format!("{IF_N}{s}X1 d g s\n")).await[0].1, "n");
    // recursion
    for text in [
        format!(".subckt a d g\n{body}X1 d g a\n.ends\nX1 d g a\n"),
        format!(".subckt a d g\nX1 d g b\n.ends\n.subckt b d g\n{body}X1 d g a\n.ends\nX1 d g a\n"),
    ] {
        let text = format!("{IF_N}{text}");
        let rows = deck_rows(&text).await;
        assert!(
            rows.iter().any(|r| r.1.starts_with("error")),
            "{text}: {rows:?}"
        );
    }
    // too deep for the walk
    let mut deep = format!("{IF_N}.subckt s0 d g\n{body}.ends\n");
    for k in 1..300 {
        deep += &format!(".subckt s{k} d g\nX1 d g s{}\n.ends\n", k - 1);
    }
    deep += "X1 d g s299\n";
    let rows = deck_rows(&deep).await;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].1.starts_with("error"), "{}", rows[0].1);
    // such errors do not count as MOSFETs
    let (_dir, parsed, files) = parse_deck(&[("deck.sp", &deep)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.model_usage().mosfets, 0);
    let text = format!("{IF_N}{s}X1 d s\nX2 d g s\n");
    let (_dir, parsed, files) = parse_deck(&[("deck.sp", &text)]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.model_usage().mosfets, 1);
}

// ---------------------------------------------------------------------------
// Forms of a line, as measured on HSPICE (`golden/NOTES.md`)

/// `(path, L, W)` of each row of a deck with no error. The errors of the parse
/// fail the test.
async fn clean_sizes(files: &[(&str, &str)]) -> Vec<(String, f64, f64)> {
    let (_dir, parsed, built) = parse_deck(files).await;
    let (ast, has_err) = built.build(parsed);
    assert!(!has_err, "{files:?}");
    let um = |x: f64| (x * 1e12).round() / 1e6;
    ast.mosfet_models()
        .into_iter()
        .map(|m| match &m.selection {
            Ok(s) => (m.path.clone(), um(s.l), um(s.w)),
            Err(e) => panic!("{}: {e}: {files:?}", m.path),
        })
        .collect()
}

/// HSPICE reads a chain of parameters of any length, in any order, at any level
/// (decks `c_*`, `r_*`, `s_*`, `sr_*`, `f_*`, `i_*`, `params50k`).
#[tokio::test]
async fn long_chains_of_parameters() {
    let chain = |n: usize, reverse: bool| {
        let mut rows = vec![".param p0=1n\n".to_owned()];
        rows.extend((1..=n).map(|k| format!(".param p{k}='p{}+1n'\n", k - 1)));
        if reverse {
            rows.reverse();
        }
        rows.concat()
    };
    for n in [99, 100, 101, 150, 1000, 5000] {
        let size = (n + 1) as f64 * 1e-3;
        let close = |(l, w): (f64, f64)| (l - size).abs() < 1e-9 && w == 1.0;
        let line = format!("M1 d g 0 0 n l=p{n} w=1u\n");
        for reverse in [false, true] {
            let rows = chain(n, reverse);
            let text = format!("{IF_N}{rows}{line}");
            assert!(close(one_size(&text).await), "top, {n}, {reverse}");
            let text = format!("{IF_N}.subckt c d g\n{rows}{line}.ends\nX1 d g c\n");
            assert!(close(one_size(&text).await), "subckt, {n}, {reverse}");
            let text = format!("{IF_N}{rows}.subckt c d g\n{line}.ends\nX1 d g c\n");
            assert!(close(one_size(&text).await), "read below, {n}, {reverse}");
            let text = format!("{IF_N}{rows}.if (p{n}>1n)\n{line}.endif\n");
            assert!(close(one_size(&text).await), ".if, {n}, {reverse}");
        }
    }
    // a loop stays an error, whatever its length
    for n in [2, 150, 1000] {
        let rows = chain(n, false).replace(".param p0=1n", &format!(".param p0='p{n}+1n'"));
        let rows = deck_rows(&format!("{IF_N}{rows}M1 d g 0 0 n l=p{n} w=1u\n")).await;
        assert!(rows[0].1.starts_with("error"), "{n}: {rows:?}");
    }
    // `temper` stops a condition also at the far end of a chain
    for n in [99, 100, 101, 200] {
        let rows = chain(n, false).replace(".param p0=1n\n", ".param p0='temper'\n");
        let text = format!("{IF_N}{rows}.if (p{n}>1n)\nM1 d g 0 0 n l=1u w=1u\n.endif\n");
        let rows = deck_rows(&text).await;
        assert!(
            rows.iter().any(|r| r.1.starts_with("error")),
            "{n}: {rows:?}"
        );
    }
    // a loop or a name with no value at the far end stops all the names before it
    for far in [".param p0='q' q='r' r='q'\n", ".param p0='nothing'\n"] {
        let rows = chain(500, false).replace(".param p0=1n\n", far);
        let rows = deck_rows(&format!("{IF_N}{rows}M1 d g 0 0 n l=p500 w=1u\n")).await;
        assert!(rows[0].1.starts_with("error"), "{far}: {rows:?}");
    }
}

/// `=` can start the `+` line after the name of a `.param` (decks `p_plus_eq*`)
#[tokio::test]
async fn equals_sign_on_a_plus_line_of_param() {
    for cards in [
        ".param q\n+ =1u\n",
        ".param q\n+=1u\n",
        ".param q\n+ =1u r\n+ =2u\n",
    ] {
        let text = format!("{IF_N}{cards}M1 d g 0 0 n l=q w=1u\n");
        assert_eq!(one_size(&text).await, (1.0, 1.0), "{cards}");
    }
}

/// `(` and `)` only separate words on a MOSFET line, also after `key=`. So a space,
/// a comma or a `)` ends the value there, and `(2u+3u)` is one word (decks `g*`, `w*`).
#[tokio::test]
async fn parentheses_after_a_key_on_a_mosfet_line() {
    let cards = format!("{IF_N}.param k=4u j=5u\n");
    for (tail, l, w) in [
        ("l=1u w=(2u + 3u)", 1.0, 2.0),
        ("l=1u w=(2u +3u)", 1.0, 2.0),
        ("l=1u w=( 2u + 3u )", 1.0, 2.0),
        ("l=1u w=(2u 3u)", 1.0, 2.0),
        ("l=1u w=(2u,3u)", 1.0, 2.0),
        ("l=1u w=(k + j)", 1.0, 4.0),
        ("l=1u w=(k j)", 1.0, 4.0),
        ("l=1u w=(k -1u)", 1.0, 4.0),
        ("l=1u w=((k + j))", 1.0, 4.0),
        ("l=1u w=((k)+(j))", 1.0, 4.0),
        ("l=1u w=(k)+(j)", 1.0, 4.0),
        ("l=1u w=(2u)+(3u)", 1.0, 2.0),
        ("l=1u w=(k * 1)", 1.0, 4.0),
        ("l=1u w=(k *1)", 1.0, 4.0),
        ("l=(2u * 3) w=1u", 2.0, 100.0),
        ("l=(k * 3) w=1u", 4.0, 100.0),
        ("l=(2u ** 3) w=1u", 2.0, 100.0),
        ("l=(2u + 3n) w=1u", 2.0, 1.0),
        ("l=(k + 3n) w=1u", 4.0, 1.0),
        ("l=(2u + 3u) (4u)", 2.0, 100.0),
        ("l=(2u + 3u) w=9u", 2.0, 9.0),
        ("l=(2u + 3u) l=9u", 9.0, 100.0),
        ("w=9u l=(2u + 3u) (4u)", 2.0, 9.0),
        ("l=(k)+1u", 4.0, 1.0),
        ("l=(k)+1u w=9u", 4.0, 9.0),
        ("l=(k)-1u w=9u", 4.0, 9.0),
        ("l=(k+j)+(j)", 9.0, 100.0),
        ("l=(k+j)+1u", 9.0, 1.0),
        ("l=((2u)+1u) w=1u", 2.0, 1.0),
        // one word
        ("l=1u w=( 2u+3u )", 1.0, 5.0),
        ("l=1u w=( 2u+3u)", 1.0, 5.0),
        ("l=1u w=(2u+3u )", 1.0, 5.0),
        ("l=1u w=(k+j)", 1.0, 9.0),
        ("l=1u w=( k+j )", 1.0, 9.0),
        ("l=1u w=((k+j))", 1.0, 9.0),
        ("l=1u w=(k*1)", 1.0, 4.0),
        ("l=1u w=( k )", 1.0, 4.0),
        ("l=1u w=( 2u )", 1.0, 2.0),
        // quotes keep the whole text
        ("l='(2u + 3u)' w=1u", 5.0, 1.0),
        ("l=\"(2u + 3u)\" w=1u", 5.0, 1.0),
        // over `+` lines
        ("l=(1u\n+ +2u)", 1.0, 2.0),
        ("l=(1u\n+ 2u)", 1.0, 2.0),
        ("l=(1u\n+ 2u) w=3u", 1.0, 3.0),
        ("w=(1u\n+ 2u) l=3u", 3.0, 2.0),
        ("l=(k\n+ +j)", 4.0, 5.0),
        ("l=(k\n+ j)", 4.0, 5.0),
        ("l=(\n+ 1u)", 1.0, 100.0),
        ("l=(\n+ 1u+2u)", 3.0, 100.0),
        ("l=(1u+2u\n+ )", 3.0, 100.0),
        ("l=(1u+2u\n+ ) w=3u", 3.0, 3.0),
    ] {
        let text = format!("{cards}M1 d g 0 0 n {tail}\n");
        assert_eq!(one_size(&text).await, (l, w), "{tail}");
    }
    // a word that ends in an operator has no value, and neither W below 0
    for tail in [
        "l=1u w=(2u+ 3u)",
        "l=1u w=(k* 1)",
        "l=(k+\n+ j)",
        "l=(k)-1u",
    ] {
        let rows = deck_rows(&format!("{cards}M1 d g 0 0 n {tail}\n")).await;
        assert!(rows[0].1.starts_with("error"), "{tail}: {rows:?}");
    }
}

/// A `+` with no word right after it takes a place and gives no value. In `+2u` it
/// is the sign of the word (decks `w*`).
#[tokio::test]
async fn a_lone_plus_on_a_mosfet_line() {
    let cards = format!("{IF_N}.param k=4u\n");
    for (tail, l, w) in [
        ("1u + 2u", 1.0, 100.0),
        ("1u +(2u)", 1.0, 100.0),
        ("1u + (2u)", 1.0, 100.0),
        ("1u + k", 1.0, 100.0),
        ("1u + 2u 3u", 1.0, 100.0),
        ("1u + w=3u", 1.0, 3.0),
        ("1u + l=3u", 3.0, 100.0),
        ("+ 1u", 100.0, 1.0),
        ("+ 1u 2u", 100.0, 1.0),
        ("+ l=1u w=2u", 1.0, 2.0),
        ("l=1u + 2u", 1.0, 100.0),
        ("l=1u + k", 1.0, 100.0),
        ("w=2u + 3u", 100.0, 2.0),
        ("w=2u + 3u l=1u", 1.0, 2.0),
        ("w=2u + 3u + 4u", 100.0, 2.0),
        ("w=2u + l=3u", 3.0, 2.0),
        ("l=1u w=2u + 3u", 1.0, 2.0),
        ("1u 2u +", 1.0, 2.0),
        ("1u +\n+ 2u", 1.0, 100.0),
        // the sign of a word
        ("1u +2u", 1.0, 2.0),
        ("1u +k", 1.0, 4.0),
        ("+1u", 1.0, 100.0),
        ("l=1u +2u", 1.0, 2.0),
        ("l=1u +k", 1.0, 4.0),
        ("w=2u +3u", 100.0, 3.0),
    ] {
        let text = format!("{cards}M1 d g 0 0 n {tail}\n");
        assert_eq!(one_size(&text).await, (l, w), "{tail}");
    }
}

/// HSPICE assigns the plain numbers of a MOSFET line first and then the
/// expressions, each in the order of the line, and the last one stays. A name, a
/// value in quotes and a word with an operator are expressions. `+2u`, `(2u)` and
/// `2umos` are plain numbers (decks `kx_*`, `ky_*`, `ma_*`, `mb_*`, `mc_*`).
#[tokio::test]
async fn numbers_then_expressions_on_a_mosfet_line() {
    let cards = format!("{IF_N}.model a nmos level=54\n.param k=2u j=3u b=4u\n");
    for (tail, l, w) in [
        ("n 1u 2u w=5u", 1.0, 5.0),
        ("n 1u k w=5u", 1.0, 2.0),
        ("n 1u k*1 w=5u", 1.0, 2.0),
        ("n 1u '2u' w=5u", 1.0, 2.0),
        ("n 1u '2u*1' w=5u", 1.0, 2.0),
        ("n k l=5u", 2.0, 100.0),
        ("n 1u l=5u", 5.0, 100.0),
        ("n '1u' l=5u", 1.0, 100.0),
        ("n k 2u l=5u", 2.0, 2.0),
        ("n l=5u k w=6u", 5.0, 2.0),
        ("n l=5u 1u w=6u", 5.0, 6.0),
        ("n w=5u 1u", 100.0, 1.0),
        ("n w=5u k w=6u", 100.0, 2.0),
        ("n w=5u 1u w=6u", 100.0, 6.0),
        ("n w='5u' 1u 2u", 100.0, 5.0),
        ("n w=j 1u k", 100.0, 3.0),
        ("n w=j k", 100.0, 2.0),
        ("n w=j k 1u", 100.0, 2.0),
        ("n 1u k w=j", 1.0, 3.0),
        ("n 1u 2u w='5u'", 1.0, 5.0),
        ("n k l=j", 3.0, 100.0),
        ("n w=k w=5u", 100.0, 2.0),
        ("n w='2u' w=5u", 100.0, 2.0),
        ("n w=5u w=k", 100.0, 2.0),
        ("n w=j w=k", 100.0, 2.0),
        ("n w=k w=j", 100.0, 3.0),
        ("n l=1u w=(2u) w=5u", 1.0, 5.0),
        ("n l=1u w=(k) w=5u", 1.0, 2.0),
        ("n l=1u w=+2u w=5u", 1.0, 5.0),
        ("n l=1u w=2umos w=5u", 1.0, 5.0),
        ("n l=1u w=+k w=5u", 1.0, 2.0),
        ("n 1u (k) w=5u", 1.0, 2.0),
        ("n 1u (2u) w=5u", 1.0, 5.0),
        ("n 1u +2u w=5u", 1.0, 5.0),
        ("n 1u +k w=5u", 1.0, 2.0),
        ("n 1u 2umos w=5u", 1.0, 5.0),
        ("n 1u 2n7002 w=5u", 1.0, 5.0),
        ("n 1u w=5u 2u", 1.0, 5.0),
        ("n nf=2 k w=5u", 100.0, 2.0),
        ("n nf=2 1u k", 100.0, 1.0),
        ("n k j 'k' w=7u l=7u", 2.0, 3.0),
        // the words between the model and the first key
        ("a 1u b l=5u", 5.0, 4.0),
        ("a k b l=5u", 2.0, 4.0),
        ("a k w=5u 3u", 2.0, 5.0),
        ("a k l=5u 3u", 2.0, 100.0),
        ("a 1u j w=5u", 1.0, 3.0),
        ("a 1u 2u j w=5u", 1.0, 5.0),
        ("a b j w=5u l=5u", 4.0, 3.0),
        ("a b 2u l=5u w=6u", 4.0, 6.0),
        ("a k w=j 2u", 2.0, 3.0),
        ("a l=5u k w=6u", 5.0, 2.0),
    ] {
        let text = format!("{cards}M1 d g 0 0 {tail}\n");
        assert_eq!(
            clean_sizes(&[("deck.sp", &text)]).await,
            [("M1".to_owned(), l, w)],
            "{tail}"
        );
    }
    // `.option wl`: the first place is W
    for (tail, l, w) in [
        ("k 1u w=5u", 1.0, 2.0),
        ("1u k l=5u", 2.0, 1.0),
        ("1u 2u l=5u w=6u", 5.0, 6.0),
        ("w=5u 1u", 1.0, 5.0),
        ("l=5u 1u", 1.0, 100.0),
        ("l=5u k", 2.0, 100.0),
        ("w=5u k", 2.0, 5.0),
    ] {
        let text = format!("{cards}.option wl\nM1 d g 0 0 n {tail}\n");
        assert_eq!(
            clean_sizes(&[("deck.sp", &text)]).await,
            [("M1".to_owned(), l, w)],
            "wl {tail}"
        );
    }
}

/// A word that starts with a number is an expression only with an operator right
/// after the number. HSPICE reads any other such word as the number (decks `gv_*`,
/// `gx_*`, `hx_*`).
#[tokio::test]
async fn words_that_start_with_a_number() {
    let cards = format!("{IF_N}.param k=2u\n");
    for (tail, l, w) in [
        ("1u 2u*1", 1.0, 2.0),
        ("1u*1 2u", 1.0, 2.0),
        ("1u*1 w=2u", 1.0, 2.0),
        ("1e-6*2 1u", 2.0, 1.0),
        ("1u 2*k", 1.0, 4.0),
        ("1u 2u/2", 1.0, 1.0),
        ("1u 4u/2+1u", 1.0, 3.0),
        ("1u 1e-6+1u", 1.0, 2.0),
        ("1u 2u-1u", 1.0, 1.0),
        ("1u -2u*-1", 1.0, 2.0),
        ("1u +2u*1", 1.0, 2.0),
        ("1u 2umos*1", 1.0, 2.0),
        ("1u 2u*1mos", 1.0, 0.002),
        ("1u 2u**1", 1.0, 2.0),
        ("1u .5u*2", 1.0, 1.0),
        ("1u 2.u*1", 1.0, 2.0),
        ("1u 2u*1 $ comment", 1.0, 2.0),
        ("1u\n+ 2u*1", 1.0, 2.0),
        ("1u 2n7002", 1.0, 0.002),
        ("1u 2u.5", 1.0, 2.0),
        ("1u 2^2", 1.0, 2e6),
        ("1u 2**2", 1.0, 4e6),
        ("1u 2*3^2", 1.0, 18e6),
        ("1u 2<3", 1.0, 2e6),
        ("1u 2>3", 1.0, 2e6),
        ("1u 2||3", 1.0, 2e6),
        ("1u 2u%", 1.0, 2.0),
        ("1u 2u#", 1.0, 2.0),
        ("1u 2u@3", 1.0, 2.0),
        ("1u 2u~3", 1.0, 2.0),
        ("1u 2u:3", 1.0, 2.0),
        ("1u 2u_3", 1.0, 2.0),
        ("1u 2u[3]", 1.0, 2.0),
        ("1u 2u|3", 1.0, 2.0),
        ("l=1u w=2^2", 1.0, 2e6),
        ("l=1u w=2**2", 1.0, 4e6),
        ("l=1u w=2*3^2", 1.0, 18e6),
        ("l=1u w=2u.5", 1.0, 2.0),
        ("l=1u w=2umos", 1.0, 2.0),
        ("l=1u w='2^2'", 1.0, 4e6),
    ] {
        let text = format!("{cards}M1 d g 0 0 n {tail}\n");
        assert_eq!(
            clean_sizes(&[("deck.sp", &text)]).await,
            [("M1".to_owned(), l, w)],
            "{tail}"
        );
    }
    // the model at the 4th place
    for tail in ["1u 2u*1", "1u*1 2u", "1u (2u*1)"] {
        let text = format!("{cards}M1 d g 0 n {tail}\n");
        assert_eq!(
            clean_sizes(&[("deck.sp", &text)]).await,
            [("M1".to_owned(), 1.0, 2.0)],
            "{tail}"
        );
    }
}

/// `(` and `)` between the values of a MOSFET line only separate them (decks
/// `gv_*`, `gx_*`, `hx_*`).
#[tokio::test]
async fn parentheses_between_mosfet_values() {
    let cards = format!("{IF_N}.param k=2u\n");
    for (tail, l, w) in [
        ("1u (2u*1)", 1.0, 2.0),
        ("l=1u (k*1)", 1.0, 2.0),
        ("l=1u (2u)", 1.0, 2.0),
        ("l=1u ( 2u*1 )", 1.0, 2.0),
        ("w=1u (k*1)", 100.0, 2.0),
        ("w=5u (1u)", 100.0, 1.0),
        ("1u 2u (1p*1)", 1.0, 2.0),
        ("l=1u w=2u (1p*1)", 1.0, 2.0),
        ("l=1u (2u + 3u)", 1.0, 2.0),
        ("l=1u (2u 3u)", 1.0, 2.0),
        ("1u ((2u*1))", 1.0, 2.0),
        ("1u (2u*1) (3p*1)", 1.0, 2.0),
        ("l=1u w=2u (3p*1) (4p*1)", 1.0, 2.0),
        ("1u (2u*1)$c", 1.0, 2.0),
        ("1u 2u*1,3p*1", 1.0, 2.0),
        ("1u,(2u*1)", 1.0, 2.0),
        ("1u\n+ (2u*1)", 1.0, 2.0),
        ("l=1u\n+ (2u*1)", 1.0, 2.0),
        ("1u (2u*1) w=5u", 1.0, 2.0),
        ("l=1u (2u*1) w=5u", 1.0, 2.0),
    ] {
        let text = format!("{cards}M1 d g 0 0 n {tail}\n");
        assert_eq!(
            clean_sizes(&[("deck.sp", &text)]).await,
            [("M1".to_owned(), l, w)],
            "{tail}"
        );
    }
}

/// A printed MOSFET line keeps the order of its values, so that it reads the same.
#[tokio::test]
async fn a_printed_mosfet_line_reads_the_same() {
    let cards = format!("{IF_N}.param k=2u\n");
    for tail in [
        "w=5u 1u",
        "w=5u k w=6u",
        "1u w=5u 2u",
        "off w=5u",
        "w=5u off 1u",
        "1u k w=5u",
        "1u + 2u",
        "w=(2u + 3u)",
        "l=5u 1u w=6u",
    ] {
        let text = format!("{cards}M1 d g 0 0 n {tail}\n");
        let rows = clean_sizes(&[("deck.sp", &text)]).await;
        let (_dir, parsed, built) = parse_deck(&[("deck.sp", &text)]).await;
        let (ast, _) = built.build(parsed);
        let printed = format!("* title\n{ast}\n");
        assert_eq!(
            clean_sizes(&[("deck.sp", &printed)]).await,
            rows,
            "{tail}: {printed}"
        );
    }
}

/// Each word after the model name takes a place, a `key=value` too. A later
/// value wins over an earlier one of the same name (decks `mx_*`,
/// `m_off_*`, `m_one_key_after_value`).
#[tokio::test]
async fn words_and_keys_mix_on_a_mosfet_line() {
    for (tail, l, w) in [
        ("l=1u w=2u", 1.0, 2.0),
        ("1u 2u", 1.0, 2.0),
        ("1u w=2u", 1.0, 2.0),
        ("w=2u 1u", 100.0, 1.0),
        ("w=2u 1u 3u", 100.0, 1.0),
        ("l=1u 2u", 1.0, 2.0),
        ("1u w=2u 3u", 1.0, 2.0),
        ("1u l=2u 3u", 2.0, 100.0),
        ("w=2u k", 100.0, 7.0),
        ("w=2u off 1u", 100.0, 2.0),
        ("1u w=2u off", 1.0, 2.0),
        ("l=1u w=2u off", 1.0, 2.0),
        ("l=1u off w=2u", 1.0, 2.0),
        ("off w=2u", 100.0, 2.0),
        ("OFF l=1u OFF w=2u", 1.0, 2.0),
        ("off off 1u", 100.0, 100.0),
        ("l=1u w=2u ic=1,1,1", 1.0, 2.0),
        ("l=1u w=2u off ic=1,1,1", 1.0, 2.0),
        ("l=1u ic=1,1,1 w=2u off", 1.0, 2.0),
        ("'1u' w=2u", 1.0, 2.0),
        ("'1u' '2u'", 1.0, 2.0),
        ("l=1u '2u'", 1.0, 2.0),
    ] {
        let text = format!("{IF_N}.param k=7u\nM1 d g 0 0 n {tail}\n");
        let rows = clean_sizes(&[("deck.sp", &text)]).await;
        assert_eq!(rows, [("M1".to_owned(), l, w)], "{tail}");
        // the same over `+` lines
        let text = format!(
            "{IF_N}.param k=7u\nM1 d g 0 0 n\n{}\n",
            tail.split(' ')
                .map(|word| format!("+ {word}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let rows = clean_sizes(&[("deck.sp", &text)]).await;
        assert_eq!(rows, [("M1".to_owned(), l, w)], "{tail}");
    }
    // `=` on the next line
    let text = format!("{IF_N}M1 d g 0 0 n l=\n+ 1u w\n+ =2u\n");
    let rows = clean_sizes(&[("deck.sp", &text)]).await;
    assert_eq!(rows, [("M1".to_owned(), 1.0, 2.0)]);
    // what is printed reads the same
    for tail in ["w=2u 1u", "1u w=2u 3u", "w=2u off 1u", "l=1u off w=2u"] {
        let text = format!("{IF_N}M1 d g 0 0 n {tail}\n");
        let (_dir, parsed, files) = parse_deck(&[("deck.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let printed = format!("{ast}");
        let again = clean_sizes(&[("deck.sp", &format!("* again\n{printed}\n"))]).await;
        let first = clean_sizes(&[("deck.sp", &text)]).await;
        assert_eq!(again, first, "{tail}: {printed}");
    }
}

/// `$` in a word that starts with a letter is a character of the name. After a
/// value, it starts a comment (decks `cm_dollar_*`, `m_name_special`,
/// `p_dollar_in_name`).
#[tokio::test]
async fn dollar_in_a_word() {
    for (text, want) in [
        ("M$1 d g 0 0 n l=1u w=1u\n", ("M$1", 1.0, 1.0)),
        ("M1 d$a g$b 0 0 n l=1u w=1u\n", ("M1", 1.0, 1.0)),
        (
            ".model q$a nmos level=54\nM1 d g 0 0 q$a l=1u w=1u\n",
            ("M1", 1.0, 1.0),
        ),
        (
            ".subckt s$a d g\nM1 d g 0 0 n l=1u w=1u\n.ends s$a\nX$1 d g s$a\n",
            ("X$1.M1", 1.0, 1.0),
        ),
        (".param a$b=2u\nM1 d g 0 0 n l=a$b w=1u\n", ("M1", 2.0, 1.0)),
        (
            ".param a$b=2u\nM1 d g 0 0 n l='a$b*2' w=1u\n",
            ("M1", 4.0, 1.0),
        ),
        (
            ".param a$b=2u\nM1 d g 0 0 n l=a$b*2 w=1u\n",
            ("M1", 4.0, 1.0),
        ),
        (".param a$=3u\nM1 d g 0 0 n l=a$ w=1u\n", ("M1", 3.0, 1.0)),
        (
            ".param a$b=2u c='a$b+1u'\nM1 d g 0 0 n l=c w=1u\n",
            ("M1", 3.0, 1.0),
        ),
        // after a value: a comment
        ("M1 d g 0 0 n l=1u$ w=9u\n", ("M1", 1.0, 100.0)),
        ("M1 d g 0 0 n l='1u'$ w=9u\n", ("M1", 1.0, 100.0)),
        ("M1 d g 0 0 n l=1u w=2u$X=3 $Y=4\n", ("M1", 1.0, 2.0)),
        ("M1 d g 0 0 n l=1u w=2u $ w=9u\n", ("M1", 1.0, 2.0)),
        (
            ".param a=1u$ b=2u\nM1 d g 0 0 n l=a w=1u\n",
            ("M1", 1.0, 1.0),
        ),
        // the other characters of a name
        (
            ".param _a=1u a_b=2u a.b=4u\nM1 d g 0 0 n l=_a w=a_b\n",
            ("M1", 1.0, 2.0),
        ),
        (
            ".param _a=1u a_b=2u a.b=4u\nM1 d g 0 0 n l=a.b w='a.b*2'\n",
            ("M1", 4.0, 8.0),
        ),
    ] {
        let rows = clean_sizes(&[("deck.sp", &format!("{IF_N}{text}"))]).await;
        assert_eq!(rows, [(want.0.to_owned(), want.1, want.2)], "{text}");
    }
    // a name read after the comment is not defined
    let text = format!("{IF_N}.param a=1u$ b=2u\nM1 d g 0 0 n l=b w=1u\n");
    assert!(if_deck(&text).await.1[0].1.starts_with("error"), "{text}");
}

/// A word `//` starts a comment on every kind of line (decks `cm_slash_*`,
/// `o_slash_*`)
#[tokio::test]
async fn slash_comments() {
    for (text, want) in [
        ("M1 d g 0 0 n l=1u w=2u // w=9u\n", ("M1", 1.0, 2.0)),
        ("M1 d g 0 0 n l=1u // w=9u\n", ("M1", 1.0, 100.0)),
        (
            "// a comment line\nM1 d g 0 0 n l=1u w=2u\n",
            ("M1", 1.0, 2.0),
        ),
        ("M1 d g 0 0 n l=1u w=2u //c\n", ("M1", 1.0, 2.0)),
        (
            ".param k=2u // k=9u\nM1 d g 0 0 n l=k w=2u\n",
            ("M1", 2.0, 2.0),
        ),
        (
            ".model q nmos level=54 // note\nM1 d g 0 0 q l=1u w=2u\n",
            ("M1", 1.0, 2.0),
        ),
        (
            ".subckt s d g // note\nM1 d g 0 0 n l=1u w=2u\n.ends // note\nX1 d g s // c\n",
            ("X1.M1", 1.0, 2.0),
        ),
        // a comment line does not end the statement
        ("M1 d g 0 0 n l=1u\n// note\n+ w=2u\n", ("M1", 1.0, 2.0)),
        ("M1 d g 0 0 n l=1u // note\n+ w=2u\n", ("M1", 1.0, 2.0)),
        // not in quotes
        (
            ".param s='a//b' k=2u\nM1 d g 0 0 n l=k w=2u\n",
            ("M1", 2.0, 2.0),
        ),
    ] {
        let rows = clean_sizes(&[("deck.sp", &format!("{IF_N}{text}"))]).await;
        assert_eq!(rows, [(want.0.to_owned(), want.1, want.2)], "{text}");
    }
    // a path keeps its slashes
    let rows = clean_sizes(&[
        (
            "deck.sp",
            &format!("{IF_N}.inc './/a.inc'\nM1 d g 0 0 n l=k w=2u\n"),
        ),
        ("a.inc", ".param k=3u\n"),
    ])
    .await;
    assert_eq!(rows, [("M1".to_owned(), 3.0, 2.0)]);
}

/// A comma separates the items after a value, on every kind of line (decks
/// `pc_*`, `p_commas`)
#[tokio::test]
async fn commas_between_items() {
    for (text, want) in [
        (".param a=1u,b=2u\nM1 d g 0 0 n l=a w=b\n", (1.0, 2.0)),
        (".param a=1u ,b=2u\nM1 d g 0 0 n l=a w=b\n", (1.0, 2.0)),
        (".param a=1u , b=2u\nM1 d g 0 0 n l=a w=b\n", (1.0, 2.0)),
        (".param a=1u,\n+ b=2u\nM1 d g 0 0 n l=a w=b\n", (1.0, 2.0)),
        (".param a='1u',b='2u'\nM1 d g 0 0 n l=a w=b\n", (1.0, 2.0)),
        (".param a=1u,b=2u,c=3u\nM1 d g 0 0 n l=a w=c\n", (1.0, 3.0)),
        (
            ".param a='1u*1',b='2u'\nM1 d g 0 0 n l='a' w='b'\n",
            (1.0, 2.0),
        ),
        ("M1 d g 0 0 n l=1u,w=2u\n", (1.0, 2.0)),
        (".option defl=1u,defw=2u\nM1 d g 0 0 n\n", (1.0, 2.0)),
        (".option defl=1u , defw=2u\nM1 d g 0 0 n\n", (1.0, 2.0)),
        (
            ".model q nmos level=54,lmin=0\nM1 d g 0 0 q l=1u w=2u\n",
            (1.0, 2.0),
        ),
        (
            ".subckt s d g a=1u,b=2u\nM1 d g 0 0 n l=a w=b\n.ends\nX1 d g s\n",
            (1.0, 2.0),
        ),
        (
            ".subckt s d g a=9u b=9u\nM1 d g 0 0 n l=a w=b\n.ends\nX1 d g s a=1u,b=2u\n",
            (1.0, 2.0),
        ),
    ] {
        let rows = clean_sizes(&[("deck.sp", &format!("{IF_N}{text}"))]).await;
        assert_eq!(rows.len(), 1, "{text}");
        assert_eq!((rows[0].1, rows[0].2), want, "{text}");
    }
}

/// A form feed is white space (deck `e_formfeed`). `.lib` goes on on `+` lines
/// (deck `lib_cont`).
#[tokio::test]
async fn form_feed_and_library_lines() {
    for text in [
        "\u{c}\nM1 d g 0 0 n l=1u w=2u\n",
        "M1 d g 0 0 n l=1u w=2u\n\u{c}\n",
        "M1 d g 0 0 n\u{c}l=1u w=2u\n",
    ] {
        let rows = clean_sizes(&[("deck.sp", &format!("{IF_N}{text}"))]).await;
        assert_eq!(rows, [("M1".to_owned(), 1.0, 2.0)], "{text:?}");
    }
    let lib = ".lib a\n.param k=3u\n.endl a\n";
    for head in [
        ".lib\n+ 'x.l'\n+ a\n",
        ".lib 'x.l'\n+ a\n",
        ".lib\n+ 'x.l' a\n",
    ] {
        let deck = format!("{IF_N}{head}M1 d g 0 0 n l=k w=2u\n");
        let rows = clean_sizes(&[("deck.sp", &deck), ("x.l", lib)]).await;
        assert_eq!(rows, [("M1".to_owned(), 3.0, 2.0)], "{head}");
    }
}

/// The short forms of `.data`, and its rows with no `+` (decks `dat_rows_*`)
#[tokio::test]
async fn data_blocks() {
    for (open, close) in [
        (".data", ".enddata"),
        (".da", ".enddata"),
        (".dat", ".enddata"),
        (".data", ".endd"),
        (".data", ".endda"),
        (".DATA", ".ENDDATA"),
    ] {
        let text = format!("{IF_N}{open} dd\n a b\n 1 2\n{close}\n{IF_M1}");
        let rows = clean_sizes(&[("deck.sp", &text)]).await;
        assert_eq!(rows, [("M1".to_owned(), 1.0, 1.0)], "{open} {close}");
    }
}

/// An include is looked for in the folder of the file that includes it, then in
/// the folder of the top file, then in the folders of `.option search`, in the
/// order of the deck (decks `inc_nested_rel_cwd`, `lib_inc_rel_cwd`, `opt_search_*`,
/// `search_option*`)
#[tokio::test]
async fn where_includes_are_found() {
    let q = ".model q nmos level=54\n";
    let m = "M1 d g 0 0 q l=k w=1u\n";
    let k = |n: u32| format!(".param k={n}u\n");
    // the folder of the file wins, then the folder of the top file
    let own = clean_sizes(&[
        ("deck.sp", &format!("* t\n{q}.inc 'sub/a.inc'\n{m}")),
        ("sub/a.inc", ".inc 'b.inc'\n"),
        ("sub/b.inc", &k(1)),
        ("b.inc", &k(2)),
    ])
    .await;
    assert_eq!(own, [("M1".to_owned(), 1.0, 1.0)]);
    let top = clean_sizes(&[
        ("deck.sp", &format!("* t\n{q}.inc 'sub/a.inc'\n{m}")),
        ("sub/a.inc", ".inc 'b.inc'\n"),
        ("b.inc", &k(2)),
    ])
    .await;
    assert_eq!(top, [("M1".to_owned(), 2.0, 1.0)]);
    let lib = clean_sizes(&[
        ("deck.sp", &format!("* t\n{q}.lib 'sub/a.l' s\n{m}")),
        ("sub/a.l", ".lib s\n.inc 'b.inc'\n.endl s\n"),
        ("b.inc", &k(2)),
    ])
    .await;
    assert_eq!(lib, [("M1".to_owned(), 2.0, 1.0)]);
    // `.option search`
    for (head, want) in [
        (".option search='d1'\n.inc 'x.inc'\n", 1.0),
        (".option search=d1\n.inc 'x.inc'\n", 1.0),
        (".option search='./d1/'\n.inc 'x.inc'\n", 1.0),
        (".option search='d1' search='d2'\n.inc 'x.inc'\n", 1.0),
        (".option search='d2' search='d1'\n.inc 'x.inc'\n", 2.0),
        (
            ".option search='d2'\n.option search='d1'\n.inc 'x.inc'\n",
            2.0,
        ),
        (".option search='d9' search='d2'\n.inc 'x.inc'\n", 2.0),
        (".inc 'set.inc'\n.inc 'x.inc'\n", 1.0),
        (
            ".subckt z d g\n.option search='d1'\n.ends\n.inc 'x.inc'\n",
            1.0,
        ),
        (".option search='d1'\n.lib 'y.l' s\n", 3.0),
    ] {
        let rows = clean_sizes(&[
            ("deck.sp", &format!("* t\n{q}{head}{m}")),
            ("set.inc", ".option search='d1'\n"),
            ("d1/x.inc", &k(1)),
            ("d2/x.inc", &k(2)),
            ("d1/y.l", ".lib s\n.param k=3u\n.endl s\n"),
        ])
        .await;
        assert_eq!(rows, [("M1".to_owned(), want, 1.0)], "{head}");
    }
    // the folder of the top file wins over a search folder
    let rows = clean_sizes(&[
        (
            "deck.sp",
            &format!("* t\n{q}.option search='d1'\n.inc 'x.inc'\n{m}"),
        ),
        ("x.inc", &k(5)),
        ("d1/x.inc", &k(1)),
    ])
    .await;
    assert_eq!(rows, [("M1".to_owned(), 5.0, 1.0)]);
    // a sibling in the search folder
    let rows = clean_sizes(&[
        (
            "deck.sp",
            &format!("* t\n{q}.option search='d1'\n.inc 'x.inc'\n{m}"),
        ),
        ("d1/x.inc", ".inc 'sib.inc'\n"),
        ("d1/sib.inc", &k(6)),
    ])
    .await;
    assert_eq!(rows, [("M1".to_owned(), 6.0, 1.0)]);
    // not found anywhere: the error of the include
    let (_dir, parsed, files) = parse_deck(&[(
        "deck.sp",
        &format!("* t\n{q}.option search='d1'\n.inc 'none.inc'\n"),
    )])
    .await;
    let (_, has_err) = files.build(parsed);
    assert!(has_err);
}

/// A size option of 0 or below gives the default back, as the last plain number
/// (decks `zd_*`, `zq_*`).
#[tokio::test]
async fn size_options_of_zero_and_below() {
    let l2 = ".model n.1 nmos level=54 lmin=1e-9 lmax=2u wmin=1e-9 wmax=1\n\
              .model n.2 nmos level=54 lmin=2u lmax=1 wmin=1e-9 wmax=1\n";
    let line = "M1 d g 0 0 n w=1u\n";
    for (head, want) in [
        (".option defl=0\n", "n.2"),
        (".option defl=-1u\n", "n.2"),
        (".option defl=1u\n.option defl=0\n", "n.2"),
        (".option defl=1u defl=-1u\n", "n.2"),
        (".option defl=1u\n", "n.1"),
        (".option defl=1u scale=10\n", "n.2"),
        // the default of 100u, scaled
        (".option defl=0 scale=1e-2\n", "n.1"),
    ] {
        let text = format!("* t\n{head}{l2}{line}");
        assert_eq!(one_model(&text).await, want, "{head}");
    }
    let (l, w) = (100.0, 100.0);
    for head in [".option defl=0 defw=0\n", ".option defl=-1u defw=-1u\n"] {
        let text = format!("{IF_N}{head}M1 d g 0 0 n\n");
        assert_eq!(one_size(&text).await, (l, w), "{head}");
    }
    for head in [
        ".option scale=0\n",
        ".option scale=-1\n",
        ".option scale=2\n.option scale=0\n",
        ".option scale=2 scale=-1\n",
    ] {
        let text = format!("{IF_N}{head}M1 d g 0 0 n l=1u w=1u\n");
        assert_eq!(one_size(&text).await, (1.0, 1.0), "{head}");
    }
    let line = "M1 d g 0 0 n l=1u w=1u\n";
    for head in [".option scalm=0\n", ".option scalm=-1\n"] {
        let text = format!("* t\n{head}{l2}{line}");
        assert_eq!(one_model(&text).await, "n.1", "{head}");
    }
    let bin = ".model n.1 nmos level=54 lmin=1e-3 lmax=2e-3 wmin=1e-9 wmax=1e3\n";
    let text = format!("* t\n.option scalm=1e-3\n{bin}M1 d g 0 0 n l=1.5u w=1u\n");
    assert_eq!(one_model(&text).await, "n.1");
    let text = format!("* t\n.option scalm=1e-3 scalm=0\n{bin}M1 d g 0 0 n l=1.5u w=1u\n");
    assert!(one_model(&text).await.starts_with("error"));
    // a size on the line below 0 still stops
    for text in [
        format!("{IF_N}M1 d g 0 0 n l=-1u w=1u\n"),
        format!("{IF_N}M1 d g 0 0 n l=1u w=-1u\n"),
        format!("{IF_N}.option scale=-1\nM1 d g 0 0 n l=-1u w=1u\n"),
    ] {
        assert!(one_model(&text).await.starts_with("error"), "{text}");
    }
}

/// `hertz` reads 0 in a value, as `time` does, and a condition cannot read it
/// (decks `sv_hertz*`)
#[tokio::test]
async fn hertz_reads_zero() {
    let text = format!("{IF_N}M1 d g 0 0 n l='hertz+1u' w=1u\n");
    assert_eq!(one_size(&text).await, (1.0, 1.0));
    let text = ".model n.1 nmos level=54 lmin='hertz+1e-9' lmax=1 wmin=1e-9 wmax=1\nM1 d g 0 0 n l=1u w=1u\n";
    assert_eq!(one_model(&format!("* t\n{text}")).await, "n.1");
    for head in [".if (hertz>1)\n", ".param h='hertz'\n.if (h>1)\n"] {
        let text = format!("{IF_N}{head}{IF_M1}.else\n{IF_M2}.endif\n");
        let (_, rows) = if_deck(&text).await;
        assert_eq!(rows.len(), 1, "{head}: {rows:?}");
        assert!(rows[0].1.starts_with("error"), "{head}: {rows:?}");
    }
}

/// A level 72 bin with both `nfin*` and `wsheet*` limits reads both. A sheet bin
/// needs a sheet width above 0 on the line. `.option scale` scales WSHEET
/// (decks `h_*`, `hb_*`).
#[tokio::test]
async fn finfet_bins_of_both_kinds() {
    let both = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4 wsheetmin=1e-8 wsheetmax=1e-6\n\
                .model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=4 nfinmax=9 wsheetmin=1e-8 wsheetmax=1e-6\n";
    let other = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4 wsheetmin=1e-8 wsheetmax=1e-7\n\
                 .model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4 wsheetmin=1e-7 wsheetmax=1e-6\n";
    let mix = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6\n\
               .model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=9\n";
    let fins = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\n";
    let sheet = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6\n";
    let sheet4 = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6 nsheetmin=3 nsheetmax=9\n";
    let zero = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=0 wsheetmax=1e-6\n";
    let fin0 = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=0 nfinmax=4\n";
    let six = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wmin=1e-3 wmax=1 nfinmin=1 nfinmax=4\n";
    let scaled = ".model f.1 nmos level=72 lmin=1e-9 lmax=1e-3 wsheetmin=1e-9 wsheetmax=1e-7\n\
                  .model f.2 nmos level=72 lmin=1e-9 lmax=1e-3 wsheetmin=1e-7 wsheetmax=1e-5\n";
    for (head, bins, tail, want) in [
        ("", both, "l=500n nfin=6 wsheet=2e-8", Some("f.2")),
        ("", both, "l=500n wsheet=2e-8", Some("f.1")),
        ("", both, "l=500n nfin=6", None),
        ("", both, "l=500n", None),
        ("", both, "l=500n w=2e-8", None),
        ("", other, "l=500n nfin=2 wsheet=2e-7", Some("f.2")),
        ("", other, "l=500n wsheet=2e-7", Some("f.2")),
        ("", other, "l=500n nfin=2", None),
        ("", mix, "l=500n nfin=2", Some("f.2")),
        ("", mix, "l=500n wsheet=2e-8", Some("f.1")),
        ("", mix, "l=500n nfin=2 wsheet=2e-8", Some("f.1")),
        ("", fins, "l=500n wsheet=2e-8", Some("f.1")),
        ("", sheet, "l=500n nfin=2 wsheet=2e-8", Some("f.1")),
        ("", sheet4, "l=500n wsheet=2e-8 nsheet=1", Some("f.1")),
        ("", sheet4, "l=500n wsheet=2e-8", Some("f.1")),
        ("", zero, "l=500n", None),
        ("", zero, "l=500n wsheet=0", None),
        ("", fin0, "l=500n", Some("f.1")),
        ("", fin0, "l=500n nfin=0", None),
        ("", six, "l=500n w=1u nfin=2", Some("f.1")),
        (
            ".option scale=10\n",
            scaled,
            "l=500n wsheet=5e-8",
            Some("f.2"),
        ),
    ] {
        let text = format!("* t\n{head}{bins}M1 d g 0 0 f {tail}\n");
        let got = one_model(&text).await;
        match want {
            Some(want) => assert_eq!(got, want, "{head}{bins}{tail}"),
            None => assert!(got.starts_with("error"), "{head}{bins}{tail}: {got}"),
        }
    }
}

/// The values after `ic=` belong to it: they take no place of L or W (decks
/// `lt_ic_*`)
#[tokio::test]
async fn values_after_ic() {
    for (tail, l, w) in [
        ("ic=1,1,1", 100.0, 100.0),
        ("ic=1,1,1 l=1u w=3u", 1.0, 3.0),
        ("l=1u ic=1,1,1 3u", 1.0, 100.0),
        ("ic=1,1,1 1u 3u", 100.0, 100.0),
        ("ic=1,2 1u 3u", 100.0, 100.0),
        ("ic=1 1u 3u", 100.0, 100.0),
        ("ic=1, 1, 1 l=1u", 1.0, 100.0),
        ("l=1u ad=1p,3u", 1.0, 100.0),
    ] {
        let text = format!("{IF_N}M1 d g 0 0 n {tail}\n");
        assert_eq!(one_size(&text).await, (l, w), "{tail}");
    }
}

/// A `,` can end a `.param` or `.option` statement, and `.inc` goes on on a `+`
/// line (decks `cc_*`, `in_over_plus`)
#[tokio::test]
async fn ends_of_statements() {
    let text = format!("{IF_N}.param a=1u,\nM1 d g 0 0 n l=a w=1u\n");
    assert_eq!(one_size(&text).await, (1.0, 1.0));
    let text = format!("{IF_N}.option defl=1u,\nM1 d g 0 0 n w=1u\n");
    assert_eq!(one_size(&text).await, (1.0, 1.0));
    let rows = clean_sizes(&[
        (
            "deck.sp",
            &format!("{IF_N}.inc\n+ 'a.inc'\nM1 d g 0 0 n l=k w=1u\n"),
        ),
        ("a.inc", ".param k=1u\n"),
    ])
    .await;
    assert_eq!(rows, [("M1".to_owned(), 1.0, 1.0)]);
}

/// A `.lib <name>` line in a section is read over, and the first `.endl` ends the
/// section (decks `ln_*`)
#[tokio::test]
async fn section_lines_in_a_section() {
    for (library, call, l) in [
        (".lib a\n.lib b\n.param k=1u\n.endl b\n.endl a\n", "a", 1.0),
        (".lib a\n.lib b\n.param k=1u\n.endl b\n.endl a\n", "b", 1.0),
        (
            ".lib a\n.param k=1u\n.lib b\n.param k=2u\n.endl b\n.endl a\n",
            "a",
            2.0,
        ),
        (
            ".lib a\n.lib 'x.l' b\n.lib b\n.param k=2u\n.endl b\n.endl a\n",
            "a",
            2.0,
        ),
        (
            ".lib a\n.lib b\n.lib c\n.param k=1u\n.endl c\n.endl b\n.endl a\n",
            "a",
            1.0,
        ),
        (
            ".lib a\n.param k=2u\n.lib b\n.lib c\n.param k=1u\n.endl c\n.endl b\n.endl a\n",
            "c",
            1.0,
        ),
        // the first `.endl` ends the section
        (
            ".lib a\n.lib b\n.param k=1u\n.endl b\n.param k=3u\n.endl a\n",
            "a",
            1.0,
        ),
        (
            ".lib a\n.param k=1u\n.endl zzz\n.param k=5u\n.lib c\n.endl c\n",
            "a",
            1.0,
        ),
        (".lib a\n.param k=1u\n.endl a\n.endl a\n", "a", 1.0),
        (
            ".LIB B $ note\n.param k=1u\n.LIB A $ note\n.ENDL\n",
            "b",
            1.0,
        ),
    ] {
        let rows = clean_sizes(&[
            (
                "deck.sp",
                &format!("{IF_N}.lib 'x.l' {call}\nM1 d g 0 0 n l=k w=1u\n"),
            ),
            ("x.l", library),
        ])
        .await;
        assert_eq!(rows, [("M1".to_owned(), l, 1.0)], "{library}");
    }
}

/// A deck of `depth` subckt levels. `s0` holds the MOSFET line `leaf`, and each `sK`
/// holds one instance of the level below it. `above` comes before the definitions, and
/// `below` after them, at the top level.
fn tower(depth: usize, above: &str, leaf: &str, below: &str) -> String {
    let mut deck = format!(".model n nmos level=54\n{above}\n.subckt s0 d g\n{leaf}\n.ends\n");
    for k in 1..depth {
        writeln!(deck, ".subckt s{k} d g\nX0 d g s{}\n.ends", k - 1).unwrap();
    }
    writeln!(deck, "{below}\nX0 d g s{}", depth - 1).unwrap();
    deck
}

/// The far reads of both walks, which must agree, and the L of the first MOSFET
async fn far_reads(deck: &str) -> (Vec<(String, usize)>, f64) {
    let (_dir, parsed, files) = parse_text(&[("top.sp", deck)]).await;
    let (ast, _) = files.build(parsed);
    let mut l = None;
    let listed = ast.for_each_mosfet_model(|m| {
        l.get_or_insert(m.selection.as_ref().unwrap().l);
    });
    assert_eq!(listed, ast.model_usage().far_reads, "{deck}");
    let rows = listed.into_iter().map(|f| (f.name, f.levels)).collect();
    (rows, l.unwrap())
}

#[tokio::test]
async fn warn_of_a_read_beyond_the_reach_of_hspice() {
    let u = || vec![("u".to_owned(), 32)];
    let read = "M0 d g 0 0 n l=u w=1u";
    for mode in ["", ".option parhier=local\n"] {
        // HSPICE reads a name 31 levels above the reader, and stops at 32 (`*_read_*`)
        let above = format!("{mode}.param u=7n");
        assert_eq!(
            far_reads(&tower(31, &above, read, "")).await,
            (vec![], 7e-9),
            "{mode}"
        );
        assert_eq!(
            far_reads(&tower(32, &above, read, "")).await,
            (u(), 7e-9),
            "{mode}"
        );
    }
    // a function has the same reach (`g_fun_*`)
    let (rows, _) = far_reads(&tower(
        32,
        ".param f(x)='x*2'",
        "M0 d g 0 0 n l='f(3n)' w=1u",
        "",
    ))
    .await;
    assert_eq!(rows, [("f".to_owned(), 32)]);
}

#[tokio::test]
async fn warn_of_a_definition_that_hspice_cannot_reach() {
    // under `global`, HSPICE takes a nearer value from 32 levels on, with no message:
    // L=5n, and L=100n here (`g_top_*`). `local` takes the near value in both tools.
    let deck = |depth: usize, mode: &str| {
        let mut deck = format!("{mode}.model n nmos level=54\n.param v=100n\n");
        deck += ".subckt s0 d g v=1n\nM0 d g 0 0 n l=v w=1u\n.ends\n";
        for k in 1..depth {
            writeln!(deck, ".subckt s{k} d g v=1n\nX0 d g s{} v=5n\n.ends", k - 1).unwrap();
        }
        writeln!(deck, "X0 d g s{} v=5n", depth - 1).unwrap();
        deck
    };
    assert_eq!(far_reads(&deck(31, "")).await, (vec![], 100e-9));
    assert_eq!(
        far_reads(&deck(32, "")).await,
        (vec![("v".to_owned(), 32)], 100e-9)
    );
    let local = ".option parhier=local\n";
    assert_eq!(far_reads(&deck(32, local)).await, (vec![], 5e-9));
    assert_eq!(far_reads(&deck(96, local)).await, (vec![], 5e-9));

    // the reach is from the reader, and not from the top (`*_mid_*`)
    let read = "M0 d g 0 0 n l=u w=1u";
    let mid = |depth: usize| {
        tower(depth, "", read, "").replace(
            &format!(".subckt s{} d g\n", depth - 1),
            &format!(".subckt s{} d g\n.param u=7n\n", depth - 1),
        )
    };
    assert_eq!(far_reads(&mid(32)).await.0, []);
    assert_eq!(far_reads(&mid(33)).await.0, [("u".to_owned(), 32)]);
}

#[tokio::test]
async fn warn_of_a_far_read_after_the_same_instance_near_the_top() {
    // the near instance comes first, and it gives the same counts: the far one must
    // still be evaluated in the summary
    let read = "M0 d g 0 0 n l=u w=1u";
    let near = tower(32, ".param u=7n", read, "X9 d g s0");
    assert_eq!(far_reads(&near).await.0, [("u".to_owned(), 32)]);
    let middle = tower(32, ".param u=7n", read, "X9 d g s5");
    assert_eq!(far_reads(&middle).await.0, [("u".to_owned(), 32)]);
    // the summary of many far instances stays a walk of each level once
    let mut wide = tower(32, ".param u=7n", read, "");
    for k in 1..32 {
        wide = wide.replace(
            &format!("X0 d g s{}\n", k - 1),
            &format!("X0 d g s{0}\nX1 d g s{0}\n", k - 1),
        );
    }
    let (_dir, parsed, files) = parse_text(&[("top.sp", &wide)]).await;
    let (ast, _) = files.build(parsed);
    let usage = ast.model_usage();
    assert_eq!(usage.mosfets, 1 << 31);
    assert_eq!(usage.far_reads[0].levels, 32);
}

#[tokio::test]
async fn warn_of_a_far_read_of_its_own_name() {
    // `local`: a text that reads its own name reads it at the level above
    let deck = tower(
        33,
        ".option parhier=local\n.param v=1n",
        ".param v='v+1n'\nM0 d g 0 0 n l=v w=1u",
        "",
    );
    let (rows, l) = far_reads(&deck).await;
    assert_eq!(rows, [("v".to_owned(), 33)]);
    assert!((l - 2e-9).abs() < 1e-15, "{l}");
}

#[tokio::test]
async fn warn_of_a_far_read_in_a_subckt_alone() {
    let deck = tower(32, ".param u=7n", "M0 d g 0 0 n l=u w=1u", "");
    let (_dir, parsed, files) = parse_text(&[("top.sp", &deck)]).await;
    let (ast, _) = files.build(parsed);
    // an instance of `s31` at the top level is the same tower
    let top = ast.model_usage_of("s31").unwrap();
    assert_eq!(top.far_reads[0].levels, 32);
    let listed = ast.for_each_mosfet_model_of("s31", |_| {}).unwrap();
    assert_eq!(listed, top.far_reads);
    assert!(ast.for_each_mosfet_model_of("nope", |_| {}).is_none());
    assert!(ast.model_usage_of("s30").unwrap().far_reads.is_empty());
}

/// A deck whose instance lines are all in an `.if` block is a circuit, not a
/// library: the tool that looks for a library must not skip these lines.
#[tokio::test]
async fn lines_in_a_top_level_if_are_not_a_library() {
    let is_library = async |text: &str| {
        let (_dir, parsed, files) = parse_deck(&[("deck.sp", text)]).await;
        files.build(parsed).0.is_library()
    };
    let cell = ".subckt s d\nM1 d d 0 0 n l=1u w=1u\n.ends\n";
    assert!(is_library(&format!("* t\n{cell}")).await);
    assert!(is_library(&format!("* t\n.if (1)\n.param a=1\n.endif\n{cell}")).await);
    assert!(!is_library(&format!("* t\n{cell}X1 d s\n")).await);
    assert!(!is_library(&format!("* t\n{cell}.if (1)\nX1 d s\n.endif\n")).await);
    let nested = ".if (0)\n.else\n.if (1)\nM1 d d 0 0 n l=1u w=1u\n.endif\n.endif\n";
    assert!(!is_library(&format!("* t\n{nested}")).await);
}

/// `.unprot` has short forms (see `canonical_cmd`): each one ends an encrypted
/// block, and the rest of the file is read.
#[tokio::test]
async fn encrypted_block_ends_at_a_short_form() {
    for end in [".unprot", ".UNPR", ".unp", ".unprotect"] {
        let text = format!("R1 a b 1k\n.prot freelib\nH]=3g\\#\n{end}\nR2 b c 1k\n");
        let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
        assert_eq!(diagnostics(&parsed), [diag("Encrypted", 2)], "{end}");
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err, "{end}");
        assert_eq!(names(&ast), ["R1", "R2"], "{end}");
    }
}

/// With no `.unprot`, an encrypted block takes the rest of the file. The lines
/// of the rest are lost, so this is an error, as for a plain `.prot` block.
#[tokio::test]
async fn encrypted_block_with_no_end_is_an_error() {
    let text = "R1 a b 1k\n.prot freelib\nH]=3g\\#\nR2 b c 1k\n";
    let (_dir, parsed, files) = parse_text(&[("top.sp", text)]).await;
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(names(&ast), ["R1"]);
}

/// A `\` continuation after a `$` that is in a name: the `$` starts no comment,
/// so the next line goes on with the statement.
#[tokio::test]
async fn continuation_after_a_dollar_in_a_name() {
    let rows =
        async |lines: &str| deck_rows(&format!("* t\n.model n nmos level=54\n{lines}")).await;
    let got = rows("M$1 d g 0 0 n l=1u \\\nw=2u\n").await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].0, "M$1");
    assert_eq!((got[0].2, got[0].3), (1.0, 2.0));
    // in a value of a parameter too
    let got = rows(".param a$b=3u\nM1 d g 0 0 n l=a$b \\\nw=2u\n").await;
    assert_eq!((got[0].2, got[0].3), (3.0, 2.0));
    // at the start of a word and after a number, a `$` still starts a comment,
    // and a `\` in the comment continues nothing
    for line in ["w=2u $ note", "w=2u$ note"] {
        let got = rows(&format!(
            "M1 d g 0 0 n l=1u {line} \\\nM2 d g 0 0 n l=1u w=2u\n"
        ))
        .await;
        assert_eq!(got.len(), 2, "{got:?}");
    }
}

/// Two names that differ in a byte that is not UTF-8 are two names: `X1` must
/// call the subckt that it names.
#[tokio::test]
async fn names_with_other_bytes_stay_apart() {
    let dir = TempDir::new("bytes");
    let deck = dir.0.join("deck.sp");
    std::fs::write(
        &deck,
        b"* t\n.subckt \xe9 d\nM1 d d 0 0 one l=1u w=1u\n.ends\n\
          .subckt \xe8 d\nM1 d d 0 0 two l=1u w=1u\n.ends\n\
          X1 a \xe9\nX2 a \xe8\n\
          .model one nmos level=54\n.model two nmos level=54\n",
    )
    .unwrap();
    let (parsed, files) = parse_files(&[deck]).await;
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.subckt.len(), 2);
    let rows = ast.mosfet_models();
    let model = |row: usize| rows[row].selection.as_ref().unwrap().model.name.to_string();
    assert_eq!((model(0).as_str(), model(1).as_str()), ("one", "two"));
}

/// A definition that an instance path re-enters: the counts of a subckt depend
/// on the path above it, so a count kept from another path is wrong. `usage_agrees`
/// (in `deck_rows`) compares the summary with the list.
#[tokio::test]
async fn usage_of_a_definition_that_a_path_reenters() {
    let defs = ".subckt a p q\nxs p q s\n.ends\n\
                .subckt s p q\n.subckt s p q\nM1 p q p q nch l=1u w=1u\n.ends\nxa p q a\n.ends\n\
                .model nch nmos level=54\n";
    for lines in ["X1 a b s\nx2 1 0 a\n", "x2 1 0 a\nX1 a b s\n"] {
        let deck = format!("* t\n{lines}{defs}");
        let rows = deck_rows(&deck).await;
        let bad = rows.iter().filter(|r| r.1.starts_with("error")).count();
        assert_eq!((rows.len(), bad), (2, 1), "{deck}: {rows:?}");
    }
    // a re-entry with other lines: a sibling reaches the definition from the
    // other side, at the same context
    let deck = "* t\nX1 a\nX2 b\n\
                .subckt a p\nM1 p p p p nch l=1u w=1u\nxb p b\n.ends\n\
                .subckt b p\nM2 p p p p nch l=1u w=1u\nxa p a\n.ends\n\
                .model nch nmos level=54\n";
    deck_rows(deck).await;
}

/// A definition under a re-entry keeps its memo when no path can re-enter it.
#[tokio::test]
async fn usage_keeps_counts_of_a_definition_outside_a_cycle() {
    let (_dir, parsed, files) = parse_deck(&[(
        "deck.sp",
        "* t\nX1 a\nX2 a\n\
         .subckt a p\nxl p leaf\nxb p b\n.ends\n\
         .subckt b p\nxl p leaf\nxa p a\n.ends\n\
         .subckt leaf p\nM1 p p p p nch l=1u w=1u\n.ends\n\
         .model nch nmos level=54\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    let names: HashSet<String> = ast
        .reentrant_defs()
        .into_iter()
        .map(|d| d.name.to_lowercase())
        .collect();
    assert_eq!(names, HashSet::from(["a".to_owned(), "b".to_owned()]));
}

/// A name of a parameter with a byte that is not UTF-8 can be read in an
/// expression, and two such names stay two names.
#[tokio::test]
async fn expressions_read_names_with_other_bytes() {
    let dir = TempDir::new("bytes");
    let deck = dir.0.join("deck.sp");
    std::fs::write(
        &deck,
        b"* t\n.model n nmos level=54\n.param p\xe9=1u p\xe8=2u\n\
          M1 d g 0 0 n l=p\xe9 w='p\xe8'\n",
    )
    .unwrap();
    let (parsed, files) = parse_files(&[deck]).await;
    let (ast, _) = files.build(parsed);
    let rows = ast.mosfet_models();
    let chosen = rows[0].selection.as_ref().unwrap();
    assert_eq!((chosen.l, chosen.w), (1e-6, 2e-6));
}
