//! Lowering: from the parser's builders (which keep file positions) to the
//! owned model.
//!
//! The walk follows the segments of each file in document order, and an
//! `.include`/`.lib` segment into the builders of the included file, so the
//! statements of a file included inside a `.subckt` belong to that
//! definition (golden `include_in_subckt`). Each statement is visited once
//! per place it appears, so the work is linear in the size of the input.
//!
//! The statements of a file are lowered in line order (element lines,
//! `.subckt` bodies and the rest interleaved), so the diagnostics of a file
//! are in line order, and each name keeps the spelling of its first
//! appearance in the deck.
//!
//! `X` lines are not bound to definitions here: HSPICE binds them along
//! the instance path (golden `nested_def_dynamic`), so the binding is a
//! query-time job. Only the callee name is kept.

use crate::{
    DefLookup, LoadOptions, Netlist,
    api::{Level, LoadError},
    intern::{Interner, Sym},
    model::{
        Def, DefId, Diag, ElemKind, Element, FuncDef, LocIdx, ModelDef, ParamDef, Parhier, ROOT,
        Range32, Root, SourceFile, Swept, TextRef,
    },
};
use netlist_db::{
    FileId, Files, ParseError, ParseErrorInner, ParseOptions, Parsed, Pos, Span,
    ast::{
        ASTBuilder, DataValuesBuilder, KeyValueBuilder, LocalAST, ModelBuilder, ModelTypeBuilder,
        ParamFuncBuilder, Segment, SubcktBuilder, TokenBuilder, UnknwonBuilder, ValueBuilder,
    },
    instance::{
        BJT_POSITIONAL, ControlledKind, CurrentBuilder, DIODE_POSITIONAL, InstanceBuilder,
        InstanceCtxBuilder, MOS_POSITIONAL, RCL_POSITIONAL, SourceBuilder, VoltageBuilder,
    },
    parser::{MAX_NESTING, parse_top_multi_with},
};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, hash_map::Entry},
};

/// The analysis and output commands that the parser keeps without an
/// error. The queries do not use them (except `.temp`).
const ANALYSIS: [&str; 15] = [
    "tran", "op", "dc", "ac", "temp", "print", "probe", "meas", "measure", "hdl", "save", "tf",
    "noise", "sens", "four",
];

/// The analysis commands whose sweeps parameter resolution names in a
/// caveat (spec 6.2a). Not `.step`: HSPICE P-2019.06 ignores it (golden
/// `step_param`, `step_plain`, `step_lin`).
const SWEEPING: [&str; 4] = ["dc", "tran", "ac", "noise"];

/// The longest statement text quoted in a diagnostic, in characters.
const QUOTE_MAX: usize = 120;

/// The model types that the parser knows (see [`model_type`]).
const MODEL_TYPES: [&str; 16] = [
    "amp", "c", "core", "d", "l", "njf", "nmos", "npn", "opt", "pjf", "pmos", "pnp", "r", "u", "w",
    "s",
];

/// The names that lowering adds itself, not from the deck: the value of
/// R/C/L (`r`, `c`, `l`), the parts of a V/I source, `temp`, the names of
/// positional numbers and the model types. The first spelling of one of
/// them in the deck is the spelling that answers show (see
/// [`Interner::with_builtins`]).
fn builtin_names() -> impl Iterator<Item = &'static str> {
    ["r", "c", "l", "dc", "ac", "acphase", "temp"]
        .into_iter()
        .chain(RCL_POSITIONAL)
        .chain(MOS_POSITIONAL)
        .chain(DIODE_POSITIONAL)
        .chain(BJT_POSITIONAL)
        .chain(MODEL_TYPES)
}

impl Netlist {
    /// The text of parameter `p` as answers show it, on one line: without
    /// the `+` markers, `$` comments and `*` comment lines of the file,
    /// except inside quotes (see [`one_line`]; no copy for a text on one
    /// line with no `$`). The
    /// stored text of a source function keeps them (`SIN` = `0 1\n+ 1k`),
    /// so every answer that shows a parameter text takes it from here.
    /// `None`: the parameter has no text.
    pub(crate) fn param_text(&self, p: &ParamDef) -> Option<Cow<'_, str>> {
        p.text.map(|t| self.span_text(t))
    }

    /// The text `t` on one line, as [`Netlist::param_text`] gives it. A
    /// text right after a `'` (the text of a quoted value, stored without
    /// its quotes) starts inside quotes.
    pub(crate) fn span_text(&self, t: TextRef) -> Cow<'_, str> {
        let file = &self.files[t.file as usize].text;
        let quoted = t.start > 0 && file.as_bytes()[t.start as usize - 1] == b'\'';
        one_line(self.text(t), quoted)
    }
}

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
fn raw_quote_fault(t: &str, quoted: bool) -> Option<QuoteFault> {
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

/// Parses and lowers the deck of `opts`.
pub(crate) fn load(opts: &LoadOptions) -> Result<Netlist, LoadError> {
    let shown = opts.path.display();
    match std::fs::metadata(&opts.path) {
        Ok(meta) if meta.is_dir() => {
            return Err(LoadError::Io(format!(
                "\"{shown}\" is a directory, not a netlist file"
            )));
        }
        Ok(_) => {}
        Err(e) => return Err(LoadError::Io(format!("cannot read \"{shown}\": {e}"))),
    }
    let (parsed, files) = parse(opts)?;
    if let Some(e) = top_io_error(&parsed) {
        return Err(LoadError::Io(format!("cannot read \"{shown}\": {e}")));
    }
    lower(parsed, files, opts)
}

/// Builds the model from the parser's output for the deck of `opts`.
fn lower(parsed: Parsed, files: Files, opts: &LoadOptions) -> Result<Netlist, LoadError> {
    let Files { inner: texts } = files;
    if let Some(i) = texts.iter().position(|t| u32::try_from(t.len()).is_err()) {
        return Err(LoadError::Io(format!(
            "\"{}\" is larger than 4 GiB, which is not supported",
            parsed.inner[i].0.path().display()
        )));
    }
    let paths: Vec<String> = parsed
        .inner
        .iter()
        .map(|(id, _)| id.path().display().to_string())
        .collect();
    let mut lower = Lower {
        parsed: &parsed,
        texts: &texts,
        paths: &paths,
        net: Netlist {
            files: Vec::new(),
            distinct_files: 0,
            syms: Interner::with_builtins(builtin_names()),
            defs: Vec::new(),
            def_index: HashMap::new(),
            root: Root::None,
            parhier: Parhier::Global,
            options: Vec::new(),
            globals: Vec::new(),
            temps: Vec::new(),
            alters: Vec::new(),
            diags: Vec::new(),
            hier: Default::default(),
            hdl: false,
            swept: Vec::new(),
            elem_names: Default::default(),
            net_index: Default::default(),
            dotted_instances: Default::default(),
            dspf: Vec::new(),
            skipped: Vec::new(),
            protected: false,
        },
        reported: vec![false; parsed.inner.len()],
        reading: Vec::new(),
        sites: Vec::new(),
        site: None,
        def_site: Vec::new(),
        model_first: HashMap::new(),
        non_utf8_files: HashSet::new(),
        alter: None,
        alter_errors: Vec::new(),
        analysis: Vec::new(),
        data_blocks: Vec::new(),
        sweeps: Vec::new(),
        sweeps_seen: HashSet::new(),
    };
    lower.run(opts)?;
    let mut net = lower.net;
    // `a/../b.sp` and `b.sp` are one file
    let mut canon: Vec<std::path::PathBuf> = parsed
        .inner
        .iter()
        .map(|(id, _)| std::fs::canonicalize(id.path()).unwrap_or_else(|_| id.path().to_owned()))
        .collect();
    canon.sort_unstable();
    canon.dedup();
    net.distinct_files = canon.len();
    net.files = texts
        .into_iter()
        .zip(&parsed.inner)
        .zip(paths)
        .map(|((text, (id, _)), path)| SourceFile {
            path,
            section: match id {
                FileId::Section { section, .. } => Some(section.clone()),
                FileId::Include { .. } => None,
            },
            text,
        })
        .collect();
    net.skipped.sort();
    net.skipped.dedup();
    // the parasitics of included DSPF files are not devices
    let dspf_diags = crate::dspf::attach_included(&mut net);
    net.diags.extend(dspf_diags);
    let (hier, mut diags) = crate::hier::analyze(&net);
    net.hier = hier;
    // the load diagnostics first; the longer list takes the other, so the
    // two are not both whole copies at once
    if diags.len() > net.diags.len() {
        diags.splice(0..0, std::mem::take(&mut net.diags));
        net.diags = diags;
    } else {
        net.diags.extend(diags);
    }
    let dspf_diags = crate::dspf::attach_sim_dspf(&mut net);
    net.diags.extend(dspf_diags);
    Ok(net)
}

/// Runs the parser on a tokio current-thread runtime. Inside a runtime
/// (where `block_on` panics) it runs on a thread of its own.
fn parse(opts: &LoadOptions) -> Result<(Parsed, Files), LoadError> {
    let popts = ParseOptions {
        title_line: opts.title_line,
        lib_override: opts.lib_override.clone(),
        // the loader reports a `.prot` region, and reads none of it
        skip_prot: true,
    };
    let file_id = FileId::Include {
        path: opts.path.clone(),
    };
    let run = move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| LoadError::Io(format!("cannot start the parser: {e}")))?;
        Ok(rt.block_on(parse_top_multi_with(std::iter::once(file_id), &popts)))
    };
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|s| s.spawn(run).join())
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    } else {
        run()
    }
}

/// The error of a top file that could not be read (the parser then gives a
/// single error without a position).
fn top_io_error(parsed: &Parsed) -> Option<&std::io::Error> {
    let top = parsed.top_ids.first()?;
    parsed.inner[top.0]
        .1
        .segments
        .iter()
        .find_map(|seg| match seg {
            Segment::Local(local) => local.errors.iter().find_map(|e| match (&e.err, e.pos) {
                (ParseErrorInner::IO(io), None) => Some(io),
                _ => None,
            }),
            _ => None,
        })
}

