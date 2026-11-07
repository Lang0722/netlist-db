mod blocks;
mod cmds;
mod data_mc;
pub use data_mc::*;
mod data_meas;
pub use data_meas::*;
mod data_sweep;
pub use data_sweep::*;
mod include;
mod instance;
mod manager;
mod source;
mod statement;
mod tokens;

use alloc::sync::Arc;
use blocks::Blocks;
pub use include::{IncludeLimits, IncludeStep, MAX_INCLUDE_DEPTH};
use include::{include_path, read_include, search_includes, spawn_include};
use manager::{Include, ParseManager};
pub(crate) use source::is_raw_byte;
use source::statement_lines;
pub use statement::{MAX_NESTING, canonical_cmd};
use statement::{Open, Step, local_ast};
use std::{
    mem,
    path::{Path, PathBuf},
    sync::OnceLock,
};
pub(crate) use tokens::{is_formula_text, is_number_list, number_prefix};

use super::{
    Files, Parsed,
    err::{ParseError, ParseErrorInner},
};
use crate::{
    ast::{ASTBuilder, AfterEnd, LocalAST, Segment, SubcktBuilder, TitleLineInfo},
    span::{FileId, FileStorage, LocatedSpan, ParsedId, Pos, Span},
};

const BEGIN_TITLE: &str = ".TITLE";

/// How line 1 of a top file is read. HSPICE ignores line 1 of the top
/// file (the title line), also when it is a dot command (golden
/// `title_line`, `title_dot`). Included files never have a title line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleLine {
    /// Line 1 is the title line (with its `+` lines), unless it is blank, a
    /// comment, a `+` line or any dot command. Its text goes to
    /// [`crate::AST::title`], and a title that is also a well-formed element
    /// gives a `TitleIsElement` warning. So a file that starts with a dot
    /// command reads all its statements, also when HSPICE would ignore that
    /// line (golden `title_dot`). This is the mode of [`parse_top_multi`].
    NotDot,
    /// Line 1 is the title line, unless it starts (in any letter case,
    /// after spaces) with `.subckt`, `.macro`, `.lib`, `.inc`, `.include`,
    /// `.model`, `.prot` or `.protect`: such a file is a library that is
    /// loaded directly (an encrypted one starts with `.prot`), and line 1
    /// is parsed.
    ///
    /// When line 1 is the title line, a `+` line right after it is one
    /// `PlusAfterTitle` error (HSPICE rejects the deck, golden
    /// `title_plus`), and its lines are skipped.
    Auto,
    /// Line 1 is the title line: it is not parsed.
    Yes,
    /// Line 1 is parsed like the other lines.
    No,
}

/// One `.lib` section override: a `.lib '<path>' <section>` call whose
/// file name (the last component of `<path>`) is the file name of `file`,
/// and whose section is `from`, loads the section `to` instead. Names are
/// compared in any letter case. Every other call keeps its section, also
/// another section of the same file (a deck that calls `tt` and `res_tt`
/// switches each one with its own override).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibOverride {
    pub file: PathBuf,
    pub from: String,
    pub to: String,
}

/// Options of [`parse_top_multi_with`].
#[derive(Debug, Clone)]
pub struct ParseOptions {
    /// How line 1 of each top file is read.
    pub title_line: TitleLine,
    /// `.lib` section overrides. When more than one matches a call, the
    /// first one is used. [`Parsed::lib_override_matched`] tells which
    /// ones matched a call.
    pub lib_override: Vec<LibOverride>,
    /// `false`: a plain `.prot` region is read as usual (HSPICE uses the
    /// definitions in it, golden `prot_subckt`, `prot_param`), and only a
    /// `.prot freelib` block, which HSPICE alone can read, is skipped with an
    /// `Encrypted` warning. `true`: the text of every `.prot` region is
    /// skipped, and `LocalAST::protected` records its lines.
    pub skip_prot: bool,
}

