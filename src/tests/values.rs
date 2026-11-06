use super::*;

// values and names

#[tokio::test]
async fn scale_factors_follow_hspice() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".param milli=1m milli2=1M mega=1meg mega2=1Megohm kilo=10kohm\n\
         + pf=10pF ff=0.5fF volt=1.8V ns=5ns mil=1mil db=20db minute=1min dot=.5p\n\
         + e=nand i=info\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let param = |k: &str| &ast.param.iter().find(|kv| kv.k == k).unwrap().v;
    for (k, v) in [
        ("milli", 1e-3),
        ("milli2", 1e-3),
        ("mega", 1e6),
        ("mega2", 1e6),
        ("kilo", 1e4),
        ("pf", 1e-11),
        ("ff", 5e-16),
        ("volt", 1.8),
        ("ns", 5e-9),
        ("mil", 25.4e-6),
        ("db", 10.0),
        ("minute", 60.0),
        ("dot", 5e-13),
    ] {
        assert!(close(num(param(k)), v), "{k}: {:?} != {v}", param(k));
    }
    // names are not NaN or infinity
    assert!(matches!(param("e"), Value::Expr(e) if e == "nand"));
    assert!(matches!(param("i"), Value::Expr(e) if e == "info"));
}

#[tokio::test]
async fn unquoted_function_calls_are_expressions() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".param a=agauss(0,1,3) b=2*(x+y)\nX1 n1 n2 sub w=max(1u,2u)\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert!(matches!(&ast.param[0].v, Value::Expr(e) if e == "agauss(0,1,3)"));
    assert!(matches!(&ast.param[1].v, Value::Expr(e) if e == "2*(x+y)"));
    let InstanceCtx::Subckt(x) = &inst(&ast, "X1").ctx else {
        panic!()
    };
    assert!(matches!(&x.params[0].v, Value::Expr(e) if e == "max(1u,2u)"));
}

/// CDL bus bits (`A<0>`), global nets (`VDD!`) and `/` before the subckt
/// name used to wipe the file.
#[tokio::test]
async fn cdl_names() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        "*.PININFO A<0>:I A<1>:I Y:O\n\
         .SUBCKT NAND2 A<0> A<1> Y VDD! VSS!\n\
         MM0 Y A<0> VDD! VDD! pch W=1u L=0.1u\n\
         MM1 Y A<1> net1 VSS! nch W=1u L=0.1u\n\
         .ENDS\n\
         X1 a<0> a<1> y VDD! VSS! / NAND2\n\
         X2 a<0> a<1> y VDD! VSS! /NAND2 $PINS A<0>=a<0>\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(
        ast.subckt["nand2"].ports,
        ["A<0>", "A<1>", "Y", "VDD!", "VSS!"]
    );
    for name in ["X1", "X2"] {
        let InstanceCtx::Subckt(x) = &inst(&ast, name).ctx else {
            panic!()
        };
        assert_eq!(x.cktname, "NAND2");
        assert_eq!(x.nodes, ["a<0>", "a<1>", "y", "VDD!", "VSS!"]);
    }
}

#[tokio::test]
async fn binned_models_and_models_without_params() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".model nch.1 nmos level=54 lmin=1e-8\n.model dmod d\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    assert_eq!(ast.model[0].name, "nch.1");
    assert_eq!(ast.model[0].params.len(), 2);
    assert_eq!(ast.model[1].name, "dmod");
    assert!(ast.model[1].params.is_empty());
}

#[tokio::test]
async fn ic_and_nodeset_forms() {
    let (_dir, parsed, files) = parse_text(&[(
        "top.sp",
        ".nodeset V(out)=0.9\n.nodeset a 1 b=2\n.ic out=1 V(x) = 0 subckt=INV\n",
    )])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, _) = files.build(parsed);
    let nodes: Vec<_> = ast.nodeset.iter().map(|n| n.0.to_string()).collect();
    assert_eq!(nodes, ["out", "a", "b"]);
    assert_eq!(ast.init_condition.len(), 2);
    assert!(
        ast.init_condition
            .iter()
            .all(|ic| ic.2.as_deref() == Some("INV"))
    );
}

#[tokio::test]
async fn double_quoted_include() {
    let (_dir, parsed, files) = parse_text(&[
        ("top.sp", ".include \"small.sp\"\n"),
        ("small.sp", "R0 a b 1k\n"),
    ])
    .await;
    assert_eq!(diagnostics(&parsed), []);
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(names(&ast), ["R0"]);
}
