//! The statements of a netlist: the dot commands, the lines of a level, and `.subckt` headers.
use regex::Regex;
use std::{path::Path, sync::LazyLock};

use nom::{
    IResult, Input, Parser,
    branch::alt,
    bytes::complete::tag_no_case,
    character::{char, complete::one_of},
    combinator::{map, map_res, opt},
    error::ErrorKind,
    multi::{many0, many1},
    sequence::preceded,
};

use crate::{
    ast::LocalAST,
    err::{ParseError, ParseErrorInner},
    instance::InstanceCtxBuilder,
    parser::{
        cmds::{init_condition, nodeset},
        instance::{instance, unsupported_controlled_form},
    },
};
use crate::{
    ast::{
        CondLine, DataBuilder, DataFileBuilder, DataFilesBuilder, DataValuesBuilder,
        KeyValueBuilder, ModelBuilder, PnameColNumBuilder, UnknwonBuilder, ValueBuilder,
    },
    span::{LocatedSpan, Pos, Span},
};

use super::blocks::{Block, Blocks};
use super::tokens::{
    ParamItem, assigned_value, comment_space_newline, continuation, equal, float_unit, integer,
    is_key, key, key_str, key_value, list_sep, loss_sep, many0_dummyfirst, multiline_sep, name,
    name_str, node_sep, option, param_item, path, path_str, raw_span, space, space1, span_text,
    token, trailing_commas, trailing_delims, value,
};

#[inline]
pub(super) fn ports_params(
    i: LocatedSpan,
) -> IResult<LocatedSpan, (Vec<Span>, Vec<KeyValueBuilder>)> {
    map(
        (
            many1(preceded(node_sep, name)),
            opt((
                equal(assigned_value),
                many0_dummyfirst(preceded(node_sep, key_value)),
            )),
            opt(trailing_delims),
        ),
        |(mut ports, _params, _)| match _params {
            Some((first_value, mut params)) => {
                let first_key = ports.pop().unwrap();
                params[0] = KeyValueBuilder {
                    k: first_key,
                    v: first_value,
                };
                (ports, params)
            }
            None => (ports, Vec::new()),
        },
    )
    .parse_complete(i)
}
/// Between the items of a `.data` block: white space, comments and line
/// breaks. When the block ends with `.enddata` (`multi_line`), its rows do not
/// need a leading `+` (HSPICE reads them up to `.enddata`); otherwise only `+`
/// continuation lines belong to it.
fn data_sep(i: LocatedSpan, multi_line: bool) -> IResult<LocatedSpan, ()> {
    if !multi_line {
        return alt((continuation, map(space1, |_| ()))).parse_complete(i);
    }
    let s: &str = i.fragment();
    let mut idx = 0;
    let mut line_start = false;
    while let Some(c) = s[idx..].chars().next() {
        match c {
            ' ' | '\t' | '\r' => idx += 1,
            '\n' => {
                idx += 1;
                line_start = true;
            }
            '+' if line_start => {
                idx += 1;
                line_start = false;
            }
            '*' | '$' => idx += s[idx..].find('\n').unwrap_or(s.len() - idx),
            _ => break,
        }
    }
    if idx == 0 {
        Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Space)))
    } else {
        Ok((i.take_from(idx), ()))
    }
}

/// A separator of a `.data` block
type DataSep = for<'a> fn(LocatedSpan<'a>) -> IResult<LocatedSpan<'a>, ()>;

/// Whether `.enddata` closes the `.data` block at `i` (before any other `.data`)
fn has_enddata(i: LocatedSpan) -> bool {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?im)^[ \t+]*\.(endd(?:ata|at|a)?|dat?a?)\b").unwrap());
    RE.captures(i.fragment())
        .is_some_and(|caps| caps[1].to_ascii_lowercase().starts_with("end"))
}

#[inline]
pub(super) fn enddata(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    map_res((char('.'), key_str), |(_, (key, _))| {
        if canonical_cmd(&key.to_ascii_lowercase()) == "enddata" {
            Ok(())
        } else {
            Err(())
        }
    })
    .parse_complete(i)
}

