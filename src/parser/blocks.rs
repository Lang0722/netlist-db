//! A scan of the blocks of a file (`.subckt`, `.data`, `.lib` and their ends), in
//! linear time, so that a statement that fails can skip its whole block.
use std::cell::OnceCell;

use nom::{Input, Parser, character::char};

use crate::span::LocatedSpan;

use super::statement::{
    Body, canonical_cmd, dot_cmd, encrypted_block, lib, skip_blank, skip_statement, statement_end,
    unprot,
};
use super::tokens::{key, name_str, path_str, space1};

/// A block statement: a first line, a body, and a closing command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Block {
    /// `.subckt` … `.ends`, or `.macro` … `.eom` (the keywords mix)
    Subckt,
    /// `.data` … `.enddata`
    Data,
    /// `.lib <section>` … `.endl`
    Lib,
}

impl Block {
    /// The block statement that starts at `i`, if any.
    pub(super) fn at(i: LocatedSpan<'_>) -> Option<Self> {
        let cmd = dot_cmd(i)?.to_ascii_lowercase();
        match canonical_cmd(&cmd) {
            "subckt" | "macro" => Some(Self::Subckt),
            "data" => Some(Self::Data),
            "lib" if lib_section(i) => Some(Self::Lib),
            _ => None,
        }
    }

    /// True when `cmd` (a dot-command keyword) closes the block.
    fn closes(self, cmd: &str) -> bool {
        match self {
            Self::Subckt => cmd.eq_ignore_ascii_case("ends") || cmd.eq_ignore_ascii_case("eom"),
            Self::Data => canonical_cmd(&cmd.to_ascii_lowercase()) == "enddata",
            Self::Lib => cmd.eq_ignore_ascii_case("endl"),
        }
    }
}

/// The input after a statement at `i` (with the dot-command keyword `cmd`)
/// whose text the main parse does not read: a `.prot` region (to its
/// `.unprot` line, or to the end of input) when `skip_prot`, else a
/// `.prot freelib` block, or a `.lib <section>` definition that parses (to its
/// `.endl` line). `None` for other statements. So the pass of
/// `BlockLines::scan` reads the file as the main parse does: a `.end`,
/// `.subckt` or `.ends` line in such text is not a statement.
fn unparsed<'a>(i: LocatedSpan<'a>, cmd: &str, skip_prot: bool) -> Option<LocatedSpan<'a>> {
    let lower = cmd.to_ascii_lowercase();
    if canonical_cmd(&lower) == "protect" {
        let (after, _) = (char('.'), key).parse_complete(i).ok()?;
        if !skip_prot {
            return encrypted_block(after).map(|(rest, _)| rest);
        }
        return Some(unprot(after).map_or_else(|| i.take_from(i.len()), |(next, _)| next));
    }
    if canonical_cmd(&lower) == "lib"
        && let Ok((rest, (_, _, _, None))) = (char('.'), key, space1, lib).parse_complete(i)
    {
        return statement_end(rest).ok();
    }
    None
}

/// True when `i` is a `.lib <section>` definition (one word after `.lib`),
/// not a `.lib <file> <section>` include.
fn lib_section(i: LocatedSpan<'_>) -> bool {
    (char('.'), key, space1, path_str)
        .parse_complete(i)
        .is_ok_and(|(i, _)| statement_end(i).is_ok())
}

/// The opening and closing lines of the blocks of one file, for the skip of
/// failed block statements. The lines of one kind of block are found in one
/// pass over the file, on the first skip that needs them.
/// Then each skip is a look-up, so the skips of a file take linear time
/// together, however many blocks fail.
///
/// In a top file, the pass stops at the first `.end` statement: HSPICE
/// reads no line after it (golden `after_end`, `after_end_second_deck`),
/// so no skip passes it. In other files, HSPICE ignores `.end` (golden
/// `end_in_include`), and so does the pass. Like the main parse, the pass
/// does not read the text of a `.prot` region or of a `.lib <section>`
/// definition that parses (see `unparsed`): a `.end`, `.subckt` or `.ends`
/// line there is not a statement.
pub struct Blocks<'a> {
    /// The whole file.
    file: LocatedSpan<'a>,
    /// The file is a top file: the blocks end at its `.end`.
    top: bool,
    /// `ParseOptions::skip_prot`
    pub(super) skip_prot: bool,
    /// One entry for each `Block`, in the order of its variants.
    lines: [OnceCell<BlockLines<'a>>; 3],
}

