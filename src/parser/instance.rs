use super::statement::{ports_params, statement_end};
use super::tokens::{
    assigned_value, close_parens, continuation, equal, group_value, integer, key_str, key_value,
    key_value_str, loss_sep, mos_sep, multiline_sep, name, name_char, name_str, node_sep,
    plus_line_after, space, space1, span_text, trailing_delims, value,
};
use crate::{
    ast::{KeyValueBuilder, ValueBuilder},
    err::ParseErrorInner,
    instance::{
        BJT_POSITIONAL, BJTBuilder, CapacitorBuilder, ControlledKind, ControlledPwlBuilder,
        ControlledSourceBuilder, CurrentBuilder, DIODE_POSITIONAL, DiodeBuilder, InductorBuilder,
        InstanceBuilder, InstanceCtxBuilder, MOSFETBuilder, MutualInductorBuilder, RCL_POSITIONAL,
        ResistorBuilder, SourceBuilder, SourceFuncBuilder, SubcktBuilder, VoltageBuilder,
        gives_no_value, plain_word,
    },
    span::{LocatedSpan, Span},
};
use nom::{
    IResult, Input as _, Parser,
    branch::alt,
    character::char,
    combinator::{map, map_res, not, opt},
    error::{Error, ErrorKind},
    multi::{count, many0, many1},
    sequence::{preceded, terminated},
};

/// The rest of the input and the element after its name, or why the line
/// is not that element.
type CtxResult<'a> = Result<(LocatedSpan<'a>, InstanceCtxBuilder), ParseErrorInner>;

#[inline]
fn fail<T>(i: LocatedSpan, kind: ErrorKind) -> IResult<LocatedSpan, T> {
    Err(nom::Err::Error(Error::new(i, kind)))
}

/// One element line. A line that does not have the form of its element
/// gives an error in plain words (`ParseErrorInner::NodeCount`, …).
#[inline]
pub(super) fn instance(i: LocatedSpan) -> Result<(LocatedSpan, InstanceBuilder), ParseErrorInner> {
    let (i, (first_char, name)) = name_char(i)?;
    // element names start with a letter: anything else is not an element
    if !first_char.is_ascii_alphabetic() {
        return Err(ParseErrorInner::Nom(ErrorKind::Alpha));
    }
    let (i, ctx) = match first_char.to_ascii_lowercase() {
        b'r' => _resistor(i)?,
        b'c' => _capacitor(i)?,
        b'l' => _inductor(i)?,
        b'v' => _voltage(i)?,
        b'i' => _current(i)?,
        b'm' => _mosfet(i)?,
        b'q' => _bjt(i)?,
        b'd' => _diode(i)?,
        b'x' => _subckt(i)?,
        b'e' => controlled(i, ControlledKind::E)?,
        b'f' => controlled(i, ControlledKind::F)?,
        b'g' => controlled(i, ControlledKind::G)?,
        b'h' => controlled(i, ControlledKind::H)?,
        b'k' => _mutual_inductor(i)?,
        r#type => {
            let (i, (nodes, params)) = ports_params(i)?;
            (
                i,
                InstanceCtxBuilder::Unknown {
                    r#type,
                    nodes,
                    params,
                },
            )
        }
    };
    Ok((i, InstanceBuilder { name, ctx }))
}

/// The parts of an R, C or L element.
struct Rcl {
    n1: Span,
    n2: Span,
    model: Option<Span>,
    value: Option<ValueBuilder>,
    positional: Vec<(Span, ValueBuilder)>,
    params: Vec<KeyValueBuilder>,
}

/// A value with its text as written (a quoted expression with its
/// quotes). A token that starts with `*` is not one: ` *` starts a
/// comment.
#[inline]
fn spanned_value(i: LocatedSpan) -> IResult<LocatedSpan, (Span, ValueBuilder)> {
    spanned(i, value)
}

/// [`group_value`] with its text: the value after the `=` of `key=value` on a
/// V or I line, where a comma ends the value (`AC=1,90`).
#[inline]
fn spanned_group_value(i: LocatedSpan) -> IResult<LocatedSpan, (Span, ValueBuilder)> {
    spanned(i, group_value)
}

#[inline]
fn spanned<'a>(
    i: LocatedSpan<'a>,
    parse: fn(LocatedSpan<'a>) -> IResult<LocatedSpan<'a>, ValueBuilder>,
) -> IResult<LocatedSpan<'a>, (Span, ValueBuilder)> {
    if i.fragment().starts_with('*') {
        return Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Char)));
    }
    let (rest, v) = parse(i)?;
    let len = rest.location_offset() - i.location_offset();
    Ok((rest, (i.take(len).into(), v)))
}

/// A token after the nodes that is not the key of a `key=value` parameter
/// (`r` in `r=1k` is a key), with its text.
#[inline]
fn bare_token(i: LocatedSpan) -> IResult<LocatedSpan, (Span, ValueBuilder)> {
    terminated(multiline_sep(spanned_value), not((space, char('=')))).parse_complete(i)
}

/// The text of `sp`, a span in `src`.
#[inline]
fn text_of<'a>(src: &LocatedSpan<'a>, sp: &Span) -> &'a str {
    let from = src.location_offset();
    &src.fragment()[sp.start - from..sp.end - from]
}

/// What a bare token after the nodes of an R, C or L line can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Word {
    /// A model name: one word of letters, digits, `_`, `.` and `:` that
    /// does not start with a digit, not in quotes (`rmod`, `nch.1`).
    Name,
    /// A name that starts with a digit: such a word that is not a number
    /// (`2n7002`), or a number with other letters after its scale
    /// (`5vnmos`, `33nmos`, `2umos`, `10pF`, see
    /// `netlist_expr::exact_number`). HSPICE rejects it as a model name
    /// (golden `mos_digit_model`).
    DigitFirst,
    /// An expression without quotes that is not one word (`rmod-1`,
    /// `x*2`): a value, or a name with a character that a model name does
    /// not have.
    Other,
    /// One number with at most one scale (`2u`, `1e-6`; also `1e-3k`,
    /// which HSPICE reads as 0.001, golden `units`), or an expression in
    /// quotes.
    Value,
}

/// What the token `text` (with value `v`) is (see [`Word`]).
fn word_kind(text: &str, v: &ValueBuilder) -> Word {
    match v {
        ValueBuilder::Num(_) if netlist_expr::exact_number(text).is_none() => Word::DigitFirst,
        ValueBuilder::Num(_) => Word::Value,
        ValueBuilder::Expr(_) => {
            let Some(first) = text.chars().next() else {
                return Word::Value;
            };
            if first == '\'' {
                return Word::Value;
            }
            let word = text
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | ':'));
            match (word, first.is_ascii_digit()) {
                (true, false) => Word::Name,
                (true, true) => Word::DigitFirst,
                (false, _) => Word::Other,
            }
        }
    }
}