impl Default for ParseOptions {
    /// The options of [`parse_top_multi`]: line 1 follows
    /// [`TitleLine::NotDot`], no `.lib` section is overridden, and a plain
    /// `.prot` region is read.
    #[inline]
    fn default() -> Self {
        Self {
            title_line: TitleLine::NotDot,
            lib_override: Vec::new(),
            skip_prot: false,
        }
    }
}

/// The dot commands after which a top file is a library loaded directly,
/// when they start line 1 (`TitleLine::Auto`).
const LIBRARY_CMDS: [&str; 8] = [
    "subckt", "macro", "lib", "inc", "include", "model", "prot", "protect",
];

/// What `mode` does with line 1 of the top file `text`, or `None` when
/// line 1 has no text or `mode` is `TitleLine::No`.
fn title_line(text: &str, mode: TitleLine) -> Option<TitleLineInfo> {
    let line = text.split('\n').next().unwrap_or_default();
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.trim().is_empty() {
        return None;
    }
    let (skipped, message) = match mode {
        TitleLine::No | TitleLine::NotDot => return None,
        TitleLine::Yes => (
            true,
            "line 1 is the title line (title_line = yes), so it is not parsed".to_owned(),
        ),
        TitleLine::Auto => {
            let cmd = line
                .trim_start_matches([' ', '\t'])
                .strip_prefix('.')
                .map(|s| {
                    let end = s
                        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .unwrap_or(s.len());
                    s[..end].to_ascii_lowercase()
                })
                .filter(|cmd| LIBRARY_CMDS.contains(&canonical_cmd(cmd)));
            match cmd {
                Some(cmd) => (
                    false,
                    format!(
                        "line 1 starts with `.{cmd}`, so the file is read as a library and line 1 is parsed"
                    ),
                ),
                None => (
                    true,
                    "line 1 is the title line (HSPICE ignores line 1 of the top file), so it is not parsed"
                        .to_owned(),
                ),
            }
        }
    };
    Some(TitleLineInfo {
        line: Span {
            start: 0,
            end: line.len(),
            line_num: 1,
        },
        skipped,
        message,
    })
}

/// One open level of a file: the file level, or a `.subckt` body.
struct Level<'a> {
    /// The `.subckt` header of the body, `None` at file level.
    open: Option<Open<'a>>,
    /// The segments of this level so far.
    ast: ASTBuilder,
    /// The statements after the last segment.
    local: LocalAST,
}

impl<'a> Level<'a> {
    fn new(open: Option<Open<'a>>) -> Self {
        Self {
            open,
            ast: ASTBuilder::new(),
            local: LocalAST::default(),
        }
    }

    /// Ends the current `LocalAST`: it becomes a segment when it has
    /// something in it.
    fn flush(&mut self) {
        if !self.local.is_empty() {
            let local = mem::take(&mut self.local);
            self.ast.segments.push(Segment::Local(Box::new(local)));
        }
    }

    /// The finished `.subckt` of a body level, whose text ends at byte
    /// `end`.
    fn into_subckt(mut self, end: usize) -> SubcktBuilder {
        self.flush();
        let open = self.open.expect("a body level");
        let header = open.body.header;
        SubcktBuilder {
            name: open.name,
            ports: open.ports,
            params: open.params,
            ast: self.ast,
            text: Span {
                start: header.location_offset(),
                end,
                line_num: header.location_line(),
            },
        }
    }
}

