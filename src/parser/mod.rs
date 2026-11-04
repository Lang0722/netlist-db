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
use indexmap::IndexMap;
use manager::ParseManager;
use nom::IResult;
use regex::Regex;
use std::{
    mem,
    path::{Path, PathBuf},
    sync::OnceLock,
};
pub use utils::MAX_SUBCKT_DEPTH;
use utils::local_ast;
pub(crate) use utils::{is_formula_text, is_number_list, number_prefix};

use super::{
    Files, Parsed,
    err::{ParseError, ParseErrorInner},
};
use crate::{
    ast::{ASTBuilder, LocalAST, Segment},
    span::{EndReason, FileId, FileStorage, LocatedSpan, ParsedId, Pos, span},
};

const BEGIN_TITLE: &str = ".TITLE";

/// Parse statements until the end of the input, or until `.ends` when
/// `in_subckt` is set. `depth` is the number of `.subckt` definitions around
/// the input. Returns the AST and whether a `.ends` closed it.
#[inline]
pub fn ast(
    manager: Arc<ParseManager>,
    loaded: IndexMap<FileId, Option<Pos>>,
    work_dir: PathBuf,
    mut i: LocatedSpan,
    in_subckt: bool,
    depth: usize,
) -> IResult<LocatedSpan, (ASTBuilder, bool)> {
    let mut ast = ASTBuilder::new();
    loop {
        let _ast;
        let reason;
        (i, (_ast, reason)) = local_ast(i, &loaded, &manager, &work_dir, in_subckt, depth)?;
        if !_ast.is_empty() {
            ast.segments.push(Segment::Local(Box::new(_ast)));
        }
        match reason {
            EndReason::Include { file_name, section } => {
                let res = Arc::new(OnceLock::new());
                ast.segments.push(Segment::Include(res.clone()));
                let manager_clone = manager.clone();
                let file_path = include_path(&work_dir, file_name);
                let include_pos = Pos::new(i);
                let mut loaded = loaded.clone();
                if let Some((_, _include_pos)) = loaded.last_mut() {
                    *_include_pos = include_pos
                }
                manager.spawn_parse(async move {
                    include(manager_clone, loaded, file_path, section, include_pos, res).await;
                });
            }
            EndReason::End => return Ok((i, (ast, false))),
            EndReason::Ends => return Ok((i, (ast, true))),
        }
    }
}

