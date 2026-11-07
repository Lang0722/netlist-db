//! Builds the netlist of a parse: the walk of its files, `.subckt` bodies and `.if` blocks.
use crate::{
    AST, CondBlock, CondBranch, Files, Parsed, Subckt, ast,
    ast::ASTBuilder,
    builder, err, instance,
    parser::{self, IncludeStep},
    span::{FileId, ParsedId, Pos},
};
use indexmap::IndexMap;
use std::mem;

/// At most this many diagnostics are reported (logged) per file; the rest
/// are counted in one summary line. (They all stay in [`Parsed`].)
pub const MAX_DIAGNOSTICS_PER_FILE: usize = 100;

/// Reports the diagnostics found while a netlist is built
#[derive(Default)]
pub(crate) struct Reporter {
    has_err: bool,
    /// per file: diagnostics reported, errors and warnings not reported
    pub(crate) counts: IndexMap<FileId, (usize, usize, usize)>,
}

impl Reporter {
    pub(crate) fn report(&mut self, e: &err::ParseError, file_id: &FileId, file: &str) {
        let counts = match self.counts.get_mut(file_id) {
            Some(counts) => counts,
            None => self.counts.entry(file_id.clone()).or_default(),
        };
        if counts.0 < MAX_DIAGNOSTICS_PER_FILE {
            counts.0 += 1;
            e.report(&mut self.has_err, file_id, file);
        } else if e.is_warning() {
            counts.2 += 1;
        } else {
            counts.1 += 1;
            self.has_err = true;
        }
    }
    /// Log what was not reported; returns whether there was any error
    pub(crate) fn finish(self) -> bool {
        for (file_id, (_, errors, warnings)) in &self.counts {
            if errors + warnings == 0 {
                continue;
            }
            let msg = format!(
                "{file_id}: {} more diagnostic(s) not shown ({errors} error(s), {warnings} \
                 warning(s)); at most {MAX_DIAGNOSTICS_PER_FILE} are reported per file",
                errors + warnings
            );
            if *errors > 0 {
                crate::error!("{}", msg)
            } else {
                crate::warn!("{}", msg)
            }
        }
        self.has_err
    }
}

/// Where [`ast::ASTBuilder::build`] adds statements: a level, or the current
/// branch of the innermost open `.if` block of that level. A block can open in
/// one file and close in another, so this is passed through the includes. Each
/// `.subckt` body has its own.
struct Target<'a, 's> {
    level: &'a mut AST<'s>,
    /// the open blocks, the outermost first
    open: Vec<CondBlock<'s>>,
    /// the blocks that are nested too deep, and so not kept
    deep: usize,
}

impl<'a, 's> Target<'a, 's> {
    fn new(level: &'a mut AST<'s>) -> Self {
        Self {
            level,
            open: Vec::new(),
            deep: 0,
        }
    }

    /// The AST that takes the statements now
    fn current(&mut self) -> &mut AST<'s> {
        match self.open.last_mut() {
            Some(block) => &mut block.branches.last_mut().expect("a branch").ast,
            None => self.level,
        }
    }

    /// Reads an `.if`, `.elseif`, `.else` or `.endif` line. `depth`: the
    /// number of `.subckt` bodies and `.if` blocks around this level.
    fn line(
        &mut self,
        line: &ast::CondLine,
        pos: Option<Pos>,
        depth: usize,
        reporter: &mut Reporter,
        file: &'s str,
        file_id: &FileId,
    ) {
        use builder::Builder as _;
        let mut report = |e: err::ParseErrorInner| reporter.report(&e.with(pos), file_id, file);
        match line {
            ast::CondLine::If(_)
                if self.deep > 0 || depth + self.open.len() >= parser::MAX_NESTING =>
            {
                if self.deep == 0 {
                    report(err::ParseErrorInner::NestingTooDeep {
                        keyword: "if",
                        subckt: "(the block)".to_owned(),
                        limit: parser::MAX_NESTING,
                    });
                }
                self.deep += 1;
            }
            ast::CondLine::If(cond) => {
                let at = self.current();
                let block = CondBlock {
                    param_at: at.param.len(),
                    func_at: at.param_func.len(),
                    model_at: at.model.len(),
                    instance_at: at.instance.len(),
                    option_at: at.option.len(),
                    unknown_at: at.unknown.len(),
                    branches: vec![CondBranch {
                        cond: Some(cond.build(file)),
                        ast: AST::default(),
                    }],
                    line: instance::Instance {
                        name: "if",
                        ctx: instance::InstanceCtx::Unknown {
                            r#type: b'.',
                            nodes: Vec::new(),
                            params: Vec::new(),
                        },
                    },
                };
                self.open.push(block);
            }
            // (the lines inside a block that is not kept)
            ast::CondLine::EndIf if self.deep > 0 => self.deep -= 1,
            _ if self.deep > 0 => {}
            ast::CondLine::EndIf => match self.open.pop() {
                Some(block) => self.current().cond.push(block),
                None => report(err::ParseErrorInner::StrayCond { keyword: "endif" }),
            },
            ast::CondLine::ElseIf(_) | ast::CondLine::Else => {
                let (keyword, cond) = match line {
                    ast::CondLine::ElseIf(cond) => ("elseif", Some(cond.build(file))),
                    _ => ("else", None),
                };
                match self.open.last_mut() {
                    None => report(err::ParseErrorInner::StrayCond { keyword }),
                    Some(block) if block.branches.last().is_some_and(|b| b.cond.is_none()) => {
                        report(err::ParseErrorInner::AfterElse { keyword })
                    }
                    Some(block) => block.branches.push(CondBranch {
                        cond,
                        ast: AST::default(),
                    }),
                }
            }
        }
    }

    /// The end of the level (or a line that cannot be in a block, at `pos`):
    /// the blocks still open are errors, and their statements stay in the
    /// netlist as the blocks are read so far.
    fn close_all(
        &mut self,
        pos: Option<Pos>,
        reporter: &mut Reporter,
        file: &str,
        file_id: &FileId,
    ) {
        if !self.open.is_empty() || self.deep > 0 {
            let e = err::ParseErrorInner::MissingEndif.with(pos);
            reporter.report(&e, file_id, file);
        }
        self.deep = 0;
        while let Some(block) = self.open.pop() {
            self.current().cond.push(block);
        }
    }
}

