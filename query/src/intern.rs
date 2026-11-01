//! Interned, case-insensitive names.
//!
//! HSPICE names are case-insensitive. A [`Sym`] stands for a name in any
//! ASCII letter case, so two spellings of one name get one `Sym`, and `Sym`
//! equality is the case-insensitive comparison. The table keeps each name
//! once, in the spelling it was first interned with: [`Interner::name`]
//! gives that spelling, for display only.
//!
//! A built-in name that lowering adds itself (`r` for a resistor value,
//! `dc`, `w` for a positional width, model types, ...) is not a spelling
//! from the deck: [`Interner::with_builtins`] adds these names first, and
//! the first spelling from the deck replaces theirs.
//!
//! The names are in one `String` arena. The hash table holds only the
//! `Sym`s; its hash of the ASCII lowercase name is computed 8 bytes at a
//! time without a lowercase copy, and each step mixes every byte into all
//! the bits of the hash, also into the bits that the table uses.

use hashbrown::{HashTable, hash_table::Entry};
use std::fmt;

/// An interned name: an index into [`Interner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Sym(pub(crate) u32);

/// The table of interned names.
#[derive(Default)]
pub(crate) struct Interner {
    /// The names, one after another, each in its first spelling.
    text: String,
    /// The end of each name in `text`, by `Sym`. A name starts where the
    /// one before it ends.
    ends: Vec<u32>,
    /// The `Sym`s, by [`hash_lower`] of their names.
    table: HashTable<Sym>,
    /// The built-in names whose spelling is not from the deck yet: bit `k`
    /// for `Sym(k)` (the built-in names are the first `Sym`s).
    provisional: u64,
}

impl fmt::Debug for Interner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Interner")
            .field("names", &self.ends.len())
            .field("bytes", &self.text.len())
            .finish()
    }
}

impl Interner {
    /// An interner that holds the built-in names `names` (at most 64
    /// distinct ones), as the first `Sym`s. Lowering adds these names
    /// itself with [`Interner::builtin`]; their spelling here is not from
    /// the deck, so the first spelling that [`Interner::intern`] gets for
    /// one of them replaces it.
    pub(crate) fn with_builtins<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut i = Self::default();
        let mut provisional = 0_u64;
        for name in names {
            let sym = i.add(name);
            assert!(sym.0 < 64, "at most 64 built-in names");
            provisional |= 1 << sym.0;
        }
        i.provisional = provisional;
        i
    }

    /// The `Sym` of `name` (a spelling from the deck), in any letter case.
    /// A new name is added in this spelling. A built-in name with no
    /// spelling from the deck yet takes this one.
    #[inline]
    pub(crate) fn intern(&mut self, name: &str) -> Sym {
        let sym = self.add(name);
        if self.provisional != 0 && sym.0 < 64 && self.provisional & (1 << sym.0) != 0 {
            self.provisional &= !(1 << sym.0);
            let i = sym.0 as usize;
            let start = if i == 0 { 0 } else { self.ends[i - 1] as usize };
            // equal ignoring ASCII case: the same length, and the bytes
            // that change are ASCII letters
            self.text.replace_range(start..self.ends[i] as usize, name);
        }
        sym
    }

    /// The `Sym` of the built-in name `name` (see
    /// [`Interner::with_builtins`]). Unlike [`Interner::intern`], it does
    /// not give the name a spelling.
    #[inline]
    pub(crate) fn builtin(&mut self, name: &str) -> Sym {
        let sym = self.add(name);
        debug_assert!(
            self.get(name).is_some_and(|s| s.0 < 64),
            "`{name}` is not a built-in name"
        );
        sym
    }

    /// The `Sym` of `name`, in any letter case. A new name is added in this
    /// spelling.
    fn add(&mut self, name: &str) -> Sym {
        let Self {
            text, ends, table, ..
        } = self;
        let hash = hash_lower(name);
        let entry = table.entry(
            hash,
            |&s| spelled(text, ends, s).eq_ignore_ascii_case(name),
            |&s| hash_lower(spelled(text, ends, s)),
        );
        match entry {
            Entry::Occupied(o) => *o.get(),
            Entry::Vacant(v) => {
                let sym = Sym(u32::try_from(ends.len()).expect("fewer than 2^32 names"));
                text.push_str(name);
                ends.push(u32::try_from(text.len()).expect("less than 4 GiB of distinct names"));
                v.insert(sym);
                sym
            }
        }
    }

    /// The `Sym` of `name`, in any letter case, if it was interned.
    pub(crate) fn get(&self, name: &str) -> Option<Sym> {
        let Self {
            text, ends, table, ..
        } = self;
        table
            .find(hash_lower(name), |&s| {
                spelled(text, ends, s).eq_ignore_ascii_case(name)
            })
            .copied()
    }

    /// The name of `sym`, in the spelling it was first interned with. For
    /// output only: compare `Sym`s, not these strings.
    pub(crate) fn name(&self, sym: Sym) -> &str {
        spelled(&self.text, &self.ends, sym)
    }

    /// The number of distinct names.
    #[allow(dead_code)] // read by the tests
    pub(crate) fn len(&self) -> usize {
        self.ends.len()
    }
}

