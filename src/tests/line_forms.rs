use super::*;

// Forms of a line, as measured on HSPICE (`golden/NOTES.md`)

/// `(path, L, W)` of each row of a deck with no error. The errors of the parse
/// fail the test.
async fn clean_sizes(files: &[(&str, &str)]) -> Vec<(String, f64, f64)> {
    let (_dir, parsed, built) = parse_deck(files).await;
    let (ast, has_err) = built.build(parsed);
    assert!(!has_err, "{files:?}");
    let um = |x: f64| (x * 1e12).round() / 1e6;
    ast.mosfet_models()
        .into_iter()
        .map(|m| match &m.selection {
            Ok(s) => (m.path.clone(), um(s.l), um(s.w)),
            Err(e) => panic!("{}: {e}: {files:?}", m.path),
        })
        .collect()
}

/// HSPICE reads a chain of parameters of any length, in any order, at any level
/// (decks `c_*`, `r_*`, `s_*`, `sr_*`, `f_*`, `i_*`, `params50k`).
#[tokio::test]
async fn long_chains_of_parameters() {
    let chain = |n: usize, reverse: bool| {
        let mut rows = vec![".param p0=1n\n".to_owned()];
        rows.extend((1..=n).map(|k| format!(".param p{k}='p{}+1n'\n", k - 1)));
        if reverse {
            rows.reverse();
        }
        rows.concat()
    };
    for n in [99, 100, 101, 150, 1000, 5000] {
        let size = (n + 1) as f64 * 1e-3;
        let close = |(l, w): (f64, f64)| (l - size).abs() < 1e-9 && w == 1.0;
        let line = format!("M1 d g 0 0 n l=p{n} w=1u\n");
        for reverse in [false, true] {
            let rows = chain(n, reverse);
            let text = format!("{IF_N}{rows}{line}");
            assert!(close(one_size(&text).await), "top, {n}, {reverse}");
            let text = format!("{IF_N}.subckt c d g\n{rows}{line}.ends\nX1 d g c\n");
            assert!(close(one_size(&text).await), "subckt, {n}, {reverse}");
            let text = format!("{IF_N}{rows}.subckt c d g\n{line}.ends\nX1 d g c\n");
            assert!(close(one_size(&text).await), "read below, {n}, {reverse}");
            let text = format!("{IF_N}{rows}.if (p{n}>1n)\n{line}.endif\n");
            assert!(close(one_size(&text).await), ".if, {n}, {reverse}");
        }
    }
    // a loop stays an error, whatever its length
    for n in [2, 150, 1000] {
        let rows = chain(n, false).replace(".param p0=1n", &format!(".param p0='p{n}+1n'"));
        let rows = deck_rows(&format!("{IF_N}{rows}M1 d g 0 0 n l=p{n} w=1u\n")).await;
        assert!(rows[0].1.starts_with("error"), "{n}: {rows:?}");
    }
    // `temper` stops a condition also at the far end of a chain
    for n in [99, 100, 101, 200] {
        let rows = chain(n, false).replace(".param p0=1n\n", ".param p0='temper'\n");
        let text = format!("{IF_N}{rows}.if (p{n}>1n)\nM1 d g 0 0 n l=1u w=1u\n.endif\n");
        let rows = deck_rows(&text).await;
        assert!(
            rows.iter().any(|r| r.1.starts_with("error")),
            "{n}: {rows:?}"
        );
    }
    // a loop or a name with no value at the far end stops all the names before it
    for far in [".param p0='q' q='r' r='q'\n", ".param p0='nothing'\n"] {
        let rows = chain(500, false).replace(".param p0=1n\n", far);
        let rows = deck_rows(&format!("{IF_N}{rows}M1 d g 0 0 n l=p500 w=1u\n")).await;
        assert!(rows[0].1.starts_with("error"), "{far}: {rows:?}");
    }
}

/// `=` can start the `+` line after the name of a `.param` (decks `p_plus_eq*`)
#[tokio::test]
async fn equals_sign_on_a_plus_line_of_param() {
    for cards in [
        ".param q\n+ =1u\n",
        ".param q\n+=1u\n",
        ".param q\n+ =1u r\n+ =2u\n",
    ] {
        let text = format!("{IF_N}{cards}M1 d g 0 0 n l=q w=1u\n");
        assert_eq!(one_size(&text).await, (1.0, 1.0), "{cards}");
    }
}

/// `(` and `)` only separate words on a MOSFET line, also after `key=`. So a space,
/// a comma or a `)` ends the value there, and `(2u+3u)` is one word (decks `g*`, `w*`).
#[tokio::test]
async fn parentheses_after_a_key_on_a_mosfet_line() {
    let cards = format!("{IF_N}.param k=4u j=5u\n");
    for (tail, l, w) in [
        ("l=1u w=(2u + 3u)", 1.0, 2.0),
        ("l=1u w=(2u +3u)", 1.0, 2.0),
        ("l=1u w=( 2u + 3u )", 1.0, 2.0),
        ("l=1u w=(2u 3u)", 1.0, 2.0),
        ("l=1u w=(2u,3u)", 1.0, 2.0),
        ("l=1u w=(k + j)", 1.0, 4.0),
        ("l=1u w=(k j)", 1.0, 4.0),
        ("l=1u w=(k -1u)", 1.0, 4.0),
        ("l=1u w=((k + j))", 1.0, 4.0),
        ("l=1u w=((k)+(j))", 1.0, 4.0),
        ("l=1u w=(k)+(j)", 1.0, 4.0),
        ("l=1u w=(2u)+(3u)", 1.0, 2.0),
        ("l=1u w=(k * 1)", 1.0, 4.0),
        ("l=1u w=(k *1)", 1.0, 4.0),
        ("l=(2u * 3) w=1u", 2.0, 100.0),
        ("l=(k * 3) w=1u", 4.0, 100.0),
        ("l=(2u ** 3) w=1u", 2.0, 100.0),
        ("l=(2u + 3n) w=1u", 2.0, 1.0),
        ("l=(k + 3n) w=1u", 4.0, 1.0),
        ("l=(2u + 3u) (4u)", 2.0, 100.0),
        ("l=(2u + 3u) w=9u", 2.0, 9.0),
        ("l=(2u + 3u) l=9u", 9.0, 100.0),
        ("w=9u l=(2u + 3u) (4u)", 2.0, 9.0),
        ("l=(k)+1u", 4.0, 1.0),
        ("l=(k)+1u w=9u", 4.0, 9.0),
        ("l=(k)-1u w=9u", 4.0, 9.0),
        ("l=(k+j)+(j)", 9.0, 100.0),
        ("l=(k+j)+1u", 9.0, 1.0),
        ("l=((2u)+1u) w=1u", 2.0, 1.0),
        // one word
        ("l=1u w=( 2u+3u )", 1.0, 5.0),
        ("l=1u w=( 2u+3u)", 1.0, 5.0),
        ("l=1u w=(2u+3u )", 1.0, 5.0),
        ("l=1u w=(k+j)", 1.0, 9.0),
        ("l=1u w=( k+j )", 1.0, 9.0),
        ("l=1u w=((k+j))", 1.0, 9.0),
        ("l=1u w=(k*1)", 1.0, 4.0),
        ("l=1u w=( k )", 1.0, 4.0),
        ("l=1u w=( 2u )", 1.0, 2.0),
        // quotes keep the whole text
        ("l='(2u + 3u)' w=1u", 5.0, 1.0),
        ("l=\"(2u + 3u)\" w=1u", 5.0, 1.0),
        // over `+` lines
        ("l=(1u\n+ +2u)", 1.0, 2.0),
        ("l=(1u\n+ 2u)", 1.0, 2.0),
        ("l=(1u\n+ 2u) w=3u", 1.0, 3.0),
        ("w=(1u\n+ 2u) l=3u", 3.0, 2.0),
        ("l=(k\n+ +j)", 4.0, 5.0),
        ("l=(k\n+ j)", 4.0, 5.0),
        ("l=(\n+ 1u)", 1.0, 100.0),
        ("l=(\n+ 1u+2u)", 3.0, 100.0),
        ("l=(1u+2u\n+ )", 3.0, 100.0),
        ("l=(1u+2u\n+ ) w=3u", 3.0, 3.0),
    ] {
        let text = format!("{cards}M1 d g 0 0 n {tail}\n");
        assert_eq!(one_size(&text).await, (l, w), "{tail}");
    }
    // a word that ends in an operator has no value, and neither W below 0
    for tail in [
        "l=1u w=(2u+ 3u)",
        "l=1u w=(k* 1)",
        "l=(k+\n+ j)",
        "l=(k)-1u",
    ] {
        let rows = deck_rows(&format!("{cards}M1 d g 0 0 n {tail}\n")).await;
        assert!(rows[0].1.starts_with("error"), "{tail}: {rows:?}");
    }
}