/// ``` spice
/// .DATA name pname1 pname2 ...
/// + val11 val12 ...
/// .ENDDATA
/// .DATA name MER|LAM
/// + FILE='file1' p1=1 p2=3 [OUT='out']
/// .ENDDATA
/// ```
/// The rows may also be written without `+`.
#[inline]
pub(super) fn data(i: LocatedSpan) -> IResult<LocatedSpan, DataBuilder> {
    let sep: DataSep = if has_enddata(i) {
        |i| data_sep(i, true)
    } else {
        |i| data_sep(i, false)
    };
    #[inline]
    fn data_files(i: LocatedSpan, sep: DataSep) -> IResult<LocatedSpan, DataFilesBuilder> {
        let keyed = move |key: &'static str| {
            map_res(
                (preceded(sep, name_str), equal(path)),
                move |((k, _), path)| {
                    if k.eq_ignore_ascii_case(key) {
                        Ok(path)
                    } else {
                        Err(())
                    }
                },
            )
        };
        let pname_col_num = map_res(
            (preceded(sep, name_str), equal(integer)),
            |((pname_str, pname), col_num)| {
                if pname_str.eq_ignore_ascii_case("FILE") || pname_str.eq_ignore_ascii_case("OUT") {
                    Err(())
                } else {
                    Ok(PnameColNumBuilder { pname, col_num })
                }
            },
        );
        map(
            (
                many1(map(
                    (keyed("FILE"), many1(pname_col_num)),
                    |(file, pname_col_num)| DataFileBuilder {
                        file,
                        pname_col_num,
                    },
                )),
                opt(keyed("OUT")),
                sep,
                enddata,
            ),
            |(files, out, _, _)| DataFilesBuilder { files, out },
        )
        .parse_complete(i)
    }
    // (the name is on the `.data` line)
    let (i, name) = preceded(|i| data_sep(i, false), key).parse_complete(i)?;
    let (i, (first_str, first)) = preceded(sep, name_str).parse_complete(i)?;
    if first_str.starts_with('.') {
        // a command, not a parameter
        return Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Tag)));
    }
    if first_str.eq_ignore_ascii_case("MER") || first_str.eq_ignore_ascii_case("LAM") {
        let (i, files) = data_files(i, sep)?;
        let values = if first_str.eq_ignore_ascii_case("MER") {
            DataValuesBuilder::MER(files)
        } else {
            DataValuesBuilder::LAM(files)
        };
        return Ok((i, DataBuilder { name, values }));
    }
    // the parameter names, up to the first number (or `DATAFORM`)
    let mut params = vec![first];
    let mut end = i;
    let mut dataform = false;
    while let Ok((j, ())) = sep(end) {
        if j.fragment().starts_with('.') || float_unit(j).is_ok() {
            break;
        }
        let Ok((k, (param_str, param))) = name_str(j) else {
            break;
        };
        end = k;
        if param_str.eq_ignore_ascii_case("DATAFORM") {
            dataform = true;
            break;
        }
        params.push(param);
    }
    // the values, up to `.enddata`
    let mut nums = Vec::new();
    let mut exprs = Vec::new();
    while let Ok((j, ())) = sep(end) {
        if let Ok((k, ())) = enddata(j) {
            end = k;
            break;
        }
        if dataform {
            let Ok((k, v)) = value(j) else { break };
            exprs.push(v);
            end = k;
        } else {
            let Ok((k, v)) = float_unit(j) else { break };
            nums.push(v);
            end = k;
        }
    }
    let values = if dataform {
        DataValuesBuilder::InlineExpr {
            params,
            values: exprs,
        }
    } else {
        DataValuesBuilder::InlineNum {
            params,
            values: nums,
        }
    };
    Ok((end, DataBuilder { name, values }))
}

/// ``` spice
/// .MODEL mname ModelType ([level=val]
/// + [keyword1=val1][keyword2=val2]
/// + [keyword3=val3][LOT distribution value]
/// + [DEV distribution value]...)
/// ```
/// The parameters are optional: `.model rmod r` (golden `res_model`).
#[inline]
pub(super) fn model(i: LocatedSpan) -> IResult<LocatedSpan, ModelBuilder> {
    map(
        (
            // binned model names have dots: `nch.1`
            multiline_sep(name),
            multiline_sep(key_str),
            // the parameters may be in parentheses and separated by commas
            // (HSPICE delimiters); there may be none (`.model dmod d`)
            many0(preceded(node_sep, key_value)),
            opt(trailing_delims),
        ),
        |(name, model_type_ctx, params, _)| ModelBuilder {
            name,
            model_type: model_type_ctx.into(),
            params,
        },
    )
    .parse_complete(i)
}

