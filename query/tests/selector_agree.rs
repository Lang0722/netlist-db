//! The query tool and the model selector (`AST::mosfet_models`) read the
//! same MOSFET lines in the same way: on each deck below, every MOSFET has
//! the same model, L and W in both, and `resolve_model` chooses the card
//! (the bin) that the selector chooses, or none where the selector gives an
//! error of the same kind. The decks hold no `.if`, because the query tool
//! reads all branches of an `.if` block.

mod common;

use common::{load, write_deck};
use netlist_db::{FileId, models::SelectError, parser::parse_top_multi};
use netlist_query::Page;
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

/// The decks with binned models, for [`card_agrees_with_selector`], as in
/// [`DECKS`]. Each MOSFET gets the card that the selector chooses, or an
/// error of the selector's kind.
const BIN_DECKS: &[(&str, &str)] = &[
    (
        "bins_top",
        ".model nch.1 nmos level=54 lmin=1e-8 lmax=1e-6 wmin=1e-8 wmax=1e-6\n\
         .model nch.2 nmos level=54 lmin=1e-6 lmax=1e-3 wmin=1e-8 wmax=1e-6\n\
         .model nch.3 nmos level=54 lmin=1e-8 lmax=1e-6 wmin=1e-6 wmax=1e-3\n\
         .model nch.4 nmos level=54 lmin=1e-6 lmax=1e-3 wmin=1e-6 wmax=1e-3\n\
         M1 d g 0 0 nch l=0.5u w=0.5u\nM2 d g 0 0 nch l=5u w=0.5u\n\
         M3 d g 0 0 nch l=0.5u w=5u\nM4 d g 0 0 nch l=5u w=5u\nM5 d g 0 0 nch l=1u w=1u\n\
         M6 d g 0 0 nch l=5m w=1u\nM7 d g 0 0 nch 0.5u 5u\n",
    ),
    (
        "bins_nf",
        ".model nch.1 nmos level=54 lmin=1e-8 lmax=1e-6 wmin=1e-8 wmax=1e-6\n\
         .model nch.3 nmos level=54 lmin=1e-8 lmax=1e-6 wmin=1e-6 wmax=1e-3\n\
         M1 d g 0 0 nch l=0.5u w=4u nf=8\nM2 d g 0 0 nch l=0.5u w=4u nf=8 wnflag=0\n\
         M3 d g 0 0 nch l=0.5u w=4u nf=0.5\nM4 d g 0 0 nch l=0.5u w=4u nf=0\n",
    ),
    (
        "bins_option_wnflag",
        ".option wnflag=0\n\
         .model nch.1 nmos level=54 lmin=1e-8 lmax=1e-6 wmin=1e-8 wmax=1e-6\n\
         .model nch.3 nmos level=54 lmin=1e-8 lmax=1e-6 wmin=1e-6 wmax=1e-3\n\
         M1 d g 0 0 nch l=0.5u w=4u nf=8\nM2 d g 0 0 nch l=0.5u w=4u nf=8 wnflag=1\n",
    ),
    (
        "bins_scale",
        ".option scale=1e-6 scalm=1e-6\n\
         .model nch.1 nmos level=54 lmin=0.01 lmax=1 wmin=0.01 wmax=1\n\
         .model nch.2 nmos level=54 lmin=1 lmax=1000 wmin=0.01 wmax=1\n\
         M1 d g 0 0 nch l=0.5 w=0.5\nM2 d g 0 0 nch l=5 w=0.5\n",
    ),
    (
        "bins_defl",
        ".option defl=2u defw=0.5u\n\
         .model nch.1 nmos level=54 lmin=1e-8 lmax=1e-6 wmin=1e-8 wmax=1e-6\n\
         .model nch.2 nmos level=54 lmin=1e-6 lmax=1e-3 wmin=1e-8 wmax=1e-6\n\
         M1 d g 0 0 nch\nM2 d g 0 0 nch l=0.5u\n",
    ),
    (
        "bins_mac_local",
        ".option parhier=local\n.subckt mac d g s lb=1e-6\n\
         .model lo.1 nmos level=54 lmin=1e-8 lmax='lb' wmin=1e-8 wmax=1\n\
         .model lo.2 nmos level=54 lmin='lb' lmax=1e-3 wmin=1e-8 wmax=1\n\
         M1 d g s s lo l=5e-7 w=1u\n.ends mac\nX1 d g 0 mac\nX2 d g 0 mac lb=1e-7\n",
    ),
    (
        "bins_mac_global",
        ".option parhier=global\n.subckt mac d g s lb=1e-6\n\
         .model lo.1 nmos level=54 lmin=1e-8 lmax='lb' wmin=1e-8 wmax=1\n\
         .model lo.2 nmos level=54 lmin='lb' lmax=1e-3 wmin=1e-8 wmax=1\n\
         M1 d g s s lo l=5e-7 w=1u\n.ends mac\nX1 d g 0 mac\nX2 d g 0 mac lb=1e-7\n",
    ),
    (
        "bins_mac_global_top",
        ".param lb=3e-7\n.subckt mac d g s lb=1e-6\n\
         .model lo.1 nmos level=54 lmin=1e-8 lmax='lb' wmin=1e-8 wmax=1\n\
         .model lo.2 nmos level=54 lmin='lb' lmax=1e-3 wmin=1e-8 wmax=1\n\
         M1 d g s s lo l=5e-7 w=1u\n.ends mac\nX1 d g 0 mac\nX2 d g 0 mac lb=1e-7\n",
    ),
    (
        "bins_nested_local",
        ".option parhier=local\n.subckt outer d g s lb=1e-6\n\
         .model lo.1 nmos level=54 lmin=1e-8 lmax='lb' wmin=1e-8 wmax=1\n\
         .model lo.2 nmos level=54 lmin='lb' lmax=1e-3 wmin=1e-8 wmax=1\n\
         .subckt inner d g s lb=1e-7\nM1 d g s s lo l=5e-7 w=1u\n.ends inner\n\
         X1 d g s inner\n.ends outer\nX1 d g 0 outer\n",
    ),
    (
        "bins_nested_global",
        ".option parhier=global\n.subckt outer d g s lb=1e-6\n\
         .model lo.1 nmos level=54 lmin=1e-8 lmax='lb' wmin=1e-8 wmax=1\n\
         .model lo.2 nmos level=54 lmin='lb' lmax=1e-3 wmin=1e-8 wmax=1\n\
         .subckt inner d g s lb=1e-7\nM1 d g s s lo l=5e-7 w=1u\n.ends inner\n\
         X1 d g s inner\n.ends outer\nX1 d g 0 outer\n",
    ),
    (
        "bins_top_limits_local",
        ".option parhier=local\n.param lb=2e-7\n\
         .model tg.1 nmos level=54 lmin=1e-8 lmax='lb' wmin=1e-8 wmax=1\n\
         .model tg.2 nmos level=54 lmin='lb' lmax=1e-3 wmin=1e-8 wmax=1\n\
         .subckt s1 d g s lb=1e-6\nM1 d g s s tg l=5e-7 w=1u\n.ends s1\n\
         .subckt s2 d g s lb=1e-7\nM1 d g s s tg l=1.5e-7 w=1u\n.ends s2\n\
         X1 d g 0 s1\nX2 d g 0 s2\n",
    ),
    (
        "bins_local_hide",
        ".model loc.1 nmos level=54 lmin=0 lmax=1e-6 wmin=0 wmax=1\n\
         .model loc.2 nmos level=54 lmin=1e-6 lmax=1 wmin=0 wmax=1\n\
         .subckt hide d g s\n.model loc.1 nmos level=54 lmin=0 lmax=5e-7 wmin=0 wmax=1\n\
         M1 d g s s loc l=8e-7 w=1u\nM2 d g s s loc l=3e-7 w=1u\nM3 d g s s loc l=2u w=1u\n\
         .ends hide\nX1 d g 0 hide\n",
    ),
    (
        "bins_dynamic",
        ".model dyn.2 nmos level=54 lmin=0 lmax=1 wmin=0 wmax=1\n\
         .subckt leaf d g s\nM1 d g s s dyn l=1u w=1u\n.ends leaf\n\
         .subckt mid d g s\n.model dyn.1 nmos level=54 lmin=0 lmax=1 wmin=0 wmax=1\n\
         X1 d g s leaf\n.ends mid\nX1 d g 0 mid\nX2 d g 0 leaf\n",
    ),
    (
        "bins_finfet",
        ".model f.1 nmos level=72 lmin=1e-8 lmax=1e-6 nfinmin=1 nfinmax=3\n\
         .model f.2 nmos level=72 lmin=1e-8 lmax=1e-6 nfinmin=3 nfinmax=10\n\
         .model s.1 nmos level=72 lmin=1e-8 lmax=1e-6 wsheetmin=1e-8 wsheetmax=1e-6\n\
         .model k.1 nmos level=72 lmin=1e-8 lmax=1e-6 wmin=1e-8 wmax=1e-6 nfinmin=1 nfinmax=9\n\
         M1 d g 0 0 f l=0.1u nfin=2\nM2 d g 0 0 f l=0.1u nfin=5\nM3 d g 0 0 f l=0.1u\n\
         M4 d g 0 0 f l=0.1u nfin=20\nM5 d g 0 0 s l=0.1u wsheet=0.5u\nM6 d g 0 0 s l=0.1u\n\
         M7 d g 0 0 k l=0.1u w=5u nfin=2\n",
    ),
    (
        "bins_missing_limits",
        ".model m.1 nmos level=54 lmin=0 lmax=1\n\
         .model m.2 nmos level=54 lmin=0 lmax=1 wmin=0 wmax=1\n\
         .model q.1 nmos level=54 lmin=0 lmax=1 wmin=0 wmax=1\n\
         M1 d g 0 0 m l=1u w=1u\nM2 d g 0 0 q l=1u w=1u\n",
    ),
    (
        "bins_limit_error",
        ".model e.1 nmos level=54 lmin=0 lmax='nosuch*2' wmin=0 wmax=1\n\
         .model e.2 nmos level=54 lmin=0 lmax=1 wmin=0 wmax=1\n\
         .model g.1 nmos level=54 lmin=0 lmax=1e-7 wmin=0 wmax=1\n\
         .model g.2 nmos level=54 lmin=1e-7 lmax='nosuch' wmin=0 wmax=1\n\
         M1 d g 0 0 e l=1u w=1u\nM2 d g 0 0 g l=5e-8 w=1u\nM3 d g 0 0 g l=1u w=1u\n",
    ),
    (
        "bins_edge",
        ".model t.1 nmos level=54 lmin=1e-8 lmax=3e-7 wmin=0 wmax=1\n\
         .model t.2 nmos level=54 lmin=3e-7 lmax=1e-6 wmin=0 wmax=1\n\
         M1 d g 0 0 t l='0.1u*3' w=1u\nM2 d g 0 0 t l=3e-7 w=1u\nM3 d g 0 0 t l=2.9999999e-7 w=1u\n",
    ),
    (
        "bins_wl",
        ".option wl\n\
         .model nch.1 nmos level=54 lmin=1e-8 lmax=1e-6 wmin=1e-8 wmax=1e-6\n\
         .model nch.3 nmos level=54 lmin=1e-8 lmax=1e-6 wmin=1e-6 wmax=1e-3\n\
         M1 d g 0 0 nch 5u 0.5u\n",
    ),
    (
        "bins_exact_and_types",
        ".model nch nmos level=54\n\
         .model nch.1 nmos level=54 lmin=0 lmax=1 wmin=0 wmax=1\n\
         .model pch.1 pmos level=54 lmin=0 lmax=1e-6 wmin=0 wmax=1\n\
         .model pch.2 d\n.model dio.1 d\n.model n2 d\n\
         M1 d g 0 0 nch l=1u w=1u\nM2 d g 0 0 pch l=0.5u w=1u\nM3 d g 0 0 pch l=5u w=1u\n\
         M4 d g 0 0 dio l=1u w=1u\nM5 d g 0 0 nosuch l=1u w=1u\n",
    ),
    (
        "bins_bad_size",
        ".model nch.1 nmos level=54 lmin=0 lmax=1 wmin=0 wmax=1\n\
         M1 d g 0 0 nch l=0 w=1u\nM2 d g 0 0 nch l=1u w=-1u\nM3 d g 0 0 nch l=1u w=1u m=0\n\
         M4 d g 0 0 nch l=1u w=1u nf=-1\nM5 d g 0 0 nch l='nosuch' w=1u\n",
    ),
    (
        "bins_pdk_macro",
        ".param dvth_g=0.01\n.subckt nch_mac d g s b l=100n w=1u nf=1\n\
         .param lx='l*1' wx='w/nf'\n\
         .model nch.1 nmos level=54 lmin=20n lmax=200n wmin=0.1u wmax=5u vth0='0.35+dvth_g'\n\
         .model nch.2 nmos level=54 lmin=200n lmax=2u wmin=0.1u wmax=5u vth0='0.4+dvth_g'\n\
         M0 d g s b nch l=lx w=w nf=nf\n.ends nch_mac\n\
         .subckt amp in out\nX1 out in 0 0 nch_mac l=60n w=2u nf=2\n\
         X2 out in 0 0 nch_mac l=500n w=1u\nX3 out in 0 0 nch_mac l=5u w=1u\n.ends amp\n\
         Xa in out amp\n",
    ),
];

