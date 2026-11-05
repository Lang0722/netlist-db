//! HSPICE expression parser (Pratt method).

use std::collections::HashSet;
use std::fmt;

use crate::number::scan_number;

/// Maximum nesting depth (and maximum tree height) of an expression.
/// It keeps parsing, evaluation and drop within a small thread stack.
const MAX_DEPTH: usize = 256;

/// Binding power of the prefix operators `-`, `+`, `!` (level 8). The
/// operand of a prefix operator binds at least this tightly, and at least as
/// tightly as the operator before it: `-2**2` = -(2**2), but `2**-3**2` =
/// (2**(-3))**2 (golden cases `ops` and `ops2`).
const PREFIX_BP: u8 = 16;

/// Binding powers of `? :` (level 1, right-associative).
const TERNARY_BP: (u8, u8) = (3, 2);

/// An HSPICE expression tree. `Ident` and `Call` names are ASCII lowercase.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    Ident(String),
    Call {
        name: String,
        args: Vec<Expr>,
    },
    Unary {
        op: UnOp,
        e: Box<Expr>,
    },
    Binary {
        op: BinOp,
        l: Box<Expr>,
        r: Box<Expr>,
    },
    Ternary {
        c: Box<Expr>,
        t: Box<Expr>,
        f: Box<Expr>,
    },
}

/// Prefix operators: `-`, `+`, `!`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    Neg,
    Plus,
    Not,
}

/// Infix operators. `Pow` is `**` or `^` (HSPICE: `2^3` = 8, golden case
/// `ops_caret`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Pow,
    Mul,
    Div,
    Add,
    Sub,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

/// A syntax error. `offset` is a byte offset into the text given to [`parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprError {
    pub msg: String,
    pub offset: usize,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at offset {}", self.msg, self.offset)
    }
}

impl std::error::Error for ExprError {}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BinOp::Pow => "**",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        })
    }
}

impl BinOp {
    /// Left and right binding powers. Level n (low to high): `||` 2,
    /// `&&` 3, `== !=` 4, `< <= > >=` 5, `+ -` 6, `* /` 7, `**` 9.
    /// All are left-associative: (2n, 2n+1). HSPICE P-2019.06 gives
    /// `2**3**2` = 64, so `**` is left-associative too (golden case `ops`).
    fn binding_power(self) -> (u8, u8) {
        match self {
            BinOp::Or => (4, 5),
            BinOp::And => (6, 7),
            BinOp::Eq | BinOp::Ne => (8, 9),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => (10, 11),
            BinOp::Add | BinOp::Sub => (12, 13),
            BinOp::Mul | BinOp::Div => (14, 15),
            BinOp::Pow => (18, 19),
        }
    }
}

/// Parses one HSPICE expression.
///
/// `(…)`, `{…}` and `'…'` group alike, so a pair around the whole
/// expression changes nothing (`'2*wn'` is `2*wn`), and a pair around a part
/// groups only that part (`'1'*'2'` = 2). Quotes pair in order: the first
/// opens, the next closes. Names are case-insensitive and stored in ASCII
/// lowercase. Error offsets are byte offsets into `src`.
pub fn parse(src: &str) -> Result<Expr, ExprError> {
    let mut p = Parser {
        toks: lex(src)?,
        pos: 0,
    };
    let (e, _) = p.expr_bp(0, 0)?;
    match p.peek() {
        Tok::End => Ok(e),
        Tok::Close(group) => Err(p.error(&format!("unmatched {}", group.closer()))),
        _ => Err(p.error("expected an operator")),
    }
}

