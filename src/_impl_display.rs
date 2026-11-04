use core::fmt;
use std::{borrow::Borrow as _, fmt::Display};

use super::*;

#[derive(Debug, Clone, Copy)]
pub struct FloatDisplay(pub f64);
impl fmt::Display for FloatDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.7e}", self.0)
    }
}

pub struct TableFloatDisplay(pub f64);
impl fmt::Display for TableFloatDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_sign_positive() {
            write!(f, " {: <13.7e}", self.0)
        } else {
            write!(f, "{: <14.7e}", self.0)
        }
    }
}

pub struct OptionDispaly<'a, T, F: Fn(&T, &mut fmt::Formatter<'_>) -> fmt::Result>(
    pub &'a Option<T>,
    pub F,
);
impl<T, F: Fn(&T, &mut fmt::Formatter<'_>) -> fmt::Result> Display for OptionDispaly<'_, T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(t) = self.0 {
            self.1(t, f)
        } else {
            Ok(())
        }
    }
}

pub fn display_wrap<
    W: fmt::Write,
    T,
    I: Iterator<Item = T>,
    SEP: fmt::Display,
    F: FnMut(T, &mut W) -> fmt::Result,
>(
    f: &mut W,
    iter: I,
    mut fmt_one: F,
    line_sep: SEP,
    item_sep: char,
    wrap_size: usize,
) -> fmt::Result {
    use itertools::Itertools as _;
    for mut ts in iter.into_iter().chunks(wrap_size).into_iter() {
        write!(f, "\n{line_sep}")?;
        if let Some(first) = ts.next() {
            fmt_one(first, f)?;
            for t in ts {
                write!(f, "{item_sep}")?;
                fmt_one(t, f)?;
            }
        }
    }
    Ok(())
}

pub fn display_inline_res<
    E: From<fmt::Error>,
    W: fmt::Write,
    T,
    I: Iterator<Item = Result<T, E>>,
    F: FnMut(T, &mut W) -> fmt::Result,
>(
    f: &mut W,
    iter: I,
    mut fmt_one: F,
    sep: char,
    skip_first_sep: bool,
) -> Result<(), E> {
    let mut iter = iter.into_iter();
    if skip_first_sep {
        if let Some(first) = iter.next() {
            fmt_one(first?, f)?;
        }
    }
    for t in iter {
        write!(f, "{sep}")?;
        fmt_one(t?, f)?;
    }
    Ok(())
}

pub fn display_inline<
    W: fmt::Write,
    T,
    I: Iterator<Item = T>,
    F: FnMut(T, &mut W) -> fmt::Result,
>(
    f: &mut W,
    iter: I,
    mut fmt_one: F,
    sep: char,
    skip_first_sep: bool,
) -> fmt::Result {
    let mut iter = iter.into_iter();
    if skip_first_sep {
        if let Some(first) = iter.next() {
            fmt_one(first, f)?;
        }
    }
    for t in iter {
        write!(f, "{sep}")?;
        fmt_one(t, f)?;
    }
    Ok(())
}

pub fn display_multiline<
    W: fmt::Write,
    T,
    I: Iterator<Item = T>,
    F: FnMut(T, &mut W) -> fmt::Result,
>(
    f: &mut W,
    iter: I,
    mut fmt_one: F,
) -> fmt::Result {
    for t in iter.into_iter() {
        writeln!(f)?;
        fmt_one(t, f)?;
    }
    Ok(())
}

impl fmt::Display for AST<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        DefaultFmt.ast(self, f)
    }
}
impl fmt::Display for Subckt<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        DefaultFmt.subckt(self, f)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DefaultFmt;
impl SpiceFmt for DefaultFmt {}

/// An optimization parameter value, `OPTxxx(init, min, max)`: written without
/// quotes (in quotes, it would be a call to a function named `OPTxxx`)
fn is_opt_call(expr: &str) -> bool {
    let name_len = expr
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(expr.len());
    let (name, rest) = expr.split_at(name_len);
    name.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("opt"))
        && rest.trim_start().starts_with('(')
        && rest.ends_with(')')
}

