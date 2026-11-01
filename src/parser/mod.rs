mod cmds;
mod data_mc;
pub use data_mc::*;
mod data_meas;
pub use data_meas::*;
mod data_sweep;
pub use data_sweep::*;
mod instance;
mod manager;
mod measure;
mod utils;

use alloc::sync::Arc;
use manager::ParseManager;
use nom::Input as _;
use regex::Regex;
use std::{collections::HashMap, mem, path::PathBuf, sync::OnceLock};
pub use utils::MAX_NESTING;
use utils::{Blocks, Open, Step, local_ast, skip_statement};

use super::{
    Files, Parsed,
    err::{ParseError, ParseErrorInner},
};
use crate::{
    ast::{ASTBuilder, AfterEnd, LocalAST, Segment, SubcktBuilder, TitleLineInfo},
    span::{FileId, FileStorage, LocatedSpan, ParsedId, Pos, Span, span},
};

const BEGIN_TITLE: &str = ".TITLE";

/// How line 1 of a top file is read. HSPICE ignores line 1 of the top
/// file (the title line), also when it is a dot command (golden
/// `title_line`, `title_dot`). Included files never have a title line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleLine {
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
}

impl Default for ParseOptions {
    /// The options of [`parse_top_multi`]: line 1 is parsed, and no
    /// `.lib` section is overridden.
    #[inline]
    fn default() -> Self {
        Self {
            title_line: TitleLine::No,
            lib_override: Vec::new(),
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
        TitleLine::No => return None,
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
                .filter(|cmd| LIBRARY_CMDS.contains(&cmd.as_str()));
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

    /// The finished `.subckt` of a body level.
    fn into_subckt(mut self) -> SubcktBuilder {
        self.flush();
        let open = self.open.expect("a body level");
        SubcktBuilder {
            name: open.name,
            ports: open.ports,
            params: open.params,
            ast: self.ast,
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
/// depth (see `utils::subckt_header`). Each body ends at its `.ends`. A
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
            Step::Close => {
                let subckt = levels.pop().expect("a body level").into_subckt();
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
                let manager_clone = manager.clone();
                let file_path = if file_name.is_absolute() {
                    file_name.to_path_buf()
                } else {
                    work_dir.join(file_name)
                };
                manager.spawn_parse(async move {
                    include(manager_clone, file_path, section, include_pos, res).await;
                });
            }
            Step::Alter { pos } => {
                level.flush();
                alter += 1;
                level
                    .ast
                    .segments
                    .push(Segment::Alter { index: alter, pos });
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
                return finish(levels, Some(after_end));
            }
            Step::Eof => return finish(levels, None),
        }
    }
}

/// The AST of a file whose parse stopped with the open levels `levels`
/// (the file level first): the bodies with no `.ends` are closed, each
/// with an error at its `.subckt`.
fn finish(mut levels: Vec<Level<'_>>, after_end: Option<AfterEnd>) -> ASTBuilder {
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
        let subckt = level.into_subckt();
        let parent = levels.last_mut().expect("the file level");
        parent.local.subckt.push(subckt);
    }
    let mut file = levels.pop().expect("the file level");
    file.flush();
    file.ast.after_end = after_end;
    file.ast
}

/// The number of lines of `text` that are not empty, not comments and not
/// in a `.lib <section>` ... `.endl` block (a `.lib` call reads a section
/// also after `.end`).
fn statement_lines(text: &str) -> u32 {
    let mut n = 0_usize;
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim_start_matches([' ', '\t', '\r']);
        if line.is_empty() || line.starts_with(['*', '$']) {
            continue;
        }
        let mut words = line[..line.find('$').unwrap_or(line.len())].split_whitespace();
        let cmd = words.next().unwrap_or_default();
        if in_section {
            in_section = !cmd.eq_ignore_ascii_case(".endl");
        } else if cmd.eq_ignore_ascii_case(".lib")
            && words.next().is_some()
            && words.next().is_none()
        {
            // `.lib <section>`, not a `.lib <file> <section>` call
            in_section = true;
        } else {
            n += 1;
        }
    }
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Parses the file `file_id`. `title` is the title-line mode of a top
/// file, `None` for an included file.
///
/// A file (or `.lib` section) is parsed once, also when its paths are
/// spelled in other ways (they are compared in canonical form), and every
/// include of it gets that parse, so an include cycle (`a` includes `b`,
/// `b` includes `a`) ends here. The cycle is not an error of the parse: which include line
/// closes it depends on the order in which the files are read, so it is
/// found by the walks that follow the includes in document order
/// (`Files::build`, and the query crate's lowering).
async fn _include(
    manager: Arc<ParseManager>,
    file_id: FileId,
    include_pos: Option<Pos>,
    title: Option<TitleLine>,
) -> Result<ParsedId, (FileId, ParseError)> {
    let mut file_path = file_id.path().to_path_buf();
    // One file can be reached by paths spelled in other ways
    // (`sub/../main.sp`, a link). It is one file with one parse, so an
    // include cycle through such a path is seen (each spelling was a new
    // file before, and the path grew at each turn of the cycle).
    let canonical = file_id.canonical();
    {
        let storage = manager.file_storage.lock().await;
        let known = storage
            .existed(&file_id)
            .or_else(|| canonical.as_ref().and_then(|c| storage.existed(c)));
        if let Some(parsed_id) = known {
            return Ok(parsed_id);
        }
    }
    let (mut contents, non_utf8) = match tokio::fs::read(&file_path).await {
        Ok(bytes) => decode(bytes),
        Err(e) => return Err((file_id, ParseErrorInner::with(e.into(), include_pos))),
    };
    // A UTF-8 byte order mark is not text. Remove it, so that line 1 parses
    // and the spans index the stored text.
    if contents.starts_with('\u{FEFF}') {
        contents.drain(..'\u{FEFF}'.len_utf8());
    }
    let (line_off_set, file_ctx) = if let FileId::Section { path: _, section } = file_id.clone() {
        if let Some((file_ctx, line_off_set)) = match_lib(&contents, &section) {
            (line_off_set, file_ctx)
        } else {
            return Err((
                file_id,
                ParseErrorInner::NoLibSection {
                    path: file_path,
                    section,
                }
                .with(include_pos),
            ));
        }
    } else {
        (0, contents)
    };
    // in a `.lib` section, its first bad line (the bytes outside the
    // section are not read)
    let non_utf8 = match &file_id {
        FileId::Section { .. } => non_utf8.and_then(|_| {
            let at = file_ctx.find('\u{FFFD}')?;
            let lines = file_ctx[..at].matches('\n').count();
            Some(line_off_set.saturating_add(1 + u32::try_from(lines).unwrap_or(u32::MAX)))
        }),
        FileId::Include { .. } => non_utf8,
    };
    let top = title.is_some();
    // Only a whole file has a title line, not a `.lib` section.
    let title = title
        .filter(|_| matches!(file_id, FileId::Include { .. }))
        .and_then(|mode| title_line(&file_ctx, mode));
    let parsed_id = {
        let mut storage = manager.file_storage.lock().await;
        // an include of this file spelled another way, read at the same
        // time, came first
        if let Some(parsed_id) = canonical.as_ref().and_then(|c| storage.existed(c)) {
            storage.alias(file_id, parsed_id);
            return Ok(parsed_id);
        }
        let parsed_id = storage.new_file(file_id);
        if let Some(canonical) = canonical {
            storage.alias(canonical, parsed_id);
        }
        parsed_id
    };
    let mut i = span(&file_ctx, line_off_set);
    let mut plus_after_title = None;
    if title.as_ref().is_some_and(|t| t.skipped) {
        // start at line 2
        i = i.take_from(file_ctx.find('\n').map_or(file_ctx.len(), |n| n + 1));
        // A `+` line right after the title line: HSPICE rejects the deck
        // (golden `title_plus`), also after `*` comment lines (golden
        // `title_plus_gap`). It is one error, and its lines are skipped.
        let next = skip_comment_lines(i);
        if next.fragment().starts_with('+') {
            plus_after_title = Some(ParseErrorInner::PlusAfterTitle.record(next));
            i = skip_statement(next);
        }
    }
    // get dir
    file_path.pop();
    #[cfg(test)]
    assert!(file_path.is_dir());

    // `ast` records statement errors inside the AST and does not fail
    let mut res = {
        let blocks = if top {
            Blocks::new_top(i)
        } else {
            Blocks::new(i)
        };
        ast(manager.clone(), file_path, &blocks, i, top)
    };
    res.title = title;
    res.non_utf8 = non_utf8;
    if let Some(e) = plus_after_title {
        let mut local = LocalAST::default();
        local.errors.push(e);
        res.segments.insert(0, Segment::Local(Box::new(local)));
    }
    manager
        .file_storage
        .lock()
        .await
        .update_ctx(&parsed_id, file_ctx, res);
    Ok(parsed_id)
}

/// The first line at or after `i` (at the start of a line) that is not
/// empty and not a `*` or `$` comment line, after its spaces. Unlike
/// `skip_blank`, it stops at every `+` line, also at a `+` line with only
/// a comment.
fn skip_comment_lines(i: LocatedSpan<'_>) -> LocatedSpan<'_> {
    let b = i.fragment().as_bytes();
    let mut k = 0;
    loop {
        while b.get(k).is_some_and(|c| matches!(c, b' ' | b'\t' | b'\r')) {
            k += 1;
        }
        match b.get(k) {
            Some(b'*' | b'$' | b'\n') => match b[k..].iter().position(|&c| c == b'\n') {
                Some(n) => k += n + 1,
                None => return i.take_from(b.len()),
            },
            _ => return i.take_from(k),
        }
    }
}

/// The text of a file, and the first line with bytes that are not UTF-8.
/// Such bytes are read as U+FFFD, so the rest of the file is kept.
fn decode(bytes: Vec<u8>) -> (String, Option<u32>) {
    match String::from_utf8(bytes) {
        Ok(text) => (text, None),
        Err(e) => {
            let bad = e.utf8_error().valid_up_to();
            let bytes = e.into_bytes();
            let line = bytes[..bad].iter().filter(|&&c| c == b'\n').count() + 1;
            let text = String::from_utf8_lossy(&bytes).into_owned();
            (text, Some(u32::try_from(line).unwrap_or(u32::MAX)))
        }
    }
}

#[inline]
async fn include(
    manager: Arc<ParseManager>,
    file_path: PathBuf,
    section: Option<String>,
    include_pos: Option<Pos>,
    res: Arc<OnceLock<Result<ParsedId, ParseError>>>,
) {
    let file_id = if let Some(section) = section {
        FileId::Section {
            path: file_path.clone(),
            section,
        }
    } else {
        FileId::Include {
            path: file_path.clone(),
        }
    };
    let _res = _include(manager, file_id, include_pos, None).await;
    res.set(_res.map_err(|(_, e)| e)).unwrap();
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

fn error2parsed(file_id: FileId, err: ParseError, lib_override: usize) -> (Parsed, Files) {
    let ast = error2ast(err);
    let parsed_id = ParsedId(0);
    let mut id2idx = HashMap::new();
    id2idx.insert(file_id.clone(), parsed_id);
    (
        Parsed {
            top_ids: vec![parsed_id],
            id2idx,
            inner: vec![(file_id, ast)],
            lib_override_matched: vec![false; lib_override],
        },
        Files {
            inner: vec![String::new()],
        },
    )
}

/// Parses the top files `file_ids` and the files they include, with the
/// default options: line 1 is parsed, and no `.lib` section is overridden.
pub async fn parse_top_multi<I: Iterator<Item = FileId>>(file_ids: I) -> (Parsed, Files) {
    parse_top_multi_with(file_ids, &ParseOptions::default()).await
}

/// Parses the top files `file_ids` and the files they include, with `opts`.
pub async fn parse_top_multi_with<I: Iterator<Item = FileId>>(
    file_ids: I,
    opts: &ParseOptions,
) -> (Parsed, Files) {
    let (manager, done_rx) = ParseManager::with_lib_override(&opts.lib_override);
    let title = opts.title_line;
    let handles: Vec<_> = file_ids
        .into_iter()
        .map(|file_id| {
            crate::spawn({
                let manager = manager.clone();
                async move { _include(manager, file_id, None, Some(title)).await }
            })
        })
        .collect();
    let mut top_ids = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await.unwrap() {
            Ok(parsed_id) => top_ids.push(parsed_id),
            Err((file_id, e)) => return error2parsed(file_id, e, opts.lib_override.len()),
        }
    }
    manager.wait(done_rx).await;
    let mut guard = manager.file_storage.lock().await;
    let file_storage: FileStorage<ASTBuilder> = mem::take(&mut *guard);
    (
        Parsed {
            top_ids,
            id2idx: file_storage.id2idx,
            inner: file_storage.parsed,
            lib_override_matched: manager.lib_override_matched(),
        },
        Files {
            inner: file_storage.file,
        },
    )
}

fn match_lib(text: &str, section: &str) -> Option<(String, u32)> {
    let section_escaped = regex::escape(section);
    let re = Regex::new(&format!(
        r"(?ims)^\s*\.lib\s+{section_escaped}\b(.*?)^\s*\.endl\b(?:\s+{section_escaped}\b)?"
    ))
    .unwrap();
    re.captures_iter(text).last().map(|caps| {
        let start_offset = caps.get(0).unwrap().start();
        let line_num_offset = (text[..start_offset].matches('\n').count()) as u32;
        (caps[1].to_owned(), line_num_offset)
    })
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
mod recover_tests {
    use super::*;

    /// A temp path that no other call in any test of this process uses: the
    /// tests run at the same time, and two of them can use one `tag`.
    pub(super) fn unique_temp_path(tag: &str, suffix: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("netlist_db_{tag}_{pid}_{n}{suffix}"))
    }

    /// Writes `text` to a temp file, parses it as a top file.
    pub(super) async fn parse_text(tag: &str, text: &str) -> (Parsed, Files) {
        let path = unique_temp_path(tag, ".sp");
        std::fs::write(&path, text).unwrap();
        let res = parse_top_multi([FileId::Include { path: path.clone() }].into_iter()).await;
        _ = std::fs::remove_file(&path);
        res
    }

    /// Every recorded error, including subckt bodies.
    pub(super) fn errors(parsed: &Parsed) -> Vec<&ParseError> {
        fn walk<'p>(ast: &'p ASTBuilder, out: &mut Vec<&'p ParseError>) {
            for seg in &ast.segments {
                if let Segment::Local(local) = seg {
                    out.extend(local.errors.iter());
                    for s in &local.subckt {
                        walk(&s.ast, out);
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

    /// `(line, byte offset)` of every recorded error, including subckt bodies.
    pub(super) fn error_pos(parsed: &Parsed) -> Vec<(u32, usize)> {
        errors(parsed)
            .into_iter()
            .map(|e| e.pos.map_or((0, usize::MAX), |p| (p.line_num, p.start)))
            .collect()
    }

    /// The message of every recorded error, including subckt bodies.
    pub(super) fn error_msgs(parsed: &Parsed) -> Vec<String> {
        errors(parsed)
            .into_iter()
            .map(|e| e.err.to_string())
            .collect()
    }

    /// Line numbers of every recorded error, including subckt bodies.
    pub(super) fn error_lines(parsed: &Parsed) -> Vec<u32> {
        error_pos(parsed)
            .into_iter()
            .map(|(line, _)| line)
            .collect()
    }

    pub(super) fn names(instances: &[crate::instance::Instance]) -> Vec<String> {
        instances.iter().map(|x| x.name.to_string()).collect()
    }

    #[tokio::test]
    async fn recover_bad_mosfet_keeps_neighbors() {
        let (parsed, files) = parse_text(
            "bad_mosfet",
            "R1 a b 1k\nM1 d g s b x y z w nch W=1u\nR2 c d 2k\n.end\n",
        )
        .await;
        assert_eq!(error_lines(&parsed), vec![2]);
        let (ast, has_err) = files.build(parsed);
        assert!(has_err);
        assert_eq!(names(&ast.instance), ["R1", "R2"]);
    }

    #[tokio::test]
    async fn recover_bad_dot_line() {
        let (parsed, files) = parse_text("bad_dot_line", ".param f(x=\nR2 a b 1\n").await;
        // at the start of the statement: line 1, column 1
        assert_eq!(error_pos(&parsed), vec![(1, 0)]);
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R2"]);
    }

    #[tokio::test]
    async fn recover_inside_subckt() {
        let (parsed, files) = parse_text(
            "inside_subckt",
            ".subckt sub a b\nR1 a b 1k\nM1 d g s b x y z w nch W=1u\nR2 a b 2k\n.ends sub\nR3 a b 3k\n.end\n",
        )
        .await;
        assert_eq!(error_lines(&parsed), vec![3]);
        let (ast, _) = files.build(parsed);
        let sub = ast.subckt.get("sub").expect("subckt kept");
        assert_eq!(names(&sub.ast.instance), ["R1", "R2"]);
        // `.ends` kept: R3 is top-level, not part of `sub`.
        assert_eq!(names(&ast.instance), ["R3"]);
    }

    #[tokio::test]
    async fn recover_skips_continuation_lines() {
        let (parsed, files) = parse_text(
            "continuation",
            "R1 a b 1k\nM1 d g s b x y z w nch\n* comment between continuation lines\n+ W=1u\n\n  + L=0.1u\nR2 c d 2k\n",
        )
        .await;
        assert_eq!(error_lines(&parsed), vec![2]);
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R1", "R2"]);
    }

    #[tokio::test]
    async fn recover_bad_subckt_header() {
        // A bad `.subckt` header drops that body (nested defs too), but its
        // `.ends` must not end the file.
        let (parsed, files) = parse_text(
            "bad_subckt_header",
            "R0 a b 1\n.subckt\n.subckt inner a b\nR1 a b 1k\n.ends inner\nR2 a b 2k\n.ends\nR3 a b 3k\n.end\n",
        )
        .await;
        assert_eq!(error_lines(&parsed), vec![2]);
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R0", "R3"]);
        assert!(ast.subckt.is_empty());
    }

    #[tokio::test]
    async fn recover_failure_on_continuation_line() {
        // The statement parses in part and stops at a bad `+` line. The
        // error is at the start of the statement, and neither the partial
        // record nor a record made from the `+` lines is kept.
        for text in [
            "M1 d g s b nch W=1u\n+ L=(bad\n+ M=2\nR2 a b 1\n",
            "X1 a b sub\n+ w=(1)\n+ l=2\nR2 a b 1\n",
        ] {
            let (parsed, files) = parse_text("continuation_fail", text).await;
            assert_eq!(error_pos(&parsed), vec![(1, 0)], "{text:?}");
            let (ast, _) = files.build(parsed);
            assert_eq!(names(&ast.instance), ["R2"], "{text:?}");
        }
    }

    #[tokio::test]
    async fn recover_orphan_continuation_line() {
        // `+` lines with no statement before them are one error.
        let (parsed, files) = parse_text("orphan_plus", "+ a b\n+ c\nR2 a b 1\n").await;
        assert_eq!(error_lines(&parsed), vec![1]);
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R2"]);
    }

    #[tokio::test]
    async fn recover_text_left_on_line() {
        // Text left on a `.subckt` header line fails the whole statement,
        // so the ports do not change and the body is not kept.
        let (parsed, files) = parse_text(
            "header_left",
            "R1 a b 1k\n.subckt sub a b w=(1)\nR2 a b 2k\n.ends sub\nR3 a b 3k\n",
        )
        .await;
        assert_eq!(error_lines(&parsed), vec![2]);
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R1", "R3"]);
        assert!(ast.subckt.is_empty());

        // Text left on a `.ends` line is an error, but the `.ends` still
        // ends the body.
        let (parsed, files) = parse_text(
            "ends_left",
            ".subckt sub a b\nR1 a b 1k\n.ends sub (x\n+ y\nR2 a b 2k\n",
        )
        .await;
        assert_eq!(error_lines(&parsed), vec![3]);
        let (ast, _) = files.build(parsed);
        let sub = ast.subckt.get("sub").expect("subckt kept");
        assert_eq!(names(&sub.ast.instance), ["R1"]);
        assert_eq!(names(&ast.instance), ["R2"]);
    }

    #[tokio::test]
    async fn recover_stray_ends() {
        // A `.ends` with no open `.subckt` is one error at its line. It does
        // not end the file.
        let (parsed, files) = parse_text(
            "stray_ends",
            "R0 a b 1\n*.subckt old a b\n*R1 a b 1\n.ends\nR3 a b 3\nR9 a b 9\n",
        )
        .await;
        assert_eq!(error_pos(&parsed), vec![(4, 36)]);
        let (ast, has_err) = files.build(parsed);
        assert!(has_err);
        assert_eq!(names(&ast.instance), ["R0", "R3", "R9"]);

        // A duplicated `.ends` (with a continuation line) after a good body.
        let (parsed, files) = parse_text(
            "dup_ends",
            ".subckt inv a b\nR1 a b 1\n.ends inv\n.ends inv\n+ x\nR2 a b 2\n",
        )
        .await;
        assert_eq!(error_lines(&parsed), vec![4]);
        let (ast, _) = files.build(parsed);
        let inv = ast.subckt.get("inv").expect("subckt kept");
        assert_eq!(names(&inv.ast.instance), ["R1"]);
        assert_eq!(names(&ast.instance), ["R2"]);
    }

    #[tokio::test]
    async fn recover_missing_ends() {
        // A body that reaches the end of input without `.ends` keeps the
        // rest of the input, with one error at its `.subckt` line.
        let (parsed, files) = parse_text(
            "missing_ends",
            "R0 a b 1\n.subckt inv a b\nR1 a b 1\nR9 a b 9",
        )
        .await;
        assert_eq!(error_pos(&parsed), vec![(2, 9)]);
        let (ast, has_err) = files.build(parsed);
        assert!(has_err);
        let inv = ast.subckt.get("inv").expect("subckt kept");
        assert_eq!(names(&inv.ast.instance), ["R1", "R9"]);
        assert_eq!(names(&ast.instance), ["R0"]);
    }

    #[tokio::test]
    async fn ends_name_chars() {
        // `.ends` reads its name with the same characters as `.subckt`.
        for name in ["inv-x1", "pm#1", "cell.a", "bus[0]"] {
            let text = format!(".subckt {name} a b\nR1 a b 1\n.ends {name}\nR2 a b 2\n");
            let (parsed, files) = parse_text("ends_name", &text).await;
            assert!(
                error_msgs(&parsed).is_empty(),
                "{name}: {:?}",
                error_msgs(&parsed)
            );
            let (ast, has_err) = files.build(parsed);
            assert!(!has_err, "{name}");
            let sub = ast.subckt.get(name).expect("subckt kept");
            assert_eq!(names(&sub.ast.instance), ["R1"], "{name}");
            assert_eq!(names(&ast.instance), ["R2"], "{name}");
        }
    }

    #[tokio::test]
    async fn binned_model_name() {
        let (parsed, files) = parse_text("binned_model", ".model nch.1 nmos level=54\n").await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let (ast, _) = files.build(parsed);
        assert_eq!(ast.model.len(), 1);
        assert_eq!(ast.model[0].name, "nch.1");
    }

    #[tokio::test]
    async fn skip_subckt_eof_fallback() {
        // A failed `.subckt` header with no `.ends` skips only its line.
        let (parsed, files) =
            parse_text("subckt_eof", "R0 a b 1\n.subckt\nR1 a b 1\nR9 a b 9\n").await;
        assert_eq!(error_lines(&parsed), vec![2]);
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R0", "R1", "R9"]);
        assert!(ast.subckt.is_empty());

        // A failed nested header with no `.ends` does not take the parent's
        // `.ends`.
        let (parsed, files) = parse_text(
            "nested_no_ends",
            ".subckt outer a b\nR1 a b 1\n.subckt\nR2 a b 2\n.ends outer\nR3 a b 3\n",
        )
        .await;
        assert_eq!(error_lines(&parsed), vec![3]);
        let (ast, _) = files.build(parsed);
        let outer = ast.subckt.get("outer").expect("subckt kept");
        assert_eq!(names(&outer.ast.instance), ["R1", "R2"]);
        assert_eq!(names(&ast.instance), ["R3"]);

        // A failed nested header with its own `.ends` skips its body only.
        let (parsed, files) = parse_text(
            "nested_own_ends",
            ".subckt outer a b\nR1 a b 1\n.subckt bad a b w=(1)\nR2 a b 2\n.ends bad\nR4 a b 4\n.ends outer\nR3 a b 3\n",
        )
        .await;
        assert_eq!(error_lines(&parsed), vec![3]);
        let (ast, _) = files.build(parsed);
        let outer = ast.subckt.get("outer").expect("subckt kept");
        assert_eq!(names(&outer.ast.instance), ["R1", "R4"]);
        assert!(outer.ast.subckt.is_empty());
        assert_eq!(names(&ast.instance), ["R3"]);

        // Two levels down: the grandparent's `.ends` does not make the
        // parent's `.ends` look like the failed header's.
        let (parsed, files) = parse_text(
            "nested_two_levels",
            ".subckt g a b\n.subckt p a b\n.subckt\nR2 a b 2\n.ends p\nR5 a b 5\n.ends g\nR3 a b 3\n",
        )
        .await;
        assert_eq!(error_lines(&parsed), vec![3]);
        let (ast, _) = files.build(parsed);
        let g = ast.subckt.get("g").expect("subckt kept");
        assert_eq!(names(&g.ast.instance), ["R5"]);
        let p = g.ast.subckt.get("p").expect("nested subckt kept");
        assert_eq!(names(&p.ast.instance), ["R2"]);
        assert_eq!(names(&ast.instance), ["R3"]);
    }

    #[tokio::test]
    async fn skip_subckt_ends_name() {
        // `.ends bad` names the failed header: it closes the failed block,
        // also when the parent has no `.ends` after it.
        let (parsed, files) = parse_text(
            "ends_names_failed",
            ".subckt outer a b\n.subckt bad a b w=(1)\nR2 a b 2\n.ends bad\nR4 a b 4\n",
        )
        .await;
        let mut lines = error_lines(&parsed);
        lines.sort_unstable();
        assert_eq!(lines, vec![1, 2]);
        assert!(error_msgs(&parsed).contains(&"`.subckt outer` has no `.ends`".to_owned()));
        let (ast, _) = files.build(parsed);
        let outer = ast.subckt.get("outer").expect("subckt kept");
        assert_eq!(names(&outer.ast.instance), ["R4"]);
        assert!(ast.instance.is_empty());

        // `.ends outer` names the parent: it is not the failed block's, so
        // the later `.ends` is a stray one.
        let (parsed, files) = parse_text(
            "ends_names_parent",
            ".subckt outer a b\n.subckt\nR2 a b 2\n.ends outer\nR3 a b 3\n.ends\nR4 a b 4\n",
        )
        .await;
        let mut lines = error_lines(&parsed);
        lines.sort_unstable();
        assert_eq!(lines, vec![2, 6]);
        assert!(
            error_msgs(&parsed)
                .contains(&"`.ends` without an open `.subckt` or `.macro`".to_owned())
        );
        let (ast, _) = files.build(parsed);
        let outer = ast.subckt.get("outer").expect("subckt kept");
        assert_eq!(names(&outer.ast.instance), ["R2"]);
        assert_eq!(names(&ast.instance), ["R3", "R4"]);
    }

    /// Skipping failed blocks stays linear in the file size: a failure does
    /// not scan the rest of the file again. The lines of each kind of block
    /// are found in one pass, and each skip after that is a look-up.
    /// The test counts the passes, not the time, so a busy machine cannot
    /// make it fail. (`#[tokio::test]` runs the parse on this thread.)
    #[tokio::test]
    async fn failed_blocks_linear() {
        const K: usize = 2000;
        let filler = "R1 a b 1\nR2 a b 2\nR3 a b 3\n";
        // failed nested headers, each with its own `.ends` (no name, so the
        // `.ends` that follow it decide)
        let nested = format!(
            ".subckt outer a b\n{}.ends outer\n",
            format!(".subckt bad a b w=(1)\n{filler}.ends\n").repeat(K)
        );
        // failed headers with no `.ends`
        let no_ends = format!(".subckt\n{filler}").repeat(K);
        // `.data` with no `.enddata`
        let no_enddata = format!(".data d\n+ a b\n+ 1 x(\n{filler}").repeat(K);
        // failed `.data` with `.enddata` in a body (the skip also finds the
        // `.subckt` and `.ends` lines, to keep the body's `.ends`)
        let data_in_body = format!(
            ".subckt outer a b\n{}.ends outer\n",
            format!(".data d\n+ a b\n+ 1 x(\n{filler}.enddata\n").repeat(K)
        );
        for (tag, text, scans) in [
            ("linear_nested", nested, 1),
            ("linear_no_ends", no_ends, 1),
            ("linear_no_enddata", no_enddata, 1),
            ("linear_data_in_body", data_in_body, 2),
        ] {
            let before = utils::BLOCK_SCANS.with(|n| n.get());
            let (parsed, _) = parse_text(tag, &text).await;
            let after = utils::BLOCK_SCANS.with(|n| n.get());
            assert_eq!(errors(&parsed).len(), K, "{tag}");
            // A skip that scans the rest of the file again for each failure
            // makes `K` passes (in a debug build it took 18 s here).
            assert_eq!(after - before, scans, "{tag}");
        }
    }

    #[tokio::test]
    async fn failed_block_keeps_parent_ends() {
        // A failed `.data` in a body does not take a stray `.enddata` after
        // the body's `.ends`: only its line is skipped, and `.ends inv`
        // still ends `inv`.
        let (parsed, files) = parse_text(
            "data_past_ends",
            ".subckt inv a b\n.data d\n+ p q\n+ 1 x(\n.ends inv\nR9 a b 9\n.enddata\nR10 a b 10\n",
        )
        .await;
        let mut lines = error_lines(&parsed);
        lines.sort_unstable();
        assert_eq!(lines, vec![2, 7]);
        let (ast, _) = files.build(parsed);
        let inv = ast.subckt.get("inv").expect("subckt kept");
        assert!(inv.ast.instance.is_empty());
        assert_eq!(names(&ast.instance), ["R9", "R10"]);

        // The same `.data` at file level has no `.ends` to keep: its skip
        // goes to the `.enddata`.
        let (parsed, files) = parse_text(
            "data_top_enddata",
            ".data d\n+ p q\n+ 1 x(\nR1 a b 1\n.enddata\nR10 a b 10\n",
        )
        .await;
        assert_eq!(error_lines(&parsed), vec![1]);
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R10"]);
    }

    #[tokio::test]
    async fn failed_block_one_error() {
        for (tag, text) in [
            // `.data` with a bad value
            (
                "data_bad",
                "R0 a b 1\n.data d\n+ a b\n+ 1 x(\n.enddata\nR9 a b 9\n",
            ),
            // `.lib` section whose `.endl` line has extra text
            (
                "lib_endl",
                "R0 a b 1\n.lib tt\nR1 a b 1\n.endl tt junk\nR9 a b 9\n",
            ),
        ] {
            let (parsed, files) = parse_text(tag, text).await;
            assert_eq!(error_lines(&parsed), vec![2], "{tag}");
            let (ast, _) = files.build(parsed);
            assert_eq!(names(&ast.instance), ["R0", "R9"], "{tag}");
            assert!(ast.data.is_empty(), "{tag}");
            assert!(ast.unknwon.is_empty(), "{tag}");
        }

        // A `.lib` section with no `.endl` skips only its line.
        let (parsed, files) = parse_text("lib_no_endl", "R0 a b 1\n.lib tt\nR1 a b 1\n").await;
        assert_eq!(error_lines(&parsed), vec![2]);
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R0", "R1"]);
    }

    #[tokio::test]
    async fn clear_error_text() {
        for (tag, text, msg) in [
            (
                "clear_stray_ends",
                "R0 a b 1\n.ends\n",
                "`.ends` without an open `.subckt` or `.macro`",
            ),
            (
                "clear_missing_ends",
                ".subckt inv a b\nR1 a b 1\n",
                "`.subckt inv` has no `.ends`",
            ),
            // `.macro`/`.eom` errors name their own keywords
            (
                "clear_stray_eom",
                "R0 a b 1\n.EOM\n",
                "`.eom` without an open `.subckt` or `.macro`",
            ),
            (
                "clear_missing_eom",
                ".Macro inv a b\nR1 a b 1\n",
                "`.macro inv` has no `.eom`",
            ),
            (
                "trailing_text",
                ".option a=1 (\n",
                "text after the end of the statement",
            ),
            (
                "trailing_ends",
                ".subckt inv a b\nR1 a b 1\n.ends inv x\n",
                "text after the end of the statement",
            ),
        ] {
            let (parsed, _) = parse_text(tag, text).await;
            assert_eq!(error_msgs(&parsed), [msg], "{tag}");
        }
    }

    /// A temp dir that is removed when it is dropped, also when a test fails.
    pub(super) struct TempDir(pub(super) PathBuf);

    impl TempDir {
        pub(super) fn new(tag: &str) -> Self {
            let dir = unique_temp_path(tag, "");
            _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn include_quotes_stripped() {
        let tmp = TempDir::new("incq");
        let dir = &tmp.0;
        std::fs::write(dir.join("f.sp"), "R1 a b 1\n").unwrap();
        std::fs::write(dir.join("l.sp"), ".lib tt\nR1 a b 1\n.endl tt\n").unwrap();
        for (k, line) in [
            ".include \"f.sp\"",
            ".include 'f.sp'",
            ".inc \"f.sp\"",
            ".lib \"l.sp\" tt",
            ".lib 'l.sp' tt",
        ]
        .into_iter()
        .enumerate()
        {
            let top = dir.join(format!("top{k}.sp"));
            std::fs::write(&top, format!("{line}\n")).unwrap();
            let (parsed, files) =
                parse_top_multi([FileId::Include { path: top }].into_iter()).await;
            let (ast, has_err) = files.build(parsed);
            assert!(!has_err, "{line}");
            assert_eq!(names(&ast.instance), ["R1"], "{line}");
        }
    }

    #[tokio::test]
    async fn data_file_quotes_stripped() {
        for quote in ['\'', '"'] {
            let text = format!(
                ".data d mer\n+ file={quote}a.txt{quote} p1=1\n+ out={quote}o.txt{quote}\n.enddata\n"
            );
            let (parsed, files) = parse_text("data_quotes", &text).await;
            assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
            let (ast, _) = files.build(parsed);
            let crate::ast::DataValues::MER(data_files) = &ast.data[0].values else {
                panic!("{quote}: {:?}", ast.data[0].values);
            };
            assert_eq!(data_files.files[0].file, "a.txt", "{quote}");
            assert_eq!(data_files.out.as_deref(), Some("o.txt"), "{quote}");
        }
    }

    #[test]
    fn io_error_text() {
        let e = ParseErrorInner::IO(std::io::Error::other("cannot read deck.sp"));
        assert_eq!(e.to_string(), "cannot read deck.sp");
    }

    #[tokio::test]
    async fn bom_skipped() {
        let (parsed, files) = parse_text(
            "bom",
            "\u{FEFF}R1 a b 1\n.subckt inv a b\nR2 a b 2\n.ends inv\n",
        )
        .await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        assert!(!files.inner[0].starts_with('\u{FEFF}'));
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R1"]);
        assert!(ast.subckt.contains_key("inv"));
    }

    #[tokio::test]
    async fn ic_only_segment_kept() {
        let (parsed, files) = parse_text("ic_only", ".ic v(a)=1\n").await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let (ast, _) = files.build(parsed);
        assert_eq!(ast.init_condition.len(), 1);

        let (parsed, files) = parse_text("nodeset_only", ".nodeset a=1\n").await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let (ast, _) = files.build(parsed);
        assert_eq!(ast.nodeset.len(), 1);
    }
}

/// Statements and parse options (Task 5 of the plan): `.param` functions,
/// `.global`, `.prot` regions, `.alter`, the title line, `.lib` overrides
/// and analysis commands.
#[cfg(test)]
mod statement_tests {
    use super::recover_tests::{TempDir, error_lines, error_msgs, errors, names, parse_text};
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

    fn opts(title_line: TitleLine) -> ParseOptions {
        ParseOptions {
            title_line,
            lib_override: Vec::new(),
        }
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
                Segment::Alter { index, pos } => {
                    format!("alter#{index}@{}", pos.map_or(0, |p| p.line_num))
                }
                Segment::Include { .. } => "include".to_owned(),
            })
            .collect();
        assert_eq!(
            kinds,
            ["local:1", "alter#1@2", "local:1", "alter#2@4", "local:1"]
        );
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err);
        assert_eq!(ast.alter_runs, 2);
        assert_eq!(ast.param.len(), 2);

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
        // `parse_top_multi` keeps line 1, as before.
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
            .unknwon
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
}

/// Follow-ups of Task 5a (5a-P): `.lib` override triples, `.endl` in
/// comments, deep `.subckt` nesting, `.macro`/`.eom`, and the title line.
#[cfg(test)]
mod follow_up_tests {
    use super::recover_tests::{TempDir, error_lines, error_msgs, errors, names, parse_text};
    use super::*;
    use crate::{AST, LibOverride, ParseOptions, TitleLine, ast::Value};
    use std::{collections::BTreeMap, path::Path};

    /// The top-level `.param` values of `ast`, by name.
    fn params(ast: &AST) -> BTreeMap<String, f64> {
        ast.param
            .iter()
            .map(|kv| match kv.v {
                Value::Num(f) => (kv.k.to_string(), f),
                Value::Expr(ref e) => panic!("{}: {e}", kv.k),
            })
            .collect()
    }

    /// Parses the top file `path` with `title_line` and `lib_override`
    /// (`(file, from, to)`).
    async fn parse_path(
        path: &Path,
        title_line: TitleLine,
        lib_override: &[(&str, &str, &str)],
    ) -> (Parsed, Files) {
        let opts = ParseOptions {
            title_line,
            lib_override: lib_override
                .iter()
                .map(|(file, from, to)| LibOverride {
                    file: PathBuf::from(file),
                    from: from.to_string(),
                    to: to.to_string(),
                })
                .collect(),
        };
        parse_top_multi_with([FileId::Include { path: path.into() }].into_iter(), &opts).await
    }

    #[tokio::test]
    async fn lib_override_triples() {
        let tmp = TempDir::new("lib_triples");
        let dir = &tmp.0;
        std::fs::write(
            dir.join("models.lib"),
            "\
* corners
.lib tt
.param rcorner=1k
.lib 'models.lib' common
.endl tt
.lib ff
.param rcorner=2k
.lib 'models.lib' common
.endl ff
.lib res_tt
.param rres=10
.endl res_tt
.lib res_ff
.param rres=20
.endl res_ff
.lib common
.param rcommon=1
.endl common
.lib common2
.param rcommon=2
.endl common2
",
        )
        .unwrap();
        let deck = dir.join("deck.sp");
        std::fs::write(
            &deck,
            "* title\n.lib 'models.lib' tt\n.lib 'models.lib' res_tt\n",
        )
        .unwrap();
        let p = |rcorner: f64, rres: f64, rcommon: f64| {
            BTreeMap::from([
                ("rcommon".to_owned(), rcommon),
                ("rcorner".to_owned(), rcorner),
                ("rres".to_owned(), rres),
            ])
        };
        for (overrides, want, matched) in [
            // the deck's own sections
            (&[][..], p(1e3, 10.0, 1.0), &[][..]),
            // only the `tt` call changes: `ff` and `res_tt` load
            (&[("models.lib", "tt", "ff")], p(2e3, 10.0, 1.0), &[true]),
            // each call is switched on its own; names in any letter case,
            // the file without its directory
            (
                &[
                    ("/elsewhere/MODELS.LIB", "TT", "FF"),
                    ("models.lib", "res_tt", "res_ff"),
                ],
                p(2e3, 20.0, 1.0),
                &[true, true],
            ),
            // no call to `res_ff` or to `other.lib`: unused
            (
                &[
                    ("models.lib", "res_ff", "res_tt"),
                    ("other.lib", "tt", "ff"),
                ],
                p(1e3, 10.0, 1.0),
                &[false, false],
            ),
            // a call made inside the library itself changes too
            (
                &[("models.lib", "common", "common2")],
                p(1e3, 10.0, 2.0),
                &[true],
            ),
        ] {
            let (parsed, files) = parse_path(&deck, TitleLine::Auto, overrides).await;
            assert!(
                error_msgs(&parsed).is_empty(),
                "{overrides:?}: {:?}",
                error_msgs(&parsed)
            );
            assert_eq!(parsed.lib_override_matched, matched, "{overrides:?}");
            let (ast, has_err) = files.build(parsed);
            assert!(!has_err, "{overrides:?}");
            assert_eq!(params(&ast), want, "{overrides:?}");
        }
    }

    #[tokio::test]
    async fn endl_in_comment() {
        // A `.lib` section in a parsed file ends at the line whose first
        // token is `.endl`, not at `.endl` in a comment or in a longer word.
        let (parsed, files) = parse_text(
            "endl_comment",
            "R0 a b 1\n.lib tt\nR1 a b 1\n* the section ends at .endl below\nR2 a b 2 $ not .endl here\n.endlx\n  .ENDL tt\nR3 a b 3\n",
        )
        .await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R0", "R3"]);

        // A section that `.lib '<file>' <section>` loads (`match_lib`). The
        // `.endlx` line does not end it: it is an unknown command, one error
        // at its line, and `R3` after it is in the section.
        let tmp = TempDir::new("endl_comment_lib");
        let dir = &tmp.0;
        std::fs::write(
            dir.join("l.sp"),
            ".lib tt\nR1 a b 1\n* old .endl tt\nR2 a b 2 $ .endl\n.endlx\nR3 a b 3\n.endl tt\n.lib ff\nR9 a b 9\n.endl ff\n",
        )
        .unwrap();
        let top = dir.join("top.sp");
        std::fs::write(&top, ".lib 'l.sp' tt\n").unwrap();
        let (parsed, files) = parse_path(&top, TitleLine::No, &[]).await;
        assert_eq!(error_lines(&parsed), [5], "{:?}", error_msgs(&parsed));
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R1", "R2", "R3"]);
    }

    /// `depth` nested `.subckt s<k>` headers, each with a resistor `R<k>`,
    /// then (with `ends`) their `.ends` and a top-level `R_top`.
    fn nested_deck(depth: usize, ends: bool) -> String {
        let mut s = String::new();
        for k in 0..depth {
            s.push_str(&format!(".subckt s{k} a b\nR{k} a b 1\n"));
        }
        if ends {
            for k in (0..depth).rev() {
                s.push_str(&format!(".ends s{k}\n"));
            }
            s.push_str("R_top a b 1\n");
        }
        s
    }

    /// The number of `NestingTooDeep` errors, and their lines.
    fn too_deep(parsed: &Parsed) -> Vec<u32> {
        errors(parsed)
            .into_iter()
            .filter(|e| matches!(e.err, ParseErrorInner::NestingTooDeep { .. }))
            .map(|e| e.pos.map_or(0, |p| p.line_num))
            .collect()
    }

    /// The depth of the `.subckt` chain `s0 > s1 > …` in `ast`.
    fn chain_depth(ast: &AST) -> usize {
        let mut depth = 0;
        let mut ast = ast;
        while let Some(s) = ast.subckt.get(&format!("s{depth}")) {
            assert_eq!(names(&s.ast.instance), [format!("R{depth}")]);
            ast = &s.ast;
            depth += 1;
        }
        depth
    }

    /// Runs on the default test thread (`#[tokio::test]`), so its stack
    /// is the default one.
    #[tokio::test]
    async fn deep_nesting_no_crash() {
        // 256 levels are allowed.
        let (parsed, files) = parse_text("nest_256", &nested_deck(256, true)).await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err);
        assert_eq!(chain_depth(&ast), 256);
        assert_eq!(names(&ast.instance), ["R_top"]);

        // Level 257 is one error at its header, and its body is skipped up
        // to its `.ends`. The levels above it keep the rest.
        let (parsed, files) = parse_text("nest_257", &nested_deck(257, true)).await;
        assert_eq!(too_deep(&parsed), [2 * 256 + 1]);
        assert_eq!(
            error_msgs(&parsed),
            ["`.subckt s256` is nested deeper than 256 levels: its definition is skipped"]
        );
        let (ast, _) = files.build(parsed);
        assert_eq!(chain_depth(&ast), 256);
        assert_eq!(names(&ast.instance), ["R_top"]);

        // 5,000 headers without `.ends`: one `NestingTooDeep`, and the parse
        // ends.
        let (parsed, _) = parse_text("nest_5000", &nested_deck(5000, false)).await;
        assert_eq!(too_deep(&parsed), [2 * 256 + 1]);
        let missing = errors(&parsed)
            .into_iter()
            .filter(|e| matches!(e.err, ParseErrorInner::MissingEnds { .. }))
            .count();
        assert_eq!(missing, 256);
    }

    /// Writes `dir/<tag>.sp`: `levels` nested `.subckt <tag><k>` headers,
    /// each with a resistor `R<tag><k>`, the line `inner` in the innermost
    /// body, the `.ends` lines, and a top-level `R<tag>_top`.
    fn nested_file(dir: &Path, tag: &str, levels: usize, inner: &str) {
        let mut s = String::new();
        for k in 0..levels {
            s.push_str(&format!(".subckt {tag}{k} a b\nR{tag}{k} a b 1\n"));
        }
        s.push_str(inner);
        for k in (0..levels).rev() {
            s.push_str(&format!(".ends {tag}{k}\n"));
        }
        s.push_str(&format!("R{tag}_top a b 1\n"));
        std::fs::write(dir.join(format!("{tag}.sp")), s).unwrap();
    }

    /// The depth of the first `.subckt` chain in `ast`, and the instances of
    /// its innermost body.
    fn first_chain(ast: &AST) -> (usize, Vec<String>) {
        let mut depth = 0;
        let mut ast = ast;
        while let Some((_, s)) = ast.subckt.first() {
            ast = &s.ast;
            depth += 1;
        }
        (depth, names(&ast.instance))
    }

    /// Runs on the default test thread (`#[tokio::test]`).
    #[tokio::test]
    async fn nesting_counts_across_includes() {
        // An `.include` in a body puts the file in that body (golden
        // `include_in_subckt`), so the depth adds up along the include
        // chain. Each file is within the limit, so the parse has no error.
        let tmp = TempDir::new("nest_include");
        let dir = &tmp.0;

        // 100 + 100 levels: within the limit.
        nested_file(dir, "a", 100, ".include 'b.sp'\n");
        nested_file(dir, "b", 100, "");
        let (parsed, files) = parse_path(&dir.join("a.sp"), TitleLine::No, &[]).await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err);
        assert_eq!(first_chain(&ast), (200, vec!["Rb99".to_owned()]));

        // 256 + 256 + 256 levels (a includes b, b includes c): `b0` is level
        // 257, one `NestingTooDeep` error at build time, and its definition
        // is skipped. The statements of `b.sp` outside its definitions stay
        // in the body of `a255`, and `c.sp` is not reached.
        nested_file(dir, "a", 256, ".include 'b.sp'\n");
        nested_file(dir, "b", 256, ".include 'c.sp'\n");
        nested_file(dir, "c", 256, "");
        let (parsed, files) = parse_path(&dir.join("a.sp"), TitleLine::No, &[]).await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let (ast, has_err) = files.build(parsed);
        assert!(has_err);
        assert_eq!(
            first_chain(&ast),
            (256, vec!["Ra255".to_owned(), "Rb_top".to_owned()])
        );
        assert_eq!(names(&ast.instance), ["Ra_top"]);
    }

    #[tokio::test]
    async fn macro_eom_synonyms() {
        // Golden `macro_eom`.
        let (parsed, files) = parse_path(
            Path::new("golden/cases/macro_eom/deck.sp"),
            TitleLine::Auto,
            &[],
        )
        .await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err);
        let inv = ast.subckt.get("inv").expect("macro kept");
        assert_eq!(inv.ports, ["a", "b"]);
        assert_eq!(inv.params.len(), 1);
        assert_eq!(names(&inv.ast.instance), ["r1"]);
        assert_eq!(names(&ast.instance), ["v1", "v2", "x1", "x2"]);

        // The keywords mix and nest, in any letter case.
        let (parsed, files) = parse_text(
            "macro_mix",
            ".MACRO m1 a b\nR1 a b 1\n.subckt inner a b\nR2 a b 2\n.EOM inner\n.ends m1\nR3 a b 3\n",
        )
        .await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let (ast, _) = files.build(parsed);
        let m1 = ast.subckt.get("m1").expect("macro kept");
        assert_eq!(names(&m1.ast.instance), ["R1"]);
        let inner = m1.ast.subckt.get("inner").expect("nested kept");
        assert_eq!(names(&inner.ast.instance), ["R2"]);
        assert_eq!(names(&ast.instance), ["R3"]);

        // A stray `.eom` is an error, and a failed `.macro` header skips its
        // body up to its `.eom`.
        let (parsed, files) = parse_text(
            "macro_errors",
            "R0 a b 1\n.eom\n.macro bad a b w=(1)\nR1 a b 1\n.eom\nR2 a b 2\n",
        )
        .await;
        assert_eq!(error_lines(&parsed), [2, 3]);
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R0", "R2"]);
        assert!(ast.subckt.is_empty());
    }

    #[tokio::test]
    async fn title_auto_prot() {
        // `.prot`/`.protect` on line 1: an encrypted library, so line 1 is
        // parsed.
        for line1 in [".prot", ".PROTECT", "  .protect x"] {
            let tmp = TempDir::new("title_prot");
            let path = tmp.0.join("deck.sp");
            std::fs::write(&path, format!("{line1}\njunk ((\n.unprot\nR1 a b 1\n")).unwrap();
            let (parsed, files) = parse_path(&path, TitleLine::Auto, &[]).await;
            assert!(error_msgs(&parsed).is_empty(), "{line1}");
            let top = &parsed.inner[parsed.top_ids[0].0].1;
            let title = top.title.as_ref().expect("title info");
            assert!(!title.skipped, "{line1}");
            let (ast, _) = files.build(parsed);
            assert_eq!(ast.protected, [(1, 3)], "{line1}");
            assert_eq!(names(&ast.instance), ["R1"], "{line1}");
        }

        // Golden `title_plus`, `title_plus_words`, `title_plus_comment`: a
        // `+` line right after the title line is one error at line 2, and
        // the rest of the deck is parsed.
        for case in ["title_plus", "title_plus_words", "title_plus_comment"] {
            for mode in [TitleLine::Auto, TitleLine::Yes] {
                let path = PathBuf::from(format!("golden/cases/{case}/deck.sp"));
                let (parsed, files) = parse_path(&path, mode, &[]).await;
                assert_eq!(error_lines(&parsed), [2], "{case}");
                assert_eq!(
                    error_msgs(&parsed),
                    ["a `+` line right after the title line: HSPICE rejects the deck"],
                    "{case}"
                );
                let (ast, _) = files.build(parsed);
                assert_eq!(names(&ast.instance), ["v1", "r2"], "{case}");
            }
        }
        // When line 1 is parsed, a `+` line continues it.
        let tmp = TempDir::new("title_plus_parsed");
        let path = tmp.0.join("deck.sp");
        for (text, mode) in [
            ("R1 a b 1\n+ m=2\nR2 a b 2\n", TitleLine::No),
            (
                ".subckt inv a b\n+ c\nR1 a b 1\n.ends\nR2 a b 2\n",
                TitleLine::Auto,
            ),
        ] {
            std::fs::write(&path, text).unwrap();
            let (parsed, files) = parse_path(&path, mode, &[]).await;
            assert!(error_msgs(&parsed).is_empty(), "{text:?}");
            let (ast, _) = files.build(parsed);
            assert_eq!(names(&ast.instance).last().map(String::as_str), Some("R2"));
        }
    }

    #[tokio::test]
    async fn title_plus_gap() {
        // Golden `title_plus_gap`: a `*` line between line 1 and the `+`
        // line does not change it (HSPICE gives the error at line 3).
        let path = PathBuf::from("golden/cases/title_plus_gap/deck.sp");
        let (parsed, files) = parse_path(&path, TitleLine::Auto, &[]).await;
        assert_eq!(error_lines(&parsed), [3]);
        assert_eq!(
            error_msgs(&parsed),
            ["a `+` line right after the title line: HSPICE rejects the deck"]
        );
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["v1", "r2"]);
        // Also after a comment or an empty line.
        let tmp = TempDir::new("title_plus_gap");
        let path = tmp.0.join("deck.sp");
        std::fs::write(&path, "title\n* c\n\n+ r9 1 0 9\n+ x\nR1 a b 1\n").unwrap();
        let (parsed, files) = parse_path(&path, TitleLine::Auto, &[]).await;
        assert_eq!(error_lines(&parsed), [4]);
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R1"]);
        // Also a `+` line with only a comment, after one or more `*` and
        // `$` lines, or right after the title line (one error each time).
        for (text, line) in [
            ("title\n* gap\n+ $ c\nR1 a b 1\n", 3),
            ("title\n* gap\n$ gap\n  + $ c\n+ $ d\nR1 a b 1\n", 4),
            ("title\n+ $ c\nR1 a b 1\n", 2),
            ("title\n* gap\n+", 3),
        ] {
            std::fs::write(&path, text).unwrap();
            let (parsed, files) = parse_path(&path, TitleLine::Auto, &[]).await;
            assert_eq!(error_lines(&parsed), [line], "{text:?}");
            let (ast, _) = files.build(parsed);
            let want: &[&str] = if text.contains("R1") { &["R1"] } else { &[] };
            assert_eq!(names(&ast.instance), want, "{text:?}");
        }
    }
}

/// Task 6a-P: the lines after `.end`, and files that are not valid UTF-8.
#[cfg(test)]
mod end_and_utf8_tests {
    use super::recover_tests::{TempDir, error_msgs, names};
    use super::*;
    use crate::{ParseOptions, TitleLine, ast::Value};
    use std::path::Path;

    async fn parse_path(path: &Path, title_line: TitleLine) -> (Parsed, Files) {
        let opts = ParseOptions {
            title_line,
            lib_override: Vec::new(),
        };
        parse_top_multi_with([FileId::Include { path: path.into() }].into_iter(), &opts).await
    }

    fn top(parsed: &Parsed) -> &ASTBuilder {
        &parsed.inner[parsed.top_ids[0].0].1
    }

    #[tokio::test]
    async fn after_end() {
        // Golden `after_end`: the lines after `.end` of the top file are
        // not read (r1 uses p=1k), and HSPICE gives no warning.
        let path = PathBuf::from("golden/cases/after_end/deck.sp");
        let (parsed, files) = parse_path(&path, TitleLine::Auto).await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let end = top(&parsed).after_end.expect("after_end");
        assert_eq!((end.line, end.lines), (6, 3));
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["v1", "r1"]);
        assert_eq!(ast.param.len(), 1);
        assert!(matches!(ast.param[0].v, Value::Num(f) if f == 1e3));

        let tmp = TempDir::new("after_end");
        let dir = &tmp.0;
        let main = dir.join("main.sp");
        for (text, want, end) in [
            // only comments and empty lines after `.end`
            (
                "* t\nR1 a b 1\n.END\n* c\n\n  $ d\n",
                &["R1"][..],
                Some((3, 0)),
            ),
            // `.end` with a name, and no newline at the end
            ("* t\nR1 a b 1\n.end x\nR2 a b 2", &["R1"], Some((3, 1))),
            // an open `.subckt` is closed at `.end`
            (
                "* t\n.subckt s a\nR1 a b 1\n.end\nR2 a b 2\n.ends\n",
                &[],
                Some((4, 2)),
            ),
            // no `.end`
            ("* t\nR1 a b 1\n", &["R1"], None),
            // a `.lib` section after `.end` is read by its `.lib` calls,
            // so its lines are not counted (`.LIB` with a `$` comment too)
            (
                "* t\n.lib 'main.sp' tt\n.end\n.lib tt\n.param p=1\n.endl tt\nR2 a b 2\n.LIB ff $ c\nR3 a b 3\n.ENDL\n",
                &[],
                Some((3, 1)),
            ),
        ] {
            std::fs::write(&main, text).unwrap();
            let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
            let got = top(&parsed).after_end.map(|e| (e.line, e.lines));
            assert_eq!(got, end, "{text:?}");
            let (ast, _) = files.build(parsed);
            assert_eq!(names(&ast.instance), want, "{text:?}");
        }
        // An open `.subckt` at `.end` has no `.ends`.
        std::fs::write(&main, "* t\n.subckt s a\nR1 a b 1\n.end\nR2 a b 2\n.ends\n").unwrap();
        let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
        assert_eq!(error_msgs(&parsed), ["`.subckt s` has no `.ends`"]);
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.subckt["s"].ast.instance), ["R1"]);

