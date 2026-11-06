//! The lowering of the element lines.
use super::*;

/// What one element line adds to its definition: pins and parameters in
/// the arenas of `def`.
pub(super) struct ElemCx<'a> {
    pub(super) syms: &'a mut Interner,
    pub(super) def: &'a mut Def,
    pub(super) text: &'a str,
    pub(super) file: u32,
    /// The line of the element name.
    pub(super) line: u32,
    pub(super) alter: Option<u32>,
}

impl ElemCx<'_> {
    pub(super) fn sym(&mut self, sp: &Span) -> Sym {
        self.syms.intern(&self.text[sp.start..sp.end])
    }

    pub(super) fn pin(&mut self, sp: &Span) {
        let sym = self.sym(sp);
        self.def.pins.push(sym);
    }

    pub(super) fn kvs(&mut self, kvs: &[KeyValueBuilder]) {
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
    pub(super) fn rcl<'v>(
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
    pub(super) fn positional<'v>(
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
    pub(super) fn mosfet_values(&mut self, m: &MOSFETBuilder) {
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
    pub(super) fn source(&mut self, src: &SourceBuilder) {
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

/// The `name=value` parameter `kv` of file `text`.
pub(super) fn kv_param(
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
pub(super) fn value_param(
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