/// Skips a `.lib <section>` definition (`i` is after `.lib `): up to and
/// including the `.endl` keyword of the first later line whose first token
/// (after spaces) is `.endl`, in any letter case. A `.endl` in a comment
/// or in a longer word (`.endlx`) does not end the section.
#[inline]
fn endlib(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    let s = i.fragment();
    let mut from = 0;
    while let Some(n) = s[from..].find('\n') {
        let line = from + n + 1;
        let text = s[line..].trim_start_matches([' ', '\t']);
        if let Some(rest) = text.strip_prefix('.')
            && rest
                .get(..4)
                .is_some_and(|k| k.eq_ignore_ascii_case("endl"))
            && !rest[4..].starts_with(is_key)
        {
            let end = s.len() - rest.len() + 4;
            return Ok((i.take_from(end), ()));
        }
        from = line;
    }
    Err(nom::Err::Error(nom::error::Error::new(
        i,
        ErrorKind::RegexpCapture,
    )))
}
pub(super) fn lib(i: LocatedSpan<'_>) -> IResult<LocatedSpan<'_>, Option<(&str, String)>> {
    alt((
        // include lib section (`.lib "file"tt` needs no space after the quote)
        map(
            (path_str, loss_sep, key_str),
            |(path_str, _, (section_str, _))| Some((path_str, section_str.to_lowercase())),
        ),
        // skip to `.endl`
        map((endlib, opt((space1, key))), |_| None),
    ))
    .parse_complete(i)
}
/// HSPICE commands kept verbatim (in [`LocalAST::unknwon`]) without a warning,
/// including the short forms `.mea`, `.prot` and `.unpr` used in vendor model
/// kits. Other unknown commands are kept as well, with a warning.
const KNOWN_COMMANDS: &[&str] = &[
    "ac",
    "acphasenoise",
    "alias",
    "alter",
    "appendmodel",
    "biaschk",
    "check",
    "connect",
    "dc",
    "dcmatch",
    "dcvolt",
    "del",
    "dellib",
    "design_exploration",
    "disto",
    "dout",
    "ebd",
    "element_variation",
    "end_element_variation",
    "end_global_variation",
    "end_local_variation",
    "end_variation",
    "envelope",
    "fft",
    "four",
    "fsoptions",
    "func",
    "global",
    "global_variation",
    "graph",
    "hb",
    "hbac",
    "hblin",
    "hblsp",
    "hbnoise",
    "hbosc",
    "hbxf",
    "hdl",
    "ibis",
    "icm",
    "if",
    "layerstack",
    "lin",
    "load",
    "local_variation",
    "lprint",
    "lstb",
    "malias",
    "material",
    "mea",
    "meas",
    "measure",
    "mosra",
    "mosraprint",
    "noise",
    "op",
    "pat",
    "pkg",
    "plot",
    "power",
    "powerdc",
    "print",
    "probe",
    "prot",
    "protect",
    "pz",
    "sample",
    "save",
    "sens",
    "shape",
    "sn",
    "snac",
    "snft",
    "snnoise",
    "snosc",
    "snxf",
    "stateye",
    "stim",
    "surge",
    "sweepblock",
    "temp",
    "temperature",
    "tf",
    "title",
    "tran",
    "trannoise",
    "unpr",
    "unprot",
    "unprotect",
    "variation",
    "vec",
    "verilog",
    "width",
];

/// The commands whose short forms HSPICE reads: the word, the shortest start of
/// it that names it, and the end of its form (a longer word is another word).
/// (Measured, decks `ab_*`: `.subc` is `.subckt`, `.subckts` is an unknown
/// command.)
const SHORT_FORMS: &[(&str, usize, &str)] = &[
    ("alter", 3, "alter"),
    ("data", 2, "data"),
    ("else", 3, "else"),
    ("elseif", 5, "elseif"),
    ("enddata", 4, "enddata"),
    ("endif", 4, "endif"),
    ("include", 2, "include"),
    ("lib", 2, "lib"),
    ("macro", 2, "macro"),
    ("model", 2, "model"),
    ("option", 3, "options"),
    ("param", 2, "parameters"),
    ("protect", 4, "protect"),
    ("subckt", 2, "subckt"),
    ("temp", 2, "temperatures"),
    ("unprot", 2, "unprotects"),
];

/// The command that `cmd`, a lowercase dot-command, stands for
pub fn canonical_cmd(cmd: &str) -> &str {
    SHORT_FORMS
        .iter()
        .find(|(_, shortest, longest)| cmd.len() >= *shortest && longest.starts_with(cmd))
        .map_or(cmd, |(word, ..)| word)
}

/// The `.subckt` body that a statement is in.
#[derive(Debug, Clone, Copy)]
pub struct Body<'a> {
    /// The start of the `.subckt` statement (the `.` of `.subckt`).
    pub(super) header: LocatedSpan<'a>,
    /// The subckt name, as written.
    pub(super) name: &'a str,
    /// `subckt` or `macro`: the keyword of the header.
    pub(super) keyword: &'static str,
    /// The number of open `.subckt` bodies, this one included
    /// (1 for a definition at file level).
    pub(super) depth: usize,
}

/// The deepest `.subckt` nesting that is parsed: a definition at file
/// level is level 1. A deeper `.subckt` is one `NestingTooDeep` error, and
/// its definition is skipped. The parser keeps the open bodies in a list,
/// not on the call stack, but the builders nest one level for each body,
/// and the code that walks them recurses.
///
/// The parser counts the levels in each file. An `.include` inside a body
/// puts the included file in that body (golden `include_in_subckt`), so a
/// chain of includes can nest deeper: a walker that follows
/// `Segment::Include` must count the levels across includes and stop at
/// this limit, as `Files::build` does.
pub const MAX_NESTING: usize = 256;

/// A `.subckt` (or `.macro`) header whose body is being parsed.
pub(super) struct Open<'a> {
    pub(super) body: Body<'a>,
    pub(super) name: Span,
    pub(super) ports: Vec<Span>,
    pub(super) params: Vec<KeyValueBuilder>,
}

/// What a statement does to the parse, besides what it adds to the
/// `LocalAST`.
pub(super) enum Step<'a> {
    /// A `.subckt` (or `.macro`) header: the statements after it are its
    /// body.
    Open(Open<'a>),
    /// `.ends` (or `.eom`): the body ends. `end` is the start of the line
    /// after the `.ends` statement and its `+` lines.
    Close { end: usize },
    /// `.include`, `.inc` or `.lib <file> <section>`: the included
    /// statements come here. The input is at the end of the include line.
    Include {
        file_name: &'a Path,
        section: Option<String>,
    },
    /// An `.alter` line (at `pos`): the statements after it belong to an
    /// `.alter` run. `stmt` keeps the text from this line to the end of the
    /// file.
    Alter {
        pos: Option<Pos>,
        stmt: UnknwonBuilder,
    },
    /// An `.end` line (at `pos`). HSPICE does not read the lines after the
    /// `.end` of the top file (golden `after_end`).
    End { pos: Option<Pos> },
    /// An `.if`, `.elseif`, `.else` or `.endif` line (at `pos`)
    Cond { line: CondLine, pos: Option<Pos> },
    /// The end of the input.
    Eof,
}

/// The end of the line of `i` (before its newline)
#[inline]
fn line_end(i: LocatedSpan) -> LocatedSpan {
    let n = i.fragment().find('\n').unwrap_or(i.fragment().len());
    i.take_from(n)
}

