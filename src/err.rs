use crate::span::{FileId, LocatedSpan, Pos, Span};

use core::fmt;
use indexmap::IndexMap;
use nom::error::ErrorKind;
use nu_ansi_term::{Color, Style};
use std::{io, path::PathBuf};

impl From<nom::Err<nom::error::Error<LocatedSpan<'_>>>> for ParseError {
    #[inline]
    fn from(e: nom::Err<nom::error::Error<LocatedSpan<'_>>>) -> Self {
        match e {
            nom::Err::Incomplete(_) => unreachable!(),
            nom::Err::Failure(e) | nom::Err::Error(e) => {
                ParseErrorInner::Nom(e.code).record(e.input)
            }
        }
    }
}
impl From<io::Error> for ParseError {
    fn from(value: io::Error) -> Self {
        Self {
            pos: None,
            err: ParseErrorInner::IO(value),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseErrorInner {
    #[error("{0}")]
    IO(#[from] std::io::Error),
    #[error("Can NOT find section [{section}] in file {path}")]
    NoLibSection { path: PathBuf, section: String },
    /// Nom Error
    #[error("Syntax error")]
    Nom(ErrorKind),
    /// Unknown dot-command; it is kept as-is in [`crate::ast::Unknwon`]
    #[error("{0:?}")]
    Unknown(Span),
    #[error("Circular definition")]
    CircularDefinition(IndexMap<FileId, Option<Pos>>, usize),
    /// Text left over after a statement was parsed
    #[error("Unexpected text at the end of the statement")]
    Trailing,
    /// A continuation line (`+ ...`) without a statement before it
    #[error("Continuation line without a statement")]
    OrphanContinuation,
    /// `.ends` outside of a `.subckt`; it is ignored
    #[error("`.ends` without a matching `.subckt`")]
    StrayEnds,
    /// `.subckt` that reaches the end of the file without `.ends`
    #[error("Missing `.ends`")]
    UnterminatedSubckt(Span),
    /// A subckt defined twice in the same scope; the later definition wins
    #[error("Duplicate definition of subckt `{0}`")]
    DuplicateSubckt(String),
    /// Valid syntax that this parser does not support yet
    #[error("{0}")]
    Unsupported(String),
    /// A parser task failed
    #[error("{0}")]
    Internal(String),
}

impl ParseErrorInner {
    pub fn record(self, i: LocatedSpan) -> ParseError {
        ParseError {
            pos: Pos::new(i),
            err: self,
        }
    }
    pub fn with(self, pos: Option<Pos>) -> ParseError {
        ParseError { pos, err: self }
    }
    /// Warnings are reported, but do not count as errors:
    /// nothing was dropped from the netlist.
    pub fn is_warning(&self) -> bool {
        matches!(
            self,
            Self::Unknown(_) | Self::StrayEnds | Self::DuplicateSubckt(_)
        )
    }
}
#[derive(Debug, Clone, Copy)]
struct Styles {
    msg: Style,
    typ: Style,
    err: Style,
}

#[derive(Debug)]
pub struct ParseError {
    pub pos: Option<Pos>,
    pub err: ParseErrorInner,
}

impl ParseError {
    #[inline]
    pub fn is_warning(&self) -> bool {
        self.err.is_warning()
    }
    /// Log this diagnostic. Errors set `has_err`; warnings do not.
    pub fn report(&self, has_err: &mut bool, file_id: &FileId, file: &str) {
        let warning = self.is_warning();
        if !warning {
            *has_err = true;
        }
        struct ReportDisplay<'a> {
            err: &'a ParseError,
            file_id: &'a FileId,
            file: &'a str,
        }
        impl fmt::Display for ReportDisplay<'_> {
            #[inline]
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                use super::builder::Builder as _;
                let styles = if colored::control::SHOULD_COLORIZE.should_colorize() {
                    Styles {
                        msg: Style::new().fg(Color::LightMagenta),
                        typ: Style::new().fg(Color::LightMagenta).bold(),
                        err: Style::new().fg(Color::LightRed).bold(),
                    }
                } else {
                    Styles {
                        msg: Style::new(),
                        typ: Style::new(),
                        err: Style::new(),
                    }
                };
                write!(
                    f,
                    "\nFile {}\"{}\"{}",
                    styles.msg.prefix(),
                    self.file_id.path().display(),
                    styles.msg.suffix()
                )?;
                if let Some(pos) = self.err.pos {
                    write!(
                        f,
                        ", line {}{}{}",
                        styles.msg.prefix(),
                        pos.line_num,
                        styles.msg.suffix()
                    )?;
                    if pos.start <= self.file.len() {
                        let span = unsafe {
                            LocatedSpan::new_from_raw_offset(
                                pos.start,
                                pos.line_num,
                                &self.file[pos.start..],
                                (),
                            )
                        };
                        if let Ok(s) = core::str::from_utf8(span.get_line_beginning()) {
                            write!(f, "\n{s}\n")?;
                            for _ in 0..span.get_column() - 1 {
                                write!(f, " ")?;
                            }
                            write!(f, "{}<-{}", styles.err.prefix(), styles.err.suffix())?;
                        }
                    }
                }
                writeln!(f)?;
                let (typ, msg) = match &self.err.err {
                    ParseErrorInner::IO(error) => ("Error", error.to_string()),
                    ParseErrorInner::NoLibSection { path, section } => (
                        "Error",
                        format!(
                            "Can NOT find section `{section}` in file \"{}\"",
                            path.display()
                        ),
                    ),
                    ParseErrorInner::Nom(e) => (
                        "SyntaxError",
                        format!("Can NOT parse this statement ({e:?}); it is skipped"),
                    ),
                    ParseErrorInner::Unknown(span) => (
                        "Warning",
                        format!("Unknown command `{}`, kept as-is", span.build(self.file)),
                    ),
                    ParseErrorInner::Trailing => (
                        "SyntaxError",
                        "Unexpected text at the end of the statement; the statement is skipped"
                            .to_owned(),
                    ),
                    ParseErrorInner::OrphanContinuation => (
                        "SyntaxError",
                        "Continuation line (`+`) without a statement; it is skipped".to_owned(),
                    ),
                    ParseErrorInner::StrayEnds => (
                        "Warning",
                        "`.ends` without a matching `.subckt`; it is ignored".to_owned(),
                    ),
                    ParseErrorInner::UnterminatedSubckt(name) => (
                        "Error",
                        format!(
                            "Missing `.ends` for `.subckt {}`; it ends at the end of the file",
                            name.build(self.file)
                        ),
                    ),
                    ParseErrorInner::DuplicateSubckt(name) => (
                        "Warning",
                        format!(
                            "Duplicate definition of subckt `{name}`; it replaces the earlier one"
                        ),
                    ),
                    ParseErrorInner::Unsupported(msg) => ("Error", msg.clone()),
                    ParseErrorInner::Internal(msg) => ("InternalError", msg.clone()),
                    ParseErrorInner::CircularDefinition(index_set, idx) => {
                        struct FileDisplay<'a>(&'a FileId, &'a Option<Pos>);
                        impl fmt::Display for FileDisplay<'_> {
                            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                                match self.0 {
                                    FileId::Include { path } => {
                                        write!(f, "File \"{}\"", path.display())?;
                                        if let Some(pos) = self.1 {
                                            write!(f, ", line {}", pos.line_num)?;
                                        }
                                        Ok(())
                                    }
                                    FileId::Section { path, section } => {
                                        write!(f, "File \"{}\"", path.display())?;
                                        if let Some(pos) = self.1 {
                                            write!(f, ", line {}", pos.line_num)?;
                                        }
                                        write!(f, ", section {section}")
                                    }
                                }
                            }
                        }
                        impl<'s> FileDisplay<'s> {
                            fn new(f: (&'s FileId, &'s Option<Pos>)) -> Self {
                                Self(f.0, f.1)
                            }
                        }
                        let circular_file = index_set.get_index(*idx).unwrap();
                        writeln!(
                            f,
                            "{}CircularDefinition{}: {}Detect circular definition in {}{}",
                            styles.typ.prefix(),
                            styles.typ.suffix(),
                            styles.msg.prefix(),
                            FileDisplay::new(circular_file),
                            styles.msg.suffix()
                        )?;
                        for (i, file) in index_set.iter().enumerate() {
                            if *idx == i {
                                writeln!(
                                    f,
                                    "{} * {}{}\n     ↓",
                                    styles.err.prefix(),
                                    FileDisplay::new(file),
                                    styles.err.suffix()
                                )?;
                            } else {
                                writeln!(f, "   {}\n     ↓", FileDisplay::new(file))?;
                            }
                        }
                        return writeln!(
                            f,
                            "{} * {}{}",
                            styles.err.prefix(),
                            FileDisplay::new(circular_file),
                            styles.err.suffix()
                        );
                    }
                };
                writeln!(
                    f,
                    "{}{typ}{}: {}{msg}{}",
                    styles.typ.prefix(),
                    styles.typ.suffix(),
                    styles.msg.prefix(),
                    styles.msg.suffix()
                )
            }
        }
        // (not inlined in the format string: `tracing`'s macros shadow some names)
        let report = ReportDisplay {
            err: self,
            file_id,
            file,
        };
        if warning {
            crate::warn!("{}", report)
        } else {
            crate::error!("{}", report)
        }
    }
}