/// Returns the parameter names that `e` uses, in first-use order, without
/// duplicates. Names of called functions are not included. Runs in time
/// linear in the size of `e`.
pub fn free_names(e: &Expr) -> Vec<String> {
    fn walk<'a>(e: &'a Expr, seen: &mut HashSet<&'a str>, out: &mut Vec<String>) {
        match e {
            Expr::Num(_) => {}
            Expr::Ident(name) => {
                if seen.insert(name) {
                    out.push(name.clone());
                }
            }
            Expr::Call { args, .. } => args.iter().for_each(|a| walk(a, seen, out)),
            Expr::Unary { e, .. } => walk(e, seen, out),
            Expr::Binary { l, r, .. } => {
                walk(l, seen, out);
                walk(r, seen, out);
            }
            Expr::Ternary { c, t, f } => {
                walk(c, seen, out);
                walk(t, seen, out);
                walk(f, seen, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(e, &mut HashSet::new(), &mut out);
    out
}

/// The bracket kinds that group a subexpression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Paren,
    Brace,
    Quote,
}

impl Group {
    /// The closing bracket, for error messages.
    fn closer(self) -> &'static str {
        match self {
            Group::Paren => "')'",
            Group::Brace => "'}'",
            Group::Quote => "closing quote",
        }
    }
}

#[derive(Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    /// An infix operator. `Add` and `Sub` are also the prefix `+` and `-`.
    Op(BinOp),
    Not,
    Open(Group),
    Close(Group),
    Comma,
    Question,
    Colon,
    End,
}

