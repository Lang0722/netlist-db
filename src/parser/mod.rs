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
use manager::{Include, ParseManager};
use nom::Input as _;
#[cfg(test)]
use regex::Regex;
use std::{
    collections::HashSet,
    mem,
    path::{Path, PathBuf},
    sync::OnceLock,
};
use utils::{Blocks, Open, Step, local_ast, skip_statement};
pub use utils::{MAX_NESTING, canonical_cmd};
pub(crate) use utils::{is_formula_text, is_number_list, number_prefix};

use super::{
    Files, Parsed,
    err::{ParseError, ParseErrorInner},
};
use crate::{
    ast::{ASTBuilder, AfterEnd, LocalAST, Segment, SubcktBuilder, TitleLineInfo, ValueBuilder},
    span::{FileId, FileStorage, LocatedSpan, ParsedId, Pos, Span, span},
};

/// The longest chain of includes that is followed. The walks that follow includes
/// recurse. HSPICE reads a chain of 1000 files (deck `i_chain_1000`), and a real deck
/// has a short chain.
pub const MAX_INCLUDE_DEPTH: usize = 1100;

/// How many files are open at one time
const MAX_OPEN_FILES: usize = 64;

/// How much text the includes can expand to, counting each copy of a file:
/// files that each include the next one twice expand to 2^n copies. `total` is the
/// size of all the files, each one once.
pub fn expanded_text_limit(total: usize) -> usize {
    let room = if cfg!(test) { 1 << 20 } else { 1 << 30 };
    total.saturating_mul(16).saturating_add(room)
}

/// The text of `path`. An include line must name a regular file: a device or a
/// FIFO can give text with no end, or no text.
async fn read_source(
    manager: &ParseManager,
    path: &Path,
    include: bool,
) -> std::io::Result<Vec<u8>> {
    let _open = manager
        .open_files
        .acquire()
        .await
        .map_err(std::io::Error::other)?;
    if include && !tokio::fs::metadata(path).await?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    tokio::fs::read(path).await
}

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

/// Starts the parse of a file that `include` reads
fn spawn_include(manager: &Arc<ParseManager>, include_line: Include) {
    let manager_clone = manager.clone();
    manager.spawn_parse(async move {
        let Include {
            path,
            section,
            pos,
            res,
            ..
        } = include_line;
        include(manager_clone, path, section, pos, res).await;
    });
}

/// The path of an included file: in `work_dir`, the folder of the file that
/// includes it, else in the first of `roots` that has it, else in `work_dir`
/// (the error names that path). HSPICE looks in the folder of the file and then in
/// its working folder (decks `inc_nested_rel_cwd`, `cwd_probe`). Here the folders
/// of the top files stand first for that folder, so the result does not depend
/// on where this program runs.
fn include_path(roots: &[PathBuf], work_dir: &Path, file_name: &Path) -> PathBuf {
    let path = path_in(work_dir, file_name);
    if path.exists() || file_name.is_absolute() {
        return path;
    }
    roots
        .iter()
        .map(|root| path_in(root, file_name))
        .find(|p| p.exists())
        .unwrap_or(path)
}

/// The folders of the `.option search` items of the files parsed so far, from
/// the files `tops` down, in the order of the deck. HSPICE takes them in that
/// order, the first one that has the file wins, and an item counts in any
/// file and in a subckt. They do not depend on the order in which the
/// files were read, because this list is made after the parse of all of them.
fn search_folders(storage: &FileStorage<ASTBuilder>, tops: &[ParsedId]) -> Vec<PathBuf> {
    let mut folders = Vec::new();
    let mut seen = HashSet::new();
    let of = |id: &ParsedId| {
        (
            storage.parsed[id.0].1.segments.iter(),
            storage.file[id.0].as_str(),
        )
    };
    let mut stack: Vec<_> = tops.iter().rev().map(of).collect();
    seen.extend(tops.iter().map(|id| id.0));
    while let Some((segments, text)) = stack.last_mut() {
        let text: &str = text;
        let Some(segment) = segments.next() else {
            stack.pop();
            continue;
        };
        match segment {
            Segment::Local(local) => {
                for (k, v) in &local.option {
                    if text[k.start..k.end].eq_ignore_ascii_case("search")
                        && let Some(ValueBuilder::Expr(folder)) = v
                    {
                        folders.push(PathBuf::from(&text[folder.start..folder.end]));
                    }
                }
                for subckt in local.subckt.iter().rev() {
                    stack.push((subckt.ast.segments.iter(), text));
                }
            }
            Segment::Include { res, .. } => {
                if let Some(Ok(id)) = res.get()
                    && seen.insert(id.0)
                {
                    stack.push(of(id));
                }
            }
            Segment::Alter { .. } | Segment::Cond { .. } => {}
        }
    }
    folders
}