/// The state of one lowering.
struct Lower<'p> {
    parsed: &'p Parsed,
    /// The file texts, by `ParsedId`.
    texts: &'p [String],
    /// The file paths, by `ParsedId`.
    paths: &'p [String],
    net: Netlist,
    /// By `ParsedId`: the file was visited. The diagnostics of a file
    /// included twice are reported on its first visit only.
    reported: Vec<bool>,
    /// The files being lowered, the top file first: a file that includes
    /// one of them again is an include cycle.
    reading: Vec<usize>,
    /// Each read of an included file: the include line, and the read that
    /// holds that line (`None`: the top file).
    sites: Vec<(LocIdx, Option<u32>)>,
    /// The read (in `sites`) of the file being lowered; `None` in the top
    /// file.
    site: Option<u32>,
    /// By `DefId`: the read (in `sites`) that gave the definition.
    def_site: Vec<Option<u32>>,
    /// The first `.model` card of each name in each scope, and its read.
    model_first: HashMap<(DefId, Sym), (LocIdx, Option<u32>)>,
    /// The files with a `non_utf8` warning.
    non_utf8_files: HashSet<&'p str>,
    /// The `.alter` block of the statements being lowered.
    alter: Option<u32>,
    /// The `.alter` lines reported as inside a `.subckt`.
    alter_errors: Vec<LocIdx>,
    /// The analysis commands: name, count, and the index in `net.diags`
    /// of the diagnostic at the first one (its text is written at the end).
    analysis: Vec<(&'static str, usize, usize)>,
    /// The `.data` blocks of the first run: name, parameter names, line.
    data_blocks: Vec<(Sym, Vec<Sym>, LocIdx)>,
    /// What the analysis lines of the first run sweep: a name, or the
    /// `.data` block of a `data=` (`true`), with the command and the line.
    sweeps: Vec<(Sym, bool, &'static str, LocIdx)>,
    /// The items of `sweeps`, for the check that each is there once (a
    /// file included twice is read twice).
    sweeps_seen: HashSet<(Sym, bool, &'static str, LocIdx)>,
}

impl<'p> Lower<'p> {
    fn run(&mut self, opts: &LoadOptions) -> Result<(), LoadError> {
        let parsed = self.parsed;
        let top = parsed.top_ids[0].0;
        let name = self.net.syms.intern("");
        self.net.defs.push(new_def(
            name,
            None,
            LocIdx {
                file: top as u32,
                line: 1,
            },
            None,
        ));
        self.def_site.push(None);
        let top_ast = &parsed.inner[top].1;
        if let Some(title) = &top_ast.title {
            let text = &self.texts[top];
            let line = &text[title.line.start..title.line.end];
            let message = format!("{}. Line: `{}`", title.message, shorten(line.trim()));
            let loc = LocIdx {
                file: top as u32,
                line: 1,
            };
            self.diag(Level::Info, "title_line", message, Some(loc));
        }
        self.reported[top] = true;
        self.reading.push(top);
        self.non_utf8(top_ast, top);
        self.walk(top_ast, top, ROOT, 0, true);
        self.reading.pop();
        self.net.defs[ROOT.idx()].shrink();
        if let Some(end) = top_ast.after_end
            && end.lines > 0
        {
            let (lines, were, they) = if end.lines == 1 {
                ("line", "was", "it does")
            } else {
                ("lines", "were", "they do")
            };
            let message = format!(
                "{} {lines} after `.end` {were} not read: {they} not join this deck, as in HSPICE. A second deck after `.end` (HSPICE runs it as a second job) is not loaded",
                end.lines
            );
            let loc = LocIdx {
                file: top as u32,
                line: end.line,
            };
            self.diag(Level::Info, "after_end", message, Some(loc));
        }

        for (cmd, count, at) in std::mem::take(&mut self.analysis) {
            let lines = if count == 1 { "line" } else { "lines" };
            let note = if cmd == "temp" {
                " (its first value is the temperature that `temper` gives)"
            } else {
                ""
            };
            self.net.diags[at].message = format!(
                "{count} `.{cmd}` {lines}: analysis and output commands are not simulated by the queries{note}"
            );
        }
        // the names that the analyses sweep, also through a `.data` block
        let mut blocks: HashMap<Sym, Vec<usize>> = HashMap::new();
        for (k, (block, ..)) in self.data_blocks.iter().enumerate() {
            blocks.entry(*block).or_default().push(k);
        }
        for (name, data, cmd, loc) in std::mem::take(&mut self.sweeps) {
            if !data {
                self.net.swept.push(Swept {
                    name,
                    cmd,
                    loc,
                    data: None,
                });
                continue;
            }
            for &k in blocks.get(&name).map_or(&[][..], Vec::as_slice) {
                let (_, params, at) = &self.data_blocks[k];
                self.net.swept.extend(params.iter().map(|&p| Swept {
                    name: p,
                    cmd,
                    loc,
                    data: Some((name, *at)),
                }));
            }
        }
        // sorted by name for the lookup of `swept_caveats`
        self.net.swept.sort_by_key(|s| s.name);
        for (o, matched) in opts.lib_override.iter().zip(&parsed.lib_override_matched) {
            if !matched {
                let message = format!(
                    "the override of section `{}` of \"{}\" (to `{}`) matched no `.lib` call: it changed nothing",
                    o.from,
                    o.file.display(),
                    o.to
                );
                self.diag(Level::Warning, "lib_override_unused", message, None);
            }
        }
        self.net.root = match &opts.top {
            Some(name) => Root::Subckt(find_top(&self.net, name)?),
            None if self.net.defs[ROOT.idx()]
                .elements
                .iter()
                .any(|e| e.alter.is_none()) =>
            {
                Root::TopLevel
            }
            None => Root::None,
        };
        Ok(())
    }

    fn diag(&mut self, level: Level, code: &'static str, message: String, loc: Option<LocIdx>) {
        self.net.diags.push(Diag {
            level,
            code,
            message,
            loc,
        });
    }

    /// The Error for the value text `t` of a parameter on line `loc` when
    /// HSPICE rejects the deck for it: a line inside quotes that is not a
    /// `+` line, a `*` comment line inside open parentheses, or a `$` inside
    /// quotes (see [`one_line`], [`quote_fault`]). Parameter resolution
    /// gives no value for such a text.
    fn quote_check(&mut self, t: TextRef, loc: LocIdx) {
        let text: &'p str = &self.texts[t.file as usize];
        let (start, end) = (t.start as usize, t.end as usize);
        let raw = &text[start..end];
        if !raw.contains(['\n', '$']) {
            return;
        }
        let quoted = start > 0 && text.as_bytes()[start - 1] == b'\'';
        let (code, what) = match raw_quote_fault(raw, quoted) {
            None => return,
            Some(QuoteFault::CommentLine) => (
                "quoted_comment_line",
                "a line inside quotes that is not a `+` line (a `*` or `$` comment line, a blank line, or a line with only spaces): HSPICE P-2019.06 does not go on with the quoted text on the `+` line after it, and it rejects this deck (golden `quoted_cmtline`, `quoted_dollarline_elem`, `quoted_blank_line`, `quoted_spaces_elem`: \"string misses dexter quotation mark\"). A `+` line with no text inside quotes is passed over (golden `quoted_plus_empty_elem`), and outside quotes a `+` line after a comment line or a blank line goes on with the statement (golden `plus_after_cmtline`, `plus_after_blank`)",
            ),
            Some(QuoteFault::ParenCommentLine) => (
                "paren_comment_line",
                "a `*` comment line inside open parentheses with no quotes: HSPICE P-2019.06 reads the comment line as a part of the expression, and it rejects this deck (golden `paren_cmtline_call`: \"Illegal character in statement\"). A blank line there is passed over (golden `paren_blank_call`)",
            ),
            Some(QuoteFault::Dollar) => (
                "quoted_dollar",
                "a `$` inside quotes: HSPICE P-2019.06 does not read it as the start of a comment there, and it rejects this deck (golden `rcl_quoted_dollar`: \"Undefined parameter or function definition\"; `rcl_quoted_dollar_line`: \"Illegal character in statement\")",
            ),
        };
        let message = format!(
            "{what}. Answers give no value for this text. Line: `{}`",
            line_at(text, start)
        );
        self.diag(Level::Error, code, message, Some(loc));
    }

    /// `path:line` of `loc`.
    fn loc_str(&self, loc: LocIdx) -> String {
        format!("{}:{}", self.paths[loc.file as usize], loc.line)
    }

    /// Lowers the segments of `ast` (a file, or a `.subckt` body) of file
    /// `pid` into `def`. `depth` is the number of `.subckt` bodies around
    /// it; `report` is false on a later visit of the file.
    fn walk(&mut self, ast: &'p ASTBuilder, pid: usize, def: DefId, depth: usize, report: bool) {
        let parsed = self.parsed;
        for seg in &ast.segments {
            match seg {
                Segment::Local(local) => self.local(local, pid, def, depth, report),
                Segment::Include { res, pos } => match res.get() {
                    // The parser reads a file once, so it does not see an
                    // include cycle: this walk, in document order, finds it
                    // (the same one in every run) and does not follow it.
                    Some(Ok(id)) if self.reading.contains(&id.0) => {
                        if report {
                            self.include_cycle(pid, id.0, *pos);
                        }
                    }
                    Some(Ok(id)) => {
                        let first = !std::mem::replace(&mut self.reported[id.0], true);
                        let at = LocIdx {
                            file: pid as u32,
                            line: pos.map_or(0, |p| p.line_num),
                        };
                        let outer = self.site;
                        let n = u32::try_from(self.sites.len()).expect("fewer than 2^32 includes");
                        self.sites.push((at, outer));
                        self.site = Some(n);
                        self.reading.push(id.0);
                        let file = &parsed.inner[id.0].1;
                        if first {
                            self.non_utf8(file, id.0);
                        }
                        self.walk(file, id.0, def, depth, first);
                        self.reading.pop();
                        self.site = outer;
                    }
                    Some(Err(e)) => {
                        if report {
                            self.parse_error(e, pid);
                        }
                    }
                    None => debug_assert!(false, "an include that the parser did not finish"),
                },
                // reached through an `.include` in a `.subckt` body (the
                // parser rejects `.alter` in the body itself)
                Segment::Alter { pos, .. } if def != ROOT => self.alter_in_subckt(*pos, pid, def),
                Segment::Alter { pos, .. } => {
                    let n = self.net.alters.len() as u32 + 1;
                    let loc = LocIdx {
                        file: pid as u32,
                        line: pos.map_or(0, |p| p.line_num),
                    };
                    self.net.alters.push(loc);
                    self.alter = Some(n);
                    let message = format!(
                        ".alter #{n}: answers describe the first run only; the statements of this block (up to the next `.alter`) are not applied"
                    );
                    self.diag(Level::Info, "alter_block", message, Some(loc));
                }
            }
        }
    }

    /// Keeps the `.data` blocks of `local` (file `pid`): their names and
    /// parameter names, for the sweeps of the analyses.
    fn data_blocks(&mut self, local: &LocalAST, pid: usize) {
        let text: &str = &self.texts[pid];
        for d in &local.data {
            let name = self.net.syms.intern(&text[d.name.start..d.name.end]);
            let spans: Vec<&Span> = match &d.values {
                DataValuesBuilder::InlineExpr { params, .. }
                | DataValuesBuilder::InlineNum { params, .. } => params.iter().collect(),
                DataValuesBuilder::MER(f) | DataValuesBuilder::LAM(f) => f
                    .files
                    .iter()
                    .flat_map(|file| file.pname_col_num.iter().map(|p| &p.pname))
                    .collect(),
            };
            let params = spans
                .into_iter()
                .map(|sp| self.net.syms.intern(&text[sp.start..sp.end]))
                .collect();
            let loc = LocIdx {
                file: pid as u32,
                line: d.name.line_num,
            };
            self.data_blocks.push((name, params, loc));
        }
    }

    /// Warns, at its first bad line, that file `pid` (with the AST `ast`)
    /// has bytes that are not UTF-8. One warning for each file: a file
    /// read for two `.lib` sections gives one, for the first section with
    /// such bytes.
    fn non_utf8(&mut self, ast: &ASTBuilder, pid: usize) {
        let paths = self.paths;
        if let Some(line) = ast.non_utf8
            && self.non_utf8_files.insert(&paths[pid])
        {
            let message = format!(
                "\"{}\" has bytes that are not UTF-8 (the first that was read is on this line): they were read as U+FFFD (\u{FFFD}) and the statements were kept, but a name or a value with such a byte may not match its other spellings",
                self.paths[pid]
            );
            let loc = LocIdx {
                file: pid as u32,
                line,
            };
            self.diag(Level::Warning, "non_utf8", message, Some(loc));
        }
    }

    /// When the statements at `a` and `b` are one line read twice (by the
    /// reads `a_site` and `b_site` of their file): the include lines that
    /// read the file, in words. Each read has a chain of include lines
    /// from the top file; only the lines where the two chains differ are
    /// named (`a.sp` included twice from `main.sp`, and `sub.sp` from
    /// `a.sp`: "through the include line at main.sp:2 and again through
    /// the include line at main.sp:3"), so no line is named twice.
    fn read_twice(
        &self,
        a: LocIdx,
        b: LocIdx,
        a_site: Option<u32>,
        b_site: Option<u32>,
    ) -> Option<String> {
        if a != b {
            return None;
        }
        // the reads from the top file in
        let chain = |mut site: Option<u32>| {
            let mut reads = Vec::new();
            while let Some(k) = site {
                reads.push(k);
                site = self.sites[k as usize].1;
            }
            reads.reverse();
            reads
        };
        let (ca, cb) = (chain(a_site), chain(b_site));
        // where the chains part (neither is a prefix of the other: a file
        // that includes itself is an include cycle, which is not followed)
        let k = ca.iter().zip(&cb).position(|(x, y)| x != y)?;
        let lines = |c: &[u32]| -> Vec<LocIdx> {
            c[k..].iter().map(|&r| self.sites[r as usize].0).collect()
        };
        let (la, lb) = (lines(&ca), lines(&cb));
        // the lines of `own` that `other` does not have, in words
        let named = |own: &[LocIdx], other: &[LocIdx]| {
            let mut only: Vec<String> = own
                .iter()
                .filter(|l| !other.contains(l))
                .map(|&l| self.loc_str(l))
                .collect();
            if only.is_empty() {
                // not expected: two reads part at two include lines of one
                // read of a file
                only.push(self.loc_str(own[0]));
            }
            match only.split_last() {
                Some((last, [])) => format!("the include line at {last}"),
                Some((last, rest)) => {
                    format!("the include lines at {} and {last}", rest.join(", "))
                }
                None => unreachable!("one line at least"),
            }
        };
        Some(format!(
            "\"{}\" is read twice: through {} and again through {} (HSPICE reads a file again at each `.include` or `.lib` call)",
            self.paths[a.file as usize],
            named(&la, &lb),
            named(&lb, &la)
        ))
    }

    /// Reports that file `pid` includes file `again` (at `pos`), which is
    /// being lowered (it includes `pid`, directly or not).
    fn include_cycle(&mut self, pid: usize, again: usize, pos: Option<Pos>) {
        // a `.lib` section by its file and its name
        let shown = |f: usize| file_id_str(&self.parsed.inner[f].0);
        let from = self.reading.iter().position(|&f| f == again).unwrap_or(0);
        let chain: Vec<String> = self.reading[from..]
            .iter()
            .chain([&again])
            .map(|&f| format!("\"{}\"", shown(f)))
            .collect();
        let message = format!(
            "circular include: {}. The include of \"{}\" in \"{}\" was skipped",
            chain.join(" -> "),
            shown(again),
            shown(pid)
        );
        let loc = pos.map(|p| LocIdx {
            file: pid as u32,
            line: p.line_num,
        });
        self.diag(Level::Error, "include_cycle", message, loc);
    }

    /// Reports an `.alter` line of file `pid` that an `.include` brings into
    /// the body of `def`, once per line. The line is ignored: the
    /// statements after it stay in the run they are in.
    fn alter_in_subckt(&mut self, pos: Option<Pos>, pid: usize, def: DefId) {
        let loc = LocIdx {
            file: pid as u32,
            line: pos.map_or(0, |p| p.line_num),
        };
        if self.alter_errors.contains(&loc) {
            return;
        }
        self.alter_errors.push(loc);
        let message = format!(
            "`.alter` inside the body of subckt `{}` (through an `.include`): `.alter` belongs at the top level of the deck. The `.alter` line was ignored, so the statements after it stay in the first run",
            self.net.def_display(def)
        );
        self.diag(Level::Error, "alter_in_subckt", message, Some(loc));
    }

    /// Lowers one `LocalAST` in line order: its statements, its `.subckt`
    /// definitions (with their bodies), its errors and its protected
    /// regions. So the diagnostics of a file are in line order, and a name
    /// gets the spelling of its first appearance.
    fn local(&mut self, local: &'p LocalAST, pid: usize, def: DefId, depth: usize, report: bool) {
        if self.alter.is_none() {
            self.data_blocks(local, pid);
            // the element lines that an error skipped (a line that starts
            // with a letter); their pins are unknown
            let text: &str = &self.texts[pid];
            for e in &local.errors {
                let Some(p) = e.pos else { continue };
                let line = line_at(text, p.start);
                if line.starts_with(|c: char| c.is_ascii_alphabetic())
                    && let Some(name) = line.split_whitespace().next()
                {
                    self.net.skipped.push((def, name.to_ascii_lowercase()));
                }
            }
        }
        // (line, error, protected region or ignored `.end`)
        let mut notes: Vec<(u32, Note<'p>)> = Vec::new();
        if report {
            notes.extend(
                local
                    .errors
                    .iter()
                    .map(|e| (e.pos.map_or(0, |p| p.line_num), Note::Error(e))),
            );
            notes.extend(
                local
                    .protected
                    .iter()
                    .map(|&(a, b)| (a, Note::Protected(a, b))),
            );
            notes.extend(local.ignored_end.iter().map(|&l| (l, Note::EndInInclude)));
            notes.sort_by_key(|(line, _)| *line);
        }
        // the line of statement `i` of kind `k`; each kind is in line order
        let line = |k: Kind, i: usize| -> Option<u32> {
            match k {
                Kind::Note => notes.get(i).map(|n| n.0),
                Kind::Subckt => local.subckt.get(i).map(|s| s.name.line_num),
                Kind::Element => local.instance.get(i).map(|e| e.name.line_num),
                Kind::Model => local.model.get(i).map(|m| m.name.line_num),
                Kind::Param => local.param.get(i).map(|kv| kv.k.line_num),
                Kind::Func => local.param_func.get(i).map(|f| f.name.line_num),
                Kind::Global => local.global.get(i).map(|g| g.line_num),
                Kind::Option => local.option.get(i).map(|(k, _)| k.line_num),
                Kind::Command => local.unknwon.get(i).map(|c| c.cmd.line_num),
            }
        };
        // a merge of the kinds: the next statement of each kind
        let mut next = [0usize; KINDS.len()];
        loop {
            // the kind with the first next statement, and the line of the
            // next statement of the other kinds
            let mut first: Option<(u32, usize)> = None;
            let mut then = u32::MAX;
            for (k, &kind) in KINDS.iter().enumerate() {
                let Some(l) = line(kind, next[k]) else {
                    continue;
                };
                match first {
                    Some((fl, _)) if fl <= l => then = then.min(l),
                    _ => {
                        if let Some((fl, _)) = first {
                            then = then.min(fl);
                        }
                        first = Some((l, k));
                    }
                }
            }
            let Some((_, k)) = first else {
                break;
            };
            // the statements of kind `k` up to `then`, in one run
            while let Some(l) = line(KINDS[k], next[k])
                && l <= then
            {
                let i = next[k];
                next[k] += 1;
                match KINDS[k] {
                    Kind::Note => self.note(notes[i].1, notes[i].0, pid),
                    Kind::Subckt => self.subckt(&local.subckt[i], pid, def, depth, report),
                    Kind::Element => self.element(&local.instance[i], pid, def, report),
                    Kind::Model => self.model(&local.model[i], pid, def),
                    Kind::Param => {
                        let text: &'p str = &self.texts[pid];
                        let alter = self.alter;
                        let p =
                            kv_param(&mut self.net.syms, text, pid as u32, &local.param[i], alter);
                        if report && let Some(t) = p.text {
                            self.quote_check(t, p.loc);
                        }
                        self.net.defs[def.idx()].params.push(p);
                    }
                    Kind::Func => self.func(&local.param_func[i], pid, def, report),
                    Kind::Global => {
                        if self.alter.is_none() {
                            let g = &local.global[i];
                            let sym = self.net.syms.intern(&self.texts[pid][g.start..g.end]);
                            if !self.net.globals.contains(&sym) {
                                self.net.globals.push(sym);
                            }
                        }
                    }
                    Kind::Option => {
                        let (key, value) = &local.option[i];
                        self.option(key, value.as_ref(), pid, report);
                    }
                    Kind::Command => self.command(&local.unknwon[i], pid, report),
                }
            }
        }
    }

    /// Reports `note`, at line `line` of file `pid`.
    fn note(&mut self, note: Note<'_>, line: u32, pid: usize) {
        match note {
            Note::Error(e) => self.parse_error(e, pid),
            Note::EndInInclude => {
                // golden `end_in_include`: HSPICE warns `'.end' can not
                // exist in include/lib files, ignored`
                let message = format!(
                    "`.end` in \"{}\", which is an included file (or a `.lib` section): HSPICE ignores it, with a warning, and reads the lines after it. So does the loader",
                    self.paths[pid]
                );
                let loc = LocIdx {
                    file: pid as u32,
                    line,
                };
                self.diag(Level::Warning, "end_in_include", message, Some(loc));
            }
            Note::Protected(a, b) => {
                let message = format!(
                    "lines {a}-{b} of {}: protected (`.prot`), not parsed; definitions, elements and counts inside are missing",
                    self.paths[pid]
                );
                let loc = LocIdx {
                    file: pid as u32,
                    line: a,
                };
                self.net.protected = true;
                self.diag(Level::Warning, "protected_region", message, Some(loc));
            }
        }
    }

    /// Adds the definition `s` (in file `pid`) inside `parent`, and lowers
    /// its body.
    fn subckt(
        &mut self,
        s: &'p SubcktBuilder,
        pid: usize,
        parent: DefId,
        depth: usize,
        report: bool,
    ) {
        let texts = self.texts;
        let text: &'p str = &texts[pid];
        let name_str = &text[s.name.start..s.name.end];
        let loc = LocIdx {
            file: pid as u32,
            line: s.name.line_num,
        };
        if depth >= MAX_NESTING {
            // only an include chain gets here: the parser limits each file
            if report {
                let message = format!(
                    "`{name_str}` is nested deeper than {MAX_NESTING} definitions (through `.include` lines): its definition is skipped"
                );
                self.diag(Level::Error, "nesting_too_deep", message, Some(loc));
            }
            return;
        }
        let name = self.net.syms.intern(name_str);
        let id = DefId(u32::try_from(self.net.defs.len()).expect("fewer than 2^32 definitions"));
        let mut d = new_def(name, (parent != ROOT).then_some(parent), loc, self.alter);
        d.text = tref(pid as u32, s.text.start, s.text.end);
        d.text_line = s.text.line_num;
        d.ports = s
            .ports
            .iter()
            .map(|p| self.net.syms.intern(&text[p.start..p.end]))
            .collect();
        d.defaults = s
            .params
            .iter()
            .map(|kv| kv_param(&mut self.net.syms, text, pid as u32, kv, self.alter))
            .collect();
        if report {
            for p in &d.defaults {
                if let Some(t) = p.text {
                    self.quote_check(t, p.loc);
                }
            }
        }
        self.net.defs.push(d);
        self.def_site.push(self.site);
        self.net.defs[parent.idx()].nested.push(id);
        // a definition in an `.alter` block is not a first-run definition
        if self.alter.is_none() {
            match self.net.def_index.entry((parent, name)) {
                Entry::Vacant(v) => {
                    v.insert(id);
                }
                Entry::Occupied(o) => {
                    let first = *o.get();
                    self.duplicate(id, first, parent);
                }
            }
        }
        self.walk(&s.ast, pid, id, depth + 1, report);
        self.net.defs[id.idx()].shrink();
    }

    /// Records that `dup` has the name of `first`, in the same scope.
    /// HSPICE rejects the deck at the top level (golden `dup_subckt`) and
    /// inside a definition (`hier_dup_nested`). The message names the two
    /// include lines when one definition is read twice (golden
    /// `inc_twice`, `lib_twice`).
    fn duplicate(&mut self, dup: DefId, first: DefId, parent: DefId) {
        self.net.defs[dup.idx()].dup_of = Some(first);
        self.net.defs[first.idx()].has_dup = true;
        let first_loc = self.net.defs[first.idx()].loc;
        let loc = self.net.defs[dup.idx()].loc;
        let what = self
            .read_twice(
                first_loc,
                loc,
                self.def_site[first.idx()],
                self.def_site[dup.idx()],
            )
            .unwrap_or_else(|| format!("the first is at {}", self.loc_str(first_loc)));
        let shown = self.net.def_display(dup);
        let message = if parent == ROOT {
            format!(
                "duplicated subckt definition `{shown}` ({what}): HSPICE rejects this deck (golden `dup_subckt`). Queries use the first definition"
            )
        } else {
            format!(
                "duplicated subckt definition `{shown}` ({what}): HSPICE rejects this deck, also for two definitions with one name inside `{}` (golden `hier_dup_nested`). Queries use the first definition",
                self.net.def_display(parent)
            )
        };
        self.diag(Level::Error, "duplicate_subckt", message, Some(loc));
    }

    /// Lowers the `.model` card `m` of file `pid` into `def`. A second
    /// card with the name of one in the same scope is an error: HSPICE
    /// aborts on the same card read twice (a file included twice, golden
    /// `inc_twice_params`). Two cards on different lines were not probed;
    /// they get the same error.
    fn model(&mut self, m: &ModelBuilder, pid: usize, def: DefId) {
        let text: &'p str = &self.texts[pid];
        let syms = &mut self.net.syms;
        let kind = match m.model_type {
            ModelTypeBuilder::Unknown(sp) => syms.intern(&text[sp.start..sp.end]),
            ref known => {
                // the type as the deck spells it: the token after the name
                let ty = model_type(known, text);
                let spelled = next_token(text.as_bytes(), m.name.end)
                    .and_then(|at| text.get(at..at + ty.len()))
                    .filter(|w| w.eq_ignore_ascii_case(ty));
                match spelled {
                    Some(w) => syms.intern(w),
                    None => syms.builtin(ty),
                }
            }
        };
        let model = ModelDef {
            name: syms.intern(&text[m.name.start..m.name.end]),
            kind,
            loc: LocIdx {
                file: pid as u32,
                line: m.name.line_num,
            },
            alter: self.alter,
            // the last `level` key wins (golden `ind_level_twice`)
            level: m
                .params
                .iter()
                .rfind(|kv| text[kv.k.start..kv.k.end].eq_ignore_ascii_case("level"))
                .map(|kv| match kv.v {
                    ValueBuilder::Num(x) => Some(x),
                    ValueBuilder::Expr(_) => None,
                }),
        };
        if self.alter.is_none() {
            match self.model_first.get(&(def, model.name)) {
                None => {
                    self.model_first
                        .insert((def, model.name), (model.loc, self.site));
                }
                Some(&(first, first_site)) => {
                    let what = self
                        .read_twice(first, model.loc, first_site, self.site)
                        .unwrap_or_else(|| format!("the first is at {}", self.loc_str(first)));
                    let message = format!(
                        "duplicated model definition `{}` ({what}): HSPICE rejects this deck. Queries use the first definition",
                        self.net.syms.name(model.name)
                    );
                    self.diag(Level::Error, "duplicate_model", message, Some(model.loc));
                }
            }
        }
        self.net.defs[def.idx()].models.push(model);
    }

    /// Lowers the `.param` function `f` of file `pid` into `def`. `report`
    /// is false on a later visit of the file.
    fn func(&mut self, f: &ParamFuncBuilder, pid: usize, def: DefId, report: bool) {
        let text: &'p str = &self.texts[pid];
        let file = pid as u32;
        let syms = &mut self.net.syms;
        let func = FuncDef {
            name: syms.intern(&text[f.name.start..f.name.end]),
            args: f
                .args
                .iter()
                .map(|a| syms.intern(&text[a.start..a.end]))
                .collect(),
            body: tref(file, f.body.start, f.body.end),
            loc: LocIdx {
                file,
                line: f.name.line_num,
            },
            alter: self.alter,
        };
        if report {
            self.quote_check(func.body, func.loc);
            if func.args.is_empty() {
                let message = format!(
                    "the `.param` function `{}` has no arguments: HSPICE P-2019.06 gives a syntax error at the empty `()` and rejects the deck (golden `func_zero_args`). Write a `.param` with no `()`",
                    self.net.syms.name(func.name)
                );
                self.diag(Level::Error, "func_no_args", message, Some(func.loc));
            }
        }
        self.net.defs[def.idx()].funcs.push(func);
    }

    /// Lowers one `.option` item. `report` is false on a later visit of
    /// the file.
    fn option(&mut self, key: &Span, value: Option<&ValueBuilder>, pid: usize, report: bool) {
        let texts = self.texts;
        let text: &'p str = &texts[pid];
        let file = pid as u32;
        let name = self.net.syms.intern(&text[key.start..key.end]);
        let loc = LocIdx {
            file,
            line: key.line_num,
        };
        let p = match value {
            Some(v) => value_param(
                name,
                v,
                text,
                file,
                key.line_num,
                value_start(text, key.end),
                self.alter,
            ),
            None => ParamDef {
                name,
                text: None,
                num: None,
                loc,
                alter: self.alter,
            },
        };
        if self.net.syms.name(name).eq_ignore_ascii_case("parhier") && self.alter.is_none() {
            let value = p
                .text
                .map(|t| text[t.start as usize..t.end as usize].trim())
                .unwrap_or_default();
            if value.eq_ignore_ascii_case("local") {
                self.net.parhier = Parhier::Local;
            } else if value.eq_ignore_ascii_case("global") {
                self.net.parhier = Parhier::Global;
            } else if report {
                let message = format!(
                    "`.option parhier={value}`: the value is not `local` or `global`, so it is ignored"
                );
                self.diag(Level::Warning, "bad_option_value", message, Some(loc));
            }
        }
        self.net.options.push(p);
    }

    /// Lowers an analysis or output command (an unknown command has an
    /// error of its own).
    fn command(&mut self, cmd: &UnknwonBuilder, pid: usize, report: bool) {
        let texts = self.texts;
        let text: &'p str = &texts[pid];
        let name = &text[cmd.cmd.start..cmd.cmd.end];
        let loc = LocIdx {
            file: pid as u32,
            line: cmd.cmd.line_num,
        };
        if self.alter.is_none()
            && let Some(&swept) = SWEEPING.iter().find(|c| c.eq_ignore_ascii_case(name))
        {
            let (names, data) = sweep_names(swept, &cmd.tokens, text);
            for (w, is_data) in names
                .into_iter()
                .map(|w| (w, false))
                .chain(data.into_iter().map(|w| (w, true)))
            {
                let sym = self.net.syms.intern(w);
                if self.sweeps_seen.insert((sym, is_data, swept, loc)) {
                    self.sweeps.push((sym, is_data, swept, loc));
                }
            }
        }
        let Some(&known) = ANALYSIS.iter().find(|c| c.eq_ignore_ascii_case(name)) else {
            return;
        };
        // a `.hdl` line of an `.alter` block is not in the first run
        self.net.hdl |= known == "hdl" && self.alter.is_none();
        if known == "temp"
            && let Some(TokenBuilder::Value(v)) = cmd.tokens.first()
        {
            // `.TEMP` spells the name `temp`
            let temp = self.net.syms.intern(name);
            let from = Some(cmd.cmd.end);
            let p = value_param(temp, v, text, pid as u32, loc.line, from, self.alter);
            self.net.temps.push(p);
        } else if known == "temp" && report {
            // `.temp=25`, `.temp "25"`, `.temp`: the line is kept, but it
            // gives no temperature (not probed)
            let message = format!(
                "the `.{name}` line has no value that the loader reads, so it gives no temperature. Write `.temp 25`. Line: `{}`",
                line_at(text, cmd.cmd.start)
            );
            self.diag(Level::Warning, "temp_not_read", message, Some(loc));
        }
        if report && matches!(known, "print" | "probe") && has_braces(text, cmd.cmd.start) {
            // golden `brace_print`, `brace_probe`, `brace_probe_quoted`
            // (the parser fails the other commands with braces)
            let message = format!(
                "HSPICE P-2019.06 reads `{{` and `}}` as `[` and `]`: it ignores this `.{name}` output (a syntax error, or an output of a node such as `[a]` that it does not find), with a warning, and runs the deck. Line: `{}`",
                line_at(text, cmd.cmd.start)
            );
            self.diag(Level::Warning, "braces_output", message, Some(loc));
        }
        if report {
            match self.analysis.iter_mut().find(|(c, ..)| *c == known) {
                Some((_, count, _)) => *count += 1,
                None => {
                    // at the first one; `run` writes the text with the count
                    self.analysis.push((known, 1, self.net.diags.len()));
                    self.diag(Level::Info, "analysis_commands", String::new(), Some(loc));
                }
            }
        }
    }

    /// Lowers one element line into `def`. `report` is false on a later
    /// visit of the file.
    fn element(&mut self, inst: &InstanceBuilder, pid: usize, def: DefId, report: bool) {
        // The parser does not count the nodes of a MOSFET from above: it does
        // not know the models. This layer reads the last word as the model.
        if let InstanceCtxBuilder::MOSFET(m) = &inst.ctx {
            let found = 4 + m.extra_nodes.len();
            if found > 7 {
                if report {
                    let err = ParseErrorInner::NodeCount {
                        element: "a MOSFET",
                        want: "3 to 7",
                        found,
                    };
                    let pos = netlist_db::Pos {
                        start: inst.name.start,
                        line_num: inst.name.line_num,
                    };
                    self.parse_error(&err.with(Some(pos)), pid);
                }
                return;
            }
        }
        let texts = self.texts;
        let text: &'p str = &texts[pid];
        let d = &mut self.net.defs[def.idx()];
        let pins_start = d.pins.len();
        let params_start = d.elem_params.len();
        let index = d.elements.len();
        let mut cx = ElemCx {
            syms: &mut self.net.syms,
            def: d,
            text,
            file: pid as u32,
            line: inst.name.line_num,
            alter: self.alter,
        };
        let mut model = None;
        let mut callee = None;
        // R, C, L: the end of the second node token
        let nodes_end = match &inst.ctx {
            InstanceCtxBuilder::Resistor(r) => Some(&r.n2),
            InstanceCtxBuilder::Capacitor(c) => Some(&c.n2),
            InstanceCtxBuilder::Inductor(l) => Some(&l.n2),
            _ => None,
        }
        // `load` checked that every file is shorter than 4 GiB
        .map(|n2| n2.end as u32);
        let kind = match &inst.ctx {
            InstanceCtxBuilder::Resistor(r) => {
                model = cx.rcl(
                    "r",
                    [&r.n1, &r.n2],
                    r.model.as_ref(),
                    r.value.as_ref(),
                    r.positional_params(),
                    &r.params,
                );
                ElemKind::Resistor
            }
            InstanceCtxBuilder::Capacitor(c) => {
                model = cx.rcl(
                    "c",
                    [&c.n1, &c.n2],
                    c.model.as_ref(),
                    c.value.as_ref(),
                    c.positional_params(),
                    &c.params,
                );
                ElemKind::Capacitor
            }
            InstanceCtxBuilder::Inductor(l) => {
                model = cx.rcl(
                    "l",
                    [&l.n1, &l.n2],
                    l.model.as_ref(),
                    l.value.as_ref(),
                    l.positional_params(),
                    &l.params,
                );
                ElemKind::Inductor
            }
            InstanceCtxBuilder::Voltage(v) => {
                cx.pin(&v.n1);
                cx.pin(&v.n2);
                cx.source(&v.source);
                ElemKind::Vsource
            }
            InstanceCtxBuilder::Current(i) => {
                cx.pin(&i.n1);
                cx.pin(&i.n2);
                cx.source(&i.source);
                ElemKind::Isource
            }
            InstanceCtxBuilder::MOSFET(m) => {
                for n in [&m.nd, &m.ng, &m.ns].into_iter().chain(&m.nb) {
                    cx.pin(n);
                }
                for n in &m.extra_nodes {
                    cx.pin(n);
                }
                model = Some(cx.sym(&m.mname));
                cx.positional(m.positional_params());
                cx.kvs(&m.params);
                ElemKind::Mosfet
            }
            InstanceCtxBuilder::BJT(b) => {
                for n in [&b.nc, &b.nb, &b.ne].into_iter().chain(&b.ns) {
                    cx.pin(n);
                }
                model = Some(cx.sym(&b.mname));
                cx.positional(b.positional_params());
                cx.kvs(&b.params);
                ElemKind::Bjt
            }
            InstanceCtxBuilder::Diode(dd) => {
                cx.pin(&dd.nplus);
                cx.pin(&dd.nminus);
                model = Some(cx.sym(&dd.mname));
                cx.positional(dd.positional_params());
                cx.kvs(&dd.params);
                ElemKind::Diode
            }
            InstanceCtxBuilder::Subckt(x) => {
                for n in &x.nodes {
                    cx.pin(n);
                }
                callee = Some(cx.sym(&x.cktname));
                cx.kvs(&x.params);
                ElemKind::Instance
            }
            InstanceCtxBuilder::Unknown {
                r#type,
                nodes,
                params,
            } => {
                for n in nodes {
                    cx.pin(n);
                }
                cx.kvs(params);
                ElemKind::Other(*r#type)
            }
            // the nodes only: the gain is no node, and an `F` or `H` line
            // names its controlling sources, which are elements
            InstanceCtxBuilder::ControlledSource(e) => {
                cx.pin(&e.n1);
                cx.pin(&e.n2);
                for (p, n) in &e.ctrl_nodes {
                    cx.pin(p);
                    cx.pin(n);
                }
                cx.kvs(&e.params);
                ElemKind::Other(match e.kind {
                    ControlledKind::E => b'e',
                    ControlledKind::F => b'f',
                    ControlledKind::G => b'g',
                    ControlledKind::H => b'h',
                })
            }
            // (it names two inductors, which are elements, and no node)
            InstanceCtxBuilder::MutualInductor(k) => {
                cx.kvs(&k.params);
                ElemKind::Other(b'k')
            }
        };
        let name = cx.sym(&inst.name);
        let d = cx.def;
        // the value texts of the line for `quote_check`, also one that a
        // later `key=` replaces
        let mut check: Vec<(TextRef, LocIdx)> = if report {
            d.elem_params[params_start..]
                .iter()
                .filter_map(|p| p.text.map(|t| (t, p.loc)))
                .filter(|(t, _)| text[t.start as usize..t.end as usize].contains(['\n', '$']))
                .collect()
        } else {
            Vec::new()
        };
        // an R, C or L value from a `key=` (the first parameter): the
        // parser keeps no token of a value after the nodes that it replaces
        let key_value = report
            && nodes_end.is_some()
            && d.elem_params.get(params_start).is_some_and(|p| {
                p.text
                    .is_some_and(|t| after_eq(text.as_bytes(), t.start as usize))
            });
        keep_last(&mut d.elem_params, params_start);
        // parameter resolution reads the tokens after the nodes again when
        // the line has a model word or a `key=value` (see `Def::rcl_ends`)
        if let Some(end) = nodes_end
            && (model.is_some()
                || d.elem_params[params_start..].iter().any(|p| {
                    p.text
                        .is_some_and(|t| after_eq(text.as_bytes(), t.start as usize))
                }))
        {
            let index = u32::try_from(index).expect("fewer than 2^32 elements in a definition");
            d.rcl_ends.push((index, end));
            // also the tokens that the parser does not keep: a quoted value
            // that a later `r=` replaces (`R1 a b 'rval $ c' r=5k`), which
            // parameter resolution reads again. The rest of the statement
            // holds every value text of the line, so it is the one text to
            // check.
            if key_value {
                let (start, end) = statement_rest(text, end as usize);
                let loc = LocIdx {
                    file: pid as u32,
                    line: inst.name.line_num,
                };
                check = vec![(tref(pid as u32, start, end), loc)];
            }
        }
        let element = Element {
            name,
            kind,
            model,
            callee,
            pins: range32(pins_start, d.pins.len()),
            params: range32(params_start, d.elem_params.len()),
            loc: LocIdx {
                file: pid as u32,
                line: inst.name.line_num,
            },
            alter: self.alter,
            parasitic: false,
        };
        let loc = element.loc;
        d.elements.push(element);
        for (t, at) in check {
            self.quote_check(t, at);
        }
        if let InstanceCtxBuilder::Voltage(VoltageBuilder { source, .. })
        | InstanceCtxBuilder::Current(CurrentBuilder { source, .. }) = &inst.ctx
        {
            self.pwl_warnings(source, text, loc);
        }
    }

    /// The warnings that HSPICE gives for the points of a PWL source
    /// `src`, which is on the line `loc` of `text`. The values are kept as
    /// written.
    /// - An odd number of values: the last time point has no value. HSPICE
    ///   uses a value for it, with the warning `missing terminal voltage`
    ///   (golden `pwl_r_param_last`).
    /// - A repeat time `R` (the last one, which wins) that is not before
    ///   the last time point: HSPICE drops the repeat, with the warning
    ///   `delay greater than piece-wise` (golden `pwl_repeat_key_only`;
    ///   `pwl_repeat_key_mid` keeps `R=1n` before the last point 2n).
    ///   Only where both read as numbers.
    fn pwl_warnings(&mut self, src: &SourceBuilder, text: &str, loc: LocIdx) {
        let Some(f) = &src.func else { return };
        if !text[f.name.start..f.name.end].eq_ignore_ascii_case("pwl") {
            return;
        }
        let args = one_line(&text[f.args.start..f.args.end], false);
        let values = pwl_values(&args);
        let n = values.len();
        let line = line_at(text, f.name.start);
        if n % 2 == 1 {
            let message = format!(
                "the PWL has an odd number of values ({n}), so its last time point `{}` has no value. HSPICE uses a value for it, with a warning (`missing terminal voltage`). The values are kept as written. Line: `{line}`",
                values[n - 1]
            );
            self.diag(Level::Warning, "pwl_odd_values", message, Some(loc));
        }
        let repeat = src
            .params
            .iter()
            .rev()
            .find(|kv| text[kv.k.start..kv.k.end].eq_ignore_ascii_case("r"));
        let r = repeat.and_then(|kv| match kv.v {
            ValueBuilder::Num(x) => Some(x),
            ValueBuilder::Expr(sp) => netlist_expr::exact_number(&text[sp.start..sp.end]),
        });
        let last = n
            .checked_sub(if n % 2 == 1 { 1 } else { 2 })
            .map(|k| values[k]);
        if let (Some(r), Some(last)) = (r, last)
            && let Some(t) = netlist_expr::exact_number(last)
            && r >= t
        {
            let message = format!(
                "the repeat time `R` of the PWL ({r:e} s) is not before its last time point `{last}`, so HSPICE drops the repeat, with a warning (`delay greater than piece-wise`). `R` is kept as written. Line: `{line}`"
            );
            self.diag(Level::Warning, "pwl_repeat_dropped", message, Some(loc));
        }
    }

    /// Adds the error `e` of file `pid` as a diagnostic.
    fn parse_error(&mut self, e: &ParseError, pid: usize) {
        // (a file that is not UTF-8 has its own warning here: `non_utf8`, and
        // a line inside quotes its own error: `quote_check`)
        if matches!(
            e.err,
            ParseErrorInner::Encoding(_) | ParseErrorInner::LineInQuotes
        ) {
            return;
        }
        let text: &str = &self.texts[pid];
        let line = e.pos.map(|p| line_at(text, p.start));
        let braces = matches!(
            e.err,
            ParseErrorInner::Nom(_)
                | ParseErrorInner::TrailingText
                | ParseErrorInner::NoValue { .. }
                | ParseErrorInner::BadValue { .. }
                // `V1 a 0 DC {x}`: "`DC` has no value after it"
                | ParseErrorInner::BadSource { .. }
                // `D1 a b dmod area={x}`: the failed `area=` reads as a
                // node, so "a diode needs 2 nodes before its model name"
                | ParseErrorInner::NodeCount { .. }
                | ParseErrorInner::NoModel { .. }
                | ParseErrorInner::NoSubcktName
                | ParseErrorInner::TooManyValues { .. }
                | ParseErrorInner::ValueThenModel { .. }
                | ParseErrorInner::BadPositional { .. }
                | ParseErrorInner::DigitFirstModel { .. }
                | ParseErrorInner::NotModelName { .. }
        ) && e.pos.is_some_and(|p| has_braces(text, p.start));
        // HSPICE P-2019.06 has no `.step` command: it warns and runs the
        // deck (golden `step_param`, `step_plain`, `step_lin`)
        let step = matches!(&e.err, ParseErrorInner::Unknown(cmd)
            if text[cmd.start..cmd.end].eq_ignore_ascii_case("step"));
        let (code, what) = if braces {
            // golden `param_braces`, `param_braces_param`, and in quotes
            // `brace_quoted_param`, `brace_quoted_elem`, `brace_quoted_fn`
            (
                "braces",
                "`{...}` is not an expression in HSPICE P-2019.06: it reads `{` and `}` as `[` and `]`, also inside quotes, and rejects this deck. Write the expression in quotes without braces, `'...'`. The statement was skipped, so it gives no value".to_owned(),
            )
        } else if step {
            (
                "unknown_command",
                "unknown command `.step`: HSPICE P-2019.06 ignores it with a warning (`unknown command \".step\" is ignored`) and sweeps nothing. It was ignored".to_owned(),
            )
        } else {
            describe(&e.err, text, line.as_deref())
        };
        let message = match &line {
            Some(line) => format!("{what}. Line: `{line}`"),
            None => what,
        };
        let loc = e.pos.map(|p| LocIdx {
            file: pid as u32,
            line: p.line_num,
        });
        let level = if step { Level::Warning } else { Level::Error };
        self.diag(level, code, message, loc);
    }
}

/// The kinds of statements of a `LocalAST` that [`Lower::local`] merges
/// in line order.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Note,
    Subckt,
    Element,
    Model,
    Param,
    Func,
    Global,
    Option,
    Command,
}

const KINDS: [Kind; 9] = [
    Kind::Note,
    Kind::Subckt,
    Kind::Element,
    Kind::Model,
    Kind::Param,
    Kind::Func,
    Kind::Global,
    Kind::Option,
    Kind::Command,
];

/// A located note of a `LocalAST`, reported in line order.
#[derive(Clone, Copy)]
enum Note<'p> {
    Error(&'p ParseError),
    /// The first and the last line of a `.prot` region.
    Protected(u32, u32),
    /// A `.end` line in a file that is not the top file: it was ignored.
    EndInInclude,
}

fn new_def(name: Sym, parent: Option<DefId>, loc: LocIdx, alter: Option<u32>) -> Def {
    Def {
        name,
        ports: Vec::new(),
        defaults: Vec::new(),
        params: Vec::new(),
        funcs: Vec::new(),
        models: Vec::new(),
        elements: Vec::new(),
        rcl_ends: Vec::new(),
        pins: Vec::new(),
        elem_params: Vec::new(),
        nested: Vec::new(),
        parent,
        loc,
        text: tref(loc.file, 0, 0),
        text_line: 0,
        alter,
        dup_of: None,
        has_dup: false,
    }
}

/// The definition `LoadOptions::top` names (see [`Netlist::find_def`]).
fn find_top(net: &Netlist, name: &str) -> Result<DefId, LoadError> {
    match net.find_def(name) {
        DefLookup::Found(id) => Ok(id),
        DefLookup::NotFound => Err(LoadError::NoTopLevel(format!(
            "no subckt named `{name}` (from `top`)"
        ))),
        DefLookup::Ambiguous(names) => Err(LoadError::NoTopLevel(format!(
            "`{name}` names {} nested definitions ({}): pass the qualified name",
            names.len(),
            names.join(", ")
        ))),
    }
}

/// The code and the plain-English text of a parser error of file `text`:
/// what is wrong, then what the loader did. `line` is the text of its
/// line.
fn describe(err: &ParseErrorInner, text: &str, line: Option<&str>) -> (&'static str, String) {
    use ParseErrorInner as E;
    const SKIPPED: &str = "The statement was skipped";
    match err {
        E::IO(e) => (
            "file_unreadable",
            format!("cannot read the included file: {e}. Its statements are missing"),
        ),
        E::NoLibSection { path, section } => (
            "lib_section_missing",
            format!(
                "section `{section}` is not in \"{}\". This `.lib` call loads nothing",
                path.display()
            ),
        ),
        E::Nom(_) | E::OrphanContinuation => ("syntax_error", format!("syntax error. {SKIPPED}")),
        E::DuplicateSubckt(_) => ("duplicate_subckt", err.to_string()),
        E::Unsupported(what) => ("unsupported_form", what.clone()),
        E::NonHspice(what) => ("not_hspice", what.clone()),
        E::Internal(what) => ("parser_failed", what.clone()),
        E::Encrypted { .. } => ("encrypted_block", err.to_string()),
        E::Encoding(what) => ("non_utf8", what.clone()),
        E::LineInQuotes => ("quoted_comment_line", err.to_string()),
        E::MosGroupLines => ("mos_group_lines", format!("{err}. {SKIPPED}")),
        E::TitleIsElement => ("title_is_element", err.to_string()),
        E::Unknown(cmd) => (
            "unknown_command",
            format!(
                "unknown command `.{}`. It was ignored",
                &text[cmd.start..cmd.end]
            ),
        ),
        E::CircularDefinition(chain, idx) => {
            let files: Vec<String> = chain
                .keys()
                .skip(*idx)
                .chain(chain.get_index(*idx).map(|(k, _)| k))
                .map(file_id_str)
                .collect();
            // the parser no longer gives it; the same code as
            // `Lower::include_cycle`
            (
                "include_cycle",
                format!(
                    "circular include: {}. The include on this line was skipped",
                    files.join(" -> ")
                ),
            )
        }
        E::StrayEnds { .. } => ("stray_ends", format!("{err}. It was ignored")),
        E::MissingEnds { .. } => (
            "missing_ends",
            format!("{err}. The definition takes the rest of the file"),
        ),
        E::TrailingText => {
            let ends = line.is_some_and(|l| {
                let l = l.to_ascii_lowercase();
                l.starts_with(".ends") || l.starts_with(".eom")
            });
            let then = if ends {
                "The text was ignored, and the line still ends its definition"
            } else {
                SKIPPED
            };
            ("trailing_text", format!("{err}. {then}"))
        }
        E::AlterInSubckt => ("alter_in_subckt", format!("{err}. {SKIPPED}")),
        E::MissingUnprot => (
            "missing_unprot",
            format!("{err}. The rest of the file is protected and was not parsed"),
        ),
        E::NestingTooDeep { .. } => ("nesting_too_deep", err.to_string()),
        E::PlusAfterTitle => ("plus_after_title", format!("{err}. Its lines were skipped")),
        E::NodeCount { .. } => ("node_count", format!("{err}. {SKIPPED}")),
        E::NoModel { .. } => ("no_model", format!("{err}. {SKIPPED}")),
        E::NoSubcktName => ("no_subckt_name", format!("{err}. {SKIPPED}")),
        E::TooManyValues { .. } => ("too_many_values", format!("{err}. {SKIPPED}")),
        E::ValueThenModel { .. } => ("value_then_model", format!("{err}. {SKIPPED}")),
        E::BadPositional { .. } => ("bad_positional", format!("{err}. {SKIPPED}")),
        E::DigitFirstModel { .. }
        | E::NotModelName {
            digit_first: true, ..
        } => ("digit_first_model", format!("{err}. {SKIPPED}")),
        E::NotModelName { .. } => ("bad_model_name", format!("{err}. {SKIPPED}")),
        E::BadSource { .. } => ("bad_source", format!("{err}. {SKIPPED}")),
        E::NoValue { .. } => ("no_value", format!("{err}. {SKIPPED}")),
        E::BadValue { .. } => ("bad_value", format!("{err}. {SKIPPED}")),
    }
}

/// True when the statement at byte `start` of `text` has a `{` or a `}`
/// outside its `$` comments (a `$` starts a comment, as in the parser) and
/// its `*` comment lines, also inside quotes: HSPICE rejects both (golden
/// `param_braces`, `brace_quoted_elem`).
fn has_braces(text: &str, start: usize) -> bool {
    let (s, e) = statement_rest(text, start);
    text[s..e].lines().any(|line| {
        let code = line.trim_start();
        !code.starts_with('*') && code[..code.find('$').unwrap_or(code.len())].contains(['{', '}'])
    })
}

fn file_id_str(id: &FileId) -> String {
    match id {
        FileId::Include { path } => path.display().to_string(),
        FileId::Section { path, section } => format!("{} (section {section})", path.display()),
    }
}

/// The text of the line of `text` that holds byte `pos`, trimmed and
/// shortened.
fn line_at(text: &str, pos: usize) -> String {
    let mut pos = pos.min(text.len());
    while !text.is_char_boundary(pos) {
        pos -= 1;
    }
    let start = text[..pos].rfind('\n').map_or(0, |n| n + 1);
    let end = text[pos..].find('\n').map_or(text.len(), |n| pos + n);
    shorten(text[start..end].trim())
}

/// `s`, cut to [`QUOTE_MAX`] characters.
fn shorten(s: &str) -> String {
    match s.char_indices().nth(QUOTE_MAX) {
        Some((cut, _)) => format!("{}...", &s[..cut]),
        None => s.to_owned(),
    }
}

/// The name of a model type, in lowercase.
fn model_type<'t>(t: &ModelTypeBuilder, text: &'t str) -> &'t str {
    match t {
        ModelTypeBuilder::AMP => "amp",
        ModelTypeBuilder::C => "c",
        ModelTypeBuilder::CORE => "core",
        ModelTypeBuilder::D => "d",
        ModelTypeBuilder::L => "l",
        ModelTypeBuilder::NJF => "njf",
        ModelTypeBuilder::NMOS => "nmos",
        ModelTypeBuilder::NPN => "npn",
        ModelTypeBuilder::OPT => "opt",
        ModelTypeBuilder::PJF => "pjf",
        ModelTypeBuilder::PMOS => "pmos",
        ModelTypeBuilder::PNP => "pnp",
        ModelTypeBuilder::R => "r",
        ModelTypeBuilder::U => "u",
        ModelTypeBuilder::W => "w",
        ModelTypeBuilder::S => "s",
        ModelTypeBuilder::Unknown(span) => &text[span.start..span.end],
    }
}

#[inline]
fn tref(file: u32, start: usize, end: usize) -> TextRef {
    // `load` checked that every file is shorter than 4 GiB
    TextRef {
        file,
        start: start as u32,
        end: end as u32,
    }
}

#[inline]
fn range32(start: usize, end: usize) -> Range32 {
    let to32 =
        |n: usize| u32::try_from(n).expect("fewer than 2^32 pins or parameters in a definition");
    Range32 {
        start: to32(start),
        end: to32(end),
    }
}

/// The `name=value` parameter `kv` of file `text`.
fn kv_param(
    syms: &mut Interner,
    text: &str,
    file: u32,
    kv: &KeyValueBuilder,
    alter: Option<u32>,
) -> ParamDef {
    let name = syms.intern(&text[kv.k.start..kv.k.end]);
    let from = value_start(text, kv.k.end);
    value_param(name, &kv.v, text, file, kv.k.line_num, from, alter)
}

/// The parameter `name` with the value `v`. For a number, its text is
/// looked for at byte `from` (after spaces), and kept only when it reads
/// as the same number.
fn value_param(
    name: Sym,
    v: &ValueBuilder,
    text: &str,
    file: u32,
    line: u32,
    from: Option<usize>,
    alter: Option<u32>,
) -> ParamDef {
    match *v {
        ValueBuilder::Expr(sp) => {
            let s = &text[sp.start..sp.end];
            let num = netlist_expr::scan_number(s)
                .filter(|&(_, n)| n == s.len())
                .map(|(x, _)| x);
            ParamDef {
                name,
                text: Some(tref(file, sp.start, sp.end)),
                num,
                loc: LocIdx {
                    file,
                    line: sp.line_num,
                },
                alter,
            }
        }
        ValueBuilder::Num(x) => {
            let found = from.and_then(|from| num_token(text, from, x));
            ParamDef {
                name,
                text: found.map(|(s, e)| tref(file, s, e)),
                num: Some(x),
                loc: LocIdx { file, line },
                alter,
            }
        }
    }
}

/// The byte after the `=` that follows a key ending at byte `key_end`.
fn value_start(text: &str, key_end: usize) -> Option<usize> {
    let b = text.as_bytes();
    let at = skip_spaces(b, key_end);
    (b.get(at) == Some(&b'=')).then_some(at + 1)
}

/// The next token of the statement at byte `from`, when it reads as the
/// number `x`: its byte range. The token ends at a space or at a `$` (a
/// comment, also right after a number, as in the parser). See
/// [`next_token`] for what comes before it.
fn num_token(text: &str, from: usize, x: f64) -> Option<(usize, usize)> {
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
fn statement_rest(text: &str, from: usize) -> (usize, usize) {
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
fn keywords(text: &str, from: usize, key: &str) -> Option<((usize, usize), usize)> {
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
fn keyword_before<'t>(text: &'t str, at: usize, key: &str, eq: bool) -> Option<&'t str> {
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

/// What the analysis line `cmd` (one of [`SWEEPING`]) with the tokens
/// `tokens` sweeps (spec 6.2a): the names (a parameter, a source, `temp`)
/// and the `.data` blocks (`data=d1`). `.dc` sweeps its first word and a
/// second word after the values of the first sweep: three values
/// (`.dc v1 0 1 0.1 v2 0 1 0.5`), a type and three values (`lin`, `dec`,
/// `oct`: `.dc v1 lin 11 0 1 v2 lin 3 1k 3k`), `poi`, the count and that
/// many values (`.dc v1 poi 2 0 1 v2 ...`), or `start=`, `stop=` and
/// `step=`. Each analysis sweeps the word after `sweep`. Not probed.
fn sweep_names<'t>(
    cmd: &str,
    tokens: &[TokenBuilder],
    text: &'t str,
) -> (Vec<&'t str>, Vec<&'t str>) {
    const KEYWORDS: [&str; 12] = [
        "sweep", "data", "monte", "optimize", "lin", "dec", "oct", "poi", "param", "list",
        "results", "model",
    ];
    let word = |t: Option<&TokenBuilder>| match t {
        Some(TokenBuilder::Value(ValueBuilder::Expr(sp))) => {
            let w = text[sp.start..sp.end].trim();
            let ident = matches!(netlist_expr::parse(w), Ok(netlist_expr::Expr::Ident(_)));
            (ident && !KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(w))).then_some(w)
        }
        _ => None,
    };
    let is = |t: Option<&TokenBuilder>, k: &str| match t {
        Some(TokenBuilder::Value(ValueBuilder::Expr(sp))) => {
            text[sp.start..sp.end].trim().eq_ignore_ascii_case(k)
        }
        _ => false,
    };
    let mut names = Vec::new();
    if cmd == "dc" {
        let mut i = 0;
        for _ in 0..2 {
            let Some(w) = word(tokens.get(i)) else { break };
            names.push(w);
            i += 1;
            if ["lin", "dec", "oct"].iter().any(|k| is(tokens.get(i), k)) {
                i += 4;
            } else if is(tokens.get(i), "poi") {
                match tokens.get(i + 1) {
                    Some(TokenBuilder::Value(ValueBuilder::Num(n)))
                        if n.fract() == 0.0 && (0.0..=tokens.len() as f64).contains(n) =>
                    {
                        i += 2 + *n as usize;
                    }
                    _ => break,
                }
            } else if matches!(tokens.get(i), Some(TokenBuilder::KV(_))) {
                while matches!(tokens.get(i), Some(TokenBuilder::KV(_))) {
                    i += 1;
                }
            } else {
                i += 3;
            }
        }
    }
    for (i, _) in tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| is(Some(t), "sweep"))
    {
        names.extend(word(tokens.get(i + 1)));
    }
    let data = tokens
        .iter()
        .filter_map(|t| match t {
            TokenBuilder::KV(kv) if text[kv.k.start..kv.k.end].eq_ignore_ascii_case("data") => {
                match kv.v {
                    ValueBuilder::Expr(sp) => Some(text[sp.start..sp.end].trim()),
                    ValueBuilder::Num(_) => None,
                }
            }
            _ => None,
        })
        .collect();
    (names, data)
}