/// The name of `sym` in the arena `text` with the name ends `ends`.
#[inline]
fn spelled<'a>(text: &'a str, ends: &[u32], sym: Sym) -> &'a str {
    let i = sym.0 as usize;
    let start = if i == 0 { 0 } else { ends[i - 1] as usize };
    &text[start..ends[i] as usize]
}

/// A hash of the ASCII lowercase form of `name`. Two names that are equal
/// ignoring ASCII case have the same hash.
///
/// Each step adds 8 bytes and does a folded multiply: the high half of the
/// 128-bit product is XORed into the low half. A plain multiply (FxHash)
/// carries a byte only to the bits above it, so names that differ only in
/// the top byte of their 8-byte words (`net_bus?net_bus?net_bus?`) would
/// differ only in the top 8 bits, at most 256 hashes for any number of
/// names. The fold carries each bit to all the bits of the hash: the
/// bucket index (low bits) and the tag (top 7 bits) of the table.
#[inline]
fn hash_lower(name: &str) -> u64 {
    // the multiplier of rustc-hash 2
    const K: u64 = 0xf135_7aea_2e62_a9c5;
    #[inline]
    fn step(h: u64, w: u64) -> u64 {
        let full = u128::from(h.wrapping_add(lower8(w))) * u128::from(K);
        (full as u64) ^ ((full >> 64) as u64)
    }
    let b = name.as_bytes();
    let mut h = b.len() as u64;
    let (words, rest) = b.as_chunks::<8>();
    for &w in words {
        h = step(h, u64::from_le_bytes(w));
    }
    if !rest.is_empty() {
        let mut w = [0u8; 8];
        w[..rest.len()].copy_from_slice(rest);
        h = step(h, u64::from_le_bytes(w));
    }
    h
}

/// The 8 bytes of `w` with each ASCII uppercase letter made lowercase.
/// Other bytes (non-ASCII included) are unchanged.
#[inline]
fn lower8(w: u64) -> u64 {
    const LOW7: u64 = 0x7f7f_7f7f_7f7f_7f7f;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let low7 = w & LOW7;
    // per byte, no carry between bytes: bit 7 set when the low 7 bits are
    // at least `A` (0x41), and when they are above `Z` (0x5a)
    let ge_a = low7 + 0x3f3f_3f3f_3f3f_3f3f;
    let gt_z = low7 + 0x2525_2525_2525_2525;
    // `!w`: a byte with bit 7 set is not ASCII
    let upper = ge_a & !gt_z & !w & HIGH;
    // bit 7 to bit 5 (0x20): `A` + 0x20 = `a`
    w | (upper >> 2)
}

#[cfg(test)]
mod tests {
    use super::{Interner, hash_lower, lower8};

    #[test]
    fn case_insensitive() {
        let mut i = Interner::default();
        let a = i.intern("VDD");
        assert_eq!(i.intern("vdd"), a);
        assert_eq!(i.intern("Vdd"), a);
        assert_ne!(i.intern("vss"), a);
        assert_eq!(i.get("vDD"), Some(a));
        assert_eq!(i.get("gnd"), None);
    }

    #[test]
    fn interner_single_copy() {
        let mut i = Interner::default();
        let vdd = i.intern("VDD");
        let net = i.intern("Net_Long_Name[12]");
        assert_eq!(i.intern("vdd"), vdd);
        assert_eq!(i.intern("NET_LONG_NAME[12]"), net);
        assert_eq!(i.intern("net_long_name[12]"), net);
        // the first spelling, stored once
        assert_eq!(i.name(vdd), "VDD");
        assert_eq!(i.name(net), "Net_Long_Name[12]");
        assert_eq!(i.len(), 2);
        assert_eq!(i.text, "VDDNet_Long_Name[12]");
        // the empty name, and names that differ only after 8 bytes
        let empty = i.intern("");
        assert_eq!(i.name(empty), "");
        let n1 = i.intern("abcdefgh1");
        let n2 = i.intern("ABCDEFGH2");
        assert_ne!(n1, n2);
        assert_eq!(i.get("ABCDEFGH1"), Some(n1));
        assert_eq!(i.get("abcdefgh2"), Some(n2));
        // case folding is ASCII only
        let e = i.intern("é");
        assert_eq!(i.get("É"), None);
        assert_eq!(i.get("é"), Some(e));
        // many names: the table grows and keeps every name
        let syms: Vec<_> = (0..10_000).map(|n| i.intern(&format!("N{n}"))).collect();
        for (n, &s) in syms.iter().enumerate() {
            assert_eq!(i.get(&format!("n{n}")), Some(s));
            assert_eq!(i.name(s), format!("N{n}"));
        }
    }

