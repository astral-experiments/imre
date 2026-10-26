use ruff_python_ast as ast;
use ruff_python_ast::visitor::{self, Visitor};
use serde_json::json;

use super::{Body, Function, LoopTarget};

enum Place {
    Name(String),
    Attr(String, String),
    Item(String, String),
    Slice(String, String),
}

impl Function<'_, '_> {
    /// A global declaration applies to the entire code block, even in an untaken branch.
    pub(super) fn declare_globals(&mut self, suite: &[ast::Stmt]) {
        #[derive(Default)]
        struct Globals<'a>(std::collections::BTreeSet<&'a str>);

        impl<'a> Visitor<'a> for Globals<'a> {
            fn visit_stmt(&mut self, statement: &'a ast::Stmt) {
                match statement {
                    ast::Stmt::Global(global) => {
                        self.0.extend(global.names.iter().map(|name| name.as_str()));
                    }
                    ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_) => {}
                    _ => visitor::walk_stmt(self, statement),
                }
            }

            fn visit_expr(&mut self, _: &'a ast::Expr) {}
        }

        let mut globals = Globals::default();
        globals.visit_body(suite);
        for name in globals.0 {
            let name = self.text(name);
            self.op("declare_global", &[&self.scope.clone(), &name]);
        }
    }

    pub(super) fn statements(&mut self, suite: &[ast::Stmt]) {
        for stmt in suite {
            if self.terminated {
                break;
            }
            self.checkpoint(stmt);
            match stmt {
                ast::Stmt::Expr(v) => {
                    if !matches!(&*v.value, ast::Expr::StringLiteral(_)) {
                        self.expr(&v.value);
                    }
                }
                ast::Stmt::Pass(_) | ast::Stmt::Global(_) => {}
                ast::Stmt::Delete(v) => {
                    for target in &v.targets {
                        if let ast::Expr::Name(name) = target {
                            let name = self.text(name.id.as_str());
                            self.op("delname", &[&self.scope.clone(), &name]);
                        } else {
                            self.unsupported("deleting an attribute or item");
                        }
                    }
                }
                ast::Stmt::Assign(v) => {
                    let value = self.expr(&v.value);
                    for t in &v.targets {
                        self.assign(t, &value);
                    }
                }
                ast::Stmt::AnnAssign(v) => {
                    if let Some(value) = &v.value {
                        let value = self.expr(value);
                        self.assign(&v.target, &value);
                    }
                    if let ast::Expr::Name(name) = &*v.target {
                        let key = self.text(name.id.as_str());
                        self.op("annotate", &[&self.scope.clone(), &key]);
                    }
                }
                ast::Stmt::AugAssign(v) => {
                    if let Some(target) = self.place(&v.target) {
                        let left = self.read(&target);
                        let right = self.expr(&v.value);
                        let value = self.op(v.op.as_str(), &[&left, &right]);
                        self.write(&target, &value);
                    }
                }
                ast::Stmt::FunctionDef(v) => {
                    let decorators = v
                        .decorator_list
                        .iter()
                        .map(|d| self.expr(&d.expression))
                        .collect::<Vec<_>>();
                    let body = if v.is_async {
                        Body::Unsupported(stmt, "async function")
                    } else if is_generator(&v.body) {
                        Body::Unsupported(stmt, "generator function")
                    } else {
                        Body::Statements(&v.body)
                    };
                    let function = self.define(
                        v.name.as_str(),
                        Some(&v.parameters),
                        v.returns.as_deref(),
                        body,
                    );
                    self.decorated(&v.name, &decorators, function);
                }
                ast::Stmt::ClassDef(v) => {
                    let decorators = v
                        .decorator_list
                        .iter()
                        .map(|d| self.expr(&d.expression))
                        .collect::<Vec<_>>();
                    let bases = if let Some(args) = v.arguments.as_deref() {
                        if !args.keywords.is_empty() {
                            self.unsupported("class keyword arguments");
                        }
                        self.arguments(args).0
                    } else {
                        self.op("list", &[])
                    };
                    let init = self.define(v.name.as_str(), None, None, Body::Namespace(&v.body));
                    let name = self.text(v.name.as_str());
                    let class = self.op("class", &[&name, &bases, &init]);
                    self.decorated(&v.name, &decorators, class);
                }
                ast::Stmt::Return(v) => {
                    let value = v
                        .value
                        .as_deref()
                        .map_or_else(|| "0".into(), |e| self.expr(e));
                    self.close_contexts(0);
                    self.end(&format!("ret i64 {value}"));
                }
                ast::Stmt::If(v) => {
                    let done = self.name("b");
                    self.if_arm(&v.test, &v.body, &done);
                    for clause in &v.elif_else_clauses {
                        if let Some(test) = &clause.test {
                            self.if_arm(test, &clause.body, &done);
                        } else {
                            self.statements(&clause.body);
                        }
                    }
                    self.jump(&done);
                    self.label(&done);
                }
                ast::Stmt::For(v) => {
                    let source = self.expr(&v.iter);
                    let iter = self.op("iter", &[&source]);
                    self.live.push(iter.clone());
                    self.loop_body(
                        &v.body,
                        &v.orelse,
                        |f| {
                            let more = f.op("more", &[&iter]);
                            f.value(&format!("icmp eq i64 {more}, 2"))
                        },
                        |f| {
                            let item = f.op("next", &[&iter]);
                            f.assign(&v.target, &item);
                        },
                    );
                    self.live.pop();
                }
                ast::Stmt::While(v) => {
                    self.loop_body(
                        &v.body,
                        &v.orelse,
                        |f| {
                            let test = f.expr(&v.test);
                            f.truth(&test)
                        },
                        |_| {},
                    );
                }
                ast::Stmt::Break(_) | ast::Stmt::Continue(_) => {
                    if let Some(target) = self.loops.last() {
                        let dest = if matches!(stmt, ast::Stmt::Break(_)) {
                            &target.done
                        } else {
                            &target.step
                        }
                        .clone();
                        self.close_contexts(target.with_depth);
                        self.jump(&dest);
                    } else {
                        self.unsupported("loop control outside a loop");
                    }
                }
                ast::Stmt::Import(v) => {
                    for alias in &v.names {
                        let full = alias.name.as_str();
                        let module = self.import(full);
                        if let Some(name) = &alias.asname {
                            self.bind(name.as_str(), &module);
                        } else if let Some((root, _)) = full.split_once('.') {
                            let outer = self.import(root);
                            self.bind(root, &outer);
                        } else {
                            self.bind(full, &module);
                        }
                    }
                }
                ast::Stmt::ImportFrom(v) => {
                    let name = v.module.as_ref().map_or("", |v| v.as_str());
                    if name == "__future__" && v.level == 0 {
                        continue;
                    }
                    let Some(name) = relative_import(self.source.package, v.level, name) else {
                        self.unsupported("relative import beyond the top-level package");
                        continue;
                    };
                    let module = self.import(&name);
                    for alias in &v.names {
                        if alias.name.as_str() == "*" {
                            self.unsupported("star import");
                            continue;
                        }
                        let attr = self.text(alias.name.as_str());
                        let value = self.op("from", &[&module, &attr]);
                        self.bind(
                            alias.asname.as_ref().unwrap_or(&alias.name).as_str(),
                            &value,
                        );
                    }
                }
                ast::Stmt::Try(v) if !v.is_star && v.finalbody.is_empty() => self.try_except(v),
                ast::Stmt::With(v) if !v.is_async => self.with(&v.items, &v.body),
                ast::Stmt::Raise(v) => {
                    let exc = if let Some(e) = &v.exc {
                        self.expr(e)
                    } else {
                        self.caught.last().cloned().unwrap_or_else(|| "0".into())
                    };
                    let cause = v
                        .cause
                        .as_deref()
                        .map_or_else(|| "0".into(), |e| self.expr(e));
                    self.raise(&exc, &cause);
                }
                ast::Stmt::Assert(v) => {
                    let test = self.expr(&v.test);
                    let condition = self.truth(&test);
                    let pass = self.name("b");
                    let fail = self.name("b");
                    self.branch(&condition, &pass, &fail);
                    self.label(&fail);
                    let cls = self.lookup("AssertionError");
                    let error = if let Some(msg) = &v.msg {
                        let msg = self.expr(msg);
                        self.invoke(&cls, &[&msg])
                    } else {
                        self.invoke(&cls, &[])
                    };
                    self.raise(&error, "0");
                    self.label(&pass);
                }
                _ => {
                    self.unsupported(&format!("{:?}", ast::AnyNodeRef::from(stmt).kind()));
                }
            }
        }
    }

    fn import(&mut self, name: &str) -> String {
        let name = self.text(name);
        self.op("import", &[&self.scope.clone(), &name])
    }

    pub(super) fn define(
        &mut self,
        name: &str,
        params: Option<&ast::Parameters>,
        returns: Option<&ast::Expr>,
        body: Body<'_>,
    ) -> String {
        let qualname = format!("{}{name}", self.prefix);
        let prefix = if matches!(body, Body::Namespace(_)) {
            format!("{qualname}.")
        } else {
            format!("{qualname}.<locals>.")
        };
        let doc = match &body {
            Body::Statements(stmts) if self.output.keep_docstrings => {
                crate::frontend::docstring(stmts)
            }
            Body::Unsupported(ast::Stmt::FunctionDef(f), _) if self.output.keep_docstrings => {
                crate::frontend::docstring(&f.body)
            }
            _ => None,
        };
        let doc = doc.map_or_else(|| "0".into(), |s| self.text(&s));
        let entries = params.map_or_else(Vec::new, |p| {
            crate::frontend::parameter_annotations(p, returns)
        });
        let annotate = if entries.is_empty() {
            "0".into()
        } else {
            let code = self
                .output
                .code(self.source, String::new(), Body::Annotations(entries));
            let aname = self.text("__annotate__");
            let aqname = self.text(&format!("{qualname}.__annotate__"));
            let sig = self.text(r#"{"posonly":["format"]}"#);
            let defaults = self.op("dict", &[]);
            self.checked(&format!("call i64 @imre_dyn_def_with_metadata(ptr %ctx, ptr {code}, i64 {}, i64 {aname}, i64 {sig}, i64 {defaults}, i64 {aqname}, i64 0, i64 0, i1 true)", self.scope))
        };
        let qualname = self.text(&qualname);
        let mut signature =
            json!({"posonly":[],"positional":[],"kwonly":[],"varargs":null,"kwargs":null});
        if params.is_some() && uses_class_cell(&body) {
            signature["class_cell"] = json!(true);
        }
        let code = self.output.code(self.source, prefix, body);
        let defaults = self.op("dict", &[]);
        signature["types"] = json!({});
        signature["return_type"] = json!(returns.and_then(scalar_annotation));
        if let Some(p) = params {
            for (group, args) in [
                ("posonly", &p.posonlyargs),
                ("positional", &p.args),
                ("kwonly", &p.kwonlyargs),
            ] {
                for arg in args {
                    let name = arg.parameter.name.as_str();
                    if let Some(ty) = arg
                        .parameter
                        .annotation
                        .as_deref()
                        .and_then(scalar_annotation)
                    {
                        signature["types"][name] = json!(ty);
                    }
                    signature[group].as_array_mut().unwrap().push(json!(name));
                    if let Some(default) = &arg.default {
                        let default = self.expr(default);
                        let name = self.text(name);
                        self.op("put", &[&defaults, &name, &default]);
                    }
                }
            }
            signature["varargs"] = json!(p.vararg.as_deref().map(|x| x.name.as_str()));
            signature["kwargs"] = json!(p.kwarg.as_deref().map(|x| x.name.as_str()));
        }
        let sig = self.text(&signature.to_string());
        let name = self.text(name);
        self.checked(&format!("call i64 @imre_dyn_def_with_metadata(ptr %ctx, ptr {code}, i64 {}, i64 {name}, i64 {sig}, i64 {defaults}, i64 {qualname}, i64 {doc}, i64 {annotate}, i1 false)", self.scope))
    }

    pub(super) fn annotations(&mut self, entries: &[(&str, &ast::Expr)]) {
        // CPython's compiled annotation callable accepts VALUE and VALUE_WITH_FAKE_GLOBALS.
        let format = self.lookup("format");
        let two = self.integer(2);
        let unsupported = self.op(">", &[&format, &two]);
        let bad = self.name("b");
        let good = self.name("b");
        let condition = self.truth(&unsupported);
        self.branch(&condition, &bad, &good);
        self.label(&bad);
        let error = self.lookup("NotImplementedError");
        self.raise(&error, "0");
        self.label(&good);
        let values = self.op("dict", &[]);
        for &(name, expression) in entries {
            let key = self.text(name);
            let value = if self.source.string_annotations {
                self.text(&crate::frontend::annotation_spelling(expression))
            } else {
                self.expr(expression)
            };
            self.op("put", &[&values, &key, &value]);
        }
        self.end(&format!("ret i64 {values}"));
    }

    fn decorated(&mut self, name: &ast::Identifier, decorators: &[String], mut value: String) {
        for decorator in decorators.iter().rev() {
            value = self.invoke(decorator, &[&value]);
        }
        self.bind(name.as_str(), &value);
    }

    fn place(&mut self, target: &ast::Expr) -> Option<Place> {
        Some(match target {
            ast::Expr::Name(n) => Place::Name(self.text(n.id.as_str())),
            ast::Expr::Attribute(a) => {
                let obj = self.expr(&a.value);
                Place::Attr(obj, self.text(a.attr.as_str()))
            }
            ast::Expr::Subscript(s) => {
                let obj = self.expr(&s.value);
                if let ast::Expr::Slice(slice) = &*s.slice {
                    Place::Slice(obj, self.slice_bounds(slice))
                } else {
                    Place::Item(obj, self.expr(&s.slice))
                }
            }
            _ => {
                self.unsupported("assignment target");
                return None;
            }
        })
    }

    fn read(&mut self, place: &Place) -> String {
        match place {
            Place::Name(n) => self.op("lookup", &[&self.scope.clone(), n]),
            Place::Attr(v, n) => self.op("attr", &[v, n]),
            Place::Item(v, k) => self.op("get", &[v, k]),
            Place::Slice(v, bounds) => self.op("slice", &[v, bounds]),
        }
    }

    fn write(&mut self, place: &Place, value: &str) {
        match place {
            Place::Name(n) => self.op("assign", &[&self.scope.clone(), n, value]),
            Place::Attr(v, n) => self.op("setattr", &[v, n, value]),
            Place::Item(v, k) => self.op("put", &[v, k, value]),
            Place::Slice(v, bounds) => self.op("setslice", &[v, bounds, value]),
        };
    }

    pub(super) fn assign(&mut self, target: &ast::Expr, value: &str) {
        let elements = match target {
            ast::Expr::Tuple(t) => &t.elts,
            ast::Expr::List(l) => &l.elts,
            _ => {
                if let Some(p) = self.place(target) {
                    self.write(&p, value);
                }
                return;
            }
        };
        let count = self.integer(elements.len());
        let star = elements
            .iter()
            .position(|v| matches!(v, ast::Expr::Starred(_)))
            .map_or_else(|| "0".into(), |i| self.integer(i));
        let sequence = self.op("unpack", &[value, &count, &star]);
        for (index, target) in elements.iter().enumerate() {
            let i = self.integer(index);
            let item = self.op("get", &[&sequence, &i]);
            let target = if let ast::Expr::Starred(s) = target {
                &s.value
            } else {
                target
            };
            self.assign(target, &item);
        }
    }

    fn if_arm(&mut self, test: &ast::Expr, body: &[ast::Stmt], done: &str) {
        let body_label = self.name("b");
        let next = self.name("b");
        let test = self.expr(test);
        let condition = self.truth(&test);
        self.branch(&condition, &body_label, &next);
        self.label(&body_label);
        self.statements(body);
        self.jump(done);
        self.label(&next);
    }

    fn loop_body(
        &mut self,
        body: &[ast::Stmt],
        otherwise: &[ast::Stmt],
        condition: impl FnOnce(&mut Self) -> String,
        bind: impl FnOnce(&mut Self),
    ) {
        let step = self.name("b");
        let body_label = self.name("b");
        let exhausted = self.name("b");
        let done = self.name("b");
        self.jump(&step);
        self.label(&step);
        let condition = condition(self);
        self.branch(&condition, &body_label, &exhausted);
        self.label(&body_label);
        bind(self);
        self.loops.push(LoopTarget {
            step: step.clone(),
            done: done.clone(),
            with_depth: self.exits.len(),
        });
        self.statements(body);
        self.loops.pop();
        self.jump(&step);
        self.label(&exhausted);
        self.statements(otherwise);
        self.jump(&done);
        self.label(&done);
    }

    fn raise(&mut self, exc: &str, cause: &str) {
        // Raising always transfers control to the enclosing handler.
        let op = self.output.data(b"raise");
        self.line(&format!(
            "call i64 @imre_dyn_op(ptr %ctx, ptr {op}, i64 {exc}, i64 {cause}, i64 0)"
        ));
        self.jump(&self.error.clone());
    }

    fn try_except(&mut self, stmt: &ast::StmtTry) {
        let handler = self.name("b");
        let done = self.name("b");
        let outer = std::mem::replace(&mut self.error, handler.clone());
        self.statements(&stmt.body);
        self.error = outer;
        self.statements(&stmt.orelse);
        self.jump(&done);
        self.label(&handler);
        let exc = self.op("take_error", &[]);
        self.live.push(exc.clone());
        for h in &stmt.handlers {
            let ast::ExceptHandler::ExceptHandler(h) = h;
            let next = self.name("b");
            if let Some(class) = &h.type_ {
                let class = self.expr(class);
                let matches = self.op("matches", &[&exc, &class]);
                let matches = self.value(&format!("icmp eq i64 {matches}, 2"));
                let body = self.name("b");
                self.branch(&matches, &body, &next);
                self.label(&body);
            }
            if let Some(name) = &h.name {
                self.bind(name.as_str(), &exc);
            }
            self.caught.push(exc.clone());
            self.statements(&h.body);
            self.caught.pop();
            if !self.terminated
                && let Some(name) = &h.name
            {
                let name = self.text(name.as_str());
                self.op("delname", &[&self.scope.clone(), &name]);
            }
            self.jump(&done);
            self.label(&next);
        }
        self.raise(&exc, "0");
        self.live.pop();
        self.label(&done);
    }

    fn close_contexts(&mut self, depth: usize) {
        let exits = self.exits[depth..].to_vec();
        for (exit, outer) in exits.iter().rev() {
            let previous = std::mem::replace(&mut self.error, outer.clone());
            self.invoke(exit, &["0", "0", "0"]);
            self.error = previous;
        }
    }

    fn with(&mut self, items: &[ast::WithItem], body: &[ast::Stmt]) {
        let Some((item, rest)) = items.split_first() else {
            self.statements(body);
            return;
        };
        let manager = self.expr(&item.context_expr);
        let enter = self.attr(&manager, "__enter__");
        let exit = self.attr(&manager, "__exit__");
        let value = self.invoke(&enter, &[]);
        let handler = self.name("b");
        let done = self.name("b");
        let outer = std::mem::replace(&mut self.error, handler.clone());
        self.exits.push((exit.clone(), outer.clone()));
        self.live.push(exit.clone());
        if let Some(target) = &item.optional_vars {
            self.assign(target, &value);
        }
        self.with(rest, body);
        self.live.pop();
        self.exits.pop();
        self.error = outer;
        if !self.terminated {
            self.invoke(&exit, &["0", "0", "0"]);
            self.jump(&done);
        }
        self.label(&handler);
        let exc = self.op("take_error", &[]);
        let ty = self.op("type", &[&exc]);
        let result = self.invoke(&exit, &[&ty, &exc, "0"]);
        let suppress = self.truth(&result);
        let propagate = self.name("b");
        self.branch(&suppress, &done, &propagate);
        self.label(&propagate);
        self.raise(&exc, "0");
        self.label(&done);
    }
}

