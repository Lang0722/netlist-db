//! Evaluation of an [`Expr`] against a caller-provided [`Env`].

use std::fmt;

use crate::builtins;
use crate::parse::{BinOp, Expr, UnOp};

/// A note attached to a value that is a nominal stand-in, not a fixed number.
#[derive(Debug, Clone, PartialEq)]
pub enum Caveat {
    /// A statistical function (`agauss`, …) returned its nominal value.
    /// The string is the function name.
    StatisticalNominal(String),
    /// The value uses `temper` (the circuit temperature).
    Temper,
}

/// Why an expression has no value.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalError {
    /// The name is not defined in any visible scope.
    Undefined(String),
    /// Unknown function, or wrong number of arguments.
    BadCall(String),
    /// The math has no finite result (division by zero, overflow, …).
    Domain(String),
    /// The environment cannot give a value (for example a cycle).
    Unresolved(String),
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvalError::Undefined(name) => write!(f, "undefined name '{name}'"),
            EvalError::BadCall(msg) | EvalError::Domain(msg) | EvalError::Unresolved(msg) => {
                f.write_str(msg)
            }
        }
    }
}

impl std::error::Error for EvalError {}

/// Supplies parameter values and user `.param` functions to [`eval`].
/// Names are ASCII lowercase.
pub trait Env {
    /// The value of parameter `name`. `eval` asks for `temper` here too.
    fn var(&mut self, name: &str) -> Result<(f64, Vec<Caveat>), EvalError>;
    /// Calls user function `name`, or returns `None` if there is none.
    /// [`eval`] asks here first, also for the name of a built-in function:
    /// `None` then calls the built-in. HSPICE uses a `.param` function with
    /// the name of a built-in where one is visible (golden
    /// `func_shadow_top`, `func_shadow_sub`, `func_shadow_scope`).
    fn user_fn(
        &mut self,
        name: &str,
        args: &[f64],
    ) -> Option<Result<(f64, Vec<Caveat>), EvalError>>;
}

/// Evaluates `e`. Returns the value and its caveats (no duplicates).
///
/// [`Env::user_fn`] is asked before the built-in functions. `? :`, `&&` and
/// `||` evaluate only the side they need. A result that is not a finite
/// number is an [`EvalError::Domain`] error, never a value. This includes a
/// value that the [`Env`] returns for a name or a user function.
pub fn eval(e: &Expr, env: &mut dyn Env) -> Result<(f64, Vec<Caveat>), EvalError> {
    let mut caveats = Vec::new();
    let v = eval_into(e, env, &mut caveats)?;
    Ok((v, caveats))
}

/// Adds `c` to `caveats` unless it is already there.
fn add_caveat(caveats: &mut Vec<Caveat>, c: Caveat) {
    if !caveats.contains(&c) {
        caveats.push(c);
    }
}

