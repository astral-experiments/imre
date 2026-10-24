use ruff_python_ast as ast;

use super::{Body, Function};

enum Element<'a> {
    Value(&'a ast::Expr),
    Pair(&'a ast::Expr, &'a ast::Expr),
}

impl Function<'_, '_> {
    pub(super) fn expr(&mut self, expr: &ast::Expr) -> String {
        match expr {
            ast::Expr::NoneLiteral(_) => "0".into(),
            ast::Expr::BooleanLiteral(v) => if v.value { "2" } else { "1" }.into(),
            ast::Expr::NumberLiteral(v) => match &v.value {
                ast::Number::Int(n) => self.integer(n),
                _ => self.unsupported("non-integer numeric literal"),
            },
            ast::Expr::StringLiteral(v) => self.text(&v.value.to_string()),
            ast::Expr::BytesLiteral(v) => self.lit(2, &v.value.bytes().collect::<Vec<_>>()),
            ast::Expr::Name(v) => self.lookup(v.id.as_str()),
            ast::Expr::Attribute(v) => {
                let base = self.expr(&v.value);
                self.attr(&base, v.attr.as_str())
            }
            ast::Expr::Subscript(v) => {
                let object = self.expr(&v.value);
                if let ast::Expr::Slice(s) = &*v.slice {
                    let indices = self.op("list", &[]);
                    for x in [&s.lower, &s.upper, &s.step] {
                        let i = x.as_deref().map_or_else(|| "0".into(), |e| self.expr(e));
                        self.op("append", &[&indices, &i]);
                    }
                    self.op("slice", &[&object, &indices])
                } else {
                    let key = self.expr(&v.slice);
                    self.op("get", &[&object, &key])
                }
            }
            ast::Expr::List(v) => self.display("list", &v.elts),
            ast::Expr::Set(v) => self.display("set", &v.elts),
            ast::Expr::Tuple(v) => {
                let list = self.display("list", &v.elts);
                self.op("tuple", &[&list])
            }
            ast::Expr::Dict(v) => {
                let dict = self.op("dict", &[]);
                for item in &v.items {
                    if let Some(key) = &item.key {
                        let key = self.expr(key);
                        let value = self.expr(&item.value);
                        self.op("put", &[&dict, &key, &value]);
                    } else {
                        let value = self.expr(&item.value);
                        self.op("update", &[&dict, &value]);
                    }
                }
                dict
            }
            ast::Expr::BinOp(v) => {
                let left = self.expr(&v.left);
                let right = self.expr(&v.right);
                self.op(v.op.as_str(), &[&left, &right])
            }
            ast::Expr::UnaryOp(v) => {
                let operand = self.expr(&v.operand);
                self.op(
                    match v.op {
                        ast::UnaryOp::Invert => "~",
                        ast::UnaryOp::Not => "not",
                        ast::UnaryOp::UAdd => "pos",
                        ast::UnaryOp::USub => "neg",
                    },
                    &[&operand],
                )
            }
            ast::Expr::BoolOp(v) => self.boolop(v.op, &v.values),
            ast::Expr::Compare(v) => {
                let left = self.expr(&v.operands[0]);
                self.compare(&left, &v.ops, &v.operands[1..])
            }
            ast::Expr::If(v) => {
                let test = self.expr(&v.test);
                let cond = self.truth(&test);
                self.select(&cond, |f| f.expr(&v.body), |f| f.expr(&v.orelse))
            }
            ast::Expr::Named(v) => {
                let value = self.expr(&v.value);
                self.assign(&v.target, &value);
                value
            }
            ast::Expr::Call(v) => {
                let callable = self.expr(&v.func);
                let (args, kwargs) = self.arguments(&v.arguments);
                self.op("call", &[&callable, &args, &kwargs])
            }
            ast::Expr::Lambda(v) => self.define(
                "<lambda>",
                v.parameters.as_deref(),
                None,
                Body::Expression(&v.body),
            ),
            ast::Expr::FString(v) => {
                let mut result = self.text("");
                for part in v.value.iter() {
                    let part = match part {
                        ast::FStringPartRef::Literal(lit) => self.text(&lit.value),
                        ast::FStringPartRef::FString(s) => self.format_parts(&s.elements),
                    };
                    result = self.op("+", &[&result, &part]);
                }
                result
            }
            ast::Expr::ListComp(v) => {
                self.comprehension("list", &v.generators, Element::Value(&v.elt))
            }
            ast::Expr::SetComp(v) => {
                self.comprehension("set", &v.generators, Element::Value(&v.elt))
            }
            ast::Expr::DictComp(v) => match &v.key {
                Some(key) => {
                    self.comprehension("dict", &v.generators, Element::Pair(key, &v.value))
                }
                None => self.unsupported("dict unpacking comprehension"),
            },
            _ => self.unsupported(&format!("{:?}", ast::AnyNodeRef::from(expr).kind())),
        }
    }

    fn boolop(&mut self, op: ast::BoolOp, values: &[ast::Expr]) -> String {
        self.short_circuit(values.len(), op == ast::BoolOp::Or, |f, i| {
            f.expr(&values[i])
        })
    }

