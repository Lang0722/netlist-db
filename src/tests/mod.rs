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
    crate::hierarchy::test::init_logger();
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
        ParseErrorInner::IncludeLimit(_) => "IncludeLimit",
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

mod commands;
mod dialect;
mod elements;
const IF_N: &str = "* t\n.model n nmos level=54\n";

const IF_M1: &str = "M1 d g 0 0 n l=1u w=1u\n";

const IF_M2: &str = "M2 d g 0 0 n l=2u w=2u\n";

/// Whether the deck `text` has errors (not warnings), with its MOSFETs as
/// `(path, chosen model or error)`
async fn if_deck(text: &str) -> (bool, Vec<(String, String)>) {
    let (_dir, parsed, files) = parse_deck(&[("deck.sp", text)]).await;
    let (ast, has_err) = files.build(parsed);
    let rows = chosen(&ast);
    (has_err, rows)
}

/// The L and W of the one row of a deck
async fn one_size(text: &str) -> (f64, f64) {
    let rows = deck_rows(text).await;
    assert_eq!(rows.len(), 1, "{text}: {rows:?}");
    assert!(!rows[0].1.starts_with("error"), "{text}: {rows:?}");
    (rows[0].2, rows[0].3)
}

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

/// The model of the one row of a deck
async fn one_model(text: &str) -> String {
    let rows = deck_rows(text).await;
    assert_eq!(rows.len(), 1, "{text}: {rows:?}");
    rows[0].1.clone()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

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

mod errors;
mod if_blocks;
mod line_forms;
mod repairs;
mod selection;
mod selector_rules;
mod stability;
mod values;
