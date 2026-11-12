use crate::span::{FileId, LocatedSpan, Pos, Span};

use core::fmt;
use indexmap::IndexMap;
use nom::error::ErrorKind;
use std::{
    io::{self, IsTerminal as _},
    path::PathBuf,
    sync::LazyLock,
};

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
impl From<nom::Err<nom::error::Error<LocatedSpan<'_>>>> for ParseErrorInner {
    #[inline]
    fn from(e: nom::Err<nom::error::Error<LocatedSpan<'_>>>) -> Self {
        match e {
            nom::Err::Incomplete(_) => Self::Nom(ErrorKind::Complete),
            nom::Err::Failure(e) | nom::Err::Error(e) => Self::Nom(e.code),
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

#[derive(Debug)]
pub enum ParseErrorInner {
    IO(std::io::Error),
    NoLibSection {
        path: PathBuf,
        section: String,
    },
    /// Nom Error
    Nom(ErrorKind),
    /// `.endif`, `.else` or `.elseif` with no open `.if`
    StrayCond {
        keyword: &'static str,
    },
    /// `.else` or `.elseif` after the `.else` of a block
    AfterElse {
        keyword: &'static str,
    },
    /// An `.if` block with no `.endif` at the end of its level, at `.alter`, or
    /// at `.ends`
    MissingEndif,
    /// Unknown dot-command; it is kept as-is in [`crate::ast::Unknown`]
    Unknown(Span),
    CircularDefinition(IndexMap<FileId, Option<Pos>>, usize),
    /// A continuation line (`+ ...`) without a statement before it
    OrphanContinuation,
    /// A subckt defined twice in the same scope; the later definition wins
    DuplicateSubckt(String),
    /// Valid syntax that this parser does not support yet
    Unsupported(String),
    /// Syntax that is not HSPICE (e.g. a Spectre directive); it is ignored
    NonHspice(String),
    /// A parser task failed
    Internal(String),
    /// A block encrypted for HSPICE (`.prot freelib`), skipped
    Encrypted {
        lines: usize,
    },
    /// The file is not valid UTF-8; it was read anyway
    Encoding(String),
    /// The file (or `.lib` section) has lone CR line ends, which were read
    /// as line ends. `line` is the line that the first one ends.
    LoneCr {
        line: u32,
    },
    /// The first line of a top-level file, read as its title (as in HSPICE),
    /// is also a well-formed element
    TitleIsElement,
    /// A `.ends` (or `.eom`) with no open `.subckt` (or `.macro`)
    StrayEnds {
        /// `ends` or `eom`
        keyword: &'static str,
    },
    /// A `.subckt` (or `.macro`) body that reaches the end of its file
    /// without `.ends` (or `.eom`)
    MissingEnds {
        /// `subckt` or `macro`
        keyword: &'static str,
        subckt: String,
    },
    /// Text after a complete statement, on its line or on a `+` line
    TrailingText,
    /// An `.alter` line in a `.subckt` body
    AlterInSubckt,
    /// An include that goes past a limit (see `parser::IncludeLimits`): the include
    /// is skipped
    IncludeLimit(String),
    /// A `.prot` region that reaches the end of its file without `.unprot`
    MissingUnprot,
    /// A `.subckt` (or `.macro`) nested deeper than `limit` levels, also
    /// through `.include` lines. Its definition is skipped, so the parser
    /// and `Files::build` do not recurse further.
    NestingTooDeep {
        /// `subckt` or `macro`
        keyword: &'static str,
        subckt: String,
        limit: usize,
    },
    /// A `+` line right after the skipped title line (golden `title_plus`)
    PlusAfterTitle,
    /// A line inside quotes that is not a `+` line (golden `quoted_cmtline`,
    /// `quoted_blank_line`, `quoted_spaces_elem`, `quoted_dollarline_param`)
    LineInQuotes,
    /// An element with the wrong number of nodes before its model name
    NodeCount {
        /// `a MOSFET`, `a BJT`, `a diode`
        element: &'static str,
        /// `3 to 7`
        want: &'static str,
        found: usize,
    },
    /// An element line without its model name
    NoModel {
        element: &'static str,
    },
    /// An `X` line without a subcircuit name
    NoSubcktName,
    /// More numbers after the model name of an `M`, `D` or `Q` line, or
    /// after the value of an R, C or L line, than parameter names for them
    TooManyValues {
        /// `a MOSFET`, `a diode`, `a BJT`, `a resistor`
        element: &'static str,
        found: usize,
        /// The names of the numbers, in order: `l w ad as pd ps nrd nrs`
        names: &'static [&'static str],
        /// `its model name` or `its value`
        after: &'static str,
    },
    /// An R or C line with a value and then a model name, which HSPICE
    /// does not read in that order (golden `res_value_model`,
    /// `cap_value_model`; L reads it, golden `ind_value_model`)
    ValueThenModel {
        /// `a resistor`, `a capacitor`
        element: &'static str,
        model: String,
        /// What HSPICE does with the line
        what: &'static str,
    },
    /// A token that is not a number or a quoted expression where the
    /// numbers after the value of an R, C or L line go
    BadPositional {
        /// `a resistor`, `a capacitor`, `an inductor`
        element: &'static str,
        token: String,
        /// The names of the numbers, in order: `tc1 tc2`
        names: &'static [&'static str],
    },
    /// A model name that starts with a digit, on a `.model` line or in the
    /// place of the model name of an element line (golden
    /// `mos_digit_model`, `mos_digit_model_ref`). On an R, C or L line, it
    /// fires only for a word right after the value with no model name
    /// before it (`after_value`): for R and C that is the place of `tc1`,
    /// where HSPICE reads no model name (golden `res_value_model`,
    /// `cap_value_model`, `res_tc_positional`, `cl_tc_positional`), and
    /// for L the place of the model name (golden `ind_value_model`). After
    /// a model name and a value, and in the place of `tc2`, the letters
    /// after a number are ignored, so no word there is this error.
    DigitFirstModel {
        /// `a MOSFET`, `a resistor`, ``a `.model` ``
        element: &'static str,
        name: String,
        /// The place can also hold a number (an element line): a number
        /// with other letters after its scale (`5vnmos`, `10pF`) is read
        /// as a name there.
        or_number: bool,
        /// The place is right after the value of an R, C or L line, with
        /// no model name before it.
        after_value: bool,
    },
    /// The first token after the nodes of an R, C or L line, before its
    /// value or `r=`, where a model name goes, that is not a model name:
    /// it starts with a digit (`10pF`, `5vnmos`; HSPICE rejects such a
    /// model name, golden `mos_digit_model`), or it has a character that
    /// model names do not have (`rmod-1`)
    NotModelName {
        /// `a resistor`, `a capacitor`, `an inductor`
        element: &'static str,
        token: String,
        /// The token starts with a digit (else it has a character that
        /// model names do not have)
        digit_first: bool,
    },
    /// A V or I line whose source parts are not in a form that HSPICE
    /// reads (golden `src_order_fn_first`)
    BadSource {
        /// `a voltage source`, `a current source`
        element: &'static str,
        /// What is wrong, and what to write
        what: &'static str,
    },
    /// An R, C or L line without a value and without parameters
    NoValue {
        /// `a resistor`, `a capacitor`, `an inductor`
        element: &'static str,
        /// `r`, `c`, `l`
        key: &'static str,
    },
    /// An R, C or L line with text after its nodes that is not a value, a
    /// model name or a `key=value` parameter (`R1 a b {x}`)
    BadValue {
        /// `a resistor`, `a capacitor`, `an inductor`
        element: &'static str,
    },
}

