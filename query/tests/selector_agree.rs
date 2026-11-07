//! The query tool and the model selector (`AST::mosfet_models`) read the
//! same MOSFET lines in the same way: on each deck below, every MOSFET has
//! the same model, L and W in both. The decks hold no `.if`, because the
//! query tool reads all branches of an `.if` block.

mod common;

use common::{load, write_deck};
use netlist_db::{FileId, parser::parse_top_multi};
use std::path::Path;

/// The decks, by name. `* t` comes before each one, and `V1 d 0 1` and
/// `.end` after it.
const DECKS: &[(&str, &str)] = &[
    (
        "model_word",
        ".param lv=1u wv=2u adp=1p asp=2p\n.model nch nmos level=54\n\
         M1 d g s b nch lv wv\nM2 d g s b nch lv 'wv*2'\nM3 d g s b nch 'lv' 'wv'\n\
         M4 d g s b nch 1u wv\nM5 d g s b nch lv 2u\nM6 d g s b nch lv wv adp asp\n",
    ),
    (
        "compare_near",
        ".model n nmos level=54\n.param fin=0.016u\n.subckt mac d g tw=1u\n\
         .param rv='(tw==0.048u)?1u:2u'\n.param low='(tw<=0.048u)?1u:2u'\n\
         M0 d g 0 0 n l=rv w=low\n.ends\nX1 d g mac tw='fin*3'\n",
    ),
    (
        "compare_product",
        ".model n nmos level=54\n.param wf=0.12u nf=3 wt='nf*wf'\n\
         M1 d g 0 0 n l='wt>=0.36u?1u:2u' w='wt==0.36u?1u:2u'\n",
    ),
    (
        "passed_loop",
        ".option parhier=local\n.model n nmos level=54\n.param k=2 c=1u\n\
         .subckt s d g a=1u\n.param c='a*3'\nM1 d g 0 0 n l=c w=a\n.ends\nX1 d g s a='c*k'\n",
    ),
    (
        "passed_loop_other",
        ".option parhier=local\n.model n nmos level=54\n.param a=1u k=2\n\
         .subckt c d g s b\n.param q='a*3'\nM1 d g s b n l=q w=a\n.ends\n\
         x1 d g s b c a='q*k'\n.param q=5u\n",
    ),
    (
        "passed_cross",
        ".option parhier=local\n.model n nmos level=54\n.param a=1u b=1u\n\
         .subckt s d g a=5u b=5u\nM1 d g 0 0 n l=a w=b\nM2 d g 0 0 n l=b w=a\n.ends\n\
         X1 d g s a='b*2' b='a*3'\n",
    ),
    (
        "passed_cross_swapped",
        ".option parhier=local\n.model n nmos level=54\n.param a=1u b=1u\n\
         .subckt s d g a=5u b=5u\nM1 d g 0 0 n l=a w=b\nM2 d g 0 0 n l=b w=a\n.ends\n\
         X1 d g s b='a*3' a='b*2'\n",
    ),
    (
        "body_self",
        ".option parhier=local\n.model n nmos level=54\n.param w=2u\n\
         .subckt c d g s b\n.param w='w*3'\nM1 d g s b n l=1u w=w\n.ends\nx1 d g s b c\n",
    ),
    (
        "default_self",
        ".option parhier=local\n.model n nmos level=54\n.param w=2u\n\
         .subckt c d g s b w='w*3'\nM1 d g s b n l=1u w=w\n.ends\nx1 d g s b c\n",
    ),
    (
        "self_two_levels",
        ".option parhier=local\n.model n nmos level=54\n.subckt leaf d g s b\n\
         .param w='w*3'\nM1 d g s b n l=1u w=w\n.ends\n.subckt mid d g s b\n.param w=1u\n\
         x1 d g s b leaf\n.ends\nx1 d g s b mid\n",
    ),
    (
        "option_wl",
        ".model n nmos level=54\nM1 d g 0 0 n 1u 2u\nM2 d g 0 0 n l=1u w=2u\n\
         .subckt z a\n.option wl\n.ends\n",
    ),
    (
        "option_wl_alter",
        ".model n nmos level=54\nM1 d g 0 0 n 1u 2u\n.op\n.alter\n.option wl\n",
    ),
    (
        "option_wl_alter_include",
        ".model n nmos level=54\nM1 d g 0 0 n 1u 2u\n.op\n.alter\n.include 'wl.sp'\n",
    ),
    (
        "option_wl_after_include",
        ".include 'wl.sp'\n.option wl=0\n.model n nmos level=54\nM1 d g 0 0 n 1u 2u\n",
    ),
    (
        "option_wl_before_include",
        ".include 'wl0.sp'\n.option wl\n.model n nmos level=54\nM1 d g 0 0 n 1u 2u\n",
    ),
    (
        "model_after_alter",
        ".param lp=1u wp=2u\n.model n nmos level=54\nM1 d g 0 0 x n lp wp\n.op\n.alter\n\
         .include 'x.sp'\n",
    ),
    (
        "parhier_in_subckt",
        ".param k=1u\n.model n nmos level=54\n.subckt z d g\n.option parhier=local\n.ends\n\
         .subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n",
    ),
    (
        "parhier_bare",
        ".option parhier\n.param k=1u\n.model n nmos level=54\n\
         .subckt s d g k=2u\nM1 d g 0 0 n l=k w=1u\n.ends\nX1 d g s\n",
    ),
    (
        "builtins",
        ".model n nmos level=54\n\
         .param a='(ln(2)+trunc(2.7)+round(2.5)+fmod(5,3)+atan2(1,1)+hypot(3,4))*1u'\n\
         .param b='(asinh(1)+acosh(2)+atanh(0.5))*1u' c=limit(1u,0.1u)\n\
         M1 d g 0 0 n l=a w=b\nM2 d g 0 0 n l=c w=1u\n",
    ),
    (
        "tnom",
        ".option tnom=50\n.model n nmos level=54\nM1 d g 0 0 n l='(1+temper/100)*1u' w=1u\n",
    ),
    (
        "tnom_in_subckt",
        ".subckt z a\n.option tnom=50\n.ends\n.model n nmos level=54\n\
         M1 d g 0 0 n l='temper*1u' w=1u\n",
    ),
    (
        "temp_spelled",
        ".temperature 75\n.model n nmos level=54\nM1 d g 0 0 n l='(1+temper/100)*1u' w=1u\n",
    ),
    (
        "temp_comma",
        ".temp 50,75\n.model n nmos level=54\nM1 d g 0 0 n l='temper*1u' w=1u\n",
    ),
    (
        "temp_dollar",
        ".temp 50$room\n.model n nmos level=54\nM1 d g 0 0 n l='temper*1u' w=1u\n",
    ),
    (
        "temp_name_comma",
        ".param tp=50\n.temp tp,75\n.model n nmos level=54\nM1 d g 0 0 n l='temper*1u' w=1u\n",
    ),
    (
        "temp_quoted_comma",
        ".temp '50',75\n.model n nmos level=54\nM1 d g 0 0 n l='temper*1u' w=1u\n",
    ),
    (
        "temp_equals_comma",
        ".temp =50,75\n.model n nmos level=54\nM1 d g 0 0 n l='temper*1u' w=1u\n",
    ),
    (
        "stat_shadow",
        ".model n nmos level=54\n.param unif(a,b)='(a+b)/2'\nM1 d g 0 0 n l='unif(1u,3u)' w=1u\n",
    ),
    (
        "three_nodes",
        ".model n nmos level=54\n.param lp=1u\nM1 d g 0 n lp w=3u\n",
    ),
    (
        "time_hertz",
        ".model n nmos level=54\n.param b='time+1u' h='hertz+2u'\nM2 d g 0 0 n l=b w=h\n",
    ),
];