/// One item after the first token of an R, C or L line (see `_rcl`).
enum RclItem<'a> {
    /// A bare token (see `bare_token`), with its text.
    Token((Span, ValueBuilder)),
    /// A `key=value` parameter, with the text of its key.
    Param(&'a str, KeyValueBuilder),
}

/// A bare token, else a `key=value` parameter, after its separator: as
/// `bare_token`, then `key_value_str`.
fn rcl_item(i: LocatedSpan) -> IResult<LocatedSpan, RclItem> {
    match terminated(spanned_value, not((space, char('=')))).parse_complete(i) {
        Ok((rest, t)) => Ok((rest, RclItem::Token(t))),
        Err(_) => map(key_value_str, |(k, kv)| RclItem::Param(k, kv)).parse_complete(i),
    }
}

/// The two nodes, the model, the value, the numbers after the value and
/// the parameters of an R, C or L element (see `ResistorBuilder` for the
/// orders that are read). `key` is the element's letter (`r`, `c`, `l`),
/// and `element` names the element in an error (`a resistor`).
/// `value_then_model` is what HSPICE does with a value before the model
/// name (R and C), or `None` when it reads that order (L).
#[inline]
fn _rcl<'a>(
    i: LocatedSpan<'a>,
    key: &'static str,
    element: &'static str,
    value_then_model: Option<&'static str>,
) -> Result<(LocatedSpan<'a>, Rcl), ParseErrorInner> {
    // (the nodes may be in parentheses: `R1 (a b) 1k`)
    let (i, (n1, n2, _)) = (
        preceded(node_sep, name),
        preceded(node_sep, name),
        opt(close_parens),
    )
        .parse_complete(i)?;
    let start = i;
    let (mut i, first) = opt(bare_token).parse_complete(i)?;
    if let Some((_, v)) = first
        && no_continuation(i.fragment())
    {
        // the common `R1 a b 1k` and `r1 a b 10.3 $X=0.1`: nothing can
        // follow
        return Ok((
            i,
            Rcl {
                n1,
                n2,
                model: None,
                value: Some(v),
                positional: Vec::new(),
                params: Vec::new(),
            },
        ));
    }
    // One pass over the rest of the statement: the tokens after `first`
    // (only when there is a `first`), then the `key=value` parameters.
    // Each separator (spaces, `$` comments, `+` lines) is read once, and
    // `no_continuation` ends the pass without a parse that fails.
    let mut second = None;
    // `Vec::new` does not allocate: most lines have at most two tokens and
    // no parameters
    let mut more = Vec::new();
    let mut params = Vec::new();
    let mut keyword = None;
    let mut add = |k: &str, kv: KeyValueBuilder| {
        if k.eq_ignore_ascii_case(key) {
            // the last one wins
            keyword = Some(kv.v);
        } else {
            params.push(kv);
        }
    };
    // the first `no_continuation` after `first` is known to be false
    let mut check = first.is_none();
    let mut done = false;
    if first.is_some() {
        loop {
            if check && no_continuation(i.fragment()) {
                done = true;
                break;
            }
            check = true;
            let Ok((rest, item)) = multiline_sep(rcl_item).parse_complete(i) else {
                done = true;
                break;
            };
            i = rest;
            match item {
                RclItem::Token(t) if second.is_none() => second = Some(t),
                RclItem::Token(t) => more.push(t),
                RclItem::Param(k, kv) => {
                    add(k, kv);
                    break;
                }
            }
        }
    }
    // the parameters
    if !done {
        loop {
            if check && no_continuation(i.fragment()) {
                break;
            }
            check = true;
            let Ok((rest, (k, kv))) = multiline_sep(key_value_str).parse_complete(i) else {
                break;
            };
            i = rest;
            add(k, kv);
        }
    }
    let text = |sp: &Span| text_of(&start, sp);
    // a word right after the value, with no model name before it: R and C
    // read no model name there (golden `res_value_model`,
    // `cap_value_model`), so it is the place of `tc1`; L reads its model
    // name there (golden `ind_value_model`)
    let digit_first = |sp: &Span| ParseErrorInner::DigitFirstModel {
        element,
        name: text(sp).to_owned(),
        or_number: true,
        after_value: true,
    };
    // the first token, in the place of the model name (before a value or
    // `r=`), is not a model name
    let not_model = |sp: &Span, digit_first: bool| ParseErrorInner::NotModelName {
        element,
        token: text(sp).to_owned(),
        digit_first,
    };
    // A name that starts with a digit is an error in each place where a
    // model name can be: before `r=` or a value, and right after the value
    // when no model name came before it. In the other places (`tc1` after
    // a model name and a value, and `tc2`), a number with letters after it
    // is read as HSPICE reads a number, without the letters (golden
    // `mos_suffix_letters`, `units2`). An expression without quotes that
    // is not one word (`rmod-1`) is an error before a value, where a model
    // name goes. `word_kind` runs only where its result decides.
    let (model, mut value, rest) = match (first, second) {
        (None, _) => (None, None, more),
        // one token alone is the value
        (Some((_, v0)), None) if keyword.is_none() => (None, Some(v0), more),
        // a word before `r=` is the model; a number or an expression there
        // is a value that `r=` replaces
        (Some((s0, v0)), None) => match word_kind(text(&s0), &v0) {
            Word::Name => (Some(s0), None, more),
            Word::DigitFirst => return Err(not_model(&s0, true)),
            Word::Other | Word::Value => (None, Some(v0), more),
        },
        (Some((s0, v0)), Some((s1, v1))) => match word_kind(text(&s0), &v0) {
            Word::Name => (Some(s0), Some(v1), more),
            Word::DigitFirst => return Err(not_model(&s0, true)),
            first_kind @ (Word::Other | Word::Value) => match word_kind(text(&s1), &v1) {
                Word::DigitFirst => return Err(digit_first(&s1)),
                Word::Name => match value_then_model {
                    Some(what) => {
                        return Err(ParseErrorInner::ValueThenModel {
                            element,
                            model: text(&s1).to_owned(),
                            what,
                        });
                    }
                    None => (Some(s1), Some(v0), more),
                },
                // `rmod-1 2k`: a model name with a character that model
                // names do not have, or an expression value without quotes
                _ if first_kind == Word::Other => return Err(not_model(&s0, false)),
                Word::Other | Word::Value => {
                    more.insert(0, (s1, v1));
                    (None, Some(v0), more)
                }
            },
        },
    };
    // a number, or an expression in quotes (HSPICE reads a quoted TC2 that
    // goes on on a `+` line: golden `quoted_plus_tc`)
    if let Some((sp, _)) = rest
        .iter()
        .find(|(sp, v)| !matches!(v, ValueBuilder::Num(_)) && !text(sp).starts_with('\''))
    {
        return Err(ParseErrorInner::BadPositional {
            element,
            token: text(sp).to_owned(),
            names: &RCL_POSITIONAL,
        });
    }
    if rest.len() > RCL_POSITIONAL.len() {
        return Err(ParseErrorInner::TooManyValues {
            element,
            found: rest.len(),
            names: &RCL_POSITIONAL,
            after: "its value",
        });
    }
    if keyword.is_some() {
        value = keyword;
    }
    if value.is_none() && params.is_empty() {
        return Err(no_value(i, element, key));
    }
    Ok((
        i,
        Rcl {
            n1,
            n2,
            model,
            value,
            positional: rest,
            params,
        },
    ))
}

