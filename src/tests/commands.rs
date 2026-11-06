use super::*;

// commands and output

/// Valid HSPICE control cards used to be reported as errors, and were written
/// back with quoted tokens (`.print 'tran'`).
#[tokio::test]
async fn control_cards_are_kept_verbatim() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".global vdd vss\n.temp 25\n.tran 1p 10n SWEEP data=d1\n.op\n.options post=2 probe\n\
         .print tran v(out) i(V1)\n.probe tran v(out) par('v(out)*2')\n\
         .meas tran tdel trig v(in) val=0.9 rise=1\n+ targ v(out) val=0.9 fall=1\n\
         .foo bar\n",
    )])
    .await;
    // only the truly unknown `.foo` is reported, as a warning
    assert_eq!(diagnostics(&parsed), [diag("UnknownCommand", 10)]);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(ast.option.len(), 2);
    let text = ast.to_string();
    for line in [
        ".GLOBAL vdd vss",
        ".temp 25",
        ".tran 1p 10n SWEEP data=d1",
        ".op",
        ".print tran v(out) i(V1)",
        ".probe tran v(out) par('v(out)*2')",
        ".meas tran tdel trig v(in) val=0.9 rise=1\n+ targ v(out) val=0.9 fall=1",
        ".foo bar",
    ] {
        assert!(text.contains(line), "missing `{line}` in:\n{text}");
    }
}

/// Printing used to produce invalid SPICE (`w=1e-6.OPTION`, `suckt=`). The
/// output must parse back without diagnostics, to the same output.
#[tokio::test]
async fn display_round_trips() {
    const DECK: &str = "\
.option post=2
.param vsupply=1.8 w0='2*lmin'
.model nch.1 nmos level=54
.subckt INV a y vdd vss w=1u
.option scale=1
M1 y a vss vss nch W=w L=0.1u
M2 y a vdd vdd pch W='2*w' L=0.1u
.ends INV
X1 in out vdd 0 INV w=2u
Xcdl a<0> b VDD! 0 / INV
R1 a b rmod 1k m=2
L1 a b 1n lmod
C1 a 0 10fF
V1 vdd 0 DC 1.8
V2 in 0 DC 0 PULSE(0 1.8 0 10p 10p 1n 2n) AC 1 90 M=1
V3 in 0 PWL(0 0 1n 1.8 R TD=1n)
I1 0 a SIN(0 1m 1meg)
I2 0 a PAT(1 0 0 1n 1n 5n b1010 r=1)
E1 out 0 POLY(2) a 0 b 0 0 1 1
F1 out 0 vsense 2
E2 out 0 VOL='V(a)*2'
K1 L1 L2 0.95
Q1 c b e npn area=2
D1 a 0 dmod
J1 d g s jmod
.ic V(a)=1 subckt=INV
.nodeset b=0.5
.data d1
+ p1 p2
+ 1 2
+ 3 4
.enddata
.global vdd
.tran 1p 10n
.meas tran t1 trig v(in) val=0.9 rise=1
+ targ v(out) val=0.9 fall=1
";
    let (_dir, parsed, files) = parse_text(&[("top.sp", DECK)]).await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    let first = ast.to_string();
    assert!(!first.contains("suckt"), "{first}");
    let (_dir2, parsed2, files2) = parse_text(&[("top.sp", &first)]).await;
    assert_eq!(diagnostics(&parsed2), [], "printed netlist:\n{first}");
    let (ast2, has_err2) = files2.build(parsed2);
    assert!(!has_err2);
    assert_eq!(count_instances(&ast2), count_instances(&ast));
    assert_eq!(ast2.to_string(), first);
}

/// Text after `.ends` / `.end` is ignored, as in HSPICE.
#[tokio::test]
async fn text_after_ends_and_end_is_ignored() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".subckt PM_INV%A a y\nR1 a y 1k\n.ends PM_INV%A * note\nR2 a b 1k\n.end 'title' 42\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert!(ast.subckt.contains_key("pm_inv%a"));
    assert_eq!(names(&ast), ["R2"]);
}
