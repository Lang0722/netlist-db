use super::*;

// `.if` blocks (each result as measured on HSPICE P-2019.06, decks `if_*`,
// `if2_*` and `ord_*` of `scripts/hspice_kit/forms*.py`)

/// `(path, L in um)` of every MOSFET of the deck `text`
async fn if_lengths(text: &str) -> Vec<(String, f64)> {
    let (_dir, parsed, files) = parse_deck(&[("deck.sp", text)]).await;
    let (ast, _) = files.build(parsed);
    ast.mosfet_models()
        .into_iter()
        .map(|m| match &m.selection {
            Ok(s) => (m.path, (s.l * 1e12).round() / 1e6),
            Err(e) => panic!("{}: {e}\n{text}", m.path),
        })
        .collect()
}

const IF_M3: &str = "M3 d g 0 0 n l=3u w=3u\n";

fn if_paths(rows: &[(String, String)]) -> Vec<&str> {
    rows.iter().map(|(p, _)| p.as_str()).collect()
}

#[tokio::test]
async fn if_counts_only_the_active_branch() {
    for (head, want) in [
        (
            ".if (1)\nM1 d g 0 0 n l=1u w=1u\n.else\nM2 d g 0 0 n l=2u w=2u\n.endif\n",
            "M1",
        ),
        (
            ".if (0)\nM1 d g 0 0 n l=1u w=1u\n.else\nM2 d g 0 0 n l=2u w=2u\n.endif\n",
            "M2",
        ),
        (
            ".IF(1)\nM1 d g 0 0 n l=1u w=1u\n.ELSE\nM2 d g 0 0 n l=2u w=2u\n.ENDIF\n",
            "M1",
        ),
        (".if ('1>0')\nM1 d g 0 0 n l=1u w=1u\n.endif\n", "M1"),
        (".if (\"1>0\")\nM1 d g 0 0 n l=1u w=1u\n.endif\n", "M1"),
        (
            ".if ((1>0)&&(2>1)) $ note\nM1 d g 0 0 n l=1u w=1u\n.endif $ note\n",
            "M1",
        ),
        (
            ".if (0)\nM1 d g 0 0 n l=1u w=1u\n.elseif(1)\nM2 d g 0 0 n l=2u w=2u\n.else\nM3 d g 0 0 n l=3u w=3u\n.endif\n",
            "M2",
        ),
        (
            ".if (1)\nM1 d g 0 0 n l=1u w=1u\n.elseif (1)\nM2 d g 0 0 n l=2u w=2u\n.else\nM3 d g 0 0 n l=3u w=3u\n.endif\n",
            "M1",
        ),
        (
            ".if (0)\nM1 d g 0 0 n l=1u w=1u\n.elseif (0)\nM2 d g 0 0 n l=2u w=2u\n.else\nM3 d g 0 0 n l=3u w=3u\n.endif\n",
            "M3",
        ),
    ] {
        let (has_err, rows) = if_deck(&format!("{IF_N}{head}")).await;
        assert!(!has_err, "{head}");
        assert_eq!(if_paths(&rows), [want], "{head}");
    }
}

/// A value is true when it is not 0, however small
#[tokio::test]
async fn if_truth_is_any_value_that_is_not_zero() {
    for value in ["0.5", "-1", "1e-20", "1e-300"] {
        let text = format!("{IF_N}.param t={value}\n.if (t)\n{IF_M1}.else\n{IF_M2}.endif\n");
        assert_eq!(if_paths(&if_deck(&text).await.1), ["M1"], "{value}");
    }
    let text = format!("{IF_N}.if (0)\n{IF_M1}.else\n{IF_M2}.endif\n");
    assert_eq!(if_paths(&if_deck(&text).await.1), ["M2"]);
}

