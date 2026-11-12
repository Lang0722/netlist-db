// cargo run --example parser_multi -- tests/data.sp tests/lib.sp
// Avoid musl's default allocator due to lackluster performance
// https://nickb.dev/blog/default-musl-allocator-considered-harmful-to-performance
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use netlist_db::{FileId, parser::parse_top_multi};
use std::{env, path::PathBuf, process::exit, time::Instant};

/// Prints the messages of the library
#[cfg(not(feature = "tracing"))]
struct Messages;

#[cfg(not(feature = "tracing"))]
impl log::Log for Messages {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        let (level, target) = (record.level(), record.target());
        println!("{level:<5} [{target}] {}", record.args());
    }
    fn flush(&self) {}
}

#[tokio::main]
async fn main() {
    #[cfg(not(feature = "tracing"))]
    {
        log::set_logger(&Messages).expect("the only logger");
        log::set_max_level(log::LevelFilter::Trace);
    }
    #[cfg(feature = "tracing")]
    {
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::DEBUG)
            .with_target(false)
            .with_file(true)
            .with_line_number(true)
            .finish();
        _ = tracing::subscriber::set_global_default(subscriber);
    }
    let now = Instant::now();
    let (parsed, files) = parse_top_multi(env::args().skip(1).map(|path| FileId::Include {
        path: PathBuf::from(path),
    }))
    .await;
    let elapsed_parse = now.elapsed();
    let now = Instant::now();
    let (ast, has_err) = files.build(parsed);
    let elapsed_build = now.elapsed();
    let now = Instant::now();
    println!("======= AST ===========");
    println!("{ast}");
    let elapsed_print = now.elapsed();
    println!("======= stats =========");
    println!("parse: {elapsed_parse:?}");
    println!("build: {elapsed_build:?}");
    println!("print: {elapsed_print:?}");
    println!("=======================");
    if has_err {
        exit(1)
    };
}
