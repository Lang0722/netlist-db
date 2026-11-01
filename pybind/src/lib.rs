//! The Python module `netlist_db.netlist_db`: a loaded netlist and its
//! queries (plan Task 11). Each query returns a JSON string, which
//! `netlist_db/__init__.py` turns into a `dict`. A query error is a JSON
//! object `{"error": {"kind", "message", "suggestions"}}`, never an
//! exception (a panic, which is a bug, is the kind `internal`); only `load`
//! raises (`OSError` when the top file cannot be read, `ValueError` for a
//! bad option, `RuntimeError` for a panic). Every query releases the GIL.

use netlist_query::{
    LibOverride, LoadError, LoadOptions, NetView, Page, QueryError, SubcktSort, TitleLine, TraceDir,
};
use pyo3::exceptions::{PyOSError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use serde::Serialize;
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::str::FromStr;

/// A loaded netlist.
#[pyclass(frozen, name = "Netlist", module = "netlist_db.netlist_db")]
struct Netlist {
    inner: netlist_query::Netlist,
}

/// The JSON of a query error.
fn error_json(e: &QueryError) -> String {
    let (kind, suggestions) = match e {
        QueryError::NotFound { suggestions, .. } => ("not_found", suggestions.clone()),
        QueryError::BadArgument(_) => ("bad_argument", Vec::new()),
    };
    serde_json::json!({
        "error": {"kind": kind, "message": e.to_string(), "suggestions": suggestions}
    })
    .to_string()
}

/// The JSON of an answer or of its error.
fn answer<T: Serialize>(r: Result<T, QueryError>) -> String {
    match r {
        Ok(v) => serde_json::to_string(&v).unwrap_or_else(|e| {
            error_json(&QueryError::BadArgument(format!(
                "the answer cannot be written as JSON: {e}"
            )))
        }),
        Err(e) => error_json(&e),
    }
}

/// The text of a panic payload.
fn panic_text(panic: &(dyn Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("a panic")
}

/// Runs query `f` without the GIL. A panic (a bug) becomes the error
/// `internal`, so that it does not end the process that serves the
/// queries (an MCP server).
fn run(py: Python<'_>, f: impl FnOnce() -> String + Send) -> String {
    py.detach(|| {
        catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|panic| {
            serde_json::json!({"error": {
                "kind": "internal",
                "message": format!("internal error: {}", panic_text(panic.as_ref())),
                "suggestions": [],
            }})
            .to_string()
        })
    })
}

/// A page request; a negative `limit` is a bad argument.
fn page(limit: i64, cursor: Option<String>) -> Result<Page, QueryError> {
    let limit = usize::try_from(limit)
        .map_err(|_| QueryError::BadArgument(format!("limit {limit} is negative")))?;
    Ok(Page { limit, cursor })
}

#[pymethods]
impl Netlist {
    /// An overview: files, definitions, the root, expanded totals,
    /// `parhier`, diagnostic counts, DSPF files.
    fn summary(&self, py: Python<'_>) -> String {
        run(py, || answer(Ok::<_, QueryError>(self.inner.summary())))
    }

    #[pyo3(signature = (level=None, limit=50, cursor=None))]
    fn diagnostics(
        &self,
        py: Python<'_>,
        level: Option<String>,
        limit: i64,
        cursor: Option<String>,
    ) -> String {
        run(py, || {
            answer((|| {
                let level = level
                    .as_deref()
                    .map(netlist_query::Level::from_str)
                    .transpose()?;
                self.inner.diagnostics(level, &page(limit, cursor)?)
            })())
        })
    }

    #[pyo3(signature = (filter=None, sort="name", limit=50, cursor=None))]
    fn list_subckts(
        &self,
        py: Python<'_>,
        filter: Option<String>,
        sort: &str,
        limit: i64,
        cursor: Option<String>,
    ) -> String {
        run(py, || {
            answer((|| {
                let sort = SubcktSort::from_str(sort)?;
                self.inner
                    .list_subckts(filter.as_deref(), sort, &page(limit, cursor)?)
            })())
        })
    }

    #[pyo3(signature = (name, limit=50, cursor=None))]
    fn subckt_info(
        &self,
        py: Python<'_>,
        name: &str,
        limit: i64,
        cursor: Option<String>,
    ) -> String {
        run(py, || {
            answer(page(limit, cursor).and_then(|p| self.inner.subckt_info(name, &p)))
        })
    }

    #[pyo3(signature = (name, limit=50, cursor=None))]
    fn instances_of(
        &self,
        py: Python<'_>,
        name: &str,
        limit: i64,
        cursor: Option<String>,
    ) -> String {
        run(py, || {
            answer(page(limit, cursor).and_then(|p| self.inner.instances_of(name, &p)))
        })
    }

    fn element_info(&self, py: Python<'_>, path: &str) -> String {
        run(py, || answer(self.inner.element_info(path)))
    }

    fn resolve_param(&self, py: Python<'_>, scope_path: &str, name: &str) -> String {
        run(py, || answer(self.inner.resolve_param(scope_path, name)))
    }

    fn resolve_element(&self, py: Python<'_>, path: &str) -> String {
        run(py, || answer(self.inner.resolve_element(path)))
    }

    #[pyo3(signature = (scope_path, net, view="logical", limit=50, cursor=None))]
    fn net_info(
        &self,
        py: Python<'_>,
        scope_path: &str,
        net: &str,
        view: &str,
        limit: i64,
        cursor: Option<String>,
    ) -> String {
        run(py, || {
            answer((|| {
                let view = NetView::from_str(view)?;
                self.inner
                    .net_info(scope_path, net, view, &page(limit, cursor)?)
            })())
        })
    }

    #[pyo3(signature = (scope_path, net, direction="down", depth=3, limit=50, cursor=None))]
    #[allow(clippy::too_many_arguments)]
    fn trace_net(
        &self,
        py: Python<'_>,
        scope_path: &str,
        net: &str,
        direction: &str,
        depth: i64,
        limit: i64,
        cursor: Option<String>,
    ) -> String {
        run(py, || {
            answer((|| {
                let dir = TraceDir::from_str(direction)?;
                if depth < 0 {
                    return Err(QueryError::BadArgument(format!(
                        "depth {depth} is negative"
                    )));
                }
                // a depth past the `u32` range has no bound
                let depth = u32::try_from(depth).unwrap_or(u32::MAX);
                self.inner
                    .trace_net(scope_path, net, dir, depth, &page(limit, cursor)?)
            })())
        })
    }
}

/// Loads the deck `path`. `top`: the definition to use as the root of a
/// bare netlist. `lib_override`: `(file, from_section, to_section)`
/// triples. `title_line`: `auto`, `yes` or `no` (how line 1 is read).
#[pyfunction]
#[pyo3(signature = (path, top=None, lib_override=None, title_line="auto"))]
fn load(
    py: Python<'_>,
    path: PathBuf,
    top: Option<String>,
    lib_override: Option<Vec<(PathBuf, String, String)>>,
    title_line: &str,
) -> PyResult<Netlist> {
    let title_line = match title_line.trim().to_ascii_lowercase().as_str() {
        "auto" => TitleLine::Auto,
        "yes" => TitleLine::Yes,
        "no" => TitleLine::No,
        _ => {
            return Err(PyValueError::new_err(format!(
                "title_line `{title_line}` is not one of `auto`, `yes`, `no`"
            )));
        }
    };
    let opts = LoadOptions {
        top,
        lib_override: lib_override
            .unwrap_or_default()
            .into_iter()
            .map(|(file, from, to)| LibOverride { file, from, to })
            .collect(),
        title_line,
        ..LoadOptions::new(path)
    };
    match py.detach(|| catch_unwind(AssertUnwindSafe(|| netlist_query::Netlist::load(&opts)))) {
        Err(panic) => Err(PyRuntimeError::new_err(format!(
            "internal error while loading: {}",
            panic_text(panic.as_ref())
        ))),
        Ok(r) => match r {
            Ok(inner) => Ok(Netlist { inner }),
            Err(LoadError::Io(msg)) => Err(PyOSError::new_err(msg)),
            Err(e) => Err(PyValueError::new_err(e.to_string())),
        },
    }
}

#[pymodule]
fn netlist_db(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(load, m)?)?;
    m.add_class::<Netlist>()?;
    Ok(())
}
