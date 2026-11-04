use std::{
    path::Path,
    sync::{Arc, LazyLock},
};

use indexmap::IndexMap;
use nom::{
    IResult, Input, Parser,
    branch::alt,
    bytes::complete::{tag, tag_no_case, take_till, take_while, take_while1},
    character::{char, complete::digit1},
    combinator::{eof, map, map_res, opt, recognize, verify},
    error::ErrorKind,
    multi::{many0, many1},
    sequence::preceded,
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
    if !may_continue(i.fragment()) {
        return Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Char)));
    }
    map((comment_space_newline, char('+'), space), |_| ()).parse_complete(i)
}

/// A quick test, true whenever [`continuation`] may match at `s`: after the
/// spaces comes a `+` or a comment, or a line break and then a line that
/// starts with `+`, a comment, or is blank. (Most statements end with a line
/// break followed by the next statement: this avoids parsing it.)
#[inline]
fn may_continue(s: &str) -> bool {
    let s = s.trim_start_matches([' ', '\t', '\r']);
    match s.as_bytes().first() {
        Some(b'+' | b'*' | b'$') => true,
        Some(b'\n') => s[1..]
            .trim_start_matches([' ', '\t', '\r'])
            .starts_with(['+', '*', '$', '\n']),
        _ => false,
    }
}

#[inline]
pub(super) fn multiline_sep<'a, T>(
    f: fn(LocatedSpan<'a>) -> IResult<LocatedSpan<'a>, T>,
) -> impl MyParser<'a, T> {
    alt((preceded(space1, f), preceded(continuation, f)))
}

/// Spaces, or a continuation (`+`) line with the comments and line breaks
/// before it
#[inline]
pub(super) fn loss_sep(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    let (j, s) = space(i)?;
    // (a continuation can only start with one of these, after the spaces)
    if j.fragment().starts_with(['\n', '$', '*', '+']) {
        if let Ok(ok) = recognize(continuation).parse_complete(i) {
            return Ok(ok);
        }
    }
    Ok((j, s))
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

/// A number with an optional scale factor. A number too large for `f64`
/// (`1e999`) is not a number here: it is kept as written, as an expression.
#[inline]
pub(super) fn float_unit(i: LocatedSpan) -> IResult<LocatedSpan, f64> {
    let (rest, f) = map((float, opt(unit)), |(f, u)| match u {
        Some(Unit::Factor(u)) => f * u,
        Some(Unit::DB) => 10.0_f64.powf(f / 20.0),
        None => f,
    })
    .parse_complete(i)?;
    if f.is_finite() {
        Ok((rest, f))
    } else {
        Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Float)))
    }
}

/// Whether `s` is an unquoted expression as a whole, such as
/// `agauss(0,0,'2*p1')`
pub(crate) fn is_formula_text(s: &str) -> bool {
    formula(crate::span::span(s, 0)).is_ok_and(|(rest, _)| rest.is_empty())
}

/// Whether `s` is a list of numbers separated by commas, such as `0,0` in
/// `TC=0,0`; it is printed without quotes.
pub(crate) fn is_number_list(s: &str) -> bool {
    s.contains(',')
        && s.split(',').all(|item| {
            let i = crate::span::span(item, 0);
            float_unit(i).is_ok_and(|(rest, _)| rest.is_empty())
        })
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
    // `<>` (bus bits) and `!` (global nets) are common in CDL netlists, `~`
    // (inverted signals) in netlists written by schematic editors
    c.is_alphanumeric() || "/_.+-*^:@%#[]<>!|~".contains(c)
}

/// A name: [`is_name`] characters, but not starting with `*`, which starts an
/// inline comment (`cload out gnd 1000f *output loading`)
#[inline]
fn name_span(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    if i.fragment().starts_with('*') {
        return Err(nom::Err::Error(nom::error::Error::new(
            i,
            ErrorKind::TakeWhile1,
        )));
    }
    take_while1(is_name).parse_complete(i)
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

/// The text between the quote `q` at the start of `i` and the matching quote.
/// The text may go on over `+` continuation lines, but not past the end of
/// the statement: a quote that is not closed is an error, instead of taking
/// in the lines up to the next quote in the file.
fn quoted(i: LocatedSpan, q: char) -> IResult<LocatedSpan, LocatedSpan> {
    let (j, _) = char(q).parse_complete(i)?;
    let s: &str = j.fragment();
    let mut from = 0;
    while let Some(off) = s[from..].find([q, '\n']) {
        let pos = from + off;
        if s[pos..].starts_with(q) {
            return Ok((j.take_from(pos + q.len_utf8()), j.take(pos)));
        }
        let next_line = s[pos + 1..].trim_start_matches([' ', '\t', '\r']);
        if !next_line.starts_with('+') {
            break;
        }
        from = pos + 1;
    }
    Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Char)))
}