/// Parses the header of a `.subckt` (or `.macro`) statement after its
/// keyword. `header` is the start of the statement (the `.` of `.subckt`),
/// `keyword` is `subckt` or `macro`, and `parent` is the body that the
/// statement is in. Fails when the header fails, or when the definition is
/// nested deeper than `MAX_NESTING` levels.
#[inline]
pub(super) fn subckt_header<'a>(
    i: LocatedSpan<'a>,
    header: LocatedSpan<'a>,
    keyword: &'static str,
    parent: Option<Body<'a>>,
) -> Result<(LocatedSpan<'a>, Open<'a>), ParseErrorInner> {
    let (i, (_, (name_str, name), ports_params)) =
        (node_sep, name_str, opt(ports_params)).parse_complete(i)?;
    let (ports, params) = ports_params.unwrap_or_default();
    // The header must use its whole logical line: text that is left over
    // would otherwise change the ports or become part of the body.
    let i = statement_end(i)?;
    let depth = parent.map_or(1, |p| p.depth + 1);
    if depth > MAX_NESTING {
        return Err(ParseErrorInner::NestingTooDeep {
            keyword,
            subckt: name_str.to_owned(),
            limit: MAX_NESTING,
        });
    }
    let body = Body {
        header,
        name: name_str,
        keyword,
        depth,
    };
    Ok((
        i,
        Open {
            body,
            name,
            ports,
            params,
        },
    ))
}

/// `simulator lang=spice|spectre`: a Spectre directive, not HSPICE syntax,
/// found in model files shared with Spectre. `spice` is ignored with a
/// warning (the rest of its line too); a section in another language is
/// reported and skipped up to the next `simulator lang=spice`, or to the end
/// of the input. Returns the input after it, and the report.
fn simulator_lang(i: LocatedSpan) -> Option<(LocatedSpan, ParseError)> {
    let (rest, (_, _, _, _, _, _, (lang, _))) = (
        tag_no_case("simulator"),
        space1,
        tag_no_case("lang"),
        space,
        char('='),
        space,
        key_str,
    )
        .parse_complete(i)
        .ok()?;
    let rest = line_end(rest);
    if lang.eq_ignore_ascii_case("spice") {
        let msg = "`simulator lang=spice` is a Spectre directive, not HSPICE; it is ignored";
        return Some((rest, ParseErrorInner::NonHspice(msg.to_owned()).record(i)));
    }
    static BACK_TO_SPICE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?im)^[ \t]*simulator[ \t]+lang[ \t]*=[ \t]*spice\b").unwrap()
    });
    let text: &str = rest.fragment();
    let (end, until) = match BACK_TO_SPICE.find(text) {
        // stop at the newline before that line, which is the next statement
        Some(m) => (
            rest.take_from(m.start() - 1),
            "up to the next `simulator lang=spice`",
        ),
        None => (rest.take_from(text.len()), "to the end of the file"),
    };
    let msg =
        format!("`simulator lang={lang}` sections (not HSPICE) are not supported; skipped {until}");
    Some((end, ParseErrorInner::Unsupported(msg).record(i)))
}

/// `.prot freelib` (or `.protect freelib`) starts a block encrypted for HSPICE,
/// which only HSPICE can read: it is skipped up to `.unprot`, with a warning.
/// With no `.unprot`, the block takes the rest of the file, and this is an error.
/// (A plain `.prot` block is not encrypted; it is parsed as usual.) `i` is
/// after the keyword. Returns the input after the block, and the report.
pub(super) fn encrypted_block(i: LocatedSpan) -> Option<(LocatedSpan, ParseError)> {
    let (rest, (_, (word, _))) = (space1, key_str).parse_complete(i).ok()?;
    if !word.eq_ignore_ascii_case("freelib") {
        return None;
    }
    let rest = line_end(rest);
    let text: &str = rest.fragment();
    let Some(line) = unprot_line(text) else {
        let report = ParseErrorInner::MissingUnprot.record(i);
        return Some((rest.take_from(text.len()), report));
    };
    // stop at the newline before `.unprot`, which is then parsed as usual
    let end = line - 1;
    let lines = text[..end].matches('\n').count();
    Some((
        rest.take_from(end),
        ParseErrorInner::Encrypted { lines }.record(i),
    ))
}

/// HSPICE reads the first line of the input netlist as its title. The first
/// line of a top-level file (with its `+` continuation lines) is its title,
/// unless it is blank, a comment or a dot-command. Returns the input after
/// the title, the title, and whether that line is also a well-formed element
/// (which HSPICE ignores as well; worth a warning).
pub(super) fn title_line(i: LocatedSpan) -> Option<(LocatedSpan, Span, bool)> {
    let text: &str = i.fragment();
    let line = text[..text.find('\n').unwrap_or(text.len())].trim();
    if line.is_empty() || line.starts_with(['*', '$', '.', '+']) {
        return None;
    }
    let rest = skip_statement(i);
    let (start, _) = space(i).ok()?;
    let element = line.starts_with(|c: char| "RCLVIMQDXEFGHKrclvimqdxefghk".contains(c))
        && instance(start).is_ok_and(|(end, inst)| {
            // (a title of plain words can read as a MOSFET whose L and W are
            // names of parameters: `mux 2 pra 1 com transmission gate`)
            let words = matches!(&inst.ctx, InstanceCtxBuilder::MOSFET(m)
                if !m.extra_nodes.is_empty()
                    || m.positional.iter().any(|(_, v)| matches!(v, ValueBuilder::Expr(_))));
            !words
                && statement_end(end)
                    .is_ok_and(|next| next.location_offset() >= rest.location_offset())
        });
    Some((rest, raw_span(i, rest), element))
}