impl fmt::Display for ParseErrorInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IO(e) => write!(f, "{e}"),
            Self::NoLibSection { path, section } => write!(
                f,
                "Can NOT find section [{section}] in file {}",
                path.display()
            ),
            Self::Nom(..) => write!(f, "Syntax error"),
            Self::StrayCond { keyword } => write!(f, "`.{keyword}` with no open `.if`"),
            Self::AfterElse { keyword } => write!(f, "`.{keyword}` after the `.else` of the block"),
            Self::MissingEndif => write!(f, "`.endif` is missing"),
            Self::Unknown(span) => write!(f, "{span:?}"),
            Self::CircularDefinition(..) => write!(f, "Circular definition"),
            Self::OrphanContinuation => write!(f, "Continuation line without a statement"),
            Self::DuplicateSubckt(name) => write!(f, "Duplicate definition of subckt `{name}`"),
            Self::Unsupported(msg) => write!(f, "{msg}"),
            Self::NonHspice(msg) => write!(f, "{msg}"),
            Self::Internal(msg) => write!(f, "{msg}"),
            Self::Encrypted { lines } => write!(f, "Encrypted block of {lines} line(s) skipped"),
            Self::Encoding(msg) => write!(f, "{msg}"),
            Self::LoneCr { line } => write!(
                f,
                "Lone CR line ends were read as line ends (the first one ends line {line}). \
         HSPICE on Windows reads a lone CR as a space: to it these lines are one line, \
         and a file with only CR line ends is one title line"
            ),
            Self::TitleIsElement => write!(f, "The title line is also an element"),
            Self::StrayEnds { keyword } => {
                write!(f, "`.{keyword}` without an open `.subckt` or `.macro`")
            }
            Self::MissingEnds { keyword, subckt } => write!(
                f,
                "`.{keyword} {subckt}` has no `.{}`",
                end_keyword(keyword)
            ),
            Self::TrailingText => write!(f, "text after the end of the statement"),
            Self::AlterInSubckt => write!(f, "`.alter` inside a `.subckt`"),
            Self::IncludeLimit(msg) => write!(f, "{msg}"),
            Self::MissingUnprot => write!(f, "`.prot` has no `.unprot`"),
            Self::NestingTooDeep {
                keyword,
                subckt,
                limit,
            } => write!(
                f,
                "`.{keyword} {subckt}` is nested deeper than {limit} levels: its definition is skipped"
            ),
            Self::PlusAfterTitle => write!(
                f,
                "a `+` line right after the title line: HSPICE rejects the deck"
            ),
            Self::LineInQuotes => write!(
                f,
                "a line inside quotes is not a `+` line (a comment line or a blank line): \
         HSPICE rejects the deck"
            ),
            Self::NodeCount {
                element,
                want,
                found,
            } => write!(
                f,
                "{element} needs {want} nodes before its model name, but this line has {found}"
            ),
            Self::NoModel { element } => write!(f, "{element} line has no model name"),
            Self::NoSubcktName => write!(f, "an `X` line has no subcircuit name"),
            Self::TooManyValues {
                element,
                found,
                names,
                after,
            } => write!(
                f,
                "{element} line has {found} numbers after {after}, but at most {} {} read ({})",
                names.len(),
                if names.len() == 1 { "is" } else { "are" },
                names.join(" ")
            ),
            Self::ValueThenModel {
                element,
                model,
                what,
            } => write!(
                f,
                "{element} line has a value and then a model name (`{model}`): HSPICE {what}. Write the model name first"
            ),
            Self::BadPositional {
                element,
                token,
                names,
            } => write!(
                f,
                "{element} line has `{token}` after its value, where only numbers or quoted expressions ({}) and `key=value` parameters may follow",
                names.join(" ")
            ),
            Self::DigitFirstModel {
                element,
                name,
                or_number,
                after_value,
            } => write!(
                f,
                "{element} line has `{name}` {}{}",
                if !*after_value {
                    "in the place of its model name: HSPICE rejects a model name that starts with a digit"
                } else if *element == "an inductor" {
                    "right after its value, with no model name before it: L takes \"value, then model\" there, and HSPICE rejects a model name that starts with a digit"
                } else {
                    "right after its value, with no model name before it: R and C take \"model, then value\", so it is not a model name. Right after a value with no model name before it, the place of `tc1`, a number has no letters after its scale (`2u`, not `2umos`). After a model name and a value, the letters after a number are ignored"
                },
                if *or_number && (!*after_value || *element == "an inductor") {
                    " (a number in this place has no letters after its scale: `2u`, not `2umos`)"
                } else {
                    ""
                }
            ),
            Self::NotModelName {
                element,
                token,
                digit_first,
            } => write!(
                f,
                "{element} line has `{token}` before its value: {}, so `{token}` is in the place of a model name, {}",
                if *element == "an inductor" {
                    "L takes \"model, then value\" or \"value, then model\""
                } else {
                    "R and C take \"model, then value\""
                },
                if *digit_first {
                    "and HSPICE rejects a model name that starts with a digit. If it is the value, write it as one number with no letters after its scale (`10p`, not `10pF`)"
                } else {
                    "but a model name has only letters, digits, `_`, `.` and `:`. If it is the value, write it in quotes (`'...'`)"
                }
            ),
            Self::BadSource { element, what } => write!(f, "{element} line: {what}"),
            Self::NoValue { element, key } => write!(
                f,
                "{element} line has no value: write it after the two nodes, or as `{key}=`"
            ),
            Self::BadValue { element } => write!(
                f,
                "{element} line has text after its two nodes that is not a value, a model name or a `key=value` parameter"
            ),
        }
    }
}

