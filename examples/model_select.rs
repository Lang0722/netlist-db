// cargo run --release --example model_select -- [--subckt NAME] deck.sp [more files...]
//
// Print the model HSPICE uses for every MOSFET of a netlist: one line per
// transistor, with its instance path, the model name on its line, its L and
// W (with every parameter resolved), and the model chosen (the bin `name.N`
// of a binned model library), or why none could be chosen.
//
// The MOSFETs are those of the circuit under the top level; with
// `--subckt NAME`, those of an instance of the subckt NAME with its default
// parameters. A netlist without instances at the top level (a library) is
// shown subckt by subckt.
use netlist_db::{FileId, models::MosfetModel, parser::parse_top_multi};
use std::{env, path::PathBuf};

fn print(prefix: &str, mosfets: &[MosfetModel<'_, '_>]) {
    for m in mosfets {
        match &m.selection {
            Ok(s) => println!(
                "{prefix}{}\t{}\t{:e}\t{:e}\t{}\t{}",
                m.path, m.mname, s.l, s.w, s.nf, s.model.name
            ),
            Err(e) => println!("{prefix}{}\t{}\t\t\t\terror: {e}", m.path, m.mname),
        }
    }
}

#[tokio::main]
async fn main() {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let subckt = args.iter().position(|a| a == "--subckt").map(|i| {
        args.remove(i);
        args.remove(i)
    });
    let (parsed, files) = parse_top_multi(args.iter().map(|path| FileId::Include {
        path: PathBuf::from(path),
    }))
    .await;
    let (ast, has_err) = files.build(parsed);
    if has_err {
        eprintln!("(the netlist has errors; install a logger to see them)");
    }
    println!("path\tmodel name\tL\tW\tNF\tmodel");
    let mut count = (0, 0);
    let mut tally = |mosfets: &[MosfetModel<'_, '_>]| {
        count.0 += mosfets.len();
        count.1 += mosfets.iter().filter(|m| m.selection.is_ok()).count();
    };
    if let Some(name) = subckt {
        let Some(mosfets) = ast.mosfet_models_of(&name) else {
            eprintln!("no subckt `{name}` at the top level");
            std::process::exit(1);
        };
        print("", &mosfets);
        tally(&mosfets);
    } else if ast.instance.is_empty() {
        // a library: each subckt, with its default parameters
        for (key, def) in &ast.subckt {
            let mosfets = ast.mosfet_models_of(key).unwrap_or_default();
            print(&format!("{}:", def.name), &mosfets);
            tally(&mosfets);
        }
    } else {
        let mosfets = ast.mosfet_models();
        print("", &mosfets);
        tally(&mosfets);
    }
    eprintln!("{} MOSFET(s), a model chosen for {}", count.0, count.1);
}