/// Keeps the statement at `i` (after its keyword `cmd`) verbatim, with the
/// tokens that can be read from it. A command that is not in
/// [`KNOWN_COMMANDS`] also gives a warning. Returns the next statement.
fn keep_verbatim<'a>(
    i: LocatedSpan<'a>,
    cmd: Span,
    known: bool,
    ast: &mut LocalAST,
) -> LocatedSpan<'a> {
    let end = skip_statement(i);
    let raw = raw_span(i, end);
    let tokens = many0(multiline_sep(token))
        .parse_complete(i)
        .map(|(_, tokens)| tokens)
        .unwrap_or_default();
    if !known {
        ast.errors.push(ParseErrorInner::Unknown(cmd).record(i));
    }
    ast.unknwon.push(UnknwonBuilder { cmd, tokens, raw });
    skip_blank(end)
}

/// Parses statements into `ast` up to a statement that opens or closes a
/// body, includes a file, or is `.alter`, or up to the end of input. It
/// returns that step and the input after its statement.
///
/// `body` is the `.subckt` body that `ast` is in, or `None` at file level.
/// Only a body ends at `.ends`: at file level a `.ends` is an error.
///
/// It never fails: a statement that does not parse, or that parses only a
/// part of its logical line, becomes one error in `LocalAST::errors` (at the
/// start of that statement) and parsing resumes at the next logical line.
/// A failed block statement (`.subckt`, `.data`, `.lib <section>`) is
/// skipped up to and including its closing command, when it has one before
/// the `.end` of a top file (see `Blocks::skip`); else only its first
/// logical line is skipped, and its body is read here. A `.subckt` nested
/// too deep is skipped up to its `.ends`, or, when it has none, to the
/// `.end` of a top file or the end of input (see `Blocks::skip_nested`).
/// `blocks` holds the blocks of the file that `i` is in.
#[inline]
pub(super) fn local_ast<'a>(
    mut i: LocatedSpan<'a>,
    body: Option<Body<'a>>,
    blocks: &Blocks<'a>,
    ast: &mut LocalAST,
) -> (LocatedSpan<'a>, Step<'a>) {
    i = skip_blank(i);
    loop {
        if i.is_empty() {
            return (i, Step::Eof);
        }
        match statement(i, body, blocks.skip_prot, ast) {
            Ok((_i, None)) => {
                // `statement_end` already skipped the blank region after it
                debug_assert_eq!(skip_blank(_i).location_offset(), _i.location_offset());
                check_quoted_lines(i, _i, ast);
                i = _i;
            }
            Ok((_i, Some(step))) => {
                check_quoted_lines(i, _i, ast);
                return (_i, step);
            }
            Err(e) => {
                let too_deep = matches!(e, ParseErrorInner::NestingTooDeep { .. });
                ast.errors.push(e.record(i));
                i = skip_blank(match Block::at(i) {
                    Some(Block::Subckt) if too_deep => blocks.skip_nested(i),
                    Some(block) => blocks.skip(i, block, body),
                    None => skip_statement(i),
                });
            }
        }
    }
}

/// Records an error for the statement at `start`, which the parser read up to
/// `next`, if it has a line inside quotes that is not a `+` line. The statement
/// stays in `ast`: the query layer gives its own error for it (`quote_check`).
fn check_quoted_lines(start: LocatedSpan<'_>, next: LocatedSpan<'_>, ast: &mut LocalAST) {
    // (most statements are one line, so this costs them nothing)
    if next.location_line() <= start.location_line() + 1 {
        return;
    }
    let len = skip_statement(start).location_offset() - start.location_offset();
    if line_in_quotes(&start.fragment()[..len]) {
        ast.errors.push(ParseErrorInner::LineInQuotes.record(start));
    }
}

/// True when the text `s` of a statement has a line inside quotes that is not
/// a `+` line. Outside quotes, comment lines and blank lines are passed over
/// (golden `plus_after_cmtline`, `plus_after_blank`).
fn line_in_quotes(s: &str) -> bool {
    let mut quote = None;
    for (k, line) in s.lines().enumerate() {
        let line = line.trim_start();
        if k > 0 && !line.starts_with('+') {
            if quote.is_some() {
                return true;
            }
            continue;
        }
        for c in line.bytes() {
            match c {
                b'\'' | b'"' if quote.is_none() => quote = Some(c),
                b'$' if quote.is_none() => break,
                c if quote == Some(c) => quote = None,
                _ => {}
            }
        }
    }
    false
}

