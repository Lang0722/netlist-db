use super::*;

fn top(params: &[(&str, f64)]) -> Scope<'static> {
    let mut s = Scope::new(ParHier::Global);
    for (k, v) in params {
        s.set(k, Ok(*v));
    }
    s
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * b.abs().max(1e-30)
}

fn kv(k: &'static str, text: &'static str) -> KeyValue<'static> {
    KeyValue {
        k: Cow::Borrowed(k),
        v: Value::Expr(Cow::Borrowed(text)),
    }
}

/// A long expression must not end the process: its length is not a depth.
#[test]
fn long_chains_of_operators() {
    let s = top(&[]);
    let n = 200_000;
    let sum = format!("1{}", "+1".repeat(n));
    assert_eq!(s.eval_str(&sum), Ok((n + 1) as f64));
    let product = format!("1{}", "*1".repeat(n));
    assert_eq!(s.eval_str(&product), Ok(1.0));
    let power = format!("2{}", "**1".repeat(n));
    assert_eq!(s.eval_str(&power), Ok(2.0));
    let and = format!("1{}", "&&1".repeat(n));
    assert_eq!(s.eval_str(&and), Ok(1.0));
    // (left to right)
    assert_eq!(s.eval_str("10-3-2"), Ok(5.0));
    assert_eq!(s.eval_str("8/2/2"), Ok(2.0));
    // (the right side of `&&` and `||` is not read when the left decides)
    assert_eq!(s.eval_str("0&&nope&&nope"), Ok(0.0));
    assert_eq!(s.eval_str("1||nope||nope"), Ok(1.0));
    assert!(s.eval_str("1&&0||nope").is_err());
}

#[test]
fn numbers_and_operators() {
    let s = top(&[("w", 2e-6), ("l", 1e-7)]);
    let e = |x: &str| s.eval_str(x).unwrap();
    assert!(close(e("2.5u"), 2.5e-6));
    assert!(close(e("1meg"), 1e6));
    assert!(close(e("1+2*3"), 7.0));
    assert!(close(e("(1+2)*3"), 9.0));
    assert!(close(e("2^10"), 1024.0));
    // (as measured on HSPICE)
    assert!(close(e("2**3**2"), 64.0));
    assert!(close(e("2^3^2"), 64.0));
    assert!(close(e("2**-3**2"), 0.015625));
    assert!(close(e("-2**2"), -4.0));
    assert!(close(e("-2**2+10"), 6.0));
    assert!(close(e("(-2)**3"), -8.0));
    assert!(close(e("(-2)**1.5"), -2.0));
    assert!(close(e("(-2)**2.5"), 4.0));
    assert!(close(e("(-2)**0.5"), 1.0));
    assert!(close(e("1+0**0"), 1.0));
    assert!(close(e("1+0**-1"), 1.0));
    assert!(close(e("1+0**0.5"), 1.0));
    assert!(close(e("w/l"), 20.0));
    assert!(close(e("2*W+100n"), 4.1e-6));
    assert!(close(e("1 - 2 - 3"), -4.0));
    assert!(close(e("8/2/2"), 2.0));
    assert!(close(e("w > l ? 1 : 0"), 1.0));
    assert!(close(e("1>0?0>1?4:8:9"), 8.0));
    assert!(close(e("!(w<l) && (1 || 0)"), 1.0));
    assert!(close(e("1 == 1"), 1.0));
    assert!(close(e("1 != 1"), 0.0));
    // a continuation line inside a quoted expression
    assert!(close(e("1+\n+ 2"), 3.0));
}

