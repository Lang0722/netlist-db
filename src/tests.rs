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
    let mut top2 = String::new();
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
        ParseErrorInner::Internal(_) => "Internal",
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
        warning: matches!(kind, "UnknownCommand" | "StrayEnds" | "DuplicateSubckt"),
    }
}

/// Parse `files` (name, content); the first one is the top-level file.
async fn parse_text(files: &[(&str, &str)]) -> (TempDir, Parsed, Files) {
    let dir = TempDir::new("netlist");
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
    let good = dir.write("good.sp", "R0 a b 1k\n");
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
        "E1 out 0 DELAY in 0 TD=1n\nG1 out 0 VCCS PWL(1) in 0 0,0 1,1\nR1 a b 1k\n",
    )])
    .await;
    assert_eq!(
        diagnostics(&parsed),
        [diag("Unsupported", 1), diag("Unsupported", 2)]
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
