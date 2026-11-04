use crate::{
    ast::ValueBuilder,
    span::{LocatedSpan, Span},
};

use super::super::parser::utils::{loss_sep, multiline_sep, name, name_str, v, value};
use nom::{
    IResult, Parser,
    branch::alt,
    character::char,
    combinator::{map, map_res, opt},
    multi::many1,
};

#[inline]
fn subckt(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    map_res(
        (name_str, loss_sep, char('='), loss_sep, name),
        |((keyword, _), _, _, _, name)| {
            if keyword.to_lowercase().eq("subckt") {
                Ok(name)
            } else {
                Err("want subckt")
            }
        },
    )
    .parse_complete(i)
}

/// A node: `V(node)` or `node`, but not the `subckt=` keyword
#[inline]
fn node(i: LocatedSpan) -> IResult<LocatedSpan, Span> {
    alt((
        v,
        map_res(name_str, |(s, span)| {
            if s.eq_ignore_ascii_case("subckt") {
                Err(())
            } else {
                Ok(span)
            }
        }),
    ))
    .parse_complete(i)
}

/// `V(node)=val`, `node=val` or `node val`
#[inline]
fn node_volt(i: LocatedSpan) -> IResult<LocatedSpan, (Span, ValueBuilder)> {
    map(
        (node, loss_sep, opt(char('=')), loss_sep, value),
        |(node, _, _, _, val)| (node, val),
    )
    .parse_complete(i)
}

#[inline]
pub(super) fn init_condition(
    i: LocatedSpan,
) -> IResult<LocatedSpan, impl Iterator<Item = (Span, ValueBuilder, Option<Span>)>> {
    map(
        (many1(multiline_sep(node_volt)), opt(multiline_sep(subckt))),
        |(iter, opt_subckt)| {
            iter.into_iter()
                .map(move |(node, val)| (node, val, opt_subckt))
        },
    )
    .parse_complete(i)
}

#[inline]
pub(super) fn nodeset(
    i: LocatedSpan,
) -> IResult<LocatedSpan, impl Iterator<Item = (Span, ValueBuilder, Option<Span>)>> {
    map(
        (many1(multiline_sep(node_volt)), opt(multiline_sep(subckt))),
        |(iter, opt_subckt)| {
            iter.into_iter()
                .map(move |(node, val)| (node, val, opt_subckt))
        },
    )
    .parse_complete(i)
}
