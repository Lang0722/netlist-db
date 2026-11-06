#[cfg(feature = "py")]
mod py;
#[expect(unused_imports)]
#[cfg(not(feature = "tracing"))]
use log::{debug, error, info, trace, warn};

#[expect(unused_imports)]
#[cfg(feature = "tracing")]
use tracing::{debug, error, info, trace, warn};

fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    #[cfg(feature = "tracing")]
    {
        use tracing::instrument::WithSubscriber;
        tokio::spawn(future.with_current_subscriber())
    }
    #[cfg(not(feature = "tracing"))]
    {
        tokio::spawn(future)
    }
}

extern crate alloc;
pub mod ast;
pub mod eval;
pub mod instance;
pub mod models;
pub mod parser;
pub mod utlis;

pub mod _impl_display;
mod builder;
mod err;
mod span;
#[cfg(test)]
mod tests;

use alloc::borrow::Cow;
use ast::ASTBuilder;
pub use err::{ParseError, ParseErrorInner};
use indexmap::IndexMap;
pub use parser::{LibOverride, ParseOptions, TitleLine};
pub use span::{FileId, ParsedId, Pos, Span};
use std::collections::HashMap;

#[derive(Debug)]
pub struct Parsed {
    pub top_ids: Vec<span::ParsedId>,
    pub id2idx: HashMap<span::FileId, span::ParsedId>,
    pub inner: Vec<(span::FileId, ast::ASTBuilder)>,
    /// One entry for each `ParseOptions::lib_override`, in order: `true`
    /// when it changed the section of at least one `.lib` call.
    pub lib_override_matched: Vec<bool>,
}
#[derive(Debug)]
pub struct Files {
    pub inner: Vec<String>,
}

/// ``` spice
/// .subckt pulvt11ll_ckt d g s b w=1e-6 l=1e-6 sa='sar'
/// ...
/// .ends pulvt11ll_ckt
/// ```
/// Do NOT support `.include` / `.lib` in `.subckt`
#[derive(Debug, Clone)]
pub struct Subckt<'s> {
    pub name: Cow<'s, str>,
    /// subckt/model name is the last arg
    pub ports: Vec<Cow<'s, str>>,
    pub params: Vec<ast::KeyValue<'s>>,
    pub ast: AST<'s>,
}

#[derive(Debug, Clone, Default)]
pub struct AST<'s> {
    /// The title of the netlist: HSPICE reads the first line of the input
    /// as its title. It is the first line of the (first) top-level file,
    /// unless that line is blank, a comment or a dot-command.
    pub title: Option<Cow<'s, str>>,
    pub subckt: IndexMap<String, Subckt<'s>>,
    pub instance: Vec<instance::Instance<'s>>,
    pub model: Vec<ast::Model<'s>>,
    pub param: Vec<ast::KeyValue<'s>>,
    /// `.param` function definitions
    pub param_func: Vec<ast::ParamFunc<'s>>,
    /// `.global` node names
    pub global: Vec<Cow<'s, str>>,
    /// The first and the last line of each `.prot` … `.unprot` region (the
    /// text in it is not parsed)
    pub protected: Vec<(u32, u32)>,
    /// The number of `.alter` lines: the runs after the first one. The
    /// statements of the `.alter` blocks are not in this AST: their text is
    /// kept verbatim in one `unknwon` entry with the command `alter`.
    pub alter_runs: usize,
    pub option: Vec<(Cow<'s, str>, Option<ast::Value<'s>>)>,
    /// transient initial conditions
    /// https://eda-cpu1.eias.junzhuo.site/~junzhuo/hspice/index.htm#page/hspice_14/ic.htm
    ///
    /// `node, val, [subckt]`
    pub init_condition: Vec<(Cow<'s, str>, ast::Value<'s>, Option<Cow<'s, str>>)>,
    /// Initializes specified nodal voltages for DC operating point analysis and corrects convergence problems in DC analysis.
    /// https://eda-cpu1.eias.junzhuo.site/~junzhuo/hspice/index.htm#page/hspice_14/nodeset.htm
    ///
    /// `node, val, [subckt]`
    pub nodeset: Vec<(Cow<'s, str>, ast::Value<'s>, Option<Cow<'s, str>>)>,
    pub general: Vec<ast::General<'s>>,
    pub data: Vec<ast::Data<'s>>,
    pub unknwon: Vec<ast::Unknwon<'s>>,
    /// The `.if` blocks of this level, in netlist order. The statements in
    /// their branches are not in the lists above.
    pub cond: Vec<CondBlock<'s>>,
}

/// An `.if` ... `.endif` block. Which branch is active depends on the instance
/// of the level that holds it (see `models`).
#[derive(Debug, Clone)]
pub struct CondBlock<'s> {
    /// How many items of the lists of the level come before the block: its
    /// place in the netlist order
    pub param_at: usize,
    pub func_at: usize,
    pub model_at: usize,
    pub instance_at: usize,
    pub option_at: usize,
    pub unknwon_at: usize,
    pub branches: Vec<CondBranch<'s>>,
    /// What the row of an error in a condition of this block refers to. A row
    /// names a line of the netlist, and this one has no element.
    pub line: instance::Instance<'s>,
}