/// Text in a branch that is not active is not read: names need no definition
#[tokio::test]
async fn if_inactive_branch_is_not_evaluated() {
    for body in [
        ".if (0)\n.if (undefined_name)\nM1 d g 0 0 n l=1u w=1u\n.endif\n.endif\nM2 d g 0 0 n l=2u w=2u\n",
        ".if (1)\nM2 d g 0 0 n l=2u w=2u\n.elseif (undefined_name)\nM3 d g 0 0 n l=3u w=3u\n.endif\n",
        ".if (0)\nM1 d g 0 0 nomodel l=1u w=1u\nX1 d g nosubckt\n.param q='undefined_name'\n.endif\nM2 d g 0 0 n l=2u w=2u\n",
    ] {
        let (has_err, rows) = if_deck(&format!("{IF_N}{body}")).await;
        assert!(!has_err, "{body}");
        assert_eq!(if_paths(&rows), ["M2"], "{body}");
        assert!(rows[0].1 == "n", "{body}: {rows:?}");
    }
}

/// A condition is evaluated for each instance, with the values passed to it
#[tokio::test]
async fn if_is_evaluated_for_each_instance() {
    let text = format!(
        "{IF_N}.macro s d g w=1u\n.if (w>2u)\nM1 d g 0 0 n l=1u w=w\n.else\nM2 d g 0 0 n l=2u w=w\n.endif\n.eom\n\
         X1 d g s w=1u\nX2 d g s w=3u\n"
    );
    assert_eq!(if_paths(&if_deck(&text).await.1), ["X1.M2", "X2.M1"]);
    // with parhier: the top level wins over a value passed (global only)
    let body = ".param k=1\n.subckt s d g\n.if (k==1)\nM1 d g 0 0 n l=1u w=1u\n.else\nM2 d g 0 0 n l=2u w=2u\n.endif\n.ends\nX1 d g s k=2\n";
    let global = format!("{IF_N}{body}");
    assert_eq!(if_paths(&if_deck(&global).await.1), ["X1.M1"]);
    let local = format!("{IF_N}.option parhier=local\n{body}");
    assert_eq!(if_paths(&if_deck(&local).await.1), ["X1.M2"]);
}

/// The values of `.param` follow the order of the file: a condition sees all
/// plain `.param` statements of its level, and the branches of earlier blocks
#[tokio::test]
async fn if_params_follow_the_order_of_the_file() {
    let b = |q: &str| format!(".if (1)\n.param q={q}\n.endif\n");
    let use_q = "M1 d g 0 0 n l=q w=1u\n";
    for (body, want) in [
        (format!(".param q=1u\n{}{use_q}", b("2u")), 2.0),
        (format!("{}.param q=1u\n{use_q}", b("2u")), 1.0),
        (format!(".if (0)\n.param q=2u\n.endif\n.param q=1u\n{use_q}"), 1.0),
        (format!("{}{}{use_q}", b("1u"), b("2u")), 2.0),
        // a text read later
        (".if (1)\n.param r='q*2'\n.endif\n.param q=1u\nM1 d g 0 0 n l=r w=1u\n".to_string(), 2.0),
        (format!(".param q=1u\n.param r='q*2'\n{}M1 d g 0 0 n l=r w=1u\n", b("2u")), 4.0),
        // nested blocks
        (format!(".if (1)\n{}.param q=1u\n.endif\n{use_q}", b("2u")), 1.0),
        (format!(".if (1)\n.param q=1u\n{}.endif\n{use_q}", b("2u")), 2.0),
        (
            ".if (1)\n.param q=1\n.if (q==1)\n.param q=2\n.endif\n.if (q==2)\n.param q=3\n.endif\n.endif\nM1 d g 0 0 n l='q*1u' w=1u\n".to_owned(),
            3.0,
        ),
    ] {
        let text = format!("{IF_N}{body}");
        assert_eq!(if_lengths(&text).await, [("M1".to_owned(), want)], "{body}");
    }
    let m = format!("{IF_M1}.else\n{IF_M2}.endif\n");
    for (body, want) in [
        (format!(".param q=1\n.if (q>1)\n{m}.param q=2\n"), "M1"),
        (format!(".if (q>1)\n{m}.param q=1\n.param q=2\n"), "M1"),
        (format!(".param q=1\n{}.if (q>1)\n{m}", b("2")), "M1"),
        (format!("{}.param q=1\n.if (q>1)\n{m}", b("2")), "M2"),
        (format!("{}.if (q>1)\n{m}.param q=1\n", b("2")), "M2"),
        (format!(".param q=1\n.if (q>1)\n{m}{}", b("2")), "M2"),
    ] {
        let text = format!("{IF_N}{body}");
        assert_eq!(if_paths(&if_deck(&text).await.1), [want], "{body}");
    }
}

