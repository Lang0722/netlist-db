use std::{
    path::Path,
    sync::{Arc, LazyLock},
};

use indexmap::IndexMap;
use nom::{
    IResult, Input, Parser,
    branch::alt,
    bytes::complete::{tag, tag_no_case, take, take_till, take_while, take_while1},
    character::{char, complete::digit1},
    combinator::{map, map_res, opt, recognize, success, verify},
    error::ErrorKind,
    multi::{many0, many1},
    sequence::{delimited, preceded},
};
use regex::Regex;

use super::{ast, manager::ParseManager};
use crate::{
    ast::{
        DataBuilder, DataFileBuilder, DataFilesBuilder, DataValuesBuilder, KeyValueBuilder,
        ModelBuilder, PnameColNumBuilder, TokenBuilder, UnknwonBuilder, ValueBuilder,
    },
    span::{EndReason, FileId, LocatedSpan, Pos, Span},
};
use crate::{
    ast::{LocalAST, SubcktBuilder},
    err::{ParseError, ParseErrorInner},
    instance::InstanceBuilder,
    parser::{
        cmds::{init_condition, nodeset},
        instance::{instance, unsupported_controlled_form},
    },
};

#[inline]
pub(super) fn space(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    take_while(|c: char| matches!(c, '\t' | '\r' | ' ')).parse_complete(i)
}
#[inline]
pub(super) fn space1(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    take_while1(|c: char| matches!(c, '\t' | '\r' | ' ')).parse_complete(i)
}
#[inline]
pub(super) fn space_newline(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    take_while(|c: char| matches!(c, '\t' | '\r' | ' ' | '\n')).parse_complete(i)
}

/// A line break inside a statement: the next line starts with `+`. (Lines that
/// end with `\` or `\\` were already joined when the file was read.)
#[inline]
pub(super) fn continuation(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    map((comment_space_newline, char('+'), space), |_| ()).parse_complete(i)
}

#[inline]
pub(super) fn multiline_sep<'a, T>(
    f: fn(LocatedSpan<'a>) -> IResult<LocatedSpan<'a>, T>,
) -> impl MyParser<'a, T> {
    alt((preceded(space1, f), preceded(continuation, f)))
}

#[inline]
pub(super) fn loss_sep(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    alt((recognize(continuation), space)).parse_complete(i)
}

#[inline]
pub(super) fn key(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    map(take_while1(is_key), |s: LocatedSpan| s.into()).parse_complete(i)
}

enum Unit {
    Factor(f64),
    DB,
}

// hspice/index.htm#page/hspice_5/about_hspice_measurement_system.htm#wwID0EOGJM
#[inline]
fn unit(i: LocatedSpan) -> IResult<LocatedSpan, Unit> {
    map(
        take_while1(|c: char| c.is_ascii_alphabetic()),
        |s: LocatedSpan| {
            let u = s.fragment().to_ascii_uppercase();
            match u.as_str() {
                "DB" => return Unit::DB,
                "MIN" => return Unit::Factor(60.0),
                "HR" => return Unit::Factor(3600.0),
                "DAY" => return Unit::Factor(86400.0),
                "YR" => return Unit::Factor(31536000.0),
                "ME" | "Z" => return Unit::Factor(1e+6),
                "MI" => return Unit::Factor(25.4e-6),
                _ => {}
            }
            // HSPICE scale factors: the leading letters select the factor and
            // the rest is ignored, so `10pF` is 10p, `1kohm` is 1k, `1.8V` is 1.8
            // (and `1M` is milli, not mega).
            Unit::Factor(if u.starts_with("MEG") {
                1e+6
            } else if u.starts_with("MIL") {
                25.4e-6
            } else {
                match u.as_bytes()[0] {
                    b'T' => 1e+12,
                    b'G' => 1e+9,
                    b'X' => 1e+6,
                    b'K' => 1e+3,
                    b'M' => 1e-3,
                    b'U' => 1e-6,
                    b'N' => 1e-9,
                    b'P' => 1e-12,
                    b'F' => 1e-15,
                    b'A' => 1e-18,
                    _ => 1.0,
                }
            })
        },
    )
    .parse_complete(i)
}

#[inline]
pub(super) fn float(i: LocatedSpan) -> IResult<LocatedSpan, f64> {
    // SPICE numbers start with a digit or a dot (after an optional sign); this
    // keeps names such as `nand` or `info` from being read as NaN or infinity
    let digits = i.fragment().trim_start_matches(['+', '-']);
    if !digits.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Float)));
    }
    match fast_float2::parse_partial(i) {
        Ok((f, pos)) => Ok((i.take_from(pos), f)),
        Err(_) => Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Float))),
    }
}

#[inline]
pub(super) fn float_unit(i: LocatedSpan) -> IResult<LocatedSpan, f64> {
    map((float, opt(unit)), |(f, u)| match u {
        Some(Unit::Factor(u)) => f * u,
        Some(Unit::DB) => 10.0_f64.powf(f / 20.0),
        None => f,
    })
    .parse_complete(i)
}

#[inline]
pub(super) fn key_str(i: LocatedSpan) -> IResult<LocatedSpan, (&str, Span)> {
    map(take_while1(is_key), |s: LocatedSpan| {
        let _s: &str = s.fragment();
        (_s, s.into())
    })
    .parse_complete(i)
}