/// Parses the file whose text is `i`. `blocks` holds its blocks. In a top
/// file (`top`), the parse stops at `.end` (golden `after_end`), and
/// `ASTBuilder::after_end` counts the lines after it. Other files read
/// past an `.end` line, and record it in `LocalAST::ignored_end` (HSPICE
/// ignores it with a warning, golden `end_in_include`).
///
/// The open `.subckt` bodies are a list here, not calls on the stack, so
/// deep nesting does not use stack space; `MAX_NESTING` still limits the
/// depth (see `statement::subckt_header`). Each body ends at its `.ends`. A
/// body that reaches the end of input (or `.end`) without `.ends` keeps the
/// rest of the input and records an error at its `.subckt`.
#[inline]
pub fn ast<'a>(
    manager: Arc<ParseManager>,
    work_dir: PathBuf,
    blocks: &Blocks<'a>,
    mut i: LocatedSpan<'a>,
    top: bool,
) -> ASTBuilder {
    // `levels[0]` is the file level, the last one is the innermost body
    let mut levels = vec![Level::new(None)];
    // the number of `.alter` lines so far
    let mut alter = 0;
    loop {
        let level = levels.last_mut().expect("the file level");
        let body = level.open.as_ref().map(|open| open.body);
        // statement errors are recorded in `level.local.errors`
        let step;
        (i, step) = local_ast(i, body, blocks, &mut level.local);
        match step {
            Step::Open(open) => levels.push(Level::new(Some(open))),
            Step::Close { end } => {
                let subckt = levels.pop().expect("a body level").into_subckt(end);
                let parent = levels.last_mut().expect("the file level");
                parent.local.subckt.push(subckt);
            }
            Step::Include { file_name, section } => {
                level.flush();
                // `lib_override` can choose another section
                let section = section.map(|section| manager.lib_section(file_name, section));
                let res = Arc::new(OnceLock::new());
                let include_pos = Pos::new(i);
                level.ast.segments.push(Segment::Include {
                    res: res.clone(),
                    pos: include_pos,
                });
                let include = Include {
                    file_name: file_name.to_path_buf(),
                    path: include_path(&manager.roots, &work_dir, file_name),
                    section,
                    pos: include_pos,
                    res,
                };
                if include.path.exists() {
                    spawn_include(&manager, include);
                } else {
                    manager.defer(include);
                }
            }
            Step::Alter { pos, stmt } => {
                level.flush();
                alter += 1;
                level.ast.segments.push(Segment::Alter {
                    index: alter,
                    pos,
                    stmt,
                });
            }
            Step::Cond { line, pos } => {
                level.flush();
                level.ast.segments.push(Segment::Cond { line, pos });
            }
            // HSPICE ignores `.end` in an included file, with a warning,
            // and reads on (golden `end_in_include`)
            Step::End { pos } if !top => {
                level.local.ignored_end.push(pos.map_or(0, |p| p.line_num))
            }
            Step::End { pos } => {
                let after_end = AfterEnd {
                    line: pos.map_or(0, |p| p.line_num),
                    lines: statement_lines(i.fragment()),
                };
                let end = pos.map_or(i.location_offset(), |p| p.start);
                return finish(levels, Some(after_end), end);
            }
            Step::Eof => return finish(levels, None, i.location_offset()),
        }
    }
}

/// The AST of a file whose parse stopped at byte `end` with the open levels
/// `levels` (the file level first): the bodies with no `.ends` are closed,
/// each with an error at its `.subckt`, and their text ends at `end`.
fn finish(mut levels: Vec<Level<'_>>, after_end: Option<AfterEnd>, end: usize) -> ASTBuilder {
    // the innermost first
    while levels.len() > 1 {
        let mut level = levels.pop().expect("a body level");
        let body = level.open.as_ref().expect("a body level").body;
        level.local.errors.push(
            ParseErrorInner::MissingEnds {
                keyword: body.keyword,
                subckt: body.name.to_owned(),
            }
            .record(body.header),
        );
        let subckt = level.into_subckt(end);
        let parent = levels.last_mut().expect("the file level");
        parent.local.subckt.push(subckt);
    }
    let mut file = levels.pop().expect("the file level");
    file.flush();
    file.ast.after_end = after_end;
    file.ast
}

/// When ParseError come form the included file,
/// also return the ASTBuilder, to recover the error
fn error2ast(err: ParseError) -> ASTBuilder {
    let mut ast = ASTBuilder::new();
    let mut local = LocalAST::default();
    local.errors.push(err);
    ast.segments.push(Segment::Local(Box::new(local)));
    ast
}

/// Parses the top files `file_ids` and the files they include, with the
/// default options (see [`ParseOptions::default`]).
pub async fn parse_top_multi<I: Iterator<Item = FileId>>(file_ids: I) -> (Parsed, Files) {
    parse_top_multi_with(file_ids, &ParseOptions::default()).await
}