/// Removes from `params[start..]` (the parameters of one element) each
/// parameter that a later one with the same name overrides: the last value
/// of a name wins (golden `elem_dup_kw`).
fn keep_last(params: &mut Vec<ParamDef>, start: usize) {
    let tail = &params[start..];
    let n = tail.len();
    let later = |i: usize| tail[i + 1..].iter().any(|q| q.name == tail[i].name);
    let overridden: Vec<bool> = if n < 2 {
        return;
    } else if n <= 16 {
        // most lines: a few parameters, no repeated name
        if !(0..n).any(later) {
            return;
        }
        (0..n).map(later).collect()
    } else {
        let mut seen = HashSet::with_capacity(n);
        let mut overridden = vec![false; n];
        for i in (0..n).rev() {
            overridden[i] = !seen.insert(tail[i].name);
        }
        if !overridden.contains(&true) {
            return;
        }
        overridden
    };
    let tail = params.split_off(start);
    params.extend(
        tail.into_iter()
            .zip(overridden)
            .filter(|(_, o)| !o)
            .map(|(p, _)| p),
    );
}

/// What one element line adds to its definition: pins and parameters in
/// the arenas of `def`.
struct ElemCx<'a> {
    syms: &'a mut Interner,
    def: &'a mut Def,
    text: &'a str,
    file: u32,
    /// The line of the element name.
    line: u32,
    alter: Option<u32>,
}