#[inline]
fn is_path(c: char) -> bool {
    c.is_alphanumeric() || !c.is_whitespace()
}

#[inline]
fn is_key(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
fn is_name(c: char) -> bool {
    // `<>` (bus bits) and `!` (global nets) are common in CDL netlists
    c.is_alphanumeric() || "/_.+-*^:@%#[]<>!|".contains(c)
}
fn is_formula(c: char) -> bool {
    c.is_alphanumeric() || "/_.+-*^:".contains(c)
}

#[inline]
pub(super) fn path(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    alt((
        unquote,
        map(dquote, |s: LocatedSpan| s.into()),
        map(take_while1(is_path), |s: LocatedSpan| s.into()),
    ))
    .parse_complete(i)
}

/// `"text"`, on one line
#[inline]
fn dquote(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    delimited(char('"'), take_till(|c| c == '"' || c == '\n'), char('"')).parse_complete(i)
}

#[inline]
pub(super) fn integer(i: LocatedSpan) -> IResult<LocatedSpan, usize> {
    map_res(digit1, |s: LocatedSpan| s.parse()).parse_complete(i)
}

#[inline]
pub(super) fn path_str(i: LocatedSpan) -> IResult<LocatedSpan, &str> {
    alt((
        unquote_str,
        map(dquote, |s: LocatedSpan| {
            let _s: &str = s.fragment();
            _s
        }),
        map(take_while1(is_path), |s: LocatedSpan| {
            let _s: &str = s.fragment();
            _s
        }),
    ))
    .parse_complete(i)
}

#[inline]
pub(super) fn name_char(i: LocatedSpan) -> IResult<LocatedSpan, (u8, Span)> {
    map(take_while1(is_name), |s: LocatedSpan| {
        (s.fragment().as_bytes()[0], s.into())
    })
    .parse_complete(i)
}

#[inline]
pub(super) fn name(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    map(take_while1(is_name), |s: LocatedSpan| s.into()).parse_complete(i)
}

#[inline]
pub(super) fn name_str(i: LocatedSpan) -> IResult<LocatedSpan, (&str, Span)> {
    map(take_while1(is_name), |s: LocatedSpan| {
        let _s: &str = s.fragment();
        (_s, s.into())
    })
    .parse_complete(i)
}

/// An unquoted expression: formula characters, plus balanced `( ... )`
/// groups such as `agauss(0,1,3)` or `2*(a+b)`.
#[inline]
pub(super) fn formula(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    let s: &str = i.fragment();
    let mut end = 0;
    while let Some(c) = s[end..].chars().next() {
        if is_formula(c) {
            end += c.len_utf8();
        } else if c == '(' {
            match balanced_group(&s[end..]) {
                Some(len) => end += len,
                None => break,
            }
        } else {
            break;
        }
    }
    if end == 0 {
        Err(nom::Err::Error(nom::error::Error::new(
            i,
            ErrorKind::TakeWhile1,
        )))
    } else {
        Ok((i.take_from(end), i.take(end).into()))
    }
}

/// Length of the `( ... )` group at the start of `s`, skipping quoted text;
/// `None` if it is not closed on the same line.
fn balanced_group(s: &str) -> Option<usize> {
    let mut depth = 0_usize;
    let mut quoted = false;
    for (idx, c) in s.char_indices() {
        match c {
            '\n' => return None,
            '\'' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx + 1);
                }
            }
            _ => {}
        }
    }
    None
}

#[inline]
pub(super) fn unquote(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    map(
        delimited(char('\''), take_till(|c| c == '\''), take(1_usize)),
        |s: LocatedSpan| s.into(),
    )
    .parse_complete(i)
}

#[inline]
pub(super) fn unquote_str(i: LocatedSpan) -> IResult<LocatedSpan, &str> {
    map(
        delimited(char('\''), take_till(|c| c == '\''), take(1_usize)),
        |s: LocatedSpan| {
            let _s: &str = s.fragment();
            _s
        },
    )
    .parse_complete(i)
}

#[inline]
pub(super) fn comment(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    recognize((alt((tag("*"), tag("$"))), take_till(|c| c == '\n'))).parse_complete(i)
}

#[inline]
pub(super) fn comment_space_newline(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    recognize(many0(verify(
        recognize((
            space,
            opt(comment),
            opt((
                char('\n'),
                // continue empty line
                opt((space, char('+'), space, opt(comment), char('\n'))),
            )),
        )),
        |s| !s.is_empty(),
    )))
    .parse_complete(i)
}

#[inline]
pub(super) fn value(i: LocatedSpan) -> IResult<LocatedSpan, ValueBuilder> {
    if let Ok((i, s)) = unquote.parse_complete(i) {
        return Ok((i, ValueBuilder::Expr(s)));
    }
    match (float_unit.parse_complete(i), formula.parse_complete(i)) {
        (Ok((i_num, num)), Ok((i_formula, formula))) => {
            // when the len of rest of formula is less than num
            // means there is a non-quote expression, like `2+2`
            if i_formula.len() < i_num.len() {
                Ok((i_formula, ValueBuilder::Expr(formula)))
            } else {
                Ok((i_num, ValueBuilder::Num(num)))
            }
        }
        (Ok((i_num, num)), Err(_)) => Ok((i_num, ValueBuilder::Num(num))),
        (Err(_), Ok((i_formula, formula))) => Ok((i_formula, ValueBuilder::Expr(formula))),
        (Err(e), Err(_)) => Err(e),
    }
}

