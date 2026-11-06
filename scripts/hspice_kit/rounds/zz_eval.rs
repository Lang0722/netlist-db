// (a scratch helper of the HSPICE comparison, not for the repository)
// usage: zz_eval deck.sp : prints `name value` for each `.param` of the top level
use netlist_db::{FileId, parser::parse_top_multi};
use std::path::PathBuf;
#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (parsed, files) = parse_top_multi(args.iter().map(|path| FileId::Include { path: PathBuf::from(path) })).await;
    let (ast, _) = files.build(parsed);
    let scope = ast.scope();
    for p in &ast.param {
        match scope.eval(&p.v) {
            Ok(v) => println!("{} {:e}", p.k, v),
            Err(e) => println!("{} error {}", p.k, e),
        }
    }
}