/// A value passed hides a branch's `.param`, which hides a `.subckt` default
#[tokio::test]
async fn if_params_of_an_instance() {
    let text = format!(
        "{IF_N}.option parhier=local\n.subckt s d g k=1\n.if (k==1)\n.param k=2\n.endif\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\n\
         X1 d g s\nX2 d g s k=5\nX3 d g s k=1\n"
    );
    assert_eq!(
        if_lengths(&text).await,
        [
            ("X1.M1".to_owned(), 2.0),
            ("X2.M1".to_owned(), 5.0),
            ("X3.M1".to_owned(), 1.0)
        ]
    );
    // the top level's branches are of the top level
    let body = ".if (1)\n.param k=9\n.endif\n.subckt s d g k=2\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s\n";
    assert_eq!(
        if_lengths(&format!("{IF_N}{body}")).await,
        [("X1.M1".to_owned(), 9.0)]
    );
    let local = format!("{IF_N}.option parhier=local\n{body}");
    assert_eq!(if_lengths(&local).await, [("X1.M1".to_owned(), 2.0)]);
    // a false branch binds nothing
    let hide = |c: u8| {
        format!(
            "{IF_N}.option parhier=local\n.param k=9\n.subckt s d g\n.if ({c})\n.param k=2\n.endif\nM1 d g 0 0 n l='k*1u' w=1u\n.ends\nX1 d g s\n"
        )
    };
    assert_eq!(if_lengths(&hide(1)).await, [("X1.M1".to_owned(), 2.0)]);
    assert_eq!(if_lengths(&hide(0)).await, [("X1.M1".to_owned(), 9.0)]);
}

/// Models and subckts in a branch are seen by the whole level, also above the
/// block. Bins keep the order of the file across a branch and the level.
#[tokio::test]
async fn if_models_and_subckts_in_a_branch() {
    let text = "* t\nM1 d g 0 0 q l=1u w=1u\n.if (1)\n.model q nmos level=54\n.else\n.model q pmos level=54\n.endif\n".to_string();
    let (has_err, rows) = if_deck(&text).await;
    assert!(!has_err);
    assert_eq!(rows, pairs(&[("M1", "q")]));
    let text = format!(
        "{IF_N}.subckt a d g\n{IF_M1}.ends\n.subckt b d g\n{IF_M2}.ends\n.param t=2\n.if (t==1)\nXs d g a\n.elseif (t==2)\nXs d g b\n.else\nXs d g a\n.endif\n"
    );
    assert_eq!(if_paths(&if_deck(&text).await.1), ["Xs.M2"]);
    // a branch defines the same name in each case
    let text = format!(
        "{IF_N}.param t=1\n.if (t==1)\n.subckt c d g\n{IF_M1}.ends\n.else\n.subckt c d g\n{IF_M2}.ends\n.endif\nX1 d g c\n"
    );
    assert_eq!(if_paths(&if_deck(&text).await.1), ["X1.M1"]);
    let bin =
        |s: &str| format!(".model b.{s} nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-3\n");
    let m = "M1 d g 0 0 b l=1u w=1u\n";
    let first_plain = format!(
        "* t\n.model b.2 nmos level=54 lmin=1e-8 lmax=1e-3 wmin=1e-8 wmax=1e-3\n.if (1)\n{}.endif\n{m}",
        bin("1")
    );
    assert_eq!(if_deck(&first_plain).await.1, pairs(&[("M1", "b.2")]));
    let first_branch = format!("* t\n.if (1)\n{}.endif\n{}{m}", bin("1"), bin("2"));
    assert_eq!(if_deck(&first_branch).await.1, pairs(&[("M1", "b.1")]));
}

/// The lines of a level keep the order of the file, with a branch in its place
#[tokio::test]
async fn if_lines_keep_the_order_of_the_file() {
    let text = format!("{IF_N}{IF_M1}.if (1)\n{IF_M2}.endif\n{IF_M3}");
    assert_eq!(if_paths(&if_deck(&text).await.1), ["M1", "M2", "M3"]);
}