/// Parses one statement into `ast`.
/// Returns `Some(step)` when the statement ends this `LocalAST`.
/// `body` is the `.subckt` body that `ast` is part of.
/// `skip_prot` is `ParseOptions::skip_prot`.
///
/// A statement must use its whole logical line (continuation lines
/// included), apart from spaces and comments. A statement that stops before
/// that fails, so a partial record never goes into `ast`.
/// On error, `ast` is unchanged.
///
/// On success the input is at the start of the next statement, after the
/// spaces, comments and empty lines (an include statement leaves the input
/// at the end of its line, which is the position of the include).
#[inline]
pub(super) fn statement<'a>(
    mut i: LocatedSpan<'a>,
    body: Option<Body<'a>>,
    skip_prot: bool,
    ast: &mut LocalAST,
) -> Result<(LocatedSpan<'a>, Option<Step<'a>>), ParseErrorInner> {
    let start = i;
    if i.fragment().starts_with('+') {
        // a continuation line without a statement before it
        return Err(ParseErrorInner::OrphanContinuation);
    }
    match char('.').parse_complete(i) {
        Err(nom::Err::Error(_)) => {
            if let Some((rest, e)) = simulator_lang(i) {
                ast.errors.push(e);
                return Ok((skip_blank(rest), None));
            }
            if let Some((keyword, pos)) = unsupported_controlled_form(i) {
                ast.errors.push(
                    ParseErrorInner::Unsupported(format!(
                        "Controlled source form `{keyword}` is not supported yet; the statement \
                         is skipped"
                    ))
                    .record(pos),
                );
                return Ok((skip_blank(skip_statement(i)), None));
            }
            let inst;
            (i, inst) = instance(i)?;
            i = statement_end(i)?;
            ast.instance.push(inst);
        }
        Err(e) => return Err(e.into()),
        Ok((_i, _)) => {
            i = _i;
            let cmd;
            let cmd_str;
            (i, (cmd_str, cmd)) = key_str(i)?;
            let cmd_lower = cmd_str.to_lowercase();
            let cmd_lower = canonical_cmd(&cmd_lower);
            match cmd_lower {
                "lib" => {
                    let lib_info;
                    (i, (_, lib_info)) = (loss_sep, lib).parse_complete(i)?;
                    let next = statement_end(i)?;
                    if let Some((file_name_str, section_str)) = lib_info {
                        return Ok((
                            i,
                            Some(Step::Include {
                                file_name: Path::new(file_name_str),
                                section: Some(section_str),
                            }),
                        ));
                    }
                    i = next;
                }
                "include" => {
                    let file_name;
                    (i, (_, file_name)) = (loss_sep, path_str).parse_complete(i)?;
                    statement_end(i)?;
                    return Ok((
                        i,
                        Some(Step::Include {
                            file_name: Path::new(file_name),
                            section: None,
                        }),
                    ));
                }
                "model" => {
                    let _model;
                    (i, _model) = model(i)?;
                    i = statement_end(i)?;
                    let name = span_text(start, _model.name);
                    if name.starts_with(|c: char| c.is_ascii_digit()) {
                        // golden `mos_digit_model`
                        return Err(ParseErrorInner::DigitFirstModel {
                            element: "a `.model`",
                            name: name.to_owned(),
                            or_number: false,
                            after_value: false,
                        });
                    }
                    ast.model.push(_model);
                }
                // `.macro`/`.eom` are `.subckt`/`.ends` (golden `macro_eom`)
                kw @ ("subckt" | "macro") => {
                    let keyword = if kw == "macro" { "macro" } else { "subckt" };
                    // `subckt_header` checks the end of the header line, and
                    // the `.ends` arm below checks the end of the `.ends`
                    // line.
                    let (i, open) = subckt_header(i, start, keyword, body)?;
                    return Ok((i, Some(Step::Open(open))));
                }
                "data" => {
                    let _data;
                    (i, _data) = data(i)?;
                    i = statement_end(i)?;
                    ast.data.push(_data);
                }
                "option" => {
                    let options;
                    (i, options) = many0(preceded(list_sep, option)).parse_complete(i)?;
                    i = opt(trailing_commas).parse_complete(i)?.0;
                    i = statement_end(i)?;
                    ast.option.extend(options);
                }
                "param" => {
                    let items;
                    (i, items) = many0(preceded(list_sep, param_item)).parse_complete(i)?;
                    i = opt(trailing_commas).parse_complete(i)?.0;
                    i = statement_end(i)?;
                    for item in items {
                        match item {
                            ParamItem::Func(f) => ast.param_func.push(f),
                            ParamItem::Value(kv) => ast.param.push(kv),
                        }
                    }
                }
                "global" => {
                    let names;
                    // (HSPICE also takes commas: `.global vdd,vss`, probes)
                    (i, names) = many1(preceded(node_sep, name)).parse_complete(i)?;
                    i = statement_end(i)?;
                    ast.global.extend(names);
                }
                "protect" if !skip_prot => {
                    i = match encrypted_block(i) {
                        Some((rest, e)) => {
                            ast.errors.push(e);
                            skip_blank(rest)
                        }
                        None => keep_verbatim(i, cmd, true, ast),
                    };
                }
                "protect" => {
                    // The region ends at the `.unprot` line. Its text,
                    // including the rest of the `.prot` line, is not parsed.
                    let first = start.location_line();
                    match unprot(i) {
                        Some((next, last)) => {
                            ast.protected.push((first, last));
                            i = next;
                        }
                        None => {
                            // The region takes the rest of the input.
                            let end = i.take_from(i.len());
                            let mut last = end.location_line();
                            if i.fragment().ends_with('\n') && last > first {
                                last -= 1;
                            }
                            ast.protected.push((first, last));
                            ast.errors
                                .push(ParseErrorInner::MissingUnprot.record(start));
                            i = end;
                        }
                    }
                }
                "alter" => {
                    if body.is_some() {
                        return Err(ParseErrorInner::AlterInSubckt);
                    }
                    // The rest of the line is the title of the run.
                    let next = skip_blank(skip_statement(i));
                    let stmt = UnknwonBuilder {
                        cmd,
                        tokens: Vec::new(),
                        raw: raw_span(i, i.take_from(i.len())),
                    };
                    return Ok((
                        next,
                        Some(Step::Alter {
                            pos: Pos::new(start),
                            stmt,
                        }),
                    ));
                }
                // Analysis and output commands: kept as raw tokens, not
                // reported as errors. When their text does not read as
                // tokens (`.meas … when v(a)=0.5`, `.hdl "x.va"`), the
                // command is kept without tokens.
                kw @ ("tran" | "op" | "dc" | "ac" | "temp" | "print" | "probe" | "meas"
                | "measure" | "hdl" | "save" | "tf" | "noise" | "sens" | "four") => {
                    // A `{` or `}`, also inside quotes, fails the statement:
                    // HSPICE rejects it (golden `brace_temp`,
                    // `brace_quoted_temp`, `brace_meas`), and it must not
                    // lose its value with no error. HSPICE only ignores a
                    // `.print` or `.probe` output with braces, with a
                    // warning (golden `brace_print`, `brace_probe`,
                    // `brace_probe_quoted`): that line is kept, and the
                    // lowering gives the warning.
                    if !matches!(kw, "print" | "probe") && has_braces(start) {
                        return Err(ParseErrorInner::Nom(ErrorKind::Char));
                    }
                    let end = skip_statement(i);
                    // HSPICE rejects a group in a `.temp` line (probes
                    // `.temp (50)`, `.temp (25+25)`, `.temp 25 (50)`: "syntax
                    // error at or before "("").
                    let len = end.location_offset() - i.location_offset();
                    if kw == "temp" && group_outside_quotes(&i.fragment()[..len]) {
                        return Err(ParseErrorInner::Nom(ErrorKind::Char));
                    }
                    let raw = raw_span(i, end);
                    let tokens = match many0(multiline_sep(token)).parse_complete(i) {
                        Ok((rest, tokens)) if statement_end(rest).is_ok() => tokens,
                        _ => Vec::new(),
                    };
                    i = skip_blank(end);
                    ast.unknwon.push(UnknwonBuilder { cmd, tokens, raw });
                }
                "if" | "elseif" => {
                    let cond;
                    (i, cond) = condition(i)?;
                    let next = statement_end(i)?;
                    let line = if cmd_lower == "if" {
                        CondLine::If(cond)
                    } else {
                        CondLine::ElseIf(cond)
                    };
                    let pos = Pos::new(start);
                    return Ok((next, Some(Step::Cond { line, pos })));
                }
                kw @ ("else" | "endif") => {
                    let next = statement_end(i)?;
                    let line = if kw == "else" {
                        CondLine::Else
                    } else {
                        CondLine::EndIf
                    };
                    let pos = Pos::new(start);
                    return Ok((next, Some(Step::Cond { line, pos })));
                }
                "ic" => {
                    let _init_condition;
                    (i, _init_condition) = init_condition(i)?;
                    i = statement_end(i)?;
                    ast.init_condition.extend(_init_condition);
                }
                "nodeset" => {
                    let _nodeset;
                    (i, _nodeset) = nodeset(i)?;
                    i = statement_end(i)?;
                    ast.nodeset.extend(_nodeset);
                }
                kw @ ("ends" | "eom") => {
                    if body.is_none() {
                        // A `.ends` with no open `.subckt` (HSPICE rejects
                        // it) fails, so it does not end the file.
                        let keyword = if kw == "eom" { "eom" } else { "ends" };
                        return Err(ParseErrorInner::StrayEnds { keyword });
                    }
                    // (the name may be in parentheses, as in `.subckt (via) a b`,
                    // and a comma is a delimiter there too: `.ends,c1`, probes)
                    (i, _) = opt((node_sep, name)).parse_complete(i)?;
                    (i, _) = opt(many1(one_of("),"))).parse_complete(i)?;
                    // the end of the statement, with its `+` lines
                    let end = skip_statement(i).location_offset();
                    i = match statement_end(i) {
                        Ok(next) => next,
                        Err(e) => {
                            // Keep the `.ends` and record the error here: if
                            // the `.ends` failed, the body would take the
                            // rest of the parent.
                            ast.errors.push(e.record(start));
                            skip_blank(skip_statement(i))
                        }
                    };
                    return Ok((i, Some(Step::Close { end })));
                }
                "end" => {
                    // HSPICE ends the deck at a `.end` line also when other
                    // text follows the keyword, and it reads none of that
                    // text (golden `end_quoted_arg`, `end_kv_arg`).
                    // `BlockLines::scan` stops at every `.end` line too.
                    let next = skip_blank(skip_statement(i));
                    return Ok((
                        next,
                        Some(Step::End {
                            pos: Pos::new(start),
                        }),
                    ));
                }
                // keep the statement verbatim; tokens are best-effort
                cmd_name => {
                    i = keep_verbatim(i, cmd, KNOWN_COMMANDS.contains(&cmd_name), ast);
                }
            }
        }
    }
    Ok((i, None))
}

