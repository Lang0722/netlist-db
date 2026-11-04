use super::utils::{
    close_parens, continuation, integer, key_str, key_value, loss_sep, multiline_sep, name,
    name_char, node_sep, ports_params, space, space1, span_text, trailing_delims, value,
};
use crate::{
    ast::{KeyValueBuilder, ValueBuilder},
    instance::{
        ACBuilder, BJTBuilder, CapacitorBuilder, ControlledKind, ControlledPwlBuilder,
        ControlledSourceBuilder, CurrentBuilder, CurrentSourceBuilder, DiodeBuilder,
        InductorBuilder, InstanceBuilder, InstanceCtxBuilder, MOSFETBuilder, MutualInductorBuilder,
        PWLBuilder, ResistorBuilder, SourceSpecBuilder, SubcktBuilder, TimeValuePointBuilder,
        TranSourceBuilder, VoltageBuilder, VoltageSourceBuilder,
    },
    span::{LocatedSpan, Span},
};
use nom::{
    IResult, Parser,
    branch::alt,
    character::char,
    combinator::{map, map_res, opt},
    error::{Error, ErrorKind},
    multi::{count, many0, many1},
    sequence::preceded,
};

#[inline]
fn fail<T>(i: LocatedSpan, kind: ErrorKind) -> IResult<LocatedSpan, T> {
    Err(nom::Err::Error(Error::new(i, kind)))
}

#[inline]
pub(super) fn instance(i: LocatedSpan) -> IResult<LocatedSpan, InstanceBuilder> {
    let start = i;
    let (i, (first_char, name)) = name_char(i)?;
    // element names start with a letter: anything else is not an element
    if !first_char.is_ascii_alphabetic() {
        return fail(start, ErrorKind::Alpha);
    }
    let parser = match first_char.to_ascii_lowercase() {
        b'r' => _resistor,
        b'c' => _capacitor,
        b'l' => _inductor,
        b'v' => _voltage,
        b'i' => _current,
        b'm' => _mosfet,
        b'q' => _bjt,
        b'd' => _diode,
        b'x' => _subckt,
        b'e' => _vcvs,
        b'f' => _cccs,
        b'g' => _vccs,
        b'h' => _ccvs,
        b'k' => _mutual_inductor,
        r#type => {
            return map(ports_params, |(nodes, params)| InstanceBuilder {
                name,
                ctx: InstanceCtxBuilder::Unknown {
                    r#type,
                    nodes,
                    params,
                },
            })
            .parse_complete(i);
        }
    };
    map(parser, |ctx| InstanceBuilder { name, ctx }).parse_complete(i)
}

/// A positional value or a `key=value` parameter
enum Arg {
    Pos(ValueBuilder),
    Kv(KeyValueBuilder),
}

#[inline]
fn arg(i: LocatedSpan) -> IResult<LocatedSpan, Arg> {
    alt((map(key_value, Arg::Kv), map(value, Arg::Pos))).parse_complete(i)
}

