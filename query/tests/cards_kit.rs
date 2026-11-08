//! The HSPICE kit of the card values (`scripts/hspice_kit/cards/kit`) holds
//! netlist-db's answers to its probes, which an HSPICE run is compared with.
//! This test checks that they are still `resolve_model`'s answers: after a
//! change of the query, make the kit again (see
//! `scripts/hspice_kit/cards/README.md`).

mod common;

use common::{load, repo};
use netlist_query::Page;
use serde_json::Value;

const STALE: &str = "the answers of the card kit are not resolve_model's: run `cargo build --release -p netlist-query --example resolve_model && python3 scripts/hspice_kit/cards/make_cards_kit.py scripts/hspice_kit/cards/kit`";

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * a.abs().max(b.abs())
}

#[test]
fn kit_answers_are_current() {
    let kit = repo("scripts/hspice_kit/cards/kit");
    let text = std::fs::read_to_string(kit.join("expected.json")).unwrap();
    let expected: Value = serde_json::from_str(&text).unwrap();
    let decks = expected["decks"].as_array().unwrap();
    assert!(decks.len() >= 20, "{} decks", decks.len());
    let mut probes = 0;
    let mut failures = Vec::new();
    for deck in decks {
        let name = deck["deck"].as_str().unwrap();
        let net = load(kit.join("decks").join(format!("{name}.sp")));
        for p in deck["probes"].as_array().unwrap() {
            probes += 1;
            let path = p["path"].as_str().unwrap();
            let param = p["param"].as_str().unwrap();
            let want = &p["netlist_db"];
            let page = Page {
                limit: 500,
                cursor: None,
            };
            let got = match net.resolve_model(path, &[param], &page) {
                Ok(got) => got,
                Err(e) => {
                    if want["error"].as_str() != Some(e.to_string().as_str()) {
                        failures.push(format!("{name} {path}: error {e}, kit {want}"));
                    }
                    continue;
                }
            };
            let card = got.card.as_ref().map(|c| c.name.as_str());
            let scope = got.card.as_ref().map(|c| c.scope.as_str());
            let values: Vec<Option<f64>> = got
                .params
                .items
                .iter()
                .filter(|i| i.name.eq_ignore_ascii_case(param))
                .map(|i| i.value)
                .collect();
            let kit_values: Vec<Option<f64>> = want["values"]
                .as_array()
                .map(|v| v.iter().map(Value::as_f64).collect())
                .unwrap_or_default();
            let same_values = values.len() == kit_values.len()
                && values.iter().zip(&kit_values).all(|(a, b)| match (a, b) {
                    (Some(a), Some(b)) => close(*a, *b),
                    (None, None) => true,
                    _ => false,
                });
            if card != want["card"].as_str() || scope != want["scope"].as_str() || !same_values {
                failures.push(format!(
                    "{name} {path}: card {card:?} scope {scope:?} values {values:?}, kit {want}"
                ));
            }
        }
    }
    assert!(probes >= 35, "{probes} probes");
    assert!(failures.is_empty(), "{STALE}\n{}", failures.join("\n"));
}