/// Parses the top files `file_ids` and the files they include, with `opts`.
pub async fn parse_top_multi_with<I: Iterator<Item = FileId>>(
    file_ids: I,
    opts: &ParseOptions,
) -> (Parsed, Files) {
    let file_ids: Vec<FileId> = file_ids.collect();
    let mut roots: Vec<PathBuf> = file_ids
        .iter()
        .filter_map(|id| id.path().parent().map(Path::to_path_buf))
        .collect();
    roots.dedup();
    roots.extend(std::env::current_dir());
    let (manager, done_rx) = ParseManager::with_options(opts, roots);
    let title = opts.title_line;
    // Hold a guard while the top-level files are parsed: the includes of one
    // file may all finish while another top-level file has not spawned its
    // includes yet, and the count must not reach zero (the "done" signal)
    // before every top-level file has been parsed.
    let top_guard = manager.task_guard();
    let handles: Vec<_> = file_ids
        .into_iter()
        .map(|file_id| {
            let handle = crate::spawn({
                let manager = manager.clone();
                let file_id = file_id.clone();
                async move { read_include(manager, file_id, None, Some(title)).await }
            });
            (file_id, handle)
        })
        .collect();
    let mut tops = Vec::with_capacity(handles.len());
    for (file_id, handle) in handles {
        tops.push(match handle.await {
            Ok(res) => res,
            Err(e) => Err(Box::new((
                file_id,
                ParseErrorInner::Internal(format!("parser task failed: {e}")).with(None),
            ))),
        });
    }
    drop(top_guard);
    manager.wait(done_rx).await;
    let top_files: Vec<ParsedId> = tops
        .iter()
        .filter_map(|top| top.as_ref().ok().copied())
        .collect();
    search_includes(&manager, &top_files).await;
    let mut guard = manager.file_storage.lock().await;
    let FileStorage {
        file: mut files,
        parsed: mut inner,
        id2idx,
    } = mem::take(&mut *guard);
    // A top-level file that failed to load becomes an error-only entry: the
    // error is reported, and the other top-level files are still built.
    let top_ids = tops
        .into_iter()
        .map(|top| match top {
            Ok(parsed_id) => parsed_id,
            Err(failed) => {
                let (file_id, e) = *failed;
                let parsed_id = ParsedId(inner.len());
                inner.push((file_id, error2ast(e)));
                files.push(String::new());
                parsed_id
            }
        })
        .collect();
    (
        Parsed {
            top_ids,
            id2idx,
            inner,
            lib_override_matched: manager.lib_override_matched(),
        },
        Files { inner: files },
    )
}

#[tokio::test]
async fn test_top() {
    #[cfg(not(feature = "tracing"))]
    {
        _ = simple_logger::SimpleLogger::new().init();
    }
    #[cfg(feature = "tracing")]
    {
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            // .with_ansi(colored::control::SHOULD_COLORIZE.should_colorize())
            .with_max_level(tracing::Level::DEBUG)
            .with_target(false)
            .with_file(true)
            .with_line_number(true)
            .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
                "%FT%T".to_owned(),
            ))
            .finish();
        _ = tracing::subscriber::set_global_default(subscriber);
    }
    let (parsed, files) = parse_top_multi(
        [FileId::Include {
            path: PathBuf::from("tests/top.sp"),
        }]
        .into_iter(),
    )
    .await;
    println!("{parsed:?}");
    println!("{files:?}");
}
#[cfg(test)]
mod recover_tests;

/// Statements and parse options: `.param` functions,
/// `.global`, `.prot` regions, `.alter`, the title line, `.lib` overrides
/// and analysis commands.
#[cfg(test)]
mod statement_tests;

/// `.lib` override triples, `.endl` in
/// comments, deep `.subckt` nesting, `.macro`/`.eom`, and the title line.
#[cfg(test)]
mod follow_up_tests;

/// The lines after `.end`, and files that are not valid UTF-8.
#[cfg(test)]
mod end_and_utf8_tests;