/// A file name: in single quotes, in double quotes if it has a `'`, or as
/// written if it has both (it was not quoted then)
struct QuotedPath<'a>(&'a str);
impl fmt::Display for QuotedPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.0.contains('\''), self.0.contains('"')) {
            (false, _) => write!(f, "'{}'", self.0),
            (true, false) => write!(f, "\"{}\"", self.0),
            (true, true) => write!(f, "{}", self.0),
        }
    }
}

pub trait SpiceFmt: Sized {
    fn value<W: fmt::Write>(&self, t: &ast::Value<'_>, f: &mut W) -> fmt::Result {
        match t {
            ast::Value::Num(float) => write!(f, "{}", FloatDisplay(*float)),
            // a list of numbers (`TC=0,0`) or an optimization parameter
            ast::Value::Expr(expr) if parser::is_number_list(expr) || is_opt_call(expr) => {
                write!(f, "{expr}")
            }
            // a quote inside (`agauss(0,0,'2*p1')`): not in single quotes
            ast::Value::Expr(expr) if expr.contains('\'') => {
                if parser::is_formula_text(expr) {
                    write!(f, "{expr}")
                } else {
                    write!(f, "\"{expr}\"")
                }
            }
            ast::Value::Expr(expr) => write!(f, "'{expr}'"),
        }
    }
    /// A value naming a file (`PWLFILE=...`): without quotes, so that a
    /// `str(param)` stays a function call
    fn file_value<W: fmt::Write>(&self, t: &ast::Value<'_>, f: &mut W) -> fmt::Result {
        match t {
            ast::Value::Expr(expr) if !expr.contains(char::is_whitespace) => write!(f, "{expr}"),
            t => self.value(t, f),
        }
    }
    fn key_value<W: fmt::Write>(&self, t: &ast::KeyValue<'_>, f: &mut W) -> fmt::Result {
        write!(f, "{}=", t.k)?;
        self.value(&t.v, f)
    }
    fn token<W: fmt::Write>(&self, t: &ast::Token<'_>, f: &mut W) -> fmt::Result {
        match t {
            ast::Token::KV(t) => self.key_value(t, f),
            ast::Token::Value(t) => self.value(t, f),
            ast::Token::V(name) => write!(f, "V({name})"),
            ast::Token::I(name) => write!(f, "I({name})"),
        }
    }
    fn data_files<W: fmt::Write>(&self, t: &ast::DataFiles<'_>, f: &mut W) -> fmt::Result {
        display_multiline(f, t.files.iter(), |file, f| {
            write!(f, "+ FILE={}", QuotedPath(&file.file))?;
            display_inline(
                f,
                file.pname_col_num.iter(),
                |pname_col_num, f| write!(f, "{}={}", pname_col_num.pname, pname_col_num.col_num),
                ' ',
                false,
            )
        })?;
        write!(
            f,
            "{}",
            OptionDispaly(&t.out, |out, f| write!(f, "\n+ OUT={}", QuotedPath(out))),
        )
    }
    fn data<W: fmt::Write>(&self, t: &ast::Data<'_>, f: &mut W) -> fmt::Result {
        write!(f, ".DATA {}", t.name)?;
        match &t.values {
            ast::DataValues::InlineExpr { params, values } => {
                write!(f, "\n+",)?;
                display_inline(f, params.iter(), |t, f| write!(f, "{t}"), ' ', false)?;
                write!(f, " DATAFORM")?;
                display_wrap(
                    f,
                    values.iter(),
                    |t, f| self.value(t, f),
                    "+ ",
                    ' ',
                    params.len(),
                )?;
            }
            ast::DataValues::InlineNum { params, values } => {
                write!(f, "\n+",)?;
                display_inline(f, params.iter(), |t, f| write!(f, "{t}"), ' ', false)?;
                display_wrap(
                    f,
                    values.iter(),
                    |float, f| write!(f, "{}", FloatDisplay(*float)),
                    "+ ",
                    ' ',
                    params.len(),
                )?;
            }
            ast::DataValues::MER(t) => {
                write!(f, " MER")?;
                self.data_files(t, f)?;
            }
            ast::DataValues::LAM(t) => {
                write!(f, " LAM")?;
                self.data_files(t, f)?;
            }
        }
        write!(f, "\n.ENDDATA")
    }
    fn instance<W: fmt::Write>(&self, t: &instance::Instance<'_>, f: &mut W) -> fmt::Result {
        write!(f, "{} ", t.name)?;
        self.instance_ctx(&t.ctx, f)
    }
    fn instance_ctx<W: fmt::Write>(&self, t: &instance::InstanceCtx<'_>, f: &mut W) -> fmt::Result {
        match t {
            instance::InstanceCtx::Resistor(t) => self.resistor(t, f),
            instance::InstanceCtx::Capacitor(t) => self.capacitor(t, f),
            instance::InstanceCtx::Inductor(t) => self.inductor(t, f),
            instance::InstanceCtx::Voltage(t) => self.voltage(t, f),
            instance::InstanceCtx::Current(t) => self.current(t, f),
            instance::InstanceCtx::MOSFET(t) => self.mosfet(t, f),
            instance::InstanceCtx::BJT(t) => self.bjt(t, f),
            instance::InstanceCtx::Diode(t) => self.diode(t, f),
            instance::InstanceCtx::Subckt(t) => self.inst_subckt(t, f),
            instance::InstanceCtx::ControlledSource(t) => self.controlled_source(t, f),
            instance::InstanceCtx::MutualInductor(t) => self.mutual_inductor(t, f),
            instance::InstanceCtx::Unknown {
                r#type: _,
                nodes,
                params,
            } => {
                display_inline(f, nodes.iter(), |t, f| write!(f, "{t}"), ' ', true)?;
                display_inline(f, params.iter(), |t, f| self.key_value(t, f), ' ', false)
            }
        }
    }
    fn inst_subckt<W: fmt::Write>(&self, t: &instance::Subckt<'_>, f: &mut W) -> fmt::Result {
        self.ports(t.nodes.iter().map(|s| s.borrow()), f)?;
        write!(f, " {}", t.cktname)?;
        display_inline(f, t.params.iter(), |t, f| self.key_value(t, f), ' ', false)
    }
    fn voltage<W: fmt::Write>(&self, t: &instance::Voltage<'_>, f: &mut W) -> fmt::Result {
        write!(f, "{} {}", t.n1, t.n2)?;
        if matches!(&t.source, instance::VoltageSource::Params(p) if p.is_empty()) {
            return Ok(());
        }
        write!(f, " ")?;
        self.voltage_source(&t.source, f)
    }
    fn current<W: fmt::Write>(&self, t: &instance::Current<'_>, f: &mut W) -> fmt::Result {
        write!(f, "{} {}", t.n1, t.n2)?;
        if matches!(&t.source, instance::CurrentSource::Params(p) if p.is_empty()) {
            return Ok(());
        }
        write!(f, " ")?;
        self.current_source(&t.source, f)
    }
    fn voltage_source<W: fmt::Write>(
        &self,
        t: &instance::VoltageSource<'_>,
        f: &mut W,
    ) -> fmt::Result {
        match t {
            instance::VoltageSource::Params(params) => {
                display_inline(f, params.iter(), |t, f| self.key_value(t, f), ' ', false)
            }
            instance::VoltageSource::Value(t) => self.value(t, f),
            instance::VoltageSource::PWL(t) => self.pwl(t, f),
            instance::VoltageSource::Spec(t) => self.source_spec(t, f),
        }
    }
    fn current_source<W: fmt::Write>(
        &self,
        t: &instance::CurrentSource<'_>,
        f: &mut W,
    ) -> fmt::Result {
        match t {
            instance::CurrentSource::Params(params) => {
                display_inline(f, params.iter(), |t, f| self.key_value(t, f), ' ', false)
            }
            instance::CurrentSource::Value(t) => self.value(t, f),
            instance::CurrentSource::PWL(t) => self.pwl(t, f),
            instance::CurrentSource::Spec(t) => self.source_spec(t, f),
        }
    }
    fn source_spec<W: fmt::Write>(&self, t: &instance::SourceSpec<'_>, f: &mut W) -> fmt::Result {
        let mut sep = "";
        if let Some(dc) = &t.dc {
            write!(f, "DC ")?;
            self.value(dc, f)?;
            sep = " ";
        }
        if let Some(tran) = &t.tran {
            write!(f, "{sep}")?;
            self.tran_source(tran, f)?;
            sep = " ";
        }
        if let Some(ac) = &t.ac {
            write!(f, "{sep}AC ")?;
            self.value(&ac.mag, f)?;
            if let Some(phase) = &ac.phase {
                write!(f, " ")?;
                self.value(phase, f)?;
            }
            sep = " ";
        }
        for param in &t.params {
            write!(f, "{sep}")?;
            self.key_value(param, f)?;
            sep = " ";
        }
        Ok(())
    }
    fn tran_source<W: fmt::Write>(&self, t: &instance::TranSource<'_>, f: &mut W) -> fmt::Result {
        match t {
            instance::TranSource::PWL(t) => self.pwl(t, f),
            instance::TranSource::Func { name, args, params } => {
                write!(f, "{name}(")?;
                display_inline(f, args.iter(), |t, f| self.value(t, f), ' ', true)?;
                display_inline(
                    f,
                    params.iter(),
                    |t, f| self.key_value(t, f),
                    ' ',
                    args.is_empty(),
                )?;
                write!(f, ")")
            }
        }
    }
    fn time_value_point<W: fmt::Write>(
        &self,
        t: &instance::TimeValuePoint<'_>,
        f: &mut W,
    ) -> fmt::Result {
        self.value(&t.time, f)?;
        write!(f, " ")?;
        self.value(&t.value, f)
    }
    fn pwl<W: fmt::Write>(&self, t: &instance::PWL<'_>, f: &mut W) -> fmt::Result {
        write!(f, "PWL(",)?;
        display_wrap(
            f,
            t.points.iter(),
            |t, f| self.time_value_point(t, f),
            "+ ",
            ' ',
            1,
        )?;
        if let Some(file) = &t.pwlfile {
            write!(f, "\n+ PWLFILE=")?;
            self.file_value(file, f)?;
        }
        for (key, value) in [
            ("R", &t.repeat),
            ("TD", &t.delay),
            ("RSTOP", &t.rstop),
            ("STOPVALUE", &t.stopvalue),
            ("STOPSLOPE", &t.stopslope),
        ] {
            if let Some(value) = value {
                write!(f, "\n+ {key}=")?;
                self.value(value, f)?;
            }
        }
        write!(f, ")",)
    }
    fn resistor<W: fmt::Write>(&self, t: &instance::Resistor<'_>, f: &mut W) -> fmt::Result {
        write!(f, "{} {} ", t.n1, t.n2)?;
        if let Some(mname) = &t.mname {
            write!(f, "{mname} ")?;
        }
        self.value(&t.value, f)?;
        display_inline(f, t.params.iter(), |t, f| self.key_value(t, f), ' ', false)
    }
    fn capacitor<W: fmt::Write>(&self, t: &instance::Capacitor<'_>, f: &mut W) -> fmt::Result {
        write!(f, "{} {} ", t.n1, t.n2)?;
        if let Some(mname) = &t.mname {
            write!(f, "{mname} ")?;
        }
        self.value(&t.value, f)?;
        display_inline(f, t.params.iter(), |t, f| self.key_value(t, f), ' ', false)
    }
    fn inductor<W: fmt::Write>(&self, t: &instance::Inductor<'_>, f: &mut W) -> fmt::Result {
        write!(f, "{} {} ", t.n1, t.n2)?;
        self.value(&t.value, f)?;
        if let Some(mname) = &t.mname {
            write!(f, " {mname}")?;
        }
        display_inline(f, t.params.iter(), |t, f| self.key_value(t, f), ' ', false)
    }
    fn controlled_source<W: fmt::Write>(
        &self,
        t: &instance::ControlledSource<'_>,
        f: &mut W,
    ) -> fmt::Result {
        write!(f, "{} {}", t.n1, t.n2)?;
        if let Some(keyword) = &t.keyword {
            write!(f, " {keyword}")?;
        }
        if let Some(n) = t.poly {
            write!(f, " POLY({n})")?;
        }
        if let Some(pwl) = &t.pwl {
            write!(f, " {}({})", pwl.function, pwl.ndim)?;
        }
        for (p, n) in &t.ctrl_nodes {
            write!(f, " {p} {n}")?;
        }
        for vsrc in &t.ctrl_sources {
            write!(f, " {vsrc}")?;
        }
        display_inline(f, t.coeffs.iter(), |t, f| self.value(t, f), ' ', false)?;
        let Some(pwl) = &t.pwl else {
            return display_inline(f, t.params.iter(), |t, f| self.key_value(t, f), ' ', false);
        };
        // `DELTA=` goes before the points
        let (delta, params): (Vec<_>, Vec<_>) = t
            .params
            .iter()
            .partition(|kv| kv.k.eq_ignore_ascii_case("DELTA"));
        display_inline(
            f,
            delta.into_iter(),
            |t, f| self.key_value(t, f),
            ' ',
            false,
        )?;
        for (x, y) in &pwl.points {
            write!(f, " ")?;
            self.value(x, f)?;
            write!(f, ",")?;
            self.value(y, f)?;
        }
        display_inline(
            f,
            params.into_iter(),
            |t, f| self.key_value(t, f),
            ' ',
            false,
        )
    }
    fn mutual_inductor<W: fmt::Write>(
        &self,
        t: &instance::MutualInductor<'_>,
        f: &mut W,
    ) -> fmt::Result {
        write!(f, "{} {} ", t.l1, t.l2)?;
        self.value(&t.coupling, f)?;
        display_inline(f, t.params.iter(), |t, f| self.key_value(t, f), ' ', false)
    }
    fn mosfet<W: fmt::Write>(&self, t: &instance::MOSFET<'_>, f: &mut W) -> fmt::Result {
        write!(
            f,
            "{} {} {}{} {}",
            t.nd,
            t.ng,
            t.ns,
            OptionDispaly(&t.nb, |nb, f| write!(f, " {nb}")),
            t.mname,
        )?;
        display_inline(f, t.params.iter(), |t, f| self.key_value(t, f), ' ', false)
    }
    fn bjt<W: fmt::Write>(&self, t: &instance::BJT<'_>, f: &mut W) -> fmt::Result {
        write!(
            f,
            "{} {} {}{} {}",
            t.nc,
            t.nb,
            t.ne,
            OptionDispaly(&t.ns, |ns, f| write!(f, " {ns}")),
            t.mname,
        )?;
        display_inline(f, t.params.iter(), |t, f| self.key_value(t, f), ' ', false)
    }
    fn diode<W: fmt::Write>(&self, t: &instance::Diode<'_>, f: &mut W) -> fmt::Result {
        write!(f, "{} {} {}", t.nplus, t.nminus, t.mname,)?;
        display_inline(f, t.params.iter(), |t, f| self.key_value(t, f), ' ', false)
    }
    fn model_type<W: fmt::Write>(&self, t: &ast::ModelType<'_>, f: &mut W) -> fmt::Result {
        match t {
            ast::ModelType::AMP => write!(f, "AMP"),
            ast::ModelType::C => write!(f, "C"),
            ast::ModelType::CORE => write!(f, "CORE"),
            ast::ModelType::D => write!(f, "D"),
            ast::ModelType::L => write!(f, "L"),
            ast::ModelType::NJF => write!(f, "NJF"),
            ast::ModelType::NMOS => write!(f, "NMOS"),
            ast::ModelType::NPN => write!(f, "NPN"),
            ast::ModelType::OPT => write!(f, "OPT"),
            ast::ModelType::PJF => write!(f, "PJF"),
            ast::ModelType::PMOS => write!(f, "PMOS"),
            ast::ModelType::PNP => write!(f, "PNP"),
            ast::ModelType::R => write!(f, "R"),
            ast::ModelType::U => write!(f, "U"),
            ast::ModelType::W => write!(f, "W"),
            ast::ModelType::S => write!(f, "S"),
            ast::ModelType::Unknown(span) => write!(f, "{span}"),
        }
    }
    fn model<W: fmt::Write>(&self, t: &ast::Model<'_>, f: &mut W) -> fmt::Result {
        write!(f, ".MODEL {} ", t.name)?;
        self.model_type(&t.model_type, f)?;
        display_wrap(
            f,
            t.params.iter(),
            |t, f| self.key_value(t, f),
            "+ ",
            ' ',
            4,
        )
    }
    fn ports<'a, W: fmt::Write, I: Iterator<Item = &'a str>>(
        &self,
        t: I,
        f: &mut W,
    ) -> fmt::Result {
        display_inline(f, t, |t, f| write!(f, "{t}"), ' ', true)
    }
    fn subckt<W: fmt::Write>(&self, t: &Subckt<'_>, f: &mut W) -> fmt::Result {
        write!(f, ".SUBCKT {} ", t.name)?;
        self.ports(t.ports.iter().map(|s| s.borrow()), f)?;
        display_inline(f, t.params.iter(), |t, f| self.key_value(t, f), ' ', false)?;
        self.ast(&t.ast, f)?;
        write!(f, "\n.ENDS {}", t.name)
    }
    fn unknwon<W: fmt::Write>(&self, t: &ast::Unknwon<'_>, f: &mut W) -> fmt::Result {
        write!(f, ".{}", t.cmd)?;
        if t.raw.is_empty() {
            // no verbatim text (built by hand): print the tokens
            display_inline(f, t.tokens.iter(), |t, f| self.token(t, f), ' ', false)
        } else if t.raw.starts_with(['\n', '\r']) {
            write!(f, "{}", t.raw)
        } else {
            write!(f, " {}", t.raw)
        }
    }
    fn general_cmd<W: fmt::Write>(&self, t: &ast::GeneralCmd, f: &mut W) -> fmt::Result {
        match t {
            ast::GeneralCmd::Ic => write!(f, "IC"),
            ast::GeneralCmd::Meas => write!(f, "MEAS"),
        }
    }
    fn general<W: fmt::Write>(&self, t: &ast::General<'_>, f: &mut W) -> fmt::Result {
        write!(f, ".")?;
        self.general_cmd(&t.cmd, f)?;
        display_inline(f, t.tokens.iter(), |t, f| self.token(t, f), ' ', false)
    }
    fn ast<W: fmt::Write>(&self, t: &AST<'_>, f: &mut W) -> fmt::Result {
        // the first line is the title (every other item starts on a new line)
        if let Some(title) = &t.title {
            write!(f, "{title}")?;
        }
        if !t.option.is_empty() {
            write!(f, "\n.OPTION ",)?;
            display_wrap(
                f,
                t.option.iter(),
                |option, f| {
                    if let Some(v) = &option.1 {
                        write!(f, "{}=", option.0)?;
                        self.value(v, f)
                    } else {
                        write!(f, "{}", option.0)
                    }
                },
                "+ ",
                ' ',
                4,
            )?;
        }
        if !t.param.is_empty() {
            write!(f, "\n.PARAM ",)?;
            display_wrap(f, t.param.iter(), |t, f| self.key_value(t, f), "+ ", ' ', 4)?;
        }
        display_multiline(f, t.model.iter(), |t, f| self.model(t, f))?;
        display_multiline(f, t.subckt.values(), |t, f| self.subckt(t, f))?;
        display_multiline(f, t.instance.iter(), |t, f| self.instance(t, f))?;
        display_multiline(f, t.init_condition.iter(), |ic, f| {
            write!(f, ".IC V({})=", ic.0)?;
            self.value(&ic.1, f)?;
            if let Some(subckt) = &ic.2 {
                write!(f, " subckt={subckt}")
            } else {
                Ok(())
            }
        })?;
        display_multiline(f, t.nodeset.iter(), |ic, f| {
            write!(f, ".NODESET {}=", ic.0)?;
            self.value(&ic.1, f)?;
            if let Some(subckt) = &ic.2 {
                write!(f, " subckt={subckt}")
            } else {
                Ok(())
            }
        })?;
        display_multiline(f, t.data.iter(), |t, f| self.data(t, f))?;
        display_multiline(f, t.general.iter(), |t, f| self.general(t, f))?;
        display_multiline(f, t.unknwon.iter(), |t, f| self.unknwon(t, f))?;
        Ok(())
    }
}