/// A `+` with no word right after it takes a place and gives no value. In `+2u` it
/// is the sign of the word (decks `w*`).
#[tokio::test]
async fn a_lone_plus_on_a_mosfet_line() {
    let cards = format!("{IF_N}.param k=4u\n");
    for (tail, l, w) in [
        ("1u + 2u", 1.0, 100.0),
        ("1u +(2u)", 1.0, 100.0),
        ("1u + (2u)", 1.0, 100.0),
        ("1u + k", 1.0, 100.0),
        ("1u + 2u 3u", 1.0, 100.0),
        ("1u + w=3u", 1.0, 3.0),
        ("1u + l=3u", 3.0, 100.0),
        ("+ 1u", 100.0, 1.0),
        ("+ 1u 2u", 100.0, 1.0),
        ("+ l=1u w=2u", 1.0, 2.0),
        ("l=1u + 2u", 1.0, 100.0),
        ("l=1u + k", 1.0, 100.0),
        ("w=2u + 3u", 100.0, 2.0),
        ("w=2u + 3u l=1u", 1.0, 2.0),
        ("w=2u + 3u + 4u", 100.0, 2.0),
        ("w=2u + l=3u", 3.0, 2.0),
        ("l=1u w=2u + 3u", 1.0, 2.0),
        ("1u 2u +", 1.0, 2.0),
        ("1u +\n+ 2u", 1.0, 100.0),
        // the sign of a word
        ("1u +2u", 1.0, 2.0),
        ("1u +k", 1.0, 4.0),
        ("+1u", 1.0, 100.0),
        ("l=1u +2u", 1.0, 2.0),
        ("l=1u +k", 1.0, 4.0),
        ("w=2u +3u", 100.0, 3.0),
    ] {
        let text = format!("{cards}M1 d g 0 0 n {tail}\n");
        assert_eq!(one_size(&text).await, (l, w), "{tail}");
    }
}

/// HSPICE assigns the plain numbers of a MOSFET line first and then the
/// expressions, each in the order of the line, and the last one stays. A name, a
/// value in quotes and a word with an operator are expressions. `+2u`, `(2u)` and
/// `2umos` are plain numbers (decks `kx_*`, `ky_*`, `ma_*`, `mb_*`, `mc_*`).
#[tokio::test]
async fn numbers_then_expressions_on_a_mosfet_line() {
    let cards = format!("{IF_N}.model a nmos level=54\n.param k=2u j=3u b=4u\n");
    for (tail, l, w) in [
        ("n 1u 2u w=5u", 1.0, 5.0),
        ("n 1u k w=5u", 1.0, 2.0),
        ("n 1u k*1 w=5u", 1.0, 2.0),
        ("n 1u '2u' w=5u", 1.0, 2.0),
        ("n 1u '2u*1' w=5u", 1.0, 2.0),
        ("n k l=5u", 2.0, 100.0),
        ("n 1u l=5u", 5.0, 100.0),
        ("n '1u' l=5u", 1.0, 100.0),
        ("n k 2u l=5u", 2.0, 2.0),
        ("n l=5u k w=6u", 5.0, 2.0),
        ("n l=5u 1u w=6u", 5.0, 6.0),
        ("n w=5u 1u", 100.0, 1.0),
        ("n w=5u k w=6u", 100.0, 2.0),
        ("n w=5u 1u w=6u", 100.0, 6.0),
        ("n w='5u' 1u 2u", 100.0, 5.0),
        ("n w=j 1u k", 100.0, 3.0),
        ("n w=j k", 100.0, 2.0),
        ("n w=j k 1u", 100.0, 2.0),
        ("n 1u k w=j", 1.0, 3.0),
        ("n 1u 2u w='5u'", 1.0, 5.0),
        ("n k l=j", 3.0, 100.0),
        ("n w=k w=5u", 100.0, 2.0),
        ("n w='2u' w=5u", 100.0, 2.0),
        ("n w=5u w=k", 100.0, 2.0),
        ("n w=j w=k", 100.0, 2.0),
        ("n w=k w=j", 100.0, 3.0),
        ("n l=1u w=(2u) w=5u", 1.0, 5.0),
        ("n l=1u w=(k) w=5u", 1.0, 2.0),
        ("n l=1u w=+2u w=5u", 1.0, 5.0),
        ("n l=1u w=2umos w=5u", 1.0, 5.0),
        ("n l=1u w=+k w=5u", 1.0, 2.0),
        ("n 1u (k) w=5u", 1.0, 2.0),
        ("n 1u (2u) w=5u", 1.0, 5.0),
        ("n 1u +2u w=5u", 1.0, 5.0),
        ("n 1u +k w=5u", 1.0, 2.0),
        ("n 1u 2umos w=5u", 1.0, 5.0),
        ("n 1u 2n7002 w=5u", 1.0, 5.0),
        ("n 1u w=5u 2u", 1.0, 5.0),
        ("n nf=2 k w=5u", 100.0, 2.0),
        ("n nf=2 1u k", 100.0, 1.0),
        ("n k j 'k' w=7u l=7u", 2.0, 3.0),
        // the words between the model and the first key
        ("a 1u b l=5u", 5.0, 4.0),
        ("a k b l=5u", 2.0, 4.0),
        ("a k w=5u 3u", 2.0, 5.0),
        ("a k l=5u 3u", 2.0, 100.0),
        ("a 1u j w=5u", 1.0, 3.0),
        ("a 1u 2u j w=5u", 1.0, 5.0),
        ("a b j w=5u l=5u", 4.0, 3.0),
        ("a b 2u l=5u w=6u", 4.0, 6.0),
        ("a k w=j 2u", 2.0, 3.0),
        ("a l=5u k w=6u", 5.0, 2.0),
    ] {
        let text = format!("{cards}M1 d g 0 0 {tail}\n");
        assert_eq!(
            clean_sizes(&[("deck.sp", &text)]).await,
            [("M1".to_owned(), l, w)],
            "{tail}"
        );
    }
    // `.option wl`: the first place is W
    for (tail, l, w) in [
        ("k 1u w=5u", 1.0, 2.0),
        ("1u k l=5u", 2.0, 1.0),
        ("1u 2u l=5u w=6u", 5.0, 6.0),
        ("w=5u 1u", 1.0, 5.0),
        ("l=5u 1u", 1.0, 100.0),
        ("l=5u k", 2.0, 100.0),
        ("w=5u k", 2.0, 5.0),
    ] {
        let text = format!("{cards}.option wl\nM1 d g 0 0 n {tail}\n");
        assert_eq!(
            clean_sizes(&[("deck.sp", &text)]).await,
            [("M1".to_owned(), l, w)],
            "wl {tail}"
        );
    }
}

