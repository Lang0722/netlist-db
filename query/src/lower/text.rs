//! Words and spans in the text of a line: the lowering reads the values that the parser left as text.
use super::*;

/// The text `t` on one line: without the `+` markers, the `$` comments and
/// the `*` comment lines of the file, as
/// `netlist_db::instance::source_args` gives it, except where HSPICE
/// rejects the deck. HSPICE rejects a deck with a `$` inside quotes (it is
/// not the start of a comment there: golden `rcl_quoted_dollar`,
/// `rcl_quoted_dollar_line`), with a line inside quotes that is not a `+`
/// line (a `*` or `$` comment line, a blank line, a line with only spaces:
/// golden `quoted_cmtline`, `quoted_dollarline_elem`, `quoted_blank_line`,
/// `quoted_spaces_elem`), and with a `*` comment line inside open
/// parentheses outside quotes (golden `paren_cmtline_call`). So these are
/// kept: a `$` with the text after it, and such a line on a line of its
/// own, which gives the only line breaks of the result. See
/// [`quote_fault`]. Outside quotes, blank lines and `$` comment lines are
/// passed over, also inside open parentheses (golden `plus_after_blank`,
/// `paren_blank_call`), and so are `*` comment lines outside parentheses
/// (golden `plus_after_cmtline`; `paren_cmtline_pwl`: the parentheses of a
/// source function are not in its argument text). A `+` line with no text
/// is passed over, also inside quotes (golden `quoted_plus_empty_elem`).
/// `quoted`: `t` starts inside quotes. No copy for a text on one line with
/// no `$`.
pub(crate) fn one_line(t: &str, quoted: bool) -> Cow<'_, str> {
    if !t.contains(['\n', '$']) {
        return Cow::Borrowed(t);
    }
    let mut out = String::with_capacity(t.len());
    let mut inside = quoted;
    // open parentheses outside quotes
    let mut depth = 0_usize;
    // before the next line: a line break after a kept line
    let mut sep = ' ';
    for (k, line) in t.split('\n').enumerate() {
        let mut line = line.trim();
        if k > 0 {
            let plus = line.strip_prefix('+');
            if plus.is_none() && (inside || depth > 0 && line.starts_with('*')) {
                out.push('\n');
                out.push_str(line);
                sep = '\n';
                continue;
            }
            if line.starts_with('*') {
                continue;
            }
            line = plus.unwrap_or(line).trim_start();
        }
        let mut end = line.len();
        for (n, c) in line.char_indices() {
            match c {
                '\'' => inside = !inside,
                '(' if !inside => depth += 1,
                ')' if !inside => depth = depth.saturating_sub(1),
                '$' if !inside => {
                    end = n;
                    break;
                }
                _ => {}
            }
        }
        let line = line[..end].trim_end();
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(sep);
        }
        sep = ' ';
        out.push_str(line);
    }
    Cow::Owned(out)
}

/// What HSPICE rejects in a text that [`one_line`] gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuoteFault {
    /// A line inside quotes that is not a `+` line: a `*` comment line
    /// (golden `quoted_cmtline`, `elem_unquoted_cmtline`), a `$` comment
    /// line (`quoted_dollarline_param`, `quoted_dollarline_elem`), a blank
    /// line (`quoted_blank_line`, `quoted_blank_param`) or a line with
    /// only spaces and tabs (`quoted_spaces_param`, `quoted_spaces_elem`):
    /// "string misses dexter quotation mark". [`quote_fault`] also gives it
    /// for [`QuoteFault::ParenCommentLine`]: the text that `one_line`
    /// gives does not tell them apart.
    CommentLine,
    /// A `*` comment line inside open parentheses outside quotes (golden
    /// `paren_cmtline_call`: "Illegal character in statement"). Only
    /// [`raw_quote_fault`] gives it.
    ParenCommentLine,
    /// A `$` inside quotes (golden `rcl_quoted_dollar`,
    /// `rcl_quoted_dollar_line`, `dollar_elem_line`, `dollar_func_body`,
    /// `dollar_override`).
    Dollar,
}