impl std::error::Error for ParseErrorInner {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IO(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ParseErrorInner {
    #[inline]
    fn from(e: io::Error) -> Self {
        Self::IO(e)
    }
}

/// The keyword that usually closes a definition that `keyword` (`subckt`
/// or `macro`) opens. The two pairs mix: `.ends` also closes a `.macro`.
fn end_keyword(keyword: &str) -> &'static str {
    if keyword == "macro" { "eom" } else { "ends" }
}

/// `macro` when the definition whose name starts at byte `name_start` of
/// `file` is a `.macro` (the keyword is before the name on its line), else
/// `subckt`.
pub(crate) fn def_keyword(file: &str, name_start: usize) -> &'static str {
    let line = &file[file[..name_start].rfind('\n').map_or(0, |n| n + 1)..name_start];
    let is_macro = line
        .trim()
        .strip_prefix('.')
        .is_some_and(|k| k.eq_ignore_ascii_case("macro"));
    if is_macro { "macro" } else { "subckt" }
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
            Self::Unknown(_)
                | Self::DuplicateSubckt(_)
                | Self::NonHspice(_)
                | Self::Encrypted { .. }
                | Self::Encoding(_)
                | Self::LoneCr { .. }
                | Self::TitleIsElement
        )
    }
}
/// The ANSI codes that turn a text style on and off
#[derive(Debug, Clone, Copy)]
struct Style {
    prefix: &'static str,
    suffix: &'static str,
}

