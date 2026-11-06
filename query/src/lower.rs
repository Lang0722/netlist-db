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
        ASTBuilder, CondLine, DataValuesBuilder, KeyValueBuilder, LocalAST, ModelBuilder,
        ModelTypeBuilder, ParamFuncBuilder, Segment, SubcktBuilder, TokenBuilder, UnknwonBuilder,
        ValueBuilder,
    },
    instance::{
        BJT_POSITIONAL, ControlledKind, CurrentBuilder, DIODE_POSITIONAL, InstanceBuilder,
        InstanceCtxBuilder, MOS_POSITIONAL, MOSFETBuilder, RCL_POSITIONAL, SourceBuilder,
        VoltageBuilder, gives_no_value,
    },
    parser::{IncludeLimits, IncludeStep, MAX_NESTING, parse_top_multi_with},
};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, hash_map::Entry},
    iter::once,
    slice,
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
        limits: IncludeLimits::new(texts.iter().map(String::len).sum()),
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
    limits: IncludeLimits,
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
                    Some(Ok(id)) if !self.include_in_limits(id.0, pid, *pos) => {}
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
                Segment::Cond { line, pos } => self.cond_line(line, *pos, pid, report),
            }
        }
    }

    /// The query layer does not evaluate `.if` blocks: it reads all branches.
    /// One note for each block says so.
    fn cond_line(&mut self, line: &CondLine, pos: Option<Pos>, pid: usize, report: bool) {
        if report && matches!(line, CondLine::If(_)) {
            let loc = LocIdx {
                file: pid as u32,
                line: pos.map_or(0, |p| p.line_num),
            };
            let message =
                ".if block: the query layer does not evaluate conditions, so its answers \
                           hold the statements of all branches"
                    .to_owned();
            self.diag(Level::Warning, "if_block", message, Some(loc));
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
                "\"{}\" has bytes that are not UTF-8 (the first that was read is on this line): each one was read as a private character, one for each byte value, and the statements were kept",
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

    /// Whether the walk can follow an include of the file `id` in the file `pid`.
    /// It reports the first limit passed.
    fn include_in_limits(&mut self, id: usize, pid: usize, pos: Option<Pos>) -> bool {
        match self.limits.step(self.reading.len(), self.texts[id].len()) {
            IncludeStep::Follow => true,
            IncludeStep::SkipQuietly => false,
            IncludeStep::Skip(message) => {
                let loc = pos.map(|p| LocIdx {
                    file: pid as u32,
                    line: p.line_num,
                });
                self.diag(Level::Error, "include_limit", message, loc);
                false
            }
        }
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
                cx.mosfet_values(m);
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
        E::IncludeLimit(what) => ("include_limit", what.clone()),
        E::Encrypted { .. } => ("encrypted_block", err.to_string()),
        E::Encoding(what) => ("non_utf8", what.clone()),
        E::LineInQuotes => ("quoted_comment_line", err.to_string()),
        E::StrayCond { .. } | E::AfterElse { .. } => {
            ("if_block_form", format!("{err}. The line was skipped"))
        }
        E::MissingEndif => ("if_block_form", format!("{err}. The block ends here")),
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
            // (`OFF` or a lone `+` on a MOSFET line takes a place and gives no value.
            // So does a parameter between the values: its place holds an empty span.)
            if sp.start == sp.end || gives_no_value(&self.text[sp.start..sp.end]) {
                continue;
            }
            let name = self.syms.builtin(name);
            self.token(name, sp, v);
        }
    }

    /// The values of a MOSFET line, in its order: an empty place stands for the
    /// next `key=value`. HSPICE assigns the plain numbers first and then the
    /// expressions, each in the order of the line, and the last one stays. So a
    /// plain number gives way to an expression of its name, and a later value of
    /// one name replaces an earlier one here.
    fn mosfet_values(&mut self, m: &MOSFETBuilder) {
        let is_expr = |v: &ValueBuilder| matches!(v, ValueBuilder::Expr(_));
        let text = self.text;
        let key_text = |kv: &KeyValueBuilder| &text[kv.k.start..kv.k.end];
        let has_expr = |name: &str| {
            let placed = m.positional_params().any(|(n, sp, v)| {
                n.eq_ignore_ascii_case(name)
                    && is_expr(v)
                    && sp.start != sp.end
                    && !gives_no_value(&text[sp.start..sp.end])
            });
            placed
                || m.params
                    .iter()
                    .any(|kv| is_expr(&kv.v) && key_text(kv).eq_ignore_ascii_case(name))
        };
        let mut keys = m.params.iter();
        for (name, sp, v) in m.positional_params() {
            if sp.start == sp.end {
                if let Some(kv) = keys.next()
                    && (is_expr(&kv.v) || !has_expr(key_text(kv)))
                {
                    self.kvs(slice::from_ref(kv));
                }
            } else if is_expr(v) || !has_expr(name) {
                self.positional(once((name, sp, v)));
            }
        }
        for kv in keys {
            if is_expr(&kv.v) || !has_expr(key_text(kv)) {
                self.kvs(slice::from_ref(kv));
            }
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
mod tests;
