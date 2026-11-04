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
    instance::{ControlledKind, CurrentSource, Instance, InstanceCtx, TranSource, VoltageSource},
};
use std::collections::HashMap;

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
        ParseErrorInner::Trailing => "Trailing",
        ParseErrorInner::OrphanContinuation => "OrphanContinuation",
        ParseErrorInner::StrayEnds => "StrayEnds",
        ParseErrorInner::UnterminatedSubckt(_) => "UnterminatedSubckt",
        ParseErrorInner::DuplicateSubckt(_) => "DuplicateSubckt",
        ParseErrorInner::Unsupported(_) => "Unsupported",
        ParseErrorInner::NonHspice(_) => "NonHspice",
        ParseErrorInner::Internal(_) => "Internal",
        ParseErrorInner::TooDeep(_) => "TooDeep",
        ParseErrorInner::Encrypted { .. } => "Encrypted",
        ParseErrorInner::Encoding(_) => "Encoding",
        ParseErrorInner::TitleIsElement => "TitleIsElement",
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
                Segment::Include(res) => {
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
                | "StrayEnds"
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
    assert_eq!(diagnostics(&parsed), [diag("Syntax", 2)]);
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
        "M1 d g s b nch W=1u L=0.1u extra junk\n\
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
    assert!(!has_err, "a warning is not an error");
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
    assert!(close(num(&r("R1").value), 1e3));
    assert_eq!(r("R1").params.len(), 2);
    assert!(close(num(&r("R2").value), 2e3) && r("R2").params.is_empty());
    assert_eq!(r("R3").mname.as_deref(), Some("rmod"));
    assert!(close(num(&r("R3").value), 3e3));
    assert_eq!(r("R4").mname.as_deref(), Some("rmod"));
    assert!(close(num(&r("R4").value), 4e3) && r("R4").params.len() == 1);
    let InstanceCtx::Capacitor(c1) = &inst(&ast, "C1").ctx else {
        panic!()
    };
    assert_eq!(c1.mname.as_deref(), Some("cmod"));
    assert!(close(num(&c1.value), 1e-15));
    let InstanceCtx::Capacitor(c2) = &inst(&ast, "C2").ctx else {
        panic!()
    };
    assert!(close(num(&c2.value), 2e-15) && c2.params.len() == 1);
    let InstanceCtx::Inductor(l1) = &inst(&ast, "L1").ctx else {
        panic!()
    };
    assert_eq!(l1.mname.as_deref(), Some("lmod"));
    assert!(close(num(&l1.value), 1e-9));
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
    assert!(matches!(v("V1"), VoltageSource::Value(x) if close(num(x), 1.8)));
    for name in ["V2", "V3"] {
        let VoltageSource::Spec(spec) = v(name) else {
            panic!("{name}")
        };
        let Some(TranSource::Func { name: f, args, .. }) = &spec.tran else {
            panic!()
        };
        assert_eq!(f.to_uppercase(), "PULSE");
        assert_eq!(args.len(), 7);
        assert!(close(num(&args[1]), 1.8));
    }
    let VoltageSource::Spec(v4) = v("V4") else {
        panic!()
    };
    assert_eq!(num(v4.dc.as_ref().unwrap()), 0.0);
    let ac = v4.ac.as_ref().unwrap();
    assert!(close(num(&ac.mag), 1.0) && close(num(ac.phase.as_ref().unwrap()), 90.0));
    assert!(matches!(&v4.tran, Some(TranSource::Func { args, .. }) if args.len() == 3));
    let VoltageSource::PWL(v5) = v("V5") else {
        panic!()
    };
    assert_eq!(v5.points.len(), 2);
    assert!(v5.repeat.is_some() && v5.delay.is_some());
    assert!(matches!(v("V6"), VoltageSource::PWL(p) if p.points.len() == 2));
    let i = |name| match &inst(&ast, name).ctx {
        InstanceCtx::Current(i) => &i.source,
        _ => panic!(),
    };
    assert!(matches!(i("I1"), CurrentSource::Value(x) if close(num(x), 1e-3)));
    assert!(matches!(i("I2"), CurrentSource::Spec(s) if s.ac.is_some()));
    let VoltageSource::Spec(v7) = v("V7") else {
        panic!()
    };
    assert!(matches!(&v7.dc, Some(Value::Expr(e)) if e == "vsupply"));
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
        ".global vdd vss",
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
I1 0 a SIN(0 1m 1meg) PAT(1 0 0 1n 1n 5n b1010 r=1)
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
    assert_eq!(diagnostics(&parsed), [diag("Syntax", 6)]);
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
    use crate::parser::MAX_SUBCKT_DEPTH;
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
    assert_eq!(too_deep.len(), DEPTH - MAX_SUBCKT_DEPTH);
    // the first `.subckt` too deep is reported on its own line
    assert_eq!(*too_deep[0], diag("TooDeep", MAX_SUBCKT_DEPTH as u32 + 1));
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(count_instances(&ast), 1);
    // building and printing it do not overflow either
    assert!(ast.to_string().contains("R1 a 0"));

    // a cell library without `.ends`: every cell is kept
    let mut lib = String::new();
    for i in 0..500 {
        writeln!(lib, ".subckt cell{i} a b\nM1 a b 0 0 nch w=1u l=1u").unwrap();
    }
    let (_dir, parsed, files) = parse_text(&[("lib.sp", &lib)]).await;
    let (ast, has_err) = files.build(parsed);
    assert!(has_err);
    assert_eq!(count_instances(&ast), 500);
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
    assert!(matches!(&v.source, VoltageSource::Params(p) if p.is_empty()));
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
    assert!(matches!(&r.value, Value::Expr(e) if e == "1e999"));
    let first = ast.to_string();
    assert!(first.contains("TC=0,0"), "{first}");
    assert!(first.contains("delay=OPT1 ( 0n, 0n, 5n )"), "{first}");
    assert!(first.contains("val2=agauss(0,0,'2*p1')"), "{first}");
    let (_dir2, parsed2, files2) = parse_deck(&[("top.sp", &first)]).await;
    assert_eq!(diagnostics(&parsed2), [], "printed netlist:\n{first}");
    let (ast2, _) = files2.build(parsed2);
    assert_eq!(ast2.to_string(), first);
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
    assert_eq!(keys, ["a1", "square(X)", "a2", "fs(x)", "DelayTime"]);
    let InstanceCtx::Voltage(v) = &inst(&ast, "Vsig1").ctx else {
        panic!()
    };
    let VoltageSource::PWL(pwl) = &v.source else {
        panic!("{:?}", v.source)
    };
    assert!(matches!(&pwl.pwlfile, Some(Value::Expr(e)) if e == "str(wav_left)"));
    let InstanceCtx::Capacitor(c) = &inst(&ast, "cload").ctx else {
        panic!()
    };
    assert!(close(num(&c.value), 1e-12));
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
        reporter.report(&ParseErrorInner::Trailing.with(None), &file_id, "");
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
    assert!(matches!(&v.source, VoltageSource::PWL(p) if p.points.len() == 3));
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

/// `.option parhier`: with GLOBAL (HSPICE's default), a parameter defined at
/// the top level wins over the `.subckt` default; with LOCAL, the definition
/// closest to the transistor wins. A value passed on the instance line wins
/// in both.
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
    assert_eq!(widths(&files.build(parsed).0), [1e-6, 3e-6]);
    let (_dir, parsed, files) = parse_text(&[("top.sp", &deck(".option parhier=local"))]).await;
    assert_eq!(widths(&files.build(parsed).0), [2e-6, 3e-6]);
}

/// `.option scale` multiplies L and W; `defl`/`defw` give L and W when the
/// instance has none; with `WNFLAG=1`, bins hold the width per finger.
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
            ("M3", "pch.2"),
            ("M4", "pch.2"),
        ])
    );
    let m4 = ast.mosfet_models().remove(3).selection.unwrap();
    assert!(close(m4.l, 5e-7) && close(m4.w, 2e-6));
}

/// A subckt that instantiates itself does not make the walk loop forever.
#[tokio::test]
async fn model_selection_stops_at_recursive_subckts() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nmod nmos level=1\n.subckt a x\nM1 x x 0 0 nmod\nX1 x a\n.ends\nX0 n a\n",
    )])
    .await;
    let (ast, _) = files.build(parsed);
    assert_eq!(chosen(&ast), pairs(&[("X0.M1", "nmod")]));
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
    let mut text = String::from(".model nmod nmos level=1\n");
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
