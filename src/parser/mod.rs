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
use std::{mem, path::PathBuf, sync::OnceLock};
use utils::local_ast;

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
/// `in_subckt` is set. Returns the AST and whether a `.ends` closed it.
#[inline]
pub fn ast(
    manager: Arc<ParseManager>,
    loaded: IndexMap<FileId, Option<Pos>>,
    work_dir: PathBuf,
    mut i: LocatedSpan,
    in_subckt: bool,
) -> IResult<LocatedSpan, (ASTBuilder, bool)> {
    let mut ast = ASTBuilder::new();
    loop {
        let _ast;
        let reason;
        (i, (_ast, reason)) = local_ast(i, &loaded, &manager, &work_dir, in_subckt)?;
        if !_ast.is_empty() {
            ast.segments.push(Segment::Local(Box::new(_ast)));
        }
        match reason {
            EndReason::Include { file_name, section } => {
                let res = Arc::new(OnceLock::new());
                ast.segments.push(Segment::Include(res.clone()));
                let manager_clone = manager.clone();
                let file_path = if file_name.is_absolute() {
                    file_name.to_path_buf()
                } else {
                    work_dir.join(file_name)
                };
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

async fn _include(
    manager: Arc<ParseManager>,
    mut loaded: IndexMap<FileId, Option<Pos>>,
    file_id: FileId,
    include_pos: Option<Pos>,
) -> Result<ParsedId, (FileId, ParseError)> {
    let mut file_path = file_id.path().to_path_buf();
    if let Some(idx) = loaded.get_index_of(&file_id) {
        return Err((
            file_id,
            ParseErrorInner::CircularDefinition(loaded, idx).with(include_pos),
        ));
    } else {
        loaded.insert(file_id.clone(), include_pos);
    }
    if let Some(parsed_id) = manager.file_storage.lock().await.existed(&file_id) {
        return Ok(parsed_id);
    }
    let contents = match tokio::fs::read_to_string(&file_path).await {
        Ok(contents) => contents,
        Err(e) => return Err((file_id, ParseErrorInner::with(e.into(), include_pos))),
    };
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
    let parsed_id = manager.file_storage.lock().await.new_file(file_id);
    let i = span(&file_ctx, line_off_set);
    // get dir
    file_path.pop();
    #[cfg(test)]
    assert!(file_path.is_dir());

    let res = match ast(manager.clone(), loaded, file_path, i, false) {
        Ok((_, (res, _))) => res,
        Err(e) => error2ast(e.into()),
    };
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
