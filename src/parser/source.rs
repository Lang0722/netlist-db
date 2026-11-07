//! The text of a file before it is parsed: its bytes as text, and its lines.
use super::statement::canonical_cmd;
use crate::span::LocatedSpan;
use nom::Input as _;

/// The character that stands for `byte`, a byte that is not UTF-8 (0x80 or
/// above). Each value has its own character in a private range, so that two
/// names that differ in such a byte stay two names.
fn raw_byte(byte: u8) -> char {
    char::from_u32(0xF700 + u32::from(byte)).expect("a private character")
}

/// Whether `c` is a character of [`raw_byte`]
pub(crate) fn is_raw_byte(c: char) -> bool {
    ('\u{F780}'..='\u{F7FF}').contains(&c)
}

/// `bytes` as text, with [`raw_byte`] for each byte that is not UTF-8
fn with_raw_bytes(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        text.push_str(chunk.valid());
        text.extend(chunk.invalid().iter().map(|&byte| raw_byte(byte)));
    }
    text
}

/// A file as text, as [`decode`] reads it: the text, its first line with bytes
/// that are not UTF-8, and the lines that a lone CR ends, in order
pub(super) type Decoded = (String, Option<u32>, Vec<u32>);

/// Read a netlist as text: remove UTF-8 byte order marks (also from inside a
/// file, e.g. one made by joining files), and turn lone CR line ends (classic
/// Mac OS) into LF. Bytes that are not UTF-8 (e.g. a `µ` written in Latin-1)
/// are read with [`raw_byte`], instead of rejecting the whole file. Also returns
/// the first line with such bytes, and the lines that a lone CR ends (the
/// lines of the text that this gives).
pub(super) fn decode(bytes: Vec<u8>) -> Decoded {
    let (mut text, lossy) = match String::from_utf8(bytes) {
        Ok(text) => (text, false),
        Err(e) => (with_raw_bytes(e.as_bytes()), true),
    };
    if text.contains('\u{feff}') {
        text = text.replace('\u{feff}', "");
    }
    // (white space, as measured: deck `e_formfeed`)
    if text.contains('\u{c}') {
        text = text.replace('\u{c}', " ");
    }
    // HSPICE on Windows reads a lone CR as a space, so a file with only CR line
    // ends is one title line to it (measured on P-2019.06-SP1-1 win64). This
    // keeps the logical lines, and the caller gives a warning.
    // (all of them: a `.lib` section reports the first one in its own lines)
    let bytes = text.as_bytes();
    let mut lone_cr = Vec::new();
    let (mut line, mut from) = (1, 0);
    for (idx, _) in text.match_indices('\r') {
        if bytes.get(idx + 1) != Some(&b'\n') {
            line += text[from..idx].matches('\n').count();
            lone_cr.push(u32::try_from(line).unwrap_or(u32::MAX));
            line += 1;
            from = idx + 1;
        }
    }
    if !lone_cr.is_empty() {
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            out.push(if c == '\r' && chars.peek() != Some(&'\n') {
                '\n'
            } else {
                c
            });
        }
        text = out;
    }
    // (after the CR line ends are LF: `slash_comments` splits the text at LF)
    if text.contains("//") {
        text = slash_comments(text);
    }
    let line = lossy.then(|| {
        let at = text.find(is_raw_byte).unwrap_or(0);
        u32::try_from(text[..at].matches('\n').count() + 1).unwrap_or(u32::MAX)
    });
    (text, line, lone_cr)
}

/// HSPICE reads a word that starts with `//` as the start of a comment, on every
/// kind of line, and a following `+` line goes on with the statement (decks
/// `cm_slash_*`, `o_slash_*`). This turns `//` into `$ `: the length of the text
/// stays, so the positions in it do too. Inside quotes, and on a line that names
/// a file, `//` stays.
fn slash_comments(text: String) -> String {
    let mut bytes = text.into_bytes();
    for line in bytes.split_mut(|b| *b == b'\n') {
        let first = line.trim_ascii_start();
        if first.starts_with(b"*") || names_a_file(first) {
            continue;
        }
        let mut quote = None;
        let mut word_start = true;
        for k in 0..line.len() {
            let c = line[k];
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None if matches!(c, b'\'' | b'"') => quote = Some(c),
                None if c == b'$' && word_start => break,
                None if c == b'/' && word_start && line.get(k + 1) == Some(&b'/') => {
                    line[k] = b'$';
                    line[k + 1] = b' ';
                    break;
                }
                None => {}
            }
            word_start = quote.is_none() && matches!(c, b' ' | b'\t' | b',' | b'(' | b')');
        }
    }
    // (only ASCII characters changed)
    String::from_utf8(bytes).expect("the text was UTF-8")
}