/// Positional values and `key=value` parameters, in any order
#[inline]
fn args(i: LocatedSpan) -> IResult<LocatedSpan, (Vec<ValueBuilder>, Vec<KeyValueBuilder>)> {
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

/// Take the parameter named `key` (case-insensitive) out of `params`
fn take_param(
    base: LocatedSpan,
    params: &mut Vec<KeyValueBuilder>,
    key: &str,
) -> Option<ValueBuilder> {
    let idx = params
        .iter()
        .position(|kv| span_text(base, kv.k).eq_ignore_ascii_case(key))?;
    Some(params.remove(idx).v)
}

type TwoTerminal = (Span, Span, ValueBuilder, Option<Span>, Vec<KeyValueBuilder>);

/// `Rxxx n1 n2 [mname] [R=]value [k=v ...]`, and likewise `C` and `L`.
///
/// `model_first`: whether the model name comes before the value (`R`, `C`)
/// or after it (`L`), when both are names.
fn two_terminal<'a>(
    i: LocatedSpan<'a>,
    key: &str,
    model_first: bool,
) -> IResult<LocatedSpan<'a>, TwoTerminal> {
    let base = i;
    // (the nodes may be in parentheses: `R1 (a b) 1k`)
    let (i, (n1, n2, _, (positional, mut params))) = (
        preceded(node_sep, name),
        preceded(node_sep, name),
        opt(close_parens),
        args,
    )
        .parse_complete(i)?;
    let keyed = take_param(base, &mut params, key);
    let (value, mname) = match (keyed, positional.as_slice()) {
        (Some(value), []) => (value, None),
        (Some(value), [ValueBuilder::Expr(mname)]) => (value, Some(*mname)),
        (None, [value]) => (*value, None),
        (None, [ValueBuilder::Expr(mname), value @ ValueBuilder::Num(_)])
        | (None, [value @ ValueBuilder::Num(_), ValueBuilder::Expr(mname)]) => {
            (*value, Some(*mname))
        }
        (
            None,
            [
                first @ ValueBuilder::Expr(a),
                second @ ValueBuilder::Expr(b),
            ],
        ) => {
            if model_first {
                (*second, Some(*a))
            } else {
                (*first, Some(*b))
            }
        }
        _ => return fail(base, ErrorKind::Verify),
    };
    Ok((i, (n1, n2, value, mname, params)))
}

#[inline]
fn _resistor(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    let (i, (n1, n2, value, mname, params)) = two_terminal(i, "R", true)?;
    Ok((
        i,
        InstanceCtxBuilder::Resistor(ResistorBuilder {
            n1,
            n2,
            value,
            mname,
            params,
        }),
    ))
}
#[inline]
fn _capacitor(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    let (i, (n1, n2, value, mname, params)) = two_terminal(i, "C", true)?;
    Ok((
        i,
        InstanceCtxBuilder::Capacitor(CapacitorBuilder {
            n1,
            n2,
            value,
            mname,
            params,
        }),
    ))
}
#[inline]
fn _inductor(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    let (i, (n1, n2, value, mname, params)) = two_terminal(i, "L", false)?;
    Ok((
        i,
        InstanceCtxBuilder::Inductor(InductorBuilder {
            n1,
            n2,
            value,
            mname,
            params,
        }),
    ))
}
#[inline]
fn _mosfet(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    map_res(ports_params, |(ports, params)| match ports.len() {
        4 => Ok(InstanceCtxBuilder::MOSFET(MOSFETBuilder {
            nd: ports[0],
            ng: ports[1],
            ns: ports[2],
            nb: None,
            mname: ports[3],
            params,
        })),
        5 => Ok(InstanceCtxBuilder::MOSFET(MOSFETBuilder {
            nd: ports[0],
            ng: ports[1],
            ns: ports[2],
            nb: Some(ports[3]),
            mname: ports[4],
            params,
        })),
        0 => Err("There is no model name".to_string()),
        n => Err(format!(
            "MOSFET is 3/4 ports device, but found {} port(s)",
            n - 1
        )),
    })
    .parse_complete(i)
}
#[inline]
fn _bjt(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    map_res(ports_params, |(ports, params)| match ports.len() {
        4 => Ok(InstanceCtxBuilder::BJT(BJTBuilder {
            nc: ports[0],
            nb: ports[1],
            ne: ports[2],
            ns: None,
            mname: ports[3],
            params,
        })),
        5 => Ok(InstanceCtxBuilder::BJT(BJTBuilder {
            nc: ports[0],
            nb: ports[1],
            ne: ports[2],
            ns: Some(ports[3]),
            mname: ports[4],
            params,
        })),
        0 => Err("There is no model name".to_string()),
        n => Err(format!(
            "BJT is 3/4 ports device, but found {} port(s)",
            n - 1
        )),
    })
    .parse_complete(i)
}
#[inline]
fn _diode(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    map_res(ports_params, |(ports, params)| match ports.len() {
        3 => Ok(InstanceCtxBuilder::Diode(DiodeBuilder {
            nplus: ports[0],
            nminus: ports[1],
            mname: ports[2],
            params,
        })),
        0 => Err("There is no model name".to_string()),
        n => Err(format!(
            "Diode is 2 ports device, but found {} port(s)",
            n - 1
        )),
    })
    .parse_complete(i)
}
#[inline]
fn _subckt(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    let base = i;
    map_res(ports_params, |(mut nodes, params)| {
        let Some(mut cktname) = nodes.pop() else {
            return Err("There is no subckt name");
        };
        // CDL puts a `/` before the subckt name: `XI0 A B / INV` or `XI0 A B /INV`
        let text = span_text(base, cktname);
        if text.len() > 1 && text.starts_with('/') {
            cktname.start += 1;
        }
        if nodes.last().is_some_and(|n| span_text(base, *n) == "/") {
            nodes.pop();
        }
        Ok(InstanceCtxBuilder::Subckt(SubcktBuilder {
            nodes,
            cktname,
            params,
        }))
    })
    .parse_complete(i)
}