impl ElemCx<'_> {
    fn sym(&mut self, sp: &Span) -> Sym {
        self.syms.intern(&self.text[sp.start..sp.end])
    }

    fn pin(&mut self, sp: &Span) {
        let sym = self.sym(sp);
        self.def.pins.push(sym);
    }

    fn kvs(&mut self, kvs: &[KeyValueBuilder]) {
        for kv in kvs {
            let p = kv_param(self.syms, self.text, self.file, kv, self.alter);
            self.def.elem_params.push(p);
        }
    }

    /// The `Sym` of the built-in name `key`. When the deck writes `key`
    /// right before the value token at byte `at` (`R=1k`, and also
    /// `DC 1` when `eq` is false), this spelling is the deck's spelling of
    /// the name (see [`Interner::with_builtins`]).
    fn key_sym(&mut self, key: &str, at: usize, eq: bool) -> Sym {
        match keyword_before(self.text, at, key, eq) {
            Some(word) => self.syms.intern(word),
            None => self.syms.builtin(key),
        }
    }

    /// Adds the parameter `name` whose value is the text `start..end`.
    fn raw(&mut self, name: Sym, start: usize, end: usize, line: u32) {
        self.def.elem_params.push(ParamDef {
            name,
            text: Some(tref(self.file, start, end)),
            num: None,
            loc: LocIdx {
                file: self.file,
                line,
            },
            alter: self.alter,
        });
    }

    /// The pins, the value (as `key`), the numbers after the value and the
    /// parameters of an R, C or L line. Returns the model.
    fn rcl<'v>(
        &mut self,
        key: &str,
        nodes: [&Span; 2],
        model: Option<&Span>,
        value: Option<&ValueBuilder>,
        positional: impl Iterator<Item = (&'static str, &'v Span, &'v ValueBuilder)>,
        params: &[KeyValueBuilder],
    ) -> Option<Sym> {
        for n in nodes {
            self.pin(n);
        }
        let model_sym = model.map(|m| self.sym(m));
        if let Some(v) = value {
            let from = model.map_or(nodes[1].end, |m| m.end);
            // the `key=` parameters: the first spelling of `key`, and the
            // value of the last one (the parser keeps the winning value
            // only, and a keyword value wins over a value after the nodes)
            let mut keys = None;
            // The parser keeps the number, not its text. Find the text: the
            // winning `key=` (the last one), else the token after the nodes
            // (also before the model name of an L line), else the token
            // after the model name. Each can be on a `+` line.
            let found = match *v {
                ValueBuilder::Num(x) => {
                    keys = keywords(self.text, from, key);
                    keys.and_then(|(_, at)| num_token(self.text, at, x))
                        .or_else(|| num_token(self.text, nodes[1].end, x))
                        .or_else(|| model.and_then(|m| num_token(self.text, m.end, x)))
                        .map(|r| (r, x))
                }
                ValueBuilder::Expr(sp) => {
                    if keyword_before(self.text, sp.start, key, true).is_some() {
                        keys = keywords(self.text, from, key);
                    }
                    None
                }
            };
            // The name keeps the first spelling of `key=` on the line
            // (`R1 a b R=1k r=2k` gives `R`), else the deck's spelling right
            // before the value, else the built-in one.
            let first_key = keys.map(|((s, e), _)| self.syms.intern(&self.text[s..e]));
            match found {
                Some(((start, end), x)) => {
                    let lines = self.text.as_bytes()[nodes[1].end..start]
                        .iter()
                        .filter(|&&c| c == b'\n')
                        .count();
                    let name = first_key.unwrap_or_else(|| self.key_sym(key, start, true));
                    self.def.elem_params.push(ParamDef {
                        name,
                        text: Some(tref(self.file, start, end)),
                        num: Some(x),
                        loc: LocIdx {
                            file: self.file,
                            line: nodes[1].line_num + lines as u32,
                        },
                        alter: self.alter,
                    });
                }
                None => {
                    let name = match (first_key, *v) {
                        (Some(name), _) => name,
                        (None, ValueBuilder::Expr(sp)) => self.key_sym(key, sp.start, true),
                        (None, ValueBuilder::Num(_)) => self.syms.builtin(key),
                    };
                    let p = value_param(name, v, self.text, self.file, self.line, None, self.alter);
                    self.def.elem_params.push(p);
                }
            }
        }
        self.positional(positional);
        self.kvs(params);
        model_sym
    }

    /// The numbers after a model name or a value, by their names, each with
    /// its token as written.
    fn positional<'v>(
        &mut self,
        values: impl Iterator<Item = (&'static str, &'v Span, &'v ValueBuilder)>,
    ) {
        for (name, sp, v) in values {
            // (`OFF` on a MOSFET line takes a place and gives no value)
            if self.text[sp.start..sp.end].eq_ignore_ascii_case("off") {
                continue;
            }
            let name = self.syms.builtin(name);
            self.token(name, sp, v);
        }
    }

    /// The source of a V or I line: the DC value as `dc`, the function by
    /// its name (as written) with its arguments as written, the AC
    /// magnitude and phase as `ac` and `acphase`, then the parameters.
    /// [`Netlist::param_text`] gives the arguments on one line.
    fn source(&mut self, src: &SourceBuilder) {
        if let Some((sp, v)) = &src.dc {
            let name = self.key_sym("dc", sp.start, false);
            self.token(name, sp, v);
        }
        if let Some(f) = &src.func {
            let name = self.sym(&f.name);
            self.raw(name, f.args.start, f.args.end, f.args.line_num);
        }
        if let Some((sp, v)) = &src.ac_mag {
            let name = self.key_sym("ac", sp.start, false);
            self.token(name, sp, v);
        }
        if let Some((sp, v)) = &src.ac_phase {
            let name = self.syms.builtin("acphase");
            self.token(name, sp, v);
        }
        self.kvs(&src.params);
    }

    /// Adds the parameter `name` = `v`, whose token is `sp` as written.
    fn token(&mut self, name: Sym, sp: &Span, v: &ValueBuilder) {
        let p = match *v {
            ValueBuilder::Num(x) => ParamDef {
                name,
                text: Some(tref(self.file, sp.start, sp.end)),
                num: Some(x),
                loc: LocIdx {
                    file: self.file,
                    line: sp.line_num,
                },
                alter: self.alter,
            },
            ValueBuilder::Expr(_) => {
                value_param(name, v, self.text, self.file, sp.line_num, None, self.alter)
            }
        };
        self.def.elem_params.push(p);
    }
}

#[cfg(test)]
mod tests {
    use super::{QuoteFault, one_line, quote_fault};
    use crate::{
        LoadOptions, Netlist,
        model::{Def, DefId, ElemKind, Element, ParamDef, ROOT, Root},
    };
    use std::{
        borrow::Cow,
        path::{Path, PathBuf},
    };

    fn repo(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
    }

    /// A deck in a new temporary directory, which is removed on drop.
    struct TempDeck {
        dir: PathBuf,
        /// The first file.
        main: PathBuf,
    }

    impl Drop for TempDeck {
        fn drop(&mut self) {
            _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Writes `files` (name, text) into a new directory for test `name`.
    fn write_deck(name: &str, files: &[(&str, &str)]) -> TempDeck {
        let dir =
            std::env::temp_dir().join(format!("netlist-query-lower-{}-{name}", std::process::id()));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (file, text) in files {
            std::fs::write(dir.join(file), text).unwrap();
        }
        let main = dir.join(files[0].0);
        TempDeck { dir, main }
    }

    fn load(path: PathBuf) -> Netlist {
        Netlist::load(&LoadOptions::new(path)).unwrap()
    }

    /// The active top-level definition `name`.
    fn top_def<'n>(net: &'n Netlist, name: &str) -> &'n Def {
        let sym = net.syms.get(name).unwrap();
        &net.defs[net.def_index[&(ROOT, sym)].idx()]
    }

    fn elem_names(net: &Netlist, d: &Def) -> Vec<String> {
        d.elements
            .iter()
            .map(|e| net.syms.name(e.name).to_owned())
            .collect()
    }

    fn element<'n>(net: &'n Netlist, d: &'n Def, name: &str) -> &'n Element {
        let sym = net.syms.get(name).unwrap();
        d.elements.iter().find(|e| e.name == sym).unwrap()
    }

    /// (name, text) of the parameters of `e`.
    fn params(net: &Netlist, d: &Def, e: &Element) -> Vec<(String, String)> {
        d.element_params(e)
            .iter()
            .map(|p| (net.syms.name(p.name).to_owned(), text(net, p)))
            .collect()
    }

    fn text(net: &Netlist, p: &ParamDef) -> String {
        p.text
            .map_or_else(|| format!("{:?}", p.num), |t| net.text(t).to_owned())
    }

    fn pins(net: &Netlist, d: &Def, e: &Element) -> Vec<String> {
        d.element_pins(e)
            .iter()
            .map(|&s| net.syms.name(s).to_owned())
            .collect()
    }

    #[test]
    fn include_inside_subckt_belongs_to_def() {
        let net = load(repo("tests/top.sp"));
        let names: Vec<String> = (1..net.defs.len() as u32)
            .map(|i| net.def_display(DefId(i)))
            .collect();
        assert_eq!(names, ["DEMO", "UNITS"]);
        let demo = top_def(&net, "demo");
        assert_eq!(elem_names(&net, demo), ["X0.CCC"]);
        assert_eq!(demo.defaults.len(), 2);
        // top level: X0.BBB from inc1.sp (line 2), then X0.AAA
        let top = elem_names(&net, &net.defs[ROOT.idx()]);
        assert_eq!(top, ["X0.BBB", "X0.AAA"]);
        // `.lib '../lib.sp' pre_layout` in inc2.sp, inside DEMO (the
        // parser does not read `.params flag=1`: an unknown command)
        let pre: Vec<&str> = demo.params.iter().map(|p| net.syms.name(p.name)).collect();
        assert_eq!(pre, ["prelayout", "flag_cc"]);
        // units.sp: `.param` lines inside UNITS
        assert!(top_def(&net, "units").params.len() >= 10);

        let net = load(repo("golden/cases/include_in_subckt/deck.sp"));
        let holder = top_def(&net, "holder");
        assert_eq!(elem_names(&net, holder), ["ri", "rj", "r0"]);
        let rj = element(&net, holder, "rj");
        assert_eq!(
            params(&net, holder, rj),
            [("r".to_owned(), "pv*2".to_owned())]
        );
        let loc = net.loc(rj.loc);
        assert!(loc.file.ends_with("inc.sp"), "{loc:?}");
        assert_eq!(loc.line, 3);
    }