impl Style {
    const PLAIN: Self = Self {
        prefix: "",
        suffix: "",
    };
    const LIGHT_MAGENTA: Self = Self::new("\x1b[95m");
    const BOLD_LIGHT_MAGENTA: Self = Self::new("\x1b[1;95m");
    const BOLD_LIGHT_RED: Self = Self::new("\x1b[1;91m");
    const fn new(prefix: &'static str) -> Self {
        Self {
            prefix,
            suffix: "\x1b[0m",
        }
    }
    const fn prefix(&self) -> &'static str {
        self.prefix
    }
    const fn suffix(&self) -> &'static str {
        self.suffix
    }
}

#[derive(Debug, Clone, Copy)]
struct Styles {
    msg: Style,
    typ: Style,
    err: Style,
}

/// Whether reports have colors. The environment is read once: `CLICOLOR_FORCE`
/// (set, and not `0`) turns colors on, else `NO_COLOR` (set) turns them off,
/// else they are on when stdout is a terminal and `CLICOLOR` is not `0`.
fn should_colorize() -> bool {
    static COLORIZE: LazyLock<bool> = LazyLock::new(|| {
        let set = |name: &str| std::env::var(name).ok().map(|value| value != "0");
        if set("CLICOLOR_FORCE") == Some(true) {
            true
        } else if set("NO_COLOR").is_some() {
            false
        } else {
            set("CLICOLOR").unwrap_or(true) && io::stdout().is_terminal()
        }
    });
    *COLORIZE
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
                let styles = if should_colorize() {
                    Styles {
                        msg: Style::LIGHT_MAGENTA,
                        typ: Style::BOLD_LIGHT_MAGENTA,
                        err: Style::BOLD_LIGHT_RED,
                    }
                } else {
                    Styles {
                        msg: Style::PLAIN,
                        typ: Style::PLAIN,
                        err: Style::PLAIN,
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
                    ParseErrorInner::StrayCond { keyword } => (
                        "SyntaxError",
                        format!("`.{keyword}` with no open `.if`; the line is skipped"),
                    ),
                    ParseErrorInner::AfterElse { keyword } => (
                        "SyntaxError",
                        format!("`.{keyword}` after the `.else` of the block; the line is skipped"),
                    ),
                    ParseErrorInner::MissingEndif => (
                        "SyntaxError",
                        "An `.if` block has no `.endif` before this point; it ends here".to_owned(),
                    ),
                    ParseErrorInner::Unknown(span) => (
                        "Warning",
                        format!("Unknown command `{}`, kept as-is", span.build(self.file)),
                    ),
                    ParseErrorInner::OrphanContinuation => (
                        "SyntaxError",
                        "Continuation line (`+`) without a statement; it is skipped".to_owned(),
                    ),
                    ParseErrorInner::DuplicateSubckt(name) => (
                        "Warning",
                        format!(
                            "Duplicate definition of subckt `{name}`; it replaces the earlier one"
                        ),
                    ),
                    ParseErrorInner::Unsupported(msg) | ParseErrorInner::IncludeLimit(msg) => {
                        ("Error", msg.clone())
                    }
                    ParseErrorInner::NonHspice(msg) => ("Warning", msg.clone()),
                    ParseErrorInner::Internal(msg) => ("InternalError", msg.clone()),
                    ParseErrorInner::Encrypted { lines } => (
                        "Warning",
                        format!(
                            "Encrypted block (`.prot freelib`) of {lines} line(s) skipped: \
                             only HSPICE can read it"
                        ),
                    ),
                    ParseErrorInner::Encoding(msg) => ("Warning", msg.clone()),
                    ParseErrorInner::LoneCr { .. } => ("Warning", self.err.err.to_string()),
                    ParseErrorInner::TitleIsElement => (
                        "Warning",
                        "HSPICE reads the first line of the netlist as its title, so this \
                         element is not part of the circuit; start the file with a comment \
                         line (`*`) if it should be"
                            .to_owned(),
                    ),
                    ParseErrorInner::StrayEnds { .. }
                    | ParseErrorInner::MissingEnds { .. }
                    | ParseErrorInner::TrailingText
                    | ParseErrorInner::AlterInSubckt
                    | ParseErrorInner::MissingUnprot
                    | ParseErrorInner::NestingTooDeep { .. }
                    | ParseErrorInner::PlusAfterTitle
                    | ParseErrorInner::LineInQuotes
                    | ParseErrorInner::NodeCount { .. }
                    | ParseErrorInner::NoModel { .. }
                    | ParseErrorInner::NoSubcktName
                    | ParseErrorInner::TooManyValues { .. }
                    | ParseErrorInner::ValueThenModel { .. }
                    | ParseErrorInner::BadPositional { .. }
                    | ParseErrorInner::DigitFirstModel { .. }
                    | ParseErrorInner::NotModelName { .. }
                    | ParseErrorInner::BadSource { .. }
                    | ParseErrorInner::NoValue { .. }
                    | ParseErrorInner::BadValue { .. } => ("SyntaxError", self.err.err.to_string()),
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

#[cfg(test)]
mod tests {
    use super::{ParseErrorInner, def_keyword};

    #[test]
    fn macro_keyword_in_messages() {
        // The keyword before a definition name, found from the file text
        // (for an error that `Files::build` reports).
        let file = ".subckt a x\n  .MACRO\tb x\n";
        let at = |name: &str| file.find(&format!("{name} x")).unwrap();
        assert_eq!(def_keyword(file, at("a")), "subckt");
        assert_eq!(def_keyword(file, at("b")), "macro");
        let msg = ParseErrorInner::NestingTooDeep {
            keyword: "macro",
            subckt: "b".to_owned(),
            limit: 256,
        }
        .to_string();
        assert_eq!(
            msg,
            "`.macro b` is nested deeper than 256 levels: its definition is skipped"
        );
    }
}