pub(super) trait MyParser<'a, T>:
    Parser<LocatedSpan<'a>, Output = T, Error = nom::error::Error<LocatedSpan<'a>>>
{
}

impl<'a, T, P> MyParser<'a, T> for P where
    P: Parser<LocatedSpan<'a>, Output = T, Error = nom::error::Error<LocatedSpan<'a>>>
{
}

#[inline]
pub(super) fn equal<'a, T, F: MyParser<'a, T>>(f: F) -> impl MyParser<'a, T> {
    map((space, char('='), space, f), |(_, _, _, v)| v)
}

#[inline]
pub(super) fn key_value(i: LocatedSpan) -> IResult<LocatedSpan, KeyValueBuilder> {
    map((name, equal(value)), |(k, v)| KeyValueBuilder { k, v }).parse_complete(i)
}

#[inline]
pub(super) fn token(input: LocatedSpan) -> IResult<LocatedSpan, TokenBuilder> {
    alt((
        map(key_value, TokenBuilder::KV),
        map(v, TokenBuilder::V),
        map(i, TokenBuilder::I),
        map(value, TokenBuilder::Value),
    ))
    .parse_complete(input)
}

#[inline]
pub(super) fn option(i: LocatedSpan) -> IResult<LocatedSpan, (Span, Option<ValueBuilder>)> {
    alt((
        map(key_value, |kv| (kv.k, Some(kv.v))),
        map(name, |k| (k, None)),
    ))
    .parse_complete(i)
}
#[inline]
pub(super) fn v(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    map(
        (
            alt((char('V'), char('v'))),
            loss_sep,
            char('('),
            loss_sep,
            name,
            loss_sep,
            char(')'),
        ),
        |(_, _, _, _, name, _, _)| name,
    )
    .parse_complete(i)
}
#[inline]
pub(super) fn i(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    map(
        (
            alt((char('I'), char('i'))),
            loss_sep,
            char('('),
            loss_sep,
            name,
            loss_sep,
            char(')'),
        ),
        |(_, _, _, _, name, _, _)| name,
    )
    .parse_complete(i)
}

pub(super) fn many0_dummyfirst<'a, T, F>(
    mut f: F,
) -> impl FnMut(LocatedSpan<'a>) -> IResult<LocatedSpan<'a>, Vec<T>>
where
    F: MyParser<'a, T>,
    T: Default,
{
    move |mut i: LocatedSpan| {
        let mut acc = Vec::with_capacity(4);
        acc.push(T::default());
        loop {
            let len = i.input_len();
            match f.parse_complete(i) {
                Err(nom::Err::Error(_)) => return Ok((i, acc)),
                Err(e) => return Err(e),
                Ok((i1, o)) => {
                    // infinite loop check: the parser must always consume
                    if i1.input_len() == len {
                        return Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Many0)));
                    }

                    i = i1;
                    acc.push(o);
                }
            }
        }
    }
}