/// The opening and closing lines of one kind of block, in file order.
pub(super) struct BlockLines<'a> {
    lines: Vec<BlockLine<'a>>,
    /// `min_level[k]` is the lowest `level` in `lines[k..]`.
    min_level: Vec<isize>,
    /// The `.end` statement where the pass stopped (top files only).
    end: Option<LocatedSpan<'a>>,
}

struct BlockLine<'a> {
    /// The offset of the line's `.` in the file.
    start: usize,
    /// The input after the line's logical line.
    after: LocatedSpan<'a>,
    /// The number of blocks open after this line, counted from the start of
    /// the file (below 0 after more closing lines than opening lines).
    level: isize,
    /// For an opening line: its closing line (an index in `lines`), matched
    /// by nesting.
    close: Option<usize>,
    /// The name after `.subckt` or `.ends` (`None` for other blocks).
    name: Option<&'a str>,
}

impl<'a> Blocks<'a> {
    /// The blocks of `file`, which is not a top file.
    pub fn new(file: LocatedSpan<'a>) -> Self {
        Self {
            file,
            top: false,
            skip_prot: false,
            lines: Default::default(),
        }
    }

    /// The blocks of the top file `file`, up to its `.end`.
    pub fn new_top(file: LocatedSpan<'a>) -> Self {
        Self {
            top: true,
            ..Self::new(file)
        }
    }

    /// The blocks with `ParseOptions::skip_prot` set to `skip_prot`.
    pub fn skip_prot(self, skip_prot: bool) -> Self {
        Self { skip_prot, ..self }
    }

    /// Skips a block statement whose start at `i` failed: its first logical
    /// line and its body up to and including the logical line of its
    /// closing command. So the body does not become part of the parent, and
    /// its closing command does not end the parent. A block of the same kind
    /// in the body nests. Without a closing command, only the first logical
    /// line is skipped.
    ///
    /// `body` is the body that `i` is in. For `.subckt`, the closing `.ends`
    /// must leave one `.ends` for each open body before the end of the file,
    /// or else it belongs to one of them. A named `.ends` decides by its name
    /// first: it is the block's when it names the failed `.subckt`, and the
    /// parent's when it names the parent. For `.data` and `.lib` in a body,
    /// no `.subckt` or `.ends` line may lie between the first line and the
    /// closing line, so the skip does not pass the body's `.ends`.
    ///
    /// Known limit: the name check looks only at the failed `.subckt` and
    /// at the body that it is in. A `.ends` that names a body further out
    /// falls back to the count rule. These rules are for ill-formed decks
    /// only: a correct `.ends` ends its body by position, not by name, and
    /// no golden case shows what HSPICE does with a wrong `.ends <name>`.
    pub(super) fn skip(
        &self,
        i: LocatedSpan<'a>,
        block: Block,
        body: Option<Body<'a>>,
    ) -> LocatedSpan<'a> {
        let close = match block {
            Block::Subckt => self.lines(block).close(i, body),
            Block::Data | Block::Lib => self.lines(block).close(i, None).filter(|close| {
                body.is_none()
                    || !self
                        .lines(Block::Subckt)
                        .any_between(i.location_offset(), close.start)
            }),
        };
        close.map_or_else(|| skip_statement(i), |close| close.after)
    }

    /// Skips a `.subckt` at `i` that is nested too deep: its first logical
    /// line and its body up to and including its `.ends`, matched by
    /// nesting. Without a `.ends`, every line after it is in its body (the
    /// bodies around it have no `.ends` either), so the skip goes to the
    /// `.end` of a top file, else to the end of input. Unlike `skip`, it
    /// does not recurse: the lines are found by the one pass of `lines`.
    ///
    /// The result is always after `i`, so the parse moves on. The pass
    /// stops at the first `.end` line, as the main parse does, so that
    /// `.end` comes after `i`. If it does not (the pass and the main parse
    /// read the file in other ways), the skip goes to the end of input: a
    /// jump back would read the same lines again and again.
    pub(super) fn skip_nested(&self, i: LocatedSpan<'a>) -> LocatedSpan<'a> {
        let lines = self.lines(Block::Subckt);
        let at = i.location_offset();
        lines
            .lines
            .binary_search_by_key(&at, |line| line.start)
            .ok()
            .and_then(|e| lines.lines[e].close)
            .map(|m| lines.lines[m].after)
            .or(lines.end)
            .filter(|next| next.location_offset() > at)
            .unwrap_or_else(|| i.take_from(i.len()))
    }

    /// The lines of one kind of block, found on the first call.
    fn lines(&self, block: Block) -> &BlockLines<'a> {
        self.lines[block as usize]
            .get_or_init(|| BlockLines::scan(self.file, block, self.top, self.skip_prot))
    }
}

