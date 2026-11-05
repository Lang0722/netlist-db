//! HSPICE number literals.

use std::io::Write;

/// What a scale factor or unit does to the number before it.
#[derive(Clone, Copy)]
enum Scale {
    /// Times 10^n.
    Pow10(i64),
    /// Times a factor that is not a power of ten (mil, time units).
    Times(f64),
    /// Decibels to a ratio: 10^(x/20).
    Db,
}

/// Scale factors and units, matched case-insensitively at the start of the
/// letters. The first match wins, so a longer name comes before its
/// one-letter prefix: `MIN` (minute) before `MI` (mil, also `MIL`), and
/// `ME` (mega, also `MEG`) before `M` (milli). Golden cases `units` and
/// `units2`.
const SCALES: [(&[u8], Scale); 19] = [
    (b"MIN", Scale::Times(60.0)),
    (b"MI", Scale::Times(25.4e-6)),
    (b"ME", Scale::Pow10(6)),
    (b"HR", Scale::Times(3600.0)),
    (b"DAY", Scale::Times(86_400.0)),
    (b"YR", Scale::Times(31_536_000.0)),
    (b"DB", Scale::Db),
    (b"T", Scale::Pow10(12)),
    (b"G", Scale::Pow10(9)),
    (b"X", Scale::Pow10(6)),
    (b"K", Scale::Pow10(3)),
    (b"M", Scale::Pow10(-3)),
    (b"U", Scale::Pow10(-6)),
    (b"N", Scale::Pow10(-9)),
    (b"P", Scale::Pow10(-12)),
    (b"F", Scale::Pow10(-15)),
    (b"A", Scale::Pow10(-18)),
    (b"Z", Scale::Pow10(-21)),
    (b"Y", Scale::Pow10(-24)),
];

/// Larger decimal exponents only give 0 or infinity.
const EXP_LIMIT: i64 = 100_000;

/// Reads one HSPICE number literal at the start of `s`.
///
/// Form: optional sign, digits with an optional `.`, optional exponent
/// (`e-3`), then any ASCII letters. Without an exponent, the letters can
/// start with a scale factor or unit (any case):
/// - `T G ME X K M U N P F A Z Y` are powers of ten, where `ME` (also `MEG`)
///   is mega, `M` is milli, `Z` is zepto (1e-21) and `Y` is yocto (1e-24);
/// - `MI` (also `MIL`) is mil (25.4e-6);
/// - `MIN`, `HR`, `DAY`, `YR` are time units in seconds (60, 3600, 86400,
///   and 365 days);
/// - `DB` is decibels, converted to the ratio 10^(x/20) (`20db` = 10).
///
/// HSPICE ignores the other letters (`10pF`, `1V`, `1meter` = 1e6). After an
/// exponent, HSPICE ignores all letters, a scale factor too (`1e-3k` =
/// 0.001, golden case `units`). A sign applies to the converted value, as a
/// unary minus in an expression does (`-20db` = -10). Returns the value and
/// the number of bytes consumed (including the ignored letters), or `None`
/// when `s` does not start with a number.
pub fn scan_number(s: &str) -> Option<(f64, usize)> {
    scan(s).map(|n| (n.value, n.end))
}

/// [`scan_number`], and also `true` when the literal is exact: one number
/// with at most one scale factor or unit name after it, and no other
/// letters (`2u`, `1e-6`, `0.5`, `1meg`, `1mil`, `20db`). A scale name after
/// an exponent counts as exact: HSPICE reads `1e-3k` as a number, 0.001,
/// with a warning (golden `units`). Other letters after the number
/// (`2umos`, `5vnmos`, `33nmos`, `10pF`), which [`scan_number`] ignores as
/// HSPICE does where it expects a number, make it not exact.
pub fn scan_number_exact(s: &str) -> Option<(f64, usize, bool)> {
    scan(s).map(|n| (n.value, n.end, n.exact))
}

/// The value of `s` when all of `s` is one exact HSPICE number (see
/// [`scan_number_exact`]): `2u`, `1e-6`, `0.5`, `1meg`, `1e-3k`. `None`
/// when other letters follow the number (`2umos`, `5vnmos`, `10pF`), and
/// when `s` is not one number.
pub fn exact_number(s: &str) -> Option<f64> {
    scan(s).and_then(|n| (n.exact && n.end == s.len()).then_some(n.value))
}

