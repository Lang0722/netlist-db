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
// a note on stderr says that the count can be too low. So does an X line that
// cannot be followed (a wrong node count, a call of its own subckt, too deep a
// hierarchy) and a `.if` condition that cannot be computed.
//
// Each error and warning in the text of the netlist goes to stderr, with its
// file and line. The exit status is 1 when the netlist has an error, a MOSFET
// has no model or a line hides MOSFETs, and the rows on stdout are those that
// could be read. Warnings do not change the exit status. With no file, or with
// `--subckt` and no name, the tool prints its usage and exits with 2.
//
// The parameters follow the last `.option parhier` of the netlist. Without one
// they follow HSPICE's default, `global`, and a note on stderr says so. One with
// no value means `local`, and a note says that too. A netlist that a
// `parhier=local` testbench includes has no such line: pass a file that holds
// it as one more file.
use netlist_db::{
    AST, FileId,
    eval::{FarRead, ParHier},
    models::{ModelUsage, MosfetModel, Short},
    parser::parse_top_multi,
};
use std::{
    env,
    io::{BufWriter, ErrorKind, Write, stdout},
    path::PathBuf,
    time::Instant,
};

/// Writes the messages of the library to stderr, so that stdout has only the rows
#[cfg(not(feature = "tracing"))]
struct Messages;

#[cfg(not(feature = "tracing"))]
impl log::Log for Messages {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        let level = record.level().as_str().to_ascii_lowercase();
        eprintln!("{level}: {}", record.args());
    }
    fn flush(&self) {}
}

/// Runs `build` with the errors and warnings of the library on stderr. The other
/// steps do not log: `report` prints the warnings of the selection, with the
/// name of the subckt.
fn with_messages<T>(build: impl FnOnce() -> T) -> T {
    #[cfg(not(feature = "tracing"))]
    {
        log::set_logger(&Messages).expect("the only logger");
        log::set_max_level(log::LevelFilter::Warn);
        let built = build();
        log::set_max_level(log::LevelFilter::Off);
        built
    }
    #[cfg(feature = "tracing")]
    {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_max_level(tracing::Level::WARN)
            .finish();
        tracing::subscriber::with_default(subscriber, build)
    }
}

/// Prints how to call the tool, and ends it with the status 2
fn exit_with_usage() -> ! {
    eprintln!("usage: model_select [--summary] [--subckt NAME] deck.sp [more files...]");
    std::process::exit(2);
}

/// Checks a write to stdout. When the reader has gone (`model_select deck.sp |
/// head`), the tool ends quietly. Another error ends it with a message.
fn written(result: std::io::Result<()>) {
    match result {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => {
            eprintln!("error: cannot write the rows: {e}");
            std::process::exit(1);
        }
    }
}

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
        // (an X line or a `.if` block is not a share of the MOSFETs)
        let share = if e.is_mosfet() {
            format!("{:.3}%", 100.0 * *n as f64 / total)
        } else {
            "-".to_owned()
        };
        writeln!(out, "{prefix}error: {e}\t{n}\t{share}\te.g. {path}")?;
    }
    writeln!(out, "{prefix}total\t{}", usage.mosfets)
}

/// The MOSFETs of `ast`, or of its subckt `name`: listed (as they are
/// found, not kept), or counted. Returns the MOSFETs, those with a model, and
/// the X lines and `.if` blocks that hide MOSFETs.
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
        written(print_usage(out, &prefix, &usage));
        warn(&usage.far_reads);
        let chosen = usage.models.iter().map(|(_, n)| n).sum();
        let missing = usage
            .errors
            .iter()
            .filter(|(e, ..)| !e.is_mosfet())
            .map(|(_, n, _)| n)
            .sum();
        (usage.mosfets, chosen, missing)
    } else {
        let (mut mosfets, mut chosen, mut missing) = (0, 0, 0);
        let mut each = |m: MosfetModel<'_, '_>| {
            // (a row that stands for a line that hides MOSFETs is not one)
            if m.selection.as_ref().is_err_and(|e| !e.is_mosfet()) {
                missing += 1;
            } else {
                mosfets += 1;
                chosen += u64::from(m.selection.is_ok());
            }
            written(print(out, &prefix, &m));
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
    let subckt = match args.iter().position(|a| a == "--subckt") {
        Some(i) if i + 1 < args.len() => {
            args.remove(i);
            Some(args.remove(i))
        }
        Some(_) => exit_with_usage(),
        None => None,
    };
    if args.is_empty() {
        exit_with_usage();
    }
    let start = Instant::now();
    let (parsed, files) = parse_top_multi(args.iter().map(|path| FileId::Include {
        path: PathBuf::from(path),
    }))
    .await;
    let (ast, has_err) = with_messages(|| files.build(parsed));
    let parsed_in = start.elapsed();
    if has_err {
        eprintln!("(the netlist has errors: see the messages above)");
    }
    // The mode is the selector's (`AST::scope`), which also reads the active
    // `.if` branches. The last `.option parhier` decides, and one with no
    // value means local, as measured in HSPICE (deck `oph_flag`). The notes
    // look at the top-level `.option` lines only.
    let local = ast.scope().parhier() == ParHier::Local;
    let parhier = ast
        .option
        .iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case("parhier"));
    match (parhier, local) {
        (None, false) => {
            eprintln!("(no `.option parhier` in the netlist: parhier=global, HSPICE's default)");
        }
        (Some((_, None)), true) => {
            eprintln!("(`.option parhier` with no value: parhier=local, as HSPICE reads it)");
        }
        _ => {}
    }
    let mut out = BufWriter::new(stdout().lock());
    if summary {
        written(writeln!(out, "model\tMOSFETs\tshare"));
    } else {
        written(writeln!(out, "path\tmodel name\tL\tW\tNF\tmodel"));
    }
    let start = Instant::now();
    let (mut mosfets, mut chosen, mut missing) = (0, 0, 0);
    if subckt.is_none() && ast.is_library() {
        // a library: each subckt, with its default parameters
        for def in ast.subckt.values() {
            let (n, c, x) = report(&mut out, &ast, Some(def.name), summary);
            mosfets += n;
            chosen += c;
            missing += x;
        }
    } else {
        (mosfets, chosen, missing) = report(&mut out, &ast, subckt.as_deref(), summary);
    }
    written(out.flush());
    if missing > 0 {
        eprintln!(
            "error: {missing} X line(s) or `.if` block(s) have no definition or cannot be \
             followed, so the count can be too low"
        );
    }
    eprintln!(
        "{mosfets} MOSFET(s), a model chosen for {chosen}; parsed in {:.2} s, models chosen in {:.2} s",
        parsed_in.as_secs_f64(),
        start.elapsed().as_secs_f64()
    );
    if has_err || missing > 0 || chosen != mosfets {
        std::process::exit(1);
    }
}