/// Keywords that start an item of an independent source
const SOURCE_KEYWORDS: [&str; 10] = [
    "DC", "AC", "PWL", "PULSE", "SIN", "EXP", "SFFM", "AM", "PAT", "LFSR",
];

#[inline]
fn is_keyword(i: LocatedSpan, keywords: &[&str]) -> bool {
    key_str(i).is_ok_and(|(_, (word, _))| keywords.iter().any(|k| word.eq_ignore_ascii_case(k)))
}

/// Separator between items: spaces, or a line continuation
#[inline]
fn item_sep(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    alt((continuation, map(space1, |_| ()))).parse_complete(i)
}

/// Separator between arguments in parentheses: spaces, a comma, or a
/// continuation line
#[inline]
fn arg_sep(i: LocatedSpan) -> IResult<LocatedSpan, ()> {
    let (j, _) = (loss_sep, opt(char(',')), loss_sep).parse_complete(i)?;
    if j.len() == i.len() {
        fail(i, ErrorKind::Space)
    } else {
        Ok((j, ()))
    }
}

/// A value that does not start a `key=value` parameter or a source keyword
#[inline]
fn plain_value(i: LocatedSpan) -> IResult<LocatedSpan, ValueBuilder> {
    if key_value(i).is_ok() || is_keyword(i, &SOURCE_KEYWORDS) {
        return fail(i, ErrorKind::Verify);
    }
    value(i)
}

/// `KEYWORD value` or `KEYWORD=value`, after the keyword
#[inline]
fn keyword_value(i: LocatedSpan) -> IResult<LocatedSpan, ValueBuilder> {
    alt((
        preceded((space, char('='), space), value),
        preceded(item_sep, plain_value),
    ))
    .parse_complete(i)
}

/// Function arguments after the function name: `(a b ... k=v ...)`, or
/// without parentheses, values up to the next keyword or `key=value`
fn func_args(i: LocatedSpan) -> IResult<LocatedSpan, (Vec<ValueBuilder>, Vec<KeyValueBuilder>)> {
    let Ok((mut i, _)) = (loss_sep, char::<_, Error<_>>('('), loss_sep).parse_complete(i) else {
        let (i, args) = many1(preceded(item_sep, plain_value)).parse_complete(i)?;
        return Ok((i, (args, Vec::new())));
    };
    let mut args = Vec::new();
    let mut params = Vec::new();
    let mut first = true;
    loop {
        let j = if first {
            i
        } else {
            match arg_sep(i) {
                Ok((j, ())) => j,
                Err(_) => break,
            }
        };
        if let Ok((k, kv)) = key_value(j) {
            params.push(kv);
            i = k;
        } else if let Ok((k, v)) = value(j) {
            args.push(v);
            i = k;
        } else {
            break;
        }
        first = false;
    }
    let (i, _) = (loss_sep, char(')')).parse_complete(i)?;
    Ok((i, (args, params)))
}