/// Reads the includes that were not found from the folders of [`include_path`],
/// in the folders of `.option search`. A file so found can set more of them, and
/// have such includes of its own. The others read as written, which gives their
/// errors.
async fn search_includes(manager: &Arc<ParseManager>, tops: &[ParsedId]) {
    loop {
        let waiting = manager.take_deferred();
        if waiting.is_empty() {
            return;
        }
        let folders = search_folders(&*manager.file_storage.lock().await, tops);
        let guard = manager.task_guard();
        let done_rx = manager.rearm();
        let mut found = false;
        let mut others = Vec::new();
        for mut include in waiting {
            let path = folders.iter().find_map(|folder| {
                let in_root = |root: &PathBuf| path_in(&root.join(folder), &include.file_name);
                manager.roots.iter().map(in_root).find(|p| p.exists())
            });
            if let Some(path) = path {
                include.path = path;
                found = true;
                spawn_include(manager, include);
            } else {
                others.push(include);
            }
        }
        for include in others {
            if found {
                manager.defer(include);
            } else {
                spawn_include(manager, include);
            }
        }
        drop(guard);
        manager.wait(done_rx).await;
    }
}

/// The path of the file `file_name` in `work_dir`. A Windows path
/// (`.\\models\\nmos.sp`) that is not found as written is also tried with `/`.
fn path_in(work_dir: &Path, file_name: &Path) -> PathBuf {
    let join = |name: &Path| {
        if name.is_absolute() {
            name.to_path_buf()
        } else {
            work_dir.join(name)
        }
    };
    let path = join(file_name);
    if let Some(name) = file_name.to_str()
        && name.contains('\\')
        && !path.exists()
    {
        let unix = join(Path::new(&name.replace('\\', "/")));
        if unix.exists() {
            return unix;
        }
    }
    path
}

/// The character that stands for `byte`, a byte that is not UTF-8 (0x80 or
/// above). Each value has its own character in a private range, so that two
/// names that differ in such a byte stay two names.
fn raw_byte(byte: u8) -> char {
    char::from_u32(0xF700 + u32::from(byte)).expect("a private character")
}

/// Whether `c` is a character of [`raw_byte`]
pub(crate) fn is_raw_byte(c: char) -> bool {
    ('\u{F780}'..='\u{F7FF}').contains(&c)
}

/// `bytes` as text, with [`raw_byte`] for each byte that is not UTF-8
fn with_raw_bytes(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        text.push_str(chunk.valid());
        text.extend(chunk.invalid().iter().map(|&byte| raw_byte(byte)));
    }
    text
}

/// Read a netlist as text: remove UTF-8 byte order marks (also from inside a
/// file, e.g. one made by joining files), and turn lone CR line ends (classic
/// Mac OS) into LF. Bytes that are not UTF-8 (e.g. a `µ` written in Latin-1)
/// are read with [`raw_byte`], instead of rejecting the whole file. Also returns
/// the first line with such bytes.
fn decode(bytes: Vec<u8>) -> (String, Option<u32>) {
    let (mut text, lossy) = match String::from_utf8(bytes) {
        Ok(text) => (text, false),
        Err(e) => (with_raw_bytes(e.as_bytes()), true),
    };
    if text.contains('\u{feff}') {
        text = text.replace('\u{feff}', "");
    }
    // (white space, as measured: deck `e_formfeed`)
    if text.contains('\u{c}') {
        text = text.replace('\u{c}', " ");
    }
    if text.contains("//") {
        text = slash_comments(text);
    }
    let bytes = text.as_bytes();
    if text
        .match_indices('\r')
        .any(|(idx, _)| bytes.get(idx + 1) != Some(&b'\n'))
    {
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            out.push(if c == '\r' && chars.peek() != Some(&'\n') {
                '\n'
            } else {
                c
            });
        }
        text = out;
    }
    let line = lossy.then(|| {
        let at = text.find(is_raw_byte).unwrap_or(0);
        u32::try_from(text[..at].matches('\n').count() + 1).unwrap_or(u32::MAX)
    });
    (text, line)
}