fn eval_into(e: &Expr, env: &mut dyn Env, caveats: &mut Vec<Caveat>) -> Result<f64, EvalError> {
    let truth = |b: bool| if b { 1.0 } else { 0.0 };
    match e {
        Expr::Num(v) => finite(*v, || "number is out of range".into()),
        Expr::Ident(name) => {
            let (v, cs) = env.var(name)?;
            cs.into_iter().for_each(|c| add_caveat(caveats, c));
            if name == "temper" {
                add_caveat(caveats, Caveat::Temper);
            }
            finite(v, || format!("'{name}' is not a finite number"))
        }
        Expr::Call { name, args } => {
            let vals = args
                .iter()
                .map(|a| eval_into(a, env, caveats))
                .collect::<Result<Vec<_>, _>>()?;
            let (v, cs) = match env.user_fn(name, &vals) {
                Some(result) => result?,
                None => match builtins::call(name, &vals) {
                    Some(result) => {
                        let (v, c) = result?;
                        (v, c.into_iter().collect())
                    }
                    None => return Err(EvalError::BadCall(format!("unknown function '{name}'"))),
                },
            };
            cs.into_iter().for_each(|c| add_caveat(caveats, c));
            finite(v, || format!("{name}() is not a finite number"))
        }
        Expr::Unary { op, e } => {
            let v = eval_into(e, env, caveats)?;
            Ok(match op {
                UnOp::Neg => -v,
                UnOp::Plus => v,
                UnOp::Not => truth(v == 0.0),
            })
        }
        Expr::Binary {
            op: BinOp::And,
            l,
            r,
        } => {
            let v = eval_into(l, env, caveats)? != 0.0 && eval_into(r, env, caveats)? != 0.0;
            Ok(truth(v))
        }
        Expr::Binary {
            op: BinOp::Or,
            l,
            r,
        } => {
            let v = eval_into(l, env, caveats)? != 0.0 || eval_into(r, env, caveats)? != 0.0;
            Ok(truth(v))
        }
        Expr::Binary { op, l, r } => {
            let a = eval_into(l, env, caveats)?;
            let b = eval_into(r, env, caveats)?;
            let v = match op {
                BinOp::Pow => power(a, b),
                BinOp::Mul => a * b,
                BinOp::Div if b == 0.0 => return Err(EvalError::Domain("division by zero".into())),
                BinOp::Div => a / b,
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Lt => truth(a < b && !near(a, b)),
                BinOp::Le => truth(a < b || near(a, b)),
                BinOp::Gt => truth(a > b && !near(a, b)),
                BinOp::Ge => truth(a > b || near(a, b)),
                BinOp::Eq => truth(near(a, b)),
                BinOp::Ne => truth(!near(a, b)),
                BinOp::And | BinOp::Or => unreachable!("handled above"),
            };
            finite(v, || format!("{a} {op} {b} is not a finite number"))
        }
        Expr::Ternary { c, t, f } => {
            if eval_into(c, env, caveats)? != 0.0 {
                eval_into(t, env, caveats)
            } else {
                eval_into(f, env, caveats)
            }
        }
    }
}

/// Whether two values count as equal in a comparison. As measured, HSPICE
/// treats two values within a relative 1e-13 as equal, for all six
/// comparisons, with no absolute margin: `1e-300==0` is false (decks `cmp_*`,
/// `eq_edge`). The model selector uses it too.
#[inline]
pub fn near(x: f64, y: f64) -> bool {
    (x - y).abs() <= 1e-13 * x.abs().max(y.abs())
}

/// `x**y` as the HSPICE built-in function table defines it: for x < 0 the
/// integer part of y is used, and 0**y is 0 for every y, also `0**0` and
/// `0**-1` (golden case `ops2`).
fn power(x: f64, y: f64) -> f64 {
    if x < 0.0 {
        x.powf(y.trunc())
    } else if x == 0.0 {
        0.0
    } else {
        x.powf(y)
    }
}

/// `Ok(v)` if `v` is finite, else a [`EvalError::Domain`] error.
fn finite(v: f64, msg: impl FnOnce() -> String) -> Result<f64, EvalError> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(EvalError::Domain(msg()))
    }
}

#[cfg(test)]
mod tests {
    use super::{Caveat, EvalError};
    use crate::test_env::{MapEnv, eval_str, val};

    #[test]
    fn user_fn_call() {
        let e = crate::parse("f(2) + 1").unwrap();
        let mut env = MapEnv::new(&[]);
        assert_eq!(crate::eval(&e, &mut env).unwrap(), (21.0, vec![]));
        assert_eq!(env.calls, ["fn:f[2.0]"]);
        // Arguments are evaluated before the call.
        let mut env = MapEnv::new(&[("x", 3.0)]);
        let e = crate::parse("F(x*2)").unwrap();
        assert_eq!(crate::eval(&e, &mut env).unwrap().0, 60.0);
        assert_eq!(env.calls, ["var:x", "fn:f[6.0]"]);
    }

