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
pub mod instance;
pub mod parser;
pub mod utlis;

pub mod _impl_display;
mod builder;
mod err;
mod span;

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
    /// statements of the `.alter` blocks are in this AST too.
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

/// What [`ast::ASTBuilder::build`] reads from the whole parse, and its
/// error flag.
struct BuildCtx<'s, 'p> {
    files: &'s Files,
    parsed_inner: &'p [(FileId, ASTBuilder)],
    has_err: bool,
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
        ast: &mut AST<'s>,
        cx: &mut BuildCtx<'s, '_>,
        file_id: &FileId,
        parsed_id: ParsedId,
        depth: usize,
    ) {
        let file: &'s str = &cx.files.inner[parsed_id.0];
        for seg in &self.segments {
            match seg {
                ast::Segment::Local(local_ast) => {
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
                            err.with(Some(pos)).report(&mut cx.has_err, file_id, file);
                            continue;
                        }
                        let child = new_subckt(&mut ast.subckt, s, file);
                        s.ast.build(child, cx, file_id, parsed_id, depth + 1);
                    }
                    build_statements(local_ast, ast, &mut cx.has_err, file, file_id);
                }
                ast::Segment::Include { res, pos } => match res.get().unwrap() {
                    Ok(id) => {
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
                            err::ParseErrorInner::CircularDefinition(chain, k)
                                .with(*pos)
                                .report(&mut cx.has_err, file_id, file);
                            continue;
                        }
                        cx.reading.push((id.0, None));
                        let (file_id, _ast) = &cx.parsed_inner[id.0];
                        _ast.build(ast, cx, file_id, *id, depth);
                        cx.reading.pop();
                    }
                    Err(e) => e.report(&mut cx.has_err, file_id, file),
                },
                ast::Segment::Alter { .. } => ast.alter_runs += 1,
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
) -> &'a mut AST<'s> {
    use builder::Builder as _;
    let name = s.name.build(file);
    let (idx, _) = subckt.insert_full(
        name.to_lowercase(),
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
    has_err: &mut bool,
    file: &'s str,
    file_id: &FileId,
) {
    use builder::Builder as _;
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
        e.report(has_err, file_id, file);
    }
}

impl Files {
    #[inline]
    pub fn build(&self, parsed: Parsed) -> (AST<'_>, bool) {
        let mut ast = AST::default();
        let mut cx = BuildCtx {
            files: self,
            parsed_inner: &parsed.inner,
            has_err: false,
            reading: Vec::new(),
        };
        for top_id in parsed.top_ids {
            let (file_id, _ast) = &parsed.inner[top_id.0];
            cx.reading.push((top_id.0, None));
            _ast.build(&mut ast, &mut cx, file_id, top_id, 0);
            cx.reading.pop();
        }
        (ast, cx.has_err)
    }
}