/// What [`ast::ASTBuilder::build`] reads from the whole parse, and what it
/// reports to.
struct BuildCtx<'s, 'p> {
    files: &'s Files,
    reporter: Reporter,
    /// The files being built (by `ParsedId`), the top file first, each
    /// with the include line in it that is followed: an include of one of
    /// them is an include cycle.
    reading: Vec<(usize, Option<Pos>)>,
    /// The name of each parsed file
    file_ids: &'p [FileId],
    /// The parse of each file. A walk takes a file's parse out, to follow it, and
    /// puts it back when the file can come again.
    builders: Vec<ASTBuilder>,
    /// How many lines read each file (with one for each time it is a top file)
    reads: Vec<u32>,
    limits: parser::IncludeLimits,
}

impl ast::ASTBuilder {
    /// Adds the statements of this file (or `.subckt` body) to `ast`.
    /// `depth` is the number of `.subckt` bodies around it, counted across
    /// `.include` lines: an included file is inside the body that includes
    /// it (golden `include_in_subckt`). A `.subckt` deeper than
    /// [`parser::MAX_NESTING`] is one `NestingTooDeep` error, and its
    /// definition is skipped. The parser limits the depth in each file, so
    /// only an include chain can go deeper; this keeps the recursion short.
    /// An include of a file that is being built (an include cycle) is one
    /// `CircularDefinition` error at that include line, and is not
    /// followed.
    ///
    /// The child ASTs are built in place in `ast.subckt`, so that a level of
    /// recursion uses little stack.
    ///
    /// `once`: this parse is not read again, so its statements can move out. The
    /// memory of a big netlist then stays near the size of one tree, not two.
    fn build<'s, 'p>(
        &mut self,
        to: &mut Target<'_, 's>,
        cx: &mut BuildCtx<'s, 'p>,
        file_id: &'p FileId,
        parsed_id: ParsedId,
        depth: usize,
        once: bool,
    ) {
        let file: &'s str = &cx.files.inner[parsed_id.0];
        // HSPICE: the netlist of a file ends at its first `.alter` line. What
        // follows describes other runs of the same circuit (other corners or
        // parameters), so it is not added to this netlist. That text is kept
        // verbatim in one `unknown` entry, and the later `.alter` lines are
        // only counted.
        let mut after_alter = false;
        for seg in &mut self.segments {
            if after_alter && !matches!(seg, ast::Segment::Alter { .. }) {
                continue;
            }
            match seg {
                // (the statements of a block that is nested too deep)
                ast::Segment::Local(_) | ast::Segment::Include { .. } if to.deep > 0 => {}
                ast::Segment::Local(local_ast) => {
                    let depth = depth + to.open.len();
                    for s in &mut local_ast.subckt {
                        if depth >= parser::MAX_NESTING {
                            let err = err::ParseErrorInner::NestingTooDeep {
                                keyword: err::def_keyword(file, s.name.start),
                                subckt: file[&s.name].to_owned(),
                                limit: parser::MAX_NESTING,
                            };
                            let pos = Pos {
                                start: s.name.start,
                                line_num: s.name.line_num.get(),
                            };
                            cx.reporter.report(&err.with(Some(pos)), file_id, file);
                            continue;
                        }
                        let child = new_subckt(
                            &mut to.current().subckt,
                            s,
                            file,
                            &mut cx.reporter,
                            file_id,
                        );
                        let mut body = Target::new(child);
                        s.ast
                            .build(&mut body, cx, file_id, parsed_id, depth + 1, once);
                        let end = Some(Pos {
                            start: s.name.start,
                            line_num: s.name.line_num.get(),
                        });
                        body.close_all(end, &mut cx.reporter, file, file_id);
                    }
                    let reporter = &mut cx.reporter;
                    build_statements(local_ast, to.current(), reporter, file, file_id, once);
                }
                ast::Segment::Cond { line, pos } => {
                    to.line(line, *pos, depth, &mut cx.reporter, file, file_id);
                }
                ast::Segment::Include { res, pos } => match res.get() {
                    Some(Ok(id)) => {
                        let size = cx.files.inner[id.0].len();
                        match cx.limits.step(cx.reading.len(), size) {
                            IncludeStep::Follow => {}
                            IncludeStep::SkipQuietly => continue,
                            IncludeStep::Skip(message) => {
                                let e = err::ParseErrorInner::IncludeLimit(message).with(*pos);
                                cx.reporter.report(&e, file_id, file);
                                continue;
                            }
                        }
                        if let Some(last) = cx.reading.last_mut() {
                            last.1 = *pos;
                        }
                        // The parser reads a file once, so it does not see
                        // a cycle: this walk does, and does not follow it.
                        if let Some(k) = cx.reading.iter().position(|&(r, _)| r == id.0) {
                            let chain = cx
                                .reading
                                .iter()
                                .map(|&(r, at)| (cx.file_ids[r].clone(), at))
                                .collect();
                            let e = err::ParseErrorInner::CircularDefinition(chain, k).with(*pos);
                            cx.reporter.report(&e, file_id, file);
                            continue;
                        }
                        cx.reading.push((id.0, None));
                        let file_ids = cx.file_ids;
                        let once = once && cx.reads[id.0] == 1;
                        let mut parse = mem::take(&mut cx.builders[id.0]);
                        parse.build(to, cx, &file_ids[id.0], *id, depth, once);
                        if !once {
                            cx.builders[id.0] = parse;
                        }
                        cx.reading.pop();
                    }
                    Some(Err(e)) => cx.reporter.report(e, file_id, file),
                    None => {
                        let e = err::ParseErrorInner::Internal(
                            "an included file was not parsed (its parser task did not finish)"
                                .to_owned(),
                        )
                        .with(None);
                        cx.reporter.report(&e, file_id, file);
                    }
                },
                ast::Segment::Alter { stmt, pos, .. } => {
                    // (HSPICE stops at an `.alter` line in an `.if` block)
                    to.close_all(*pos, &mut cx.reporter, file, file_id);
                    let ast = to.current();
                    ast.alter_runs += 1;
                    if !after_alter {
                        after_alter = true;
                        ast.unknown
                            .push(builder::Builder::build(stmt.clone(), file));
                    }
                }
            }
        }
    }
}