/// Whether `line`, with no spaces before it, is a command with a file name
fn names_a_file(line: &[u8]) -> bool {
    let Some(word) = line.strip_prefix(b".") else {
        return false;
    };
    let len = word
        .iter()
        .position(|b| !b.is_ascii_alphabetic())
        .unwrap_or(word.len());
    let word = String::from_utf8_lossy(&word[..len]).to_ascii_lowercase();
    matches!(canonical_cmd(&word), "include" | "lib" | "hdl" | "vec")
}

/// The number of lines of `text` that are not empty, not comments and not
/// in a `.lib <section>` ... `.endl` block (a `.lib` call reads a section
/// also after `.end`).
pub(super) fn statement_lines(text: &str) -> u32 {
    let mut n = 0_usize;
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim_start_matches([' ', '\t', '\r']);
        if line.is_empty() || line.starts_with(['*', '$']) {
            continue;
        }
        let mut words = line[..line.find('$').unwrap_or(line.len())].split_whitespace();
        let cmd = words.next().unwrap_or_default();
        if in_section {
            in_section = !cmd.eq_ignore_ascii_case(".endl");
        } else if cmd.eq_ignore_ascii_case(".lib")
            && words.next().is_some()
            && words.next().is_none()
        {
            // `.lib <section>`, not a `.lib <file> <section>` call
            in_section = true;
        } else {
            n += 1;
        }
    }
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The first line at or after `i` (at the start of a line) that is not
/// empty and not a `*` or `$` comment line, after its spaces. Unlike
/// `skip_blank`, it stops at every `+` line, also at a `+` line with only
/// a comment.
pub(super) fn skip_comment_lines(i: LocatedSpan<'_>) -> LocatedSpan<'_> {
    let b = i.fragment().as_bytes();
    let mut k = 0;
    loop {
        while b.get(k).is_some_and(|c| matches!(c, b' ' | b'\t' | b'\r')) {
            k += 1;
        }
        match b.get(k) {
            Some(b'*' | b'$' | b'\n') => match b[k..].iter().position(|&c| c == b'\n') {
                Some(n) => k += n + 1,
                None => return i.take_from(b.len()),
            },
            _ => return i.take_from(k),
        }
    }
}

/// HSPICE continues a statement with a `\` at the end of a line (the line
/// break counts as white space) or a `\\` (the lines are joined with no white
/// space, e.g. to split a long path), also inside quoted strings. Such lines
/// are joined here, before parsing. Each removed newline is put back after the
/// joined line, so the line numbers of what follows stay right. A `\` inside a
/// comment continues nothing; a `+` that also starts the next line (outside
/// quotes) is dropped.
pub(super) fn join_continued_lines(text: String) -> String {
    if !text.contains('\\') {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut moved_newlines = 0;
    let mut continuing = false;
    // the previous line ended with `\\`
    let mut squeeze = false;
    // quote state of the statement being joined
    let mut in_quote = false;
    for line in text.split_inclusive('\n') {
        let (mut body, eol) = match line.strip_suffix('\n') {
            Some(b) => match b.strip_suffix('\r') {
                Some(b) => (b, "\r\n"),
                None => (b, "\n"),
            },
            None => (line, ""),
        };
        let comment_line = !continuing && body.trim_start().starts_with('*');
        if continuing {
            if squeeze {
                body = body.trim_start();
            }
            // (inside quotes a leading `+` is an operator, and is kept)
            let plus = if in_quote {
                None
            } else {
                body.trim_start().strip_prefix('+')
            };
            if let Some(rest) = plus {
                body = if squeeze { rest.trim_start() } else { rest };
            }
        } else {
            in_quote = false;
        }
        let mut dollar_comment = false;
        // whether the word here starts with a letter: a `$` in such a word is
        // in a name (see `tokens::dollar_in_name`)
        let mut in_name = None;
        for c in body.chars() {
            match c {
                '\'' => in_quote = !in_quote,
                '$' if !in_quote && in_name != Some(true) => {
                    dollar_comment = true;
                    break;
                }
                _ => {}
            }
            in_name = (c.is_alphanumeric() || "_.$".contains(c))
                .then(|| in_name.unwrap_or(c.is_alphabetic() || c == '_'));
        }
        let trimmed = body.trim_end();
        let backslashes = if comment_line || dollar_comment {
            0
        } else if trimmed.ends_with("\\\\") {
            2
        } else if trimmed.ends_with('\\') {
            1
        } else {
            0
        };
        if backslashes == 0 {
            out.push_str(body);
            out.push_str(eol);
            out.extend(std::iter::repeat_n('\n', moved_newlines));
            moved_newlines = 0;
            continuing = false;
        } else {
            let content = &trimmed[..trimmed.len() - backslashes];
            if backslashes == 2 {
                out.push_str(content.trim_end());
            } else {
                out.push_str(content);
                out.push(' ');
            }
            if !eol.is_empty() {
                moved_newlines += 1;
            }
            continuing = true;
            squeeze = backslashes == 2;
        }
    }
    out.extend(std::iter::repeat_n('\n', moved_newlines));
    out
}
