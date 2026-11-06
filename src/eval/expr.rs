//! An expression: its parser, and the functions that HSPICE knows.
use super::*;

/// `x**y`, as HSPICE computes it: 0 for a zero base and any `y`, and the
/// integer part of `y` for a negative base
pub(super) fn power(x: f64, y: f64) -> f64 {
    if x == 0.0 {
        0.0
    } else if x < 0.0 {
        x.powf(y.trunc())
    } else {
        x.powf(y)
    }
}

/// `exp`, as HSPICE computes it: its argument stops at [`EXP_LIMIT`] at both ends
fn limited_exp(x: f64) -> f64 {
    x.clamp(-EXP_LIMIT, EXP_LIMIT).exp()
}

/// A logarithm of `|x|`, from [`LOG_FROM`] on, with the negative of the result
/// for `x < 0`
fn signed_log(x: f64, log: fn(f64) -> f64) -> f64 {
    let y = log(x.abs().max(LOG_FROM));
    if x < 0.0 { -y } else { y }
}

/// HSPICE's built-in functions; `None` if `name` is not one
pub(super) fn builtin(name: &str, a: &[f64]) -> Option<Result<f64, EvalError>> {
    let arity = |expected: &'static str| EvalError::Arguments {
        function: name.to_owned(),
        expected,
        found: a.len(),
    };
    let one = |f: fn(f64) -> f64| match a {
        [x] => Ok(f(*x)),
        _ => Err(arity("1")),
    };
    let two = |f: fn(f64, f64) -> f64| match a {
        [x, y] => Ok(f(*x, *y)),
        _ => Err(arity("2")),
    };
    let sgn = |x: f64| {
        if x > 0.0 {
            1.0
        } else if x < 0.0 {
            -1.0
        } else {
            0.0
        }
    };
    Some(match name {
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        // (out of range: 0, as measured)
        "asin" => one(|x| if x.abs() > 1.0 { 0.0 } else { x.asin() }),
        "acos" => one(|x| if x.abs() > 1.0 { 0.0 } else { x.acos() }),
        "atan" => one(f64::atan),
        "sinh" => one(|x| (limited_exp(x) - limited_exp(-x)) / 2.0),
        "cosh" => one(|x| (limited_exp(x) + limited_exp(-x)) / 2.0),
        "tanh" => one(f64::tanh),
        "asinh" => one(f64::asinh),
        "acosh" => one(f64::acosh),
        "atanh" => one(f64::atanh),
        // (a negative argument gives the negative of the result for `|x|`)
        "sqrt" => one(|x| x.signum() * x.abs().sqrt()),
        // (its argument stops at 80 at both ends, as measured: deck `f_exp_big`)
        "exp" => one(limited_exp),
        // natural logarithm, as in HSPICE
        // (`log(0)` is the logarithm of 1e-28, as measured: decks `g_log*`)
        "log" | "ln" => one(|x| signed_log(x, f64::ln)),
        "log10" => one(|x| signed_log(x, f64::log10)),
        "abs" => one(f64::abs),
        "floor" => one(f64::floor),
        "ceil" => one(f64::ceil),
        "int" => one(f64::trunc),
        "trunc" => one(f64::trunc),
        "nint" | "round" => one(f64::round),
        "sgn" => one(sgn),
        "db" => one(|x| 20.0 * signed_log(x, f64::log10)),
        // `pow`: the power with the integer part of the exponent;
        // `pwr`: the sign of `x` times `|x|^y`; both 0 for a zero base, as measured
        "pow" => two(|x, y| if x == 0.0 { 0.0 } else { x.powf(y.trunc()) }),
        "pwr" => two(|x, y| {
            if x == 0.0 {
                0.0
            } else {
                x.signum() * x.abs().powf(y)
            }
        }),
        "sign" => two(|x, y| if y >= 0.0 { x.abs() } else { -x.abs() }),
        "fmod" => two(|x, y| x % y),
        "atan2" => two(f64::atan2),
        "hypot" => two(f64::hypot),
        "min" | "max" if a.is_empty() => Err(arity("1 or more")),
        "min" => Ok(a.iter().copied().fold(f64::INFINITY, f64::min)),
        "max" => Ok(a.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

/// A parsed expression
#[derive(Debug)]
pub(super) enum Expr {
    Num(f64),
    /// a parameter, lowercase
    Var(String),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    /// operators of one precedence, left to right: a long chain is not a deep tree
    Chain(Box<Expr>, Vec<(Op, Expr)>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// a function call; the name is lowercase
    Call(String, Vec<Expr>),
}

/// `first`, then the operators `rest`
pub(super) fn chain(first: Expr, rest: Vec<(Op, Expr)>) -> Expr {
    if rest.is_empty() {
        first
    } else {
        Expr::Chain(Box::new(first), rest)
    }
}

/// A recursive-descent parser, by increasing precedence:
/// `?:`, `||`, `&&`, `== !=`, `< <= > >=`, `+ -`, `* /`, unary `- + !`,
/// `** ^`. As in HSPICE, `** ^` associate to the left (`2**3**2` is 64) and bind
/// tighter than a unary minus before them (`-2**2` is -4); a unary minus right
/// after them takes the next operand only (`2**-3**2` is `(2**-3)**2`).
pub(super) struct Parser<'e> {
    s: &'e str,
    pos: usize,
    depth: usize,
}

impl<'e> Parser<'e> {
    pub(super) fn new(s: &'e str) -> Self {
        Self {
            s,
            pos: 0,
            depth: 0,
        }
    }

    pub(super) fn parse(mut self) -> Result<Expr, EvalError> {
        let e = self.ternary()?;
        self.ws();
        if self.pos < self.s.len() {
            return Err(self.error(format!("unexpected `{}`", &self.s[self.pos..])));
        }
        Ok(e)
    }

    /// The error on one line (the text of a value can hold `+` lines)
    fn error(&self, message: String) -> EvalError {
        EvalError::Syntax {
            expr: self.s.trim().replace(['\r', '\n'], " "),
            message: message.replace(['\r', '\n'], " "),
        }
    }

    /// Skip white space, and the `+` of a continuation line
    fn ws(&mut self) {
        let b = self.s.as_bytes();
        let mut line_start = false;
        while let Some(&c) = b.get(self.pos) {
            match c {
                // (quotes only come in the arguments of a call, `agauss(0,0,'2')`)
                b' ' | b'\t' | b'\r' | b'\'' | b'"' => {}
                b'\n' => line_start = true,
                b'+' if line_start => line_start = false,
                _ => break,
            }
            self.pos += 1;
        }
    }

    /// Consume `token` (after white space) if it is next
    fn eat(&mut self, token: &str) -> bool {
        self.ws();
        if self.s[self.pos..].starts_with(token) {
            self.pos += token.len();
            true
        } else {
            false
        }
    }

    /// Whether `token` is next, but not `longer` (e.g. `*` but not `**`)
    fn peek_not(&mut self, token: &str, longer: &str) -> bool {
        self.ws();
        let rest = &self.s[self.pos..];
        rest.starts_with(token) && !rest.starts_with(longer)
    }

    /// Count one more level of nesting
    fn nest(&mut self) -> Result<(), EvalError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("expression nested too deeply".to_owned()));
        }
        Ok(())
    }

    fn ternary(&mut self) -> Result<Expr, EvalError> {
        self.nest()?;
        let cond = self.or()?;
        let e = if self.eat("?") {
            let a = self.ternary()?;
            if !self.eat(":") {
                return Err(self.error("`?` without `:`".to_owned()));
            }
            let b = self.ternary()?;
            Expr::Cond(Box::new(cond), Box::new(a), Box::new(b))
        } else {
            cond
        };
        self.depth -= 1;
        Ok(e)
    }

    fn binary(
        &mut self,
        ops: &[(&str, &str, Op)],
        next: fn(&mut Self) -> Result<Expr, EvalError>,
    ) -> Result<Expr, EvalError> {
        let first = next(self)?;
        let mut rest = Vec::new();
        'outer: loop {
            for (token, longer, op) in ops {
                if self.peek_not(token, longer) {
                    self.pos += token.len();
                    rest.push((*op, next(self)?));
                    continue 'outer;
                }
            }
            return Ok(chain(first, rest));
        }
    }

    fn or(&mut self) -> Result<Expr, EvalError> {
        self.binary(&[("||", "|||", Op::Or)], Self::and)
    }
    fn and(&mut self) -> Result<Expr, EvalError> {
        self.binary(&[("&&", "&&&", Op::And)], Self::equality)
    }
    fn equality(&mut self) -> Result<Expr, EvalError> {
        self.binary(
            &[("==", "===", Op::Eq), ("!=", "!==", Op::Ne)],
            Self::relational,
        )
    }
    fn relational(&mut self) -> Result<Expr, EvalError> {
        self.binary(
            &[
                ("<=", "<==", Op::Le),
                (">=", ">==", Op::Ge),
                ("<", "<=", Op::Lt),
                (">", ">=", Op::Gt),
            ],
            Self::additive,
        )
    }
    fn additive(&mut self) -> Result<Expr, EvalError> {
        // (`a--b` is a binary sign and the sign of `b`, as measured; three signs stop)
        self.binary(
            &[("+", "+++", Op::Add), ("-", "---", Op::Sub)],
            Self::multiplicative,
        )
    }
    fn multiplicative(&mut self) -> Result<Expr, EvalError> {
        self.binary(&[("*", "**", Op::Mul), ("/", "//", Op::Div)], Self::unary)
    }

    fn unary(&mut self) -> Result<Expr, EvalError> {
        self.nest()?;
        let e = if self.eat("-") {
            Expr::Neg(Box::new(self.unary()?))
        } else if self.eat("+") {
            self.unary()?
        } else if self.peek_not("!", "!=") {
            self.pos += 1;
            Expr::Not(Box::new(self.unary()?))
        } else {
            self.power()?
        };
        self.depth -= 1;
        Ok(e)
    }

    fn power(&mut self) -> Result<Expr, EvalError> {
        let base = self.primary()?;
        let mut rest = Vec::new();
        while self.eat("**") || self.eat("^") {
            rest.push((Op::Pow, self.exponent()?));
        }
        Ok(chain(base, rest))
    }

    fn exponent(&mut self) -> Result<Expr, EvalError> {
        self.nest()?;
        let e = if self.eat("-") {
            Expr::Neg(Box::new(self.exponent()?))
        } else if self.eat("+") {
            self.exponent()?
        } else if self.peek_not("!", "!=") {
            self.pos += 1;
            Expr::Not(Box::new(self.exponent()?))
        } else {
            self.primary()?
        };
        self.depth -= 1;
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, EvalError> {
        self.ws();
        let rest = &self.s[self.pos..];
        let Some(c) = rest.chars().next() else {
            return Err(self.error("missing value".to_owned()));
        };
        if c == '(' {
            self.pos += 1;
            let e = self.ternary()?;
            if !self.eat(")") {
                return Err(self.error("`(` without `)`".to_owned()));
            }
            return Ok(e);
        }
        if c.is_ascii_digit() || c == '.' {
            let Some((v, len)) = crate::parser::number_prefix(rest) else {
                return Err(self.error(format!("bad number `{rest}`")));
            };
            self.pos += len;
            return Ok(Expr::Num(v));
        }
        if c.is_alphabetic() || c == '_' || is_raw_byte(c) {
            let len = rest
                .find(|c: char| !(c.is_alphanumeric() || "_.$".contains(c) || is_raw_byte(c)))
                .unwrap_or(rest.len());
            let name = rest[..len].to_ascii_lowercase();
            self.pos += len;
            if self.eat("(") {
                let mut args = Vec::new();
                if !self.eat(")") {
                    loop {
                        args.push(self.ternary()?);
                        if self.eat(")") {
                            break;
                        }
                        if !self.eat(",") {
                            return Err(self.error(format!("bad arguments of `{name}`")));
                        }
                    }
                }
                return Ok(Expr::Call(name, args));
            }
            return Ok(Expr::Var(name));
        }
        Err(self.error(format!("unexpected `{c}`")))
    }
}
