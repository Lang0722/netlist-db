//! The files of a netlist: its include lines, their limits, and how a file is found and read.
use super::blocks::Blocks;
use super::manager::{Include, ParseManager};
use super::source::{decode, is_raw_byte, join_continued_lines, skip_comment_lines};
use super::statement::skip_statement;
use super::{TitleLine, ast, title_line};
use crate::err::{ParseError, ParseErrorInner};
use crate::{
    ast::{ASTBuilder, LocalAST, Segment, ValueBuilder},
    span::{FileId, FileStorage, ParsedId, Pos, span},
};
use alloc::sync::Arc;
use nom::Input as _;
#[cfg(test)]
use regex::Regex;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// The longest chain of includes that is followed. The walks that follow includes
/// recurse. HSPICE reads a chain of 1000 files (deck `i_chain_1000`), and a real deck
/// has a short chain.
pub const MAX_INCLUDE_DEPTH: usize = 1100;

/// How many files are open at one time
pub(super) const MAX_OPEN_FILES: usize = 64;

/// What a walk does at an include line
pub enum IncludeStep {
    Follow,
    /// It skips the include, with this message.
    Skip(String),
    /// It skips the include, and an earlier message said why.
    SkipQuietly,
}

/// The limits of a walk that follows the include lines, and each copy of a file
/// that it read: files that each include the next one twice expand to 2^n copies.
pub struct IncludeLimits {
    /// How much text the walk can still take in. `None`: past the limit.
    room: Option<usize>,
}

impl IncludeLimits {
    /// `total`: the size of all the files, each one once
    pub fn new(total: usize) -> Self {
        let room = if cfg!(test) { 1 << 20 } else { 1 << 30 };
        Self {
            room: Some(total.saturating_mul(16).saturating_add(room)),
        }
    }

    /// The step at an include of a file of `size` bytes, with `depth` files in the
    /// chain of includes above it
    pub fn step(&mut self, depth: usize, size: usize) -> IncludeStep {
        let Some(room) = self.room else {
            return IncludeStep::SkipQuietly;
        };
        if depth >= MAX_INCLUDE_DEPTH {
            return IncludeStep::Skip(format!(
                "more than {MAX_INCLUDE_DEPTH} files in a chain of includes: this include is \
                 skipped"
            ));
        }
        self.room = room.checked_sub(size);
        if self.room.is_none() {
            return IncludeStep::Skip(
                "the includes expand to too much text (a file that includes others twice?): \
                 this include and all later ones are skipped"
                    .to_owned(),
            );
        }
        IncludeStep::Follow
    }
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

/// Starts the parse of a file that `include` reads
pub(super) fn spawn_include(manager: &Arc<ParseManager>, include_line: Include) {
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
pub(super) fn include_path(roots: &[PathBuf], work_dir: &Path, file_name: &Path) -> PathBuf {
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
pub(super) async fn search_includes(manager: &Arc<ParseManager>, tops: &[ParsedId]) {
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
pub(super) async fn read_include(
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
    if not_dot_title && let Some((rest, title, element)) = super::statement::title_line(i) {
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

#[inline]
pub(super) async fn include(
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
    let _res = read_include(manager, file_id, include_pos, None).await;
    res.set(_res.map_err(|failed| failed.1)).unwrap();
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
