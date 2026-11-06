use super::*;

// elements

#[tokio::test]
async fn resistor_capacitor_inductor_params_and_models() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "R1 a b 1k m=2 tc1=1e-3\nR2 a b r=2k\nR3 a b rmod r=3k\nR4 a b rmod 4k w=1u\n\
         C1 a b cmod 1f\nC2 a b c=2f m=4\nL1 a b 1n lmod\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let r = |name| match &inst(&ast, name).ctx {
        InstanceCtx::Resistor(r) => r,
        _ => panic!(),
    };
    assert!(close(value_of(&r("R1").value), 1e3));
    assert_eq!(r("R1").params.len(), 2);
    assert!(close(value_of(&r("R2").value), 2e3) && r("R2").params.is_empty());
    assert_eq!(r("R3").model.as_deref(), Some("rmod"));
    assert!(close(value_of(&r("R3").value), 3e3));
    assert_eq!(r("R4").model.as_deref(), Some("rmod"));
    assert!(close(value_of(&r("R4").value), 4e3) && r("R4").params.len() == 1);
    let InstanceCtx::Capacitor(c1) = &inst(&ast, "C1").ctx else {
        panic!()
    };
    assert_eq!(c1.model.as_deref(), Some("cmod"));
    assert!(close(value_of(&c1.value), 1e-15));
    let InstanceCtx::Capacitor(c2) = &inst(&ast, "C2").ctx else {
        panic!()
    };
    assert!(close(value_of(&c2.value), 2e-15) && c2.params.len() == 1);
    let InstanceCtx::Inductor(l1) = &inst(&ast, "L1").ctx else {
        panic!()
    };
    assert_eq!(l1.model.as_deref(), Some("lmod"));
    assert!(close(value_of(&l1.value), 1e-9));
}

/// `DC`, `AC`, transient functions and PWL without parentheses used to be
/// misread or wipe the file.
#[tokio::test]
async fn independent_sources() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "V1 vdd 0 DC 1.8\n\
         V2 in 0 PULSE(0 1.8 0 10p 10p 1n 2n)\n\
         V3 in 0 PULSE 0 1.8 0 10p 10p 1n 2n\n\
         V4 in 0 DC 0 AC 1 90 SIN(0, 1, 1meg)\n\
         V5 in 0 PWL 0 0 1n 1.8 R TD=1n\n\
         V6 in 0 PWL(0 0\n+ 1n 1.8)\n\
         I1 0 a 1m\n\
         I2 0 a AC=1\n\
         V7 vdd 0 'vsupply' M=2\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.instance.len(), 9, "no fake instances:\n{ast}");
    let v = |name| match &inst(&ast, name).ctx {
        InstanceCtx::Voltage(v) => &v.source,
        _ => panic!(),
    };
    let dc = |s: &Source<'_>| num(&s.dc.as_ref().expect("a DC value").1);
    assert!(close(dc(v("V1")), 1.8));
    for name in ["V2", "V3"] {
        let f = v(name).func.as_ref().expect(name);
        assert_eq!(f.name.to_uppercase(), "PULSE");
        let args = func_args(v(name));
        assert_eq!(args.len(), 7);
        assert_eq!(args[1], "1.8");
    }
    let v4 = v("V4");
    assert_eq!(dc(v4), 0.0);
    let (mag, phase) = (v4.ac_mag.as_ref().unwrap(), v4.ac_phase.as_ref().unwrap());
    assert!(close(num(&mag.1), 1.0) && close(num(&phase.1), 90.0));
    assert_eq!(func_args(v4).len(), 3);
    let v5 = v("V5");
    assert_eq!(func_args(v5).len(), 4);
    let keys: Vec<_> = v5.params.iter().map(|kv| kv.k.to_string()).collect();
    assert_eq!(keys, ["R", "TD"]);
    assert_eq!(func_args(v("V6")).len(), 4);
    let i = |name| match &inst(&ast, name).ctx {
        InstanceCtx::Current(i) => &i.source,
        _ => panic!(),
    };
    assert!(close(dc(i("I1")), 1e-3));
    assert!(i("I2").ac_mag.is_some());
    let v7 = v("V7");
    assert!(matches!(&v7.dc, Some((_, Value::Expr(e))) if e == "vsupply"));
    assert_eq!(v7.params.len(), 1);
}

/// E/F/G/H/K used to be generic elements whose gains (and coupled inductor
/// names) showed up as nodes.
#[tokio::test]
async fn controlled_sources_and_mutual_inductors() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "E1 out 0 in 0 10\n\
         G1 out 0 VCCS in 0 1m MAX=1\n\
         E2 out 0 POLY(2) a 0 b 0 0 1 1\n\
         F1 out 0 vsense 2\n\
         H1 out 0 POLY(2) v1 v2 0 1k 1k\n\
         E3 out 0 VOL='V(a)*2'\n\
         K1 L1 L2 0.95\n\
         K2 L1 L3 K=0.5\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let c = |name| match &inst(&ast, name).ctx {
        InstanceCtx::ControlledSource(c) => c,
        _ => panic!("{name}"),
    };
    assert_eq!(c("E1").kind, ControlledKind::E);
    assert!(close(num(&c("E1").coeffs[0]), 10.0));
    assert_eq!(nets(inst(&ast, "E1")), ["0", "in", "out"]);
    assert_eq!(c("G1").keyword.as_deref(), Some("VCCS"));
    assert!(close(num(&c("G1").coeffs[0]), 1e-3) && c("G1").params.len() == 1);
    assert_eq!(c("E2").poly, Some(2));
    assert_eq!(c("E2").ctrl_nodes.len(), 2);
    assert_eq!(c("E2").coeffs.len(), 3);
    assert_eq!(nets(inst(&ast, "E2")), ["0", "a", "b", "out"]);
    assert_eq!(c("F1").ctrl_sources, ["vsense"]);
    // the controlling source is an element, not a node
    assert_eq!(nets(inst(&ast, "F1")), ["0", "out"]);
    assert_eq!(c("H1").ctrl_sources, ["v1", "v2"]);
    assert!(c("E3").coeffs.is_empty() && c("E3").params.len() == 1);
    for name in ["K1", "K2"] {
        let InstanceCtx::MutualInductor(k) = &inst(&ast, name).ctx else {
            panic!()
        };
        assert_eq!(k.l1, "L1");
        assert!(nets(inst(&ast, name)).is_empty());
    }
}

/// Forms that are not supported yet are reported instead of misread.
#[tokio::test]
async fn unsupported_controlled_form_is_reported() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "* title\nE1 out 0 DELAY in 0 TD=1n\nH1 out 0 LAPLACE vin 1 0 / 1 1\nR1 a b 1k\n",
    )])
    .await;
    assert_eq!(
        diagnostics(&parsed),
        [diag("Unsupported", 2), diag("Unsupported", 3)]
    );
    let (ast, _) = files.build(parsed);
    assert_eq!(names(&ast), ["R1"]);
}
