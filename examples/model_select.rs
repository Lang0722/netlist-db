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
//
// An X line with no definition on its instance path gets an `error:` row, and
// a note on stderr says that the count can be too low. So does a `.if` condition
// that cannot be computed. The exit status does not change, as for the other
// errors.
//
// The parameters follow the last `.option parhier` of the netlist. Without one
// they follow HSPICE's default, `global`, and a note on stderr says so. A
// netlist that a `parhier=local` testbench includes has no such line: pass a
// file that holds it as one more file.
use netlist_db::{
    AST, FileId,
    eval::FarRead,
    models::{ModelUsage, MosfetModel, SelectError, Short},
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
        // (an X line is not a share of the MOSFETs)
        let share = match e {
            SelectError::NoSubckt(_) => "-".to_owned(),
            _ => format!("{:.3}%", 100.0 * *n as f64 / total),
        };
        writeln!(out, "{prefix}error: {e}\t{n}\t{share}\te.g. {path}")?;
    }
    writeln!(out, "{prefix}total\t{}", usage.mosfets)
}

/// The MOSFETs of `ast`, or of its subckt `name`: listed (as they are
/// found, not kept), or counted. Returns the MOSFETs, those with a model, and
/// the X lines with no definition.
fn report(
    out: &mut impl Write,
    ast: &AST<'_>,
    subckt: Option<&str>,
    summary: bool,
) -> (u64, u64, u64) {
    let no_subckt = || -> ! {
        eprintln!(
            "no subckt `{}` at the top level",
            subckt.unwrap_or_default()
        );
        std::process::exit(1);
    };
    let prefix = subckt.map(|s| format!("{s}:")).unwrap_or_default();
    let warn = |far_reads: &[FarRead]| {
        for far in far_reads {
            eprintln!("warning: {prefix}{far}");
        }
    };
    if summary {
        let usage = match subckt {
            Some(name) => ast.model_usage_of(name).unwrap_or_else(|| no_subckt()),
            None => ast.model_usage(),
        };
        print_usage(out, &prefix, &usage).expect("write");
        warn(&usage.far_reads);
        let chosen = usage.models.iter().map(|(_, n)| n).sum();
        let missing = usage
            .errors
            .iter()
            .filter(|(e, ..)| matches!(e, SelectError::NoSubckt(_)))
            .map(|(_, n, _)| n)
            .sum();
        (usage.mosfets, chosen, missing)
    } else {
        let (mut mosfets, mut chosen, mut missing) = (0, 0, 0);
        let mut each = |m: MosfetModel<'_, '_>| {
            if matches!(m.selection, Err(SelectError::NoSubckt(_))) {
                missing += 1;
            } else {
                mosfets += 1;
                chosen += u64::from(m.selection.is_ok());
            }
            print(out, &prefix, &m).expect("write");
        };
        let far_reads = match subckt {
            Some(name) => ast
                .for_each_mosfet_model_of(name, &mut each)
                .unwrap_or_else(|| no_subckt()),
            None => ast.for_each_mosfet_model(&mut each),
        };
        warn(&far_reads);
        (mosfets, chosen, missing)
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
    if ast.option_value("parhier").is_none() {
        eprintln!("(no `.option parhier` in the netlist: parhier=global, HSPICE's default)");
    }
    let mut out = BufWriter::new(stdout().lock());
    if summary {
        writeln!(out, "model\tMOSFETs\tshare").expect("write");
    } else {
        writeln!(out, "path\tmodel name\tL\tW\tNF\tmodel").expect("write");
    }
    let start = Instant::now();
    let (mut mosfets, mut chosen, mut missing) = (0, 0, 0);
    if subckt.is_none() && ast.instance.is_empty() {
        // a library: each subckt, with its default parameters
        for def in ast.subckt.values() {
            let (n, c, x) = report(&mut out, &ast, Some(&def.name), summary);
            mosfets += n;
            chosen += c;
            missing += x;
        }
    } else {
        (mosfets, chosen, missing) = report(&mut out, &ast, subckt.as_deref(), summary);
    }
    out.flush().expect("write");
    if missing > 0 {
        eprintln!("error: {missing} X line(s) have no definition, so the count can be too low");
    }
    eprintln!(
        "{mosfets} MOSFET(s), a model chosen for {chosen}; parsed in {:.2} s, models chosen in {:.2} s",
        parsed_in.as_secs_f64(),
        start.elapsed().as_secs_f64()
    );
}