        // `.end` in an included file does not end the top file, and the
        // included file is read past it.
        std::fs::write(dir.join("inc.sp"), "R1 a b 1\n.end\nR2 a b 2\n").unwrap();
        std::fs::write(&main, "* t\n.inc 'inc.sp'\nR3 a b 3\n.end\nR4 a b 4\n").unwrap();
        let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let inc = parsed
            .inner
            .iter()
            .find(|(id, _)| id.path().ends_with("inc.sp"))
            .unwrap();
        assert!(inc.1.after_end.is_none());
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R1", "R2", "R3"]);
    }

    #[tokio::test]
    async fn non_utf8() {
        // Golden `nonutf8_comment`: a Latin-1 byte in a comment; HSPICE
        // reads the deck with no warning.
        let path = PathBuf::from("golden/cases/nonutf8_comment/deck.sp");
        let (parsed, files) = parse_path(&path, TitleLine::Auto).await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        assert_eq!(top(&parsed).non_utf8, Some(2));
        assert!(files.inner[parsed.top_ids[0].0].contains("caf\u{FFFD} "));
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["v1", "r1"]);

        // A bad byte in a statement: the statement is kept, with U+FFFD.
        let tmp = TempDir::new("non_utf8");
        let dir = &tmp.0;
        let main = dir.join("main.sp");
        std::fs::write(&main, b"* t\nR1 a b 1\nR\xe92 a b 2\n.inc 'inc.sp'\n").unwrap();
        std::fs::write(dir.join("inc.sp"), b"R3 a b 3\n\n* \xff\xfe\nR4 a b 4\n").unwrap();
        let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        assert_eq!(top(&parsed).non_utf8, Some(3));
        let inc = parsed
            .inner
            .iter()
            .find(|(id, _)| id.path().ends_with("inc.sp"))
            .unwrap();
        assert_eq!(inc.1.non_utf8, Some(3));
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R1", "R\u{FFFD}2", "R3", "R4"]);

        // A `.lib` section: the first bad line in the section, and none
        // when the bad bytes are outside it.
        std::fs::write(
            dir.join("m.lib"),
            b"* \xe9\n.lib tt\nR5 a b 5\n.endl tt\n.lib ff\nR6 a b 6\n* \xe9\nR\xe97 a b 7\n.endl ff\n",
        )
        .unwrap();
        std::fs::write(&main, "* t\n.lib 'm.lib' tt\n.lib 'm.lib' ff\n").unwrap();
        let (parsed, _) = parse_path(&main, TitleLine::Auto).await;
        let section = |name: &str| {
            parsed
                .inner
                .iter()
                .find(|(id, _)| matches!(id, FileId::Section { section, .. } if section == name))
                .map(|(_, ast)| ast.non_utf8)
                .unwrap()
        };
        assert_eq!(section("tt"), None);
        assert_eq!(section("ff"), Some(7));

        // Valid UTF-8 has no mark.
        std::fs::write(&main, "* t\nR1 a b 1 $ caf\u{e9}\n").unwrap();
        let (parsed, _) = parse_path(&main, TitleLine::Auto).await;
        assert_eq!(top(&parsed).non_utf8, None);
    }

    #[tokio::test]
    async fn include_cycles() {
        // a -> b -> c -> a, and the top file includes `a` and `c`. The parse
        // reads each file once, so which include line closed the cycle (a
        // parse error) depended on the order in which the files were read.
        // Now no include is a parse error, and `Files::build` finds the
        // cycle in document order: the same result in every run.
        let tmp = TempDir::new("include_cycles");
        let dir = &tmp.0;
        for (name, text) in [
            ("main.sp", "* t\n.inc 'a.sp'\n.inc 'c.sp'\n"),
            ("a.sp", ".inc 'b.sp'\nra 1 0 1\n"),
            ("b.sp", ".inc 'c.sp'\nrb 1 0 1\n"),
            ("c.sp", ".inc 'a.sp'\nrc 1 0 1\n"),
        ] {
            std::fs::write(dir.join(name), text).unwrap();
        }
        for _ in 0..20 {
            let (parsed, files) = parse_path(&dir.join("main.sp"), TitleLine::Auto).await;
            assert_eq!(parsed.inner.len(), 4);
            let include_errors = parsed
                .inner
                .iter()
                .flat_map(|(_, ast)| &ast.segments)
                .filter(|seg| {
                    matches!(seg, Segment::Include { res, .. }
                        if res.get().is_none_or(Result::is_err))
                })
                .count();
            assert_eq!(include_errors, 0);
            let (ast, has_err) = files.build(parsed);
            assert!(has_err);
            // main -> a -> b -> c (its include of `a` closes a cycle), then
            // main -> c -> a -> b (its include of `c` closes one)
            assert_eq!(names(&ast.instance), ["rc", "rb", "ra", "rb", "ra", "rc"]);
        }
        // A file that includes itself.
        let main = dir.join("self.sp");
        std::fs::write(&main, "* t\nR1 a b 1\n.inc 'self.sp'\nR2 a b 2\n").unwrap();
        let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
        assert!(error_msgs(&parsed).is_empty());
        let (ast, has_err) = files.build(parsed);
        assert!(has_err);
        assert_eq!(names(&ast.instance), ["R1", "R2"]);
    }

    #[tokio::test]
    async fn include_cycle_canonical_paths() {
        // `main.sp` -> `sub/b.sp` -> `../main.sp`: the path is spelled
        // another way, but it is the same file, so it is the same parse and
        // the include closes a cycle (each spelling was a new file before,
        // and the paths grew until "File name too long").
        let tmp = TempDir::new("cycle_canonical");
        let dir = &tmp.0;
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("main.sp"), "* t\n.inc 'sub/b.sp'\nR1 a b 1\n").unwrap();
        std::fs::write(dir.join("sub/b.sp"), "R2 a b 2\n.inc '../main.sp'\n").unwrap();
        let (parsed, files) = parse_path(&dir.join("main.sp"), TitleLine::Auto).await;
        assert_eq!(parsed.inner.len(), 2);
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let (ast, has_err) = files.build(parsed);
        assert!(has_err);
        assert_eq!(names(&ast.instance), ["R2", "R1"]);
        // Two spellings of one file that is not in a cycle: one parse,
        // read at each include (HSPICE reads a file again at each
        // `.include`).
        std::fs::write(dir.join("x.sp"), "R3 a b 3\n").unwrap();
        std::fs::write(
            dir.join("two.sp"),
            "* t\n.inc 'x.sp'\n.inc './sub/../x.sp'\n",
        )
        .unwrap();
        let (parsed, files) = parse_path(&dir.join("two.sp"), TitleLine::Auto).await;
        assert_eq!(parsed.inner.len(), 2);
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err);
        assert_eq!(names(&ast.instance), ["R3", "R3"]);
    }

    /// The lines of the `.end` statements that `ast` (a file) ignored,
    /// also in its `.subckt` bodies.
    fn ignored_ends(ast: &ASTBuilder) -> Vec<u32> {
        let mut out = Vec::new();
        for seg in &ast.segments {
            if let Segment::Local(local) = seg {
                out.extend(&local.ignored_end);
                for s in &local.subckt {
                    out.extend(ignored_ends(&s.ast));
                }
            }
        }
        out.sort_unstable();
        out
    }

    #[tokio::test]
    async fn end_in_include() {
        // Golden `end_in_include`: HSPICE ignores a `.end` in an included
        // file, with a warning, and reads the lines after it. The parse
        // records the line, so the loader can warn too.
        let path = PathBuf::from("golden/cases/end_in_include/deck.sp");
        let (parsed, files) = parse_path(&path, TitleLine::Auto).await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let inc = parsed
            .inner
            .iter()
            .find(|(id, _)| id.path().ends_with("inc.sp"))
            .unwrap();
        assert_eq!(ignored_ends(&inc.1), [2]);
        assert!(ignored_ends(top(&parsed)).is_empty());
        assert_eq!(
            top(&parsed).after_end.map(|e| (e.line, e.lines)),
            Some((6, 0))
        );
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["v1", "r1", "r2", "r3"]);
        // In a `.subckt` body of an included file, and a file with only
        // `.end` (its segment is kept for the note).
        let tmp = TempDir::new("end_in_include");
        let dir = &tmp.0;
        std::fs::write(
            dir.join("inc.sp"),
            ".subckt s a\nR1 a b 1\n.END $ c\nR2 a b 2\n.ends\n",
        )
        .unwrap();
        std::fs::write(dir.join("only.sp"), "* c\n.end\n").unwrap();
        let main = dir.join("main.sp");
        std::fs::write(&main, "* t\n.inc 'inc.sp'\n.inc 'only.sp'\n").unwrap();
        let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let file = |name: &str| {
            &parsed
                .inner
                .iter()
                .find(|(id, _)| id.path().ends_with(name))
                .unwrap()
                .1
        };
        assert_eq!(ignored_ends(file("inc.sp")), [3]);
        assert_eq!(ignored_ends(file("only.sp")), [2]);
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.subckt["s"].ast.instance), ["R1", "R2"]);
    }

    #[tokio::test]
    async fn block_recovery_stops_at_end() {
        // The skip of a failed block statement does not pass the `.end`
        // of the top file: HSPICE reads no line after it (golden
        // `after_end`, `after_end_second_deck`). The skip went on to the
        // `.ends` after `.end` before, so R3 was loaded and the `.end` was
        // lost. Now the block has no closing line, so only its first line is
        // skipped and R1 is read in the enclosing scope.
        let tmp = TempDir::new("block_end");
        let dir = &tmp.0;
        let main = dir.join("main.sp");
        for (text, want, end) in [
            (
                "* t\n.subckt\nR1 a b 1\n.end\nR2 a b 2\n.ends\nR3 a b 3\n",
                &["R1"][..],
                (4, 3),
            ),
            (
                "* t\n.SUBCKT\nR1 a b 1\n.END $ c\n.ENDS\nR3 a b 3\n",
                &["R1"],
                (4, 2),
            ),
            (
                "* t\n.data\nR1 a b 1\n.end\nR2 a b 2\n.enddata\nR3 a b 3\n",
                &["R1"],
                (4, 3),
            ),
            // a `.lib` section whose `.endl` line has extra text
            (
                "* t\n.lib tt\nR1 a b 1\n.end\nR2 a b 2\n.endl tt junk\nR3 a b 3\n",
                &["R1"],
                (4, 3),
            ),
            // a failed `.subckt` inside a body
            (
                "* t\n.subckt s a\n.subckt\nR1 a b 1\n.end\n.ends\n.ends\nR3 a b 3\n",
                &[],
                (5, 3),
            ),
            // a `.ends` before the `.end` still closes the failed block
            (
                "* t\n.subckt\nR1 a b 1\n.ends\nR2 a b 2\n.end\nR3 a b 3\n",
                &["R2"],
                (6, 1),
            ),
        ] {
            std::fs::write(&main, text).unwrap();
            let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
            let got = top(&parsed).after_end.map(|e| (e.line, e.lines));
            assert_eq!(got, Some(end), "{text:?}");
            let (ast, _) = files.build(parsed);
            assert_eq!(names(&ast.instance), want, "{text:?}");
        }
        // A `.subckt` nested too deep, with no `.ends`: the skip stops at
        // `.end`, not at the end of the input.
        let mut text = String::from("* t\n");
        for k in 0..=MAX_NESTING {
            text += &format!(".subckt s{k} a\n");
        }
        text += ".end\nR9 a b 9\n";
        std::fs::write(&main, &text).unwrap();
        let (parsed, _) = parse_path(&main, TitleLine::Auto).await;
        let end_line = MAX_NESTING as u32 + 3;
        assert_eq!(
            top(&parsed).after_end.map(|e| (e.line, e.lines)),
            Some((end_line, 1))
        );
        // In an included file, `.end` is ignored, so the skip goes on to
        // the `.ends`.
        std::fs::write(
            dir.join("inc.sp"),
            ".subckt\nR1 a b 1\n.end\nR2 a b 2\n.ends\nR3 a b 3\n",
        )
        .unwrap();
        std::fs::write(&main, "* t\n.inc 'inc.sp'\nR4 a b 4\n").unwrap();
        let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R3", "R4"]);
    }

    #[tokio::test]
    async fn block_scan_reads_as_main_parse() {
        // The main parse does not read the text of a `.prot` region or of
        // a `.lib <section>` definition, so a `.end` there does not end the
        // top file. The pass that finds the closing line of a failed block
        // reads the file the same way (it stopped at such a `.end`, so the
        // skip of the failed `.subckt` after it found no `.ends`: R3 joined
        // the top level, and its `.ends` was a `stray_ends`). Not probed.
        let tmp = TempDir::new("block_scan_unparsed");
        let main = tmp.0.join("main.sp");
        for (text, want, errors) in [
            (
                "* t\n.lib tt\nR1 a b 1\n.end\n.endl tt\n.subckt\nR3 a b 3\n.ends\nR4 a b 4\n",
                &["R4"][..],
                vec![6],
            ),
            (
                "* t\n.prot\nR1 a b 1\n.end\n.unprot\n.subckt\nR3 a b 3\n.ends\nR4 a b 4\n",
                &["R4"],
                vec![6],
            ),
            // `.ends` lines in such text do not close the failed block
            (
                "* t\n.subckt\nR1 a b 1\n.prot\n.ends\n.unprot\n.ends\nR3 a b 3\n",
                &["R3"],
                vec![2],
            ),
            (
                "* t\n.subckt\nR1 a b 1\n.lib tt\n.ends\n.endl\n.ends\nR3 a b 3\n",
                &["R3"],
                vec![2],
            ),
            // A `.lib` section that does not parse (text after its `.endl`)
            // is read line by line: its `.end` stops the skip (as in
            // `block_recovery_stops_at_end`).
            (
                "* t\n.lib tt\nR1 a b 1\n.end\n.endl tt junk\n.subckt\nR3 a b 3\n.ends\n",
                &["R1"],
                vec![2],
            ),
        ] {
            std::fs::write(&main, text).unwrap();
            let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
            assert_eq!(
                super::recover_tests::error_lines(&parsed),
                errors,
                "{text:?}"
            );
            let (ast, _) = files.build(parsed);
            assert_eq!(names(&ast.instance), want, "{text:?}");
        }
    }

    #[tokio::test]
    async fn end_line_with_text_ends_file() {
        // Golden `end_quoted_arg`, `end_kv_arg`: HSPICE ends the deck at a
        // `.end` line also when other text follows the keyword (`.end 'x'`,
        // `.end x=1`), with no warning. Such a line failed before
        // (`trailing_text`), and the lines after it were read. The pass
        // that finds the closing lines of blocks stopped at it, so the skip
        // of a `.subckt` nested too deep after it jumped back to it, and
        // the parse read the same lines again and again (a hang with
        // unbounded memory).
        for case in ["end_quoted_arg", "end_kv_arg"] {
            let path = PathBuf::from(format!("golden/cases/{case}/deck.sp"));
            let (parsed, files) = parse_path(&path, TitleLine::Auto).await;
            assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
            // the second deck: 4 lines with statements
            let got = top(&parsed).after_end.map(|e| (e.line, e.lines));
            assert_eq!(got, Some((5, 4)), "{case}");
            let (ast, _) = files.build(parsed);
            assert_eq!(names(&ast.instance), ["v1", "r1"], "{case}");
        }
        let tmp = TempDir::new("end_text");
        let main = tmp.0.join("main.sp");
        let deck = |before: usize, end: &str, after: usize| {
            let mut text = String::from("* t\n");
            for k in 0..before {
                text += &format!(".subckt p{k} a\n");
            }
            text += end;
            text += "\n";
            for k in 0..after {
                text += &format!(".subckt s{k} a\n");
            }
            text + "R1 a b 1\n"
        };
        let deep = MAX_NESTING + 2;
        for (text, end, missing) in [
            (deck(0, ".end 'x'", deep), (2, deep as u32 + 1), 0),
            (deck(0, ".end x=1", deep), (2, deep as u32 + 1), 0),
            (deck(100, ".END 'x' $ c", 200), (102, 201), 100),
            // the text after `.end` can go on on `+` lines
            (deck(0, ".end x\n+ y", 1), (2, 2), 0),
        ] {
            std::fs::write(&main, &text).unwrap();
            let (parsed, _) = parse_path(&main, TitleLine::Auto).await;
            let errors = super::recover_tests::errors(&parsed);
            assert_eq!(errors.len(), missing, "{text:?}");
            assert!(
                errors
                    .iter()
                    .all(|e| matches!(e.err, ParseErrorInner::MissingEnds { .. }))
            );
            let got = top(&parsed).after_end.map(|e| (e.line, e.lines));
            assert_eq!(got, Some(end), "{text:?}");
        }
        // The `.end 'x'` line is the end of the file for the main parse and
        // for the skip of a failed block: the failed `.subckt` has no
        // closing line before it, so only its line is skipped (as in
        // `block_recovery_stops_at_end`). The main parse read on before,
        // so R1, R2 and R3 joined the top level, with a `stray_ends`.
        std::fs::write(
            &main,
            "* t\n.subckt\nR1 a b 1\n.end 'x'\nR2 a b 2\n.ends\nR3 a b 3\n",
        )
        .unwrap();
        let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
        assert_eq!(super::recover_tests::error_lines(&parsed), [2]);
        let got = top(&parsed).after_end.map(|e| (e.line, e.lines));
        assert_eq!(got, Some((4, 3)));
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R1"]);
        // In an included file, such a line is ignored as `.end` is
        // (golden `end_in_include`), and the lines after it are read.
        std::fs::write(tmp.0.join("inc.sp"), "R1 a b 1\n.end 'x'\nR2 a b 2\n").unwrap();
        std::fs::write(&main, "* t\n.inc 'inc.sp'\nR3 a b 3\n").unwrap();
        let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
        assert!(error_msgs(&parsed).is_empty(), "{:?}", error_msgs(&parsed));
        let (ast, _) = files.build(parsed);
        assert_eq!(names(&ast.instance), ["R1", "R2", "R3"]);
    }

    #[tokio::test]
    async fn braces_in_quoted_param() {
        // Golden `brace_quoted_param`: HSPICE reads `{a}` inside quotes as
        // the name `[a]` and rejects the deck. So a `.param` statement with
        // `{` or `}` in a quoted value is an error, as without quotes
        // (golden `param_braces_param`), and gives no parameter.
        let path = PathBuf::from("golden/cases/brace_quoted_param/deck.sp");
        let (parsed, files) = parse_path(&path, TitleLine::Auto).await;
        assert_eq!(super::recover_tests::error_lines(&parsed), [2]);
        let (ast, _) = files.build(parsed);
        assert!(ast.param.is_empty());
        let tmp = TempDir::new("brace_param");
        let main = tmp.0.join("main.sp");
        for text in [
            "* t\n.param p='{a}'\nR1 a b 1\n",
            "* t\n.param f(x)='{x}+1'\nR1 a b 1\n",
            "* t\n.param p='a}'\nR1 a b 1\n",
            "* t\n.subckt s a w='{x}'\n.ends\nR1 a b 1\n",
            "* t\n.option x='{a}'\nR1 a b 1\n",
        ] {
            std::fs::write(&main, text).unwrap();
            let (parsed, files) = parse_path(&main, TitleLine::Auto).await;
            assert_eq!(super::recover_tests::error_lines(&parsed), [2], "{text:?}");
            let (ast, _) = files.build(parsed);
            assert!(
                ast.param.is_empty() && ast.param_func.is_empty(),
                "{text:?}"
            );
            assert!(ast.subckt.is_empty(), "{text:?}");
            assert_eq!(names(&ast.instance), ["R1"], "{text:?}");
        }
    }
}

#[test]
fn test_match_lib() {
    let text = r#"
        .LIB TT
        * Some lines here
        M1 d g s b NMOS
        .MEAS ...
        .ENDL TT

        .lib tt
        R1 in out 10k
        .endl
        .lib ttg
        it is wrong
        .endl"#;
    assert_eq!(
        Some(("\n        R1 in out 10k\n".to_owned(), 6)),
        match_lib(text, "tt")
    );
}