/// The model, L and W of each MOSFET from the selector, by instance path.
fn selector(path: &Path) -> Vec<(String, String, f64, f64)> {
    selector_rows(path)
        .into_iter()
        .map(|(at, row)| {
            let (model, l, w) = row.unwrap_or_else(|e| panic!("{at}: {e}"));
            (at, model, l, w)
        })
        .collect()
}

/// The model, L and W that the selector chose for a MOSFET, or the kind of
/// its error (see [`kind`]).
type Row = Result<(String, f64, f64), &'static str>;

/// The [`Row`] of each MOSFET from the selector, by instance path. The rows
/// that stand for no MOSFET are left out.
fn selector_rows(path: &Path) -> Vec<(String, Row)> {
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
        .filter(|m| {
            m.selection
                .as_ref()
                .err()
                .is_none_or(SelectError::is_mosfet)
        })
        .map(|m| {
            let row = match &m.selection {
                Ok(s) => Ok((s.model.name.to_ascii_lowercase(), s.l, s.w)),
                Err(e) => Err(kind(e)),
            };
            (m.path.to_ascii_lowercase(), row)
        })
        .collect()
}

/// The kind of a selector error, as `resolve_model` words the reason.
fn kind(e: &SelectError) -> &'static str {
    match e {
        SelectError::NoModel(_) => "no model",
        SelectError::NoBin { .. } => "no bin",
        SelectError::MissingLimits(_) => "missing limits",
        SelectError::Eval { .. } => "no value",
        SelectError::BadSize { .. } => "bad size",
        _ => "other",
    }
}