#[inline]
pub(super) fn ports_params(
    i: LocatedSpan,
) -> IResult<LocatedSpan, (Vec<Span>, Vec<KeyValueBuilder>)> {
    map(
        (
            many1(multiline_sep(name)),
            opt((equal(value), many0_dummyfirst(multiline_sep(key_value)))),
        ),
        |(mut ports, _params)| match _params {
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
#[inline]
pub(super) fn data(mut i: LocatedSpan) -> IResult<LocatedSpan, DataBuilder> {
    #[inline]
    fn enddata(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
        map_res((char('.'), key_str), |(_, (key, _))| {
            if key.to_uppercase().as_str() == "ENDDATA" {
                Ok(())
            } else {
                // TODO: error information?
                Err(())
            }
        })
        .parse_complete(i)
    }
    #[inline]
    fn data_files(i: LocatedSpan) -> IResult<LocatedSpan, DataFilesBuilder> {
        #[inline]
        fn file(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
            map_res(
                (multiline_sep(name_str), equal(path)),
                |((key, _), path)| {
                    if key.to_uppercase().as_str() == "FILE" {
                        Ok(path)
                    } else {
                        Err(())
                    }
                },
            )
            .parse_complete(i)
        }
        #[inline]
        fn out(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
            map_res(
                (multiline_sep(name_str), equal(path)),
                |((key, _), path)| {
                    if key.to_uppercase().as_str() == "OUT" {
                        Ok(path)
                    } else {
                        Err(())
                    }
                },
            )
            .parse_complete(i)
        }
        #[inline]
        fn pname_col_num(i: LocatedSpan) -> IResult<LocatedSpan, PnameColNumBuilder> {
            map_res(
                (multiline_sep(name_str), equal(integer)),
                |((pname_str, pname), col_num)| {
                    let binding = pname_str.to_uppercase();
                    let s = binding.as_str();
                    if s != "FILE" && s != "OUT" {
                        Ok(PnameColNumBuilder { pname, col_num })
                    } else {
                        Err(())
                    }
                },
            )
            .parse_complete(i)
        }
        map(
            (
                many1(map(
                    (file, many1(pname_col_num)),
                    |(file, pname_col_num)| DataFileBuilder {
                        file,
                        pname_col_num,
                    },
                )),
                opt(out),
                space_newline,
                enddata,
            ),
            |(files, out, _, _)| DataFilesBuilder { files, out },
        )
        .parse_complete(i)
    }
    let name;
    (i, name) = multiline_sep(key).parse_complete(i)?;
    let first;
    let first_str;
    (i, (first_str, first)) = multiline_sep(name_str).parse_complete(i)?;
    match first_str.to_uppercase().as_str() {
        "MER" => {
            return data_files.parse_complete(i).map(|(i, data_files)| {
                (
                    i,
                    DataBuilder {
                        name,
                        values: DataValuesBuilder::MER(data_files),
                    },
                )
            });
        }
        "LAM" => {
            return data_files.parse_complete(i).map(|(i, data_files)| {
                (
                    i,
                    DataBuilder {
                        name,
                        values: DataValuesBuilder::LAM(data_files),
                    },
                )
            });
        }
        _ => {}
    }
    let mut params = vec![first];
    loop {
        match multiline_sep(float_unit).parse_complete(i) {
            Ok((_i, first_n)) => {
                return map(
                    (
                        many0_dummyfirst(multiline_sep(float_unit)),
                        opt(preceded(comment_space_newline, enddata)),
                    ),
                    |(mut values, _)| {
                        values[0] = first_n;
                        values
                    },
                )
                .parse_complete(_i)
                .map(|(i, values)| {
                    (
                        i,
                        DataBuilder {
                            name,
                            values: DataValuesBuilder::InlineNum { params, values },
                        },
                    )
                });
            }
            Err(_) => {
                let param;
                let param_str;
                (i, (param_str, param)) = multiline_sep(name_str).parse_complete(i)?;
                if param_str.to_uppercase().as_str() == "DATAFORM" {
                    return map(
                        (
                            many1(multiline_sep(value)),
                            opt(preceded(comment_space_newline, enddata)),
                        ),
                        |(values, _)| values,
                    )
                    .parse_complete(i)
                    .map(|(i, values)| {
                        (
                            i,
                            DataBuilder {
                                name,
                                values: DataValuesBuilder::InlineExpr { params, values },
                            },
                        )
                    });
                } else {
                    params.push(param);
                }
            }
        }
    }
}

/// ``` spice
/// .MODEL mname ModelType ([level=val]
/// + [keyword1=val1][keyword2=val2]
/// + [keyword3=val3][LOT distribution value]
/// + [DEV distribution value]...)
/// ```
#[inline]
pub(super) fn model(i: LocatedSpan) -> IResult<LocatedSpan, ModelBuilder> {
    map(
        (
            // `name`, not `key`: binned models are named like `nch.1`
            multiline_sep(name),
            multiline_sep(key_str),
            alt((
                many1(multiline_sep(key_value)),
                map(
                    (
                        loss_sep,
                        char('('),
                        loss_sep,
                        opt((key_value, many0_dummyfirst(multiline_sep(key_value)))),
                        loss_sep,
                        char(')'),
                    ),
                    |(_, _, _, v, _, _)| {
                        if let Some((first, mut vec)) = v {
                            vec[0] = first;
                            vec
                        } else {
                            Vec::new()
                        }
                    },
                ),
                // no parameters, e.g. `.model dmod d`
                success(Vec::new()),
            )),
        ),
        |(name, model_type_ctx, params)| ModelBuilder {
            name,
            model_type: model_type_ctx.into(),
            params,
        },
    )
    .parse_complete(i)
}

#[inline]
fn endlib(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("(?i)\\.endl").unwrap());
    match RE.find(i.fragment()) {
        Some(m) => Ok((i.take_from(m.end()), ())),
        None => Err(nom::Err::Error(nom::error::Error::new(
            i,
            ErrorKind::RegexpCapture,
        ))),
    }
}
pub(super) fn lib(i: LocatedSpan) -> IResult<LocatedSpan, Option<(&str, String)>> {
    alt((
        // include lib section
        map(
            (path_str, space1, key_str),
            |(path_str, _, (section_str, _))| Some((path_str, section_str.to_lowercase())),
        ),
        // skip to `.endl`
        map((endlib, opt((space1, key))), |_| None),
    ))
    .parse_complete(i)
}
/// A parsed statement. It is added to the [`LocalAST`] only if nothing is
/// left over on its line(s).
enum Item {
    None,
    Instance(InstanceBuilder),
    Model(ModelBuilder),
    Subckt(SubcktBuilder),
    Data(DataBuilder),
    Options(Vec<(Span, Option<ValueBuilder>)>),
    Params(Vec<KeyValueBuilder>),
    InitCondition(Vec<(Span, ValueBuilder, Option<Span>)>),
    Nodeset(Vec<(Span, ValueBuilder, Option<Span>)>),
    Unknown(UnknwonBuilder),
}

fn push_item(ast: &mut LocalAST, item: Item) {
    match item {
        Item::None => {}
        Item::Instance(t) => ast.instance.push(t),
        Item::Model(t) => ast.model.push(t),
        Item::Subckt(t) => ast.subckt.push(t),
        Item::Data(t) => ast.data.push(t),
        Item::Options(t) => ast.option.extend(t),
        Item::Params(t) => ast.param.extend(t),
        Item::InitCondition(t) => ast.init_condition.extend(t),
        Item::Nodeset(t) => ast.nodeset.extend(t),
        Item::Unknown(t) => ast.unknwon.push(t),
    }
}

struct Statement<'a> {
    item: Item,
    /// stop the current [`LocalAST`] here (`.inc`, `.lib`, `.ends`)
    end: Option<EndReason<'a>>,
    /// diagnostics that do not invalidate the statement
    diags: Vec<ParseError>,
}

impl Statement<'_> {
    #[inline]
    fn item(item: Item) -> Self {
        Self {
            item,
            end: None,
            diags: Vec::new(),
        }
    }
}