/// HSPICE reads a word that starts with `//` as the start of a comment, on every
/// kind of line, and a following `+` line goes on with the statement (decks
/// `cm_slash_*`, `o_slash_*`). This turns `//` into `$ `: the length of the text
/// stays, so the positions in it do too. Inside quotes, and on a line that names
/// a file, `//` stays.
fn slash_comments(text: String) -> String {
    let mut bytes = text.into_bytes();
    for line in bytes.split_mut(|b| *b == b'\n') {
        let first = line.trim_ascii_start();
        if first.starts_with(b"*") || names_a_file(first) {
            continue;
        }
        let mut quote = None;
        let mut word_start = true;
        for k in 0..line.len() {
            let c = line[k];
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None if matches!(c, b'\'' | b'"') => quote = Some(c),
                None if c == b'$' && word_start => break,
                None if c == b'/' && word_start && line.get(k + 1) == Some(&b'/') => {
                    line[k] = b'$';
                    line[k + 1] = b' ';
                    break;
                }
                None => {}
            }
            word_start = quote.is_none() && matches!(c, b' ' | b'\t' | b',' | b'(' | b')');
        }
    }
    // (only ASCII characters changed)
    String::from_utf8(bytes).expect("the text was UTF-8")
}

/// Whether `line`, with no spaces before it, is a command with a file name
fn names_a_file(line: &[u8]) -> bool {
    let Some(word) = line.strip_prefix(b".") else {
        return false;
    };
    let len = word
        .iter()
        .position(|b| !b.is_ascii_alphabetic())
        .unwrap_or(word.len());
    let word = String::from_utf8_lossy(&word[..len]).to_ascii_lowercase();
    matches!(canonical_cmd(&word), "include" | "lib" | "hdl" | "vec")
}

/// The text of a file read (and its first line that is not UTF-8, if any):
/// its own, or shared by the sections loaded from a library file
enum LibText {
    Own((String, Option<u32>)),
    Shared(Arc<(String, Option<u32>)>),
}

impl LibText {
    fn text(&self) -> &str {
        match self {
            Self::Own((text, _)) => text,
            Self::Shared(shared) => &shared.0,
        }
    }

    fn non_utf8(&self) -> Option<u32> {
        match self {
            Self::Own((_, line)) => *line,
            Self::Shared(shared) => shared.1,
        }
    }