/// The error of an R, C, L, V or I line with no value and no parameters,
/// where `i` is after its nodes: `R1 a b`, or a token that is not a value
/// (`R1 a b {x}`).
#[cold]
fn no_value(i: LocatedSpan, element: &'static str, key: &'static str) -> ParseErrorInner {
    if statement_end(i).is_ok() {
        ParseErrorInner::NoValue { element, key }
    } else {
        ParseErrorInner::BadValue { element }
    }
}

/// True when the statement cannot go on after `s`: only spaces and a `$`
/// comment follow on this line, and the next line does not start (after
/// spaces) with `+`, `*`, `$` or an empty line, which a `+` line may
/// follow. `+` lines with only spaces and a comment (`+ $Y=0.2`, as
/// extraction tools write) may come first, each ended by a newline or by
/// the end of the input. A
/// quick test that gives the same result as parsing the rest of the
/// statement (`multiline_sep`, `comment_space_newline`). `false` when it
/// cannot tell.
#[inline]
fn no_continuation(s: &str) -> bool {
    let b = s.as_bytes();
    let skip_space = |mut j: usize| {
        while b.get(j).is_some_and(|c| matches!(c, b' ' | b'\t' | b'\r')) {
            j += 1;
        }
        j
    };
    // a comment up to the end of the line
    let skip_comment = |j: usize| {
        j + b[j..]
            .iter()
            .position(|&c| c == b'\n')
            .unwrap_or(b.len() - j)
    };
    let mut j = skip_space(0);
    if b.get(j) == Some(&b'$') {
        j = skip_comment(j);
    }
    match b.get(j) {
        None => return true,
        Some(b'\n') => {}
        Some(_) => return false,
    }
    let mut k = skip_space(j + 1);
    while b.get(k) == Some(&b'+') {
        k = skip_space(k + 1);
        if matches!(b.get(k), Some(b'$' | b'*')) {
            k = skip_comment(k);
        }
        match b.get(k) {
            // the input ends after a comment-only `+` line
            None => return true,
            Some(b'\n') => {}
            Some(_) => return false,
        }
        k = skip_space(k + 1);
    }
    !matches!(b.get(k), Some(b'*' | b'$' | b'\n'))
}
#[inline]
fn _resistor(i: LocatedSpan<'_>) -> CtxResult<'_> {
    // golden `res_value_model`
    let what = "reads the value as a third node and rejects the line";
    let (i, r) = _rcl(i, "r", "a resistor", Some(what))?;
    Ok((
        i,
        InstanceCtxBuilder::Resistor(ResistorBuilder {
            n1: r.n1,
            n2: r.n2,
            model: r.model,
            value: r.value,
            positional: r.positional,
            params: r.params,
        }),
    ))
}
#[inline]
fn _capacitor(i: LocatedSpan<'_>) -> CtxResult<'_> {
    // golden `cap_value_model`
    let what = "does not read the value in this order and uses a capacitance of 0";
    let (i, c) = _rcl(i, "c", "a capacitor", Some(what))?;
    Ok((
        i,
        InstanceCtxBuilder::Capacitor(CapacitorBuilder {
            n1: c.n1,
            n2: c.n2,
            model: c.model,
            value: c.value,
            positional: c.positional,
            params: c.params,
        }),
    ))
}
#[inline]
fn _inductor(i: LocatedSpan<'_>) -> CtxResult<'_> {
    // golden `ind_value_model`: L reads "value, then model"
    let (i, l) = _rcl(i, "l", "an inductor", None)?;
    Ok((
        i,
        InstanceCtxBuilder::Inductor(InductorBuilder {
            n1: l.n1,
            n2: l.n2,
            model: l.model,
            value: l.value,
            positional: l.positional,
            params: l.params,
        }),
    ))
}

/// A positional value or a `key=value` parameter
enum Arg {
    Pos((Span, ValueBuilder)),
    Kv(KeyValueBuilder),
}

#[inline]
fn arg(i: LocatedSpan) -> IResult<LocatedSpan, Arg> {
    alt((map(key_value, Arg::Kv), map(spanned_value, Arg::Pos))).parse_complete(i)
}

/// Values after the model name and `key=value` parameters
type Args = (Vec<(Span, ValueBuilder)>, Vec<KeyValueBuilder>);

/// Positional values and `key=value` parameters, in any order
#[inline]
fn args(i: LocatedSpan) -> IResult<LocatedSpan, Args> {
    map(many0(multiline_sep(arg)), |args| {
        let mut positional = Vec::new();
        let mut params = Vec::new();
        for arg in args {
            match arg {
                Arg::Pos(v) => positional.push(v),
                Arg::Kv(kv) => params.push(kv),
            }
        }
        (positional, params)
    })
    .parse_complete(i)
}

/// The nodes, the model name and the numbers after it, of an `M`, `D` or
/// `Q` line.
struct Split {
    nodes: Vec<Span>,
    mname: Span,
    positional: Vec<(Span, ValueBuilder)>,
}

/// Splits the tokens before the parameters of an `M`, `D` or `Q` line
/// (`tokens`, parsed from `start`) into the nodes, the model name and the
/// numbers after the model name. `element` names the element in an error
/// (`a MOSFET`), `names` are the names of the numbers, and `full_nodes` is
/// the largest usual node count (4 for M and Q, 2 for D).
///
/// The model name is the last token that does not start as a number and
/// gives a value (see [`gives_no_value`]), so node names can be numbers
/// (`M1 1 2 3 4 nch`). After
/// it, a bare `OFF` (an initial-condition flag) is dropped, but with no `names`
/// it stays at its place (see [`crate::instance::is_off`]). The other tokens are
/// the values of `names`, in that order:
///
/// - one number with at most one scale factor (`2u`, `1e-6`, `0.5`,
///   `1e-3k`; see `netlist_expr::scan_number_exact`);
/// - a number with other letters after it (`2umos`: HSPICE ignores the
///   letters, golden `mos_suffix_letters`), but only after `full_nodes`
///   nodes. Else it can be the model name, and a model name that starts
///   with a digit is an error (golden `mos_digit_model`, `5vnmos`).
///
/// Any other token there (`2n7002`) is a model name that starts with a
/// digit: an error. More values than `names` are an error, but with no
/// `names` any number of values is read.
///
/// With no `names` (a MOSFET), a number with an operator after it (`2u*1`) is an
/// expression, and after `full_nodes` nodes any other token that starts with a
/// number is that number (see [`plain_word`]).
fn split_model(
    start: LocatedSpan,
    mut tokens: Vec<Span>,
    element: &'static str,
    names: &'static [&'static str],
    full_nodes: usize,
) -> Result<Split, ParseErrorInner> {
    let text = |s: &Span| text_of(&start, s);
    let off = gives_no_value;
    let digit_first = |name: &str| ParseErrorInner::DigitFirstModel {
        element,
        name: name.to_owned(),
        or_number: true,
        after_value: false,
    };
    // One pass from the end: each token is scanned once. After the model
    // name, the first token that is not one number (`2n7002`), and the
    // first that is not one exact number (`2umos` too); with no model
    // name, the last token that is not one exact number.
    let mut positional = Vec::new();
    let mut first_not_number = None;
    let mut first_inexact = None;
    let mut last_inexact = None;
    let mut m = tokens.len();
    let mname = loop {
        let Some(k) = m.checked_sub(1) else {
            // no name: a token that is not one number is a model name that
            // starts with a digit
            return Err(match last_inexact {
                Some(t) => digit_first(t),
                None => ParseErrorInner::NoModel { element },
            });
        };
        m = k;
        let t = text(&tokens[k]);
        match netlist_expr::scan_number_exact(t) {
            None if off(t) => {
                if names.is_empty() {
                    positional.push((tokens[k], ValueBuilder::Expr(tokens[k])));
                }
            }
            None => break tokens[k],
            Some((f, n, exact)) => {
                if names.is_empty() && plain_word(t).is_none() {
                    positional.push((tokens[k], ValueBuilder::Expr(tokens[k])));
                    continue;
                }
                if n != t.len() {
                    first_not_number = Some(t);
                }
                if n != t.len() || !exact {
                    first_inexact = Some(t);
                    last_inexact = last_inexact.or(Some(t));
                }
                positional.push((tokens[k], ValueBuilder::Num(f)));
            }
        }
    };
    if let Some(t) = if m < full_nodes {
        first_inexact
    } else if names.is_empty() {
        None
    } else {
        first_not_number
    } {
        return Err(digit_first(t));
    }
    if !names.is_empty() && positional.len() > names.len() {
        return Err(ParseErrorInner::TooManyValues {
            element,
            found: positional.len(),
            names,
            after: "its model name",
        });
    }
    positional.reverse();
    tokens.truncate(m);
    Ok(Split {
        nodes: tokens,
        mname,
        positional,
    })
}

/// `Mxxx nd ng ns [nb [n5 [n6 [n7]]]] mname [l [w [ad [as [pd [ps [nrd
/// [nrs]]]]]]]] [OFF] params`: 3 to 7 nodes. Models with more than 4
/// terminals (BSIM-SOI: `d g s e p b t`) put the nodes after the 4th in
/// `extra_nodes`. See `split_model` for the model name. The numbers after
/// it are `l w ad as pd ps nrd nrs`, in that order (golden
/// `mos_positional`, `mos_positional_all`). HSPICE also runs a line with more
/// values (deck `q_positional_numbers_9`).
///
/// Values, `OFF` and `key=value` parameters can mix after the model name, and
/// each takes a place. `M1 d g 0 0 n w=2u 1u 3u` has W=1u: `w=2u` takes the place
/// of L and `1u` is at the place of W (decks `mx_*`). A place that a parameter
/// takes holds an empty value (see [`crate::instance::is_key_place`]). This keeps
/// the order of the line for its readers, which choose among the values of one
/// name. The parser cannot choose, because a word before the first parameter can
/// be a value (`M1 d g 0 0 a b w=2u 1u`, where `a` is the model).
///
/// `(`, `)` and `,` separate the values, also after `key=`: `w=(2u + 3u)` has W=2u,
/// and `w=(2u+3u)` has W=5u, because a space ends a word (decks `g*`). So a value can
/// start on a line and the next word can stand on a `+` line: `l=(1u` / `+ 2u)` has
/// L=1u and W=2u. A comma ends the value of a parameter (`l=1u,3u` has W=3u), but not
/// of `ic=` (decks `gx_*`, `hx_*`, `lt_*`). A lone `+` takes a place and gives no value
/// (see [`crate::instance::gives_no_value`]).
///
/// Only too few nodes are an error here. The parser does not know the models,
/// so in `M1 d g s b nch pl pw` it has `nch` and `pl` in the place of nodes.
/// The readers of the line, which know the models, count the nodes
/// (`models::MosLine`, the lowering of the query layer).
#[inline]
fn _mosfet(i: LocatedSpan<'_>) -> CtxResult<'_> {
    let start = i;
    let (i, mut tokens) = many1(preceded(mos_sep, name)).parse_complete(i)?;
    // (the last word is the first key when `=` comes after it)
    let i = match (loss_sep, char('=')).parse_complete(i) {
        Ok(_) => {
            let key = tokens.pop().expect("one word at least");
            start.take_from(key.start - start.location_offset())
        }
        Err(_) => i,
    };
    let Split {
        mut nodes,
        mname,
        mut positional,
    } = split_model(start, tokens, "a MOSFET", &[], 4)?;
    let mut params = Vec::new();
    let i = if no_continuation(i.fragment()) {
        i
    } else {
        mixed_args(start, i, &mut positional, &mut params)
    };
    if nodes.len() < 3 {
        return Err(ParseErrorInner::NodeCount {
            element: "a MOSFET",
            want: "3 to 7",
            found: nodes.len(),
        });
    }
    let extra_nodes = if nodes.len() > 4 {
        nodes.split_off(4)
    } else {
        Vec::new()
    };
    Ok((
        i,
        InstanceCtxBuilder::MOSFET(MOSFETBuilder {
            nd: nodes[0],
            ng: nodes[1],
            ns: nodes[2],
            nb: nodes.get(3).copied(),
            extra_nodes,
            mname,
            positional,
            params,
        }),
    ))
}

/// A `key=value` of a MOSFET line: see [`_mosfet`]
fn mos_key_value<'a>(
    src: LocatedSpan<'a>,
    i: LocatedSpan<'a>,
) -> IResult<LocatedSpan<'a>, KeyValueBuilder> {
    let (i, k) = name(i)?;
    let (i, v) = if text_of(&src, &k).eq_ignore_ascii_case("ic") {
        equal(assigned_value).parse_complete(i)?
    } else {
        equal(mos_value).parse_complete(i)?
    };
    Ok((i, KeyValueBuilder { k, v }))
}

/// The value after `key=` on a MOSFET line. `(` and `)` only separate words here, so
/// the value is the first word in them: `w=(2u + 3u)` is 2u, `w=(2u+3u)` is 5u.
fn mos_value(i: LocatedSpan) -> IResult<LocatedSpan, ValueBuilder> {
    let i = if i.fragment().starts_with('(') {
        mos_sep(i)?.0
    } else {
        i
    };
    group_value(i)
}

/// One item after the model name of a MOSFET line. A word can be no value at all,
/// such as the `+` of `l=1u (2u + 3u)`, which HSPICE reads as AD.
fn mos_arg<'a>(src: LocatedSpan<'a>, i: LocatedSpan<'a>) -> IResult<LocatedSpan<'a>, Arg> {
    // (`mos_sep` stops at a `+` only when it is a word)
    if i.fragment().starts_with('+') {
        let (after, word) = i.take_split(1);
        let word: Span = word.into();
        return Ok((after, Arg::Pos((word, ValueBuilder::Expr(word)))));
    }
    if let Ok((after, kv)) = mos_key_value(src, i) {
        return Ok((after, Arg::Kv(kv)));
    }
    if let Ok((after, value)) = spanned_value(i) {
        return Ok((after, Arg::Pos(value)));
    }
    let (after, word) = name(i)?;
    Ok((after, Arg::Pos((word, ValueBuilder::Expr(word)))))
}

/// A value without quotes that reads as one number (see [`plain_word`]) becomes
/// that number.
fn read_plain_word(src: &LocatedSpan<'_>, v: &mut ValueBuilder) {
    if let ValueBuilder::Expr(sp) = *v
        && !src.fragment()[..sp.start - src.location_offset()].ends_with(['\'', '"'])
        && let Some(number) = plain_word(text_of(src, &sp))
    {
        *v = ValueBuilder::Num(number);
    }
}

/// The rest of a MOSFET line at `i`, after the values `positional` from the
/// words before the first parameter: see [`_mosfet`]
fn mixed_args<'a>(
    start: LocatedSpan<'a>,
    i: LocatedSpan<'a>,
    positional: &mut Vec<(Span, ValueBuilder)>,
    params: &mut Vec<KeyValueBuilder>,
) -> LocatedSpan<'a> {
    let is_ic = |kv: &KeyValueBuilder| text_of(&start, &kv.k).eq_ignore_ascii_case("ic");
    let mut place = positional.len();
    // the values after `ic=` belong to it (decks `lt_ic_*`)
    let mut after_ic = false;
    let mut rest = mos_sep(i).map_or(i, |(j, ())| j);
    let mut end = i;
    while let Ok((after, item)) = mos_arg(start, rest) {
        match item {
            Arg::Kv(mut kv) => {
                after_ic = is_ic(&kv);
                read_plain_word(&start, &mut kv.v);
                params.push(kv);
                place += 1;
            }
            Arg::Pos(_) if after_ic => {}
            Arg::Pos((sp, mut value)) => {
                let hole: Span = rest.take(0).into();
                positional.resize(place, (hole, ValueBuilder::Expr(hole)));
                read_plain_word(&start, &mut value);
                positional.push((sp, value));
                place += 1;
            }
        }
        end = after;
        match mos_sep(after) {
            Ok((j, ())) => rest = j,
            Err(_) => break,
        }
    }
    opt(trailing_delims)
        .parse_complete(end)
        .map_or(end, |(j, _)| j)
}