/// HSPICE commands kept verbatim (in [`LocalAST::unknwon`]) without a warning.
/// Other unknown commands are kept as well, with a warning.
const KNOWN_COMMANDS: &[&str] = &[
    "ac",
    "alias",
    "alter",
    "appendmodel",
    "biaschk",
    "connect",
    "dc",
    "dcmatch",
    "dcvolt",
    "del",
    "dellib",
    "disto",
    "dout",
    "else",
    "elseif",
    "end_variation",
    "endif",
    "fft",
    "four",
    "func",
    "global",
    "graph",
    "hdl",
    "if",
    "lin",
    "load",
    "malias",
    "meas",
    "measure",
    "mosra",
    "noise",
    "op",
    "pat",
    "plot",
    "print",
    "probe",
    "prot",
    "protect",
    "pz",
    "sample",
    "save",
    "sens",
    "stim",
    "temp",
    "temperature",
    "tf",
    "title",
    "tran",
    "unprot",
    "unprotect",
    "variation",
    "vec",
    "width",
];

/// After a statement, only spaces and a comment may remain on its last line,
/// and the next line must not continue it (`+`).
pub(super) fn end_of_statement(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    let (i, _) = space(i)?;
    let (i, _) = opt(comment).parse_complete(i)?;
    if !(i.is_empty() || i.fragment().starts_with('\n')) {
        return Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Eof)));
    }
    let (next, _) = comment_space_newline(i)?;
    if next.fragment().starts_with('+') {
        return Err(nom::Err::Error(nom::error::Error::new(
            next,
            ErrorKind::Eof,
        )));
    }
    Ok((i, ()))
}

/// Skip to the end of the line (before the newline)
#[inline]
fn line_end(i: LocatedSpan) -> LocatedSpan {
    let n = i.fragment().find('\n').unwrap_or(i.fragment().len());
    i.take_from(n)
}

/// Skip to the end of the statement at `i`: the rest of its line, and every
/// continuation (`+`) line after it.
pub(super) fn skip_statement(i: LocatedSpan) -> LocatedSpan {
    let mut i = line_end(i);
    loop {
        match comment_space_newline(i) {
            Ok((next, _)) if next.fragment().starts_with('+') => i = line_end(next),
            _ => return i,
        }
    }
}

/// [`ParseErrorInner::Trailing`] where [`end_of_statement`] failed
fn trailing(e: nom::Err<nom::error::Error<LocatedSpan>>) -> ParseError {
    match e {
        nom::Err::Error(e) | nom::Err::Failure(e) => ParseErrorInner::Trailing.record(e.input),
        nom::Err::Incomplete(_) => unreachable!(),
    }
}

/// The text of `span`, which was parsed from `base` (or from input after it)
pub(super) fn span_text<'a>(base: LocatedSpan<'a>, span: Span) -> &'a str {
    let offset = base.location_offset();
    let fragment: &'a str = base.fragment();
    &fragment[span.start - offset..span.end - offset]
}

/// The text from `start` to `end`, without the spaces around it
fn raw_span(start: LocatedSpan, end: LocatedSpan) -> Span {
    let text = &start.fragment()[..end.location_offset() - start.location_offset()];
    let lead = text.len() - text.trim_start_matches([' ', '\t']).len();
    let begin = start.location_offset() + lead;
    Span {
        start: begin,
        end: begin + text[lead..].trim_end().len(),
        line_num: start.location_line(),
    }
}

/// ``` spice
/// .subckt name [ports...] [k=v ...]
/// ...
/// .ends [name]
/// ```
fn subckt<'a>(
    i: LocatedSpan<'a>,
    loaded: &IndexMap<FileId, Option<Pos>>,
    manager: &Arc<ParseManager>,
    work_dir: &Path,
) -> Result<(LocatedSpan<'a>, Statement<'a>), ParseError> {
    let mut diags = Vec::new();
    let (i, (_, name, ports_params)) = (space1, name, opt(ports_params)).parse_complete(i)?;
    let (ports, params) = ports_params.unwrap_or_default();
    // left-over text on the header is reported, but the body is still parsed
    let i = match end_of_statement(i) {
        Ok((i, ())) => i,
        Err(e) => {
            diags.push(trailing(e));
            skip_statement(i)
        }
    };
    let (i, (ast, closed)) = ast(
        manager.clone(),
        loaded.clone(),
        work_dir.to_path_buf(),
        i,
        true,
    )?;
    if !closed {
        diags.push(ParseErrorInner::UnterminatedSubckt(name).with(Some(Pos {
            start: name.start,
            line_num: name.line_num,
        })));
    }
    Ok((
        i,
        Statement {
            item: Item::Subckt(SubcktBuilder {
                name,
                ports,
                params,
                ast,
            }),
            end: None,
            diags,
        },
    ))
}