/// The files that the decks above include: (deck, file, text).
const INCLUDED: &[(&str, &str, &str)] = &[
    ("option_wl_alter_include", "wl.sp", ".option wl\n"),
    ("option_wl_after_include", "wl.sp", ".option wl\n"),
    ("option_wl_before_include", "wl0.sp", ".option wl=0\n"),
    ("model_after_alter", "x.sp", ".model x nmos level=54\n"),
];

/// The model, L and W of each MOSFET from the selector, by instance path.
fn selector(path: &Path) -> Vec<(String, String, f64, f64)> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (parsed, files) = rt.block_on(parse_top_multi(std::iter::once(FileId::Include {
        path: path.to_path_buf(),
    })));
    let (ast, _) = files.build(parsed);
    ast.mosfet_models()
        .into_iter()
        .map(|m| {
            let s = m
                .selection
                .as_ref()
                .unwrap_or_else(|e| panic!("{}: {e:?}", m.path));
            (
                m.path.to_ascii_lowercase(),
                s.model.name.to_ascii_lowercase(),
                s.l,
                s.w,
            )
        })
        .collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * a.abs().max(b.abs())
}

#[test]
fn query_agrees_with_selector() {
    let mut failures = Vec::new();
    for (name, body) in DECKS {
        let text = format!("* t\n{body}V1 d 0 1\n.end\n");
        let mut files = vec![("main.sp", text.as_str())];
        files.extend(
            INCLUDED
                .iter()
                .filter(|(deck, ..)| deck == name)
                .map(|&(_, file, text)| (file, text)),
        );
        let deck = write_deck(name, &files);
        let net = load(&deck);
        let mosfets = selector(&deck);
        assert!(!mosfets.is_empty(), "{name}: no MOSFET");
        for (at, model, l, w) in mosfets {
            let info = net.element_info(&at).unwrap();
            let got = net.resolve_element(&at).unwrap();
            let value = |key: &str| {
                got.params
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(key))
                    .and_then(|p| p.value)
            };
            let (ql, qw) = (value("l"), value("w"));
            let qm = info.model.as_deref().map(str::to_ascii_lowercase);
            if qm.as_deref() != Some(model.as_str())
                || !ql.is_some_and(|v| close(v, l))
                || !qw.is_some_and(|v| close(v, w))
            {
                failures.push(format!(
                    "{name} {at}: selector {model} L={l} W={w}, query {qm:?} L={ql:?} W={qw:?}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