    #[test]
    fn builtin_spelling_replaced() {
        // Lowering adds `r`, `w`, `nmos`, ... itself. Those spellings are
        // not from the deck: the first spelling from the deck replaces them.
        let mut i = Interner::with_builtins(["r", "w", "nmos", "W"]);
        assert_eq!(i.len(), 3);
        let w = i.builtin("w");
        assert_eq!(i.name(w), "w");
        assert_eq!(i.builtin("W"), w);
        assert_eq!(i.name(w), "w");
        // `.param W=1` in the deck
        assert_eq!(i.intern("W"), w);
        assert_eq!(i.name(w), "W");
        // the first deck spelling stays
        assert_eq!(i.intern("w"), w);
        assert_eq!(i.builtin("w"), w);
        assert_eq!(i.name(w), "W");
        // a model type and a net with that name
        let nmos = i.builtin("nmos");
        let x = i.intern("X1");
        assert_eq!(i.intern("NMOS"), nmos);
        assert_eq!(i.name(nmos), "NMOS");
        assert_eq!(i.name(x), "X1");
        // a built-in name the deck never spells keeps its spelling
        let r = i.builtin("r");
        assert_eq!(i.name(r), "r");
        assert_eq!(i.get("R"), Some(r));
        // the arena: each name once, the replaced ones in place
        assert_eq!(i.text, "rWNMOSX1");
        // a name that is not built-in is added as usual
        let n = i.intern("Net1");
        assert_eq!(i.intern("NET1"), n);
        assert_eq!(i.name(n), "Net1");
    }

    #[test]
    fn hash_mixes_every_byte() {
        // Names that differ only at bytes 7, 15 and 23, the top byte of
        // each 8-byte word: 62^3 = 238328 names. A hash that kept these
        // bytes in its top 8 bits gave at most 256 hashes, and the table
        // compared each new name with thousands of others.
        let chars: Vec<char> = ('0'..='9').chain('a'..='z').chain('A'..='Z').collect();
        let names: Vec<String> = chars
            .iter()
            .flat_map(|&a| {
                let chars = &chars;
                chars.iter().flat_map(move |&b| {
                    chars
                        .iter()
                        .map(move |&c| format!("net_bus{a}net_bus{b}net_bus{c}"))
                })
            })
            .collect();
        assert_eq!(names.len(), 238_328);
        let t = std::time::Instant::now();
        let mut i = Interner::default();
        for n in &names {
            i.intern(n);
        }
        let dt = t.elapsed();
        // upper and lower case are one name: 36^3 distinct names
        assert_eq!(i.len(), 36 * 36 * 36);
        assert!(dt.as_secs_f64() < 1.0, "{dt:?}");
        // the bucket index (low bits) and the tag (top 7 bits) of the table
        // both vary
        let hashes: std::collections::HashSet<u64> = names.iter().map(|n| hash_lower(n)).collect();
        assert_eq!(hashes.len(), 36 * 36 * 36);
        let low: std::collections::HashSet<u64> = hashes.iter().map(|h| h & 0xffff).collect();
        let tags: std::collections::HashSet<u64> = hashes.iter().map(|h| h >> 57).collect();
        assert!(low.len() > 30_000, "{}", low.len());
        assert_eq!(tags.len(), 128);
        // one changed byte at any place changes the tag of most names
        let base = "abcdefghijklmnopqrstuvwx";
        for at in 0..base.len() {
            let tags: std::collections::HashSet<u64> = (b'0'..=b'9')
                .map(|c| {
                    let mut n = base.as_bytes().to_vec();
                    n[at] = c;
                    hash_lower(std::str::from_utf8(&n).unwrap()) >> 57
                })
                .collect();
            assert!(tags.len() >= 5, "byte {at}: {} tags", tags.len());
        }
    }

    #[test]
    fn hash_ignores_ascii_case() {
        for (a, b) in [
            ("AbC_[1]", "aBc_[1]"),
            ("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz"),
            ("X0.AAA", "x0.aaa"),
        ] {
            assert_eq!(hash_lower(a), hash_lower(b));
        }
        assert_ne!(hash_lower("a"), hash_lower("b"));
        assert_ne!(hash_lower("a"), hash_lower("a\0"));
        // `@` and `[` are next to `A` and `Z`: not letters
        assert_ne!(hash_lower("@"), hash_lower("`"));
        assert_ne!(hash_lower("["), hash_lower("{"));
    }

    #[test]
    fn lower8_matches_std() {
        for b in 0..=255u8 {
            for at in 0..8 {
                let mut bytes = *b"Az@[`{\xc3\x89";
                bytes[at] = b;
                let mut want = bytes;
                want.make_ascii_lowercase();
                let got = lower8(u64::from_le_bytes(bytes)).to_le_bytes();
                assert_eq!(got, want, "byte {b:#x} at {at}");
            }
        }
    }
}