/// `simulator lang=spice|spectre`: a Spectre directive, not HSPICE syntax,
/// found in model files shared with Spectre. `spice` is ignored with a
/// warning (the rest of its line too); a section in another language is
/// reported and skipped up to the next `simulator lang=spice`, or to the end
/// of the input.
fn simulator_lang(i: LocatedSpan) -> Option<(LocatedSpan, Statement)> {
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
        return Some((
            rest,
            Statement {
                item: Item::None,
                end: None,
                diags: vec![
                    ParseErrorInner::NonHspice(
                        "`simulator lang=spice` is a Spectre directive, not HSPICE; it is ignored"
                            .to_owned(),
                    )
                    .record(i),
                ],
            },
        ));
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
    Some((
        end,
        Statement {
            item: Item::None,
            end: None,
            diags: vec![
                ParseErrorInner::Unsupported(format!(
                    "`simulator lang={lang}` sections (not HSPICE) are not supported; skipped {until}"
                ))
                .record(i),
            ],
        },
    ))
}

/// Parse one statement; the caller checks that nothing is left over.
fn statement<'a>(
    i: LocatedSpan<'a>,
    loaded: &IndexMap<FileId, Option<Pos>>,
    manager: &Arc<ParseManager>,
    work_dir: &Path,
    in_subckt: bool,
) -> Result<(LocatedSpan<'a>, Statement<'a>), ParseError> {
    if i.fragment().starts_with('+') {
        return Err(ParseErrorInner::OrphanContinuation.record(i));
    }
    let Ok((i, _)) = char::<_, nom::error::Error<_>>('.').parse_complete(i) else {
        if let Some(stmt) = simulator_lang(i) {
            return Ok(stmt);
        }
        if let Some((keyword, pos)) = unsupported_controlled_form(i) {
            return Err(ParseErrorInner::Unsupported(format!(
                "Controlled source form `{keyword}` is not supported yet; the statement is skipped"
            ))
            .record(pos));
        }
        let (i, inst) = instance(i)?;
        return Ok((i, Statement::item(Item::Instance(inst))));
    };
    let (i, (cmd_str, cmd)) = key_str(i)?;
    let cmd_lower = cmd_str.to_ascii_lowercase();
    Ok(match cmd_lower.as_str() {
        "lib" => {
            let (i, (_, lib_info)) = (space1, lib).parse_complete(i)?;
            (
                i,
                Statement {
                    item: Item::None,
                    end: lib_info.map(|(file_name, section)| EndReason::Include {
                        file_name: Path::new(file_name),
                        section: Some(section),
                    }),
                    diags: Vec::new(),
                },
            )
        }
        "inc" | "include" => {
            let (i, (_, file_name)) = (space1, path_str).parse_complete(i)?;
            (
                i,
                Statement {
                    item: Item::None,
                    end: Some(EndReason::Include {
                        file_name: Path::new(file_name),
                        section: None,
                    }),
                    diags: Vec::new(),
                },
            )
        }
        "model" => {
            let (i, t) = model(i)?;
            (i, Statement::item(Item::Model(t)))
        }
        "subckt" | "macro" => subckt(i, loaded, manager, work_dir)?,
        "data" => {
            let (i, t) = data(i)?;
            (i, Statement::item(Item::Data(t)))
        }
        "option" | "options" => {
            let (i, t) = many0(multiline_sep(option)).parse_complete(i)?;
            (i, Statement::item(Item::Options(t)))
        }
        "param" | "parameter" | "parameters" => {
            let (i, t) = many0(multiline_sep(key_value)).parse_complete(i)?;
            (i, Statement::item(Item::Params(t)))
        }
        "ic" => {
            let (i, t) = init_condition(i)?;
            (i, Statement::item(Item::InitCondition(t.collect())))
        }
        "nodeset" => {
            let (i, t) = nodeset(i)?;
            (i, Statement::item(Item::Nodeset(t.collect())))
        }
        "ends" | "eom" => {
            // the rest of the line (subckt name, comment) is ignored
            let rest = line_end(i);
            if in_subckt {
                (
                    rest,
                    Statement {
                        item: Item::None,
                        end: Some(EndReason::Ends),
                        diags: Vec::new(),
                    },
                )
            } else {
                // used to end the file here, silently dropping the rest of it
                (
                    rest,
                    Statement {
                        item: Item::None,
                        end: None,
                        diags: vec![ParseErrorInner::StrayEnds.record(i)],
                    },
                )
            }
        }
        // the rest of the line is a comment
        "end" => (line_end(i), Statement::item(Item::None)),
        _ => {
            // keep the statement verbatim; tokens are best-effort
            let end = skip_statement(i);
            let raw = raw_span(i, end);
            let tokens = many0(multiline_sep(token))
                .parse_complete(i)
                .map(|(_, tokens)| tokens)
                .unwrap_or_default();
            let diags = if KNOWN_COMMANDS.contains(&cmd_lower.as_str()) {
                Vec::new()
            } else {
                vec![ParseErrorInner::Unknown(cmd).record(i)]
            };
            (
                end,
                Statement {
                    item: Item::Unknown(UnknwonBuilder { cmd, tokens, raw }),
                    end: None,
                    diags,
                },
            )
        }
    })
}

