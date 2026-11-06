use regex::Regex;
use std::{cell::OnceCell, path::Path, sync::LazyLock};

use nom::{
    IResult, Input, Parser,
    branch::alt,
    bytes::complete::{tag_no_case, take_while, take_while1},
    character::{
        char,
        complete::{digit1, one_of},
    },
    combinator::{map, map_res, opt, recognize},
    error::ErrorKind,
    multi::{many0, many1, separated_list0},
    sequence::{delimited, preceded},
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
        KeyValueBuilder, ModelBuilder, ParamFuncBuilder, PnameColNumBuilder, TokenBuilder,
        UnknwonBuilder, ValueBuilder,
    },
    span::{LocatedSpan, Pos, Span},
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
    if j.fragment().starts_with(['\n', '$', '*', '+'])
        && let Ok(ok) = recognize(continuation).parse_complete(i)
    {
        return Ok(ok);
    }
    Ok((j, s))
}

#[inline]
pub(super) fn key(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    map(take_while1(is_key), |s: LocatedSpan| s.into()).parse_complete(i)
}

/// A plain floating-point number, without scale factors (for simulator
/// output files).
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

/// An HSPICE number with its scale factor or unit (`1m`, `1meg`, `10pF`,
/// `20db`). The rules are those of [`netlist_expr::scan_number`], the one
/// unit table of the repository (golden cases `units` and `units2`).
///
/// A plain number (digits, `.`, exponent) with no letter after it has no
/// unit, so `fast_float2` reads it: the same value, about 7 times faster
/// than `scan_number` for `8.1e-08`. Every other number goes to
/// `scan_number`.
#[inline]
pub(super) fn float_unit(i: LocatedSpan) -> IResult<LocatedSpan, f64> {
    let s = i.fragment();
    let b = s.as_bytes();
    // `inf` and `nan` are names, not numbers
    let digit_first = match b.first() {
        Some(b'+' | b'-') => b.get(1),
        first => first,
    }
    .is_some_and(|&c| c.is_ascii_digit() || c == b'.');
    let number = if digit_first
        && let Ok((f, len)) = fast_float2::parse_partial::<f64, _>(s)
        && !b.get(len).is_some_and(u8::is_ascii_alphabetic)
    {
        Some((f, len))
    } else {
        netlist_expr::scan_number(s)
    };
    match number {
        // A number too large for `f64` (`1e999`) is not a number here: it is
        // kept as written, as an expression.
        Some((f, len)) if f.is_finite() => Ok((i.take_from(len), f)),
        _ => Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Float))),
    }
}

/// The number (with its scale factor, as in `2.5u` or `1meg`) at the start of
/// `s`, and its length
pub(crate) fn number_prefix(s: &str) -> Option<(f64, usize)> {
    float_unit(crate::span::span(s, 0))
        .ok()
        .map(|(rest, v)| (v, s.len() - rest.fragment().len()))
}