/// Adds `s` to `subckt` with an empty AST (a later definition with the same
/// name, in any letter case, replaces an earlier one), and returns that AST.
/// It is not inlined, so the `Subckt` value is not in the frame of the
/// recursive `ASTBuilder::build`.
#[inline(never)]
fn new_subckt<'s, 'a>(
    subckt: &'a mut IndexMap<String, Subckt<'s>>,
    s: &ast::SubcktBuilder,
    file: &'s str,
    reporter: &mut Reporter,
    file_id: &FileId,
) -> &'a mut AST<'s> {
    use builder::Builder as _;
    let name = s.name.build(file);
    let key = name.to_lowercase();
    if subckt.contains_key(&key) {
        let e = err::ParseErrorInner::DuplicateSubckt(name.to_string()).with(Some(Pos {
            start: s.name.start,
            line_num: s.name.line_num.get(),
        }));
        reporter.report(&e, file_id, file);
    }
    let (idx, _) = subckt.insert_full(
        key,
        Subckt {
            name,
            ports: s.ports.clone().build(file),
            params: s.params.clone().build(file),
            ast: AST::default(),
        },
    );
    &mut subckt[idx].ast
}

/// Adds the statements of `local_ast`, except its `.subckt` definitions, to
/// `ast`, and reports its errors. It is not inlined, so its values are not
/// in the frame of the recursive `ASTBuilder::build`.
#[inline(never)]
fn build_statements<'s>(
    local_ast: &mut ast::LocalAST,
    ast: &mut AST<'s>,
    reporter: &mut Reporter,
    file: &'s str,
    file_id: &FileId,
    once: bool,
) {
    use builder::Builder as _;
    if ast.title.is_none() {
        ast.title = local_ast.title.build(file);
    }
    add_statements(&mut ast.instance, &mut local_ast.instance, file, once);
    add_statements(&mut ast.model, &mut local_ast.model, file, once);
    add_statements(&mut ast.param, &mut local_ast.param, file, once);
    add_statements(&mut ast.param_func, &mut local_ast.param_func, file, once);
    add_statements(&mut ast.global, &mut local_ast.global, file, once);
    ast.protected.extend(local_ast.protected.iter().copied());
    add_statements(&mut ast.option, &mut local_ast.option, file, once);
    add_statements(&mut ast.general, &mut local_ast.general, file, once);
    add_statements(&mut ast.data, &mut local_ast.data, file, once);
    add_statements(
        &mut ast.init_condition,
        &mut local_ast.init_condition,
        file,
        once,
    );
    add_statements(&mut ast.nodeset, &mut local_ast.nodeset, file, once);
    add_statements(&mut ast.unknown, &mut local_ast.unknown, file, once);
    for e in &local_ast.errors {
        reporter.report(e, file_id, file);
    }
}

