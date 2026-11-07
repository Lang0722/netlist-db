//! What `examples/model_select.rs` tells its user: the text of each message on
//! stderr, and the exit status. `cargo test` builds the examples. A run with
//! `--test model_select_cli` alone needs `cargo build --example model_select`.
use std::{
    env, fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
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

/// An X line that cannot be followed (a wrong node count, a call of its own
/// subckt, too deep a hierarchy) and a `.if` condition that cannot be computed
/// hide MOSFETs. As for an X line with no definition, a note says that the count
/// can be too low, the exit status is 1, and both modes count the same MOSFETs.
#[test]
fn hidden_mosfets_exit_with_1() {
    let leaf = ".subckt s0 d g\nM1 d g 0 0 n l=1u w=1u\n.ends\n";
    let mut deep = leaf.to_owned();
    for i in 1..300 {
        deep += &format!(".subckt s{i} d g\nx d g s{}\n.ends\n", i - 1);
    }
    let recursive = ".subckt r d\nM1 d d 0 0 n l=1u w=1u\nX1 d r\n.ends\nX1 a r\n";
    let cond = ".subckt c d\n.if (nosuch)\nM1 d d 0 0 n l=1u w=1u\n.endif\n.ends\nX1 a c\n";
    let decks = [
        ("nodes", format!("{leaf}X1 a b c s0\n"), 1),
        ("recursive", recursive.to_owned(), 2),
        ("deep", format!("{deep}X1 a b s299\n"), 1),
        ("cond", cond.to_owned(), 1),
    ];
    for (name, body, mosfets) in decks {
        let deck = format!("{MODEL}M9 d g s b n l=1u w=1u\n{body}.end\n");
        for (mode, args) in [("", &[][..]), ("_summary", &["--summary"][..])] {
            let (out, stdout, stderr) = run(&format!("hidden_{name}{mode}"), args, deck.as_bytes());
            assert_eq!(out.status.code(), Some(1), "{name}{mode}: {stderr}");
            assert!(
                stderr.contains("cannot be followed"),
                "{name}{mode}: {stderr}"
            );
            let count = format!("{mosfets} MOSFET(s), a model chosen for {mosfets};");
            assert!(stderr.contains(&count), "{name}{mode}: {stderr}");
            if !mode.is_empty() {
                // (a line that hides MOSFETs is not a share of them)
                assert!(stdout.contains("\t-\te.g. "), "{name}: {stdout}");
            }
        }
    }
}

/// A `.if` block with no `.endif` before `.end`: the message names the line
/// of `.end` and quotes it.
#[test]
fn missing_endif_quotes_the_end_line() {
    let deck = format!("{MODEL}.if (1)\nM1 d g s b n l=1u w=1u\n.end\n");
    let (out, _, stderr) = run("missing_endif", &[], deck.as_bytes());
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("line 6\n.end\n"), "{stderr}");
}

/// No file, or `--subckt` with no name: a usage line, and the exit status 2.
#[test]
fn bad_arguments_exit_with_2() {
    let deck = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("model_select_cli_args.sp");
    fs::write(&deck, "* t\n.end\n").unwrap();
    let deck = deck.to_str().unwrap();
    for args in [&[][..], &["--summary"], &["--subckt"], &[deck, "--subckt"]] {
        let out = Command::new(tool()).args(args).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(
            stderr.starts_with("usage: model_select "),
            "{args:?}: {stderr}"
        );
        assert!(out.stdout.is_empty(), "{args:?}");
    }
}

/// A reader that stops early (`model_select deck.sp | head`) ends the tool
/// quietly, with no panic.
#[test]
fn closed_stdout_ends_quietly() {
    let mut deck = MODEL.to_owned();
    for i in 0..20_000 {
        deck += &format!("M{i} d g s b n l=1u w=1u\n");
    }
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("model_select_cli_closed.sp");
    fs::write(&path, deck).unwrap();
    for args in [&[][..], &["--summary"]] {
        let mut child = Command::new(tool())
            .args(args)
            .arg(&path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {stderr}");
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
    }
}

/// The note on `parhier` says what the tool computes. A bare `.option parhier`
/// means `local` (HSPICE deck `oph_flag`): here X1.M1 takes L from its subckt.
#[test]
fn parhier_note_follows_the_rule() {
    let body = ".model n nmos level=54\n.param p=1u\n\
                .subckt s d p=2u\nM1 d d 0 0 n l=p w=1u\n.ends\nX1 a s\n.end\n";
    for (name, option, mode, l) in [
        ("parhier_bare", ".option parhier\n", "local", "2e-6"),
        ("parhier_none", "", "global", "1e-6"),
    ] {
        let deck = format!("* t\n{option}{body}");
        let (out, stdout, stderr) = run(name, &[], deck.as_bytes());
        assert_eq!(out.status.code(), Some(0), "{name}: {stderr}");
        assert!(
            stdout.contains(&format!("X1.M1\tn\t{l}\t")),
            "{name}: {stdout}"
        );
        assert!(
            stderr.contains(&format!("parhier={mode}")),
            "{name}: {stderr}"
        );
        assert_eq!(stderr.matches("parhier=").count(), 1, "{name}: {stderr}");
    }
    // `.option parhier=local` in an active `.if` branch: the tool computes
    // in local mode, so no note says global
    let deck = format!("* t\n.if (1)\n.option parhier=local\n.endif\n{body}");
    let (out, stdout, stderr) = run("parhier_if", &[], deck.as_bytes());
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stdout.contains("X1.M1\tn\t2e-6\t"), "{stdout}");
    assert!(!stderr.contains("parhier=global"), "{stderr}");
}