/// Comparisons in an expression, as measured (decks `cmp_ternary`): two
/// values within a relative 1e-13 are equal; `<` and `>` need more
#[test]
fn comparisons_have_a_tolerance() {
    for base in [1.0, 1e-6, 1e3, 1e-12, -1.0, 1e-30] {
        let sign: f64 = if base < 0.0 { -1.0 } else { 1.0 };
        let lo = |delta: f64| {
            // (a and b, with a<b)
            let (a, b): (f64, f64) = (base, base * (1.0 + delta));
            if sign > 0.0 { (a, b) } else { (b, a) }
        };
        let s = |delta: f64| {
            let (a, b) = lo(delta);
            top(&[("a", a), ("b", b)])
        };
        for (delta, equal) in [
            (0.0, true),
            (1e-15, true),
            (1e-13, true),
            (2e-13, false),
            (1e-9, false),
        ] {
            let s = s(delta);
            let e = |x: &str| s.eval_str(x).unwrap() != 0.0;
            assert_eq!(e("a==b"), equal, "{base} {delta}");
            assert_eq!(e("b==a"), equal, "{base} {delta}");
            assert_eq!(e("a!=b"), !equal, "{base} {delta}");
            assert_eq!(e("a<b"), !equal, "{base} {delta}");
            assert_eq!(e("b>a"), !equal, "{base} {delta}");
            assert!(e("a<=b"), "{base} {delta}");
            assert!(e("b>=a"), "{base} {delta}");
            assert_eq!(e("b<=a"), equal, "{base} {delta}");
            assert_eq!(e("a>=b"), equal, "{base} {delta}");
            assert!(!e("b<a"), "{base} {delta}");
            assert!(!e("a>b"), "{base} {delta}");
        }
    }
    let s = top(&[]);
    let e = |x: &str| s.eval_str(x).unwrap() != 0.0;
    assert!(e("0.1*3==0.3"));
    assert!(e("1/3*3==1"));
    assert!(!e("1e-300==0"));
    assert!(e("0==0"));
}

/// In the condition of an `.if` line, `<` and `>` compare exactly; the other
/// comparisons are those of an expression (decks `cmp_if`)
#[test]
fn condition_order_is_exact() {
    let s = top(&[("a", 1.0), ("b", 1.0 + 1e-15), ("c", 1.0 + 1e-13)]);
    let c = |x: &str| s.condition(x).unwrap();
    assert!(c("a<b"));
    assert!(c("b>a"));
    assert!(!c("b<a"));
    assert!(!c("a<a"));
    assert!(c("a==b"));
    assert!(c("a==c"));
    assert!(c("c<=a"));
    assert!(c("a>=c"));
    assert!(!c("a!=c"));
    // (in a call too)
    assert!(c("max(a<b,0)"));
    // the same text in an expression keeps the tolerance
    assert_eq!(s.eval_str("a<b"), Ok(0.0));
}

/// A condition reads a parameter by its rule: the parameter's text compares
/// exactly there, and with the tolerance on an element line (decks
/// `xo_param_body`, `v_param_lt`)
#[test]
fn condition_rule_reaches_the_parameters_it_reads() {
    let mut s = top(&[("a", 1.0), ("b", 1.0 + 1e-15), ("t", 1e-30)]);
    s.define(&kv("c", "a<b"));
    s.define(&kv("d", "t&&1"));
    assert_eq!(s.get("c"), Ok(0.0));
    assert_eq!(s.condition("c"), Ok(true));
    assert_eq!(s.get("c"), Ok(0.0));
    assert_eq!(s.get("d"), Ok(0.0));
    assert_eq!(s.condition("d"), Ok(true));
    assert_eq!(s.condition("!t"), Ok(false));
    assert_eq!(s.condition("t?1:0"), Ok(true));
    assert_eq!(s.eval_str("t?1:0"), Ok(0.0));
}