#[cfg(test)]
thread_local! {
    /// The number of `BlockLines::scan` passes on this thread.
    pub(super) static BLOCK_SCANS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

impl<'a> BlockLines<'a> {
    /// The lines of `block` in `file`, up to its first `.end` statement
    /// when `top`.
    fn scan(file: LocatedSpan<'a>, block: Block, top: bool, skip_prot: bool) -> Self {
        #[cfg(test)]
        BLOCK_SCANS.with(|n| n.set(n.get() + 1));
        let mut lines: Vec<BlockLine<'a>> = Vec::new();
        // the opening lines that are not closed yet
        let mut open = Vec::new();
        let mut level = 0;
        let mut end = None;
        let mut j = skip_blank(file);
        while !j.is_empty() {
            let cmd = dot_cmd(j);
            if let Some(next) = cmd.and_then(|c| unparsed(j, c, skip_prot)) {
                j = skip_blank(next);
                continue;
            }
            // every `.end` line ends a top file, as in the main parse
            if top && cmd.is_some_and(|c| c.eq_ignore_ascii_case("end")) {
                end = Some(j);
                break;
            }
            let opens = Block::at(j) == Some(block);
            let closes = !opens && cmd.is_some_and(|c| block.closes(c));
            let start = j;
            j = skip_statement(j);
            if opens || closes {
                if opens {
                    level += 1;
                    open.push(lines.len());
                } else {
                    level -= 1;
                    if let Some(k) = open.pop() {
                        lines[k].close = Some(lines.len());
                    }
                }
                let name = if block == Block::Subckt {
                    (char('.'), key, space1, name_str)
                        .parse_complete(start)
                        .ok()
                        .map(|(_, (_, _, _, (name, _)))| name)
                } else {
                    None
                };
                lines.push(BlockLine {
                    start: start.location_offset(),
                    after: j,
                    level,
                    close: None,
                    name,
                });
            }
            j = skip_blank(j);
        }
        let mut min_level = vec![0; lines.len()];
        let mut low = isize::MAX;
        for (k, line) in lines.iter().enumerate().rev() {
            low = low.min(line.level);
            min_level[k] = low;
        }
        Self {
            lines,
            min_level,
            end,
        }
    }

    /// True when a line starts after offset `from` and before offset `to`.
    fn any_between(&self, from: usize, to: usize) -> bool {
        let k = self.lines.partition_point(|line| line.start <= from);
        self.lines.get(k).is_some_and(|line| line.start < to)
    }

    /// The closing line of the block at `i`, or `None` when the block has
    /// no closing line of its own. `body` is the `.subckt` body that `i` is
    /// in, for `.subckt` blocks only.
    fn close(&self, i: LocatedSpan<'a>, body: Option<Body<'a>>) -> Option<&BlockLine<'a>> {
        let e = self
            .lines
            .binary_search_by_key(&i.location_offset(), |line| line.start)
            .ok()?;
        let m = self.lines[e].close?;
        let close = &self.lines[m];
        let Some(body) = body else {
            return Some(close);
        };
        let names = |name: Option<&str>| {
            close
                .name
                .zip(name)
                .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
        };
        if names(self.lines[e].name) {
            return Some(close);
        }
        if names(Some(body.name)) {
            return None;
        }
        // The level after `close` is the level before the failed line. The
        // open bodies close when the level goes `depth` lower.
        let target = close.level - body.depth as isize;
        self.min_level
            .get(m + 1)
            .is_some_and(|&low| low <= target)
            .then_some(close)
    }
}