    #[test]
    fn duplicate_first_is_active() {
        let net = load(repo("golden/cases/dup_subckt/deck.sp"));
        let inv: Vec<DefId> = (1..net.defs.len() as u32).map(DefId).collect();
        assert_eq!(inv.len(), 2);
        let first = top_def(&net, "inv");
        assert_eq!(first.loc.line, 2);
        assert!(first.has_dup && first.dup_of.is_none());
        let second = &net.defs[2];
        assert_eq!(second.dup_of, Some(DefId(1)));
        let r1 = element(&net, first, "r1");
        assert_eq!(params(&net, first, r1), [("r".to_owned(), "1".to_owned())]);
    }

    #[test]
    fn nested_definitions() {
        let net = load(repo("golden/cases/nested_def/deck.sp"));
        let names: Vec<String> = (0..net.defs.len() as u32)
            .map(|i| net.def_display(DefId(i)))
            .collect();
        assert_eq!(
            names,
            ["(top level)", "cell", "outer", "outer>cell", "outer>deep"]
        );
        let outer = top_def(&net, "outer");
        assert_eq!(outer.nested, [DefId(3), DefId(4)]);
        assert_eq!(net.defs[3].parent, Some(DefId(2)));
        assert_eq!(net.defs[1].parent, None);
        assert_eq!(net.defs[ROOT.idx()].nested, [DefId(1), DefId(2)]);
        // nested `cell` has its own index entry; not a duplicate
        let cell = net.syms.get("cell").unwrap();
        assert_eq!(net.def_index[&(DefId(2), cell)], DefId(3));
        assert_eq!(net.def_index[&(ROOT, cell)], DefId(1));
        assert!(net.defs.iter().all(|d| d.dup_of.is_none()));
        // `X` lines keep the callee name only
        let xc = element(&net, outer, "xc");
        assert_eq!(xc.kind, ElemKind::Instance);
        assert_eq!(xc.callee, Some(cell));
        assert_eq!(pins(&net, outer, xc), ["a", "b"]);
        let deep = &net.defs[4];
        assert_eq!(element(&net, deep, "xc").callee, Some(cell));
        assert_eq!(net.root, Root::TopLevel);
    }

