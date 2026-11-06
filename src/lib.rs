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
pub mod hierarchy;
pub mod instance;
pub mod models;
pub mod parser;
/// The old, misspelled name of [`hierarchy`]
#[doc(hidden)]
pub use hierarchy as utlis;

pub mod _impl_display;
mod builder;
mod err;
mod netlist_build;
mod span;
#[cfg(test)]
mod tests;

use alloc::borrow::Cow;
pub use err::{ParseError, ParseErrorInner};
use indexmap::IndexMap;
pub use netlist_build::MAX_DIAGNOSTICS_PER_FILE;
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