/// A word that starts with a number is an expression only with an operator right
/// after the number. HSPICE reads any other such word as the number (decks `gv_*`,
/// `gx_*`, `hx_*`).
#[tokio::test]
async fn words_that_start_with_a_number() {
    let cards = format!("{IF_N}.param k=2u\n");
    for (tail, l, w) in [
        ("1u 2u*1", 1.0, 2.0),
        ("1u*1 2u", 1.0, 2.0),
        ("1u*1 w=2u", 1.0, 2.0),
        ("1e-6*2 1u", 2.0, 1.0),
        ("1u 2*k", 1.0, 4.0),
        ("1u 2u/2", 1.0, 1.0),
        ("1u 4u/2+1u", 1.0, 3.0),
        ("1u 1e-6+1u", 1.0, 2.0),
        ("1u 2u-1u", 1.0, 1.0),
        ("1u -2u*-1", 1.0, 2.0),
        ("1u +2u*1", 1.0, 2.0),
        ("1u 2umos*1", 1.0, 2.0),
        ("1u 2u*1mos", 1.0, 0.002),
        ("1u 2u**1", 1.0, 2.0),
        ("1u .5u*2", 1.0, 1.0),
        ("1u 2.u*1", 1.0, 2.0),
        ("1u 2u*1 $ comment", 1.0, 2.0),
        ("1u\n+ 2u*1", 1.0, 2.0),
        ("1u 2n7002", 1.0, 0.002),
        ("1u 2u.5", 1.0, 2.0),
        ("1u 2^2", 1.0, 2e6),
        ("1u 2**2", 1.0, 4e6),
        ("1u 2*3^2", 1.0, 18e6),
        ("1u 2<3", 1.0, 2e6),
        ("1u 2>3", 1.0, 2e6),
        ("1u 2||3", 1.0, 2e6),
        ("1u 2u%", 1.0, 2.0),
        ("1u 2u#", 1.0, 2.0),
        ("1u 2u@3", 1.0, 2.0),
        ("1u 2u~3", 1.0, 2.0),
        ("1u 2u:3", 1.0, 2.0),
        ("1u 2u_3", 1.0, 2.0),
        ("1u 2u[3]", 1.0, 2.0),
        ("1u 2u|3", 1.0, 2.0),
        ("l=1u w=2^2", 1.0, 2e6),
        ("l=1u w=2**2", 1.0, 4e6),
        ("l=1u w=2*3^2", 1.0, 18e6),
        ("l=1u w=2u.5", 1.0, 2.0),
        ("l=1u w=2umos", 1.0, 2.0),
        ("l=1u w='2^2'", 1.0, 4e6),
    ] {
        let text = format!("{cards}M1 d g 0 0 n {tail}\n");
        assert_eq!(
            clean_sizes(&[("deck.sp", &text)]).await,
            [("M1".to_owned(), l, w)],
            "{tail}"
        );
    }
    // the model at the 4th place
    for tail in ["1u 2u*1", "1u*1 2u", "1u (2u*1)"] {
        let text = format!("{cards}M1 d g 0 n {tail}\n");
        assert_eq!(
            clean_sizes(&[("deck.sp", &text)]).await,
            [("M1".to_owned(), 1.0, 2.0)],
            "{tail}"
        );
    }
}

/// `(` and `)` between the values of a MOSFET line only separate them (decks
/// `gv_*`, `gx_*`, `hx_*`).
#[tokio::test]
async fn parentheses_between_mosfet_values() {
    let cards = format!("{IF_N}.param k=2u\n");
    for (tail, l, w) in [
        ("1u (2u*1)", 1.0, 2.0),
        ("l=1u (k*1)", 1.0, 2.0),
        ("l=1u (2u)", 1.0, 2.0),
        ("l=1u ( 2u*1 )", 1.0, 2.0),
        ("w=1u (k*1)", 100.0, 2.0),
        ("w=5u (1u)", 100.0, 1.0),
        ("1u 2u (1p*1)", 1.0, 2.0),
        ("l=1u w=2u (1p*1)", 1.0, 2.0),
        ("l=1u (2u + 3u)", 1.0, 2.0),
        ("l=1u (2u 3u)", 1.0, 2.0),
        ("1u ((2u*1))", 1.0, 2.0),
        ("1u (2u*1) (3p*1)", 1.0, 2.0),
        ("l=1u w=2u (3p*1) (4p*1)", 1.0, 2.0),
        ("1u (2u*1)$c", 1.0, 2.0),
        ("1u 2u*1,3p*1", 1.0, 2.0),
        ("1u,(2u*1)", 1.0, 2.0),
        ("1u\n+ (2u*1)", 1.0, 2.0),
        ("l=1u\n+ (2u*1)", 1.0, 2.0),
        ("1u (2u*1) w=5u", 1.0, 2.0),
        ("l=1u (2u*1) w=5u", 1.0, 2.0),
    ] {
        let text = format!("{cards}M1 d g 0 0 n {tail}\n");
        assert_eq!(
            clean_sizes(&[("deck.sp", &text)]).await,
            [("M1".to_owned(), l, w)],
            "{tail}"
        );
    }
}