/// The fault of the text `s` that [`one_line`] gives, which makes HSPICE
/// reject the deck: a line break (a line that `one_line` keeps on a line
/// of its own), else a `$` (inside quotes: `one_line` drops a `$` comment).
pub(crate) fn quote_fault(s: &str) -> Option<QuoteFault> {
    if s.contains('\n') {
        Some(QuoteFault::CommentLine)
    } else if s.contains('$') {
        Some(QuoteFault::Dollar)
    } else {
        None
    }
}

/// `quote_fault(&one_line(t, quoted))`, with no copy of `t`, and with
/// [`QuoteFault::ParenCommentLine`] for a `*` comment line inside open
/// parentheses outside quotes.
pub(super) fn raw_quote_fault(t: &str, quoted: bool) -> Option<QuoteFault> {
    let mut inside = quoted;
    let mut depth = 0_usize;
    let mut dollar = false;
    for (k, line) in t.split('\n').enumerate() {
        let line = line.trim_start();
        if k > 0 && !line.starts_with('+') {
            if inside {
                return Some(QuoteFault::CommentLine);
            }
            if line.starts_with('*') {
                if depth > 0 {
                    return Some(QuoteFault::ParenCommentLine);
                }
                continue;
            }
        }
        for c in line.bytes() {
            match c {
                b'\'' => inside = !inside,
                b'(' if !inside => depth += 1,
                b')' if !inside => depth = depth.saturating_sub(1),
                b'$' if inside => dollar = true,
                b'$' => break,
                _ => {}
            }
        }
    }
    dollar.then_some(QuoteFault::Dollar)
}

/// True when the statement at byte `start` of `text` has a `{` or a `}`
/// outside its `$` comments (a `$` starts a comment, as in the parser) and
/// its `*` comment lines, also inside quotes: HSPICE rejects both (golden
/// `param_braces`, `brace_quoted_elem`).
pub(super) fn has_braces(text: &str, start: usize) -> bool {
    let (s, e) = statement_rest(text, start);
    text[s..e].lines().any(|line| {
        let code = line.trim_start();
        !code.starts_with('*') && code[..code.find('$').unwrap_or(code.len())].contains(['{', '}'])
    })
}

/// The text of the line of `text` that holds byte `pos`, trimmed and
/// shortened.
pub(super) fn line_at(text: &str, pos: usize) -> String {
    let mut pos = pos.min(text.len());
    while !text.is_char_boundary(pos) {
        pos -= 1;
    }
    let start = text[..pos].rfind('\n').map_or(0, |n| n + 1);
    let end = text[pos..].find('\n').map_or(text.len(), |n| pos + n);
    shorten(text[start..end].trim())
}

/// `s`, cut to [`QUOTE_MAX`] characters.
pub(super) fn shorten(s: &str) -> String {
    match s.char_indices().nth(QUOTE_MAX) {
        Some((cut, _)) => format!("{}...", &s[..cut]),
        None => s.to_owned(),
    }
}

/// The byte after the `=` that follows a key ending at byte `key_end`.
pub(super) fn value_start(text: &str, key_end: usize) -> Option<usize> {
    let b = text.as_bytes();
    let at = skip_spaces(b, key_end);
    (b.get(at) == Some(&b'=')).then_some(at + 1)
}

/// The next token of the statement at byte `from`, when it reads as the
/// number `x`: its byte range. The token ends at a space or at a `$` (a
/// comment, also right after a number, as in the parser). See
/// [`next_token`] for what comes before it.
pub(super) fn num_token(text: &str, from: usize, x: f64) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let start = next_token(b, from)?;
    let mut end = start;
    while end < b.len() && !b[end].is_ascii_whitespace() && b[end] != b'$' {
        end += 1;
    }
    let token = text.get(start..end)?;
    match netlist_expr::scan_number(token) {
        Some((v, n)) if n == token.len() && v == x => Some((start, end)),
        _ => None,
    }
}