#[inline]
pub(super) fn local_ast<'a>(
    mut i: LocatedSpan<'a>,
    loaded: &IndexMap<FileId, Option<Pos>>,
    manager: &Arc<ParseManager>,
    work_dir: &Path,
    in_subckt: bool,
) -> IResult<LocatedSpan<'a>, (LocalAST, EndReason<'a>)> {
    let mut ast = LocalAST::default();
    loop {
        (i, _) = comment_space_newline(i)?;
        if i.is_empty() {
            return Ok((i, (ast, EndReason::End)));
        }
        let start = i;
        match statement(i, loaded, manager, work_dir, in_subckt) {
            Ok((rest, stmt)) => {
                ast.errors.extend(stmt.diags);
                match end_of_statement(rest) {
                    Ok((rest, ())) => {
                        i = rest;
                        push_item(&mut ast, stmt.item);
                    }
                    Err(e) => {
                        // Report the left-over text and skip the statement, instead
                        // of parsing the left-over as a new statement.
                        ast.errors.push(trailing(e));
                        i = skip_statement(rest);
                    }
                }
                if let Some(end) = stmt.end {
                    return Ok((i, (ast, end)));
                }
            }
            Err(mut e) => {
                // Skip this statement (with its continuation lines) and go on:
                // one bad statement used to drop the whole file.
                i = skip_statement(start);
                // a parser can fail after looking past the end of the statement
                if e.pos.is_none_or(|pos| pos.start > i.location_offset()) {
                    e.pos = Pos::new(start);
                }
                ast.errors.push(e);
            }
        }
    }
}

// #[cfg(test)]
// mod test {
//     macro_rules! assert_ctx {
//         ($i_res:expr, $ctx:expr $(,)?) => {
//             assert_eq!($ctx, $i_res.unwrap().1.ctx);
//         };
//     }
//     use std::path::PathBuf;

//     use crate::file::TestLocatedSpan;

