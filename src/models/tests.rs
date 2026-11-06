use super::Short;

/// The fewest digits that give back the number within 1e-12 (relative)
#[test]
fn short_numbers() {
    let cases = [
        (1e-6, "1e-6"),
        (1.2e-4, "1.2e-4"),
        (1.1999999999999999e-4, "1.2e-4"),
        (100e-9 + 123456.0e-12, "2.23456e-7"),
        (std::f64::consts::PI, "3.14159265359e0"),
        (-2.5e3, "-2.5e3"),
        (0.0, "0e0"),
    ];
    for (x, text) in cases {
        assert_eq!(Short(x).to_string(), text);
    }
}
