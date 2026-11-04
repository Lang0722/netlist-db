// cargo run --release --example model_select -- [--summary] [--subckt NAME] deck.sp [more files...]
//
// Print the model HSPICE uses for every MOSFET of a netlist: one line per
// transistor, with its instance path, the model name on its line, its L and
// W (with every parameter resolved), and the model chosen (the bin `name.N`
// of a binned model library), or why none could be chosen. The lines are
// written as the transistors are found: the listing needs no memory per
// transistor.
//
// With `--summary`, print instead how many MOSFETs use each model, counted
// through the hierarchy without listing them (for a netlist whose hierarchy
// holds millions of transistors).
//
// The MOSFETs are those of the circuit under the top level; with
// `--subckt NAME`, those of an instance of the subckt NAME with its default
// parameters. A netlist without instances at the top level (a library) is
// shown subckt by subckt.
use netlist_db::{
    AST, FileId,
    models::{ModelUsage, MosfetModel, Short},
    parser::parse_top_multi,
};
use std::{
    env,
    io::{BufWriter, Write, stdout},
    path::PathBuf,
    time::Instant,
};

fn print(out: &mut impl Write, prefix: &str, m: &MosfetModel<'_, '_>) -> std::io::Result<()> {
    match &m.selection {
        Ok(s) => writeln!(
            out,
            "{prefix}{}\t{}\t{}\t{}\t{}\t{}",
            m.path,
            m.mname,
            Short(s.l),
            Short(s.w),
            s.nf,
            s.model.name
        ),
        Err(e) => writeln!(out, "{prefix}{}\t{}\t\t\t\terror: {e}", m.path, m.mname),
    }
}

fn print_usage(
    out: &mut impl Write,
    prefix: &str,
    usage: &ModelUsage<'_, '_>,
) -> std::io::Result<()> {
    let total = usage.mosfets.max(1) as f64;
    for (model, n) in &usage.models {
        writeln!(
            out,
            "{prefix}{}\t{n}\t{:.3}%",
            model.name,
            100.0 * *n as f64 / total
        )?;
    }
    for (e, n, path) in &usage.errors {
        writeln!(
            out,
            "{prefix}error: {e}\t{n}\t{:.3}%\te.g. {path}",
            100.0 * *n as f64 / total
        )?;
    }
    writeln!(out, "{prefix}total\t{}", usage.mosfets)
}

/// The MOSFETs of `ast`, or of its subckt `name`: listed (as they are
/// found, not kept), or counted
fn report(out: &mut impl Write, ast: &AST<'_>, subckt: Option<&str>, summary: bool) -> (u64, u64) {
    let no_subckt = || -> ! {
        eprintln!(
            "no subckt `{}` at the top level",
            subckt.unwrap_or_default()
        );
        std::process::exit(1);
    };
    let prefix = subckt.map(|s| format!("{s}:")).unwrap_or_default();
    if summary {
        let usage = match subckt {
            Some(name) => ast.model_usage_of(name).unwrap_or_else(|| no_subckt()),
            None => ast.model_usage(),
        };
        print_usage(out, &prefix, &usage).expect("write");
        let chosen = usage.models.iter().map(|(_, n)| n).sum();
        (usage.mosfets, chosen)
    } else {
        let (mut mosfets, mut chosen) = (0, 0);
        let mut each = |m: MosfetModel<'_, '_>| {
            mosfets += 1;
            chosen += u64::from(m.selection.is_ok());
            print(out, &prefix, &m).expect("write");
        };
        match subckt {
            Some(name) => {
                if !ast.for_each_mosfet_model_of(name, &mut each) {
                    no_subckt();
                }
            }
            None => ast.for_each_mosfet_model(&mut each),
        }
        (mosfets, chosen)
    }
}

#[tokio::main]
async fn main() {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let mut flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .map(|i| args.remove(i))
            .is_some()
    };
    let summary = flag("--summary");
    let subckt = args.iter().position(|a| a == "--subckt").map(|i| {
        args.remove(i);
        args.remove(i)
    });
    let start = Instant::now();
    let (parsed, files) = parse_top_multi(args.iter().map(|path| FileId::Include {
        path: PathBuf::from(path),
    }))
    .await;
    let (ast, has_err) = files.build(parsed);
    let parsed_in = start.elapsed();
    if has_err {
        eprintln!("(the netlist has errors; install a logger to see them)");
    }
    let mut out = BufWriter::new(stdout().lock());
    if summary {
        writeln!(out, "model\tMOSFETs\tshare").expect("write");
    } else {
        writeln!(out, "path\tmodel name\tL\tW\tNF\tmodel").expect("write");
    }
    let start = Instant::now();
    let (mut mosfets, mut chosen) = (0, 0);
    if subckt.is_none() && ast.instance.is_empty() {
        // a library: each subckt, with its default parameters
        for def in ast.subckt.values() {
            let (n, c) = report(&mut out, &ast, Some(&def.name), summary);
            mosfets += n;
            chosen += c;
        }
    } else {
        (mosfets, chosen) = report(&mut out, &ast, subckt.as_deref(), summary);
    }
    out.flush().expect("write");
    eprintln!(
        "{mosfets} MOSFET(s), a model chosen for {chosen}; parsed in {:.2} s, models chosen in {:.2} s",
        parsed_in.as_secs_f64(),
        start.elapsed().as_secs_f64()
    );
}
