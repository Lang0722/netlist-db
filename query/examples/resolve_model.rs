// cargo run --release -p netlist-query --example resolve_model -- DECK PATH[=NAME,NAME...] ...
//
// Print the answer of `Netlist::resolve_model` for each element path of the
// deck DECK: the `.model` card that the element uses (for a MOSFET, its bin)
// and the card's parameters, evaluated where the card is defined. One JSON
// object per line, in the order of the paths: `{"path": ..., "answer": ...}`,
// or `{"path": ..., "error": ...}` when the query fails. `PATH=vth0,u0` asks for
// those parameters only. `--lib FILE:FROM:TO` loads the section TO of the
// `.lib` file named FILE where the deck loads FROM (another corner).
//
// The HSPICE kit of the card values (`scripts/hspice_kit/cards`) uses it.
use netlist_query::{LibOverride, LoadOptions, Netlist, Page};
use std::process::exit;

fn usage() -> ! {
    eprintln!("usage: resolve_model [--lib FILE:FROM:TO] DECK PATH[=NAME,NAME...] ...");
    exit(2)
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut lib_override = Vec::new();
    while let Some(i) = args.iter().position(|a| a == "--lib") {
        if i + 1 >= args.len() {
            usage();
        }
        let spec = args.remove(i + 1);
        args.remove(i);
        let parts: Vec<&str> = spec.splitn(3, ':').collect();
        let [file, from, to] = parts[..] else {
            usage();
        };
        lib_override.push(LibOverride {
            file: file.into(),
            from: from.to_owned(),
            to: to.to_owned(),
        });
    }
    if args.len() < 2 {
        usage();
    }
    let deck = args.remove(0);
    let net = match Netlist::load(&LoadOptions {
        lib_override,
        ..LoadOptions::new(&deck)
    }) {
        Ok(net) => net,
        Err(e) => {
            eprintln!("{deck}: {e}");
            exit(1)
        }
    };
    let page = Page {
        limit: netlist_query::MAX_LIMIT,
        cursor: None,
    };
    for spec in &args {
        let (path, names) = spec.split_once('=').unwrap_or((spec, ""));
        let names: Vec<&str> = names.split(',').filter(|n| !n.is_empty()).collect();
        let line = match net.resolve_model(path, &names, &page) {
            Ok(answer) => serde_json::json!({"path": path, "answer": answer}),
            Err(e) => serde_json::json!({"path": path, "error": e.to_string()}),
        };
        println!("{line}");
    }
}