/// As measured (decks `tr_ternary`)
#[test]
fn truth_of_a_value_starts_at_1e_28() {
    let e = |v: f64| {
        let s = top(&[("t", v)]);
        (
            s.eval_str("t?1:0").unwrap(),
            s.eval_str("t&&1").unwrap(),
            s.eval_str("t||0").unwrap(),
            s.eval_str("!t").unwrap(),
        )
    };
    assert_eq!(e(5e-29), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(e(-5e-29), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(e(1e-28), (1.0, 1.0, 1.0, 0.0));
    assert_eq!(e(-1e-28), (1.0, 1.0, 1.0, 0.0));
    assert_eq!(e(0.0), (0.0, 0.0, 0.0, 1.0));
}

/// As measured (decks `d_*`, `f_exp_big`)
#[test]
fn division_by_zero_and_large_exponentials() {
    let s = top(&[]);
    let e = |x: &str| s.eval_str(x).unwrap();
    assert!(close(e("1/0"), 1e28));
    assert!(close(e("2/0"), 2e28));
    assert!(close(e("-1/0"), -1e28));
    assert_eq!(e("0/0"), 0.0);
    assert!(close(e("1/1e-30"), 1e30));
    assert!(close(e("exp(1000)"), 80f64.exp()));
    assert!(close(e("exp(80)"), 80f64.exp()));
    assert!(close(e("exp(1)"), 1f64.exp()));
}

/// Two signs read as a binary sign and the sign of the value. Three stop.
#[test]
fn two_signs_between_values() {
    let s = top(&[]);
    let e = |x: &str| s.eval_str(x);
    assert_eq!(e("1.5--10").unwrap(), 11.5);
    assert_eq!(e("1.5++10").unwrap(), 11.5);
    assert_eq!(e("1 -- 2").unwrap(), 3.0);
    assert_eq!(e("(1)--(2)").unwrap(), 3.0);
    assert_eq!(e("max(1,2)--1").unwrap(), 3.0);
    assert!(e("1.5---10").is_err());
    assert!(e("1.5----10").is_err());
}

/// As measured: rounds of random expressions through `.meas`
#[test]
fn functions_at_their_limits() {
    let s = top(&[]);
    let e = |x: &str| s.eval_str(x).unwrap();
    // the exponentials stop at 80 at both ends
    assert!(close(e("exp(-100)"), (-80f64).exp()));
    assert!(close(e("exp(-300)*1e130"), (-80f64).exp() * 1e130));
    assert!(close(e("sinh(81)"), 80f64.sinh()));
    assert!(close(e("sinh(-100)"), -(80f64.sinh())));
    assert!(close(e("sinh(1000)"), 80f64.sinh()));
    assert!(close(e("cosh(-100)"), 80f64.cosh()));
    assert!(close(e("cosh(1000)"), 80f64.cosh()));
    // (HSPICE computes these from `exp`, which shows at a small size)
    assert!(close(e("sinh(2e-12)"), 1.999955756559757e-12));
    // `db`: as `log10`, with the negative of the result below 0
    assert!(close(e("db(100)"), 40.0));
    assert!(close(e("db(-100)"), -40.0));
    assert!(close(e("db(0)"), -560.0));
    assert!(close(e("db(-0)"), -560.0));
    assert!(close(e("db(1e-30)"), -560.0));
    assert!(close(e("db(-1e-30)"), 560.0));
    assert!(close(e("db(-1e-27)"), 540.0));
    // zero with a sign is zero
    assert!(close(e("log(-0)"), 1e-28f64.ln()));
    assert!(close(e("log10(-0)"), -28.0));
    // out of range: 0
    for x in [
        "asin(2)",
        "asin(-2)",
        "asin(1.0000001)",
        "acos(2)",
        "acos(-2)",
    ] {
        assert_eq!(e(x), 0.0, "{x}");
    }
    assert!(close(e("acos(2)+1"), 1.0));
    assert!(close(e("asin(1)"), 1f64.asin()));
    assert!(close(e("acos(-1)"), (-1f64).acos()));
    // zero to any power: 0
    for x in [
        "pow(0,0)",
        "pow(0,-1)",
        "pow(0,.5)",
        "pow(-0,0)",
        "pwr(0,0)",
        "pwr(0,-1)",
    ] {
        assert_eq!(e(x), 0.0, "{x}");
    }
    assert!(close(e("pow(2,1.5)"), 2.0));
    assert!(close(e("pwr(-2,2)"), -4.0));
}

/// As measured: rounds of function names through `.meas`
#[test]
fn rounding_remainder_and_inverse_hyperbolic_functions() {
    let s = top(&[]);
    let e = |x: &str| s.eval_str(x).unwrap();
    assert_eq!(e("round(2.5)"), 3.0);
    assert_eq!(e("round(-2.5)"), -3.0);
    assert_eq!(e("round(0.5)"), 1.0);
    assert_eq!(e("round(2.4)"), 2.0);
    assert_eq!(e("trunc(2.9)"), 2.0);
    assert_eq!(e("trunc(-2.9)"), -2.0);
    assert_eq!(e("fmod(5,3)"), 2.0);
    assert_eq!(e("fmod(-5,3)"), -2.0);
    assert_eq!(e("fmod(5,-3)"), 2.0);
    assert_eq!(e("fmod(5.5,3)"), 2.5);
    assert_eq!(e("fmod(5,1e30)"), 5.0);
    assert!(close(e("asinh(2)"), 2f64.asinh()));
    assert!(close(e("asinh(-2)"), -(2f64.asinh())));
    assert!(close(e("acosh(2)"), 2f64.acosh()));
    assert_eq!(e("acosh(1)"), 0.0);
    assert!(close(e("atanh(.5)"), 0.5f64.atanh()));
    assert!(s.eval_str("fmod(5,0)").is_err());
    assert!(s.eval_str("acosh(.5)").is_err());
    assert!(s.eval_str("atanh(2)").is_err());
}

#[test]
fn numbers_as_measured_on_hspice() {
    let s = top(&[]);
    for (text, value) in [
        ("1me", 1e6),
        ("1meter", 1e6),
        ("1meg", 1e6),
        ("1megs", 1e6),
        ("1x", 1e6),
        ("1m", 1e-3),
        ("1M", 1e-3),
        ("1mohm", 1e-3),
        ("1mi", 25.4e-6),
        ("1mil", 25.4e-6),
        ("1mile", 25.4e-6),
        ("1min", 60.0),
        ("1minute", 60.0),
        ("1hr", 3600.0),
        ("1hrs", 3600.0),
        ("1day", 86400.0),
        ("1days", 86400.0),
        ("1yr", 31536000.0),
        ("1y", 1e-24),
        ("1years", 1e-24),
        ("1z", 1e-21),
        ("1zz", 1e-21),
        ("1a", 1e-18),
        ("20db", 10.0),
        ("20dbm", 10.0),
        ("10pF", 1e-11),
        ("1.8V", 1.8),
        // (an exponent makes the scale factor void)
        ("1e-3k", 1e-3),
        ("2E3meg", 2e3),
    ] {
        assert!(close(s.eval_str(text).unwrap(), value), "{text}");
    }
}

#[test]
fn functions() {
    let s = top(&[("x", 4.0)]);
    let e = |x: &str| s.eval_str(x).unwrap();
    assert!(close(e("sqrt(x)"), 2.0));
    // (as measured on HSPICE)
    assert!(close(e("sqrt(-4)"), -2.0));
    assert!(close(e("log(-100)"), -100f64.abs().ln()));
    assert!(close(e("log10(-100)"), -2.0));
    assert!(close(e("max(1, x, 3)"), 4.0));
    assert!(close(e("min(1, -x)"), -4.0));
    assert!(close(e("log(exp(2))"), 2.0));
    assert!(close(e("log10(1k)"), 3.0));
    assert!(close(e("pow(2, 3.7)"), 8.0));
    assert!(close(e("pwr(-2, 2)"), -4.0));
    assert!(close(e("abs(-x)"), 4.0));
    assert!(close(e("int(2.7) + nint(2.5)"), 5.0));
    assert!(close(e("sin(pi/2)"), 1.0));
    // statistical functions: the nominal value
    assert!(close(e("agauss(1u, 0.1u, 3)"), 1e-6));
    assert!(close(e("2*aunif(x, 1)"), 8.0));
    assert_eq!(
        s.eval_str("v(out)*2"),
        Err(EvalError::NotStatic("v(...)".to_owned()))
    );
    assert_eq!(
        s.eval_str("foo(1)"),
        Err(EvalError::UnknownFunction("foo".to_owned()))
    );
    assert!(matches!(
        s.eval_str("sqrt(1, 2)"),
        Err(EvalError::Arguments { .. })
    ));
}

/// Deep nesting is an error, not a stack overflow, and a long chain of
/// parameters has a value
#[test]
fn depth_limits() {
    let mut s = top(&[]);
    assert!(matches!(
        s.eval_str(&format!("{}1", "-".repeat(100_000))),
        Err(EvalError::Syntax { .. })
    ));
    assert!(matches!(
        s.eval_str(&format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000))),
        Err(EvalError::Syntax { .. })
    ));
    assert_eq!(s.eval_str("--1"), Ok(1.0));
    for i in 0..10_000 {
        let kv = crate::ast::KeyValue {
            k: format!("p{i}").into(),
            v: Value::Expr(format!("p{}+1", i + 1).into()),
        };
        s.define(&kv);
    }
    s.set("p10000", Ok(0.0));
    assert_eq!(s.get("p0"), Ok(10000.0));
    assert_eq!(s.get("p9950"), Ok(50.0));
}

#[test]
fn errors() {
    let mut s = top(&[]);
    assert_eq!(
        s.eval_str("2*wn"),
        Err(EvalError::Undefined("wn".to_owned()))
    );
    assert!(matches!(s.eval_str("2*"), Err(EvalError::Syntax { .. })));
    assert!(matches!(s.eval_str("(1"), Err(EvalError::Syntax { .. })));
    assert!(matches!(s.eval_str("1 2"), Err(EvalError::Syntax { .. })));
    // on one line, for the listing of `model_select`
    let e = s.eval_str("1\n+ x y").unwrap_err().to_string();
    assert!(!e.contains('\n'), "{e}");
    assert_eq!(
        s.eval_str("1e308*10"),
        Err(EvalError::NotFinite("1e308*10".to_owned()))
    );
    s.set("a", Err(EvalError::Undefined("q".to_owned())));
    assert_eq!(s.get("a"), Err(EvalError::Undefined("q".to_owned())));
}