/// One number literal read by [`scan`].
struct Scanned {
    value: f64,
    /// The end of the literal, after its letters.
    end: usize,
    /// See [`scan_number_exact`].
    exact: bool,
}

/// Reads one HSPICE number literal at the start of `s` (see
/// [`scan_number`]).
fn scan(s: &str) -> Option<Scanned> {
    let b = s.as_bytes();
    let digits_from = |i: usize| i + b[i..].iter().take_while(|c| c.is_ascii_digit()).count();

    let negative = b.first() == Some(&b'-');
    let sign_len = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let mut i = sign_len;
    let int_end = digits_from(i);
    let mut n_digits = int_end - i;
    i = int_end;
    if b.get(i) == Some(&b'.') {
        let frac_end = digits_from(i + 1);
        n_digits += frac_end - (i + 1);
        i = frac_end;
    }
    if n_digits == 0 {
        return None;
    }
    let mantissa = &s[sign_len..i];

    let mut exp10: i64 = 0;
    let mut has_exponent = false;
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        let negative = b.get(j) == Some(&b'-');
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let exp_end = digits_from(j);
        if exp_end > j {
            let magnitude = b[j..exp_end].iter().fold(0_i64, |acc, d| {
                (acc * 10 + i64::from(d - b'0')).min(EXP_LIMIT)
            });
            exp10 = if negative { -magnitude } else { magnitude };
            has_exponent = true;
            i = exp_end;
        }
    }

    let (unit, unit_len) = if has_exponent {
        (Scale::Pow10(0), 0)
    } else {
        scale(&b[i..])
    };
    let digits_end = i;
    let letters = b[i..]
        .iter()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    i += letters;
    let exact = letters == 0 || {
        // one scale name (`MEG` and `MIL` match as `ME` and `MI`)
        let name_len = if has_exponent {
            scale(&b[digits_end..]).1
        } else {
            unit_len
        };
        let word = &b[digits_end..i];
        (name_len > 0 && letters == name_len)
            || word.eq_ignore_ascii_case(b"MEG")
            || word.eq_ignore_ascii_case(b"MIL")
    };

    let value = match unit {
        Scale::Pow10(n) => parse_decimal(mantissa, exp10 + n),
        Scale::Times(factor) => parse_decimal(mantissa, exp10) * factor,
        Scale::Db => 10_f64.powf(parse_decimal(mantissa, exp10) / 20.0),
    };
    Some(Scanned {
        value: if negative { -value } else { value },
        end: i,
        exact,
    })
}

/// Matches a scale factor or unit at the start of `b`: the scale and the
/// length of its name. No match gives `Pow10(0)` and 0.
fn scale(b: &[u8]) -> (Scale, usize) {
    SCALES
        .iter()
        .find(|(name, _)| {
            b.get(..name.len())
                .is_some_and(|p| p.eq_ignore_ascii_case(name))
        })
        .map_or((Scale::Pow10(0), 0), |&(name, scale)| (scale, name.len()))
}