/// Splits `src` into tokens with their byte offsets. The last token is
/// always `End`.
fn lex(src: &str) -> Result<Vec<(Tok, usize)>, ExprError> {
    let b = src.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    let mut in_quote = false;
    loop {
        i += b[i..]
            .iter()
            .take_while(|c| c.is_ascii_whitespace())
            .count();
        let Some(&c) = b.get(i) else {
            toks.push((Tok::End, i));
            return Ok(toks);
        };
        let next = b.get(i + 1).copied();
        let (tok, len) = match (c, next) {
            (b'0'..=b'9' | b'.', _) => match scan_number(&src[i..]) {
                Some((v, len)) => (Tok::Num(v), len),
                None => {
                    return Err(ExprError {
                        msg: "expected a digit after '.'".into(),
                        offset: i,
                    });
                }
            },
            (b'a'..=b'z' | b'A'..=b'Z' | b'_', _) => {
                let len = b[i..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_')
                    .count();
                (Tok::Ident(src[i..i + len].to_ascii_lowercase()), len)
            }
            (b'*', Some(b'*')) => (Tok::Op(BinOp::Pow), 2),
            (b'^', _) => (Tok::Op(BinOp::Pow), 1),
            (b'*', _) => (Tok::Op(BinOp::Mul), 1),
            (b'/', _) => (Tok::Op(BinOp::Div), 1),
            (b'+', _) => (Tok::Op(BinOp::Add), 1),
            (b'-', _) => (Tok::Op(BinOp::Sub), 1),
            (b'<', Some(b'=')) => (Tok::Op(BinOp::Le), 2),
            (b'<', _) => (Tok::Op(BinOp::Lt), 1),
            (b'>', Some(b'=')) => (Tok::Op(BinOp::Ge), 2),
            (b'>', _) => (Tok::Op(BinOp::Gt), 1),
            (b'=', Some(b'=')) => (Tok::Op(BinOp::Eq), 2),
            (b'!', Some(b'=')) => (Tok::Op(BinOp::Ne), 2),
            (b'!', _) => (Tok::Not, 1),
            (b'&', Some(b'&')) => (Tok::Op(BinOp::And), 2),
            (b'|', Some(b'|')) => (Tok::Op(BinOp::Or), 2),
            (b'(', _) => (Tok::Open(Group::Paren), 1),
            (b')', _) => (Tok::Close(Group::Paren), 1),
            (b'{', _) => (Tok::Open(Group::Brace), 1),
            (b'}', _) => (Tok::Close(Group::Brace), 1),
            (b'\'', _) => {
                in_quote = !in_quote;
                let tok = if in_quote {
                    Tok::Open(Group::Quote)
                } else {
                    Tok::Close(Group::Quote)
                };
                (tok, 1)
            }
            (b',', _) => (Tok::Comma, 1),
            (b'?', _) => (Tok::Question, 1),
            (b':', _) => (Tok::Colon, 1),
            _ => {
                let ch = src[i..].chars().next().unwrap_or_default();
                return Err(ExprError {
                    msg: format!("unexpected character '{ch}'"),
                    offset: i,
                });
            }
        };
        toks.push((tok, i));
        i += len;
    }
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }

    /// Takes the current token out of the list (tokens are read once).
    /// At `End` the position stays at `End`.
    fn bump(&mut self) -> (Tok, usize) {
        let (tok, offset) = &mut self.toks[self.pos];
        let tok = std::mem::replace(tok, Tok::End);
        let offset = *offset;
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        (tok, offset)
    }

    /// An error at the current token.
    fn error(&self, msg: &str) -> ExprError {
        ExprError {
            msg: msg.to_string(),
            offset: self.toks[self.pos].1,
        }
    }

    fn expect(&mut self, tok: Tok, msg: &str) -> Result<(), ExprError> {
        if *self.peek() == tok {
            self.bump();
            Ok(())
        } else {
            Err(self.error(msg))
        }
    }

    /// Parses an expression whose operators bind at least `min_bp`.
    /// `depth` is the recursion depth. Returns the tree and its height.
    fn expr_bp(&mut self, min_bp: u8, depth: usize) -> Result<(Expr, usize), ExprError> {
        if depth >= MAX_DEPTH {
            return Err(self.error("expression is nested too deeply"));
        }
        let (mut lhs, mut height) = self.prefix(min_bp, depth)?;
        loop {
            let op = match self.peek() {
                Tok::Op(op) => *op,
                Tok::Question => {
                    if TERNARY_BP.0 < min_bp {
                        break;
                    }
                    self.bump();
                    let (t, t_height) = self.expr_bp(0, depth + 1)?;
                    self.expect(Tok::Colon, "expected ':'")?;
                    let (f, f_height) = self.expr_bp(TERNARY_BP.1, depth + 1)?;
                    height = 1 + height.max(t_height).max(f_height);
                    lhs = Expr::Ternary {
                        c: Box::new(lhs),
                        t: Box::new(t),
                        f: Box::new(f),
                    };
                    self.check_height(height)?;
                    continue;
                }
                _ => break,
            };
            let (l_bp, r_bp) = op.binding_power();
            if l_bp < min_bp {
                break;
            }
            self.bump();
            let (rhs, r_height) = self.expr_bp(r_bp, depth + 1)?;
            height = 1 + height.max(r_height);
            lhs = Expr::Binary {
                op,
                l: Box::new(lhs),
                r: Box::new(rhs),
            };
            self.check_height(height)?;
        }
        Ok((lhs, height))
    }

    /// Parses a number, name, call, grouped expression, or prefix operator
    /// with its operand. `min_bp` is the binding power of the operator to
    /// the left, if any.
    fn prefix(&mut self, min_bp: u8, depth: usize) -> Result<(Expr, usize), ExprError> {
        let (tok, offset) = self.bump();
        let op = match tok {
            Tok::Num(v) => return Ok((Expr::Num(v), 1)),
            Tok::Ident(name) if *self.peek() == Tok::Open(Group::Paren) => {
                return self.call(name, depth);
            }
            Tok::Ident(name) => return Ok((Expr::Ident(name), 1)),
            Tok::Open(group) => {
                let (e, height) = self.expr_bp(0, depth + 1)?;
                self.expect(Tok::Close(group), &format!("expected {}", group.closer()))?;
                return Ok((e, height));
            }
            Tok::Op(BinOp::Sub) => UnOp::Neg,
            Tok::Op(BinOp::Add) => UnOp::Plus,
            Tok::Not => UnOp::Not,
            _ => {
                return Err(ExprError {
                    msg: "expected a value".into(),
                    offset,
                });
            }
        };
        let (e, height) = self.expr_bp(PREFIX_BP.max(min_bp), depth + 1)?;
        Ok((Expr::Unary { op, e: Box::new(e) }, height + 1))
    }

    /// Parses the argument list of `name(…)`; the current token is `(`.
    fn call(&mut self, name: String, depth: usize) -> Result<(Expr, usize), ExprError> {
        self.bump();
        let mut args = Vec::new();
        let mut height = 0;
        if *self.peek() == Tok::Close(Group::Paren) {
            self.bump();
        } else {
            loop {
                let (arg, arg_height) = self.expr_bp(0, depth + 1)?;
                height = height.max(arg_height);
                args.push(arg);
                match self.bump() {
                    (Tok::Comma, _) => {}
                    (Tok::Close(Group::Paren), _) => break,
                    (_, offset) => {
                        return Err(ExprError {
                            msg: "expected ',' or ')'".into(),
                            offset,
                        });
                    }
                }
            }
        }
        Ok((Expr::Call { name, args }, height + 1))
    }

    fn check_height(&self, height: usize) -> Result<(), ExprError> {
        if height >= MAX_DEPTH {
            Err(self.error("expression is too long (nested too deeply)"))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BinOp, Expr, UnOp, free_names, parse};
    use crate::test_env::{close, eval_str, val};

    fn num(v: f64) -> Box<Expr> {
        Box::new(Expr::Num(v))
    }

    fn err_offset(src: &str) -> usize {
        match parse(src) {
            Ok(e) => panic!("{src:?} parsed as {e:?}"),
            Err(err) => err.offset,
        }
    }

    #[test]
    fn pratt_precedence() {
        assert_eq!(val("2+3*4"), 14.0);
        // HSPICE: `**` is left-associative, (2**3)**2.
        assert_eq!(val("2**3**2"), 64.0);
        assert_eq!(val("-2**2"), -4.0);
        assert_eq!(val("(1+2)*3"), 9.0);
        assert_eq!(val("1<2 && 3>2"), 1.0);
        assert_eq!(val("0||0"), 0.0);
        assert_eq!(val("1?2:3"), 2.0);
        let (v, caveats) = eval_str("2*wn", &[("wn", 1e-6)]).unwrap();
        assert!(close(v, 2e-6) && caveats.is_empty(), "{v:e}");
        // Names are case-insensitive: `WN` is the variable `wn`.
        let (v, _) = eval_str("2*WN", &[("wn", 1e-6)]).unwrap();
        assert!(close(v, 2e-6), "{v:e}");
    }

    #[test]
    fn pratt_associativity_and_levels() {
        assert_eq!(val("10-4-3"), 3.0);
        assert_eq!(val("64/4/2"), 8.0);
        assert_eq!(val("-2*3"), -6.0);
        assert_eq!(val("2**-1"), 0.5);
        assert_eq!(val("--3"), 3.0);
        assert_eq!(val("+3"), 3.0);
        assert_eq!(val("!0 + !5"), 1.0);
        assert_eq!(val("1+2 < 4"), 1.0);
        assert_eq!(val("1 < 2 == 1"), 1.0);
        assert_eq!(val("1 || 0 && 0"), 1.0);
        assert_eq!(val("0 ? 1 : 0 ? 2 : 3"), 3.0);
        assert_eq!(val("1 ? 0 ? 4 : 5 : 6"), 5.0);
        assert_eq!(val("2 >= 2"), 1.0);
        assert_eq!(val("2 <= 1"), 0.0);
        assert_eq!(val("2 != 2"), 0.0);
        assert_eq!(val("2 == 2.0"), 1.0);
        assert_eq!(
            parse("-2**2").unwrap(),
            Expr::Unary {
                op: UnOp::Neg,
                e: Box::new(Expr::Binary {
                    op: BinOp::Pow,
                    l: num(2.0),
                    r: num(2.0)
                }),
            }
        );
    }

    #[test]
    fn parse_numbers_names_and_calls() {
        assert_eq!(parse("1meg").unwrap(), Expr::Num(1e6));
        assert_eq!(parse("WN").unwrap(), Expr::Ident("wn".into()));
        assert_eq!(parse("_a1").unwrap(), Expr::Ident("_a1".into()));
        assert_eq!(
            parse("MAX(A, 2u)").unwrap(),
            Expr::Call {
                name: "max".into(),
                args: vec![Expr::Ident("a".into()), Expr::Num(2e-6)]
            }
        );
        assert_eq!(
            parse("f()").unwrap(),
            Expr::Call {
                name: "f".into(),
                args: vec![]
            }
        );
        // Whitespace includes tabs, CR and LF.
        assert_eq!(parse(" \t1\r\n+\t2 ").unwrap(), parse("1+2").unwrap());
    }

    #[test]
    fn parse_quote_and_brace_pairs_group() {
        // A pair around the whole expression changes nothing.
        assert_eq!(parse("'2*wn'").unwrap(), parse("2*wn").unwrap());
        assert_eq!(parse("{2*wn}").unwrap(), parse("2*wn").unwrap());
        assert_eq!(parse("  ' 1 + 2 '  ").unwrap(), parse("1+2").unwrap());
        // A pair that does not enclose the whole text groups only its part.
        assert_eq!(val("'1'*'2'"), 2.0);
        assert_eq!(val("{1}+{2}"), 3.0);
        assert_eq!(val("'1+2'*'3'"), 9.0);
        assert_eq!(val("{1+2}*3"), 9.0);
        assert_eq!(val("'(1+2)*{3}'"), 9.0);
        assert_eq!(val("max('1', {2})"), 2.0);
        // Quotes pair in order (the first opens, the next closes), so they
        // do not nest.
        assert!(parse("''1''").is_err());
        // Error offsets count from the start of the text given to `parse`.
        assert_eq!(err_offset("'2+'"), 3);
        assert_eq!(err_offset("'1+2"), 4);
        assert_eq!(err_offset("{1+2"), 4);
        assert_eq!(err_offset("(1}"), 2);
        assert_eq!(err_offset("{1)"), 2);
        assert_eq!(err_offset("1}"), 1);
        assert_eq!(err_offset("'1'+2'"), 5);
        assert_eq!(err_offset("('1)'"), 3);
    }

    #[test]
    fn parse_errors() {
        let err = parse("2+").unwrap_err();
        assert_eq!(err.offset, 2, "{err}");
        assert_eq!(err_offset(""), 0);
        assert_eq!(err_offset("''"), 1);
        assert_eq!(err_offset("(1"), 2);
        assert_eq!(err_offset("1)"), 1);
        assert_eq!(err_offset("2 3"), 2);
        assert_eq!(err_offset("1 ? 2"), 5);
        assert_eq!(err_offset("max(1,"), 6);
        assert_eq!(err_offset("max(1 2)"), 6);
        assert_eq!(err_offset("1 = 2"), 2);
        assert_eq!(err_offset("1 & 2"), 2);
        assert_eq!(err_offset("a$b"), 1);
        assert_eq!(err_offset("1 + ."), 4);
        assert_eq!(err_offset("1 + * 2"), 4);
    }

    #[test]
    fn golden_ops_values() {
        // Deck text and resistor values from HSPICE P-2019.06: golden cases
        // `ops`, `ops_not`, `ops_caret` and `ops2` (hspice.lis).
        let cases: &[(&str, f64)] = &[
            // ops
            ("2+3*4", 14.0),
            ("2**3**2", 64.0),
            ("10-2**2", 6.0),
            ("-2**2+10", 6.0),
            ("(1+2)*3", 9.0),
            ("8/2/2", 2.0),
            ("10-3-2", 5.0),
            ("2*-3+10", 4.0),
            ("10+(1<2)", 11.0),
            ("10+(1<2 && 3>2)", 11.0),
            ("10+(0||0)", 10.0),
            ("1?2:3", 2.0),
            ("10+(2>=2)+(2<=1)", 11.0),
            ("10+(2==2)", 11.0),
            ("10+(2!=2)", 10.0),
            ("0?2:3", 3.0),
            ("1+1>1?5:6", 5.0),
            ("+3", 3.0),
            ("2*3**2", 18.0),
            ("10+(1||0&&0)", 11.0),
            ("1?2:0?4:5", 2.0),
            ("2**0.5", std::f64::consts::SQRT_2),
            ("10+(-2)**3", 2.0),
            // ops_not
            ("10+!0", 11.0),
            ("10+!3", 10.0),
            // ops_caret
            ("2^3", 8.0),
            ("2^3^2", 64.0),
            ("-2^2+10", 6.0),
            ("2*3^2", 18.0),
            // ops2
            ("2**-3**2", 0.015625),
            ("2^-3^2", 0.015625),
            ("10+0**0", 10.0),
            ("10+0^0", 10.0),
            ("10+0**-1", 10.0),
            ("10+0**0.5", 10.0),
        ];
        for &(src, want) in cases {
            let got = val(src);
            assert!(close(got, want), "{src}: got {got}, want {want}");
        }
    }

    #[test]
    fn caret_is_power() {
        // HSPICE: `^` is `**` (golden case `ops_caret`).
        assert_eq!(parse("2^3").unwrap(), parse("2**3").unwrap());
        assert_eq!(parse("-2^2").unwrap(), parse("-2**2").unwrap());
        assert_eq!(parse("2^-3^2").unwrap(), parse("2**-3**2").unwrap());
    }

    #[test]
    fn prefix_operator_after_power_takes_one_operand() {
        // HSPICE: `2**-3**2` = (2**-3)**2 (golden case `ops2`), but
        // `-2**2` = -(2**2) (golden case `ops`).
        assert_eq!(
            parse("2**-3**2").unwrap(),
            Expr::Binary {
                op: BinOp::Pow,
                l: Box::new(Expr::Binary {
                    op: BinOp::Pow,
                    l: num(2.0),
                    r: Box::new(Expr::Unary {
                        op: UnOp::Neg,
                        e: num(3.0)
                    }),
                }),
                r: num(2.0),
            }
        );
        assert_eq!(val("2**-3*4"), 0.5);
        assert_eq!(val("-2**-2"), -0.25);
        assert_eq!(val("2**--3"), 8.0);
        assert_eq!(val("2**+3**2"), 64.0);
        assert_eq!(val("2**!0**3"), 8.0);
        assert_eq!(val("2**-(1+2)**2"), 0.015625);
        // After the other operators a prefix operator still takes a power.
        assert_eq!(val("2*-3**2"), -18.0);
        assert_eq!(val("1+-2**2"), -3.0);
    }

    #[test]
    fn deep_nesting_is_an_error_not_a_stack_overflow() {
        let deep = format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000));
        assert!(parse(&deep).is_err());
        let long_chain = vec!["1"; 100_000].join("+");
        assert!(parse(&long_chain).is_err());
        let unary = format!("{}1", "-".repeat(100_000));
        assert!(parse(&unary).is_err());
    }

    #[test]
    fn deepest_accepted_expression_fits_a_small_stack() {
        // 2 MiB is the default stack of Rust std and tokio worker threads.
        // Measured need at this depth: about 1 MiB in debug, 128 KiB in release.
        let run = || {
            let n = super::MAX_DEPTH - 1;
            let nested = format!("{}1{}", "(".repeat(n), ")".repeat(n));
            let e = parse(&nested).unwrap();
            assert_eq!(
                crate::eval(&e, &mut crate::test_env::MapEnv::new(&[]))
                    .unwrap()
                    .0,
                1.0
            );
            let chain = vec!["1"; n].join("+");
            let e = parse(&chain).unwrap();
            assert_eq!(free_names(&e), Vec::<String>::new());
            let v = crate::eval(&e, &mut crate::test_env::MapEnv::new(&[]))
                .unwrap()
                .0;
            assert_eq!(v, n as f64);
            // One level more is rejected.
            assert!(parse(&format!("({nested})")).is_err());
            assert!(parse(&format!("{chain}+1")).is_err());
        };
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(run)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn free_names_skips_called_functions() {
        let e = parse("f(a, B) + max(c, a) * d ? e : temper").unwrap();
        assert_eq!(free_names(&e), ["a", "b", "c", "d", "e", "temper"]);
        assert_eq!(free_names(&parse("1+2").unwrap()), Vec::<String>::new());
    }

    #[test]
    fn free_names_is_linear() {
        // One call with 50,000 distinct names, each used twice. A quadratic
        // duplicate check needs billions of string compares here (seconds);
        // a linear one needs milliseconds.
        let names: Vec<String> = (0..50_000).map(|i| format!("p{i}")).collect();
        let args = names
            .iter()
            .chain(names.iter().rev())
            .map(|n| Expr::Ident(n.clone()))
            .collect();
        let e = Expr::Call {
            name: "max".into(),
            args,
        };
        let start = std::time::Instant::now();
        let got = free_names(&e);
        let elapsed = start.elapsed();
        assert_eq!(got, names);
        assert!(elapsed.as_secs_f64() < 1.0, "free_names took {elapsed:?}");
    }
}