/// Whether `s` is an unquoted expression as a whole, such as
/// `agauss(0,0,'2*p1')`
pub(crate) fn is_formula_text(s: &str) -> bool {
    assigned_value(crate::span::span(s, 0)).is_ok_and(|(rest, _)| rest.is_empty())
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
pub(super) fn key_str(i: LocatedSpan<'_>) -> IResult<LocatedSpan<'_>, (&str, Span)> {
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

/// A character that stands for a byte that is not UTF-8 (see
/// `parser::raw_byte`), or U+FFFD: it stays in its name, so the statement is
/// kept.
#[inline]
fn is_unread(c: char) -> bool {
    c == '\u{FFFD}' || super::is_raw_byte(c)
}

/// The ASCII letters and digits, and `others`: a table for each ASCII character
const fn ascii_set(others: &str) -> [bool; 128] {
    let mut set = [false; 128];
    let mut c = 0;
    while c < 128 {
        set[c] = (c as u8).is_ascii_alphanumeric();
        c += 1;
    }
    let others = others.as_bytes();
    let mut k = 0;
    while k < others.len() {
        set[others[k] as usize] = true;
        k += 1;
    }
    set
}

#[inline]
fn is_key(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
/// `!` is in names such as `gnd!` (ground) and `vdd!` (golden `ground_alias`).
#[inline]
fn is_name(c: char) -> bool {
    // `<>` (bus bits) and `!` (global nets) are common in CDL netlists, `~`
    // (inverted signals) in netlists written by schematic editors
    static SET: [bool; 128] = ascii_set("/_.+-*^:@%#[]<>!|~");
    match SET.get(c as usize) {
        Some(&inside) => inside,
        None => c.is_alphanumeric() || is_unread(c),
    }
}

/// A name: [`is_name`] characters, but not starting with `*`, which starts an
/// inline comment (`cload out gnd 1000f *output loading`). A `$` is a character
/// of a name, but it starts a comment at the start of a word and after a word
/// that starts with a digit: `M$1`, `q$a`, and `1u$ note` (decks `cm_dollar_*`).
#[inline]
fn name_span(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    let s = i.fragment();
    let dollar = !s.starts_with(|c: char| c.is_ascii_digit());
    let end = if s.starts_with(['*', '$']) {
        0
    } else {
        s.find(|c: char| !(is_name(c) || (dollar && c == '$')))
            .unwrap_or(s.len())
    };
    if end == 0 {
        return Err(nom::Err::Error(nom::error::Error::new(
            i,
            ErrorKind::TakeWhile1,
        )));
    }
    let (rest, taken) = i.take_split(end);
    Ok((rest, taken))
}
#[inline]
fn is_formula(c: char) -> bool {
    static SET: [bool; 128] = ascii_set("/_.+-*^:");
    match SET.get(c as usize) {
        Some(&inside) => inside,
        None => c.is_alphanumeric() || is_unread(c),
    }
}

/// Whether a `$` at `end` of `s`, an expression without quotes, is in a name: the
/// last word before it starts with a letter. After a number, a `$` starts a
/// comment (`l=1u$ w=9u`, `l=a$b*2`: decks `cm_dollar_*`). `named` is 0, or the
/// offset after an earlier `$` that is in a name: the search for the start of the
/// word stops there, so that many `$` in a word take linear time.
fn dollar_in_name(s: &str, named: usize, end: usize) -> bool {
    let text = &s[named..end];
    let word = text
        .rsplit(|c: char| !(c.is_alphanumeric() || "_.$".contains(c)))
        .next()
        .unwrap_or_default();
    (named > 0 && word.len() == text.len())
        || word.starts_with(|c: char| c.is_alphabetic() || c == '_')
}

/// A file path in `'…'` or `"…"`: the text between the quotes.
#[inline]
fn quoted_path(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    alt((|i| quoted(i, '\''), dquote)).parse_complete(i)
}

#[inline]
pub(super) fn path(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    map(
        alt((quoted_path, take_while1(is_path))),
        |s: LocatedSpan| s.into(),
    )
    .parse_complete(i)
}

/// The text between the quote `q` at the start of `i` and the matching quote.
/// The text may go on over `+` continuation lines (with blank lines and comment
/// lines before them, which the reader of the text reports; a quote in such a
/// comment line is the matching quote, golden `elem_unquoted_cmtline`), but
/// not past the end of the statement: a quote that is not closed is an error,
/// instead of taking in the lines up to the next quote in the file.
fn quoted(i: LocatedSpan, q: char) -> IResult<LocatedSpan, LocatedSpan> {
    let (j, _) = char(q).parse_complete(i)?;
    let s: &str = j.fragment();
    let mut from = 0;
    while let Some(off) = s[from..].find([q, '\n']) {
        let pos = from + off;
        if s[pos..].starts_with(q) {
            return Ok((j.take_from(pos + q.len_utf8()), j.take(pos)));
        }
        if plus_line_after(&s[pos..]).is_none() {
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
pub(super) fn path_str(i: LocatedSpan<'_>) -> IResult<LocatedSpan<'_>, &str> {
    map(
        alt((quoted_path, take_while1(is_path))),
        |s: LocatedSpan| {
            let _s: &str = s.fragment();
            _s
        },
    )
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
pub(super) fn name_str(i: LocatedSpan<'_>) -> IResult<LocatedSpan<'_>, (&str, Span)> {
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
    separator::<true, false>(i)
}

/// [`node_sep`] for the words of a MOSFET line. It stops at a `+` with no word right
/// after it, because such a `+` is a word there (decks `w*`): `1u + 2u`.
pub(super) fn mos_sep(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    separator::<true, true>(i)
}

/// Between the items of a `.param` or `.option` statement: white space, a
/// continuation line, or `,`. HSPICE reads `.param a=1u,b=2u` (decks `pc_*`).
fn list_sep(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    separator::<false, false>(i)
}

/// The `,` that can end a `.param` or `.option` statement (decks `cc_*`)
fn trailing_commas(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    let (j, ()) = list_sep(i)?;
    if i.fragment()[..j.location_offset() - i.location_offset()].contains(',') {
        Ok((j, ()))
    } else {
        Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Char)))
    }
}

#[inline]
fn separator<const PARENS: bool, const LONE_PLUS: bool>(
    i: LocatedSpan,
) -> IResult<LocatedSpan, ()> {
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
            Some(b',') => j = j.take_from(1),
            Some(b'(' | b')') if PARENS => j = j.take_from(1),
            Some(b'+')
                if LONE_PLUS
                    && s.get(1).is_none_or(|c| {
                        matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'(' | b')' | b',')
                    }) =>
            {
                break;
            }
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

/// An expression without quotes: formula characters, and groups in
/// parentheses after them, such as `agauss(1k,0.1k,3)` (HSPICE accepts it,
/// golden `stat_nominal`) or `2*(a+b)`. A group has no spaces, quotes or `$`
/// in it, except at the end of a line where it goes on on a `+` line (see
/// [`paren_group_len`]). The expression ends before a `(` whose group does
/// not close in its statement. A group can start the value: HSPICE reads
/// `r1 1 0 (3k)` and `v1 1 0 (1)` (probes on P-2019.06).
#[inline]
pub(super) fn formula(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    let s = i.fragment();
    // a `*` starts an inline comment
    if s.starts_with('*') {
        return Err(nom::Err::Error(nom::error::Error::new(
            i,
            ErrorKind::TakeWhile1,
        )));
    }
    let (mut end, mut named) = (0, 0);
    loop {
        end = s[end..]
            .find(|c: char| !is_formula(c))
            .map_or(s.len(), |n| end + n);
        if s[end..].starts_with('$') && dollar_in_name(s, named, end) {
            end += 1;
            named = end;
            continue;
        }
        match paren_group_len(&s[end..]) {
            Some(n) => end += n,
            None => break,
        }
    }
    if end == 0 {
        return Err(nom::Err::Error(nom::error::Error::new(
            i,
            ErrorKind::TakeWhile1,
        )));
    }
    let (rest, taken) = i.take_split(end);
    Ok((rest, taken.into()))
}

/// An expression without quotes after the `=` of `key=value`: formula
/// characters and `( ... )` groups that close on the line. Unlike in
/// [`formula`], a group here can hold spaces and can start the value:
/// HSPICE reads `.param p=max(1k, 3k)`, `w=(3k)` and `(1+2)*1k`, on `.param`,
/// `.subckt`, `.model`, `X` and `M` lines (probes on P-2019.06). A quote in a
/// group is rejected there: "the argument "3*1k" of function "max(1k," cannot
/// use quotation marks", except in the calls of [`QUOTED_ARG_FUNCTIONS`]. No
/// `(` of a source function comes here, because it has no `=` before it.
pub(super) fn assigned_formula(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    let s: &str = i.fragment();
    if s.starts_with('*') {
        return Err(nom::Err::Error(nom::error::Error::new(
            i,
            ErrorKind::TakeWhile1,
        )));
    }
    let (mut end, mut named) = (0, 0);
    while let Some(c) = s[end..].chars().next() {
        if c == '$' && dollar_in_name(s, named, end) {
            end += 1;
            named = end;
        } else if is_formula(c) {
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

/// Length of the `( ... )` group at the start of `s` (which starts with `(`);
/// `None` if it holds a quote, or if it is not closed on the same line.
fn balanced_group(s: &str) -> Option<usize> {
    debug_assert!(s.starts_with('('));
    let mut depth = 0_usize;
    for (idx, c) in s.char_indices() {
        match c {
            '\n' | '\'' | '"' => return None,
            '(' => depth += 1,
            ')' => {
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

/// The length of the group in parentheses at the start of `s`, nested
/// groups included, or `None` when `s` does not start with a group that
/// closes before a space, a quote, a `$` or the end of its statement. At
/// the end of a line (after spaces and a `$` comment), the group goes on on
/// the next `+` line, after blank lines and comment lines (see
/// [`plus_line_after`]): HSPICE reads `.param p=max(1k,` / `+ 3k)` as 3k,
/// also with a blank line between (golden `paren_plus_call`,
/// `paren_blank_call`). It rejects a `*` comment line there (golden
/// `paren_cmtline_call`), which the text keeps for the reader of the
/// statement to report.
fn paren_group_len(s: &str) -> Option<usize> {
    if !s.starts_with('(') {
        return None;
    }
    let b = s.as_bytes();
    let mut depth = 0_usize;
    let mut k = 0;
    while k < b.len() {
        match b[k] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k + 1);
                }
            }
            b'\'' | b'"' => return None,
            b' ' | b'\t' | b'\r' | b'\n' | b'$' => {
                // only spaces and a `$` comment up to the end of the line
                while matches!(b.get(k), Some(b' ' | b'\t' | b'\r')) {
                    k += 1;
                }
                if b.get(k) == Some(&b'$') {
                    k += s[k..].find('\n')?;
                }
                if b.get(k) != Some(&b'\n') {
                    return None;
                }
                k += plus_line_after(&s[k..])?;
                continue;
            }
            c if c.is_ascii_whitespace() => return None,
            c if c >= 0x80 && s.is_char_boundary(k) && s[k..].starts_with(char::is_whitespace) => {
                return None;
            }
            _ => {}
        }
        k += 1;
    }
    None
}

/// The length of the text at the start of `s`, which starts at a line end,
/// up to the text of the next `+` line of the statement, after its `+` and
/// its spaces. Blank lines and `*` and `$` comment lines before it are
/// passed over, as [`comment_space_newline`] does. `None` when the
/// statement ends first: the next line that is not blank and not a comment
/// line is not a `+` line, or the input ends.
pub(super) fn plus_line_after(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut k = 0;
    while b.get(k) == Some(&b'\n') {
        k += 1;
        while matches!(b.get(k), Some(b' ' | b'\t' | b'\r')) {
            k += 1;
        }
        match b.get(k) {
            Some(b'+') => {
                k += 1;
                while matches!(b.get(k), Some(b' ' | b'\t' | b'\r')) {
                    k += 1;
                }
                return Some(k);
            }
            Some(b'*' | b'$') => k += s[k..].find('\n')?,
            Some(b'\n') => {}
            _ => return None,
        }
    }
    None
}

/// An expression in `'…'`: the text between the quotes. It fails when the
/// text has a `{` or a `}`: HSPICE reads them as `[` and `]` also inside
/// quotes (`'{a}'` is the name `[a]`) and rejects the deck (golden
/// `brace_quoted_param`, `brace_quoted_elem`, `brace_quoted_fn`), as it
/// rejects `{...}` without quotes (golden `param_braces`). So the
/// statement fails, and it gives no value.
#[inline]
pub(super) fn unquote(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    let (rest, text) = quoted(i, '\'')?;
    if text.fragment().contains(['{', '}']) {
        return Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Char)));
    }
    Ok((rest, text.into()))
}

/// Spaces (` `, tab, CR), comments (from `*` or `$` to the end of the line)
/// and line ends, any number of them. After a line end, `+` lines with
/// only spaces and a comment (`+ $Y=0.2`, as extraction tools write), each
/// ended by a line end or by the end of the input, are passed over too,
/// any number of them. Never fails; returns the rest and the text passed
/// over.
///
/// A scan of the bytes: it is on the path of every statement end. It
/// reads as the parser `many0((space, opt(comment), opt(('\n',
/// many0((space, '+', space, opt(comment), '\n' | eof))))))`, where
/// `comment` is `('*' | '$')` and the rest of the line.
#[inline]
pub(super) fn comment_space_newline(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    let b = i.fragment().as_bytes();
    let skip_space = |mut k: usize| {
        while b.get(k).is_some_and(|c| matches!(c, b' ' | b'\t' | b'\r')) {
            k += 1;
        }
        k
    };
    // from a `*` or `$` to the line end (or the end of the input)
    let skip_comment = |k: usize| match b.get(k) {
        Some(b'*' | b'$') => b[k..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(b.len(), |n| k + n),
        _ => k,
    };
    let mut k = 0;
    loop {
        k = skip_comment(skip_space(k));
        if b.get(k) != Some(&b'\n') {
            break;
        }
        k += 1;
        // comment-only `+` lines, each with its line end (or the end of
        // the input)
        loop {
            let j = skip_space(k);
            if b.get(j) != Some(&b'+') {
                break;
            }
            let j = skip_comment(skip_space(j + 1));
            match b.get(j) {
                Some(b'\n') => k = j + 1,
                None => k = j,
                Some(_) => break,
            }
        }
    }
    let (rest, taken) = i.take_split(k);
    Ok((rest, taken))
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
    // (a number that a white space ends: `formula` stops there too)
    if let Ok((i_num, num)) = float_unit.parse_complete(i)
        && i_num
            .fragment()
            .as_bytes()
            .first()
            .is_none_or(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
    {
        return Ok((i_num, ValueBuilder::Num(num)));
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

/// `=` and the value of `f` after it. The `=` and the value can be on the next
/// `+` line (see [`eq_sep`], deck `m_eq_split`).
#[inline]
pub(super) fn equal<'a, T, F: MyParser<'a, T>>(f: F) -> impl MyParser<'a, T> {
    map((loss_sep, char('='), eq_sep, f), |(_, _, _, v)| v)
}

/// The separator after the `=` of `key=value`: spaces, or, when only
/// spaces and a `$` comment follow on the line, the line end and the `+`
/// of the next `+` line (after blank lines and comment lines) with the
/// spaces after it. HSPICE reads `r1 1 0 r=` / `+ 5k` as `r=5k` (golden
/// `kw_value_next_line`).
#[inline]
fn eq_sep(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    let (rest, s) = space(i)?;
    if matches!(rest.fragment().as_bytes().first(), Some(b'\n' | b'$'))
        && let Ok(r) =
            map((comment_space_newline, char('+'), space), |(_, _, s)| s).parse_complete(rest)
    {
        return Ok(r);
    }
    Ok((rest, s))
}

/// The functions whose arguments can hold quotes, when the call is the whole
/// value: HSPICE runs `.param a=agauss(0,0,'2*p')` and `.param a=str('x')`, on
/// `.param`, `.subckt` and `X` lines. It rejects quotes in any other group, and
/// `2*agauss(0,0,'2')` and `agauss(0,0,'2')*2` (probes on P-2019.06).
const QUOTED_ARG_FUNCTIONS: [&str; 6] = ["agauss", "gauss", "aunif", "unif", "limit", "str"];

/// The length of the group in parentheses at the start of `s`, with its spaces,
/// its `+` lines and the quotes of its arguments (none in a nested group).
fn group_with_quotes(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0_usize;
    let mut k = 0;
    while k < b.len() {
        match b[k] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k + 1);
                }
            }
            q @ (b'\'' | b'"') => {
                let close = s[k + 1..].find([q as char, '\n'])?;
                if depth > 1 || b[k + 1 + close] == b'\n' {
                    return None;
                }
                k += 1 + close;
            }
            b'\n' => {
                k += plus_line_after(&s[k..])?;
                continue;
            }
            _ => {}
        }
        k += 1;
    }
    None
}

/// The value of a `key=value` parameter: see [`group_value`]. A list of numbers
/// (`TC=0,0`) is kept as one expression.
#[inline]
pub(super) fn assigned_value(start: LocatedSpan) -> IResult<LocatedSpan, ValueBuilder> {
    let (mut i, mut v) = group_value(start)?;
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

/// A value after a `=`: see [`value`] and [`assigned_formula`], and a call of
/// [`QUOTED_ARG_FUNCTIONS`].
#[inline]
pub(super) fn group_value(start: LocatedSpan) -> IResult<LocatedSpan, ValueBuilder> {
    let (mut i, mut v) = match value(start) {
        Ok(ok) => ok,
        Err(e) if start.fragment().starts_with('(') => {
            let Ok((i, f)) = assigned_formula(start) else {
                return Err(e);
            };
            (i, ValueBuilder::Expr(f))
        }
        Err(e) => return Err(e),
    };
    // (HSPICE stops on `'max'(1u,2u)`)
    if matches!(v, ValueBuilder::Expr(_))
        && i.fragment().starts_with('(')
        && !start.fragment().starts_with(['\'', '"'])
    {
        if let Ok((j, f)) = assigned_formula(start) {
            i = j;
            v = ValueBuilder::Expr(f);
        }
        let word = &start.fragment()[..i.location_offset() - start.location_offset()];
        if i.fragment().starts_with('(')
            && QUOTED_ARG_FUNCTIONS
                .iter()
                .any(|f| word.eq_ignore_ascii_case(f))
            && let Some(len) = group_with_quotes(i.fragment())
            && !i.fragment()[len..].starts_with(|c| is_formula(c) || c == '(')
        {
            i = i.take_from(len);
            v = ValueBuilder::Expr(raw_span(start, i));
        }
    }
    Ok((i, v))
}

#[inline]
pub(super) fn key_value(i: LocatedSpan) -> IResult<LocatedSpan, KeyValueBuilder> {
    let (i, (k, _, _, _)) = (name, loss_sep, char('='), eq_sep).parse_complete(i)?;
    let (i, v) = assigned_value(i)?;
    Ok((i, KeyValueBuilder { k, v }))
}

/// One assignment of a `.param` statement: `name=value`; a function
/// `name(a,b)='expr'` (the key is `name(a,b)`); or an optimization parameter
/// `name=OPTxxx(init, min, max)` (the value is `OPTxxx(init, min, max)`).
fn param_assignment(i: LocatedSpan) -> IResult<LocatedSpan, KeyValueBuilder> {
    let (mut i, mut k) = name(i)?;
    if i.fragment().starts_with('(')
        && let Some(len) = balanced_group(i.fragment())
    {
        k.end += len;
        i = i.take_from(len);
    }
    let (i, _) = (loss_sep, char('='), eq_sep).parse_complete(i)?;
    let (mut rest, mut v) = assigned_value(i)?;
    if let ValueBuilder::Expr(e) = v
        && !i.fragment().starts_with(['\'', '"'])
    {
        let word = span_text(i, e);
        if word.starts_with(|c: char| c.is_ascii_alphabetic())
            && word.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            let (group, _) = space(rest)?;
            if group.fragment().starts_with('(')
                && let Some(len) = balanced_group(group.fragment())
            {
                rest = group.take_from(len);
                v = ValueBuilder::Expr(Span {
                    end: rest.location_offset(),
                    ..e
                });
            }
        }
    }
    Ok((rest, KeyValueBuilder { k, v }))
}

/// One item of a `.param` statement.
enum ParamItem {
    /// A function definition: `f(a,b)='a*b'`.
    Func(ParamFuncBuilder),
    /// `name=value`
    Value(KeyValueBuilder),
}

/// The body of a function without quotes: [`formula`], which reads a group
/// that goes on on a `+` line, or else [`assigned_formula`], which reads a group
/// with spaces.
fn func_body(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    match formula(i) {
        Ok((rest, body)) if !rest.fragment().starts_with('(') => Ok((rest, body)),
        _ => assigned_formula(i),
    }
}

/// One item of a `.param` statement: `name=value`, or a function
/// definition `f(a,b)='a*b'`. In a function definition, spaces may stand
/// around the arguments and around `=`, not between the name and `(`, the
/// argument list can go on on a `+` line (HSPICE accepts `.param g(x,` /
/// `+ y)='x+y'`: golden `func_args_plus`), and the body is the text of the
/// expression, without its quotes.
#[inline]
fn param_item(i: LocatedSpan) -> IResult<LocatedSpan, ParamItem> {
    fn func(i: LocatedSpan) -> IResult<LocatedSpan, ParamFuncBuilder> {
        let (i, name) = name(i)?;
        map(
            (
                char('('),
                separated_list0(char(','), delimited(loss_sep, key, loss_sep)),
                char(')'),
                equal(alt((unquote, func_body))),
            ),
            |(_, args, _, body)| ParamFuncBuilder { name, args, body },
        )
        .parse_complete(i)
    }
    alt((
        map(func, ParamItem::Func),
        map(param_assignment, ParamItem::Value),
    ))
    .parse_complete(i)
}

/// `key_value` that also gives the key's text.
#[inline]
pub(super) fn key_value_str<'a>(
    i: LocatedSpan<'a>,
) -> IResult<LocatedSpan<'a>, (&'a str, KeyValueBuilder)> {
    map((name_str, equal(assigned_value)), |((s, k), v)| {
        (s, KeyValueBuilder { k, v })
    })
    .parse_complete(i)
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

/// One `.option` item: `name`, `name=value`, or `name="string"`
/// (`sim_dspf="<scope> <file>"`). A string is kept as an `Expr` span
/// without the quotes.
#[inline]
pub(super) fn option(i: LocatedSpan) -> IResult<LocatedSpan, (Span, Option<ValueBuilder>)> {
    alt((
        map((name, equal(dquote)), |(k, s)| {
            (k, Some(ValueBuilder::Expr(s.into())))
        }),
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
fn enddata(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
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
    depth: usize,
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
fn encrypted_block(i: LocatedSpan) -> Option<(LocatedSpan, ParseError)> {
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
fn statement<'a>(
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
fn unprot(i: LocatedSpan<'_>) -> Option<(LocatedSpan<'_>, u32)> {
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
fn dot_cmd(i: LocatedSpan<'_>) -> Option<&str> {
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

/// A block statement: a first line, a body, and a closing command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    /// `.subckt` … `.ends`, or `.macro` … `.eom` (the keywords mix)
    Subckt,
    /// `.data` … `.enddata`
    Data,
    /// `.lib <section>` … `.endl`
    Lib,
}

impl Block {
    /// The block statement that starts at `i`, if any.
    fn at(i: LocatedSpan<'_>) -> Option<Self> {
        let cmd = dot_cmd(i)?.to_ascii_lowercase();
        match canonical_cmd(&cmd) {
            "subckt" | "macro" => Some(Self::Subckt),
            "data" => Some(Self::Data),
            "lib" if lib_section(i) => Some(Self::Lib),
            _ => None,
        }
    }

    /// True when `cmd` (a dot-command keyword) closes the block.
    fn closes(self, cmd: &str) -> bool {
        match self {
            Self::Subckt => cmd.eq_ignore_ascii_case("ends") || cmd.eq_ignore_ascii_case("eom"),
            Self::Data => canonical_cmd(&cmd.to_ascii_lowercase()) == "enddata",
            Self::Lib => cmd.eq_ignore_ascii_case("endl"),
        }
    }
}

/// The input after a statement at `i` (with the dot-command keyword `cmd`)
/// whose text the main parse does not read: a `.prot` region (to its
/// `.unprot` line, or to the end of input) when `skip_prot`, else a
/// `.prot freelib` block, or a `.lib <section>` definition that parses (to its
/// `.endl` line). `None` for other statements. So the pass of
/// `BlockLines::scan` reads the file as the main parse does: a `.end`,
/// `.subckt` or `.ends` line in such text is not a statement.
fn unparsed<'a>(i: LocatedSpan<'a>, cmd: &str, skip_prot: bool) -> Option<LocatedSpan<'a>> {
    let lower = cmd.to_ascii_lowercase();
    if canonical_cmd(&lower) == "protect" {
        let (after, _) = (char('.'), key).parse_complete(i).ok()?;
        if !skip_prot {
            return encrypted_block(after).map(|(rest, _)| rest);
        }
        return Some(unprot(after).map_or_else(|| i.take_from(i.len()), |(next, _)| next));
    }
    if canonical_cmd(&lower) == "lib"
        && let Ok((rest, (_, _, _, None))) = (char('.'), key, space1, lib).parse_complete(i)
    {
        return statement_end(rest).ok();
    }
    None
}

/// True when `i` is a `.lib <section>` definition (one word after `.lib`),
/// not a `.lib <file> <section>` include.
fn lib_section(i: LocatedSpan<'_>) -> bool {
    (char('.'), key, space1, path_str)
        .parse_complete(i)
        .is_ok_and(|(i, _)| statement_end(i).is_ok())
}

/// The opening and closing lines of the blocks of one file, for the skip of
/// failed block statements. The lines of one kind of block are found in one
/// pass over the file, on the first skip that needs them.
/// Then each skip is a look-up, so the skips of a file take linear time
/// together, however many blocks fail.
///
/// In a top file, the pass stops at the first `.end` statement: HSPICE
/// reads no line after it (golden `after_end`, `after_end_second_deck`),
/// so no skip passes it. In other files, HSPICE ignores `.end` (golden
/// `end_in_include`), and so does the pass. Like the main parse, the pass
/// does not read the text of a `.prot` region or of a `.lib <section>`
/// definition that parses (see `unparsed`): a `.end`, `.subckt` or `.ends`
/// line there is not a statement.
pub struct Blocks<'a> {
    /// The whole file.
    file: LocatedSpan<'a>,
    /// The file is a top file: the blocks end at its `.end`.
    top: bool,
    /// `ParseOptions::skip_prot`
    skip_prot: bool,
    /// One entry for each `Block`, in the order of its variants.
    lines: [OnceCell<BlockLines<'a>>; 3],
}

/// The opening and closing lines of one kind of block, in file order.
struct BlockLines<'a> {
    lines: Vec<BlockLine<'a>>,
    /// `min_level[k]` is the lowest `level` in `lines[k..]`.
    min_level: Vec<isize>,
    /// The `.end` statement where the pass stopped (top files only).
    end: Option<LocatedSpan<'a>>,
}

struct BlockLine<'a> {
    /// The offset of the line's `.` in the file.
    start: usize,
    /// The input after the line's logical line.
    after: LocatedSpan<'a>,
    /// The number of blocks open after this line, counted from the start of
    /// the file (below 0 after more closing lines than opening lines).
    level: isize,
    /// For an opening line: its closing line (an index in `lines`), matched
    /// by nesting.
    close: Option<usize>,
    /// The name after `.subckt` or `.ends` (`None` for other blocks).
    name: Option<&'a str>,
}

impl<'a> Blocks<'a> {
    /// The blocks of `file`, which is not a top file.
    pub fn new(file: LocatedSpan<'a>) -> Self {
        Self {
            file,
            top: false,
            skip_prot: false,
            lines: Default::default(),
        }
    }

    /// The blocks of the top file `file`, up to its `.end`.
    pub fn new_top(file: LocatedSpan<'a>) -> Self {
        Self {
            top: true,
            ..Self::new(file)
        }
    }

    /// The blocks with `ParseOptions::skip_prot` set to `skip_prot`.
    pub fn skip_prot(self, skip_prot: bool) -> Self {
        Self { skip_prot, ..self }
    }

    /// Skips a block statement whose start at `i` failed: its first logical
    /// line and its body up to and including the logical line of its
    /// closing command. So the body does not become part of the parent, and
    /// its closing command does not end the parent. A block of the same kind
    /// in the body nests. Without a closing command, only the first logical
    /// line is skipped.
    ///
    /// `body` is the body that `i` is in. For `.subckt`, the closing `.ends`
    /// must leave one `.ends` for each open body before the end of the file,
    /// or else it belongs to one of them. A named `.ends` decides by its name
    /// first: it is the block's when it names the failed `.subckt`, and the
    /// parent's when it names the parent. For `.data` and `.lib` in a body,
    /// no `.subckt` or `.ends` line may lie between the first line and the
    /// closing line, so the skip does not pass the body's `.ends`.
    ///
    /// Known limit: the name check looks only at the failed `.subckt` and
    /// at the body that it is in. A `.ends` that names a body further out
    /// falls back to the count rule. These rules are for ill-formed decks
    /// only: a correct `.ends` ends its body by position, not by name, and
    /// no golden case shows what HSPICE does with a wrong `.ends <name>`.
    fn skip(&self, i: LocatedSpan<'a>, block: Block, body: Option<Body<'a>>) -> LocatedSpan<'a> {
        let close = match block {
            Block::Subckt => self.lines(block).close(i, body),
            Block::Data | Block::Lib => self.lines(block).close(i, None).filter(|close| {
                body.is_none()
                    || !self
                        .lines(Block::Subckt)
                        .any_between(i.location_offset(), close.start)
            }),
        };
        close.map_or_else(|| skip_statement(i), |close| close.after)
    }

    /// Skips a `.subckt` at `i` that is nested too deep: its first logical
    /// line and its body up to and including its `.ends`, matched by
    /// nesting. Without a `.ends`, every line after it is in its body (the
    /// bodies around it have no `.ends` either), so the skip goes to the
    /// `.end` of a top file, else to the end of input. Unlike `skip`, it
    /// does not recurse: the lines are found by the one pass of `lines`.
    ///
    /// The result is always after `i`, so the parse moves on. The pass
    /// stops at the first `.end` line, as the main parse does, so that
    /// `.end` comes after `i`. If it does not (the pass and the main parse
    /// read the file in other ways), the skip goes to the end of input: a
    /// jump back would read the same lines again and again.
    fn skip_nested(&self, i: LocatedSpan<'a>) -> LocatedSpan<'a> {
        let lines = self.lines(Block::Subckt);
        let at = i.location_offset();
        lines
            .lines
            .binary_search_by_key(&at, |line| line.start)
            .ok()
            .and_then(|e| lines.lines[e].close)
            .map(|m| lines.lines[m].after)
            .or(lines.end)
            .filter(|next| next.location_offset() > at)
            .unwrap_or_else(|| i.take_from(i.len()))
    }

    /// The lines of one kind of block, found on the first call.
    fn lines(&self, block: Block) -> &BlockLines<'a> {
        self.lines[block as usize]
            .get_or_init(|| BlockLines::scan(self.file, block, self.top, self.skip_prot))
    }
}

#[cfg(test)]
thread_local! {
    /// The number of `BlockLines::scan` passes on this thread.
    pub(super) static BLOCK_SCANS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

impl<'a> BlockLines<'a> {
    /// The lines of `block` in `file`, up to its first `.end` statement
    /// when `top`.
    fn scan(file: LocatedSpan<'a>, block: Block, top: bool, skip_prot: bool) -> Self {
        #[cfg(test)]
        BLOCK_SCANS.with(|n| n.set(n.get() + 1));
        let mut lines: Vec<BlockLine<'a>> = Vec::new();
        // the opening lines that are not closed yet
        let mut open = Vec::new();
        let mut level = 0;
        let mut end = None;
        let mut j = skip_blank(file);
        while !j.is_empty() {
            let cmd = dot_cmd(j);
            if let Some(next) = cmd.and_then(|c| unparsed(j, c, skip_prot)) {
                j = skip_blank(next);
                continue;
            }
            // every `.end` line ends a top file, as in the main parse
            if top && cmd.is_some_and(|c| c.eq_ignore_ascii_case("end")) {
                end = Some(j);
                break;
            }
            let opens = Block::at(j) == Some(block);
            let closes = !opens && cmd.is_some_and(|c| block.closes(c));
            let start = j;
            j = skip_statement(j);
            if opens || closes {
                if opens {
                    level += 1;
                    open.push(lines.len());
                } else {
                    level -= 1;
                    if let Some(k) = open.pop() {
                        lines[k].close = Some(lines.len());
                    }
                }
                let name = if block == Block::Subckt {
                    (char('.'), key, space1, name_str)
                        .parse_complete(start)
                        .ok()
                        .map(|(_, (_, _, _, (name, _)))| name)
                } else {
                    None
                };
                lines.push(BlockLine {
                    start: start.location_offset(),
                    after: j,
                    level,
                    close: None,
                    name,
                });
            }
            j = skip_blank(j);
        }
        let mut min_level = vec![0; lines.len()];
        let mut low = isize::MAX;
        for (k, line) in lines.iter().enumerate().rev() {
            low = low.min(line.level);
            min_level[k] = low;
        }
        Self {
            lines,
            min_level,
            end,
        }
    }

    /// True when a line starts after offset `from` and before offset `to`.
    fn any_between(&self, from: usize, to: usize) -> bool {
        let k = self.lines.partition_point(|line| line.start <= from);
        self.lines.get(k).is_some_and(|line| line.start < to)
    }

    /// The closing line of the block at `i`, or `None` when the block has
    /// no closing line of its own. `body` is the `.subckt` body that `i` is
    /// in, for `.subckt` blocks only.
    fn close(&self, i: LocatedSpan<'a>, body: Option<Body<'a>>) -> Option<&BlockLine<'a>> {
        let e = self
            .lines
            .binary_search_by_key(&i.location_offset(), |line| line.start)
            .ok()?;
        let m = self.lines[e].close?;
        let close = &self.lines[m];
        let Some(body) = body else {
            return Some(close);
        };
        let names = |name: Option<&str>| {
            close
                .name
                .zip(name)
                .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
        };
        if names(self.lines[e].name) {
            return Some(close);
        }
        if names(Some(body.name)) {
            return None;
        }
        // The level after `close` is the level before the failed line. The
        // open bodies close when the level goes `depth` lower.
        let target = close.level - body.depth as isize;
        self.min_level
            .get(m + 1)
            .is_some_and(|&low| low <= target)
            .then_some(close)
    }
}

#[cfg(test)]
mod tests;