/// `PWL(t1 v1 t2 v2 ... [R[=repeat]] [TD=delay] ...)`, after the keyword,
/// with or without parentheses
fn pwl(i: LocatedSpan) -> IResult<LocatedSpan, PWLBuilder> {
    let base = i;
    let paren = (loss_sep, char::<_, Error<_>>('('), loss_sep).parse_complete(i);
    let in_parens = paren.is_ok();
    let mut i = paren.map_or(i, |(i, _)| i);
    let mut pwl = PWLBuilder::default();
    let mut coords = Vec::new();
    let mut first = true;
    loop {
        let sep = if in_parens && first {
            Ok((i, ()))
        } else {
            // (commas are delimiters too: `PWL 0n 0, 4n 0, 4.05n 5`)
            arg_sep(i)
        };
        let Ok((j, ())) = sep else { break };
        if let Ok((k, kv)) = key_value(j) {
            let field = match span_text(base, kv.k).to_ascii_uppercase().as_str() {
                "TD" => &mut pwl.delay,
                "R" => &mut pwl.repeat,
                "RSTOP" => &mut pwl.rstop,
                "STOPVALUE" => &mut pwl.stopvalue,
                "STOPSLOPE" => &mut pwl.stopslope,
                "PWLFILE" => &mut pwl.pwlfile,
                // without parentheses, other parameters belong to the source
                _ if !in_parens => break,
                _ => return fail(j, ErrorKind::Verify),
            };
            *field = Some(kv.v);
            i = k;
        } else if key_str(j).is_ok_and(|(_, (word, _))| word.eq_ignore_ascii_case("R")) {
            // `R` alone: repeat from the beginning
            (i, _) = key_str(j)?;
            pwl.repeat = Some(ValueBuilder::Num(0.0));
        } else if !in_parens && is_keyword(j, &SOURCE_KEYWORDS) {
            break;
        } else if let Ok((k, v)) = value(j) {
            coords.push(v);
            i = k;
        } else {
            break;
        }
        first = false;
    }
    if in_parens {
        (i, _) = (loss_sep, char(')')).parse_complete(i)?;
    }
    if (coords.is_empty() && pwl.pwlfile.is_none()) || coords.len() % 2 != 0 {
        return fail(base, ErrorKind::Verify);
    }
    pwl.points = coords
        .chunks_exact(2)
        .map(|c| TimeValuePointBuilder {
            time: c[0],
            value: c[1],
        })
        .collect();
    Ok((i, pwl))
}

/// The items of an independent source after its nodes:
/// ``` spice
/// [[DC] dcval] [tranfun] [AC [=] acmag [acphase]] [M=val] ...
/// ```
fn source(mut i: LocatedSpan) -> IResult<LocatedSpan, SourceSpecBuilder> {
    let mut spec = SourceSpecBuilder::default();
    let mut first = true;
    while let Ok((j, ())) = item_sep(i) {
        let keyword = key_str(j).ok().and_then(|(k, (word, span))| {
            SOURCE_KEYWORDS
                .iter()
                .find(|kw| word.eq_ignore_ascii_case(kw))
                .map(|kw| (k, *kw, span))
        });
        if let Some((k, kw, span)) = keyword {
            match kw {
                "DC" => {
                    let (k, v) = keyword_value(k)?;
                    spec.dc = Some(v);
                    i = k;
                }
                "AC" => {
                    let (k, mag) = keyword_value(k)?;
                    let (k, phase) = opt(preceded(arg_sep, plain_value)).parse_complete(k)?;
                    spec.ac = Some(ACBuilder { mag, phase });
                    i = k;
                }
                "PWL" => {
                    let (k, t) = pwl(k)?;
                    spec.tran = Some(TranSourceBuilder::PWL(t));
                    i = k;
                }
                _ => match func_args(k) {
                    Ok((k, (args, params))) => {
                        spec.tran = Some(TranSourceBuilder::Func {
                            name: span,
                            args,
                            params,
                        });
                        i = k;
                    }
                    // not a function call after all, e.g. a parameter named `exp`
                    Err(_) if first => {
                        let (k, v) = value(j)?;
                        spec.dc = Some(v);
                        i = k;
                    }
                    Err(e) => return Err(e),
                },
            }
        } else if let Ok((k, kv)) = key_value(j) {
            spec.params.push(kv);
            i = k;
        } else if first {
            // `Vxxx n+ n- dcval`; `Vxxx n+ n-` alone has no value
            let Ok((k, v)) = value(j) else { break };
            spec.dc = Some(v);
            i = k;
        } else {
            break;
        }
        first = false;
    }
    Ok((i, spec))
}