fn relative_import(package: &str, level: u32, name: &str) -> Option<String> {
    if level == 0 {
        return Some(name.to_owned());
    }
    if package.is_empty() {
        return None;
    }
    let mut base = package;
    for _ in 1..level {
        base = base.rsplit_once('.')?.0;
    }
    Some(if name.is_empty() {
        base.to_owned()
    } else {
        format!("{base}.{name}")
    })
}

/// A call to a generator must never run the statements before its first yield.
fn is_generator(body: &[ast::Stmt]) -> bool {
    #[derive(Default)]
    struct Yields(bool);
    impl<'a> Visitor<'a> for Yields {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if !self.0 && !matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
                visitor::walk_stmt(self, stmt);
            }
        }
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            match expr {
                ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) => self.0 = true,
                ast::Expr::Lambda(_) => {}
                _ if !self.0 => visitor::walk_expr(self, expr),
                _ => {}
            }
        }
    }
    let mut yields = Yields::default();
    yields.visit_body(body);
    yields.0
}

/// A nested function can close over the declaring class through its enclosing method.
fn uses_class_cell(body: &Body<'_>) -> bool {
    struct ClassCell(bool);
    impl<'a> Visitor<'a> for ClassCell {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if !self.0 && !matches!(stmt, ast::Stmt::ClassDef(_)) {
                visitor::walk_stmt(self, stmt);
            }
        }
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            if matches!(expr, ast::Expr::Name(n) if matches!(n.id.as_str(), "super" | "__class__"))
            {
                self.0 = true;
            } else if !self.0 {
                visitor::walk_expr(self, expr);
            }
        }
    }
    let mut captures = ClassCell(false);
    match body {
        Body::Statements(stmts) => captures.visit_body(stmts),
        Body::Expression(expr) => captures.visit_expr(expr),
        Body::Generator(generator, element) => {
            for filter in &generator.ifs {
                captures.visit_expr(filter);
            }
            captures.visit_expr(element);
        }
        Body::Namespace(_) | Body::Annotations(_) | Body::Unsupported(..) => {}
    }
    captures.0
}

fn scalar_annotation(expr: &ast::Expr) -> Option<&'static str> {
    match expr {
        ast::Expr::NoneLiteral(_) => Some("None"),
        ast::Expr::Name(n) => match n.id.as_str() {
            "int" => Some("int"),
            "str" => Some("str"),
            "bool" => Some("bool"),
            "bytes" => Some("bytes"),
            "float" => Some("float"),
            _ => None,
        },
        _ => None,
    }
}
