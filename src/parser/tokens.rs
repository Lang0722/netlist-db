//! The words of a netlist: separators, numbers, names, expressions and values.

use nom::{
    IResult, Input, Parser,
    branch::alt,
    bytes::complete::{take_while, take_while1},
    character::{char, complete::digit1},
    combinator::{map, map_res, recognize},
    error::ErrorKind,
    multi::{many0, separated_list0},
    sequence::{delimited, preceded},
};

use crate::{
    ast::{KeyValueBuilder, ParamFuncBuilder, TokenBuilder, ValueBuilder},
    span::{LocatedSpan, Span},
};

/// `i` after its first `n` bytes, which hold no line break: as `i.take_from(n)`,
/// with no count of the line breaks. It is on the path of every word.
#[inline]
pub(super) fn past(i: LocatedSpan<'_>, n: usize) -> LocatedSpan<'_> {
    // The line number stays the same, so the skipped bytes hold no line break.
    debug_assert!(!i.fragment().as_bytes()[..n].contains(&b'\n'));
    // SAFETY: the offset and the line are those of the rest of the same text.
    unsafe {
        LocatedSpan::new_from_raw_offset(
            i.location_offset() + n,
            i.location_line(),
            &i.fragment()[n..],
            (),
        )
    }
}

#[inline]
pub(super) fn space(i: LocatedSpan) -> IResult<LocatedSpan, LocatedSpan> {
    let s = i.fragment().as_bytes();
    let n = s
        .iter()
        .position(|c| !matches!(c, b'\t' | b'\r' | b' '))
        .unwrap_or(s.len());
    Ok((past(i, n), i.take(n)))
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
        Some((f, len)) if f.is_finite() => Ok((past(i, len), f)),
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
pub(super) fn is_key(c: char) -> bool {
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
    Ok((past(i, end), i.take(end)))
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
pub(super) fn list_sep(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    separator::<false, false>(i)
}

/// The `,` that can end a `.param` or `.option` statement (decks `cc_*`)
pub(super) fn trailing_commas(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    let (j, ()) = list_sep(i)?;
    if i.fragment()[..j.location_offset() - i.location_offset()].contains(',') {
        Ok((j, ()))
    } else {
        Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Char)))
    }
}

#[inline]
pub(super) fn separator<const PARENS: bool, const LONE_PLUS: bool>(
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
                j = past(j, n);
            }
            Some(b',') => j = past(j, 1),
            Some(b'(' | b')') if PARENS => j = past(j, 1),
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
        k.len += len as u32;
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
                v = ValueBuilder::Expr(Span::new(
                    e.start,
                    rest.location_offset(),
                    e.line_num.get(),
                ));
            }
        }
    }
    Ok((rest, KeyValueBuilder { k, v }))
}

/// One item of a `.param` statement.
pub(super) enum ParamItem {
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
pub(super) fn param_item(i: LocatedSpan) -> IResult<LocatedSpan, ParamItem> {
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
/// `V(name)` or `I(name)`: `letter` (lower case) in either case, then the
/// name in parentheses. Gives the name.
#[inline]
fn probe_node(i: LocatedSpan, letter: char) -> IResult<LocatedSpan, Span> {
    map(
        (
            alt((char(letter.to_ascii_uppercase()), char(letter))),
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
/// `V(name)` in either case
#[inline]
pub(super) fn v(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    probe_node(i, 'v')
}
/// `I(name)` in either case
#[inline]
pub(super) fn i(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    probe_node(i, 'i')
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

/// The text of `span`, which was parsed from `base` (or from input after it)
pub(super) fn span_text<'a>(base: LocatedSpan<'a>, span: Span) -> &'a str {
    let offset = base.location_offset();
    let fragment: &'a str = base.fragment();
    &fragment[span.start - offset..span.end() - offset]
}

/// The text from `start` to `end`, without the spaces around it
pub(super) fn raw_span(start: LocatedSpan, end: LocatedSpan) -> Span {
    let text = &start.fragment()[..end.location_offset() - start.location_offset()];
    let lead = text.len() - text.trim_start_matches([' ', '\t']).len();
    let begin = start.location_offset() + lead;
    // (the rest of a file after `.alter` can pass 4 GiB)
    let len = Span::capped_len(text[lead..].trim_end());
    Span::new(begin, begin + len, start.location_line())
}