/// A branch of an [`CondBlock`]: `.if`, `.elseif` or `.else`
#[derive(Debug, Clone, Default)]
pub struct CondBranch<'s> {
    /// The text in the parentheses; `None` for `.else`
    pub cond: Option<Cow<'s, str>>,
    /// The statements of the branch, as those of a level
    pub ast: AST<'s>,
}

// #[cfg(feature = "py")]
// use polars::{error::PolarsError, frame::DataFrame, prelude::Column};
// pub struct DataValuesCsv<'s, 'a>(pub(crate) &'a ast::DataValues<'s>);
// impl<'s> ast::DataValues<'s> {
//     pub fn csv(&self) -> DataValuesCsv<'s, '_> {
//         DataValuesCsv(self)
//     }
//     #[cfg(feature = "py")]
//     pub fn dataframe(&self) -> Result<DataFrame, PolarsError> {
//         if let Self::InlineNum { params, values } = self {
//             let ncols = params.len();
//             if ncols == 0 {
//                 return Err(PolarsError::ComputeError("Header is empty".into()));
//             }
//             if values.len() % ncols != 0 {
//                 return Err(PolarsError::ComputeError(
//                     "Data length is not a multiple of the number of columns".into(),
//                 ));
//             }
//             let nrows = values.len() / ncols;
//             let columns = params
//                 .into_iter()
//                 .enumerate()
//                 .map(|(col_idx, name)| {
//                     Column::new(
//                         name.as_ref().into(),
//                         (0..nrows)
//                             .into_iter()
//                             .map(|row| values[row * ncols + col_idx])
//                             .collect::<Vec<f64>>(),
//                     )
//                 })
//                 .collect();
//             DataFrame::new(columns)
//         } else {
//             Err(PolarsError::ComputeError("Is not inline data".into()))
//         }
//     }
// }

/// At most this many diagnostics are reported (logged) per file; the rest
/// are counted in one summary line. (They all stay in [`Parsed`].)
pub const MAX_DIAGNOSTICS_PER_FILE: usize = 100;

/// Reports the diagnostics found while a netlist is built
#[derive(Default)]
struct Reporter {
    has_err: bool,
    /// per file: diagnostics reported, errors and warnings not reported
    counts: IndexMap<FileId, (usize, usize, usize)>,
}