/// The path of an included file. A Windows path (`.\models\nmos.sp`) that is
/// not found as written is also tried with `/`.
fn include_path(work_dir: &Path, file_name: &Path) -> PathBuf {
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

/// Read a netlist as text: remove UTF-8 byte order marks (also from inside a
/// file, e.g. one made by joining files), turn lone CR line ends (classic
/// Mac OS) into LF, and read bytes that are not UTF-8 (e.g. a `µ` written in
/// Latin-1) as Latin-1, with a warning, instead of rejecting the whole file.
fn decode(bytes: Vec<u8>) -> (String, Option<ParseError>) {
    let (mut text, note) = match String::from_utf8(bytes) {
        Ok(text) => (text, None),
        Err(e) => {
            let bytes = e.into_bytes();
            let mut text = String::with_capacity(bytes.len() + bytes.len() / 8);
            let mut invalid = 0_usize;
            let mut first_line = 0;
            for chunk in bytes.utf8_chunks() {
                text.push_str(chunk.valid());
                for &b in chunk.invalid() {
                    if invalid == 0 {
                        first_line = text.matches('\n').count() + 1;
                    }
                    invalid += 1;
                    text.push(char::from(b));
                }
            }
            let note = ParseErrorInner::Encoding(format!(
                "The file is not valid UTF-8: {invalid} byte(s) were read as Latin-1 \
                 (the first one on line {first_line})"
            ))
            .with(None);
            (text, Some(note))
        }
    };
    if text.contains('\u{feff}') {
        text = text.replace('\u{feff}', "");
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
    (text, note)
}

async fn _include(
    manager: Arc<ParseManager>,
    mut loaded: IndexMap<FileId, Option<Pos>>,
    file_id: FileId,
    include_pos: Option<Pos>,
) -> Result<ParsedId, (FileId, ParseError)> {
    // (a top-level file is not included from anywhere)
    let top_level = include_pos.is_none();
    let mut file_path = file_id.path().to_path_buf();
    if let Some(idx) = loaded.get_index_of(&file_id) {
        return Err((
            file_id,
            ParseErrorInner::CircularDefinition(loaded, idx).with(include_pos),
        ));
    } else {
        loaded.insert(file_id.clone(), include_pos);
    }
    // Claim the file before reading it: a file included from several places
    // at once is read and parsed once.
    let parsed_id = {
        let mut storage = manager.file_storage.lock().await;
        if let Some(parsed_id) = storage.existed(&file_id) {
            return Ok(parsed_id);
        }
        storage.new_file(file_id.clone())
    };
    // (a file that cannot be read is released, so that each include of it
    // reports the error)
    let release = async |file_id: &FileId| {
        manager.file_storage.lock().await.id2idx.remove(file_id);
    };
    let (contents, encoding_note) = match tokio::fs::read(&file_path).await {
        Ok(bytes) => decode(bytes),
        Err(e) => {
            release(&file_id).await;
            return Err((file_id, ParseErrorInner::with(e.into(), include_pos)));
        }
    };
    let (line_off_set, file_ctx) = if let FileId::Section { path: _, section } = file_id.clone() {
        if let Some((file_ctx, line_off_set)) = match_lib(&contents, &section) {
            (line_off_set, file_ctx)
        } else {
            release(&file_id).await;
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
    let file_ctx = join_continued_lines(file_ctx);
    let mut i = span(&file_ctx, line_off_set);
    // get dir
    file_path.pop();
    #[cfg(test)]
    assert!(file_path.is_dir());

    let mut head = LocalAST::default();
    head.errors.extend(encoding_note);
    if top_level && matches!(file_id, FileId::Include { .. }) {
        if let Some((rest, title, element)) = utils::title_line(i) {
            if element {
                head.errors.push(ParseErrorInner::TitleIsElement.record(i));
            }
            head.title = Some(title);
            i = rest;
        }
    }
    let mut res = match ast(manager.clone(), loaded, file_path, i, false, 0) {
        Ok((_, (res, _))) => res,
        Err(e) => error2ast(e.into()),
    };
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
async fn include(
    manager: Arc<ParseManager>,
    loaded: IndexMap<FileId, Option<Pos>>,
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
    let _res = _include(manager, loaded, file_id, include_pos).await;
    res.set(_res.map_err(|(_, e)| e)).unwrap();
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
        for c in body.chars() {
            match c {
                '\'' => in_quote = !in_quote,
                '$' if !in_quote => {
                    dollar_comment = true;
                    break;
                }
                _ => {}
            }
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

pub async fn parse_top_multi<I: Iterator<Item = FileId>>(file_ids: I) -> (Parsed, Files) {
    let (manager, done_rx) = ParseManager::new();
    // Hold a guard while the top-level files are parsed: the includes of one
    // file may all finish while another top-level file has not spawned its
    // includes yet, and the count must not reach zero (the "done" signal)
    // before every top-level file has been parsed.
    let top_guard = manager.task_guard();
    let handles: Vec<_> = file_ids
        .map(|file_id| {
            let handle = crate::spawn({
                let manager = manager.clone();
                let file_id = file_id.clone();
                async move { _include(manager, IndexMap::with_capacity(1), file_id, None).await }
            });
            (file_id, handle)
        })
        .collect();
    let mut tops = Vec::with_capacity(handles.len());
    for (file_id, handle) in handles {
        tops.push(match handle.await {
            Ok(res) => res,
            Err(e) => Err((
                file_id,
                ParseErrorInner::Internal(format!("parser task failed: {e}")).with(None),
            )),
        });
    }
    drop(top_guard);
    manager.wait(done_rx).await;
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
            Err((file_id, e)) => {
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
        },
        Files { inner: files },
    )
}

fn match_lib(text: &str, section: &str) -> Option<(String, u32)> {
    let section_escaped = regex::escape(section);
    let re = Regex::new(&format!(
        r"(?ims)^\s*\.lib\s+{section_escaped}\b(.*?)^\s*\.endl(?:\s+{section_escaped}\b)?"
    ))
    .unwrap();
    re.captures_iter(text).last().map(|caps| {
        // Count lines up to the section body (which starts on the `.lib` line),
        // not up to the match start: `^\s*` also matches blank lines before it.
        let start_offset = caps.get(1).unwrap().start();
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
    // the matched `.lib tt` is on line 8 (offset 7), after a blank line 7
    assert_eq!(
        Some(("\n        R1 in out 10k\n".to_owned(), 7)),
        match_lib(text, "tt")
    );
}