/// Checks that only spaces and comments follow `i` up to the next logical
/// line, so the statement that stopped at `i` used all of its text. Returns
/// the input after those spaces, comments and empty lines: the start of the
/// next statement. Text on the same line or on a `+` continuation line is
/// `TrailingText`.
pub(super) fn statement_end(i: LocatedSpan<'_>) -> Result<LocatedSpan<'_>, ParseErrorInner> {
    let next = skip_blank(i);
    let at_end = next.is_empty() || {
        let skipped = &i.fragment()[..next.location_offset() - i.location_offset()];
        // `next` must be on a new line, and that line must not continue
        // this statement.
        let new_line = skipped.contains('\n') || i.get_column() == 1;
        new_line && !next.fragment().starts_with('+')
    };
    if at_end {
        Ok(next)
    } else {
        Err(ParseErrorInner::TrailingText)
    }
}

/// The offset of the first line after the first newline of `s` that starts,
/// after spaces, with `.unprot` or one of its short forms (in any letter case).
/// The lines before it are not parsed, so their text can be anything.
fn unprot_line(s: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(n) = s[from..].find('\n') {
        let line = from + n + 1;
        let text = s[line..].trim_start_matches([' ', '\t']);
        if let Some(cmd) = text.strip_prefix('.') {
            let end = cmd.find(|c: char| !is_key(c)).unwrap_or(cmd.len());
            if canonical_cmd(&cmd[..end].to_ascii_lowercase()) == "unprot" {
                return Some(line);
            }
        }
        from = line;
    }
    None
}

/// The end of a protected region whose `.prot` line is at `i` (after the
/// `.prot` keyword): see [`unprot_line`]. Returns the input after that logical
/// line, and the number of that line. `None` when no line ends the region.
pub(super) fn unprot(i: LocatedSpan<'_>) -> Option<(LocatedSpan<'_>, u32)> {
    let s = i.fragment();
    let line = unprot_line(s)?;
    let at = i.take_from(s.len() - s[line..].trim_start_matches([' ', '\t']).len());
    let last = at.location_line();
    Some((skip_blank(skip_statement(at)), last))
}

/// True when the statement at `i` has a `{` or a `}` outside its `$`
/// comments and its `*` comment lines, also inside quotes (as the
/// lowering's check of a failed statement).
fn has_braces(i: LocatedSpan<'_>) -> bool {
    let len = skip_statement(i).location_offset() - i.location_offset();
    i.fragment()[..len].lines().any(|line| {
        let code = line.trim_start();
        !code.starts_with('*') && code[..code.find('$').unwrap_or(code.len())].contains(['{', '}'])
    })
}

/// True when the text `s` of a statement has a `(` outside its quotes and its
/// `$` comment.
fn group_outside_quotes(s: &str) -> bool {
    s[..s.find('$').unwrap_or(s.len())]
        .split(['\'', '"'])
        .step_by(2)
        .any(|outside| outside.contains('('))
}

/// The condition of an `.if` or `.elseif` line: the text in its parentheses,
/// on the one line. HSPICE reads nothing else here: no braces, no group after
/// the group, no `+` line (decks `if2_brace`, `if2_paren_then_more`,
/// `if2_plus_*`).
fn condition(i: LocatedSpan<'_>) -> Result<(LocatedSpan<'_>, Span), ParseErrorInner> {
    let (i, _) = space(i)?;
    let text: &str = i.fragment();
    let line = &text[..text.find('\n').unwrap_or(text.len())];
    let bad = || ParseErrorInner::Nom(ErrorKind::Char);
    if !line.starts_with('(') {
        return Err(bad());
    }
    let mut depth = 0_usize;
    let mut quote = None;
    let end = line
        .char_indices()
        .find_map(|(k, c)| {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '\'' | '"') => quote = Some(c),
                (None, '(') => depth += 1,
                (None, ')') => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(k);
                    }
                }
                _ => {}
            }
            None
        })
        .ok_or_else(bad)?;
    if line[1..end].trim().is_empty() {
        return Err(bad());
    }
    let inner = i.take_from(1);
    let cond = Span {
        start: inner.location_offset(),
        end: inner.location_offset() + end - 1,
        line_num: i.location_line(),
    };
    Ok((i.take_from(end + 1), cond))
}