    #[test]
    fn element_lowering() {
        let deck = write_deck(
            "elements",
            &[(
                "main.sp",
                "* t\n\
                 .model nch nmos level=1\n\
                 R1 a b 1k tc1=1e-3\n\
                 R2 a b rmod 2k\n\
                 R3 a b r='2*x' m=2\n\
                 C1 a 0 c=10pF\n\
                 M1 d g s b nch 2u 1u\n\
                 M2 d g s nch W=0.135u L=40.00n\n\
                 M3 d g s b e1 nch w=1u\n\
                 V1 a 0 1.2\n\
                 V2 a 0 PULSE(0,1,0,1n,1n,5n,10n)\n\
                 I1 a 0 PWL(0 0 1n 1m\n\
                 + 2n 0)\n\
                 Q1 c b e qmod 2\n\
                 D1 a k dmod\n\
                 X1 a b inv WN=2u m=2\n\
                 E1 out 0 in 0 2\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(p("r1"), [s("r", "1k"), s("tc1", "1e-3")]);
        assert_eq!(p("r2"), [s("r", "2k")]);
        assert_eq!(element(&net, top, "r2").model, net.syms.get("rmod"));
        assert_eq!(p("r3"), [s("r", "2*x"), s("m", "2")]);
        assert_eq!(p("c1"), [s("c", "10pF")], "the keyword value as written");
        // the positional names take the deck's first spelling, from M2's
        // `W=` and `L=` (a later line)
        assert_eq!(p("m1"), [s("L", "2u"), s("W", "1u")]);
        assert_eq!(p("m2"), [s("W", "0.135u"), s("L", "40.00n")]);
        assert_eq!(pins(&net, top, element(&net, top, "m2")), ["d", "g", "s"]);
        assert_eq!(
            pins(&net, top, element(&net, top, "m3")),
            ["d", "g", "s", "b", "e1"]
        );
        assert_eq!(p("v1"), [s("dc", "1.2")]);
        // names as written
        assert_eq!(p("v2"), [s("PULSE", "0,1,0,1n,1n,5n,10n")]);
        // a source function: its arguments only, as written
        assert_eq!(p("i1"), [s("PWL", "0 0 1n 1m\n+ 2n 0")]);
        assert_eq!(p("q1"), [s("area", "2")]);
        assert_eq!(element(&net, top, "d1").model, net.syms.get("dmod"));
        assert_eq!(p("x1"), [s("WN", "2u"), s("m", "2")]);
        assert_eq!(element(&net, top, "x1").callee, net.syms.get("inv"));
        let e1 = element(&net, top, "e1");
        assert_eq!(e1.kind, ElemKind::Other(b'e'));
        assert_eq!(e1.kind.name(), "e");
        assert_eq!(pins(&net, top, e1), ["out", "0", "in", "0"]);
        assert_eq!(top.models.len(), 1);
        // numbers: text as written, value as read
        let w = &top.element_params(element(&net, top, "m2"))[0];
        assert_eq!(w.num, Some(0.135e-6));
    }

    #[test]
    fn alter_flags() {
        let net = load(repo("golden/cases/alter/deck.sp"));
        let top = &net.defs[ROOT.idx()];
        let alters: Vec<(String, Option<u32>)> = top
            .params
            .iter()
            .map(|p| (text(&net, p), p.alter))
            .collect();
        assert_eq!(
            alters,
            [("1k".to_owned(), None), ("3k".to_owned(), Some(1))]
        );
        let r2_sym = net.syms.get("r2").unwrap();
        let r2: Vec<Option<u32>> = top
            .elements
            .iter()
            .filter(|e| e.name == r2_sym)
            .map(|e| e.alter)
            .collect();
        assert_eq!(r2, [None, Some(2)]);
        assert_eq!(net.alters.len(), 2);
    }

    #[test]
    fn options_globals_temps() {
        let net = load(repo("golden/cases/global_net/deck.sp"));
        assert_eq!(net.globals, [net.syms.get("vdd").unwrap()]);
        let net = load(repo("golden/cases/temper/deck.sp"));
        assert_eq!(net.temps.len(), 1);
        assert_eq!(net.temps[0].num, Some(50.0));
        let net = load(repo("tests/top.sp"));
        // `.option gmindc=1e-14 scale=0.9` in the `pre_layout` section
        let opts: Vec<(&str, String)> = net
            .options
            .iter()
            .map(|p| (net.syms.name(p.name), text(&net, p)))
            .collect();
        assert_eq!(
            opts,
            [("gmindc", "1e-14".to_owned()), ("scale", "0.9".to_owned())]
        );
        // A `.temp` line whose value is not read gives no temperature, with
        // a Warning (only the Info `analysis_commands` before). Not probed.
        let deck = write_deck(
            "temp_not_read",
            &[("main.sp", "* t\n.temp=25\n.temp 25,50\n.temp\n.TEMP 30\n")],
        );
        let net = load(deck.main.clone());
        let lines: Vec<String> = diags_of(&net, "temp_not_read")
            .into_iter()
            .map(|d| d.1)
            .collect();
        assert_eq!(
            lines,
            (2..=4).map(|n| format!("main.sp:{n}")).collect::<Vec<_>>()
        );
        assert_eq!(net.temps.len(), 1);
        assert_eq!(net.temps[0].num, Some(30.0));
        // HSPICE reads a value in quotes (probes `.temp "50"` and `.temp '50'`:
        // `temp= 50.000`), and it rejects a group (`.temp (50)`)
        for line in [".temp \"50\"", ".temp '50'"] {
            let deck = write_deck("temp_quoted", &[("main.sp", &format!("* t\n{line}\n"))]);
            let net = load(deck.main.clone());
            assert_eq!(net.temps.len(), 1, "{line}");
        }
        let deck = write_deck("temp_group", &[("main.sp", "* t\n.temp (50)\n")]);
        let net = load(deck.main.clone());
        assert!(net.temps.is_empty());
        assert_eq!(diags_of(&net, "syntax_error").len(), 1);
    }

    /// The parser leaves the upper limit of the nodes of a MOSFET to the
    /// readers of the line.
    #[test]
    fn mosfet_node_count() {
        let deck = write_deck(
            "mos_nodes",
            &[(
                "main.sp",
                "* t\nM1 d g s b x y z w nch W=1u\nM2 d g s b x y z nch W=1u\nM3 d g nch\n",
            )],
        );
        let net = load(deck.main.clone());
        let lines: Vec<String> = diags_of(&net, "node_count")
            .into_iter()
            .map(|d| d.1)
            .collect();
        assert_eq!(lines, ["main.sp:2", "main.sp:4"]);
        assert_eq!(elem_names(&net, &net.defs[ROOT.idx()]), ["M2"]);
    }

    #[test]
    fn funcs_and_macro() {
        let net = load(repo("golden/cases/funcs/deck.sp"));
        let top = &net.defs[ROOT.idx()];
        assert_eq!(top.funcs.len(), 1);
        let f = &top.funcs[0];
        assert_eq!(f.args.len(), 2);
        assert!(!net.text(f.body).is_empty());
        let net = load(repo("golden/cases/macro_eom/deck.sp"));
        let inv = top_def(&net, "inv");
        assert_eq!(inv.defaults.len(), 1);
        assert_eq!(elem_names(&net, inv), ["r1"]);
    }

    #[test]
    fn twice_included_file_reports_once() {
        let deck = write_deck(
            "twice",
            &[
                ("main.sp", "* t\n.inc 'a.sp'\n.inc 'a.sp'\n"),
                ("a.sp", "R1 a b 1\nR2 a b\n"),
            ],
        );
        let net = load(deck.main.clone());
        // both copies are in the model
        assert_eq!(net.defs[ROOT.idx()].elements.len(), 2);
        // the error of `R2 a b` is reported once
        let errors = net.diags.iter().filter(|d| d.code == "no_value").count();
        assert_eq!(errors, 1);
    }

    #[test]
    fn diagnostics_in_document_order() {
        let deck = write_deck(
            "order",
            &[(
                "main.sp",
                "* t\nR1 a b\n.subckt s a\nR2 a b\n.ends\nR3 a b\n",
            )],
        );
        let net = load(deck.main.clone());
        let lines: Vec<u32> = net
            .diags
            .iter()
            .filter(|d| d.code == "no_value")
            .map(|d| d.loc.unwrap().line)
            .collect();
        assert_eq!(lines, [2, 4, 6]);
    }

    #[test]
    fn temp_decks_are_removed() {
        let deck = write_deck("removed", &[("main.sp", "* t\nR1 a b 1\n")]);
        let dir = deck.dir.clone();
        assert!(dir.is_dir());
        drop(deck);
        assert!(!dir.exists());
    }

    #[test]
    fn interner_single_copy() {
        // the X line comes first: its spellings are the first ones
        let deck = write_deck(
            "spelling",
            &[(
                "main.sp",
                "* t\nX1 VDD 0 INV WN=2u\n.subckt Inv A b wn=1u\nR1 A b 1k\n.param Pv=1\n.ends\nr2 vdd Gnd 1\n",
            )],
        );
        let net = load(deck.main.clone());
        let name = |s| net.syms.name(s);
        let inv = top_def(&net, "inv");
        assert_eq!(name(inv.name), "INV");
        let ports: Vec<&str> = inv.ports.iter().map(|&p| name(p)).collect();
        assert_eq!(ports, ["A", "b"]);
        assert_eq!(name(inv.defaults[0].name), "WN");
        assert_eq!(name(inv.params[0].name), "Pv");
        assert_eq!(elem_names(&net, inv), ["R1"]);
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["X1", "r2"]);
        assert_eq!(pins(&net, top, element(&net, top, "x1")), ["VDD", "0"]);
        assert_eq!(pins(&net, top, element(&net, top, "R2")), ["VDD", "Gnd"]);
        let x1 = element(&net, top, "x1");
        assert_eq!(x1.callee, Some(inv.name));
        assert_eq!(params(&net, top, x1), [("WN".to_owned(), "2u".to_owned())]);
    }

    #[test]
    fn builtin_names_take_deck_spelling() {
        // `r`, `l`, `w`, `dc`, model types, ... are names that lowering adds
        // itself. Their first spelling in the deck is the one answers show,
        // also when the deck spells them after lowering added them. The
        // deck spells them as a keyword (`DC 1`, `AC=2`, `C=1p`, `L=1n`),
        // as a model type (`.model nch NMOS`), as `.TEMP`, or as any other
        // name (`.param R=2`, a net `Nmos`).
        let deck = write_deck(
            "builtins",
            &[(
                "main.sp",
                "* t\n\
                 .model nch NMOS level=1\n\
                 R1 a b 1k\n\
                 M1 d g s b nch 2u 1u\n\
                 V1 a 0 DC 1 AC=2\n\
                 .param R=2 Dc=3\n\
                 X1 Nmos b inv W=2\n\
                 C1 a b C=1p\n\
                 L1 a b lmod L='2*x'\n\
                 V2 b 0 dc=2 ac 1\n\
                 .model rm R\n\
                 R2 a b rm 1k\n\
                 .TEMP 50\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(p("r1"), [s("R", "1k")]);
        assert_eq!(p("m1"), [s("L", "2u"), s("W", "1u")]);
        assert_eq!(p("v1"), [s("DC", "1"), s("AC", "2")]);
        assert_eq!(p("v2"), [s("DC", "2"), s("AC", "1")]);
        assert_eq!(p("x1"), [s("W", "2")]);
        assert_eq!(p("c1"), [s("C", "1p")]);
        assert_eq!(p("l1"), [s("L", "2*x")]);
        let pins = pins(&net, top, element(&net, top, "x1"));
        assert_eq!(pins, ["NMOS", "b"]);
        let kinds: Vec<&str> = top.models.iter().map(|m| net.syms.name(m.kind)).collect();
        assert_eq!(kinds, ["NMOS", "R"]);
        let names: Vec<&str> = top.params.iter().map(|p| net.syms.name(p.name)).collect();
        assert_eq!(names, ["R", "DC"]);
        assert_eq!(net.syms.name(net.temps[0].name), "TEMP");
        // No deck spelling: the built-in spelling.
        let deck = write_deck(
            "builtins_plain",
            &[(
                "main.sp",
                "* t\n.model nch nmos\nR1 a b 1k\nV1 a 0 1 AC 1 0\nM1 d g s b nch 2u\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        assert_eq!(p("r1"), [s("r", "1k")]);
        assert_eq!(p("v1"), [s("dc", "1"), s("AC", "1"), s("acphase", "0")]);
        assert_eq!(p("m1"), [s("l", "2u")]);
        assert_eq!(net.syms.name(top.models[0].kind), "nmos");
        // `R=` is the first spelling, also with spaces and an expression.
        for line in ["R1 a b R=1k", "R1 a b rmod R = 1k", "R1 a b R='1k'"] {
            let deck = write_deck("builtins_r", &[("main.sp", &format!("* t\n{line}\n"))]);
            let net = load(deck.main.clone());
            let top = &net.defs[ROOT.idx()];
            let r = &top.element_params(element(&net, top, "r1"))[0];
            assert_eq!(net.syms.name(r.name), "R", "{line}");
        }
    }

    #[test]
    fn keyword_value_text() {
        let deck = write_deck(
            "kw_text",
            &[(
                "main.sp",
                "* t\n\
                 R1 a b r=1k\n\
                 C1 a 0 c=10pF\n\
                 R2 2 0 1k r=3k\n\
                 R3 a b 2k\n\
                 + r = 4k $X=9k\n\
                 R4 a b rmod R=5K\n\
                 L1 a b l='2*x'\n\
                 R5 a b 3000 r=3k\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        let s = |a: &str, b: &str| vec![(a.to_owned(), b.to_owned())];
        assert_eq!(p("r1"), s("r", "1k"));
        assert_eq!(p("c1"), s("c", "10pF"));
        assert_eq!(p("r2"), s("r", "3k"));
        assert_eq!(p("r3"), s("r", "4k"));
        assert_eq!(p("r4"), s("r", "5K"));
        assert_eq!(p("l1"), s("l", "2*x"));
        // the text of the winning `r=`, not the positional number
        assert_eq!(p("r5"), s("r", "3k"));
        let r3 = &top.element_params(element(&net, top, "r3"))[0];
        assert_eq!((r3.num, r3.loc.line), (Some(4e3), 6));
    }

    #[test]
    fn key_value_next_line() {
        // `after_eq`: the `=` can end the line before the `+` line of the
        // value (golden `kw_value_next_line`)
        let at = |t: &str, v: &str| super::after_eq(t.as_bytes(), t.rfind(v).unwrap());
        assert!(at("r1 a b r=\n+ 5k", "5k"));
        assert!(at("r1 a b r= $ c\n* c\n\n   \n+ 5k", "5k"));
        assert!(at("r1 a b r=\n+ $ c\n+ 5k", "5k"));
        assert!(at("r1 a b r=\n+ '5k'", "5k'"));
        assert!(at("r1 a b\n+ r=\n+ 5k", "5k"));
        assert!(!at("r1 a b\n+ 5k", "5k"));
        assert!(!at("r1 a b r='a'\n+ 5k", "5k"));
        assert!(!at("r1 a b 'x=$'\n+ 5k", "5k"));
        assert!(!at("+ 5k", "5k"));
        assert!(!at("r1 a b 1k +5k", "5k"));
        let deck = write_deck(
            "kw_next_line",
            &[(
                "main.sp",
                "* t\n\
                 R1 a b r=\n\
                 + 5k\n\
                 R2 a b r= $ c\n\
                 * c\n\
                 \n\
                 + 6k tc1=\n\
                 + 0.004\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        assert_eq!(p("r1"), vec![("r".to_owned(), "5k".to_owned())]);
        assert_eq!(
            p("r2"),
            vec![
                ("r".to_owned(), "6k".to_owned()),
                ("tc1".to_owned(), "0.004".to_owned())
            ]
        );
        let r1 = &top.element_params(element(&net, top, "r1"))[0];
        assert_eq!((r1.num, r1.loc.line), (Some(5e3), 3));
        assert!(
            net.diags
                .iter()
                .all(|d| d.level != crate::api::Level::Error)
        );
    }

    #[test]
    fn element_params_unique() {
        let deck = write_deck(
            "unique",
            &[(
                "main.sp",
                "* t\n\
                 R1 1 0 r=1k r=2k\n\
                 R2 2 0 1k tc1=1 TC1=2\n\
                 M1 d g s b nch 2u 1u l=3u\n\
                 X1 a b inv w=1 W=2 m=2\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(p("r1"), [s("r", "2k")]);
        assert_eq!(p("r2"), [s("r", "1k"), s("tc1", "2")]);
        assert_eq!(p("m1"), [s("w", "1u"), s("l", "3u")]);
        assert_eq!(p("x1"), [s("w", "2"), s("m", "2")]);
    }

    #[test]
    fn alter_in_included_subckt() {
        let deck = write_deck(
            "alter_sub",
            &[
                (
                    "main.sp",
                    "* t\n.subckt holder a b\n.inc 'alt.sp'\nr9 a b 9\n.ends\nx1 1 0 holder\nr0 1 0 1\n",
                ),
                ("alt.sp", "r1 a b 1\n.alter\nr2 a b 2\n"),
            ],
        );
        let net = load(deck.main.clone());
        let holder = top_def(&net, "holder");
        assert_eq!(elem_names(&net, holder), ["r1", "r2", "r9"]);
        // the marker is ignored: everything stays in the first run
        assert!(holder.elements.iter().all(|e| e.alter.is_none()));
        let top = &net.defs[ROOT.idx()];
        assert!(top.elements.iter().all(|e| e.alter.is_none()));
        assert!(net.alters.is_empty());
        let errs: Vec<_> = net
            .diags
            .iter()
            .filter(|d| d.code == "alter_in_subckt")
            .collect();
        assert_eq!(errs.len(), 1, "{:#?}", net.diags);
        assert_eq!(errs[0].level, crate::api::Level::Error);
        let loc = net.loc(errs[0].loc.unwrap());
        assert!(loc.file.ends_with("alt.sp"), "{loc:?}");
        assert_eq!(loc.line, 2);
    }

    #[test]
    fn include_cycle_guard() {
        use netlist_db::{
            FileId, Files, Parsed, ParsedId,
            ast::{ASTBuilder, Segment},
        };
        use std::sync::{Arc, OnceLock};
        // top -> a -> b -> a: a parse result that the parser's file reuse
        // can give
        let include = |id: usize| {
            let res = OnceLock::new();
            assert!(res.set(Ok(ParsedId(id))).is_ok());
            Segment::Include {
                res: Arc::new(res),
                pos: None,
            }
        };
        let file = |name: &str, segments: Vec<Segment>| {
            let id = FileId::Include { path: name.into() };
            let ast = ASTBuilder {
                segments,
                ..Default::default()
            };
            (id, ast)
        };
        let parsed = Parsed {
            top_ids: vec![ParsedId(0)],
            id2idx: std::collections::HashMap::new(),
            inner: vec![
                file("top.sp", vec![include(1), include(2)]),
                file("a.sp", vec![include(2)]),
                file("b.sp", vec![include(1)]),
            ],
            lib_override_matched: Vec::new(),
        };
        let files = Files {
            inner: vec![String::new(); 3],
        };
        let net = super::lower(parsed, files, &LoadOptions::new("top.sp")).unwrap();
        let cycles: Vec<&crate::model::Diag> = net
            .diags
            .iter()
            .filter(|d| d.code == "include_cycle")
            .collect();
        // reported once: `b.sp` is read again from `top.sp`, but its
        // diagnostics are reported on its first visit only
        assert_eq!(cycles.len(), 1, "{:#?}", net.diags);
        assert_eq!(cycles[0].level, crate::api::Level::Error);
        assert!(
            cycles[0]
                .message
                .starts_with(r#"circular include: "a.sp" -> "b.sp" -> "a.sp"."#),
            "{}",
            cycles[0].message
        );
    }

    #[test]
    fn include_cycles_deterministic() {
        // a -> b -> c -> a, and the top file includes `a` and `c`. The
        // parser reads each file once and no longer decides where a cycle
        // closes (that depended on the order in which the files were read).
        // The walk does, in document order: every load gives the same
        // elements and diagnostics.
        let deck = write_deck(
            "cycles_same",
            &[
                ("main.sp", "* t\n.inc 'a.sp'\n.inc 'c.sp'\n"),
                ("a.sp", ".inc 'b.sp'\nra 1 0 1\n"),
                ("b.sp", ".inc 'c.sp'\nrb 1 0 1\n"),
                ("c.sp", ".inc 'a.sp'\nrc 1 0 1\n"),
            ],
        );
        let dir = format!("{}/", deck.dir.display());
        let run = || {
            let net = load(deck.main.clone());
            let top = &net.defs[ROOT.idx()];
            let elements: Vec<String> = top
                .elements
                .iter()
                .map(|e| {
                    let loc = net.loc(e.loc);
                    format!("{} {}:{}", net.syms.name(e.name), loc.file, loc.line).replace(&dir, "")
                })
                .collect();
            let diags: Vec<(crate::api::Level, String, String)> = diags_of(&net, "include_cycle")
                .into_iter()
                .map(|(l, at, m)| (l, at, m.replace(&dir, "")))
                .collect();
            let all: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
            (elements, diags, all.join(" "))
        };
        let first = run();
        for _ in 0..19 {
            assert_eq!(run(), first);
        }
        let (elements, cycles, all) = first;
        // main -> a -> b -> c (its include of `a` closes a cycle), then
        // main -> c -> a -> b (its include of `c` closes one; a second read
        // reports nothing again)
        assert_eq!(
            elements,
            [
                "rc c.sp:2",
                "rb b.sp:2",
                "ra a.sp:2",
                "rb b.sp:2",
                "ra a.sp:2",
                "rc c.sp:2"
            ]
        );
        assert_eq!(
            cycles,
            [(
                crate::api::Level::Error,
                "c.sp:1".to_owned(),
                r#"circular include: "a.sp" -> "b.sp" -> "c.sp" -> "a.sp". The include of "a.sp" in "c.sp" was skipped"#
                    .to_owned()
            )]
        );
        // `ra`, `rb`, `rc` are read twice at the top level: HSPICE rejects
        // two lines with one name there (golden `elem_dup_name`)
        assert_eq!(
            all,
            "title_line include_cycle duplicate_element duplicate_element duplicate_element"
        );
        // A file that includes itself: one cycle, at its include line.
        let deck = write_deck(
            "cycle_self",
            &[("main.sp", "* t\nR1 a b 1\n.inc 'main.sp'\nR2 a b 2\n")],
        );
        let net = load(deck.main.clone());
        assert_eq!(elem_names(&net, &net.defs[ROOT.idx()]), ["R1", "R2"]);
        let cycles = diags_of(&net, "include_cycle");
        assert_eq!(cycles.len(), 1, "{:#?}", net.diags);
        assert_eq!(cycles[0].1, "main.sp:3");
        // A cycle of `.lib` sections of one file: the message names the
        // sections.
        let deck = write_deck(
            "cycle_sections",
            &[
                ("main.sp", "* t\n.lib 'm.lib' tt\n"),
                (
                    "m.lib",
                    ".lib tt\nRtt a b 1\n.lib 'm.lib' ff\n.endl tt\n.lib ff\nRff a b 2\n.lib 'm.lib' tt\n.endl ff\n",
                ),
            ],
        );
        let net = load(deck.main.clone());
        assert_eq!(elem_names(&net, &net.defs[ROOT.idx()]), ["Rtt", "Rff"]);
        let cycles = diags_of(&net, "include_cycle");
        assert_eq!(cycles.len(), 1, "{:#?}", net.diags);
        assert_eq!(cycles[0].1, "m.lib:7");
        let message = cycles[0].2.replace(&format!("{}/", deck.dir.display()), "");
        assert_eq!(
            message,
            r#"circular include: "m.lib (section tt)" -> "m.lib (section ff)" -> "m.lib (section tt)". The include of "m.lib (section tt)" in "m.lib (section ff)" was skipped"#
        );
    }

    /// The diagnostics with `code`: (level, file name and line, message).
    fn diags_of(net: &Netlist, code: &str) -> Vec<(crate::api::Level, String, String)> {
        net.diags
            .iter()
            .filter(|d| d.code == code)
            .map(|d| {
                let at = d.loc.map_or_else(String::new, |l| {
                    let loc = net.loc(l);
                    let name = Path::new(&loc.file).file_name().unwrap().to_string_lossy();
                    format!("{name}:{}", loc.line)
                });
                (d.level, at, d.message.clone())
            })
            .collect()
    }

    #[test]
    fn source_lowering() {
        // Golden `src_forms`, `src_ac_phase`, `src_order_ac_first`: the DC
        // value as `dc`, the AC magnitude and phase as `ac` and `acphase`,
        // a function by its name with its arguments only. These decks
        // spell the keywords `DC` and `AC`, so answers show that spelling.
        let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        let net = load(repo("golden/cases/src_forms/deck.sp"));
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        assert_eq!(p("v1"), [s("DC", "1.2")]);
        assert_eq!(p("v2"), [s("DC", "1"), s("PULSE", "0 1 0 1n 1n 5n 10n")]);
        assert_eq!(p("v3"), [s("DC", "1.2"), s("AC", "1"), s("acphase", "0")]);
        assert_eq!(p("v4"), [s("PULSE", "0 1 0 1n 1n 5n 10n")]);
        assert_eq!(p("v5"), [s("DC", "2")]);
        assert_eq!(p("i1"), [s("DC", "1m")]);
        assert_eq!(p("v7"), [s("PULSE", "0.3 1 0 1n 1n 5n 10n")]);
        let dc = &top.element_params(element(&net, top, "i1"))[0];
        assert_eq!((dc.num, dc.loc.line), (Some(1e-3), 8));
        assert!(
            net.diags
                .iter()
                .all(|d| d.level != crate::api::Level::Error)
        );

        let net = load(repo("golden/cases/src_ac_phase/deck.sp"));
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        assert_eq!(p("v1"), [s("DC", "1.2"), s("AC", "1"), s("acphase", "45")]);
        assert_eq!(p("v2"), [s("DC", "0.5"), s("AC", "2"), s("acphase", "90")]);
        let net = load(repo("golden/cases/src_order_ac_first/deck.sp"));
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        assert_eq!(p("v1"), [s("DC", "0.7"), s("AC", "1"), s("acphase", "45")]);
        let acphase = &top.element_params(element(&net, top, "v1"))[2];
        assert_eq!(acphase.num, Some(45.0));

        // Expressions, `key=value`, and a function on `+` lines.
        let deck = write_deck(
            "sources",
            &[(
                "main.sp",
                "* t\n\
                 V1 a 0 'vdd/2' AC=1 foo=2\n\
                 V2 a 0 SIN(0 1\n\
                 + 1k) $ c\n\
                 V3 a 0 PWL(0 0 $ start\n\
                 * a comment line\n\
                 + 1n 1)\n\
                 V4 a 0\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        assert_eq!(p("v1"), [s("dc", "vdd/2"), s("AC", "1"), s("foo", "2")]);
        assert_eq!(p("v2"), [s("SIN", "0 1\n+ 1k")]);
        let f = &top.element_params(element(&net, top, "v2"))[0];
        assert_eq!(f.loc.line, 3);
        // The stored text keeps the `+` markers and comments; answers show
        // it on one line through `param_text`.
        assert_eq!(net.param_text(f).as_deref(), Some("0 1 1k"));
        let f = &top.element_params(element(&net, top, "v3"))[0];
        assert_eq!(
            net.text(f.text.unwrap()),
            "0 0 $ start\n* a comment line\n+ 1n 1"
        );
        assert_eq!(net.param_text(f).as_deref(), Some("0 0 1n 1"));
        let v1 = &top.element_params(element(&net, top, "v1"))[0];
        assert!(matches!(net.param_text(v1), Some(Cow::Borrowed("vdd/2"))));
        // A `$` inside quotes is not a comment (golden `rcl_quoted_dollar`,
        // `rcl_quoted_dollar_line`): it stays, with the text after it; a
        // `$` outside quotes starts one, also after a quoted word.
        assert_eq!(one_line("rval*2 $ c", true), "rval*2 $ c");
        assert_eq!(one_line("rval $ c\n+ *2", true), "rval $ c *2");
        assert_eq!(one_line("rval\n+ *3", true), "rval *3");
        assert_eq!(one_line("0 '0.3 $ c' 1n 1 $ d", false), "0 '0.3 $ c' 1n 1");
        assert_eq!(one_line("0 'a' 1n $ it's\n+ 1", false), "0 'a' 1n 1");
        assert_eq!(one_line("0 0 $ c\n* it's\n+ 1n 1", false), "0 0 1n 1");
        // A `*` comment line inside quotes stays on a line of its own
        // (golden `quoted_cmtline`); outside quotes it is passed over.
        assert_eq!(one_line("a\n* c\n+ *5", true), "a\n* c\n*5");
        assert_eq!(one_line("\n* c\n+ a", true), "\n* c\na");
        assert_eq!(
            one_line("0 '0.3\n* c\n+ *1' 1n", false),
            "0 '0.3\n* c\n*1' 1n"
        );
        assert_eq!(one_line("'a' 1n\n* it's\n+ 1", false), "'a' 1n 1");
        assert_eq!(quote_fault("a\n* c\n*5"), Some(QuoteFault::CommentLine));
        assert_eq!(quote_fault("a $ c *2"), Some(QuoteFault::Dollar));
        assert_eq!(quote_fault("0 'a' 1n 1"), None);
        for (t, quoted) in [
            ("a\n* c\n+ *5", true),
            ("a $ c\n* d\n+ *5", true),
            ("a $ c\n+ *5", true),
            ("a\n+ *5", true),
            ("0 '0.3\n* c\n+ *1' 1n", false),
            ("'a' 1n\n* it's\n+ 1", false),
            ("'a' $ it's\n+ '1 $ c'", false),
            ("0 0 $ c\n* it's\n+ 1n 1", false),
            ("x*2 $ c", true),
            ("\n* c\n+ a", true),
            ("a\n\n+ *5", true),
            ("a\n   \t\n+ *5", true),
            ("a\n$ c\n+ *5", true),
            ("a\n+\n+ *5", true),
            ("max(1k,\n\n+ 3k)", false),
            ("max(1k,\n$ c\n+ 3k)", false),
            ("max(1k,\n* c\n+ 3k)", false),
            ("'(' x\n* c\n+ 1", false),
            ("0 0.3\n* c\n+ 1n 1", false),
        ] {
            // `quote_fault` does not tell a comment line in parentheses
            // from one in quotes
            let raw = super::raw_quote_fault(t, quoted).map(|f| match f {
                QuoteFault::ParenCommentLine => QuoteFault::CommentLine,
                f => f,
            });
            assert_eq!(raw, quote_fault(&one_line(t, quoted)), "{t:?}");
        }
        // Inside quotes, a line that is not a `+` line stays on a line of
        // its own: a blank line (golden `quoted_blank_line`), a line with
        // only spaces and a tab (`quoted_spaces_elem`) and a `$` comment
        // line (`quoted_dollarline_elem`). A `+` line with no text is
        // passed over (`quoted_plus_empty_elem`).
        assert_eq!(one_line("a\n\n+ *5", true), "a\n\n*5");
        assert_eq!(one_line("a\n   \t\n+ *5", true), "a\n\n*5");
        assert_eq!(one_line("a\n$ c\n+ *5", true), "a\n$ c\n*5");
        assert_eq!(one_line("a\n+\n+ *5", true), "a *5");
        for t in ["a\n\n+ *5", "a\n   \t\n+ *5", "a\n$ c\n+ *5"] {
            assert_eq!(
                super::raw_quote_fault(t, true),
                Some(QuoteFault::CommentLine)
            );
        }
        assert_eq!(super::raw_quote_fault("a\n+\n+ *5", true), None);
        // Inside open parentheses with no quotes, a blank line and a `$`
        // comment line are passed over (golden `paren_blank_call`), and a
        // `*` comment line stays (golden `paren_cmtline_call`). With the
        // parentheses closed, it is passed over (`paren_cmtline_pwl`: the
        // argument text of a source function has no outer parentheses).
        assert_eq!(one_line("max(1k,\n\n+ 3k)", false), "max(1k, 3k)");
        assert_eq!(one_line("max(1k,\n$ c\n+ 3k)", false), "max(1k, 3k)");
        assert_eq!(one_line("max(1k,\n* c\n+ 3k)", false), "max(1k,\n* c\n3k)");
        assert_eq!(
            super::raw_quote_fault("max(1k,\n* c\n+ 3k)", false),
            Some(QuoteFault::ParenCommentLine)
        );
        assert_eq!(one_line("0 0.3\n* c\n+ 1n 1", false), "0 0.3 1n 1");
        assert_eq!(super::raw_quote_fault("0 0.3\n* c\n+ 1n 1", false), None);
        // a `(` inside quotes does not count
        assert_eq!(one_line("'(' x\n* c\n+ 1", false), "'(' x 1");
        assert_eq!(super::raw_quote_fault("'(' x\n* c\n+ 1", false), None);
        // No source part: a 0 V source with no parameters (HSPICE manual).
        assert!(p("v4").is_empty());
        assert!(
            net.diags
                .iter()
                .all(|d| d.level != crate::api::Level::Error),
            "{:#?}",
            net.diags
        );
        // `ac_then_function`: a function with no space in it, after the AC
        // magnitude, is the function (it was the AC phase before).
        let deck = write_deck(
            "ac_then_function",
            &[(
                "main.sp",
                "* t\n\
                 V1 a 0 AC 1 SIN(0,1,1meg)\n\
                 V2 b 0 DC 0 AC 1 PULSE(0,1.8,0,1n,1n,5n,10n)\n\
                 V3 c 0 AC=1 PWL(0,0,1n,1)\n\
                 V4 d 0 PULSE(0 1) AC 1 SIN(0,1,1k)\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        assert_eq!(p("v1"), [s("SIN", "0,1,1meg"), s("AC", "1")]);
        assert_eq!(
            p("v2"),
            [
                s("DC", "0"),
                s("PULSE", "0,1.8,0,1n,1n,5n,10n"),
                s("AC", "1")
            ]
        );
        assert_eq!(p("v3"), [s("PWL", "0,0,1n,1"), s("AC", "1")]);
        assert_eq!(elem_names(&net, top), ["V1", "V2", "V3"]);
        let errs = diags_of(&net, "bad_source");
        assert_eq!(errs.len(), 1, "{:#?}", net.diags);
        assert_eq!(errs[0].1, "main.sp:5");
        assert!(
            errs[0].2.contains("two transient functions"),
            "{}",
            errs[0].2
        );
        // `PULSE(...) DC 0.7` is rejected (golden `src_order_fn_first`).
        let net = load(repo("golden/cases/src_order_fn_first/deck.sp"));
        let errs = diags_of(&net, "bad_source");
        assert_eq!(errs.len(), 1, "{:#?}", net.diags);
        assert!(errs[0].2.contains("Write `DC` first"), "{}", errs[0].2);
    }

    #[test]
    fn rcl_order_lowering() {
        let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        // Golden `res_tc_positional`: the numbers after the value are tc1
        // and tc2.
        let net = load(repo("golden/cases/res_tc_positional/deck.sp"));
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        assert_eq!(p("r1"), [s("r", "1k"), s("tc1", "0.004")]);
        assert_eq!(p("r2"), [s("r", "1k"), s("tc1", "0.004"), s("tc2", "1e-5")]);
        assert_eq!(p("r3"), p("r1"));
        assert_eq!(p("r4"), p("r2"));
        // The numbers after a value or a model name keep their text as
        // written: on `+` lines, after a value that `r=` replaces, and
        // after `OFF`, which takes the place of `l`.
        let deck = write_deck(
            "positional_text",
            &[(
                "main.sp",
                "* t\n\
                 R9 a 0 1k\n\
                 + 4m $ c\n\
                 + 1e-5\n\
                 R20 a 0 1k 0.004 r=2k\n\
                 M6 d g s b nch off 2u\n\
                 + 1u\n\
                 D1 a 0 dmod 2 1e-6\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        assert_eq!(p("r9"), [s("r", "1k"), s("tc1", "4m"), s("tc2", "1e-5")]);
        assert_eq!(p("r20"), [s("r", "2k"), s("tc1", "0.004")]);
        assert_eq!(p("m6"), [s("w", "2u"), s("ad", "1u")]);
        assert_eq!(p("d1"), [s("area", "2"), s("pj", "1e-6")]);
        let tc2 = &top.element_params(element(&net, top, "r9"))[2];
        assert_eq!((tc2.num, tc2.loc.line), (Some(1e-5), 4));
        // The value after the nodes or the model name keeps its text and its
        // line also on a `+` line and before a `$` comment.
        let deck = write_deck(
            "value_text",
            &[(
                "main.sp",
                "* t\n\
                 R1 a b rmod\n\
                 + 2k\n\
                 R3 a b\n\
                 * c\n\
                 + 1k $ c\n\
                 R2 a b 1k$c\n\
                 L1 a b\n\
                 + 1n lmod\n\
                 C1 a b 1p\n\
                 + 0.004\n\
                 R4 a b 2k\n\
                 R5 a b 3k\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        for (name, key, want, line) in [
            ("R1", "r", "2k", 3),
            ("R3", "r", "1k", 6),
            ("R2", "r", "1k", 7),
            ("L1", "l", "1n", 9),
            ("C1", "c", "1p", 10),
            ("R4", "r", "2k", 12),
        ] {
            let v = &top.element_params(element(&net, top, name))[0];
            assert_eq!(net.syms.name(v.name), key, "{name}");
            assert_eq!(
                (text(&net, v), v.loc.line),
                (want.to_owned(), line),
                "{name}"
            );
        }
        // Golden `ind_value_model`: L reads "value, then model".
        let net = load(repo("golden/cases/ind_value_model/deck.sp"));
        let top = &net.defs[ROOT.idx()];
        let l1 = element(&net, top, "l1");
        assert_eq!(l1.model, net.syms.get("lmod"));
        assert_eq!(params(&net, top, l1), [s("l", "1n")]);
        // Golden `res_value_model`, `cap_value_model`: an error that says
        // what HSPICE does.
        for (case, line, what) in [
            ("res_value_model", 5, "reads the value as a third node"),
            ("cap_value_model", 5, "uses a capacitance of 0"),
        ] {
            let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
            let errs = diags_of(&net, "value_then_model");
            assert_eq!(errs.len(), 1, "{case}: {:#?}", net.diags);
            assert_eq!(errs[0].0, crate::api::Level::Error);
            assert_eq!(errs[0].1, format!("deck.sp:{line}"), "{case}");
            assert!(errs[0].2.contains(what), "{case}: {}", errs[0].2);
        }
        // Golden `mos_digit_model_ref`: a model name that starts with a
        // digit.
        let net = load(repo("golden/cases/mos_digit_model_ref/deck.sp"));
        let errs = diags_of(&net, "digit_first_model");
        assert!(!errs.is_empty(), "{:#?}", net.diags);
        assert!(errs[0].2.contains("`5vnmos`"), "{}", errs[0].2);
        // A token before the value where a model name goes: a value with a
        // unit, or a name with a character that model names do not have.
        // Comment-only `+` lines (any number) keep the element.
        let deck = write_deck(
            "model_place",
            &[(
                "main.sp",
                "* t\n\
                 C1 a b 10pF 0.004\n\
                 R1 a b 1kohm r=2k\n\
                 R2 a b rmod-1 2k\n\
                 R6 a b 1k $X=1\n\
                 + $Y=2\n\
                 + $Z=3\n\
                 R7 a b 2k\n\
                 M1 d g s b e 5vnmos\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["R6", "R7", "M1"]);
        let p = |name: &str| params(&net, top, element(&net, top, name));
        assert_eq!(p("r6"), [s("r", "1k")]);
        assert_eq!(p("m1"), [s("l", "5vnmos")]);
        assert_eq!(element(&net, top, "m1").model, net.syms.get("e"));
        let digit = diags_of(&net, "digit_first_model");
        let at: Vec<&str> = digit.iter().map(|e| e.1.as_str()).collect();
        assert_eq!(at, ["main.sp:2", "main.sp:3"], "{:#?}", net.diags);
        assert!(
            digit[0].2.starts_with(
                "a capacitor line has `10pF` before its value: R and C take \"model, then value\""
            ),
            "{}",
            digit[0].2
        );
        let bad = diags_of(&net, "bad_model_name");
        assert_eq!(bad.len(), 1, "{:#?}", net.diags);
        assert_eq!(bad[0].1, "main.sp:4");
        assert!(bad[0].2.contains("`rmod-1`"), "{}", bad[0].2);
        assert_eq!(
            net.diags
                .iter()
                .filter(|d| d.level == crate::api::Level::Error)
                .count(),
            3,
            "{:#?}",
            net.diags
        );
    }

    #[test]
    fn after_end_info() {
        // Golden `after_end`: the lines after `.end` are not read.
        let net = load(repo("golden/cases/after_end/deck.sp"));
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["v1", "r1"]);
        assert_eq!(top.params.len(), 1);
        let info = diags_of(&net, "after_end");
        assert_eq!(
            info,
            [(
                crate::api::Level::Info,
                "deck.sp:6".to_owned(),
                "3 lines after `.end` were not read: they do not join this deck, as in HSPICE. A second deck after `.end` (HSPICE runs it as a second job) is not loaded".to_owned()
            )]
        );
        // Only comments after `.end`: nothing to say.
        let deck = write_deck("end_comments", &[("main.sp", "* t\nR1 a b 1\n.end\n* c\n")]);
        let net = load(deck.main.clone());
        assert!(diags_of(&net, "after_end").is_empty());
        // A `.lib` section after `.end` is read by its `.lib` call; only
        // the other lines are "not read".
        let deck = write_deck(
            "end_lib",
            &[(
                "main.sp",
                "* t\n.lib 'main.sp' tt\nR1 a b p\n.end\n.lib tt\n.param p=3\n.endl tt\nR2 a b 2\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["R1"]);
        assert_eq!(top.params.len(), 1);
        let info = diags_of(&net, "after_end");
        assert_eq!(info.len(), 1, "{:#?}", net.diags);
        assert_eq!(
            info[0].2,
            "1 line after `.end` was not read: it does not join this deck, as in HSPICE. A second deck after `.end` (HSPICE runs it as a second job) is not loaded"
        );
        let deck = write_deck(
            "end_lib_only",
            &[(
                "main.sp",
                "* t\n.lib 'main.sp' tt\n.end\n.lib tt\n.param p=3\n.endl tt\n",
            )],
        );
        let net = load(deck.main.clone());
        assert!(diags_of(&net, "after_end").is_empty(), "{:#?}", net.diags);
    }

    #[test]
    fn non_utf8_warning() {
        // Golden `nonutf8_comment`: the deck is read, with a warning.
        let net = load(repo("golden/cases/nonutf8_comment/deck.sp"));
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["v1", "r1"]);
        let warn = diags_of(&net, "non_utf8");
        assert_eq!(warn.len(), 1, "{:#?}", net.diags);
        assert_eq!(warn[0].0, crate::api::Level::Warning);
        assert_eq!(warn[0].1, "deck.sp:2");
        assert!(warn[0].2.contains("U+FFFD"), "{}", warn[0].2);
        // Once per file, also when the file is included twice.
        let deck = write_deck(
            "non_utf8",
            &[("main.sp", "* t\n.inc 'a.sp'\n.inc 'a.sp'\n"), ("a.sp", "")],
        );
        std::fs::write(deck.dir.join("a.sp"), b"R\xe91 a b 1\n").unwrap();
        let net = load(deck.main.clone());
        let warn = diags_of(&net, "non_utf8");
        assert_eq!(warn.len(), 1, "{:#?}", net.diags);
        assert_eq!(warn[0].1, "a.sp:1");
        // One file read for two `.lib` sections: one warning, at the first
        // bad line in a section that was read. A bad byte outside the
        // sections that were read gives none.
        let deck = write_deck(
            "non_utf8_lib",
            &[(
                "main.sp",
                "* t\n.lib 'm.lib' tt\n.lib 'm.lib' ff\n.lib 'n.lib' tt\n",
            )],
        );
        std::fs::write(
            deck.dir.join("m.lib"),
            b".lib tt\nR1 a b 1 $ \xe9\n.endl tt\n.lib ff\nR2 a b 2 $ \xe9\n.endl ff\n",
        )
        .unwrap();
        std::fs::write(
            deck.dir.join("n.lib"),
            b".lib tt\nR3 a b 3\n.endl tt\n* \xe9\n.lib ff\nR4 a b 4\n.endl ff\n",
        )
        .unwrap();
        let net = load(deck.main.clone());
        let warn = diags_of(&net, "non_utf8");
        assert_eq!(warn.len(), 1, "{:#?}", net.diags);
        assert_eq!(warn[0].1, "m.lib:2");
    }

    #[test]
    fn comment_only_plus_at_end_of_file() {
        // A comment-only `+` line that ends the file with no line end after
        // it continues the statement, as with the line end: the statement
        // is kept, with no diagnostic of its own (`X1` calls `inv`, which
        // the deck does not define, so the hierarchy analysis gives the
        // Error `unresolved_subckt` for it).
        for (k, (body, elem)) in [
            ("R1 a b 1k $X=1\n+ $Y=2", Some("R1")),
            ("X1 a b inv $c\n+ $d", Some("X1")),
            ("V1 a 0 DC 1 $c\n+ $d", Some("V1")),
            ("R1 a b 1k $c\n+", Some("R1")),
            ("R1 a b 1k $c\n+ $d\n+ $e", Some("R1")),
            (".param a=1 $c\n+ $d", None),
        ]
        .into_iter()
        .enumerate()
        {
            // CRLF also on every line, not only at the end
            let crlf = body.replace('\n', "\r\n");
            for (body, end) in [
                (body, ""),
                (body, "\n"),
                (body, "\r\n"),
                (crlf.as_str(), ""),
                (crlf.as_str(), "\r\n"),
            ] {
                let text = if end.is_empty() || end == "\n" {
                    format!("* t\n{body}{end}")
                } else {
                    format!("* t\r\n{body}{end}")
                };
                let deck = write_deck(&format!("plus_eof_{k}"), &[("main.sp", &text)]);
                let net = load(deck.main.clone());
                let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
                // `X1` calls `inv`, which the deck does not define: the
                // hierarchy analysis reports it
                let want: &[&str] = if elem == Some("X1") {
                    &["title_line", "unresolved_subckt"]
                } else {
                    &["title_line"]
                };
                assert_eq!(codes, want, "{text:?}: {:#?}", net.diags);
                let top = &net.defs[ROOT.idx()];
                match elem {
                    Some(name) => assert_eq!(elem_names(&net, top), [name], "{text:?}"),
                    None => assert_eq!(top.params.len(), 1, "{text:?}"),
                }
            }
        }
    }

    #[test]
    fn param_braces() {
        // Golden `param_braces`, `param_braces_param`: HSPICE rejects
        // `{...}`, and the error says so.
        let net = load(repo("golden/cases/param_braces/deck.sp"));
        let errs = diags_of(&net, "braces");
        let lines: Vec<&str> = errs.iter().map(|e| e.1.as_str()).collect();
        assert_eq!(lines, ["deck.sp:2", "deck.sp:4"], "{:#?}", net.diags);
        for e in &errs {
            assert!(e.2.contains("Write the expression in quotes"), "{}", e.2);
        }
        let net = load(repo("golden/cases/param_braces_param/deck.sp"));
        assert_eq!(diags_of(&net, "braces").len(), 1, "{:#?}", net.diags);
        // Also after `DC` and `AC` on a source line, where the parser says
        // that no value follows.
        // Also in the arguments of a source function, in or out of
        // parentheses.
        let deck = write_deck(
            "braces_source",
            &[(
                "main.sp",
                "* t\nV1 a 0 DC {x}\nV2 b 0 AC {x}\nV3 a 0 PULSE({x} 1 0 1n)\nV4 a 0 SIN(0 1\n+ {f})\nV5 a 0 PULSE {x} 1 0 1n\nV6 a 0 PULSE(0 1 0 1n) $ {x}\nR1 a b 1\n",
            )],
        );
        let net = load(deck.main.clone());
        let lines: Vec<String> = diags_of(&net, "braces").into_iter().map(|e| e.1).collect();
        assert_eq!(
            lines,
            [
                "main.sp:2",
                "main.sp:3",
                "main.sp:4",
                "main.sp:5",
                "main.sp:7"
            ],
            "{:#?}",
            net.diags
        );
        assert!(diags_of(&net, "bad_source").is_empty(), "{:#?}", net.diags);
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["V6", "R1"]);
        // In quotes too (golden `brace_quoted_param`, `brace_quoted_elem`,
        // `brace_quoted_fn`): HSPICE reads `{a}` there as the name `[a]`
        // and rejects the deck. Each line gets `braces` (they were kept
        // with no diagnostic before, and `'{c}'` read as `c`), also with
        // another error on the line.
        let deck = write_deck(
            "braces_quoted",
            &[(
                "main.sp",
                "* t\nR2 a b '{c}'\n.param p='{2}'\nV5 a 0 PWL(0 0 1n '{x}')\nV7 a 0 PWL(0 0 1n '{x}') PULSE(0 1)\nR8 a b 1\n",
            )],
        );
        let net = load(deck.main.clone());
        let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            ["title_line", "braces", "braces", "braces", "braces"],
            "{:#?}",
            net.diags
        );
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["R8"]);
        assert!(top.params.is_empty());
    }

    #[test]
    fn braces_quoted_golden() {
        // Golden `brace_quoted_param`, `brace_quoted_elem`,
        // `brace_quoted_fn`, `brace_fn_unquoted`: HSPICE aborts on `{a}`
        // inside quotes in all three places, as on `{a}` without quotes.
        // One `braces` Error at the line, which is skipped: no value for
        // it, never a number.
        for (case, line, elements, params) in [
            ("brace_quoted_param", "deck.sp:2", &["v1", "R1"][..], 0),
            ("brace_quoted_elem", "deck.sp:4", &["v1"], 1),
            ("brace_quoted_fn", "deck.sp:4", &["r1"], 1),
            ("brace_fn_unquoted", "deck.sp:4", &["r1"], 1),
        ] {
            let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
            let errs = diags_of(&net, "braces");
            assert_eq!(errs.len(), 1, "{case}: {:#?}", net.diags);
            assert_eq!(errs[0].0, crate::api::Level::Error, "{case}");
            assert_eq!(errs[0].1, line, "{case}");
            assert!(errs[0].2.contains("also inside quotes"), "{}", errs[0].2);
            let errors = net
                .diags
                .iter()
                .filter(|d| d.level == crate::api::Level::Error)
                .count();
            assert_eq!(errors, 1, "{case}: {:#?}", net.diags);
            let top = &net.defs[ROOT.idx()];
            assert_eq!(elem_names(&net, top), elements, "{case}");
            assert_eq!(top.params.len(), params, "{case}");
        }
        // Golden `brace_quoted_temp`, `brace_temp`: HSPICE aborts on `{a}`
        // in a `.temp` value too, in quotes or not. The line gets `braces`
        // (the temperature was lost with no diagnostic before), and it
        // gives no temperature.
        for case in ["brace_quoted_temp", "brace_temp"] {
            let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
            let errs = diags_of(&net, "braces");
            assert_eq!(errs.len(), 1, "{case}: {:#?}", net.diags);
            assert_eq!(errs[0].1, "deck.sp:3", "{case}");
            let errors = net
                .diags
                .iter()
                .filter(|d| d.level == crate::api::Level::Error)
                .count();
            assert_eq!(errors, 1, "{case}: {:#?}", net.diags);
            assert!(net.temps.is_empty(), "{case}");
            let top = &net.defs[ROOT.idx()];
            assert_eq!(elem_names(&net, top), ["v1", "R1"], "{case}");
        }
        // Braces in a parameter of a D, Q or M line, or in another analysis
        // command, also give `braces`, not the error that the failed
        // parameter causes (`D1` gave `node_count`: the failed `area=`
        // read as a node).
        let deck = write_deck(
            "braces_shape",
            &[(
                "main.sp",
                "* t\n.model dmod d\nD1 a k dmod area='{a}'\nD2 a k dmod area={a}\nQ1 c b e qmod area='{a}'\nM1 d g s b nch l='{a}'\nM2 d g s b nch 1u w='{a}'\n.tran 1n '{a}'\n.tran 1n 2n\nR8 a b 1\n",
            )],
        );
        let net = load(deck.main.clone());
        let lines: Vec<String> = diags_of(&net, "braces").into_iter().map(|e| e.1).collect();
        assert_eq!(
            lines,
            (3..=8).map(|n| format!("main.sp:{n}")).collect::<Vec<_>>(),
            "{:#?}",
            net.diags
        );
        let errors = net
            .diags
            .iter()
            .filter(|d| d.level == crate::api::Level::Error)
            .count();
        assert_eq!(errors, 6, "{:#?}", net.diags);
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["R8"]);
        // `brace_quoted_param`: no `p` (nor `a`, on the same line), so the
        // `p` of R1 has no value.
        let net = load(repo("golden/cases/brace_quoted_param/deck.sp"));
        assert!(
            net.syms
                .get("p")
                .is_none_or(|p| { net.defs[ROOT.idx()].params.iter().all(|d| d.name != p) })
        );
        // A `{` on a `+` line after a `$` comment line (the lowering passed
        // over `*` comment lines only, so it gave `syntax_error`), and in
        // quotes after a `*` comment line.
        let deck = write_deck(
            "braces_dollar_line",
            &[(
                "main.sp",
                "* t\nR6 a b 1k\n$ c\n+ m={x}\nR7 a b 1k\n* c\n$ d\n+ m='{x}'\nR8 a b 1\n",
            )],
        );
        let net = load(deck.main.clone());
        let lines: Vec<String> = diags_of(&net, "braces").into_iter().map(|e| e.1).collect();
        assert_eq!(lines, ["main.sp:2", "main.sp:5"], "{:#?}", net.diags);
        assert!(
            diags_of(&net, "syntax_error").is_empty(),
            "{:#?}",
            net.diags
        );
        assert!(
            diags_of(&net, "trailing_text").is_empty(),
            "{:#?}",
            net.diags
        );
    }

    #[test]
    fn pwl_repeat_golden() {
        // Golden `pwl_repeat`, `pwl_repeat_td`, `pwl_td`: HSPICE accepts
        // `R`, `R=0 TD=1n` and `TD=1n` after the `)` of a PWL (`R` gave
        // `trailing_text` and lost V1 before). `R` alone is `R=0` (repeat
        // time 0 in `.option list` for both); the PWL keeps its points.
        // Golden `pwl_repeat_noparen`, `pwl_repeat_key_bare`, `pwl_ac_r`:
        // `R` is the repeat flag also after PWL values with no parentheses,
        // after `R=1n` (the last one wins), and after the AC magnitude.
        let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        for (case, want) in [
            (
                "pwl_repeat",
                vec![s("PWL", "0 0.3 1n 1"), s("R", "Some(0.0)")],
            ),
            (
                "pwl_repeat_td",
                vec![s("PWL", "0 0.3 1n 1"), s("R", "0"), s("TD", "1n")],
            ),
            ("pwl_td", vec![s("PWL", "0 0.3 1n 1"), s("TD", "1n")]),
            // `R` is the repeat flag, not a fifth argument
            (
                "pwl_repeat_noparen",
                vec![s("PWL", "0 0.3 1n 1"), s("R", "Some(0.0)")],
            ),
            // `R=1n R`: the bare `R` wins (repeat delay 0)
            (
                "pwl_repeat_key_bare",
                vec![s("PWL", "0 0.3 1n 1"), s("R", "Some(0.0)")],
            ),
            // `AC 1 R`: `R` is the repeat flag, not the AC phase (phase 0)
            (
                "pwl_ac_r",
                vec![s("PWL", "0 0.3 1n 1"), s("AC", "1"), s("R", "Some(0.0)")],
            ),
            // `R=1n` before the last time point: repeat delay 1n
            (
                "pwl_repeat_key_mid",
                vec![s("PWL", "0 0.3 1n 1 2n 0.5"), s("R", "1n")],
            ),
            // `R=1n R`: the last one wins (repeat delay 0)
            (
                "pwl_repeat_key_bare_mid",
                vec![s("PWL", "0 0.3 1n 1 2n 0.5"), s("R", "Some(0.0)")],
            ),
        ] {
            let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
            let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
            assert_eq!(codes, ["title_line", "analysis_commands"], "{case}");
            let top = &net.defs[ROOT.idx()];
            assert_eq!(elem_names(&net, top), ["V1", "r1"], "{case}");
            assert_eq!(params(&net, top, element(&net, top, "v1")), want, "{case}");
        }
        let net = load(repo("golden/cases/pwl_repeat/deck.sp"));
        let top = &net.defs[ROOT.idx()];
        let r = &top.element_params(element(&net, top, "v1"))[1];
        assert_eq!((r.num, r.loc.line), (Some(0.0), 3));
    }

    #[test]
    fn pwl_points_golden() {
        // Golden `pwl_r_param_last`: a parameter named `r` as the last word
        // of a PWL with no parentheses is the repeat flag (repeat delay 0),
        // not the value of `.param r=2`, and HSPICE warns about the last
        // time point, which has no value. Golden `pwl_repeat_key_only`:
        // `R=` at the last time point drops the repeat, with a warning.
        // The values and `R` are kept as written, with a Warning (there
        // was no diagnostic before).
        let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        for (case, code, line, want) in [
            (
                "pwl_r_param_last",
                "pwl_odd_values",
                "deck.sp:4",
                vec![s("PWL", "0 0.3 1n"), s("r", "Some(0.0)")],
            ),
            (
                "pwl_repeat_key_only",
                "pwl_repeat_dropped",
                "deck.sp:3",
                vec![s("PWL", "0 0.3 1n 1"), s("R", "1n")],
            ),
        ] {
            let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
            let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
            assert_eq!(codes, ["title_line", code, "analysis_commands"], "{case}");
            let warn = diags_of(&net, code);
            assert_eq!(warn[0].0, crate::api::Level::Warning, "{case}");
            assert_eq!(warn[0].1, line, "{case}");
            let top = &net.defs[ROOT.idx()];
            assert_eq!(elem_names(&net, top), ["V1", "r1"], "{case}");
            assert_eq!(params(&net, top, element(&net, top, "v1")), want, "{case}");
        }
        // Golden `pulse_r_noparen`: after PULSE values with no parentheses,
        // `R` is a fifth argument (HSPICE rejects the deck because no
        // parameter `r` exists; the queries report an unknown parameter
        // when they evaluate it).
        let net = load(repo("golden/cases/pulse_r_noparen/deck.sp"));
        let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
        assert_eq!(codes, ["title_line", "analysis_commands"]);
        let top = &net.defs[ROOT.idx()];
        let want = vec![s("PULSE", "0 1 0 1n R")];
        assert_eq!(params(&net, top, element(&net, top, "v1")), want);
        // Forms that were not probed: an odd count in parentheses, on an I
        // line, and `R=` after the last time point give the Warning. No
        // Warning: an even count, `R=` before the last point, an `R` that
        // is not a plain number, and `R` or `TD=` inside the parentheses
        // (not values).
        let deck = write_deck(
            "pwl_points",
            &[(
                "main.sp",
                "* t\nV1 a 0 PWL(0 0 1n)\nI2 a 0 pwl 0 0 1n 1 2n\nV3 a 0 PWL(0 0 1n 1) R=2n\nV4 a 0 PWL(0,0,1n,1)\nV5 a 0 PWL(0 0 1n 1 2n 0) R=1n\nV6 a 0 PWL(0 0 1n 1) R='x'\nV7 a 0 PWL(0 0 1n 1 R TD=1n)\nV8 a 0 PWL(0 0 1n '2 * 3') R\n",
            )],
        );
        let net = load(deck.main.clone());
        let at = |code| -> Vec<String> { diags_of(&net, code).into_iter().map(|d| d.1).collect() };
        assert_eq!(
            at("pwl_odd_values"),
            ["main.sp:2", "main.sp:3"],
            "{:#?}",
            net.diags
        );
        assert_eq!(at("pwl_repeat_dropped"), ["main.sp:4"], "{:#?}", net.diags);
        assert_eq!(net.diags.len(), 4, "{:#?}", net.diags);
    }

    #[test]
    fn pwl_values_words() {
        use super::pwl_values;
        assert_eq!(pwl_values("0 0.3 1n 1"), ["0", "0.3", "1n", "1"]);
        assert_eq!(pwl_values("0,0, 1n ,1"), ["0", "0", "1n", "1"]);
        assert_eq!(
            pwl_values("0 '1 + 2' 1n (3)"),
            ["0", "'1 + 2'", "1n", "(3)"]
        );
        assert_eq!(pwl_values("0 0 1n 1 r td=1n"), ["0", "0", "1n", "1"]);
        assert_eq!(pwl_values("0 0 1n 1 TD=1n"), ["0", "0", "1n", "1"]);
        assert_eq!(pwl_values("0 'a=b' 1n"), ["0", "'a=b'", "1n"]);
        assert_eq!(pwl_values("0 'x"), ["0", "'x"]);
        assert!(pwl_values("").is_empty());
    }

    #[test]
    fn end_text_golden() {
        // Golden `end_quoted_arg`, `end_kv_arg`: `.end 'x'` and `.end x=1`
        // end the first deck, as `.end` does. They gave `trailing_text`
        // before, and the second deck's v2 and r2 joined the first.
        for case in ["end_quoted_arg", "end_kv_arg"] {
            let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
            let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
            assert_eq!(
                codes,
                ["title_line", "analysis_commands", "after_end"],
                "{case}"
            );
            assert_eq!(diags_of(&net, "after_end")[0].1, "deck.sp:5", "{case}");
            let top = &net.defs[ROOT.idx()];
            assert_eq!(elem_names(&net, top), ["v1", "r1"], "{case}");
        }
    }

    #[test]
    fn braces_output_golden() {
        // Golden `brace_print`, `brace_probe`, `brace_probe_quoted`: HSPICE
        // ignores a `.print` or `.probe` output with `{...}`, with a
        // warning, and runs the deck: the Warning `braces_output`, no
        // Error (`.probe v('{a}')` gave the Error `braces` before, and
        // `.probe v({a})` only the Info). Golden `brace_meas`: HSPICE
        // aborts on `{...}` in a `.meas`: the Error `braces` (only the
        // Info before).
        for (case, code, level) in [
            ("brace_print", "braces_output", crate::api::Level::Warning),
            ("brace_probe", "braces_output", crate::api::Level::Warning),
            (
                "brace_probe_quoted",
                "braces_output",
                crate::api::Level::Warning,
            ),
            ("brace_meas", "braces", crate::api::Level::Error),
        ] {
            let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
            let found = diags_of(&net, code);
            assert_eq!(found.len(), 1, "{case}: {:#?}", net.diags);
            assert_eq!(
                (found[0].0, found[0].1.as_str()),
                (level, "deck.sp:6"),
                "{case}"
            );
            let errors = net
                .diags
                .iter()
                .filter(|d| d.level == crate::api::Level::Error)
                .count();
            let want = usize::from(level == crate::api::Level::Error);
            assert_eq!(errors, want, "{case}: {:#?}", net.diags);
            let top = &net.defs[ROOT.idx()];
            assert_eq!(elem_names(&net, top), ["v1", "r1"], "{case}");
        }
        // Every analysis command but `.print` and `.probe` fails on a brace,
        // also where its tokens read (not probed except `.temp`, `.meas`).
        let deck = write_deck(
            "braces_commands",
            &[(
                "main.sp",
                "* t\n.tran 1n {a}\n.dc v1 0 '{a}' 1\n.measure tran m2 avg v({a})\n.print tran v(a) $ {a}\n.probe v({a})\n.print\n+ v('{a}')\nR1 a b 1\n",
            )],
        );
        let net = load(deck.main.clone());
        let at = |code| -> Vec<String> { diags_of(&net, code).into_iter().map(|d| d.1).collect() };
        assert_eq!(
            at("braces"),
            ["main.sp:2", "main.sp:3", "main.sp:4"],
            "{:#?}",
            net.diags
        );
        assert_eq!(
            at("braces_output"),
            ["main.sp:6", "main.sp:7"],
            "{:#?}",
            net.diags
        );
    }

    #[test]
    fn cl_tc_positional_golden() {
        // Golden `cl_tc_positional`: the numbers after a C or L value are
        // TC1, then TC2, with or without a model name; for L the model name
        // comes after the value.
        let net = load(repo("golden/cases/cl_tc_positional/deck.sp"));
        assert!(
            net.diags
                .iter()
                .all(|d| d.level != crate::api::Level::Error),
            "{:#?}",
            net.diags
        );
        let top = &net.defs[ROOT.idx()];
        let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        // the value as `c` or `l`, then `tc1` and `tc2` as written
        let tcs = |key: &str, v: &str, n: usize| {
            let mut p = vec![s(key, v)];
            p.extend([s("tc1", "0.004"), s("tc2", "1e-5")].into_iter().take(n));
            p
        };
        let cases = [
            ("c1", Some("cmod"), tcs("c", "1p", 1)),
            ("c2", None, tcs("c", "1p", 1)),
            ("c3", Some("cmod"), tcs("c", "1p", 2)),
            ("c4", None, tcs("c", "1p", 2)),
            ("c5", Some("cmod"), tcs("c", "1p", 1)),
            ("c6", None, tcs("c", "1p", 2)),
            ("c7", None, tcs("c", "1p", 0)),
            ("l1", Some("lmod"), tcs("l", "1n", 1)),
            ("l2", None, tcs("l", "1n", 1)),
            ("l3", Some("lmod"), tcs("l", "1n", 2)),
            ("l4", None, tcs("l", "1n", 2)),
            ("l5", Some("lmod"), tcs("l", "1n", 1)),
            ("l6", None, tcs("l", "1n", 2)),
            ("l7", None, tcs("l", "1n", 0)),
        ];
        for (name, model, want) in cases {
            let e = element(&net, top, name);
            assert_eq!(
                e.model.map(|m| net.syms.name(m).to_owned()).as_deref(),
                model,
                "{name}"
            );
            assert_eq!(params(&net, top, e), want, "{name}");
        }
        let c4 = top.element_params(element(&net, top, "c4"));
        assert_eq!((c4[1].num, c4[2].num), (Some(0.004), Some(1e-5)));
    }

    #[test]
    fn src_fn_dc_more_golden() {
        // Golden `src_fn_dc_more`, `src_fn_dc_more_noparen`: HSPICE rejects
        // `DC` after a function, with or without parentheses, as in
        // `src_order_fn_first`. V1 is skipped with one `bad_source` Error
        // that says why.
        for case in ["src_fn_dc_more", "src_fn_dc_more_noparen"] {
            let net = load(repo(&format!("golden/cases/{case}/deck.sp")));
            let errs: Vec<_> = net
                .diags
                .iter()
                .filter(|d| d.level == crate::api::Level::Error)
                .collect();
            assert_eq!(errs.len(), 1, "{case}: {:#?}", net.diags);
            assert_eq!(errs[0].code, "bad_source", "{case}");
            assert_eq!(errs[0].loc.map(|l| l.line), Some(3), "{case}");
            assert!(
                errs[0]
                    .message
                    .contains("a transient function comes before `DC`"),
                "{}",
                errs[0].message
            );
            let top = &net.defs[ROOT.idx()];
            assert_eq!(elem_names(&net, top), ["r1"], "{case}");
        }
    }

    #[test]
    fn after_end_second_deck_golden() {
        // Golden `after_end_second_deck`: a complete deck after `.end`
        // runs as a second job in HSPICE. The first job has only the
        // first deck (r1 = 1k), and the queries describe the first job.
        let net = load(repo("golden/cases/after_end_second_deck/deck.sp"));
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["v1", "r1"]);
        let info = diags_of(&net, "after_end");
        assert_eq!(info.len(), 1, "{:#?}", net.diags);
        assert_eq!(info[0].1, "deck.sp:5");
        assert!(
            info[0].2.starts_with("4 lines after `.end` were not read")
                && info[0].2.contains("second job"),
            "{}",
            info[0].2
        );
        assert!(
            net.diags.iter().all(|d| d.level == crate::api::Level::Info),
            "{:#?}",
            net.diags
        );
    }

    #[test]
    fn end_in_include_golden() {
        // Golden `end_in_include`: HSPICE ignores a `.end` in an included
        // file, warns, and reads the lines after it (r2) and after the
        // `.include` (r3). The loader does the same, with a Warning at the
        // `.end` line.
        let net = load(repo("golden/cases/end_in_include/deck.sp"));
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["v1", "r1", "r2", "r3"]);
        let warn = diags_of(&net, "end_in_include");
        assert_eq!(warn.len(), 1, "{:#?}", net.diags);
        assert_eq!(warn[0].0, crate::api::Level::Warning);
        assert_eq!(warn[0].1, "inc.sp:2");
        assert!(warn[0].2.contains("HSPICE ignores"), "{}", warn[0].2);
        let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            ["title_line", "analysis_commands", "end_in_include"],
            "{:#?}",
            net.diags
        );
        // In line order with the other diagnostics of the file, once for a
        // file included twice, and also in a `.subckt` body.
        let deck = write_deck(
            "end_in_include",
            &[
                ("main.sp", "* t\n.inc 'inc.sp'\n.inc 'inc.sp'\n"),
                (
                    "inc.sp",
                    "R1 a b {x}\n.end\n.subckt s{0} a\n.subckt s a\n.end\n.ends\nR2 a b {y}\n",
                ),
            ],
        );
        let net = load(deck.main.clone());
        let got: Vec<(&str, String)> = net
            .diags
            .iter()
            .filter(|d| d.code != "title_line" && d.code != "duplicate_subckt")
            .map(|d| {
                let l = net.loc(d.loc.unwrap());
                let name = Path::new(&l.file).file_name().unwrap().to_string_lossy();
                (d.code, format!("{name}:{}", l.line))
            })
            .collect();
        let at = |c: &'static str, l: &str| (c, l.to_owned());
        assert_eq!(
            got,
            [
                at("braces", "inc.sp:1"),
                at("end_in_include", "inc.sp:2"),
                at("braces", "inc.sp:3"),
                at("end_in_include", "inc.sp:5"),
                at("braces", "inc.sp:7"),
            ],
            "{:#?}",
            net.diags
        );
    }

    #[test]
    fn block_recovery_stops_at_end() {
        // The skip of a failed `.subckt` does not pass the `.end` of the top
        // file: R3 is not loaded, and the lines after `.end` get the
        // `after_end` Info. With no `.ends` before the `.end`, only the
        // failed line is skipped, so R1 is read at the top level (as for a
        // failed block with no `.ends` at all; golden/NOTES.md "Choices not
        // probed").
        let deck = write_deck(
            "block_end",
            &[(
                "main.sp",
                "* t\n.subckt\nR1 a b 1\n.end\nR2 a b 2\n.ends\nR3 a b 3\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["R1"]);
        let info = diags_of(&net, "after_end");
        assert_eq!(info.len(), 1, "{:#?}", net.diags);
        assert_eq!(info[0].1, "main.sp:4");
        assert!(
            info[0].2.starts_with("3 lines after `.end`"),
            "{}",
            info[0].2
        );
    }

    #[test]
    fn include_cycle_canonical_paths() {
        // `main.sp` -> `sub/b.sp` -> `../main.sp` is a cycle: the paths
        // are compared after `..` and links are resolved (each spelling
        // was a new file before, until "File name too long", and the
        // statements were read many times).
        let deck = write_deck(
            "cycle_canonical",
            &[("main.sp", "* t\n.include 'sub/b.sp'\nR1 a b 1\n")],
        );
        std::fs::create_dir_all(deck.dir.join("sub")).unwrap();
        std::fs::write(
            deck.dir.join("sub/b.sp"),
            "R2 a b 2\n.include '../main.sp'\n",
        )
        .unwrap();
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        assert_eq!(elem_names(&net, top), ["R2", "R1"]);
        let cycles = diags_of(&net, "include_cycle");
        assert_eq!(cycles.len(), 1, "{:#?}", net.diags);
        assert_eq!(cycles[0].1, "b.sp:2");
        let codes: Vec<&str> = net.diags.iter().map(|d| d.code).collect();
        assert_eq!(codes, ["title_line", "include_cycle"], "{:#?}", net.diags);
        assert_eq!(net.summary().files, 2);
    }

    #[test]
    fn rcl_keyword_first_spelling() {
        // The name of the R/C/L value keeps the first spelling of its
        // keyword on the line, not the spelling of the winning (last) one.
        let deck = write_deck(
            "kw_first",
            &[(
                "main.sp",
                "* t\nR1 a b R=1k r=2k\nC1 a b C='x' c='2*x'\nL1 a b lmod L = 1n\n+ l=2n\n",
            )],
        );
        let net = load(deck.main.clone());
        let top = &net.defs[ROOT.idx()];
        let p = |name: &str| params(&net, top, element(&net, top, name));
        let s = |a: &str, b: &str| vec![(a.to_owned(), b.to_owned())];
        assert_eq!(p("r1"), s("R", "2k"));
        assert_eq!(p("c1"), s("C", "2*x"));
        assert_eq!(p("l1"), s("L", "2n"));
    }

    #[test]
    fn repeated_include() {
        // Golden `inc_twice`, `lib_twice`: the second read of the same
        // definition is a duplicate. The message names both include lines,
        // not the same file:line twice.
        for (case, file, def_at, first, again) in [
            (
                "inc_twice",
                "sub.inc",
                "sub.inc:2",
                "deck.sp:2",
                "deck.sp:4",
            ),
            ("lib_twice", "m.lib", "m.lib:3", "deck.sp:2", "deck.sp:4"),
        ] {
            let dir = repo(&format!("golden/cases/{case}/"));
            let net = load(dir.join("deck.sp"));
            let dups = diags_of(&net, "duplicate_subckt");
            assert_eq!(dups.len(), 1, "{case}: {:#?}", net.diags);
            let (level, at, message) = &dups[0];
            assert_eq!(*level, crate::api::Level::Error);
            assert_eq!(at, def_at, "{case}");
            let message = message.replace(&dir.display().to_string(), "");
            assert!(
                message.contains(&format!(
                    "\"{file}\" is read twice: through the include line at {first} and again through the include line at {again} ("
                )),
                "{case}: {message}"
            );
            assert_eq!(message.matches(def_at).count(), 0, "{case}: {message}");
        }
        // Golden `inc_twice_params`: the same `.model` card read twice.
        let dir = repo("golden/cases/inc_twice_params/");
        let net = load(dir.join("deck.sp"));
        let dups = diags_of(&net, "duplicate_model");
        assert_eq!(dups.len(), 1, "{:#?}", net.diags);
        assert_eq!(dups[0].1, "params.inc:3");
        let message = dups[0].2.replace(&dir.display().to_string(), "");
        assert!(
            message.contains(
                "duplicated model definition `rmod` (\"params.inc\" is read twice: through the include line at deck.sp:2 and again through the include line at deck.sp:4 ("
            ),
            "{message}"
        );
        // A file read only for its `.param` lines: no error (golden
        // `inc_twice_param_only`), and the second read is the last
        // definition.
        let net = load(repo("golden/cases/inc_twice_param_only/deck.sp"));
        assert!(
            net.diags
                .iter()
                .all(|d| d.level != crate::api::Level::Error),
            "{:#?}",
            net.diags
        );
        // Nested: `a.sp` (included twice) includes `sub.sp`. The message
        // names only the include lines where the two reads differ: the two
        // lines of main.sp, not a.sp:2 twice.
        let deck = write_deck(
            "nested_twice",
            &[
                ("main.sp", "* t\n.inc 'a.sp'\n.inc 'a.sp'\n"),
                ("a.sp", "R1 a b 1\n.inc 'sub.sp'\n"),
                ("sub.sp", ".subckt cell a b\nR1 a b 1\n.ends\n"),
            ],
        );
        let net = load(deck.main.clone());
        let dups = diags_of(&net, "duplicate_subckt");
        assert_eq!(dups.len(), 1, "{:#?}", net.diags);
        assert_eq!(dups[0].1, "sub.sp:1");
        let message = dups[0].2.replace(&format!("{}/", deck.dir.display()), "");
        assert!(
            message.contains(
                "\"sub.sp\" is read twice: through the include line at main.sp:2 and again through the include line at main.sp:3 ("
            ),
            "{message}"
        );
        assert!(!message.contains("a.sp:2"), "{message}");
        // Include chains of different lengths: `a.sp` is read by main.sp:2,
        // and again by b.sp:1, which main.sp:3 reads.
        let deck = write_deck(
            "chain_lengths",
            &[
                ("main.sp", "* t\n.inc 'a.sp'\n.inc 'b.sp'\n"),
                ("a.sp", ".subckt cell a b\nR1 a b 1\n.ends\n"),
                ("b.sp", ".inc 'a.sp'\n"),
            ],
        );
        let net = load(deck.main.clone());
        let dups = diags_of(&net, "duplicate_subckt");
        assert_eq!(dups.len(), 1, "{:#?}", net.diags);
        let message = dups[0].2.replace(&format!("{}/", deck.dir.display()), "");
        assert!(
            message.contains(
                "\"a.sp\" is read twice: through the include line at main.sp:2 and again through the include lines at main.sp:3 and b.sp:1 ("
            ),
            "{message}"
        );
        // Two definitions on different lines keep the old message.
        let deck = write_deck(
            "two_defs",
            &[("main.sp", "* t\n.subckt c a\n.ends\n.subckt c a\n.ends\n")],
        );
        let net = load(deck.main.clone());
        let dups = diags_of(&net, "duplicate_subckt");
        assert_eq!(dups.len(), 1);
        assert!(dups[0].2.contains("(the first is at "), "{}", dups[0].2);
        // Two chains that share an include line in the middle: `x.sp` is
        // read by main.sp:2 and by y.sp:1 (main.sp:3); each read of x.sp
        // reads z.sp at x.sp:1. Only the lines where they differ are named.
        let deck = write_deck(
            "shared_middle",
            &[
                ("main.sp", "* t\n.inc 'x.sp'\n.inc 'y.sp'\n"),
                ("x.sp", ".inc 'z.sp'\n"),
                ("y.sp", ".inc 'x.sp'\n"),
                ("z.sp", ".model m r\n"),
            ],
        );
        let net = load(deck.main.clone());
        let dups = diags_of(&net, "duplicate_model");
        assert_eq!(dups.len(), 1, "{:#?}", net.diags);
        let message = dups[0].2.replace(&format!("{}/", deck.dir.display()), "");
        assert!(
            message.contains(
                "\"z.sp\" is read twice: through the include line at main.sp:2 and again through the include lines at main.sp:3 and y.sp:1 ("
            ),
            "{message}"
        );
    }

    #[test]
    fn duplicate_model() {
        // Two `.model` cards with one name in one scope (any letter case),
        // on different lines. Not probed: HSPICE aborts on the same card
        // read twice (golden `inc_twice_params`, in `repeated_include`), and
        // this gets the same error. Other scopes and `.alter` blocks do not
        // count.
        let deck = write_deck(
            "two_models",
            &[(
                "main.sp",
                "* t\n.model m r\n.model M r\n.subckt s a\n.model m r\n.ends\nR1 a b 1\n.alter\n.model m r\n",
            )],
        );
        let net = load(deck.main.clone());
        let dups = diags_of(&net, "duplicate_model");
        assert_eq!(dups.len(), 1, "{:#?}", net.diags);
        assert_eq!(dups[0].0, crate::api::Level::Error);
        assert_eq!(dups[0].1, "main.sp:3");
        let message = dups[0].2.replace(&format!("{}/", deck.dir.display()), "");
        assert_eq!(
            message,
            "duplicated model definition `m` (the first is at main.sp:2): HSPICE rejects this deck. Queries use the first definition"
        );
    }
}