    fn compare(&mut self, left: &str, ops: &[ast::CmpOp], operands: &[ast::Expr]) -> String {
        let mut previous = left.to_owned();
        self.short_circuit(ops.len(), false, |f, i| {
            let right = f.expr(&operands[i]);
            let compared = f.op(ops[i].as_str(), &[&previous, &right]);
            previous = right;
            compared
        })
    }

    /// Merge all exits at one block; a long flat chain need not recurse in the compiler.
    fn short_circuit(
        &mut self,
        count: usize,
        stop_if_true: bool,
        mut value: impl FnMut(&mut Self, usize) -> String,
    ) -> String {
        if count == 1 {
            return value(self, 0);
        }
        let done = self.name("b");
        let mut incoming = Vec::with_capacity(count);
        for i in 0..count {
            let v = value(self, i);
            if i + 1 == count {
                incoming.push(format!("[ {v}, %{} ]", self.block));
                self.jump(&done);
            } else {
                let condition = self.truth(&v);
                incoming.push(format!("[ {v}, %{} ]", self.block));
                let next = self.name("b");
                if stop_if_true {
                    self.branch(&condition, &done, &next);
                } else {
                    self.branch(&condition, &next, &done);
                }
                self.label(&next);
            }
        }
        self.label(&done);
        self.value(&format!("phi i64 {}", incoming.join(", ")))
    }

    fn display(&mut self, kind: &str, elements: &[ast::Expr]) -> String {
        let target = self.op(kind, &[]);
        for element in elements {
            if let ast::Expr::Starred(v) = element {
                let items = self.expr(&v.value);
                self.op("extend", &[&target, &items]);
            } else {
                let value = self.expr(element);
                self.op("append", &[&target, &value]);
            }
        }
        target
    }

    pub(super) fn arguments(&mut self, syntax: &ast::Arguments) -> (String, String) {
        let args = self.op("list", &[]);
        let kwargs = self.op("dict", &[]);
        // Python evaluates starred positional arguments before the keyword arguments.
        for arg in &syntax.args {
            if let ast::Expr::Starred(s) = arg {
                let value = self.expr(&s.value);
                self.op("extend", &[&args, &value]);
            } else {
                let value = self.expr(arg);
                self.op("append", &[&args, &value]);
            }
        }
        for key in &syntax.keywords {
            let value = self.expr(&key.value);
            if let Some(name) = &key.arg {
                let name = self.text(name.as_str());
                self.op("keyword", &[&kwargs, &name, &value]);
            } else {
                self.op("keywords", &[&kwargs, &value]);
            }
        }
        (args, kwargs)
    }

    fn format_parts(&mut self, elements: &[ast::InterpolatedStringElement]) -> String {
        let mut out = self.text("");
        for element in elements {
            let value = match element {
                ast::InterpolatedStringElement::Literal(lit) => self.text(&lit.value),
                ast::InterpolatedStringElement::Interpolation(field) => {
                    if let Some(debug) = &field.debug_text {
                        let prefix = self.text(debug.as_str());
                        out = self.op("+", &[&out, &prefix]);
                    }
                    let v = self.expr(&field.expression);
                    let conversion = field
                        .conversion
                        .to_char()
                        .map_or_else(|| "0".into(), |c| self.text(&c.to_string()));
                    let spec = match &field.format_spec {
                        Some(s) => self.format_parts(&s.elements),
                        None => self.text(""),
                    };
                    self.op("format", &[&v, &conversion, &spec])
                }
            };
            out = self.op("+", &[&out, &value]);
        }
        out
    }

    fn comprehension(
        &mut self,
        kind: &str,
        generators: &[ast::Comprehension],
        element: Element<'_>,
    ) -> String {
        if generators.iter().any(|g| g.is_async) {
            return self.unsupported("async comprehension");
        }
        let output = self.op(kind, &[]);
        let first = self.expr(&generators[0].iter);
        let scope = self.op("scope", &[&self.scope.clone()]);
        let previous = std::mem::replace(&mut self.scope, scope);
        self.comprehension_loop(generators, &first, &output, &element);
        self.scope = previous;
        output
    }

    fn comprehension_loop(
        &mut self,
        generators: &[ast::Comprehension],
        source: &str,
        out: &str,
        element: &Element<'_>,
    ) {
        let g = &generators[0];
        let iter = self.op("iter", &[source]);
        let step = self.name("b");
        let body = self.name("b");
        let done = self.name("b");
        self.jump(&step);
        self.label(&step);
        let more = self.op("more", &[&iter]);
        let cond = self.value(&format!("icmp eq i64 {more}, 2"));
        self.branch(&cond, &body, &done);
        self.label(&body);
        let item = self.op("next", &[&iter]);
        self.assign(&g.target, &item);
        for test in &g.ifs {
            let value = self.expr(test);
            let yes = self.truth(&value);
            let keep = self.name("b");
            self.branch(&yes, &keep, &step);
            self.label(&keep);
        }
        if let Some(next) = generators.get(1) {
            let source = self.expr(&next.iter);
            self.comprehension_loop(&generators[1..], &source, out, element);
        } else {
            match element {
                Element::Value(e) => {
                    let v = self.expr(e);
                    self.op("append", &[out, &v]);
                }
                Element::Pair(k, v) => {
                    let k = self.expr(k);
                    let v = self.expr(v);
                    self.op("put", &[out, &k, &v]);
                }
            }
        }
        self.jump(&step);
        self.label(&done);
    }
}
