//! HSPICE built-in functions.
//!
//! Semantics follow the HSPICE built-in function table: `sqrt`, `log`,
//! `log10` and `db` use |x| and keep the sign of x; `pow(x,y)` uses the
//! integer part of y; `pwr(x,y)` is |x|**y with the sign of x; `nint` rounds
//! to the nearest integer (`nint(2.5)` = 3); `db(x)` is 20*log10(x); the
//! trigonometric and hyperbolic functions use radians. The statistical
//! functions return their nominal (first) argument, as `.op` without Monte
//! Carlo does. Golden cases `funcs`, `funcs_neg` and `funcs_extra` check
//! these against HSPICE.

use crate::eval::{Caveat, EvalError};

/// The names of the built-in functions, lowercase.
pub(crate) const NAMES: [&str; 29] = [
    "sqrt", "log", "log10", "exp", "abs", "db", "int", "nint", "floor", "ceil", "sin", "cos",
    "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "sgn", "pow", "pwr", "sign", "min",
    "max", "agauss", "gauss", "aunif", "unif",
];

/// `name` (any letter case) is a built-in function. A `.param` function
/// with such a name can hide it (see [`crate::Env::user_fn`]).
pub fn is_builtin(name: &str) -> bool {
    NAMES.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// Calls built-in `name` (lowercase) with evaluated `args`.
/// Returns `None` when `name` is not a built-in. The caller rejects a
/// result that is not finite.
pub(crate) fn call(name: &str, args: &[f64]) -> Option<Result<(f64, Option<Caveat>), EvalError>> {
    let signed = |x: f64, f: fn(f64) -> f64| x.signum() * f(x.abs());
    let v = match name {
        "sqrt" => one(name, args, |x| signed(x, f64::sqrt)),
        "log" => one(name, args, |x| signed(x, f64::ln)),
        "log10" => one(name, args, |x| signed(x, f64::log10)),
        "exp" => one(name, args, f64::exp),
        "abs" => one(name, args, f64::abs),
        "db" => one(name, args, |x| signed(x, |a| 20.0 * a.log10())),
        "int" => one(name, args, f64::trunc),
        "nint" => one(name, args, f64::round),
        "floor" => one(name, args, f64::floor),
        "ceil" => one(name, args, f64::ceil),
        "sin" => one(name, args, f64::sin),
        "cos" => one(name, args, f64::cos),
        "tan" => one(name, args, f64::tan),
        "asin" => one(name, args, f64::asin),
        "acos" => one(name, args, f64::acos),
        "atan" => one(name, args, f64::atan),
        "sinh" => one(name, args, f64::sinh),
        "cosh" => one(name, args, f64::cosh),
        "tanh" => one(name, args, f64::tanh),
        "sgn" => one(name, args, |x| if x == 0.0 { 0.0 } else { x.signum() }),
        "pow" => two(name, args, |x, y| x.powf(y.trunc())),
        "pwr" => two(name, args, |x, y| x.signum() * x.abs().powf(y)),
        "sign" => two(name, args, |x, y| if y < 0.0 { -x.abs() } else { x.abs() }),
        "min" => two(name, args, f64::min),
        "max" => two(name, args, f64::max),
        // (nominal, abs or rel variation, sigma [, multiplier])
        "agauss" | "gauss" => return Some(statistical(name, args, 3, 4)),
        // (nominal, abs or rel variation [, multiplier])
        "aunif" | "unif" => return Some(statistical(name, args, 2, 3)),
        _ => return None,
    };
    Some(v.map(|v| (v, None)))
}

fn arity(name: &str, args: &[f64], min: usize, max: usize) -> Result<(), EvalError> {
    if (min..=max).contains(&args.len()) {
        return Ok(());
    }
    let want = if min == max {
        min.to_string()
    } else {
        format!("{min} to {max}")
    };
    Err(EvalError::BadCall(format!(
        "{name}() takes {want} arguments, got {}",
        args.len()
    )))
}

fn one(name: &str, args: &[f64], f: impl Fn(f64) -> f64) -> Result<f64, EvalError> {
    arity(name, args, 1, 1).map(|()| f(args[0]))
}

fn two(name: &str, args: &[f64], f: impl Fn(f64, f64) -> f64) -> Result<f64, EvalError> {
    arity(name, args, 2, 2).map(|()| f(args[0], args[1]))
}

fn statistical(
    name: &str,
    args: &[f64],
    min: usize,
    max: usize,
) -> Result<(f64, Option<Caveat>), EvalError> {
    arity(name, args, min, max)?;
    Ok((args[0], Some(Caveat::StatisticalNominal(name.to_string()))))
}

#[cfg(test)]
mod tests {
    use crate::Caveat;
    use crate::EvalError;
    use crate::test_env::{close, eval_str, val};

    #[test]
    fn builtins() {
        assert_eq!(val("sqrt(4)"), 2.0);
        assert_eq!(val("pow(2,3)"), 8.0);
        assert_eq!(val("pwr(-2,2)"), -4.0);
        let (v, _) = eval_str("log(e)", &[("e", std::f64::consts::E)]).unwrap();
        assert!(close(v, 1.0), "{v}");
        assert!(close(val("log10(100)"), 2.0));
        assert_eq!(val("exp(0)"), 1.0);
        assert_eq!(val("abs(-1)"), 1.0);
        assert_eq!(val("min(1,2)"), 1.0);
        assert_eq!(val("max(1,2)"), 2.0);
        assert_eq!(val("int(2.7)"), 2.0);
        assert_eq!(val("sgn(-3)"), -1.0);
        assert_eq!(val("sign(3,-1)"), -3.0);
        for f in ["agauss", "gauss"] {
            let got = eval_str(&format!("{f}(1k,0.1k,3)"), &[]).unwrap();
            assert_eq!(got, (1000.0, vec![Caveat::StatisticalNominal(f.into())]));
        }
        for f in ["aunif", "unif"] {
            let got = eval_str(&format!("{f}(1k,0.1k)"), &[]).unwrap();
            assert_eq!(got, (1000.0, vec![Caveat::StatisticalNominal(f.into())]));
        }
    }

    #[test]
    fn builtin_hspice_sign_rules() {
        // HSPICE: sqrt, log and log10 work on |x| and keep the sign of x.
        assert_eq!(val("sqrt(-4)"), -2.0);
        assert!(close(val("log10(-100)"), -2.0));
        assert_eq!(val("log(-1)"), 0.0);
        // pow uses the integer part of the exponent.
        assert_eq!(val("pow(2,3.9)"), 8.0);
        assert_eq!(val("pow(-2,3)"), -8.0);
        // pwr: |x|**y with the sign of x.
        assert!(close(val("pwr(-4,0.5)"), -2.0));
        assert_eq!(val("pwr(4,0.5)"), 2.0);
        // `**` with a negative base uses the integer part of the exponent.
        assert_eq!(val("(-2)**2.5"), 4.0);
        assert_eq!(val("0**2"), 0.0);
        assert!(close(val("4**0.5"), 2.0));
        assert_eq!(val("int(-2.7)"), -2.0);
        assert_eq!(val("sgn(0)"), 0.0);
        assert_eq!(val("sgn(5)"), 1.0);
        assert_eq!(val("sign(-3,2)"), 3.0);
        assert_eq!(val("ABS(-1)"), 1.0);
        let (v, caveats) = eval_str("agauss(1k,0.1k,3,2)", &[]).unwrap();
        assert_eq!((v, caveats.len()), (1000.0, 1));
    }

    #[test]
    #[allow(
        clippy::approx_constant,
        reason = "values as the HSPICE listing prints them"
    )]
    fn golden_funcs_values() {
        // Deck text and resistor values from HSPICE P-2019.06: golden cases
        // `funcs`, `funcs_neg` and `funcs_extra`. The listing prints 4
        // decimals, so a value matches when it is within half a unit of the
        // last printed digit.
        let cases: &[(&str, f64)] = &[
            // funcs (`f(2,3)` needs a two-argument user function: not here)
            ("sqrt(4)", 2.0),
            ("pow(2,3)", 8.0),
            ("10+pow(-2,3)", 2.0),
            ("pow(2,0.5)", 1.0),
            ("pow(2,2.7)", 4.0),
            ("10+pwr(-2,2)", 6.0),
            ("10+pwr(-2,0.5)", 8.5858),
            ("log(100)", 4.6052),
            ("log10(100)", 2.0),
            ("exp(0)", 1.0),
            ("exp(1)", 2.7183),
            ("abs(-1)", 1.0),
            ("min(1,2)", 1.0),
            ("max(1,2)", 2.0),
            ("int(2.7)", 2.0),
            ("10+int(-2.7)", 8.0),
            ("10+sgn(-3)", 9.0),
            ("10+sign(3,-1)", 7.0),
            // funcs_neg
            ("10+sqrt(-4)", 8.0),
            ("10+log(-100)", 5.3948),
            ("10+log10(-100)", 8.0),
            // funcs_extra
            ("nint(2.5)", 3.0),
            ("floor(2.7)", 2.0),
            ("ceil(2.2)", 3.0),
            ("10+sin(1)", 10.8415),
            ("10+cos(1)", 10.5403),
            ("10+tan(1)", 11.5574),
            ("10+asin(0.5)", 10.5236),
            ("10+acos(0.5)", 11.0472),
            ("10+atan(1)", 10.7854),
            ("10+sinh(1)", 11.1752),
            ("10+cosh(1)", 11.5431),
            ("10+tanh(1)", 10.7616),
            ("10+db(10)", 30.0),
        ];
        for &(src, want) in cases {
            let got = val(src);
            assert!((got - want).abs() <= 5e-5, "{src}: got {got}, want {want}");
        }
    }

    #[test]
    fn builtin_extra_rules() {
        // Exact integer results (golden `funcs_extra`).
        assert_eq!(val("nint(2.5)"), 3.0);
        assert_eq!(val("floor(2.7)"), 2.0);
        assert_eq!(val("ceil(2.2)"), 3.0);
        assert_eq!(val("db(10)"), 20.0);
        // Not in the golden data. nint rounds half away from zero; floor and
        // ceil are the usual ones.
        assert_eq!(val("nint(-2.5)"), -3.0);
        assert_eq!(val("nint(2.4)"), 2.0);
        assert_eq!(val("floor(-2.7)"), -3.0);
        assert_eq!(val("ceil(-2.2)"), -2.0);
        // db, like log10 (golden `funcs_neg`), works on |x| and keeps the
        // sign of x, as the HSPICE built-in function table states.
        assert_eq!(val("db(-10)"), -20.0);
        assert!(close(val("db(0.1)"), -20.0));
        assert!(close(val("sin(0)"), 0.0));
        assert!(close(val("COSH(0)"), 1.0));
        // No finite result: an error, not a number.
        for src in ["db(0)", "asin(2)", "acos(-1.5)"] {
            assert!(
                matches!(eval_str(src, &[]), Err(EvalError::Domain(_))),
                "{src}"
            );
        }
    }

    #[test]
    fn builtin_arity_is_checked() {
        for src in [
            "sqrt()",
            "sqrt(1,2)",
            "max(1)",
            "max(1,2,3)",
            "agauss(1,2)",
            "unif(1)",
            "sign(1)",
            "nint()",
            "floor(1,2)",
            "db()",
            "tanh(1,2)",
        ] {
            assert!(
                matches!(eval_str(src, &[]), Err(EvalError::BadCall(_))),
                "{src}"
            );
        }
    }

    #[test]
    fn names_are_the_builtins() {
        for name in super::NAMES {
            let args = [1.0, 1.0, 1.0];
            let arity = (1..=3).find(|&n| super::call(name, &args[..n]).is_some_and(|r| r.is_ok()));
            assert!(arity.is_some(), "{name} is not callable");
            assert!(super::is_builtin(&name.to_ascii_uppercase()), "{name}");
        }
        for name in ["f", "maxx", "temper", "v", "i"] {
            assert!(!super::is_builtin(name), "{name}");
            assert!(super::call(name, &[1.0]).is_none(), "{name}");
        }
        // the other direction: every name that an arm of `call` matches is
        // in `NAMES` (read from the source of `call`), so a new arm without
        // a `NAMES` entry fails here
        let src = include_str!("builtins.rs");
        let start = src.find("pub(crate) fn call(").unwrap();
        let end = start + src[start..].find("_ => return None,").unwrap();
        let mut arms: Vec<&str> = src[start..end]
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with('"'))
            .filter_map(|l| l.split("=>").next())
            .flat_map(|pat| pat.split('|'))
            .map(|n| n.trim().trim_matches('"'))
            .collect();
        arms.sort_unstable();
        let mut names = super::NAMES.to_vec();
        names.sort_unstable();
        assert_eq!(arms, names);
    }

    #[test]
    fn statistical_arguments_must_resolve() {
        assert_eq!(
            eval_str("agauss(1, sig, 3)", &[]),
            Err(EvalError::Undefined("sig".into()))
        );
    }
}