/// `"text"`
#[inline]
fn dquote(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    quoted(i, '"')
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
    map(name_span, |s: LocatedSpan| {
        (s.fragment().as_bytes()[0], s.into())
    })
    .parse_complete(i)
}

#[inline]
pub(super) fn name(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    map(name_span, |s: LocatedSpan| s.into()).parse_complete(i)
}

#[inline]
pub(super) fn name_str(i: LocatedSpan) -> IResult<LocatedSpan, (&str, Span)> {
    map(name_span, |s: LocatedSpan| {
        let _s: &str = s.fragment();
        (_s, s.into())
    })
    .parse_complete(i)
}

/// Between the names of a node list: white space, a continuation line, or
/// `(`, `)` and `,`, which HSPICE also reads as delimiters:
/// `X1 (a b) sub`, `X2 a b sub (w=1 l=2)`, `E1 o 0 (p,n) 1`, `.SUBCKT (via) a b`.
pub(super) fn node_sep(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    let mut j = i;
    loop {
        let s = j.fragment().as_bytes();
        match s.first() {
            Some(b' ' | b'\t' | b'\r') => {
                let n = s
                    .iter()
                    .position(|b| !matches!(b, b' ' | b'\t' | b'\r'))
                    .unwrap_or(s.len());
                j = j.take_from(n);
            }
            Some(b'(' | b')' | b',') => j = j.take_from(1),
            // (a continuation can only start with one of these)
            Some(b'\n' | b'$' | b'*' | b'+') => match continuation(j) {
                Ok((k, ())) => j = k,
                Err(_) => break,
            },
            _ => break,
        }
    }
    if j.location_offset() == i.location_offset() {
        Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Space)))
    } else {
        Ok((j, ()))
    }
}

/// `)` closing a list of nodes in parentheses, as in `V1 (a 0) 1`; the white
/// space after it is left for the next separator
pub(super) fn close_parens(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    if !i
        .fragment()
        .trim_start_matches([' ', '\t', '\r'])
        .starts_with(')')
    {
        return Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Char)));
    }
    map((space, char(')'), many0((space, char(')')))), |_| ()).parse_complete(i)
}

/// `)` (or `,`) closing a node or parameter list, as in `X2 a b sub (w=1 l=2)`
pub(super) fn trailing_delims(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    // (a quick test first: most lists end with a line break or a comment)
    let next = i.fragment().trim_start_matches([' ', '\t', '\r']);
    if !next.starts_with(['(', ')', ',', '\n', '+']) {
        return Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Char)));
    }
    let (j, ()) = node_sep(i)?;
    let skipped = &i.fragment()[..j.location_offset() - i.location_offset()];
    if skipped.contains(['(', ')', ',']) {
        Ok((j, ()))
    } else {
        Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Char)))
    }
}

/// An unquoted expression: formula characters, plus balanced `( ... )`
/// groups such as `agauss(0,1,3)` or `2*(a+b)`. It does not start with `*`,
/// which starts an inline comment.
#[inline]
pub(super) fn formula(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    let s: &str = i.fragment();
    if s.starts_with('*') {
        return Err(nom::Err::Error(nom::error::Error::new(
            i,
            ErrorKind::TakeWhile1,
        )));
    }
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

/// Length of the `( ... )` group at the start of `s` (which starts with `(`),
/// skipping quoted text; `None` if it is not closed on the same line.
fn balanced_group(s: &str) -> Option<usize> {
    debug_assert!(s.starts_with('('));
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
    map(|i| quoted(i, '\''), |s: LocatedSpan| s.into()).parse_complete(i)
}

#[inline]
pub(super) fn unquote_str(i: LocatedSpan) -> IResult<LocatedSpan, &str> {
    map(
        |i| quoted(i, '\''),
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
                // an empty continuation line (also the last line of the input)
                opt((
                    space,
                    char('+'),
                    space,
                    opt(comment),
                    alt((map(char('\n'), |_| ()), map(eof, |_| ()))),
                )),
            )),
        )),
        |s| !s.is_empty(),
    )))
    .parse_complete(i)
}