impl Reporter {
    fn report(&mut self, e: &err::ParseError, file_id: &FileId, file: &str) {
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
    fn finish(self) -> bool {
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
                    unknwon_at: at.unknwon.len(),
                    branches: vec![CondBranch {
                        cond: Some(cond.build(file)),
                        ast: AST::default(),
                    }],
                    line: instance::Instance {
                        name: Cow::Borrowed("if"),
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
    parsed_inner: &'p [(FileId, ASTBuilder)],
    reporter: Reporter,
    /// The files being built (by `ParsedId`), the top file first, each
    /// with the include line in it that is followed: an include of one of
    /// them is an include cycle.
    reading: Vec<(usize, Option<Pos>)>,
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
    fn build<'s>(
        &self,
        to: &mut Target<'_, 's>,
        cx: &mut BuildCtx<'s, '_>,
        file_id: &FileId,
        parsed_id: ParsedId,
        depth: usize,
    ) {
        let file: &'s str = &cx.files.inner[parsed_id.0];
        // HSPICE: the netlist of a file ends at its first `.alter` line. What
        // follows describes other runs of the same circuit (other corners or
        // parameters), so it is not added to this netlist. That text is kept
        // verbatim in one `unknwon` entry, and the later `.alter` lines are
        // only counted.
        let mut after_alter = false;
        for seg in &self.segments {
            if after_alter && !matches!(seg, ast::Segment::Alter { .. }) {
                continue;
            }
            match seg {
                // (the statements of a block that is nested too deep)
                ast::Segment::Local(_) | ast::Segment::Include { .. } if to.deep > 0 => {}
                ast::Segment::Local(local_ast) => {
                    let depth = depth + to.open.len();
                    for s in &local_ast.subckt {
                        if depth >= parser::MAX_NESTING {
                            let err = err::ParseErrorInner::NestingTooDeep {
                                keyword: err::def_keyword(file, s.name.start),
                                subckt: file[&s.name].to_owned(),
                                limit: parser::MAX_NESTING,
                            };
                            let pos = Pos {
                                start: s.name.start,
                                line_num: s.name.line_num,
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
                        s.ast.build(&mut body, cx, file_id, parsed_id, depth + 1);
                        let end = Some(Pos {
                            start: s.name.start,
                            line_num: s.name.line_num,
                        });
                        body.close_all(end, &mut cx.reporter, file, file_id);
                    }
                    build_statements(local_ast, to.current(), &mut cx.reporter, file, file_id);
                }
                ast::Segment::Cond { line, pos } => {
                    to.line(line, *pos, depth, &mut cx.reporter, file, file_id);
                }
                ast::Segment::Include { res, pos } => match res.get() {
                    Some(Ok(id)) => {
                        if let Some(last) = cx.reading.last_mut() {
                            last.1 = *pos;
                        }
                        // The parser reads a file once, so it does not see
                        // a cycle: this walk does, and does not follow it.
                        if let Some(k) = cx.reading.iter().position(|&(r, _)| r == id.0) {
                            let chain = cx
                                .reading
                                .iter()
                                .map(|&(r, at)| (cx.parsed_inner[r].0.clone(), at))
                                .collect();
                            let e = err::ParseErrorInner::CircularDefinition(chain, k).with(*pos);
                            cx.reporter.report(&e, file_id, file);
                            continue;
                        }
                        cx.reading.push((id.0, None));
                        let (file_id, _ast) = &cx.parsed_inner[id.0];
                        _ast.build(to, cx, file_id, *id, depth);
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
                        ast.unknwon.push(builder::Builder::build(stmt, file));
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
            line_num: s.name.line_num,
        }));
        reporter.report(&e, file_id, file);
    }
    let (idx, _) = subckt.insert_full(
        key,
        Subckt {
            name,
            ports: s.ports.build(file),
            params: s.params.build(file),
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
    local_ast: &ast::LocalAST,
    ast: &mut AST<'s>,
    reporter: &mut Reporter,
    file: &'s str,
    file_id: &FileId,
) {
    use builder::Builder as _;
    if ast.title.is_none() {
        ast.title = local_ast.title.build(file);
    }
    ast.instance
        .extend(local_ast.instance.iter().map(|b| b.build(file)));
    ast.model
        .extend(local_ast.model.iter().map(|b| b.build(file)));
    ast.param
        .extend(local_ast.param.iter().map(|b| b.build(file)));
    ast.param_func
        .extend(local_ast.param_func.iter().map(|b| b.build(file)));
    ast.global
        .extend(local_ast.global.iter().map(|b| b.build(file)));
    ast.protected.extend(local_ast.protected.iter().copied());
    ast.option
        .extend(local_ast.option.iter().map(|b| b.build(file)));
    ast.general
        .extend(local_ast.general.iter().map(|b| b.build(file)));
    ast.data
        .extend(local_ast.data.iter().map(|b| b.build(file)));
    ast.init_condition
        .extend(local_ast.init_condition.iter().map(|b| b.build(file)));
    ast.nodeset
        .extend(local_ast.nodeset.iter().map(|b| b.build(file)));
    ast.unknwon
        .extend(local_ast.unknwon.iter().map(|b| b.build(file)));
    for e in &local_ast.errors {
        reporter.report(e, file_id, file);
    }
}

impl Files {
    #[inline]
    pub fn build(&self, parsed: Parsed) -> (AST<'_>, bool) {
        let mut ast = AST::default();
        let mut cx = BuildCtx {
            files: self,
            parsed_inner: &parsed.inner,
            reporter: Reporter::default(),
            reading: Vec::new(),
        };
        let mut to = Target::new(&mut ast);
        let mut last = None;
        for top_id in parsed.top_ids {
            let (file_id, _ast) = &parsed.inner[top_id.0];
            cx.reading.push((top_id.0, None));
            _ast.build(&mut to, &mut cx, file_id, top_id, 0);
            cx.reading.pop();
            last = Some((top_id, file_id));
        }
        if let Some((top_id, file_id)) = last {
            let line = parsed.inner[top_id.0].1.after_end.map(|end| Pos {
                start: 0,
                line_num: end.line,
            });
            to.close_all(line, &mut cx.reporter, &self.inner[top_id.0], file_id);
        }
        (ast, cx.reporter.finish())
    }
}