#[inline]
fn _voltage(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    let (i, (n1, n2, _, spec)) = (
        preceded(node_sep, name),
        preceded(node_sep, name),
        opt(close_parens),
        source,
    )
        .parse_complete(i)?;
    // keep the simple forms as they were before `Spec` existed
    let source = match spec {
        SourceSpecBuilder {
            dc: Some(v),
            ac: None,
            tran: None,
            params,
        } if params.is_empty() => VoltageSourceBuilder::Value(v),
        SourceSpecBuilder {
            dc: None,
            ac: None,
            tran: Some(TranSourceBuilder::PWL(pwl)),
            params,
        } if params.is_empty() => VoltageSourceBuilder::PWL(Box::new(pwl)),
        SourceSpecBuilder {
            dc: None,
            ac: None,
            tran: None,
            params,
        } => VoltageSourceBuilder::Params(params),
        spec => VoltageSourceBuilder::Spec(Box::new(spec)),
    };
    Ok((
        i,
        InstanceCtxBuilder::Voltage(VoltageBuilder { n1, n2, source }),
    ))
}
#[inline]
fn _current(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    let (i, (n1, n2, _, spec)) = (
        preceded(node_sep, name),
        preceded(node_sep, name),
        opt(close_parens),
        source,
    )
        .parse_complete(i)?;
    let source = match spec {
        SourceSpecBuilder {
            dc: Some(v),
            ac: None,
            tran: None,
            params,
        } if params.is_empty() => CurrentSourceBuilder::Value(v),
        SourceSpecBuilder {
            dc: None,
            ac: None,
            tran: Some(TranSourceBuilder::PWL(pwl)),
            params,
        } if params.is_empty() => CurrentSourceBuilder::PWL(Box::new(pwl)),
        SourceSpecBuilder {
            dc: None,
            ac: None,
            tran: None,
            params,
        } => CurrentSourceBuilder::Params(params),
        spec => CurrentSourceBuilder::Spec(Box::new(spec)),
    };
    Ok((
        i,
        InstanceCtxBuilder::Current(CurrentBuilder { n1, n2, source }),
    ))
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
    let points = coords.chunks_exact(2).map(|c| (c[0], c[1])).collect();
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

#[inline]
fn _vcvs(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    controlled(i, ControlledKind::E)
}
#[inline]
fn _cccs(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    controlled(i, ControlledKind::F)
}
#[inline]
fn _vccs(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    controlled(i, ControlledKind::G)
}
#[inline]
fn _ccvs(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    controlled(i, ControlledKind::H)
}

/// `Kxxx Lyyy Lzzz [K=]coupling`
#[inline]
fn _mutual_inductor(i: LocatedSpan) -> IResult<LocatedSpan, InstanceCtxBuilder> {
    let base = i;
    let (i, (l1, l2, (positional, mut params))) =
        (multiline_sep(name), multiline_sep(name), args).parse_complete(i)?;
    let coupling = match (take_param(base, &mut params, "K"), positional.as_slice()) {
        (Some(k), []) => k,
        (None, [k]) => *k,
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