//     // macro_rules! assert_kv {
//     //     ($i_res:expr, $k:expr, $v:expr $(,)?) => {{
//     //         let kv = $i_res.unwrap().1;
//     //         assert_eq!($k, kv.k.ctx);
//     //         assert_eq!($v, kv.v.ctx);
//     //     }};
//     // }
//     // macro_rules! assert_token {
//     //     ($i_res:expr, $k:expr, $v:expr $(,)?) => {{
//     //         let token = $i_res.unwrap().1;
//     //         if let TokenBuilder::KV(kv) = token {
//     //             assert_eq!($k, kv.k.ctx);
//     //             assert_eq!($v, kv.v.ctx);
//     //         } else {
//     //             panic!("should be key-value!")
//     //         }
//     //     }};
//     //     ($i_res:expr, $word:expr $(,)?) => {{
//     //         let token = $i_res.unwrap().1;
//     //         if let TokenBuilder::ValueBuilder(word) = token {
//     //             assert_eq!($word, word.ctx);
//     //         } else {
//     //             panic!("should be word!")
//     //         }
//     //     }};
//     // }
//     use super::*;
//     #[test]
//     fn test_num() {
//         assert_eq!(1.233, float_unit(span("1.233 ")).unwrap().1);
//         assert_eq!(1.233e-6, float_unit(span("1.233u ")).unwrap().1);
//     }
//     #[test]
//     fn test_key() {
//         assert_ctx!(key(span("iw_ww ")), "iw_ww");
//         assert_ctx!(key(span("iw.ww ")), "iw");
//         assert_ctx!(name(span("iw.ww ")), "iw.ww");
//         assert_ctx!(formula(span("8.00000000e-01+2")), "8.00000000e-01+2");
//         assert_ctx!(unquote(span("'8.00000000e-01 + 2'")), "8.00000000e-01 + 2");
//         // assert_kv!(
//         //     key_value(span("a='8.00000000e-01 + 2'")),
//         //     "a",
//         //     "8.00000000e-01 + 2"
//         // );
//         // assert!(formula(span("'8.00000000e-01 + 2'")).is_err());
//         // assert_token!(token(span("a")), "a");
//         // assert_token!(
//         //     token(span("a='8.00000000e-01 + 2'")),
//         //     "a",
//         //     "8.00000000e-01 + 2"
//         // );
//         crate::text_diff("'2+2'", value(span("2+2")).unwrap().1.to_string().as_str());
//         crate::text_diff(
//             "R1 node1 node2 var1='key1'",
//             instance(span("R1 node1 node2 var1=key1"))
//                 .unwrap()
//                 .1
//                 .to_string()
//                 .as_str(),
//         );
//         crate::text_diff(
//             "R1 node1 node2 var1='key1' var2='key2'",
//             instance(span("R1 node1 node2 var1=key1 var2=key2"))
//                 .unwrap()
//                 .1
//                 .to_string()
//                 .as_str(),
//         );
//         let (manager, _) = ParseManager::new();
//         let work_dir = PathBuf::new();
//         let loaded = IndexMap::new();
//         crate::text_diff(
//             r#"
// .MODEL nch_mac NMOS
// + level=2 version=4.5
// .SUBCKT INV0SR_12TH40 I ZN VDD VSS
// XX0 ZN I VSS VPW NHVT11LL_CKT W=0.00000031 L=0.00000004
// XX3 ZN I VDD VNW PHVT11LL_CKT W=0.00000027 L=0.00000004
// .ENDS INV0SR_12TH40
// R1 node1 node2 var1='key1' var2='key2'
// R2 node1 node2 var1='key1' var2='key2'
// .mode 'nch_mac' 'nmos' level=2"#,
//             local_ast(
//                 span(
//                     r#"R1 node1 node2 var1=key1 var2=key2
//         R2 node1 node2 var1=key1
//         +var2=key2
//         .model nch_mac nmos level=2
//         + version=4.5
//         .SUBCKT INV0SR_12TH40 I ZN VDD VSS
// XX0 ZN I VSS VPW NHVT11LL_CKT W=310.00n L=40.00n
// XX3 ZN I VDD VNW PHVT11LL_CKT W=270.00n L=40.00n
// .ENDS INV0SR_12TH40
// .mode 'nch_mac' 'nmos' level=2
//         "#,
//                 ),
//                 &loaded,
//                 &manager,
//                 &work_dir,
//             )
//             .unwrap()
//             .1
//             .0
//             .to_string()
//             .as_str(),
//         );
//         crate::text_diff(
//             r#"
// .MODEL nch_mac NMOS
// + level=2 version=4.5
// .SUBCKT INV0SR_12TH40 I ZN VDD VSS
// XX0 ZN I VSS VPW NHVT11LL_CKT W=0.00000031 L=0.00000004
// XX3 ZN I VDD VNW PHVT11LL_CKT W=0.00000027 L=0.00000004
// .ENDS INV0SR_12TH40
// R1 node1 node2 var1='key1' var2='key2'
// R2 node1 node2 var1='key1' var2='key2'
// .mode 'nch_mac' 'nmos' level=2"#,
//             local_ast(
//                 span(
//                     r#"R1 node1 node2 var1=key1 var2=key2
//         R2 node1 node2 var1=key1
//         +var2=key2
//         .model nch_mac
//         + nmos (
//             + level=2
//         + version=4.5
//     + )
//         .SUBCKT INV0SR_12TH40 I ZN VDD VSS
// XX0 ZN I VSS VPW NHVT11LL_CKT W=310.00n L=40.00n
// XX3 ZN I VDD VNW PHVT11LL_CKT W=270.00n L=40.00n
// .ENDS INV0SR_12TH40
// .mode 'nch_mac' 'nmos' level=2
//         "#,
//                 ),
//                 &loaded,
//                 &manager,
//                 &work_dir,
//             )
//             .unwrap()
//             .1
//             .0
//             .to_string()
//             .as_str(),
//         );
//         crate::text_diff(
//             r#"
// .DATA SWEEP
// + var1 var2
// + 1 2
// + 2 4
// .ENDDATA"#,
//             local_ast(
//                 span(
//                     r#".data SWEEP
//                     + var1 var2
//                     + 1    2
//                     + 2    4
//                     .enddata
//         "#,
//                 ),
//                 &loaded,
//                 &manager,
//                 &work_dir,
//             )
//             .unwrap()
//             .1
//             .0
//             .to_string()
//             .as_str(),
//         );
//         crate::text_diff(
//             r#"
// .DATA SWEEP
// + var1 var2 DATAFORM
// + 1 '2+2'
// + 2 4
// .ENDDATA"#,
//             local_ast(
//                 span(
//                     r#".data SWEEP
//                     + var1 var2 DATAFORM
//                     + 1 2+2
//                     + 2 4
//                     .enddata
//         "#,
//                 ),
//                 &loaded,
//                 &manager,
//                 &work_dir,
//             )
//             .unwrap()
//             .1
//             .0
//             .to_string()
//             .as_str(),
//         );
//         crate::text_diff(
//             r#"
// .DATA inputdata MER
// + FILE='file1' p1=1 p2=3 p3=4
// + FILE='file2' p1=1
// + FILE='file3' p1=1
// .ENDDATA"#,
//             local_ast(
//                 span(
//                     r#".DATA inputdata MER
//                     + FILE='file1' p1=1 p2=3 p3=4
//                     + FILE='file2' p1=1
//                     + FILE='file3' p1=1
//                  .ENDDATA
//         "#,
//                 ),
//                 &loaded,
//                 &manager,
//                 &work_dir,
//             )
//             .unwrap()
//             .1
//             .0
//             .to_string()
//             .as_str(),
//         );
//     }

//     #[test]
//     fn libmatch() {
//         assert_eq!(
//             TestLocatedSpan {
//                 offset: 10,
//                 line: 1,
//                 fragment: " \nlines"
//             },
//             lib(span("some .EndL \nlines")).unwrap().0
//         );
//         assert_eq!(
//             TestLocatedSpan {
//                 offset: 13,
//                 line: 1,
//                 fragment: " \nlines"
//             },
//             lib(span("some .EndL tt \nlines")).unwrap().0
//         );
//         assert_eq!(
//             TestLocatedSpan {
//                 offset: 12,
//                 line: 2,
//                 fragment: " "
//             },
//             lib(span("some \n .EndL ")).unwrap().0
//         );
//         println!("{:?}", lib(span("'some' tt")));
//     }
// }