/// The start of the next token at or after byte `at` in its statement:
/// spaces, `$` comments and line ends with the `+` marker of the next line
/// are passed over, and so are blank lines and `*` comment lines before a
/// `+` line. `None` when the statement ends first.
pub(crate) fn next_token(b: &[u8], mut at: usize) -> Option<usize> {
    // after a line end: a `+` goes on with the statement
    let mut line_start = false;
    loop {
        while matches!(b.get(at), Some(b' ' | b'\t' | b'\r')) {
            at += 1;
        }
        match *b.get(at)? {
            b'\n' => {
                at += 1;
                line_start = true;
            }
            b'+' if line_start => {
                at += 1;
                line_start = false;
            }
            b'*' if line_start => at = line_end(b, at),
            b'$' => at = line_end(b, at),
            _ if line_start => return None,
            _ => return Some(at),
        }
    }
}

fn skip_spaces(b: &[u8], mut at: usize) -> usize {
    while at < b.len() && matches!(b[at], b' ' | b'\t') {
        at += 1;
    }
    at
}

/// The index of the `\n` that ends the line of byte `at`, or the length.
fn line_end(b: &[u8], at: usize) -> usize {
    b[at.min(b.len())..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |n| at + n)
}

/// The values of a PWL, from its arguments on one line (see
/// [`one_line`]): the words of [`arg_words`]. The values end at a word `R`
/// or at a `key=value` word (HSPICE manual: `R` and `TD=` can come before
/// the `)`).
pub(crate) fn pwl_values(args: &str) -> Vec<&str> {
    let mut words = arg_words(args);
    let end = words
        .iter()
        .position(|w| w.eq_ignore_ascii_case("r") || (!w.starts_with('\'') && w.contains('=')))
        .unwrap_or(words.len());
    words.truncate(end);
    words
}

/// The words of the arguments of a source function, on one line (see
/// [`one_line`]): the words between spaces and commas. A `'…'` expression
/// and a `(…)` group are one word.
pub(crate) fn arg_words(args: &str) -> Vec<&str> {
    let b = args.as_bytes();
    let mut words = Vec::new();
    let mut start = None;
    let mut depth = 0_usize;
    let mut k = 0;
    while k < b.len() {
        match b[k] {
            b'\'' => {
                start.get_or_insert(k);
                k += args[k + 1..].find('\'').map_or(b.len() - k, |n| n + 1);
            }
            b'(' => {
                start.get_or_insert(k);
                depth += 1;
            }
            b')' => depth = depth.saturating_sub(1),
            b' ' | b'\t' | b'\r' | b',' if depth == 0 => {
                if let Some(s) = start.take() {
                    words.push(&args[s..k]);
                }
            }
            _ => {
                start.get_or_insert(k);
            }
        }
        k += 1;
    }
    if let Some(s) = start {
        words.push(&args[s..]);
    }
    words
}