    #[test]
    fn user_fn_hides_builtin() {
        // the environment is asked first; `None` calls the built-in
        let e = crate::parse("max(1, 2)").unwrap();
        let mut env = MapEnv::new(&[]);
        assert_eq!(crate::eval(&e, &mut env).unwrap().0, 2.0);
        assert_eq!(env.calls, ["fn:max[1.0, 2.0]"]);
        // a user `max(a,b) = a+b` hides the built-in (golden
        // `func_shadow_top`)
        struct Sum;
        impl crate::Env for Sum {
            fn var(&mut self, name: &str) -> Result<(f64, Vec<Caveat>), EvalError> {
                Err(EvalError::Undefined(name.to_string()))
            }
            fn user_fn(
                &mut self,
                name: &str,
                args: &[f64],
            ) -> Option<Result<(f64, Vec<Caveat>), EvalError>> {
                (name == "max").then(|| Ok((args.iter().sum(), Vec::new())))
            }
        }
        assert_eq!(crate::eval(&e, &mut Sum).unwrap().0, 3.0);
        assert_eq!(
            crate::eval(&crate::parse("min(1, 2)").unwrap(), &mut Sum)
                .unwrap()
                .0,
            1.0
        );
    }

    #[test]
    fn unknown_function_is_bad_call() {
        assert!(matches!(eval_str("g(1)", &[]), Err(EvalError::BadCall(_))));
    }

    #[test]
    fn temper_reads_env_and_adds_caveat() {
        let e = crate::parse("temper + 1").unwrap();
        let mut env = MapEnv::new(&[("temper", 25.0)]);
        assert_eq!(
            crate::eval(&e, &mut env).unwrap(),
            (26.0, vec![Caveat::Temper])
        );
        assert_eq!(env.calls, ["var:temper"]);
        // One caveat even if `temper` appears twice.
        let (_, caveats) = eval_str("TEMPER*temper", &[("temper", 25.0)]).unwrap();
        assert_eq!(caveats, [Caveat::Temper]);
    }

    #[test]
    fn caveats_merge_without_duplicates() {
        let (v, caveats) =
            eval_str("agauss(1,0.1,3) + gauss(2,0.1,3) + agauss(3,1,3)", &[]).unwrap();
        assert_eq!(v, 6.0);
        assert_eq!(
            caveats,
            [
                Caveat::StatisticalNominal("agauss".into()),
                Caveat::StatisticalNominal("gauss".into())
            ]
        );
    }

    #[test]
    fn undefined_name_is_reported() {
        assert_eq!(
            eval_str("1 + nope", &[]),
            Err(EvalError::Undefined("nope".into()))
        );
    }

    #[test]
    fn domain_errors_never_give_a_number() {
        for src in ["1/0", "0/0", "1e300*1e300", "exp(1000)", "log(0)", "1e999"] {
            assert!(
                matches!(eval_str(src, &[]), Err(EvalError::Domain(_))),
                "{src}"
            );
        }
    }

    #[test]
    fn values_from_env_must_be_finite() {
        for bad in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            for src in ["x", "x > 1", "x == x", "-x", "1 ? x : 0"] {
                assert!(
                    matches!(eval_str(src, &[("x", bad)]), Err(EvalError::Domain(_))),
                    "{src} with x = {bad}"
                );
            }
        }
        // A user function result is checked too: f(x) = 10*x.
        assert!(matches!(
            eval_str("f(1e308)", &[]),
            Err(EvalError::Domain(_))
        ));
    }

    #[test]
    fn zero_base_power_is_zero() {
        // HSPICE gives 0 for every exponent of a zero base, with no error
        // (golden case `ops2`).
        for src in ["0**0", "0^0", "0**-1", "0**0.5", "0**2", "(-0)**-1"] {
            assert_eq!(val(src), 0.0, "{src}");
        }
    }

    #[test]
    fn ternary_and_logic_evaluate_only_the_taken_side() {
        assert_eq!(val("1 ? 2 : 1/0"), 2.0);
        assert_eq!(val("0 ? 1/0 : 3"), 3.0);
        assert_eq!(val("0 && 1/0"), 0.0);
        assert_eq!(val("1 || 1/0"), 1.0);
        assert_eq!(val("2 && 3"), 1.0);
        assert_eq!(val("0 || -2"), 1.0);
    }
}