#[inline]
pub(super) fn value(i: LocatedSpan) -> IResult<LocatedSpan, ValueBuilder> {
    match i.fragment().as_bytes().first() {
        Some(b'\'') => {
            if let Ok((i, s)) = unquote.parse_complete(i) {
                return Ok((i, ValueBuilder::Expr(s)));
            }
        }
        // HSPICE also delimits expressions with double quotes: `size="2"`
        Some(b'"') => {
            if let Ok((i, s)) = dquote.parse_complete(i) {
                return Ok((i, ValueBuilder::Expr(s.into())));
            }
        }
        _ => {}
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

/// `= value`; the value may be on the next (continuation) line: `Lo=\n+ 3.4e-7`
#[inline]
pub(super) fn equal<'a, T, F: MyParser<'a, T>>(f: F) -> impl MyParser<'a, T> {
    map((space, char('='), loss_sep, f), |(_, _, _, v)| v)
}

/// The value of a `key=value` parameter. A list of numbers (`TC=0,0`) is
/// kept as one expression.
#[inline]
fn assigned_value(start: LocatedSpan) -> IResult<LocatedSpan, ValueBuilder> {
    let (mut i, mut v) = value(start)?;
    if matches!(v, ValueBuilder::Num(_)) {
        let mut list = false;
        while let Ok((j, _)) = (char(','), float_unit).parse_complete(i) {
            i = j;
            list = true;
        }
        if list {
            v = ValueBuilder::Expr(raw_span(start, i));
        }
    }
    Ok((i, v))
}

#[inline]
pub(super) fn key_value(i: LocatedSpan) -> IResult<LocatedSpan, KeyValueBuilder> {
    let (i, (k, _, _, _)) = (name, space, char('='), loss_sep).parse_complete(i)?;
    let (i, v) = assigned_value(i)?;
    Ok((i, KeyValueBuilder { k, v }))
}

/// One assignment of a `.param` statement: `name=value`; a function
/// `name(a,b)='expr'` (the key is `name(a,b)`); or an optimization parameter
/// `name=OPTxxx(init, min, max)` (the value is `OPTxxx(init, min, max)`).
fn param_assignment(i: LocatedSpan) -> IResult<LocatedSpan, KeyValueBuilder> {
    let (mut i, mut k) = name(i)?;
    if i.fragment().starts_with('(') {
        if let Some(len) = balanced_group(i.fragment()) {
            k.end += len;
            i = i.take_from(len);
        }
    }
    let (i, _) = (space, char('='), loss_sep).parse_complete(i)?;
    let (mut rest, mut v) = assigned_value(i)?;
    if let ValueBuilder::Expr(e) = v {
        let word = span_text(i, e);
        if word.starts_with(|c: char| c.is_ascii_alphabetic())
            && word.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            let (group, _) = space(rest)?;
            if group.fragment().starts_with('(') {
                if let Some(len) = balanced_group(group.fragment()) {
                    rest = group.take_from(len);
                    v = ValueBuilder::Expr(Span {
                        end: rest.location_offset(),
                        ..e
                    });
                }
            }
        }
    }
    Ok((rest, KeyValueBuilder { k, v }))
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
            many1(preceded(node_sep, name)),
            opt((
                equal(value),
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
        LazyLock::new(|| Regex::new(r"(?im)^[ \t+]*\.(enddata|data)\b").unwrap());
    RE.captures(i.fragment())
        .is_some_and(|caps| caps[1].eq_ignore_ascii_case("enddata"))
}

#[inline]
fn enddata(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    map_res((char('.'), key_str), |(_, (key, _))| {
        if key.eq_ignore_ascii_case("ENDDATA") {
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
    let (i, name) = preceded(sep, key).parse_complete(i)?;
    let (i, (first_str, first)) = preceded(sep, name_str).parse_complete(i)?;
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
    loop {
        let Ok((j, ())) = sep(end) else { break };
        if enddata(j).is_ok() || float_unit(j).is_ok() {
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
#[inline]
pub(super) fn model(i: LocatedSpan) -> IResult<LocatedSpan, ModelBuilder> {
    map(
        (
            // `name`, not `key`: binned models are named like `nch.1`
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
        // include lib section (`.lib "file"tt` needs no space after the quote)
        map(
            (path_str, space, key_str),
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
    "else",
    "elseif",
    "end_element_variation",
    "end_global_variation",
    "end_local_variation",
    "end_variation",
    "endif",
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
    depth: usize,
) -> Result<(LocatedSpan<'a>, Statement<'a>), ParseError> {
    let mut diags = Vec::new();
    let (i, (_, name, ports_params)) = (node_sep, name, opt(ports_params)).parse_complete(i)?;
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
        depth + 1,
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

/// `.prot freelib` (or `.protect freelib`) starts a block encrypted for HSPICE,
/// which only HSPICE can read: it is skipped up to `.unprot`, with a warning.
/// (A plain `.prot` block is not encrypted; it is parsed as usual.)
fn encrypted_block(i: LocatedSpan) -> Option<(LocatedSpan, Statement)> {
    let (rest, (_, (word, _))) = (space1, key_str).parse_complete(i).ok()?;
    if !word.eq_ignore_ascii_case("freelib") {
        return None;
    }
    static UNPROT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?im)^[ \t]*\.unprot(ect)?\b").unwrap());
    let rest = line_end(rest);
    let text: &str = rest.fragment();
    // stop at the newline before `.unprot`, which is then parsed as usual
    let end = UNPROT.find(text).map_or(text.len(), |m| m.start() - 1);
    Some((
        rest.take_from(end),
        Statement {
            item: Item::None,
            end: None,
            diags: vec![
                ParseErrorInner::Encrypted {
                    lines: text[..end].matches('\n').count(),
                }
                .record(i),
            ],
        },
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
        && instance(start).is_ok_and(|(end, _)| {
            end_of_statement(end)
                .is_ok_and(|(end, ())| end.location_offset() >= rest.location_offset())
        });
    Some((rest, raw_span(i, rest), element))
}

/// `.subckt` definitions nested deeper than this are not nested: the
/// enclosing definition is closed there, with an error. (A cell library whose
/// `.ends` lines are missing would otherwise nest each cell in the previous
/// one, until the parser ran out of stack.)
pub const MAX_SUBCKT_DEPTH: usize = 32;

/// Whether the statement at `i` is `.subckt` or `.macro`
fn starts_subckt(i: LocatedSpan) -> bool {
    (char('.'), key_str, space1)
        .parse_complete(i)
        .is_ok_and(|(_, (_, (cmd, _), _))| {
            cmd.eq_ignore_ascii_case("subckt") || cmd.eq_ignore_ascii_case("macro")
        })
}

/// Parse one statement; the caller checks that nothing is left over.
fn statement<'a>(
    i: LocatedSpan<'a>,
    loaded: &IndexMap<FileId, Option<Pos>>,
    manager: &Arc<ParseManager>,
    work_dir: &Path,
    in_subckt: bool,
    depth: usize,
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
    if matches!(cmd_lower.as_str(), "prot" | "protect") {
        if let Some(stmt) = encrypted_block(i) {
            return Ok(stmt);
        }
    }
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
        "subckt" | "macro" => subckt(i, loaded, manager, work_dir, depth)?,
        "data" => {
            let (i, t) = data(i)?;
            (i, Statement::item(Item::Data(t)))
        }
        "option" | "options" => {
            let (i, t) = many0(multiline_sep(option)).parse_complete(i)?;
            (i, Statement::item(Item::Options(t)))
        }
        "param" | "parameter" | "parameters" => {
            let (i, t) = many0(multiline_sep(param_assignment)).parse_complete(i)?;
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
    depth: usize,
) -> IResult<LocatedSpan<'a>, (LocalAST, EndReason<'a>)> {
    let mut ast = LocalAST::default();
    loop {
        let before = i;
        (i, _) = comment_space_newline(i)?;
        if i.is_empty() {
            return Ok((i, (ast, EndReason::End)));
        }
        if depth >= MAX_SUBCKT_DEPTH && starts_subckt(i) {
            // close the enclosing `.subckt` here; its parent parses this one
            ast.errors
                .push(ParseErrorInner::TooDeep(MAX_SUBCKT_DEPTH).record(i));
            return Ok((before, (ast, EndReason::Ends)));
        }
        let start = i;
        match statement(i, loaded, manager, work_dir, in_subckt, depth) {
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