/// `Qxxx nc nb ne [ns] mname [area] [OFF] params`. See `split_model`
/// for the model name.
#[inline]
fn _bjt(i: LocatedSpan<'_>) -> CtxResult<'_> {
    let start = i;
    let (i, (tokens, params)) = ports_params(i)?;
    let Split {
        nodes,
        mname,
        positional,
    } = split_model(start, tokens, "a BJT", &BJT_POSITIONAL, 4)?;
    if !(3..=4).contains(&nodes.len()) {
        return Err(ParseErrorInner::NodeCount {
            element: "a BJT",
            want: "3 or 4",
            found: nodes.len(),
        });
    }
    Ok((
        i,
        InstanceCtxBuilder::BJT(BJTBuilder {
            nc: nodes[0],
            nb: nodes[1],
            ne: nodes[2],
            ns: nodes.get(3).copied(),
            mname,
            positional,
            params,
        }),
    ))
}
/// `Dxxx nplus nminus mname [area [pj]] [OFF] params`. See `split_model`
/// for the model name.
#[inline]
fn _diode(i: LocatedSpan<'_>) -> CtxResult<'_> {
    let start = i;
    let (i, (tokens, params)) = ports_params(i)?;
    let Split {
        nodes,
        mname,
        positional,
    } = split_model(start, tokens, "a diode", &DIODE_POSITIONAL, 2)?;
    if nodes.len() != 2 {
        return Err(ParseErrorInner::NodeCount {
            element: "a diode",
            want: "2",
            found: nodes.len(),
        });
    }
    Ok((
        i,
        InstanceCtxBuilder::Diode(DiodeBuilder {
            nplus: nodes[0],
            nminus: nodes[1],
            mname,
            positional,
            params,
        }),
    ))
}
#[inline]
fn _subckt(i: LocatedSpan<'_>) -> CtxResult<'_> {
    let base = i;
    let (i, (mut nodes, params)) = ports_params(i)?;
    let Some(mut cktname) = nodes.pop() else {
        return Err(ParseErrorInner::NoSubcktName);
    };
    // CDL puts a `/` before the subckt name: `XI0 A B / INV` or `XI0 A B /INV`
    let text = span_text(base, cktname);
    if text.len() > 1 && text.starts_with('/') {
        cktname.start += 1;
    }
    if nodes.last().is_some_and(|n| span_text(base, *n) == "/") {
        nodes.pop();
    }
    Ok((
        i,
        InstanceCtxBuilder::Subckt(SubcktBuilder {
            nodes,
            cktname,
            params,
        }),
    ))
}

/// The transient functions of V and I elements (any letter case).
const SOURCE_FUNCS: [&str; 8] = ["pulse", "sin", "exp", "pwl", "sffm", "am", "pat", "lfsr"];

/// True when the word `t` is a keyword of a V or I line: `DC`, `AC` or a
/// function name.
fn source_keyword(t: &str) -> bool {
    t.eq_ignore_ascii_case("dc")
        || t.eq_ignore_ascii_case("ac")
        || SOURCE_FUNCS.iter().any(|f| t.eq_ignore_ascii_case(f))
}

/// A value of a V or I line (see `spanned_value`) that is not a keyword
/// (see `source_keyword`). A function call written with no space in it
/// (`SIN(0,1,1meg)`) is one token, so its name before the `(` is checked:
/// such a token is the function, never a value.
fn source_value(i: LocatedSpan) -> IResult<LocatedSpan, (Span, ValueBuilder)> {
    let (rest, (sp, v)) = spanned_value(i)?;
    if matches!(v, ValueBuilder::Expr(_)) {
        let t = text_of(&i, &sp);
        if source_keyword(t.split_once('(').map_or(t, |(name, _)| name)) {
            return Err(nom::Err::Error(nom::error::Error::new(i, ErrorKind::Tag)));
        }
    }
    Ok((rest, (sp, v)))
}

/// A value on a V or I line after its separator: a `source_value` that is
/// not the key of a `key=value` parameter.
fn source_operand(i: LocatedSpan) -> IResult<LocatedSpan, (Span, ValueBuilder)> {
    terminated(multiline_sep(source_value), not((space, char('=')))).parse_complete(i)
}

/// The AC phase after the AC magnitude, when there is one: a value after
/// spaces (`AC 1 90`, golden `src_ac_phase`) or after a comma (`AC=1,90`,
/// `AC 1, 90`: HSPICE manual `AC=acmag, acphase`; not probed). A function
/// is never the phase (`AC 1 SIN(0,1,1meg)`). When `repeat` (after a PWL
/// with no repeat flag yet), a bare `R` is the repeat flag of the PWL,
/// never the phase (golden `pwl_ac_r`). `element` names the element in an
/// error (`a voltage source`).
fn ac_phase<'a>(
    i: LocatedSpan<'a>,
    element: &'static str,
    repeat: bool,
) -> Result<(LocatedSpan<'a>, Option<(Span, ValueBuilder)>), ParseErrorInner> {
    let flag =
        |at: &LocatedSpan<'a>, sp: &Span| repeat && text_of(at, sp).eq_ignore_ascii_case("r");
    if let Ok((after, _)) = (space, char(',')).parse_complete(i) {
        let Some((rest, phase)) = alt((source_operand, preceded(space, source_value)))
            .parse_complete(after)
            .ok()
            .filter(|(_, (sp, _))| !flag(&after, sp))
        else {
            return Err(ParseErrorInner::BadSource {
                element,
                what: "the `,` after the AC magnitude has no AC phase after it",
            });
        };
        return Ok((rest, Some(phase)));
    }
    match opt(source_operand).parse_complete(i)? {
        (_, Some((sp, _))) if flag(&i, &sp) => Ok((i, None)),
        res => Ok(res),
    }
}

/// `key=value` with the text of the value.
fn key_token<'a>(
    i: LocatedSpan<'a>,
) -> IResult<LocatedSpan<'a>, (&'a str, Span, (Span, ValueBuilder))> {
    map((name_str, equal(spanned_group_value)), |((s, k), t)| {
        (s, k, t)
    })
    .parse_complete(i)
}