/// A printed MOSFET line keeps the order of its values, so that it reads the same.
#[tokio::test]
async fn a_printed_mosfet_line_reads_the_same() {
    let cards = format!("{IF_N}.param k=2u\n");
    for tail in [
        "w=5u 1u",
        "w=5u k w=6u",
        "1u w=5u 2u",
        "off w=5u",
        "w=5u off 1u",
        "1u k w=5u",
        "1u + 2u",
        "w=(2u + 3u)",
        "l=5u 1u w=6u",
    ] {
        let text = format!("{cards}M1 d g 0 0 n {tail}\n");
        let rows = clean_sizes(&[("deck.sp", &text)]).await;
        let (_dir, parsed, built) = parse_deck(&[("deck.sp", &text)]).await;
        let (ast, _) = built.build(parsed);
        let printed = format!("* title\n{ast}\n");
        assert_eq!(
            clean_sizes(&[("deck.sp", &printed)]).await,
            rows,
            "{tail}: {printed}"
        );
    }
}

/// Each word after the model name takes a place, a `key=value` too. A later
/// value wins over an earlier one of the same name (decks `mx_*`,
/// `m_off_*`, `m_one_key_after_value`).
#[tokio::test]
async fn words_and_keys_mix_on_a_mosfet_line() {
    for (tail, l, w) in [
        ("l=1u w=2u", 1.0, 2.0),
        ("1u 2u", 1.0, 2.0),
        ("1u w=2u", 1.0, 2.0),
        ("w=2u 1u", 100.0, 1.0),
        ("w=2u 1u 3u", 100.0, 1.0),
        ("l=1u 2u", 1.0, 2.0),
        ("1u w=2u 3u", 1.0, 2.0),
        ("1u l=2u 3u", 2.0, 100.0),
        ("w=2u k", 100.0, 7.0),
        ("w=2u off 1u", 100.0, 2.0),
        ("1u w=2u off", 1.0, 2.0),
        ("l=1u w=2u off", 1.0, 2.0),
        ("l=1u off w=2u", 1.0, 2.0),
        ("off w=2u", 100.0, 2.0),
        ("OFF l=1u OFF w=2u", 1.0, 2.0),
        ("off off 1u", 100.0, 100.0),
        ("l=1u w=2u ic=1,1,1", 1.0, 2.0),
        ("l=1u w=2u off ic=1,1,1", 1.0, 2.0),
        ("l=1u ic=1,1,1 w=2u off", 1.0, 2.0),
        ("'1u' w=2u", 1.0, 2.0),
        ("'1u' '2u'", 1.0, 2.0),
        ("l=1u '2u'", 1.0, 2.0),
    ] {
        let text = format!("{IF_N}.param k=7u\nM1 d g 0 0 n {tail}\n");
        let rows = clean_sizes(&[("deck.sp", &text)]).await;
        assert_eq!(rows, [("M1".to_owned(), l, w)], "{tail}");
        // the same over `+` lines
        let text = format!(
            "{IF_N}.param k=7u\nM1 d g 0 0 n\n{}\n",
            tail.split(' ')
                .map(|word| format!("+ {word}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let rows = clean_sizes(&[("deck.sp", &text)]).await;
        assert_eq!(rows, [("M1".to_owned(), l, w)], "{tail}");
    }
    // `=` on the next line
    let text = format!("{IF_N}M1 d g 0 0 n l=\n+ 1u w\n+ =2u\n");
    let rows = clean_sizes(&[("deck.sp", &text)]).await;
    assert_eq!(rows, [("M1".to_owned(), 1.0, 2.0)]);
    // what is printed reads the same
    for tail in ["w=2u 1u", "1u w=2u 3u", "w=2u off 1u", "l=1u off w=2u"] {
        let text = format!("{IF_N}M1 d g 0 0 n {tail}\n");
        let (_dir, parsed, files) = parse_deck(&[("deck.sp", &text)]).await;
        let (ast, _) = files.build(parsed);
        let printed = format!("{ast}");
        let again = clean_sizes(&[("deck.sp", &format!("* again\n{printed}\n"))]).await;
        let first = clean_sizes(&[("deck.sp", &text)]).await;
        assert_eq!(again, first, "{tail}: {printed}");
    }
}

/// `$` in a word that starts with a letter is a character of the name. After a
/// value, it starts a comment (decks `cm_dollar_*`, `m_name_special`,
/// `p_dollar_in_name`).
#[tokio::test]
async fn dollar_in_a_word() {
    for (text, want) in [
        ("M$1 d g 0 0 n l=1u w=1u\n", ("M$1", 1.0, 1.0)),
        ("M1 d$a g$b 0 0 n l=1u w=1u\n", ("M1", 1.0, 1.0)),
        (
            ".model q$a nmos level=54\nM1 d g 0 0 q$a l=1u w=1u\n",
            ("M1", 1.0, 1.0),
        ),
        (
            ".subckt s$a d g\nM1 d g 0 0 n l=1u w=1u\n.ends s$a\nX$1 d g s$a\n",
            ("X$1.M1", 1.0, 1.0),
        ),
        (".param a$b=2u\nM1 d g 0 0 n l=a$b w=1u\n", ("M1", 2.0, 1.0)),
        (
            ".param a$b=2u\nM1 d g 0 0 n l='a$b*2' w=1u\n",
            ("M1", 4.0, 1.0),
        ),
        (
            ".param a$b=2u\nM1 d g 0 0 n l=a$b*2 w=1u\n",
            ("M1", 4.0, 1.0),
        ),
        (".param a$=3u\nM1 d g 0 0 n l=a$ w=1u\n", ("M1", 3.0, 1.0)),
        (
            ".param a$b=2u c='a$b+1u'\nM1 d g 0 0 n l=c w=1u\n",
            ("M1", 3.0, 1.0),
        ),
        // after a value: a comment
        ("M1 d g 0 0 n l=1u$ w=9u\n", ("M1", 1.0, 100.0)),
        ("M1 d g 0 0 n l='1u'$ w=9u\n", ("M1", 1.0, 100.0)),
        ("M1 d g 0 0 n l=1u w=2u$X=3 $Y=4\n", ("M1", 1.0, 2.0)),
        ("M1 d g 0 0 n l=1u w=2u $ w=9u\n", ("M1", 1.0, 2.0)),
        (
            ".param a=1u$ b=2u\nM1 d g 0 0 n l=a w=1u\n",
            ("M1", 1.0, 1.0),
        ),
        // the other characters of a name
        (
            ".param _a=1u a_b=2u a.b=4u\nM1 d g 0 0 n l=_a w=a_b\n",
            ("M1", 1.0, 2.0),
        ),
        (
            ".param _a=1u a_b=2u a.b=4u\nM1 d g 0 0 n l=a.b w='a.b*2'\n",
            ("M1", 4.0, 8.0),
        ),
    ] {
        let rows = clean_sizes(&[("deck.sp", &format!("{IF_N}{text}"))]).await;
        assert_eq!(rows, [(want.0.to_owned(), want.1, want.2)], "{text}");
    }
    // a name read after the comment is not defined
    let text = format!("{IF_N}.param a=1u$ b=2u\nM1 d g 0 0 n l=b w=1u\n");
    assert!(if_deck(&text).await.1[0].1.starts_with("error"), "{text}");
}

/// A word `//` starts a comment on every kind of line (decks `cm_slash_*`,
/// `o_slash_*`)
#[tokio::test]
async fn slash_comments() {
    for (text, want) in [
        ("M1 d g 0 0 n l=1u w=2u // w=9u\n", ("M1", 1.0, 2.0)),
        ("M1 d g 0 0 n l=1u // w=9u\n", ("M1", 1.0, 100.0)),
        (
            "// a comment line\nM1 d g 0 0 n l=1u w=2u\n",
            ("M1", 1.0, 2.0),
        ),
        ("M1 d g 0 0 n l=1u w=2u //c\n", ("M1", 1.0, 2.0)),
        (
            ".param k=2u // k=9u\nM1 d g 0 0 n l=k w=2u\n",
            ("M1", 2.0, 2.0),
        ),
        (
            ".model q nmos level=54 // note\nM1 d g 0 0 q l=1u w=2u\n",
            ("M1", 1.0, 2.0),
        ),
        (
            ".subckt s d g // note\nM1 d g 0 0 n l=1u w=2u\n.ends // note\nX1 d g s // c\n",
            ("X1.M1", 1.0, 2.0),
        ),
        // a comment line does not end the statement
        ("M1 d g 0 0 n l=1u\n// note\n+ w=2u\n", ("M1", 1.0, 2.0)),
        ("M1 d g 0 0 n l=1u // note\n+ w=2u\n", ("M1", 1.0, 2.0)),
        // not in quotes
        (
            ".param s='a//b' k=2u\nM1 d g 0 0 n l=k w=2u\n",
            ("M1", 2.0, 2.0),
        ),
    ] {
        let rows = clean_sizes(&[("deck.sp", &format!("{IF_N}{text}"))]).await;
        assert_eq!(rows, [(want.0.to_owned(), want.1, want.2)], "{text}");
    }
    // a path keeps its slashes
    let rows = clean_sizes(&[
        (
            "deck.sp",
            &format!("{IF_N}.inc './/a.inc'\nM1 d g 0 0 n l=k w=2u\n"),
        ),
        ("a.inc", ".param k=3u\n"),
    ])
    .await;
    assert_eq!(rows, [("M1".to_owned(), 3.0, 2.0)]);
}

/// A comma separates the items after a value, on every kind of line (decks
/// `pc_*`, `p_commas`)
#[tokio::test]
async fn commas_between_items() {
    for (text, want) in [
        (".param a=1u,b=2u\nM1 d g 0 0 n l=a w=b\n", (1.0, 2.0)),
        (".param a=1u ,b=2u\nM1 d g 0 0 n l=a w=b\n", (1.0, 2.0)),
        (".param a=1u , b=2u\nM1 d g 0 0 n l=a w=b\n", (1.0, 2.0)),
        (".param a=1u,\n+ b=2u\nM1 d g 0 0 n l=a w=b\n", (1.0, 2.0)),
        (".param a='1u',b='2u'\nM1 d g 0 0 n l=a w=b\n", (1.0, 2.0)),
        (".param a=1u,b=2u,c=3u\nM1 d g 0 0 n l=a w=c\n", (1.0, 3.0)),
        (
            ".param a='1u*1',b='2u'\nM1 d g 0 0 n l='a' w='b'\n",
            (1.0, 2.0),
        ),
        ("M1 d g 0 0 n l=1u,w=2u\n", (1.0, 2.0)),
        (".option defl=1u,defw=2u\nM1 d g 0 0 n\n", (1.0, 2.0)),
        (".option defl=1u , defw=2u\nM1 d g 0 0 n\n", (1.0, 2.0)),
        (
            ".model q nmos level=54,lmin=0\nM1 d g 0 0 q l=1u w=2u\n",
            (1.0, 2.0),
        ),
        (
            ".subckt s d g a=1u,b=2u\nM1 d g 0 0 n l=a w=b\n.ends\nX1 d g s\n",
            (1.0, 2.0),
        ),
        (
            ".subckt s d g a=9u b=9u\nM1 d g 0 0 n l=a w=b\n.ends\nX1 d g s a=1u,b=2u\n",
            (1.0, 2.0),
        ),
    ] {
        let rows = clean_sizes(&[("deck.sp", &format!("{IF_N}{text}"))]).await;
        assert_eq!(rows.len(), 1, "{text}");
        assert_eq!((rows[0].1, rows[0].2), want, "{text}");
    }
}

/// A form feed is white space (deck `e_formfeed`). `.lib` goes on on `+` lines
/// (deck `lib_cont`).
#[tokio::test]
async fn form_feed_and_library_lines() {
    for text in [
        "\u{c}\nM1 d g 0 0 n l=1u w=2u\n",
        "M1 d g 0 0 n l=1u w=2u\n\u{c}\n",
        "M1 d g 0 0 n\u{c}l=1u w=2u\n",
    ] {
        let rows = clean_sizes(&[("deck.sp", &format!("{IF_N}{text}"))]).await;
        assert_eq!(rows, [("M1".to_owned(), 1.0, 2.0)], "{text:?}");
    }
    let lib = ".lib a\n.param k=3u\n.endl a\n";
    for head in [
        ".lib\n+ 'x.l'\n+ a\n",
        ".lib 'x.l'\n+ a\n",
        ".lib\n+ 'x.l' a\n",
    ] {
        let deck = format!("{IF_N}{head}M1 d g 0 0 n l=k w=2u\n");
        let rows = clean_sizes(&[("deck.sp", &deck), ("x.l", lib)]).await;
        assert_eq!(rows, [("M1".to_owned(), 3.0, 2.0)], "{head}");
    }
}

/// The short forms of `.data`, and its rows with no `+` (decks `dat_rows_*`)
#[tokio::test]
async fn data_blocks() {
    for (open, close) in [
        (".data", ".enddata"),
        (".da", ".enddata"),
        (".dat", ".enddata"),
        (".data", ".endd"),
        (".data", ".endda"),
        (".DATA", ".ENDDATA"),
    ] {
        let text = format!("{IF_N}{open} dd\n a b\n 1 2\n{close}\n{IF_M1}");
        let rows = clean_sizes(&[("deck.sp", &text)]).await;
        assert_eq!(rows, [("M1".to_owned(), 1.0, 1.0)], "{open} {close}");
    }
}

/// An include is looked for in the folder of the file that includes it, then in
/// the folder of the top file, then in the folders of `.option search`, in the
/// order of the deck (decks `inc_nested_rel_cwd`, `lib_inc_rel_cwd`, `opt_search_*`,
/// `search_option*`)
#[tokio::test]
async fn where_includes_are_found() {
    let q = ".model q nmos level=54\n";
    let m = "M1 d g 0 0 q l=k w=1u\n";
    let k = |n: u32| format!(".param k={n}u\n");
    // the folder of the file wins, then the folder of the top file
    let own = clean_sizes(&[
        ("deck.sp", &format!("* t\n{q}.inc 'sub/a.inc'\n{m}")),
        ("sub/a.inc", ".inc 'b.inc'\n"),
        ("sub/b.inc", &k(1)),
        ("b.inc", &k(2)),
    ])
    .await;
    assert_eq!(own, [("M1".to_owned(), 1.0, 1.0)]);
    let top = clean_sizes(&[
        ("deck.sp", &format!("* t\n{q}.inc 'sub/a.inc'\n{m}")),
        ("sub/a.inc", ".inc 'b.inc'\n"),
        ("b.inc", &k(2)),
    ])
    .await;
    assert_eq!(top, [("M1".to_owned(), 2.0, 1.0)]);
    let lib = clean_sizes(&[
        ("deck.sp", &format!("* t\n{q}.lib 'sub/a.l' s\n{m}")),
        ("sub/a.l", ".lib s\n.inc 'b.inc'\n.endl s\n"),
        ("b.inc", &k(2)),
    ])
    .await;
    assert_eq!(lib, [("M1".to_owned(), 2.0, 1.0)]);
    // `.option search`
    for (head, want) in [
        (".option search='d1'\n.inc 'x.inc'\n", 1.0),
        (".option search=d1\n.inc 'x.inc'\n", 1.0),
        (".option search='./d1/'\n.inc 'x.inc'\n", 1.0),
        (".option search='d1' search='d2'\n.inc 'x.inc'\n", 1.0),
        (".option search='d2' search='d1'\n.inc 'x.inc'\n", 2.0),
        (
            ".option search='d2'\n.option search='d1'\n.inc 'x.inc'\n",
            2.0,
        ),
        (".option search='d9' search='d2'\n.inc 'x.inc'\n", 2.0),
        (".inc 'set.inc'\n.inc 'x.inc'\n", 1.0),
        (
            ".subckt z d g\n.option search='d1'\n.ends\n.inc 'x.inc'\n",
            1.0,
        ),
        (".option search='d1'\n.lib 'y.l' s\n", 3.0),
    ] {
        let rows = clean_sizes(&[
            ("deck.sp", &format!("* t\n{q}{head}{m}")),
            ("set.inc", ".option search='d1'\n"),
            ("d1/x.inc", &k(1)),
            ("d2/x.inc", &k(2)),
            ("d1/y.l", ".lib s\n.param k=3u\n.endl s\n"),
        ])
        .await;
        assert_eq!(rows, [("M1".to_owned(), want, 1.0)], "{head}");
    }
    // the folder of the top file wins over a search folder
    let rows = clean_sizes(&[
        (
            "deck.sp",
            &format!("* t\n{q}.option search='d1'\n.inc 'x.inc'\n{m}"),
        ),
        ("x.inc", &k(5)),
        ("d1/x.inc", &k(1)),
    ])
    .await;
    assert_eq!(rows, [("M1".to_owned(), 5.0, 1.0)]);
    // a sibling in the search folder
    let rows = clean_sizes(&[
        (
            "deck.sp",
            &format!("* t\n{q}.option search='d1'\n.inc 'x.inc'\n{m}"),
        ),
        ("d1/x.inc", ".inc 'sib.inc'\n"),
        ("d1/sib.inc", &k(6)),
    ])
    .await;
    assert_eq!(rows, [("M1".to_owned(), 6.0, 1.0)]);
    // not found anywhere: the error of the include
    let (_dir, parsed, files) = parse_deck(&[(
        "deck.sp",
        &format!("* t\n{q}.option search='d1'\n.inc 'none.inc'\n"),
    )])
    .await;
    let (_, has_err) = files.build(parsed);
    assert!(has_err);
}

/// A size option of 0 or below gives the default back, as the last plain number
/// (decks `zd_*`, `zq_*`).
#[tokio::test]
async fn size_options_of_zero_and_below() {
    let l2 = ".model n.1 nmos level=54 lmin=1e-9 lmax=2u wmin=1e-9 wmax=1\n\
              .model n.2 nmos level=54 lmin=2u lmax=1 wmin=1e-9 wmax=1\n";
    let line = "M1 d g 0 0 n w=1u\n";
    for (head, want) in [
        (".option defl=0\n", "n.2"),
        (".option defl=-1u\n", "n.2"),
        (".option defl=1u\n.option defl=0\n", "n.2"),
        (".option defl=1u defl=-1u\n", "n.2"),
        (".option defl=1u\n", "n.1"),
        (".option defl=1u scale=10\n", "n.2"),
        // the default of 100u, scaled
        (".option defl=0 scale=1e-2\n", "n.1"),
    ] {
        let text = format!("* t\n{head}{l2}{line}");
        assert_eq!(one_model(&text).await, want, "{head}");
    }
    let (l, w) = (100.0, 100.0);
    for head in [".option defl=0 defw=0\n", ".option defl=-1u defw=-1u\n"] {
        let text = format!("{IF_N}{head}M1 d g 0 0 n\n");
        assert_eq!(one_size(&text).await, (l, w), "{head}");
    }
    for head in [
        ".option scale=0\n",
        ".option scale=-1\n",
        ".option scale=2\n.option scale=0\n",
        ".option scale=2 scale=-1\n",
    ] {
        let text = format!("{IF_N}{head}M1 d g 0 0 n l=1u w=1u\n");
        assert_eq!(one_size(&text).await, (1.0, 1.0), "{head}");
    }
    let line = "M1 d g 0 0 n l=1u w=1u\n";
    for head in [".option scalm=0\n", ".option scalm=-1\n"] {
        let text = format!("* t\n{head}{l2}{line}");
        assert_eq!(one_model(&text).await, "n.1", "{head}");
    }
    let bin = ".model n.1 nmos level=54 lmin=1e-3 lmax=2e-3 wmin=1e-9 wmax=1e3\n";
    let text = format!("* t\n.option scalm=1e-3\n{bin}M1 d g 0 0 n l=1.5u w=1u\n");
    assert_eq!(one_model(&text).await, "n.1");
    let text = format!("* t\n.option scalm=1e-3 scalm=0\n{bin}M1 d g 0 0 n l=1.5u w=1u\n");
    assert!(one_model(&text).await.starts_with("error"));
    // a size on the line below 0 still stops
    for text in [
        format!("{IF_N}M1 d g 0 0 n l=-1u w=1u\n"),
        format!("{IF_N}M1 d g 0 0 n l=1u w=-1u\n"),
        format!("{IF_N}.option scale=-1\nM1 d g 0 0 n l=-1u w=1u\n"),
    ] {
        assert!(one_model(&text).await.starts_with("error"), "{text}");
    }
}

/// `hertz` reads 0 in a value, as `time` does, and a condition cannot read it
/// (decks `sv_hertz*`)
#[tokio::test]
async fn hertz_reads_zero() {
    let text = format!("{IF_N}M1 d g 0 0 n l='hertz+1u' w=1u\n");
    assert_eq!(one_size(&text).await, (1.0, 1.0));
    let text = ".model n.1 nmos level=54 lmin='hertz+1e-9' lmax=1 wmin=1e-9 wmax=1\nM1 d g 0 0 n l=1u w=1u\n";
    assert_eq!(one_model(&format!("* t\n{text}")).await, "n.1");
    for head in [".if (hertz>1)\n", ".param h='hertz'\n.if (h>1)\n"] {
        let text = format!("{IF_N}{head}{IF_M1}.else\n{IF_M2}.endif\n");
        let (_, rows) = if_deck(&text).await;
        assert_eq!(rows.len(), 1, "{head}: {rows:?}");
        assert!(rows[0].1.starts_with("error"), "{head}: {rows:?}");
    }
}

/// A level 72 bin with both `nfin*` and `wsheet*` limits reads both. A sheet bin
/// needs a sheet width above 0 on the line. `.option scale` scales WSHEET
/// (decks `h_*`, `hb_*`).
#[tokio::test]
async fn finfet_bins_of_both_kinds() {
    let both = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4 wsheetmin=1e-8 wsheetmax=1e-6\n\
                .model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=4 nfinmax=9 wsheetmin=1e-8 wsheetmax=1e-6\n";
    let other = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4 wsheetmin=1e-8 wsheetmax=1e-7\n\
                 .model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4 wsheetmin=1e-7 wsheetmax=1e-6\n";
    let mix = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6\n\
               .model f.2 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=9\n";
    let fins = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=1 nfinmax=4\n";
    let sheet = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6\n";
    let sheet4 = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=1e-8 wsheetmax=1e-6 nsheetmin=3 nsheetmax=9\n";
    let zero = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wsheetmin=0 wsheetmax=1e-6\n";
    let fin0 = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u nfinmin=0 nfinmax=4\n";
    let six = ".model f.1 nmos level=72 lmin=1e-8 lmax=1u wmin=1e-3 wmax=1 nfinmin=1 nfinmax=4\n";
    let scaled = ".model f.1 nmos level=72 lmin=1e-9 lmax=1e-3 wsheetmin=1e-9 wsheetmax=1e-7\n\
                  .model f.2 nmos level=72 lmin=1e-9 lmax=1e-3 wsheetmin=1e-7 wsheetmax=1e-5\n";
    for (head, bins, tail, want) in [
        ("", both, "l=500n nfin=6 wsheet=2e-8", Some("f.2")),
        ("", both, "l=500n wsheet=2e-8", Some("f.1")),
        ("", both, "l=500n nfin=6", None),
        ("", both, "l=500n", None),
        ("", both, "l=500n w=2e-8", None),
        ("", other, "l=500n nfin=2 wsheet=2e-7", Some("f.2")),
        ("", other, "l=500n wsheet=2e-7", Some("f.2")),
        ("", other, "l=500n nfin=2", None),
        ("", mix, "l=500n nfin=2", Some("f.2")),
        ("", mix, "l=500n wsheet=2e-8", Some("f.1")),
        ("", mix, "l=500n nfin=2 wsheet=2e-8", Some("f.1")),
        ("", fins, "l=500n wsheet=2e-8", Some("f.1")),
        ("", sheet, "l=500n nfin=2 wsheet=2e-8", Some("f.1")),
        ("", sheet4, "l=500n wsheet=2e-8 nsheet=1", Some("f.1")),
        ("", sheet4, "l=500n wsheet=2e-8", Some("f.1")),
        ("", zero, "l=500n", None),
        ("", zero, "l=500n wsheet=0", None),
        ("", fin0, "l=500n", Some("f.1")),
        ("", fin0, "l=500n nfin=0", None),
        ("", six, "l=500n w=1u nfin=2", Some("f.1")),
        (
            ".option scale=10\n",
            scaled,
            "l=500n wsheet=5e-8",
            Some("f.2"),
        ),
    ] {
        let text = format!("* t\n{head}{bins}M1 d g 0 0 f {tail}\n");
        let got = one_model(&text).await;
        match want {
            Some(want) => assert_eq!(got, want, "{head}{bins}{tail}"),
            None => assert!(got.starts_with("error"), "{head}{bins}{tail}: {got}"),
        }
    }
}

/// The values after `ic=` belong to it: they take no place of L or W (decks
/// `lt_ic_*`)
#[tokio::test]
async fn values_after_ic() {
    for (tail, l, w) in [
        ("ic=1,1,1", 100.0, 100.0),
        ("ic=1,1,1 l=1u w=3u", 1.0, 3.0),
        ("l=1u ic=1,1,1 3u", 1.0, 100.0),
        ("ic=1,1,1 1u 3u", 100.0, 100.0),
        ("ic=1,2 1u 3u", 100.0, 100.0),
        ("ic=1 1u 3u", 100.0, 100.0),
        ("ic=1, 1, 1 l=1u", 1.0, 100.0),
        ("l=1u ad=1p,3u", 1.0, 100.0),
    ] {
        let text = format!("{IF_N}M1 d g 0 0 n {tail}\n");
        assert_eq!(one_size(&text).await, (l, w), "{tail}");
    }
}

/// A `,` can end a `.param` or `.option` statement, and `.inc` goes on on a `+`
/// line (decks `cc_*`, `in_over_plus`)
#[tokio::test]
async fn ends_of_statements() {
    let text = format!("{IF_N}.param a=1u,\nM1 d g 0 0 n l=a w=1u\n");
    assert_eq!(one_size(&text).await, (1.0, 1.0));
    let text = format!("{IF_N}.option defl=1u,\nM1 d g 0 0 n w=1u\n");
    assert_eq!(one_size(&text).await, (1.0, 1.0));
    let rows = clean_sizes(&[
        (
            "deck.sp",
            &format!("{IF_N}.inc\n+ 'a.inc'\nM1 d g 0 0 n l=k w=1u\n"),
        ),
        ("a.inc", ".param k=1u\n"),
    ])
    .await;
    assert_eq!(rows, [("M1".to_owned(), 1.0, 1.0)]);
}

/// A `.lib <name>` line in a section is read over, and the first `.endl` ends the
/// section (decks `ln_*`)
#[tokio::test]
async fn section_lines_in_a_section() {
    for (library, call, l) in [
        (".lib a\n.lib b\n.param k=1u\n.endl b\n.endl a\n", "a", 1.0),
        (".lib a\n.lib b\n.param k=1u\n.endl b\n.endl a\n", "b", 1.0),
        (
            ".lib a\n.param k=1u\n.lib b\n.param k=2u\n.endl b\n.endl a\n",
            "a",
            2.0,
        ),
        (
            ".lib a\n.lib 'x.l' b\n.lib b\n.param k=2u\n.endl b\n.endl a\n",
            "a",
            2.0,
        ),
        (
            ".lib a\n.lib b\n.lib c\n.param k=1u\n.endl c\n.endl b\n.endl a\n",
            "a",
            1.0,
        ),
        (
            ".lib a\n.param k=2u\n.lib b\n.lib c\n.param k=1u\n.endl c\n.endl b\n.endl a\n",
            "c",
            1.0,
        ),
        // the first `.endl` ends the section
        (
            ".lib a\n.lib b\n.param k=1u\n.endl b\n.param k=3u\n.endl a\n",
            "a",
            1.0,
        ),
        (
            ".lib a\n.param k=1u\n.endl zzz\n.param k=5u\n.lib c\n.endl c\n",
            "a",
            1.0,
        ),
        (".lib a\n.param k=1u\n.endl a\n.endl a\n", "a", 1.0),
        (
            ".LIB B $ note\n.param k=1u\n.LIB A $ note\n.ENDL\n",
            "b",
            1.0,
        ),
    ] {
        let rows = clean_sizes(&[
            (
                "deck.sp",
                &format!("{IF_N}.lib 'x.l' {call}\nM1 d g 0 0 n l=k w=1u\n"),
            ),
            ("x.l", library),
        ])
        .await;
        assert_eq!(rows, [("M1".to_owned(), l, 1.0)], "{library}");
    }
}

/// A deck of `depth` subckt levels. `s0` holds the MOSFET line `leaf`, and each `sK`
/// holds one instance of the level below it. `above` comes before the definitions, and
/// `below` after them, at the top level.
fn tower(depth: usize, above: &str, leaf: &str, below: &str) -> String {
    let mut deck = format!(".model n nmos level=54\n{above}\n.subckt s0 d g\n{leaf}\n.ends\n");
    for k in 1..depth {
        writeln!(deck, ".subckt s{k} d g\nX0 d g s{}\n.ends", k - 1).unwrap();
    }
    writeln!(deck, "{below}\nX0 d g s{}", depth - 1).unwrap();
    deck
}

/// The far reads of both walks, which must agree, and the L of the first MOSFET
async fn far_reads(deck: &str) -> (Vec<(String, usize)>, f64) {
    let (_dir, parsed, files) = parse_text(&[("top.sp", deck)]).await;
    let (ast, _) = files.build(parsed);
    let mut l = None;
    let listed = ast.for_each_mosfet_model(|m| {
        l.get_or_insert(m.selection.as_ref().unwrap().l);
    });
    assert_eq!(listed, ast.model_usage().far_reads, "{deck}");
    let rows = listed.into_iter().map(|f| (f.name, f.levels)).collect();
    (rows, l.unwrap())
}

#[tokio::test]
async fn warn_of_a_read_beyond_the_reach_of_hspice() {
    let u = || vec![("u".to_owned(), 32)];
    let read = "M0 d g 0 0 n l=u w=1u";
    for mode in ["", ".option parhier=local\n"] {
        // HSPICE reads a name 31 levels above the reader, and stops at 32 (`*_read_*`)
        let above = format!("{mode}.param u=7n");
        assert_eq!(
            far_reads(&tower(31, &above, read, "")).await,
            (vec![], 7e-9),
            "{mode}"
        );
        assert_eq!(
            far_reads(&tower(32, &above, read, "")).await,
            (u(), 7e-9),
            "{mode}"
        );
    }
    // a function has the same reach (`g_fun_*`)
    let (rows, _) = far_reads(&tower(
        32,
        ".param f(x)='x*2'",
        "M0 d g 0 0 n l='f(3n)' w=1u",
        "",
    ))
    .await;
    assert_eq!(rows, [("f".to_owned(), 32)]);
}

#[tokio::test]
async fn warn_of_a_definition_that_hspice_cannot_reach() {
    // under `global`, HSPICE takes a nearer value from 32 levels on, with no message:
    // L=5n, and L=100n here (`g_top_*`). `local` takes the near value in both tools.
    let deck = |depth: usize, mode: &str| {
        let mut deck = format!("{mode}.model n nmos level=54\n.param v=100n\n");
        deck += ".subckt s0 d g v=1n\nM0 d g 0 0 n l=v w=1u\n.ends\n";
        for k in 1..depth {
            writeln!(deck, ".subckt s{k} d g v=1n\nX0 d g s{} v=5n\n.ends", k - 1).unwrap();
        }
        writeln!(deck, "X0 d g s{} v=5n", depth - 1).unwrap();
        deck
    };
    assert_eq!(far_reads(&deck(31, "")).await, (vec![], 100e-9));
    assert_eq!(
        far_reads(&deck(32, "")).await,
        (vec![("v".to_owned(), 32)], 100e-9)
    );
    let local = ".option parhier=local\n";
    assert_eq!(far_reads(&deck(32, local)).await, (vec![], 5e-9));
    assert_eq!(far_reads(&deck(96, local)).await, (vec![], 5e-9));

    // the reach is from the reader, and not from the top (`*_mid_*`)
    let read = "M0 d g 0 0 n l=u w=1u";
    let mid = |depth: usize| {
        tower(depth, "", read, "").replace(
            &format!(".subckt s{} d g\n", depth - 1),
            &format!(".subckt s{} d g\n.param u=7n\n", depth - 1),
        )
    };
    assert_eq!(far_reads(&mid(32)).await.0, []);
    assert_eq!(far_reads(&mid(33)).await.0, [("u".to_owned(), 32)]);
}

#[tokio::test]
async fn warn_of_a_far_read_after_the_same_instance_near_the_top() {
    // the near instance comes first, and it gives the same counts: the far one must
    // still be evaluated in the summary
    let read = "M0 d g 0 0 n l=u w=1u";
    let near = tower(32, ".param u=7n", read, "X9 d g s0");
    assert_eq!(far_reads(&near).await.0, [("u".to_owned(), 32)]);
    let middle = tower(32, ".param u=7n", read, "X9 d g s5");
    assert_eq!(far_reads(&middle).await.0, [("u".to_owned(), 32)]);
    // the summary of many far instances stays a walk of each level once
    let mut wide = tower(32, ".param u=7n", read, "");
    for k in 1..32 {
        wide = wide.replace(
            &format!("X0 d g s{}\n", k - 1),
            &format!("X0 d g s{0}\nX1 d g s{0}\n", k - 1),
        );
    }
    let (_dir, parsed, files) = parse_text(&[("top.sp", &wide)]).await;
    let (ast, _) = files.build(parsed);
    let usage = ast.model_usage();
    assert_eq!(usage.mosfets, 1 << 31);
    assert_eq!(usage.far_reads[0].levels, 32);
}

#[tokio::test]
async fn warn_of_a_far_read_of_its_own_name() {
    // `local`: a text that reads its own name reads it at the level above
    let deck = tower(
        33,
        ".option parhier=local\n.param v=1n",
        ".param v='v+1n'\nM0 d g 0 0 n l=v w=1u",
        "",
    );
    let (rows, l) = far_reads(&deck).await;
    assert_eq!(rows, [("v".to_owned(), 33)]);
    assert!((l - 2e-9).abs() < 1e-15, "{l}");
}

#[tokio::test]
async fn warn_of_a_far_read_in_a_subckt_alone() {
    let deck = tower(32, ".param u=7n", "M0 d g 0 0 n l=u w=1u", "");
    let (_dir, parsed, files) = parse_text(&[("top.sp", &deck)]).await;
    let (ast, _) = files.build(parsed);
    // an instance of `s31` at the top level is the same tower
    let top = ast.model_usage_of("s31").unwrap();
    assert_eq!(top.far_reads[0].levels, 32);
    let listed = ast.for_each_mosfet_model_of("s31", |_| {}).unwrap();
    assert_eq!(listed, top.far_reads);
    assert!(ast.for_each_mosfet_model_of("nope", |_| {}).is_none());
    assert!(ast.model_usage_of("s30").unwrap().far_reads.is_empty());
}