/// Parses the unsigned `mantissa` times `10^exp10` with one correctly
/// rounded conversion, so `10p` gives exactly the double nearest to `1e-11`.
fn parse_decimal(mantissa: &str, exp10: i64) -> f64 {
    let parsed = if exp10 == 0 {
        mantissa.parse()
    } else {
        let mut buf = [0_u8; 64];
        let mut rest = &mut buf[..];
        if write!(rest, "{mantissa}e{exp10}").is_ok() {
            let len = 64 - rest.len();
            // Only ASCII was written.
            std::str::from_utf8(&buf[..len]).unwrap_or_default().parse()
        } else {
            format!("{mantissa}e{exp10}").parse()
        }
    };
    // `mantissa` is `digits[.digits]` with at least one digit, which
    // Rust's float grammar accepts.
    parsed.unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::scan_number;
    use crate::test_env::close;

    fn value(s: &str) -> f64 {
        scan_number(s)
            .unwrap_or_else(|| panic!("no number in {s:?}"))
            .0
    }

    fn consumed(s: &str) -> usize {
        scan_number(s)
            .unwrap_or_else(|| panic!("no number in {s:?}"))
            .1
    }

    #[test]
    fn scan_suffixes() {
        // Values from HSPICE P-2019.06 (golden case `units`).
        let cases: &[(&str, f64)] = &[
            ("1m", 1e-3),
            ("1M", 1e-3),
            ("1meg", 1e6),
            ("1MEG", 1e6),
            ("1me", 1e6),
            ("1x", 1e6),
            ("1mil", 25.4e-6),
            ("1mi", 25.4e-6),
            ("2.5k", 2500.0),
            ("10pF", 1e-11),
            ("1u", 1e-6),
            ("1n", 1e-9),
            ("1p", 1e-12),
            ("1f", 1e-15),
            ("1a", 1e-18),
            ("1t", 1e12),
            ("1g", 1e9),
            // HSPICE ignores a scale factor after an exponent.
            ("1e-3k", 1e-3),
            ("1V", 1.0),
        ];
        for &(s, want) in cases {
            let got = value(s);
            assert!(close(got, want), "{s}: got {got:e}, want {want:e}");
            assert_eq!(consumed(s), s.len(), "{s}: consumed");
        }
        assert_eq!(scan_number("10pF"), Some((1e-11, 4)));
        assert_eq!(scan_number("1V"), Some((1.0, 2)));
        assert_eq!(scan_number("abc"), None);
    }

    #[test]
    fn scan_units2() {
        // Values from HSPICE P-2019.06 (golden case `units2`).
        let cases: &[(&str, f64)] = &[
            ("1min", 60.0),
            ("1hr", 3600.0),
            ("1day", 86400.0),
            ("1yr", 31_536_000.0),
            ("1z", 1e-21),
            ("20db", 10.0),
            ("1meter", 1e6),
            ("1ms", 1e-3),
            ("1mv", 1e-3),
            ("1mohm", 1e-3),
        ];
        for &(s, want) in cases {
            let got = value(s);
            assert!(close(got, want), "{s}: got {got:e}, want {want:e}");
            assert_eq!(consumed(s), s.len(), "{s}: consumed");
        }
        // The listing prints `6db` as 1.9953: 10**(6/20).
        let got = value("6db");
        assert!((got - 1.9953).abs() <= 5e-5, "6db: {got}");
        assert!(close(got, 10_f64.powf(0.3)), "6db: {got}");
    }

    #[test]
    fn scan_unit_order_and_forms() {
        // `min` is read before the `mi` (mil) prefix; `mi` alone stays mil.
        assert!(close(value("1mi"), 25.4e-6));
        assert!(close(value("1mil"), 25.4e-6));
        assert!(close(value("1mile"), 25.4e-6));
        assert_eq!(scan_number("1.5MIN"), Some((90.0, 6)));
        assert_eq!(scan_number("1minute"), Some((60.0, 7)));
        assert_eq!(scan_number("2Hr"), Some((7200.0, 3)));
        assert_eq!(scan_number("1days"), Some((86400.0, 5)));
        assert_eq!(scan_number("2YRS"), Some((63_072_000.0, 4)));
        assert_eq!(scan_number("3Z"), Some((3e-21, 2)));
        assert_eq!(scan_number("1y"), Some((1e-24, 2)));
        assert_eq!(scan_number("40dB"), Some((100.0, 4)));
        assert_eq!(scan_number("0db"), Some((1.0, 3)));
        // Only the full unit counts: these letters are ignored.
        assert_eq!(scan_number("1h"), Some((1.0, 2)));
        assert_eq!(scan_number("1d"), Some((1.0, 2)));
        // As with `1e-3k`, letters after an exponent are ignored.
        assert_eq!(scan_number("1e1min"), Some((10.0, 6)));
        assert_eq!(scan_number("2e1db"), Some((20.0, 5)));
        // A sign applies to the converted value, the same as a unary minus
        // in an expression (`-20db` = -(20db)).
        assert_eq!(scan_number("-20db"), Some((-10.0, 5)));
        assert_eq!(scan_number("-1min"), Some((-60.0, 5)));
        assert_eq!(scan_number("-2k"), Some((-2000.0, 3)));
    }

    #[test]
    fn scan_mantissa_and_exponent_forms() {
        assert_eq!(scan_number("42"), Some((42.0, 2)));
        assert_eq!(scan_number(".5"), Some((0.5, 2)));
        assert_eq!(scan_number("1."), Some((1.0, 2)));
        assert_eq!(scan_number("1.5e3"), Some((1500.0, 5)));
        assert_eq!(scan_number("1.5E+3"), Some((1500.0, 6)));
        assert_eq!(scan_number("-2k"), Some((-2000.0, 3)));
        assert_eq!(scan_number("+.5u"), Some((0.5e-6, 4)));
        // An `e` without exponent digits is an ignored letter, not an exponent.
        assert_eq!(scan_number("3e"), Some((3.0, 2)));
        assert_eq!(scan_number("3e+"), Some((3.0, 2)));
        // ME (MEG, MEGA) and MI (MIL) win over M; mixed case is accepted.
        assert_eq!(scan_number("2MeG"), Some((2e6, 4)));
        assert_eq!(scan_number("2mega"), Some((2e6, 5)));
        assert_eq!(scan_number("2Mil"), Some((2.0 * 25.4e-6, 4)));
        assert_eq!(scan_number("2ms"), Some((2e-3, 3)));
        // No scale factor after an exponent: the letters are only skipped.
        assert_eq!(scan_number("1e3meg"), Some((1e3, 6)));
        assert_eq!(scan_number("2.5E-2mil"), Some((2.5e-2, 9)));
        assert_eq!(scan_number("1e-12F"), Some((1e-12, 6)));
        // Scanning stops at the first byte that is not part of the literal.
        assert_eq!(scan_number("2u*w"), Some((2e-6, 2)));
        assert_eq!(scan_number("5ns)"), Some((5e-9, 3)));
        assert_eq!(scan_number("1.5.3"), Some((1.5, 3)));
        assert_eq!(scan_number("7_x"), Some((7.0, 1)));
    }

    #[test]
    fn scan_rejects_non_numbers() {
        for s in ["", ".", "-", "+", "e3", "-e3", "k", " 1", "-.e1"] {
            assert_eq!(scan_number(s), None, "{s:?}");
        }
    }

    #[test]
    fn exact_number_rule() {
        use super::exact_number;
        // One number with at most one scale factor or unit name.
        for (s, want) in [
            ("2u", 2e-6),
            ("1e-6", 1e-6),
            ("0.5", 0.5),
            ("7", 7.0),
            ("-2k", -2000.0),
            ("1meg", 1e6),
            ("1MEG", 1e6),
            ("1me", 1e6),
            ("1mil", 25.4e-6),
            ("1MI", 25.4e-6),
            ("1min", 60.0),
            ("20db", 10.0),
            ("1x", 1e6),
            ("3Z", 3e-21),
            (".5", 0.5),
            // HSPICE reads a scale after an exponent as a number and ignores
            // the scale (golden `units`: `1e-3k` = 0.001, with a warning).
            ("1e-3k", 1e-3),
            ("1e3MEG", 1e3),
        ] {
            let got = exact_number(s).unwrap_or_else(|| panic!("{s:?}: None"));
            assert!(close(got, want), "{s}: got {got:e}, want {want:e}");
        }
        // Other letters after the number (`scan_number` ignores them), or
        // no number.
        for s in [
            "2umos", "5vnmos", "33nmos", "10pF", "1V", "1meter", "1e-3kohm", "2mega", "1d",
            "2n7002", "nch", "", "1.5.3", "2u*w",
        ] {
            assert_eq!(exact_number(s), None, "{s:?}");
        }
        // `scan_number_exact` gives the value and the length of
        // `scan_number`, and whether the literal is exact.
        use super::scan_number_exact;
        assert_eq!(scan_number_exact("2u*w"), Some((2e-6, 2, true)));
        assert_eq!(scan_number_exact("2umos"), Some((2e-6, 5, false)));
        assert_eq!(scan_number_exact("2n7002"), Some((2e-9, 2, true)));
        assert_eq!(scan_number_exact("1mil"), Some((25.4e-6, 4, true)));
        assert_eq!(scan_number_exact("nch"), None);
    }

    #[test]
    fn scan_huge_exponent_is_infinite_not_a_panic() {
        assert_eq!(
            scan_number("1e99999999999999999999"),
            Some((f64::INFINITY, 22))
        );
        assert_eq!(scan_number("1e-99999999999999999999"), Some((0.0, 23)));
    }
}