/// The text from byte `from` (after spaces) to the end of its statement:
/// its line, and the `+` lines after it (blank lines and `*` and `$`
/// comment lines between them are passed over, as the parser does).
/// Trailing spaces are not in it.
pub(super) fn statement_rest(text: &str, from: usize) -> (usize, usize) {
    let b = text.as_bytes();
    let start = skip_spaces(b, from);
    let mut end = line_end(b, start);
    let mut next = end;
    while next < b.len() {
        let line = skip_spaces(b, next + 1);
        match b.get(line) {
            Some(b'+') => {
                end = line_end(b, line);
                next = end;
            }
            Some(b'*' | b'$' | b'\n' | b'\r') => next = line_end(b, line),
            _ => break,
        }
    }
    while end > start && b[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    (start, end)
}

/// The `key=value` parameters (`key` in any case) of the statement that
/// goes on at byte `from` of `text`: the byte range of the first `key`
/// word (its spelling on the line), and the byte after the `=` of the last
/// one (its value wins, golden `elem_dup_kw`). `None` when there is none.
/// Quoted texts, `{…}` groups, `$` comments and `*` comment lines are
/// passed over.
pub(super) fn keywords(text: &str, from: usize, key: &str) -> Option<((usize, usize), usize)> {
    let (start, end) = statement_rest(text, from);
    let b = &text.as_bytes()[..end];
    let mut first = None;
    let mut last = None;
    let mut at = start;
    // after a line end: a `*` line is a comment
    let mut line_start = false;
    while at < end {
        let c = b[at];
        if line_start && !matches!(c, b' ' | b'\t' | b'\r') {
            line_start = false;
            if c == b'*' {
                at = line_end(b, at);
                continue;
            }
        }
        match c {
            b'\n' => {
                line_start = true;
                at += 1;
            }
            b'$' if at == start || b[at - 1].is_ascii_whitespace() => at = line_end(b, at),
            b'\'' | b'{' => {
                let close = if c == b'{' { b'}' } else { b'\'' };
                at = b[at + 1..]
                    .iter()
                    .position(|&d| d == close)
                    .map_or(end, |n| at + 2 + n);
            }
            b'=' => {
                // the word before the `=`
                let mut word_end = at;
                while word_end > start && matches!(b[word_end - 1], b' ' | b'\t') {
                    word_end -= 1;
                }
                let mut word_start = word_end;
                while word_start > start
                    && !b[word_start - 1].is_ascii_whitespace()
                    && !matches!(b[word_start - 1], b'+' | b'=')
                {
                    word_start -= 1;
                }
                if b[word_start..word_end].eq_ignore_ascii_case(key.as_bytes()) {
                    first = first.or(Some((word_start, word_end)));
                    last = Some(at + 1);
                }
                at += 1;
            }
            _ => at += 1,
        }
    }
    first.zip(last)
}

/// The word `key` (in any letter case) as the deck writes it right before
/// the value token that starts at byte `at` of `text`: `key=value`, with
/// spaces around `=` and a `'` before the value allowed, or also
/// `key value` when `eq` is false (`DC 1`). `None` otherwise.
pub(super) fn keyword_before<'t>(text: &'t str, at: usize, key: &str, eq: bool) -> Option<&'t str> {
    let b = text.as_bytes();
    let back = |mut p: usize| {
        while p > 0 && matches!(b[p - 1], b' ' | b'\t') {
            p -= 1;
        }
        p
    };
    let mut p = at.min(b.len());
    if p > 0 && b[p - 1] == b'\'' {
        p -= 1;
    }
    let before = back(p);
    p = if before > 0 && b[before - 1] == b'=' {
        back(before - 1)
    } else if !eq && before < p {
        before
    } else {
        return None;
    };
    let start = p.checked_sub(key.len())?;
    let word = text.get(start..p)?;
    let bounded = start == 0 || matches!(b[start - 1], b' ' | b'\t' | b'\n' | b'\r' | b'+');
    (bounded && word.eq_ignore_ascii_case(key)).then_some(word)
}

/// The text that starts at byte `start` of `b` is written as `key=value`
/// (a `=` before it, also before a quote), not as a bare token. The `=` can
/// end the line before the `+` line of the value, with blank lines and
/// comment lines between (golden `kw_value_next_line`: `r1 1 0 r=` /
/// `+ 5k`).
pub(crate) fn after_eq(b: &[u8], start: usize) -> bool {
    let back = |mut at: usize| {
        while at > 0 && matches!(b[at - 1], b' ' | b'\t') {
            at -= 1;
        }
        at
    };
    let mut at = back(start.min(b.len()));
    if at > 0 && b[at - 1] == b'\'' {
        at = back(at - 1);
    }
    if at > 0 && b[at - 1] == b'=' {
        return true;
    }
    // the `+` that starts the line of the value
    if at == 0 || b[at - 1] != b'+' {
        return false;
    }
    let mut line_start = back(at - 1);
    // up the lines before it: the last one that is not blank, not a
    // comment line and not a `+` line with only a comment ends with `=`
    while line_start > 0 && b[line_start - 1] == b'\n' {
        let end = line_start - 1;
        let begin = b[..end]
            .iter()
            .rposition(|&c| c == b'\n')
            .map_or(0, |n| n + 1);
        let line = &b[begin..end];
        let from = line.iter().position(|c| !matches!(c, b' ' | b'\t'));
        line_start = begin;
        if from.is_none_or(|n| line[n] == b'*') {
            continue;
        }
        // up to a `$` comment
        let mut inside = false;
        let cut = line
            .iter()
            .position(|&c| {
                if c == b'\'' {
                    inside = !inside;
                }
                c == b'$' && !inside
            })
            .unwrap_or(line.len());
        let text = line[..cut].trim_ascii();
        if text.is_empty() || text == b"+" {
            continue;
        }
        return text.ends_with(b"=");
    }
    false
}