/// The kind of a `resolve_model` reason (see [`kind`]).
fn query_kind(why: &str) -> &'static str {
    if why.starts_with("no NMOS or PMOS card") || why.contains("cannot name a bin") {
        "no model"
    } else if why.starts_with("none of the") {
        "no bin"
    } else if why.contains("must set LMIN, LMAX") {
        "missing limits"
    } else if why.contains("HSPICE stops at this size") {
        "bad size"
    } else if why.contains(" of model `") || why.split_once(": ").is_some() {
        "no value"
    } else {
        "other"
    }
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

#[test]
fn card_agrees_with_selector() {
    let mut failures = Vec::new();
    let mut counts = (0, 0);
    for (name, body) in DECKS.iter().chain(BIN_DECKS) {
        let text = format!("* t\n{body}V1 d 0 1\n.end\n");
        let mut files = vec![("main.sp", text.as_str())];
        files.extend(
            INCLUDED
                .iter()
                .filter(|(deck, ..)| deck == name)
                .map(|&(_, file, text)| (file, text)),
        );
        // (a name of its own: the other test writes the decks of `DECKS`
        // at the same time)
        let deck = write_deck(&format!("card_{name}"), &files);
        let net = load(&deck);
        let rows = selector_rows(&deck);
        assert!(!rows.is_empty(), "{name}: no MOSFET");
        for (at, row) in rows {
            let got = net
                .resolve_model(&at, &[], &Page::default())
                .unwrap_or_else(|e| panic!("{name} {at}: {e}"));
            let card = got.card.as_ref().map(|c| c.name.to_ascii_lowercase());
            let sel = got.selection.as_ref().expect("a MOSFET has a selection");
            let ok = match &row {
                Ok((model, l, w)) => {
                    counts.0 += 1;
                    card.as_deref() == Some(model.as_str())
                        && sel.l.is_some_and(|v| close(v, *l))
                        && sel.w.is_some_and(|v| close(v, *w))
                }
                Err(k) => {
                    counts.1 += 1;
                    card.is_none() && got.unresolved.as_deref().map(query_kind) == Some(*k)
                }
            };
            if !ok {
                failures.push(format!(
                    "{name} {at}: selector {row:?}, query card {card:?} L={:?} W={:?} unresolved {:?}",
                    sel.l, sel.w, got.unresolved
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // the decks reach both outcomes
    assert!(counts.0 > 40 && counts.1 > 10, "{counts:?}");
}