/// Adds the statements `from` to `to`. With `once`, each one moves out of `from`:
/// its memory is free when it is built. The first list of a level takes the
/// memory of `from` (a `Vec` that is collected from its own values reuses it).
fn add_statements<'s, B: builder::Builder<'s> + Clone>(
    to: &mut Vec<B::Out>,
    from: &mut Vec<B>,
    file: &'s str,
    once: bool,
) {
    if !once {
        to.extend(from.iter().cloned().map(|b| b.build(file)));
    } else if to.is_empty() {
        *to = mem::take(from).into_iter().map(|b| b.build(file)).collect();
    } else {
        to.extend(mem::take(from).into_iter().map(|b| b.build(file)));
    }
}

/// Adds to `reads` the lines of `parse` that read a file
fn count_reads(parse: &ASTBuilder, reads: &mut [u32]) {
    for seg in &parse.segments {
        match seg {
            ast::Segment::Local(local_ast) => {
                for s in &local_ast.subckt {
                    count_reads(&s.ast, reads);
                }
            }
            ast::Segment::Include { res, .. } => {
                if let Some(Ok(id)) = res.get() {
                    reads[id.0] += 1;
                }
            }
            ast::Segment::Cond { .. } | ast::Segment::Alter { .. } => {}
        }
    }
}

impl Files {
    #[inline]
    pub fn build(&self, parsed: Parsed) -> (AST<'_>, bool) {
        let (file_ids, builders): (Vec<_>, Vec<_>) = parsed.inner.into_iter().unzip();
        let mut reads = vec![0; builders.len()];
        for top_id in &parsed.top_ids {
            reads[top_id.0] += 1;
        }
        for parse in &builders {
            count_reads(parse, &mut reads);
        }
        let mut ast = AST::default();
        let mut cx = BuildCtx {
            files: self,
            file_ids: &file_ids,
            builders,
            reads,
            reporter: Reporter::default(),
            reading: Vec::new(),
            limits: parser::IncludeLimits::new(self.inner.iter().map(String::len).sum()),
        };
        let mut to = Target::new(&mut ast);
        let mut last = None;
        for top_id in parsed.top_ids {
            let once = cx.reads[top_id.0] == 1;
            let mut parse = mem::take(&mut cx.builders[top_id.0]);
            cx.reading.push((top_id.0, None));
            parse.build(&mut to, &mut cx, &file_ids[top_id.0], top_id, 0, once);
            cx.reading.pop();
            last = Some((top_id, parse.after_end));
            if !once {
                cx.builders[top_id.0] = parse;
            }
        }
        if let Some((top_id, after_end)) = last {
            // (the error quotes the `.end` line)
            let line = after_end.map(|end| Pos {
                start: end.start,
                line_num: end.line,
            });
            let (file, file_id) = (&self.inner[top_id.0], &file_ids[top_id.0]);
            to.close_all(line, &mut cx.reporter, file, file_id);
        }
        (ast, cx.reporter.finish())
    }
}