/// Skips spaces, comments and empty lines (never fails).
#[inline]
pub(super) fn skip_blank(i: LocatedSpan<'_>) -> LocatedSpan<'_> {
    comment_space_newline(i).map_or(i, |(i, _)| i)
}

/// The dot-command keyword at the start of `i` (`.SUBCKT x` → `SUBCKT`).
#[inline]
pub(super) fn dot_cmd(i: LocatedSpan<'_>) -> Option<&str> {
    (char('.'), key_str)
        .parse_complete(i)
        .ok()
        .map(|(_, (_, (cmd, _)))| cmd)
}

/// Skips the rest of the logical line at `i`: the rest of the current line
/// and every `+` continuation line after it. Empty lines and comment lines
/// between continuation lines are skipped too, as `multiline_sep` does.
/// Returns the input at the start of the next line.
pub(super) fn skip_statement(i: LocatedSpan<'_>) -> LocatedSpan<'_> {
    let bytes = i.fragment().as_bytes();
    let next_line = |from: usize| {
        bytes[from..]
            .iter()
            .position(|&c| c == b'\n')
            .map(|p| from + p + 1)
    };
    let mut line_end = next_line(0);
    // `line_end` is the start of the line after the current logical line.
    while let Some(end) = line_end {
        let mut j = end;
        loop {
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\r') {
                j += 1;
            }
            match bytes.get(j) {
                // continuation line: it belongs to this statement
                Some(b'+') => {
                    line_end = next_line(j);
                    break;
                }
                // empty or comment line: look further
                Some(b'\n') => j += 1,
                Some(b'*' | b'$') => match next_line(j) {
                    Some(k) => j = k,
                    None => return i.take_from(end),
                },
                _ => return i.take_from(end),
            }
        }
    }
    i.take_from(bytes.len())
}

#[cfg(test)]
mod tests;
