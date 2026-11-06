//! What `examples/model_select.rs` tells its user: the text of each message on
//! stderr, and the exit status. `cargo test` builds the examples. A run with
//! `--test model_select_cli` alone needs `cargo build --example model_select`.
use std::{
    env, fs,
    path::PathBuf,
    process::{Command, Output},
};

fn tool() -> PathBuf {
    let mut path = env::current_exe().unwrap();
    path.pop(); // deps
    path.pop();
    path.join("examples")
        .join(format!("model_select{}", env::consts::EXE_SUFFIX))
}

/// `model_select ARGS... DECK`, with `deck` in a file of its own
fn run(name: &str, args: &[&str], deck: &[u8]) -> (Output, String, String) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!("model_select_cli_{name}.sp"));
    fs::write(&path, deck).unwrap();
    let out = Command::new(tool())
        .args(args)
        .arg(&path)
        .output()
        .expect("the example is built: see the top of this file");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (out, stdout, stderr)
}

const MODEL: &str = "top\n.option parhier=local\n.model n nmos level=54\n";

#[test]
fn clean_deck_exits_with_0() {
    let deck = format!("{MODEL}M1 d g s b n l=1u w=1u\n.end\n");
    let (out, stdout, stderr) = run("clean", &[], deck.as_bytes());
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stdout.contains("M1\tn\t"), "{stdout}");
    assert!(!stderr.contains("error"), "{stderr}");
}

/// The block with no end takes M2. The user sees why, on which line, and the
/// rows that the tool could read.
#[test]
fn text_error_is_shown_and_exits_with_1() {
    let deck =
        format!("{MODEL}M1 d g s b n l=1u w=1u\n.prot freelib\nxx\nM2 d g s b n l=1u w=1u\n.end\n");
    for (name, args) in [("text", &[][..]), ("text_summary", &["--summary"][..])] {
        let (out, stdout, stderr) = run(name, args, deck.as_bytes());
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains("`.prot` has no `.unprot`"), "{stderr}");
        assert!(
            stderr.contains(&format!("model_select_cli_{name}.sp")),
            "{stderr}"
        );
        assert!(!stderr.contains("install a logger"), "{stderr}");
        // (`rows.py` and `make_kit.py` look for this text)
        assert!(stderr.contains("the netlist has errors"), "{stderr}");
        assert!(!stdout.contains("error"), "only the rows: {stdout}");
        if args.is_empty() {
            assert!(stdout.contains("M1\tn\t"), "{stdout}");
        }
    }
}

#[test]
fn x_line_with_no_definition_exits_with_1() {
    let deck = format!("{MODEL}M1 d g s b n l=1u w=1u\nX1 a b nope\n.end\n");
    for (name, args) in [("x", &[][..]), ("x_summary", &["--summary"][..])] {
        let (out, _, stderr) = run(name, args, deck.as_bytes());
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains("no definition"), "{stderr}");
    }
}

#[test]
fn mosfet_with_no_model_exits_with_1() {
    let deck = format!("{MODEL}M1 d g s b nope l=1u w=1u\n.end\n");
    for (name, args) in [
        ("no_model", &[][..]),
        ("no_model_summary", &["--summary"][..]),
    ] {
        let (out, stdout, stderr) = run(name, args, deck.as_bytes());
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        assert!(stdout.contains("error"), "{stdout}");
    }
}

/// A warning is shown, and it is not an error.
#[test]
fn warning_is_shown_and_exits_with_0() {
    let mut deck = format!("{MODEL}* caf").into_bytes();
    deck.push(0xE9);
    deck.extend_from_slice(b"\nM1 d g s b n l=1u w=1u\n.end\n");
    let (out, _, stderr) = run("warning", &[], &deck);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("not valid UTF-8"), "{stderr}");
}

/// The library logs a read past 31 levels, and the tool prints it: once.
#[test]
fn far_read_is_shown_once() {
    let mut deck = format!("{MODEL}.param p=1u\n");
    deck += ".subckt s0 d g\nM1 d g 0 0 n l=p w=1u\n.ends\n";
    for i in 1..=40 {
        deck += &format!(".subckt s{i} d g\nx d g s{}\n.ends\n", i - 1);
    }
    deck += "x0 a b s40\n.end\n";
    let (out, _, stderr) = run("far_read", &[], deck.as_bytes());
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let warnings = stderr.matches("warning: ").count();
    assert_eq!(warnings, 1, "{stderr}");
}
