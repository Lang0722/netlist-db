//! HSPICE numbers and expressions.
//!
//! - [`scan_number`] reads one HSPICE number literal with its scale factor
//!   or unit (`1m` = milli, `1me` and `1meg` = mega, `1mi` and `1mil` = mil,
//!   `1min` = 60 s, `20db` = 10, letters after the scale are ignored, and no
//!   scale applies after an exponent). [`exact_number`] reads a token that
//!   is one number with at most one scale factor or unit, and nothing else,
//!   and [`scan_number_exact`] tells whether a literal is so.
//! - [`parse`] turns expression text into an [`Expr`] tree (a Pratt parser
//!   with the HSPICE operator precedence: `**` and `^` are left-associative
//!   and bind tighter than a unary minus before them).
//! - [`eval`] computes the value of an [`Expr`]. Parameter names and user
//!   `.param` functions come from an [`Env`] that the caller provides. A
//!   user function hides a built-in function of the same name
//!   ([`is_builtin`]), as in HSPICE.
//!
//! The crate has no dependencies and knows nothing about netlists or scopes.

mod builtins;
mod eval;
mod number;
mod parse;

pub use builtins::is_builtin;
pub use eval::{Caveat, Env, EvalError, eval, near};
pub use number::{exact_number, scan_number, scan_number_exact};
pub use parse::{BinOp, Expr, ExprError, UnOp, free_names, parse};

/// Test helpers shared by the module tests.
#[cfg(test)]
pub(crate) mod test_env {
    use crate::{Caveat, Env, EvalError};

    /// An [`Env`] with fixed variables and one user function `f(x) = 10*x`.
    /// It records every call so tests can check what [`crate::eval`] asked.
    pub struct MapEnv {
        pub vars: Vec<(&'static str, f64)>,
        pub calls: Vec<String>,
    }

    impl MapEnv {
        pub fn new(vars: &[(&'static str, f64)]) -> Self {
            Self {
                vars: vars.to_vec(),
                calls: Vec::new(),
            }
        }
    }

    impl Env for MapEnv {
        fn var(&mut self, name: &str) -> Result<(f64, Vec<Caveat>), EvalError> {
            self.calls.push(format!("var:{name}"));
            match self.vars.iter().find(|(n, _)| *n == name) {
                Some(&(_, v)) => Ok((v, Vec::new())),
                None => Err(EvalError::Undefined(name.to_string())),
            }
        }

        fn user_fn(
            &mut self,
            name: &str,
            args: &[f64],
        ) -> Option<Result<(f64, Vec<Caveat>), EvalError>> {
            self.calls.push(format!("fn:{name}{args:?}"));
            match (name, args) {
                ("f", [x]) => Some(Ok((10.0 * x, Vec::new()))),
                _ => None,
            }
        }
    }

    /// Parse `src` and evaluate it with `vars`.
    pub fn eval_str(
        src: &str,
        vars: &[(&'static str, f64)],
    ) -> Result<(f64, Vec<Caveat>), EvalError> {
        let e = crate::parse(src).unwrap_or_else(|err| panic!("parse {src:?}: {err}"));
        crate::eval(&e, &mut MapEnv::new(vars))
    }

    /// Parse `src`, evaluate it without variables, and return the value.
    pub fn val(src: &str) -> f64 {
        match eval_str(src, &[]) {
            Ok((v, _)) => v,
            Err(err) => panic!("eval {src:?}: {err}"),
        }
    }

    /// Relative comparison for values that pass through decimal scaling.
    pub fn close(a: f64, b: f64) -> bool {
        a == b || (a - b).abs() <= 1e-12 * a.abs().max(b.abs())
    }
}