/// `.if` can open in one file and close in another; `.lib` and `.inc` are read
/// in a true branch
#[tokio::test]
async fn if_across_files_and_libraries() {
    let (_dir, parsed, files) = parse_deck(&[
        ("deck.sp", "* t\n.model n nmos level=54\n.if (0)\n.inc 'mid.inc'\nM3 d g 0 0 n l=3u w=3u\n.endif\n"),
        ("mid.inc", "M1 d g 0 0 n l=1u w=1u\n.else\nM2 d g 0 0 n l=2u w=2u\n"),
    ])
    .await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(if_paths(&chosen(&ast)), ["M2", "M3"]);

    let lib = ".lib tt\n.param lc=1u\n.model q nmos level=54\n.endl tt\n.lib ss\n.param lc=2u\n.model q pmos level=54\n.endl ss\n";
    let (_dir, parsed, files) = parse_deck(&[
        ("deck.sp", "* t\n.param corner=2\n.if (corner==1)\n.lib 'c.l' tt\n.else\n.lib 'c.l' ss\n.endif\nM1 d g 0 0 q l=lc w=1u\n"),
        ("c.l", lib),
    ])
    .await;
    let (ast, _) = files.build(parsed);
    let m = ast.mosfet_models().remove(0);
    let s = m.selection.unwrap();
    assert!(close(s.l, 2e-6), "{}", s.l);
    assert!(matches!(s.model.model_type, crate::ast::ModelType::PMOS));
}

/// HSPICE stops at these forms: each one is an error here
#[tokio::test]
async fn if_bad_forms_are_errors() {
    for body in [
        // a block that does not close, or closes what is not open
        ".if (1)\nM1 d g 0 0 n l=1u w=1u\n",
        ".endif\n",
        ".else\n",
        ".elseif (1)\n",
        ".if (1)\n.else\n.else\n.endif\n",
        ".if (1)\n.else\n.elseif (1)\n.endif\n",
        // across a body, or with `.alter` in it
        ".subckt s d g\n.if (1)\n.ends\n.endif\n",
        ".if (1)\n.subckt s d g\n.endif\n.ends\n",
        ".if (1)\n.alter\n.endif\n",
        // the condition
        ".if\n.endif\n",
        ".if ()\n.endif\n",
        ".if 1\n.endif\n",
        ".if {1}\n.endif\n",
        ".if (1) && (1)\n.endif\n",
        ".if (1\n+ )\n.endif\n",
        ".if (0)\n.elseif 1\n.endif\n",
        ".if (1) text\n.endif\n",
        ".if (1)\n+ text\n.endif\n",
        // text after the keywords
        ".if (1)\n.else text\n.endif\n",
        ".if (1)\n.else if (1)\n.endif\n",
        ".if (1)\n.endif text\n",
        ".if (1)\n.endif\n+ w=3u\n",
    ] {
        let (has_err, _) = if_deck(&format!("{IF_N}{body}")).await;
        assert!(has_err, "no error for:\n{body}");
    }
    // `.elsif` is not a keyword: it is an unknown command, and is ignored
    let text = format!("{IF_N}.if (0)\n{IF_M1}.elsif (1)\n{IF_M2}.else\n{IF_M3}.endif\n");
    let (has_err, rows) = if_deck(&text).await;
    assert!(!has_err);
    assert_eq!(if_paths(&rows), ["M3"]);
}