/// The arguments of a source function, after its name: the text between
/// the parentheses (`PULSE(0,1,0,1n)`, `SIN (0 1 1meg)`), or the values up
/// to the next keyword or `key=value` (`PULSE 0 1 0 1n`, golden
/// `src_forms`). The arguments can use commas or spaces and continue on
/// `+` lines. Without parentheses, the values of a PWL (`pwl`) also end
/// at a bare `R`: it is the repeat flag, not an argument (golden
/// `pwl_repeat_noparen`), and they can be absent before `PWLFILE=`, so the
/// span is empty (`PWL PWLFILE=wave.csv`).
fn func_args<'a>(
    i: LocatedSpan<'a>,
    element: &'static str,
    pwl: bool,
) -> Result<(LocatedSpan<'a>, Span), ParseErrorInner> {
    if let Ok((after, _)) = (loss_sep, char('(')).parse_complete(i) {
        let Some(len) = func_args_len(after.fragment()) else {
            return Err(ParseErrorInner::BadSource {
                element,
                what: "the `(` of its transient function does not close on its logical line",
            });
        };
        let (rest, args) = after.take_split(len);
        let (rest, _) = char(')').parse_complete(rest)?;
        // without the spaces and `+` markers around the arguments
        let (from, _) = loss_sep(args)?;
        let args = args.take_from((from.location_offset() - args.location_offset()).min(len));
        let text = args.fragment();
        let args = args.take(text.trim_end_matches([' ', '\t', '\r']).len());
        if crate::instance::source_args(args.fragment()).contains(['{', '}']) {
            // golden `brace_fn_unquoted`, and in quotes `brace_quoted_fn`
            // (HSPICE reads `'{a}'` as the name `[a]`), as in an element
            // value (`param_braces`, `brace_quoted_elem`; see `unquote`)
            return Err(ParseErrorInner::BadSource {
                element,
                what: "`{...}` in the arguments of its transient function: HSPICE rejects `{` and `}`, also inside quotes. Write the expression in quotes without braces, `'...'`",
            });
        }
        return Ok((rest, args.into()));
    }
    let arg = |at: LocatedSpan<'a>| {
        // (commas are delimiters too: `PWL 0n 0, 4n 0, 4.05n 5`)
        match (space, char(',')).parse_complete(at) {
            Ok((after, _)) => alt((source_operand, preceded(space, source_value)))
                .parse_complete(after)
                .ok(),
            Err::<_, nom::Err<Error<_>>>(_) => source_operand(at).ok(),
        }
        .filter(|(_, (sp, _))| !(pwl && text_of(&at, sp).eq_ignore_ascii_case("r")))
    };
    let Some((mut rest, (first, _))) = arg(i) else {
        if pwl
            && multiline_sep(name_str)
                .parse_complete(i)
                .is_ok_and(|(_, (word, _))| word.eq_ignore_ascii_case("pwlfile"))
        {
            return Ok((i, i.take(0).into()));
        }
        return Err(ParseErrorInner::BadSource {
            element,
            what: "its transient function has no arguments",
        });
    };
    let mut last = first;
    while let Some((r, (sp, _))) = arg(rest) {
        last = sp;
        rest = r;
    }
    Ok((
        rest,
        Span {
            start: first.start,
            end: last.end,
            line_num: first.line_num,
        },
    ))
}

