//! The helpers that the test files share. The test files that use them
//! have `mod common;`, and no file uses all of them.
#![allow(dead_code)]

use netlist_query::{LoadOptions, NetInfo, Netlist};
use std::path::{Path, PathBuf};

/// The path of `rel` in the repository.
pub fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// A deck written by `write_deck`. It derefs to the path of its first
/// file. Its directory belongs to this test target and this process, so
/// two test runs at the same time (in one target directory) do not share
/// it. The directory is removed on drop.
pub struct Deck {
    pub dir: PathBuf,
    pub main: PathBuf,
}

impl Drop for Deck {
    fn drop(&mut self) {
        _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl std::ops::Deref for Deck {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.main
    }
}

impl AsRef<Path> for Deck {
    fn as_ref(&self) -> &Path {
        &self.main
    }
}

/// Writes `files` (name, text) into a new directory for test `name`. The
/// directory is in a folder named after the test target (`dspf`, `hier`,
/// ...), and its name ends with the process id.
pub fn write_deck(name: &str, files: &[(&str, &str)]) -> Deck {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(env!("CARGO_CRATE_NAME"))
        .join(format!("{name}-{}", std::process::id()));
    _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        std::fs::write(dir.join(file), text).unwrap();
    }
    let main = dir.join(files[0].0);
    Deck { dir, main }
}

/// Loads the deck at `path`. It panics with the path when the load fails.
pub fn load(path: impl AsRef<Path>) -> Netlist {
    Netlist::load(&LoadOptions::new(path.as_ref().to_path_buf()))
        .unwrap_or_else(|e| panic!("{}: {e}", path.as_ref().display()))
}

/// Writes `text` as `main.sp` for test `name`, and loads it. Keep the
/// `Deck` while the test reads the files.
pub fn load_text(name: &str, text: &str) -> (Deck, Netlist) {
    let deck = write_deck(name, &[("main.sp", text)]);
    let net = load(&deck.main);
    (deck, net)
}

/// (element, pin role) of each connection.
pub fn pins(i: &NetInfo) -> Vec<(String, String)> {
    i.connections
        .items
        .iter()
        .map(|c| (c.element.to_ascii_lowercase(), c.pin_role.clone()))
        .collect()
}

/// The pairs of `v` as owned strings, to compare with [`pins`].
pub fn owned(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}