/// A condition that cannot be computed is one row with an error; it is not a
/// MOSFET, and the lines around the block still count
#[tokio::test]
async fn if_condition_that_cannot_be_computed() {
    for cond in ["nothing>1", "1=1", "'a'=='a'", "temper>50"] {
        let text = format!(
            "{IF_N}.subckt s d g\n{IF_M1}.if ({cond})\n{IF_M2}.else\n{IF_M3}.endif\n.ends\nX1 d g s\nX2 d g s\n"
        );
        let (_dir, parsed, files) = parse_deck(&[("deck.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let rows = chosen(&ast);
        assert_eq!(rows.len(), 4, "{cond}: {rows:?}");
        assert_eq!(rows[0].0, "X1.if", "{cond}");
        assert!(rows[0].1.starts_with("error: "), "{cond}: {rows:?}");
        assert_eq!(rows[1], ("X1.M1".to_owned(), "n".to_owned()), "{cond}");
        assert!(matches!(
            ast.mosfet_models()[0].selection,
            Err(crate::models::SelectError::Condition { .. })
        ));
        let usage = ast.model_usage();
        assert_eq!(usage.mosfets, 2, "{cond}");
        assert_eq!(usage.errors.len(), 1, "{cond}");
        assert_eq!(usage.errors[0].1, 2, "{cond}");
    }
}

/// `.option` in an active branch applies; in another branch it does not
#[tokio::test]
async fn if_options() {
    let parhier = |c: u8| {
        format!(
            "{IF_N}.if ({c})\n.option parhier=local\n.endif\n.param k=1u\n.subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n"
        )
    };
    assert_eq!(if_lengths(&parhier(1)).await, [("X1.M1".to_owned(), 2.0)]);
    assert_eq!(if_lengths(&parhier(0)).await, [("X1.M1".to_owned(), 1.0)]);
    let scale =
        |c: u8| format!("{IF_N}.if ({c})\n.option scale=1e-6\n.endif\nM1 d g 0 0 n l=0.5 w=2\n");
    assert_eq!(if_lengths(&scale(1)).await, [("M1".to_owned(), 0.5)]);
    assert_eq!(if_lengths(&scale(0)).await, [("M1".to_owned(), 500000.0)]);
    // the order of the file decides between a branch and the level
    let order = |before: &str, after: &str| {
        format!("{IF_N}{before}.if (1)\n.option scale=2e-6\n.endif\n{after}M1 d g 0 0 n l=1 w=1\n")
    };
    assert_eq!(
        if_lengths(&order(".option scale=1e-6\n", "")).await,
        [("M1".to_owned(), 2.0)]
    );
    assert_eq!(
        if_lengths(&order("", ".option scale=1e-6\n")).await,
        [("M1".to_owned(), 1.0)]
    );
}

/// [`AST::model_usage`] counts the active branches of each instance, as the
/// list does
#[tokio::test]
async fn if_usage_agrees_with_the_list() {
    let leaf = ".subckt leaf d g v=1\n.if (v>1)\n.param w0=1u\nM1 d g 0 0 n l=1u w=w0\n.else\nM2 d g 0 0 p l=2u w=2u\nM3 d g 0 0 p l=2u w=2u\n.endif\n.ends\n";
    let models = "* t\n.model n nmos level=54\n.model p pmos level=54\n";
    let x4 = "X1 d g mid\nX2 d g mid v=2\nX3 d g mid\nX4 d g mid v=2\n";
    let mid = ".subckt mid d g v=1\nX1 d g leaf v=v\nX2 d g leaf\n.ends\n";
    let branch_model = ".subckt inner d g\nM1 d g 0 0 mm l=1u w=1u\n.ends\n\
         .subckt host d g k=1\n.if (k==1)\n.model mm nmos level=54\n.else\n.model mm pmos level=54\n.endif\nX1 d g inner\nX2 d g inner\n.ends\n\
         X1 d g host\nX2 d g host k=2\nX3 d g host\n";
    let branch_subckt = ".subckt host d g k=1\n.if (k==1)\n.subckt inner d g\nM1 d g 0 0 n l=1u w=1u\n.ends\n.else\n.subckt inner d g\nM1 d g 0 0 p l=1u w=1u\nM2 d g 0 0 p l=1u w=1u\n.ends\n.endif\nX1 d g inner\n.ends\n\
         X1 d g host\nX2 d g host k=2\nX3 d g host\n";
    let branch_x = ".subckt host d g k=1\n.if (k==1)\nX1 d g leaf\n.endif\n.ends\nX1 d g host\nX2 d g host k=2\nX3 d g host\n";
    let top_if =
        ".param top=1\n.if (top==1)\n.param v0=2\nX1 d g leaf v=v0\n.else\nX1 d g leaf\n.endif\n";
    let nested = ".subckt host d g k=1 j=1\n.if (k==1)\n.if (j==1)\nM1 d g 0 0 n l=1u w=1u\n.else\nM1 d g 0 0 p l=1u w=1u\n.endif\n.endif\n.ends\nX1 d g host\nX2 d g host j=2\nX3 d g host k=2\nX4 d g host\n";
    let read_above = ".param a=1\n.subckt host d g\n.if (a==1)\nM1 d g 0 0 n l=1u w=1u\n.else\nM1 d g 0 0 p l=1u w=1u\n.endif\n.ends\n.subckt wrap d g a=1\nX1 d g host\n.ends\nX1 d g wrap\nX2 d g wrap a=2\nX3 d g wrap\n";
    for deck in [
        format!("{models}{leaf}{mid}{x4}"),
        format!("{models}{branch_model}"),
        format!("{models}{branch_subckt}"),
        format!("{models}{leaf}{branch_x}"),
        format!("{models}{leaf}{top_if}"),
        format!("{models}{nested}"),
        format!("{models}{read_above}"),
    ] {
        for option in ["", ".option parhier=local\n"] {
            let text = format!("{option}{deck}");
            let (_dir, parsed, files) = parse_text(&[("top.sp", &text)]).await;
            let (ast, _) = files.build(parsed);
            let mut expected: HashMap<String, u64> = HashMap::new();
            for m in ast.mosfet_models() {
                let name = m
                    .selection
                    .as_ref()
                    .map_or_else(|e| e.to_string(), |s| s.model.name.to_string());
                *expected.entry(name).or_default() += 1;
            }
            let usage = ast.model_usage();
            let mut got: HashMap<String, u64> = HashMap::new();
            for (m, n) in &usage.models {
                *got.entry(m.name.to_string()).or_default() += n;
            }
            for (e, n, _) in &usage.errors {
                *got.entry(e.to_string()).or_default() += n;
            }
            assert_eq!(got, expected, "{text}");
        }
    }
}

/// A deep nest of blocks is one error, not a stack overflow; a nest within the
/// limit runs
#[tokio::test]
async fn if_nesting_limit() {
    let nest = |n: usize| {
        format!(
            "{IF_N}{}{IF_M1}{}",
            ".if (1)\n".repeat(n),
            ".endif\n".repeat(n)
        )
    };
    let (has_err, rows) = if_deck(&nest(200)).await;
    assert!(!has_err);
    assert_eq!(if_paths(&rows), ["M1"]);
    let (has_err, _) = if_deck(&nest(100_000)).await;
    assert!(has_err);
}

/// The text of a block survives printing the netlist
#[tokio::test]
async fn if_blocks_are_printed() {
    let text = format!("{IF_N}.if (1)\n{IF_M1}.elseif (2)\n{IF_M2}.else\n{IF_M3}.endif\n");
    let (_dir, parsed, files) = parse_deck(&[("deck.sp", &text)]).await;
    let (ast, _) = files.build(parsed);
    let printed = ast.to_string().to_lowercase();
    for word in [
        ".if (1)",
        ".elseif (2)",
        ".else",
        ".endif",
        "m1 ",
        "m2 ",
        "m3 ",
    ] {
        assert!(printed.contains(word), "no `{word}` in:\n{printed}");
    }
}

/// Short forms of the commands, and longer words that are other commands
#[tokio::test]
async fn short_forms_of_commands() {
    for (form, want) in [
        (".par k=2u\nM1 d g 0 0 n l=k w=1u\n", 2.0),
        (".parame k=2u\nM1 d g 0 0 n l=k w=1u\n", 2.0),
        (".parameters k=2u\nM1 d g 0 0 n l=k w=1u\n", 2.0),
        (".su s d g\nM1 d g 0 0 n l=2u w=1u\n.ends\nX1 d g s\n", 2.0),
        (
            ".subc s d g\nM1 d g 0 0 n l=2u w=1u\n.ends\nX1 d g s\n",
            2.0,
        ),
        (".mac s d g\nM1 d g 0 0 n l=2u w=1u\n.eom\nX1 d g s\n", 2.0),
        (".mod q nmos level=54\nM1 d g 0 0 q l=2u w=1u\n", 2.0),
        (".mode q nmos level=54\nM1 d g 0 0 q l=2u w=1u\n", 2.0),
        (".els\n", 0.0),
    ] {
        if want == 0.0 {
            continue;
        }
        let text = format!("{IF_N}{form}");
        let (_dir, parsed, files) = parse_deck(&[("deck.sp", &text)]).await;
        let (ast, has_err) = files.build(parsed);
        assert!(!has_err, "{form}");
        let rows = ast.mosfet_models();
        assert_eq!(rows.len(), 1, "{form}");
        let s = rows[0].selection.as_ref().unwrap();
        assert!(close(s.l * 1e6, want), "{form}: L={}", s.l);
    }
    // `.opt` is `.option`
    let text = format!("{IF_N}.opti scale=1e-6\nM1 d g 0 0 n l=2 w=1\n");
    assert_eq!(if_lengths(&text).await, [("M1".to_owned(), 2.0)]);
    // `.in` and `.li` read files
    let (_dir, parsed, files) = parse_deck(&[
        (
            "deck.sp",
            "* t\n.in 'a.inc'\n.li 'b.l' s\nM1 d g 0 0 q l=k w=1u\n",
        ),
        ("a.inc", ".param k=2u\n"),
        ("b.l", ".lib s\n.model q nmos level=54\n.endl s\n"),
    ])
    .await;
    let (ast, has_err) = files.build(parsed);
    assert!(!has_err);
    assert_eq!(chosen(&ast), pairs(&[("M1", "q")]));
    // the keywords of a block
    let text = format!("{IF_N}.if (0)\n{IF_M1}.elsei (0)\n{IF_M2}.els\n{IF_M3}.endi\n");
    assert_eq!(if_paths(&if_deck(&text).await.1), ["M3"]);
    // `.alt` ends the first run
    let text = format!("{IF_N}{IF_M1}.alt\n{IF_M2}");
    assert_eq!(if_paths(&if_deck(&text).await.1), ["M1"]);
    // longer words: unknown commands, which are ignored
    for word in ["paramx", "params", "alters", "alterx"] {
        let text = format!("{IF_N}.{word} k\n{IF_M1}");
        let (has_err, rows) = if_deck(&text).await;
        assert!(!has_err, "{word}");
        assert_eq!(if_paths(&rows), ["M1"], "{word}");
    }
    // the form of `.temp`: the value of `temper`
    for (head, temper) in [
        (".te 75\n", 75.0),
        (".temper 75\n", 75.0),
        (".temperatures 75\n", 75.0),
        (".tempx 75\n", 25.0),
        (".temperature 75\n", 75.0),
        (".temp=75\n", 75.0),
        (".temp '50+25'\n", 75.0),
        (".param t=75\n.temp t\n", 75.0),
        (".temp 50\n.temp 75\n", 50.0),
        (".temp 75 100\n", 75.0),
        // HSPICE splits the values at commas, with or without a space, and a
        // `$` right after the number starts a comment: temper is 50 in the
        // first run of each (measured on P-2019.06-SP1-1)
        (".temp 50,75\n", 50.0),
        (".temp 50, 75\n", 50.0),
        (".temp 50,\n", 50.0),
        (".temp 50$room\n", 50.0),
        (".temp 50 $room\n", 50.0),
        (".option tnom=50\n", 50.0),
        (".option tnom=50\n.option tnom=75\n", 75.0),
        (".option tnom=50\n.temp 75\n", 75.0),
        (".option tnom='50'\n", 25.0),
        (".param tt=50\n.option tnom=tt\n", 25.0),
        (".if (1)\n.temp 75\n.endif\n", 75.0),
        (".if (0)\n.temp 75\n.endif\n", 25.0),
        (".subckt s d g\n.temp 75\n.ends\nX1 d g s\n", 25.0),
    ] {
        let text = format!("{IF_N}{head}M1 d g 0 0 n l='temper*1u' w=1u\n");
        let (_dir, parsed, files) = parse_deck(&[("deck.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let s = ast.mosfet_models().remove(0).selection.unwrap();
        assert!(close(s.l * 1e6, temper), "{head}: L={}", s.l);
    }
}