/// The length of the arguments at the start of `s` (after the `(` of a
/// source function), up to the `)` that closes the function. Parentheses
/// nest, and a `'…'` expression or a comment (`$ …` outside quotes, or a
/// `*` line between `+` lines) does not count. A `'…'` expression can go
/// on on a `+` line (HSPICE reads `PWL(0 '0.3` / `+ *1' 1n 1)` as 0.3:
/// golden `quoted_plus_pwl`). `None` when the function does not close on
/// its logical line.
fn func_args_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 1_usize;
    let mut inside = false;
    let mut k = 0;
    while k < b.len() {
        match b[k] {
            b'\'' => inside = !inside,
            b'(' if !inside => depth += 1,
            b')' if !inside => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            b'$' if !inside => {
                k += s[k..].find('\n').unwrap_or(s.len() - k);
                continue;
            }
            // the next line that is not blank and not a comment line must
            // be a `+` line; inside quotes too, where the reader of the
            // statement reports such a line (HSPICE rejects it: golden
            // `quoted_blank_line`, `quoted_cmtline`)
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

/// The source of a V or I line after its nodes (see `SourceBuilder`).
/// `element` names the element in an error (`a voltage source`).
fn _source<'a>(
    mut i: LocatedSpan<'a>,
    element: &'static str,
) -> Result<(LocatedSpan<'a>, SourceBuilder), ParseErrorInner> {
    // golden `src_order_fn_first`
    let fn_before_dc = ParseErrorInner::BadSource {
        element,
        what: "a transient function comes before `DC`, so HSPICE reads that `dc` as a parameter name and rejects the line. Write `DC` first",
    };
    let mut src = SourceBuilder::default();
    let mut items = 0_usize;
    // A bare `R` may come after a PWL function, once (golden `pwl_repeat`,
    // `pwl_repeat_noparen`, `pwl_ac_r`), also after `R=` (golden
    // `pwl_repeat_key_bare`: the last one wins).
    let mut repeat = false;
    loop {
        if let Ok((mut rest, (k, kspan, (vspan, v)))) = multiline_sep(key_token).parse_complete(i) {
            if k.eq_ignore_ascii_case("dc") {
                if src.func.is_some() {
                    return Err(fn_before_dc);
                }
                src.dc = Some((vspan, v));
            } else if k.eq_ignore_ascii_case("ac") {
                src.ac_mag = Some((vspan, v));
                (rest, src.ac_phase) = ac_phase(rest, element, repeat)?;
            } else {
                src.params.push(KeyValueBuilder { k: kspan, v });
            }
            i = rest;
        } else if let Ok((after, (word, wspan))) = multiline_sep(name_str).parse_complete(i)
            && (source_keyword(word) || repeat && word.eq_ignore_ascii_case("r"))
        {
            if word.eq_ignore_ascii_case("r") {
                // golden `pwl_repeat`, `pwl_repeat_td`: `.option list`
                // gives the repeat time 0 for `R` and for `R=0`
                src.params.push(KeyValueBuilder {
                    k: wspan,
                    v: ValueBuilder::Num(0.0),
                });
                repeat = false;
                i = after;
            } else if word.eq_ignore_ascii_case("dc") {
                if src.func.is_some() {
                    return Err(fn_before_dc);
                }
                let Ok((rest, dc)) = source_operand(after) else {
                    return Err(ParseErrorInner::BadSource {
                        element,
                        what: "`DC` has no value after it",
                    });
                };
                src.dc = Some(dc);
                i = rest;
            } else if word.eq_ignore_ascii_case("ac") {
                let Ok((rest, mag)) = source_operand(after) else {
                    return Err(ParseErrorInner::BadSource {
                        element,
                        what: "`AC` has no magnitude after it",
                    });
                };
                src.ac_mag = Some(mag);
                (i, src.ac_phase) = ac_phase(rest, element, repeat)?;
            } else {
                if src.func.is_some() {
                    return Err(ParseErrorInner::BadSource {
                        element,
                        what: "two transient functions. Write one",
                    });
                }
                repeat = word.eq_ignore_ascii_case("pwl");
                let (rest, args) = match func_args(after, element, repeat) {
                    Ok(ok) => ok,
                    // not a function after all: a parameter named `exp`
                    Err(_) if items == 0 && statement_end(after).is_ok() => {
                        src.dc = Some((wspan, ValueBuilder::Expr(wspan)));
                        i = after;
                        items += 1;
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                src.func = Some(SourceFuncBuilder { name: wspan, args });
                i = rest;
            }
        } else if items == 0
            && let Ok((rest, dc)) = source_operand(i)
        {
            // a value with no keyword is the DC value as the first item
            src.dc = Some(dc);
            i = rest;
        } else {
            break;
        }
        items += 1;
    }
    // No source part: a 0 V (0 A) source (HSPICE manual: `Vxxx n+ n-
    // <<DC=> dcval> <tranfun> <AC=acmag, <acphase>>`, each part optional;
    // not probed).
    if items == 0 && statement_end(i).is_err() {
        return Err(no_value(i, element, "dc"));
    }
    Ok((i, src))
}

#[inline]
fn _voltage(i: LocatedSpan<'_>) -> CtxResult<'_> {
    let (i, (n1, n2, _)) = (
        preceded(node_sep, name),
        preceded(node_sep, name),
        opt(close_parens),
    )
        .parse_complete(i)?;
    let (i, source) = _source(i, "a voltage source")?;
    Ok((
        i,
        InstanceCtxBuilder::Voltage(VoltageBuilder {
            n1,
            n2,
            source: Box::new(source),
        }),
    ))
}
#[inline]
fn _current(i: LocatedSpan<'_>) -> CtxResult<'_> {
    let (i, (n1, n2, _)) = (
        preceded(node_sep, name),
        preceded(node_sep, name),
        opt(close_parens),
    )
        .parse_complete(i)?;
    let (i, source) = _source(i, "a current source")?;
    Ok((
        i,
        InstanceCtxBuilder::Current(CurrentBuilder {
            n1,
            n2,
            source: Box::new(source),
        }),
    ))
}

/// Separator between items: spaces, or a line continuation
#[inline]
fn item_sep(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    alt((continuation, map(space1, |_| ()))).parse_complete(i)
}

const CONTROLLED_KEYWORDS: [&str; 6] = ["VCVS", "VCCS", "CCCS", "CCVS", "VCR", "VCCAP"];
/// Controlled-source forms that are not parsed yet: they are reported
/// instead of being misread as nodes
const UNSUPPORTED_CONTROLLED: [&str; 6] =
    ["DELAY", "LAPLACE", "POLE", "FREQ", "OPAMP", "TRANSFORMER"];
/// Piecewise-linear transfer functions of controlled sources
const PWL_FUNCTIONS: [&str; 7] = ["PWL", "NPWL", "PPWL", "AND", "NAND", "OR", "NOR"];

/// The optional `VCVS`/`VCCS`/... keyword
#[inline]
fn controlled_keyword(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    map_res(key_str, |(word, span)| {
        if CONTROLLED_KEYWORDS
            .iter()
            .any(|k| word.eq_ignore_ascii_case(k))
        {
            Ok(span)
        } else {
            Err(())
        }
    })
    .parse_complete(i)
}

/// `POLY(ndim)`
#[inline]
fn poly(i: LocatedSpan) -> IResult<LocatedSpan, usize> {
    map_res(
        (
            key_str,
            loss_sep,
            char('('),
            loss_sep,
            integer,
            loss_sep,
            char(')'),
        ),
        |((word, _), _, _, _, n, _, _)| {
            if word.eq_ignore_ascii_case("POLY") {
                Ok(n)
            } else {
                Err(())
            }
        },
    )
    .parse_complete(i)
}

/// `PWL(n)`, `NPWL(n)`, `PPWL(n)`, `AND(n)`, ...: the function and `n`
#[inline]
fn pwl_function(i: LocatedSpan) -> IResult<LocatedSpan, (Span, usize)> {
    map_res(
        (
            key_str,
            loss_sep,
            char('('),
            loss_sep,
            integer,
            loss_sep,
            char(')'),
        ),
        |((word, span), _, _, _, n, _, _)| {
            if PWL_FUNCTIONS.iter().any(|k| word.eq_ignore_ascii_case(k)) {
                Ok((span, n))
            } else {
                Err(())
            }
        },
    )
    .parse_complete(i)
}

type Points = Vec<(ValueBuilder, ValueBuilder)>;

/// The points of a piecewise-linear controlled source, written `x1,y1 x2,y2`,
/// `(x1,y1) (x2,y2)` or `x1 y1 x2 y2`, with its parameters (`DELTA=`,
/// `SMOOTH=`, ...) among them
fn pwl_points(mut i: LocatedSpan) -> IResult<LocatedSpan, (Points, Vec<KeyValueBuilder>)> {
    let base = i;
    let mut coords = Vec::new();
    let mut params = Vec::new();
    while let Ok((j, ())) = node_sep(i) {
        if let Ok((k, kv)) = key_value(j) {
            params.push(kv);
            i = k;
        } else if let Ok((k, v)) = value(j) {
            coords.push(v);
            i = k;
        } else {
            break;
        }
    }
    let (i, _) = opt(trailing_delims).parse_complete(i)?;
    if coords.is_empty() || coords.len() % 2 != 0 {
        return fail(base, ErrorKind::Verify);
    }
    let points = coords
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| (c[0], c[1]))
        .collect();
    Ok((i, (points, params)))
}

/// For an `E`/`F`/`G`/`H` element written in a form that is not supported
/// (e.g. `DELAY`, `PWL(1)`, `LAPLACE`): the keyword and its position.
pub(super) fn unsupported_controlled_form<'a>(
    i: LocatedSpan<'a>,
) -> Option<(&'a str, LocatedSpan<'a>)> {
    let (i, (first, _)) = name_char(i).ok()?;
    if !matches!(first.to_ascii_lowercase(), b'e' | b'f' | b'g' | b'h') {
        return None;
    }
    let (i, _) = (
        multiline_sep(name),
        multiline_sep(name),
        opt(multiline_sep(controlled_keyword)),
    )
        .parse_complete(i)
        .ok()?;
    unsupported_controlled_form_after_keyword(i)
}

/// A coefficient: a value that does not start a `key=value` parameter
#[inline]
fn coeff(i: LocatedSpan) -> IResult<LocatedSpan, ValueBuilder> {
    if key_value(i).is_ok() {
        return fail(i, ErrorKind::Verify);
    }
    value(i)
}

/// ``` spice
/// Exxx n+ n- [VCVS] in+ in- gain [k=v ...]
/// Exxx n+ n- [VCVS] POLY(ndim) in1+ in1- ... p0 p1 ... [k=v ...]
/// Exxx n+ n- VOL='expr' [k=v ...]
/// Fxxx n+ n- [CCCS] vsrc gain [k=v ...]
/// Fxxx n+ n- [CCCS] POLY(ndim) vsrc1 ... p0 p1 ... [k=v ...]
/// ```
fn controlled(i: LocatedSpan, kind: ControlledKind) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    // (nodes may be delimited by `(`, `)` and `,`: `E1 o 0 (p,n) 1`)
    let (i, (n1, n2, keyword)) = (
        preceded(node_sep, name),
        preceded(node_sep, name),
        opt(multiline_sep(controlled_keyword)),
    )
        .parse_complete(i)?;
    if let Some((_, pos)) = unsupported_controlled_form_after_keyword(i) {
        return fail(pos, ErrorKind::Verify);
    }
    let current_controlled = matches!(kind, ControlledKind::F | ControlledKind::H);
    let mut ctrl_nodes = Vec::new();
    let mut ctrl_sources = Vec::new();
    let mut pwl = None;
    let mut params = Vec::new();
    let (i, poly) = opt(multiline_sep(poly)).parse_complete(i)?;
    let (i, pwl_fn) = match poly {
        Some(_) => (i, None),
        None => opt(multiline_sep(pwl_function)).parse_complete(i)?,
    };
    let (i, coeffs) = if let Some(n) = poly.or(pwl_fn.map(|(_, n)| n)) {
        let i = if current_controlled {
            let (i, sources) = count(preceded(node_sep, name), n).parse_complete(i)?;
            ctrl_sources = sources;
            i
        } else {
            let (i, pairs) =
                count((preceded(node_sep, name), preceded(node_sep, name)), n).parse_complete(i)?;
            ctrl_nodes = pairs;
            i
        };
        if let Some((function, ndim)) = pwl_fn {
            let (i, (points, pwl_params)) = pwl_points(i)?;
            pwl = Some(Box::new(ControlledPwlBuilder {
                function,
                ndim,
                points,
            }));
            params = pwl_params;
            (i, Vec::new())
        } else {
            many0(multiline_sep(coeff)).parse_complete(i)?
        }
    } else if multiline_sep(key_value).parse_complete(i).is_ok() {
        // behavioral: `VOL='...'` or `CUR='...'`
        (i, Vec::new())
    } else if current_controlled {
        let (i, (vsrc, gain)) =
            (preceded(node_sep, name), preceded(node_sep, value)).parse_complete(i)?;
        ctrl_sources.push(vsrc);
        (i, vec![gain])
    } else {
        let (i, (p, n, gain)) = (
            preceded(node_sep, name),
            preceded(node_sep, name),
            preceded(node_sep, value),
        )
            .parse_complete(i)?;
        ctrl_nodes.push((p, n));
        (i, vec![gain])
    };
    let (i, more) = many0(multiline_sep(key_value)).parse_complete(i)?;
    params.extend(more);
    Ok((
        i,
        InstanceCtxBuilder::ControlledSource(ControlledSourceBuilder {
            kind,
            n1,
            n2,
            keyword,
            poly,
            pwl,
            ctrl_nodes,
            ctrl_sources,
            coeffs,
            params,
        }),
    ))
}

/// The unsupported-form check of [`unsupported_controlled_form`], after the keyword
#[inline]
fn unsupported_controlled_form_after_keyword(
    i: LocatedSpan<'_>,
) -> Option<(&str, LocatedSpan<'_>)> {
    let (j, ()) = item_sep(i).ok()?;
    let (_, (word, _)) = key_str(j).ok()?;
    UNSUPPORTED_CONTROLLED
        .iter()
        .any(|k| word.eq_ignore_ascii_case(k))
        .then_some((word, j))
}

/// `Kxxx Lyyy Lzzz [K=]coupling`
#[inline]
fn _mutual_inductor(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    let base = i;
    let (i, (l1, l2, (positional, mut params))) =
        (multiline_sep(name), multiline_sep(name), args).parse_complete(i)?;
    let keyed = params
        .iter()
        .position(|kv| span_text(base, kv.k).eq_ignore_ascii_case("k"))
        .map(|idx| params.remove(idx).v);
    let coupling = match (keyed, positional.as_slice()) {
        (Some(k), []) => k,
        (None, [(_, k)]) => *k,
        _ => return fail(base, ErrorKind::Verify),
    };
    Ok((
        i,
        InstanceCtxBuilder::MutualInductor(MutualInductorBuilder {
            l1,
            l2,
            coupling,
            params,
        }),
    ))
}

#[cfg(test)]
mod tests;