    fn into_text(self) -> String {
        match self {
            Self::Own((text, _)) => text,
            Self::Shared(shared) => shared.0.clone(),
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
) -> Result<ParsedId, Box<(FileId, ParseError)>> {
    let mut file_path = file_id.path().to_path_buf();
    // Claim the file before reading it: a file included from several places
    // at once is read and parsed once. One file can also be reached by paths
    // spelled in other ways (`sub/../main.sp`, a link): it is one file with
    // one parse, so an include cycle through such a path is seen too.
    let canonical = file_id.canonical();
    let parsed_id = {
        let mut storage = manager.file_storage.lock().await;
        let known = storage
            .existed(&file_id)
            .or_else(|| canonical.as_ref().and_then(|c| storage.existed(c)));
        if let Some(parsed_id) = known {
            storage.alias(file_id, parsed_id);
            return Ok(parsed_id);
        }
        let parsed_id = storage.new_file(file_id.clone());
        if let Some(canonical) = &canonical {
            storage.alias(canonical.clone(), parsed_id);
        }
        parsed_id
    };
    // (a file that cannot be read is released, so that each include of it
    // reports the error)
    let release = async |file_id: &FileId| {
        let mut storage = manager.file_storage.lock().await;
        storage.id2idx.remove(file_id);
        if let Some(canonical) = &canonical {
            storage.id2idx.remove(canonical);
        }
    };
    // the sections of a library file share one reading of it
    let text = match &file_id {
        FileId::Section { .. } => {
            let cell = manager
                .lib_text
                .lock()
                .await
                .entry(file_path.clone())
                .or_default()
                .clone();
            cell.get_or_init(|| async {
                read_source(&manager, &file_path, true)
                    .await
                    .map(|bytes| Arc::new(decode(bytes)))
                    .map_err(|e| (e.kind(), e.to_string()))
            })
            .await
            .clone()
            .map(LibText::Shared)
            .map_err(|(kind, msg)| std::io::Error::new(kind, msg))
        }
        FileId::Include { .. } => read_source(&manager, &file_path, title.is_none())
            .await
            .map(|bytes| LibText::Own(decode(bytes))),
    };
    let text = match text {
        Ok(text) => text,
        Err(e) => {
            release(&file_id).await;
            return Err(Box::new((
                file_id,
                ParseErrorInner::with(e.into(), include_pos),
            )));
        }
    };
    let non_utf8 = text.non_utf8();
    let (line_off_set, file_ctx) = if let FileId::Section { path: _, section } = file_id.clone() {
        if let Some((file_ctx, line_off_set)) = match_lib(text.text(), &section) {
            (line_off_set, without_section_lines(file_ctx))
        } else {
            release(&file_id).await;
            return Err(Box::new((
                file_id,
                ParseErrorInner::NoLibSection {
                    path: file_path,
                    section,
                }
                .with(include_pos),
            )));
        }
    } else {
        (0, text.into_text())
    };
    // in a `.lib` section, its first bad line (the bytes outside the
    // section are not read)
    let non_utf8 = match &file_id {
        FileId::Section { .. } => non_utf8.and_then(|_| {
            let at = file_ctx.find(is_raw_byte)?;
            let lines = file_ctx[..at].matches('\n').count();
            Some(line_off_set.saturating_add(1 + u32::try_from(lines).unwrap_or(u32::MAX)))
        }),
        FileId::Include { .. } => non_utf8,
    };
    let encoding_note = non_utf8.map(|line| {
        ParseErrorInner::Encoding(format!(
            "The file is not valid UTF-8: each byte that is not UTF-8 was read as a private \
             character (the first one on line {line})"
        ))
        .with(None)
    });
    let file_ctx = join_continued_lines(file_ctx);
    let top = title.is_some();
    // Only a whole file has a title line, not a `.lib` section.
    let title = title.filter(|_| matches!(file_id, FileId::Include { .. }));
    let not_dot_title = title == Some(TitleLine::NotDot);
    let title = title.and_then(|mode| title_line(&file_ctx, mode));
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

    let mut head = LocalAST::default();
    head.errors.extend(encoding_note);
    head.errors.extend(plus_after_title);
    if not_dot_title && let Some((rest, title, element)) = utils::title_line(i) {
        if element {
            head.errors.push(ParseErrorInner::TitleIsElement.record(i));
        }
        head.title = Some(title);
        i = rest;
    }
    // `ast` records statement errors inside the AST and does not fail
    let mut res = {
        let blocks = if top {
            Blocks::new_top(i)
        } else {
            Blocks::new(i)
        }
        .skip_prot(manager.skip_prot());
        ast(manager.clone(), file_path, &blocks, i, top)
    };
    res.title = title;
    res.non_utf8 = non_utf8;
    if !head.is_empty() {
        res.segments.insert(0, Segment::Local(Box::new(head)));
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
    res.set(_res.map_err(|failed| failed.1)).unwrap();
}

/// HSPICE continues a statement with a `\` at the end of a line (the line
/// break counts as white space) or a `\\` (the lines are joined with no white
/// space, e.g. to split a long path), also inside quoted strings. Such lines
/// are joined here, before parsing. Each removed newline is put back after the
/// joined line, so the line numbers of what follows stay right. A `\` inside a
/// comment continues nothing; a `+` that also starts the next line (outside
/// quotes) is dropped.
fn join_continued_lines(text: String) -> String {
    if !text.contains('\\') {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut moved_newlines = 0;
    let mut continuing = false;
    // the previous line ended with `\\`
    let mut squeeze = false;
    // quote state of the statement being joined
    let mut in_quote = false;
    for line in text.split_inclusive('\n') {
        let (mut body, eol) = match line.strip_suffix('\n') {
            Some(b) => match b.strip_suffix('\r') {
                Some(b) => (b, "\r\n"),
                None => (b, "\n"),
            },
            None => (line, ""),
        };
        let comment_line = !continuing && body.trim_start().starts_with('*');
        if continuing {
            if squeeze {
                body = body.trim_start();
            }
            // (inside quotes a leading `+` is an operator, and is kept)
            let plus = if in_quote {
                None
            } else {
                body.trim_start().strip_prefix('+')
            };
            if let Some(rest) = plus {
                body = if squeeze { rest.trim_start() } else { rest };
            }
        } else {
            in_quote = false;
        }
        let mut dollar_comment = false;
        // whether the word here starts with a letter: a `$` in such a word is
        // in a name (see `utils::dollar_in_name`)
        let mut in_name = None;
        for c in body.chars() {
            match c {
                '\'' => in_quote = !in_quote,
                '$' if !in_quote && in_name != Some(true) => {
                    dollar_comment = true;
                    break;
                }
                _ => {}
            }
            in_name = (c.is_alphanumeric() || "_.$".contains(c))
                .then(|| in_name.unwrap_or(c.is_alphabetic() || c == '_'));
        }
        let trimmed = body.trim_end();
        let backslashes = if comment_line || dollar_comment {
            0
        } else if trimmed.ends_with("\\\\") {
            2
        } else if trimmed.ends_with('\\') {
            1
        } else {
            0
        };
        if backslashes == 0 {
            out.push_str(body);
            out.push_str(eol);
            out.extend(std::iter::repeat_n('\n', moved_newlines));
            moved_newlines = 0;
            continuing = false;
        } else {
            let content = &trimmed[..trimmed.len() - backslashes];
            if backslashes == 2 {
                out.push_str(content.trim_end());
            } else {
                out.push_str(content);
                out.push(' ');
            }
            if !eol.is_empty() {
                moved_newlines += 1;
            }
            continuing = true;
            squeeze = backslashes == 2;
        }
    }
    out.extend(std::iter::repeat_n('\n', moved_newlines));
    out
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
                async move { _include(manager, file_id, None, Some(title)).await }
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

/// The section `section` of a library file: the text after `.lib section`,
/// up to the next line that starts with `.endl` (the last such section, if
/// there are several), and the number of the line where it starts (from 0).
///
/// A library file is scanned once per section loaded from it, so this is a
/// single pass over its lines (PDK model files are tens of MB).
fn match_lib(text: &str, section: &str) -> Option<(String, u32)> {
    let starts_with = |s: &str, prefix: &str| {
        s.get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    };
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let last_is_word = section.chars().last().is_some_and(is_word);
    let mut found = None;
    // in a section: where its text starts, and where the run of blank lines
    // before the current line starts
    let mut open: Option<usize> = None;
    let mut blank_from: Option<usize> = None;
    let mut end = 0;
    for line in text.split_inclusive('\n') {
        let start = end;
        end += line.len();
        let t = line.trim_start();
        if let Some(body) = open {
            if t.is_empty() {
                blank_from.get_or_insert(start);
            } else {
                if starts_with(t, ".endl") && !t[5..].chars().next().is_some_and(is_word) {
                    found = Some((body, blank_from.unwrap_or(start)));
                    open = None;
                }
                blank_from = None;
            }
            continue;
        }
        // `.lib section`, the name followed by the end of a word
        if !section.is_empty() && starts_with(t, ".lib") {
            let rest = &t[4..];
            let name = rest.trim_start();
            if name.len() < rest.len()
                && starts_with(name, section)
                && last_is_word != name[section.len()..].chars().next().is_some_and(is_word)
            {
                open = Some(end - name.len() + section.len());
                blank_from = None;
            }
        }
    }
    found.map(|(body, body_end)| {
        let line = text[..body].matches('\n').count() as u32;
        (text[body..body_end].to_owned(), line)
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
/// The text `body` of a section, with its `.lib <name>` lines blank: HSPICE reads
/// on over such a line (the first `.endl` line ended the section), so the text
/// of an inner section is a part of the outer one (decks `ln_*`).
fn without_section_lines(mut body: String) -> String {
    let mut lines = Vec::new();
    let mut end = 0;
    for line in body.split_inclusive('\n') {
        let start = end;
        end += line.len();
        let t = line.trim_start();
        let is_lib = t.get(..4).is_some_and(|k| k.eq_ignore_ascii_case(".lib"));
        if is_lib && t[4..].starts_with([' ', '\t']) {
            let words = t[4..].split('$').next().unwrap_or_default();
            if words.split_whitespace().count() == 1 {
                lines.push(start..start + line.trim_end().len());
            }
        }
    }
    for line in lines {
        let blank = " ".repeat(line.len());
        body.replace_range(line, &blank);
    }
    body
}

/// `match_lib` as it was written with a regex: the regex of `section`
#[cfg(test)]
fn match_lib_regex(section: &str) -> impl Fn(&str) -> Option<(String, u32)> {
    let section_escaped = regex::escape(section);
    let re = Regex::new(&format!(
        r"(?ims)^\s*\.lib\s+{section_escaped}\b(.*?)^\s*\.endl\b(?:\s+{section_escaped}\b)?"
    ))
    .unwrap();
    move |text| {
        re.captures_iter(text).last().map(|caps| {
            let start_offset = caps.get(1).unwrap().start();
            let line_num_offset = (text[..start_offset].matches('\n').count()) as u32;
            (caps[1].to_owned(), line_num_offset)
        })
    }
}

#[test]
fn match_lib_like_the_regex() {
    // random texts made of lines that open, close or fill sections
    let pieces = [
        ".lib tt",
        ".LIB TT",
        "  .lib  tt",
        ".lib tt_mc",
        ".lib ttg",
        ".lib ss",
        ".libtt",
        ".lib 'f.l' tt",
        ".endl",
        ".ENDL TT",
        "  .endl ss",
        ".endlx",
        "",
        "   ",
        "\t",
        "M1 d g s b nch",
        "* .lib tt",
        "+ .endl",
        ".lib",
        ".lib\ttt x",
        ".lib tt$c",
        ".lib tt.x",
        ".lib tt\u{b5}",
        "\u{b5}",
        ".lib\u{a0}tt",
        "\u{a0}.endl",
    ];
    let regexes: Vec<_> = ["tt", "ss", "TT"]
        .into_iter()
        .map(|section| (section, match_lib_regex(section)))
        .collect();
    let mut seed = 12345u64;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    for _ in 0..100_000 {
        let n = next() % 12;
        let mut text = String::new();
        for i in 0..n {
            if i > 0 {
                text.push_str(if next() % 5 == 0 { "\r\n" } else { "\n" });
            }
            text.push_str(pieces[next() % pieces.len()]);
        }
        for (section, regex) in &regexes {
            assert_eq!(
                match_lib(&text, section),
                regex(&text),
                "{text:?} {section}"
            );
        }
    }
}

#[cfg(test)]
mod recover_tests;

/// Statements and parse options (Task 5 of the plan): `.param` functions,
/// `.global`, `.prot` regions, `.alter`, the title line, `.lib` overrides
/// and analysis commands.
#[cfg(test)]
mod statement_tests;

/// Follow-ups of Task 5a (5a-P): `.lib` override triples, `.endl` in
/// comments, deep `.subckt` nesting, `.macro`/`.eom`, and the title line.
#[cfg(test)]
mod follow_up_tests;

/// Task 6a-P: the lines after `.end`, and files that are not valid UTF-8.
#[cfg(test)]
mod end_and_utf8_tests;

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
    // `.lib tt` is line 8: 7 lines come before it
    assert_eq!(
        Some(("\n        R1 in out 10k\n".to_owned(), 7)),
        match_lib(text, "tt")
    );
    // blank and space-only lines before the `.lib` line count
    let text = "* lib\n\n   \n\t\n.lib tt\n.subckt res a b\n.ends\n.endl tt\n";
    assert_eq!(match_lib(text, "tt").map(|(_, n)| n), Some(4));
}
