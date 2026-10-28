mod class_metadata;

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_text_size::{Ranged, TextSize};
use serde_json::json;

use super::{Body, Cleanup, Function, LoopTarget};

#[derive(Default)]
struct ClosureNames {
    locals: std::collections::BTreeSet<String>,
    reads: std::collections::BTreeSet<String>,
    globals: std::collections::BTreeSet<String>,
    nonlocals: std::collections::BTreeSet<String>,
    in_comprehension: bool,
    incomplete: bool,
    locals_incomplete: bool,
}

pub(super) fn class_locals(body: &[ast::Stmt]) -> std::collections::BTreeSet<String> {
    let mut names = ClosureNames::default();
    names.visit_body(body);
    names
        .locals
        .retain(|name| !names.globals.contains(name) && !names.nonlocals.contains(name));
    names.locals
}

impl<'a> Visitor<'a> for ClosureNames {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        match stmt {
            ast::Stmt::FunctionDef(f) => {
                self.locals.insert(f.name.to_string());
                self.incomplete = true;
                if !f.decorator_list.is_empty()
                    || f.parameters
                        .posonlyargs
                        .iter()
                        .chain(&f.parameters.args)
                        .chain(&f.parameters.kwonlyargs)
                        .any(|arg| arg.default.is_some())
                {
                    self.locals_incomplete = true;
                }
            }
            ast::Stmt::ClassDef(c) => {
                self.locals.insert(c.name.to_string());
                self.incomplete = true;
                self.locals_incomplete = true;
            }
            ast::Stmt::Global(g) => {
                self.globals.extend(g.names.iter().map(ToString::to_string));
            }
            ast::Stmt::Nonlocal(n) => {
                self.nonlocals
                    .extend(n.names.iter().map(ToString::to_string));
            }
            ast::Stmt::Import(i) => {
                for name in &i.names {
                    self.locals.insert(name.asname.as_ref().map_or_else(
                        || name.name.as_str().split('.').next().unwrap().to_string(),
                        ToString::to_string,
                    ));
                }
            }
            ast::Stmt::ImportFrom(i) => {
                for name in &i.names {
                    if name.name.as_str() == "*" {
                        self.incomplete = true;
                        self.locals_incomplete = true;
                    } else {
                        self.locals
                            .insert(name.asname.as_ref().unwrap_or(&name.name).to_string());
                    }
                }
            }
            ast::Stmt::Match(_) | ast::Stmt::TypeAlias(_) => {
                self.incomplete = true;
                self.locals_incomplete = true;
                visitor::walk_stmt(self, stmt);
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }
    fn visit_pattern(&mut self, pattern: &'a ast::Pattern) {
        let capture = match pattern {
            ast::Pattern::MatchAs(p) => p.name.as_ref(),
            ast::Pattern::MatchStar(p) => p.name.as_ref(),
            ast::Pattern::MatchMapping(p) => p.rest.as_ref(),
            _ => None,
        };
        if let Some(name) = capture {
            self.locals.insert(name.to_string());
        }
        visitor::walk_pattern(self, pattern);
    }

    fn visit_except_handler(&mut self, handler: &'a ast::ExceptHandler) {
        let ast::ExceptHandler::ExceptHandler(handler_data) = handler;
        if let Some(name) = &handler_data.name {
            self.locals.insert(name.to_string());
        }
        visitor::walk_except_handler(self, handler);
    }
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        match expr {
            ast::Expr::Named(named) if self.in_comprehension => {
                if let ast::Expr::Name(target) = &*named.target {
                    self.locals.insert(target.id.to_string());
                }
                self.visit_expr(&named.value);
            }
            ast::Expr::Name(_) if self.in_comprehension => {}
            ast::Expr::Name(n) => match n.ctx {
                ast::ExprContext::Load => {
                    self.reads.insert(n.id.to_string());
                }
                ast::ExprContext::Store | ast::ExprContext::Del => {
                    self.locals.insert(n.id.to_string());
                }
                _ => {}
            },
            ast::Expr::Lambda(lambda) => {
                self.incomplete = true;
                if let Some(parameters) = &lambda.parameters {
                    for parameter in parameters.iter_non_variadic_params() {
                        if let Some(default) = &parameter.default {
                            self.visit_expr(default);
                        }
                    }
                }
            }
            ast::Expr::ListComp(_)
            | ast::Expr::SetComp(_)
            | ast::Expr::DictComp(_)
            | ast::Expr::Generator(_) => {
                self.incomplete = true;
                self.locals_incomplete = true;
                let previous = std::mem::replace(&mut self.in_comprehension, true);
                visitor::walk_expr(self, expr);
                self.in_comprehension = previous;
            }
            _ => visitor::walk_expr(self, expr),
        }
    }
}

enum Place {
    Name(String),
    Attr(String, String),
    Item(String, String),
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
            let name = self.code_name(name);
            self.op("declare_global", &[&self.scope.clone(), &name]);
        }
    }

    pub(super) fn statements(&mut self, suite: &[ast::Stmt]) {
        for stmt in suite {
            if self.terminated {
                break;
            }
            self.checkpoint(stmt);
            self.simple_call = match stmt {
                ast::Stmt::Assign(v)
                    if matches!(v.targets.as_slice(), [ast::Expr::Name(_)])
                        && matches!(&*v.value, ast::Expr::Call(_)) =>
                {
                    Some(v.value.range())
                }
                ast::Stmt::Return(v) => v.value.as_deref().and_then(|e| match e {
                    ast::Expr::Call(c) if matches!(&*c.func, ast::Expr::Name(_)) => Some(e.range()),
                    _ => None,
                }),
                _ => None,
            };
            match stmt {
                ast::Stmt::Expr(v) => {
                    if !matches!(&*v.value, ast::Expr::StringLiteral(_)) {
                        self.expr(&v.value);
                    }
                }
                ast::Stmt::Pass(_) | ast::Stmt::Global(_) | ast::Stmt::Nonlocal(_) => {}
                ast::Stmt::Assign(v) => {
                    let value = self.expr(&v.value);
                    let chained = v.targets.len() > 1;
                    if chained {
                        self.live.push(value.clone());
                    }
                    for (index, target) in v.targets.iter().enumerate() {
                        if chained && index + 1 == v.targets.len() {
                            self.live.pop();
                        }
                        let owned =
                            chained || matches!(target, ast::Expr::List(_) | ast::Expr::Tuple(_));
                        self.assign_target(target, &value, owned);
                    }
                }
                ast::Stmt::Delete(v) => {
                    for target in &v.targets {
                        match self.place(target) {
                            Some(Place::Name(name)) => {
                                self.op("delname", &[&self.scope.clone(), &name]);
                            }
                            Some(Place::Item(obj, key)) => {
                                self.op("delitem", &[&obj, &key]);
                            }
                            Some(Place::Attr(obj, name)) => {
                                self.op("delattr", &[&obj, &name]);
                            }
                            _ => {
                                self.unsupported("deletion target");
                            }
                        }
                    }
                }
                ast::Stmt::AnnAssign(v) => {
                    if let Some(value) = &v.value {
                        let value = self.expr(value);
                        self.assign(&v.target, &value);
                    }
                    if let ast::Expr::Name(name) = &*v.target
                        && (self.class_locals.is_none() || v.simple)
                    {
                        let key = self.code_name(name.id.as_str());
                        if self.class_locals.is_some() && !self.source.string_annotations {
                            // Class annotations may refer to a name bound after the class body.
                            let code = self.output.definition(
                                self.source,
                                String::new(),
                                Body::Expression(&v.annotation),
                                v.annotation.start(),
                            );
                            let name = self.text("__annotate__");
                            let sig = self.text("{}");
                            let defaults = self.op("dict", &[]);
                            let thunk = self.checked(&format!("call i64 @imre_dyn_def_from_site(ptr %ctx, ptr {code}, i64 {}, i64 {name}, i64 {sig}, i64 {defaults}, i64 {name}, i64 0, i64 0, i1 true)", self.scope));
                            self.op("annotate_deferred", &[&self.scope.clone(), &key, &thunk]);
                        } else {
                            let value =
                                if self.class_locals.is_some() && self.source.string_annotations {
                                    self.text(&crate::frontend::annotation_spelling(&v.annotation))
                                } else {
                                    "0".into()
                                };
                            self.op("annotate", &[&self.scope.clone(), &key, &value]);
                        }
                    }
                }
                ast::Stmt::AugAssign(v) => {
                    if let Some(target) = self.place(&v.target) {
                        let left = self.read(&target);
                        let right = self.expr(&v.value);
                        let op = format!("inplace:{}", v.op.as_str());
                        let value = self.op(&op, &[&left, &right]);
                        self.write(&target, &value);
                    }
                }
                ast::Stmt::FunctionDef(v) => {
                    let decorators = v
                        .decorator_list
                        .iter()
                        .map(|d| self.expr(&d.expression))
                        .collect::<Vec<_>>();
                    let generator = is_generator(&v.body);
                    let body = if v.is_async {
                        Body::Unsupported(stmt, "async function")
                    } else if generator {
                        Body::GeneratorFunction(&v.body)
                    } else {
                        Body::Statements(&v.body)
                    };
                    let function = self.define(
                        v.decorator_list.first().map_or(v.start(), Ranged::start),
                        self.source.private_names.definition(&v.name),
                        Some(&v.parameters),
                        v.returns.as_deref(),
                        body,
                    );
                    if v.is_async && !generator {
                        // Set CO_COROUTINE on the definition before any decorator
                        // can inspect it. Async generators use a different flag.
                        self.op("coroutine_function", &[&function]);
                    }
                    self.decorated(&v.name, &decorators, function);
                }
                ast::Stmt::ClassDef(v) => {
                    let decorators = v
                        .decorator_list
                        .iter()
                        .map(|d| self.expr(&d.expression))
                        .collect::<Vec<_>>();
                    let bases = self.op("list", &[]);
                    if let Some(args) = v.arguments.as_deref() {
                        if !args.keywords.is_empty() {
                            self.unsupported("class keyword arguments");
                        }
                        self.arguments(&bases, args);
                    }
                    let init = self.define(
                        v.start(),
                        self.source.private_names.definition(&v.name),
                        None,
                        None,
                        Body::Namespace(&v.body),
                    );
                    let name = self.code_name(self.source.private_names.definition(&v.name));
                    let class = self.op("class", &[&name, &bases, &init]);
                    self.decorated(&v.name, &decorators, class);
                }
                ast::Stmt::Return(v) => {
                    let value = v
                        .value
                        .as_deref()
                        .map_or_else(|| "0".into(), |e| self.expr(e));
                    self.live.push(value.clone());
                    self.run_cleanups(0);
                    self.live.pop();
                    self.end(&format!("ret i64 {value}"));
                }
                ast::Stmt::Match(v) => self.match_statement(v),
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
                    self.loop_body(
                        &v.body,
                        &v.orelse,
                        Some(&iter),
                        |f| {
                            let op = if v.orelse.is_empty() {
                                "more"
                            } else {
                                "more_else"
                            };
                            let more = f.op(op, &[&iter]);
                            f.value(&format!("icmp eq i64 {more}, 2"))
                        },
                        |f| {
                            let item = f.op("next", &[&iter]);
                            f.assign(&v.target, &item);
                        },
                    );
                }
                ast::Stmt::While(v) => {
                    self.loop_body(&v.body, &v.orelse, None, |f| f.truth_expr(&v.test), |_| {});
                }
                ast::Stmt::Break(_) | ast::Stmt::Continue(_) => {
                    if let Some(target) = self.loops.last() {
                        let (dest, iterator) = if matches!(stmt, ast::Stmt::Break(_)) {
                            (target.done.clone(), target.iterator.clone())
                        } else {
                            (target.step.clone(), None)
                        };
                        self.run_cleanups(target.cleanup_depth);
                        if let Some(iter) = iterator
                            && !self.terminated
                        {
                            self.op("break_iter", &[&iter]);
                        }
                        self.jump(&dest);
                    } else {
                        self.unsupported("loop control outside a loop");
                    }
                }
                ast::Stmt::Import(v) => {
                    for alias in &v.names {
                        let full = alias.name.as_str();
                        let module = self.import(full);
                        if let Some(name) = &alias.asname
                            && !self.source.private_names.imports_root(&alias.name)
                        {
                            self.bind(name.as_str(), &module);
                        } else if let Some((root, _)) = full.split_once('.') {
                            let outer = self.import(root);
                            let binding =
                                alias.asname.as_ref().map_or(root, ast::Identifier::as_str);
                            self.bind(binding, &outer);
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
                        let attr = self.code_name(alias.name.as_str());
                        let value = self.op("from", &[&module, &attr]);
                        self.bind(
                            alias.asname.as_ref().unwrap_or(&alias.name).as_str(),
                            &value,
                        );
                    }
                }
                ast::Stmt::Try(v) if !v.is_star => {
                    if v.finalbody.is_empty() {
                        self.try_except(v);
                    } else {
                        self.try_finally(v);
                    }
                }
                ast::Stmt::With(v) if !v.is_async => self.with(&v.items, &v.body),
                ast::Stmt::Raise(v) => {
                    let exc = if let Some(e) = &v.exc {
                        self.expr(e)
                    } else {
                        self.caught.last().cloned().unwrap_or_else(|| "0".into())
                    };
                    let cause = v.cause.as_deref().map(|e| self.expr(e));
                    self.raise(&exc, cause.as_deref(), v.exc.is_none());
                }
                ast::Stmt::Assert(v) => {
                    let condition = self.truth_expr(&v.test);
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
                    self.raise(&error, None, false);
                    self.label(&pass);
                }
                _ => {
                    self.unsupported(&format!("{:?}", ast::AnyNodeRef::from(stmt).kind()));
                }
            }
        }
    }

    fn import(&mut self, name: &str) -> String {
        let name = self.code_name(name);
        self.op("import", &[&self.scope.clone(), &name])
    }

    pub(super) fn define(
        &mut self,
        start: TextSize,
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
            Body::Statements(stmts) | Body::GeneratorFunction(stmts)
                if self.output.keep_docstrings =>
            {
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
            let code = self.output.definition(
                self.source,
                String::new(),
                Body::Annotations(entries),
                params.map_or(start, Ranged::start),
            );
            let aname = self.text("__annotate__");
            let aqname = self.text(&format!("{qualname}.__annotate__"));
            let sig = self.text(r#"{"posonly":["format"]}"#);
            let defaults = self.op("dict", &[]);
            self.checked(&format!("call i64 @imre_dyn_def_from_site(ptr %ctx, ptr {code}, i64 {}, i64 {aname}, i64 {sig}, i64 {defaults}, i64 {aqname}, i64 0, i64 0, i1 true)", self.scope))
        };
        let qualname = self.text(&qualname);
        let mut signature =
            json!({"posonly":[],"positional":[],"kwonly":[],"varargs":null,"kwargs":null});
        if params.is_some() && uses_class_cell(&body) {
            signature["class_cell"] = json!(true);
        }
        let generator = if let Body::GeneratorFunction(stmts) = &body {
            if !self.source.method_imports.is_empty() {
                signature["method_imports"] = json!(self.source.method_imports);
            }
            let first = stmts[0].start();
            if let Some(owner) = self.source.private_names.suite_owner(first) {
                signature["private_scope"] = json!([owner, self.source.string_annotations]);
            }
            let start = first.to_usize();
            let end = stmts.last().unwrap().end().to_usize();
            let prefix = self.source.text[..start].rsplit('\n').next().unwrap();
            let indent = if prefix.chars().all(char::is_whitespace) {
                prefix
            } else {
                " "
            };
            // Slice the suite as written so indentation inside multiline strings survives.
            let text = format!("def _():\n{indent}{}", &self.source.text[start..end]);
            Some((self.output.data(text.as_bytes()), self.source.line(first)))
        } else {
            None
        };
        let mut closure_names = ClosureNames::default();
        match &body {
            Body::Statements(stmts) | Body::GeneratorFunction(stmts) => {
                closure_names.visit_body(stmts)
            }
            Body::Namespace(stmts) => {
                let mut bindings = ClosureNames::default();
                bindings.visit_body(stmts);
                class_metadata::capture(&mut bindings, stmts, self.source.string_annotations);
                closure_names.nonlocals = bindings.nonlocals;
                // Methods and nested classes can still capture the enclosing frame.
                closure_names.incomplete = true;
            }
            Body::Expression(expr) => closure_names.visit_expr(expr),
            _ => closure_names.incomplete = true,
        }
        let code = self.output.definition(self.source, prefix, body, start);
        let defaults = self.op("dict", &[]);
        signature["types"] = json!({});
        signature["return_type"] = json!(returns.and_then(scalar_annotation));
        if let Some(p) = params {
            self.output
                .interned_identifiers
                .extend(p.iter().map(|p| p.name().to_string()));
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
                        let name = self.code_name(name);
                        self.op("put", &[&defaults, &name, &default]);
                    }
                }
            }
            signature["varargs"] = json!(p.vararg.as_deref().map(|x| x.name.as_str()));
            signature["kwargs"] = json!(p.kwarg.as_deref().map(|x| x.name.as_str()));
        }
        for group in ["posonly", "positional", "kwonly"] {
            closure_names.locals.extend(
                signature[group]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned)),
            );
        }
        for group in ["varargs", "kwargs"] {
            if let Some(name) = signature[group].as_str() {
                closure_names.locals.insert(name.to_owned());
            }
        }
        for name in closure_names.globals.iter().chain(&closure_names.nonlocals) {
            closure_names.locals.remove(name);
        }
        closure_names.reads.retain(|name| {
            !closure_names.locals.contains(name) && !closure_names.globals.contains(name)
        });
        closure_names
            .reads
            .extend(closure_names.nonlocals.iter().cloned());
        signature["scope_locals"] = json!(closure_names.locals);
        signature["scope_reads"] = json!(closure_names.reads);
        signature["scope_nonlocals"] = json!(closure_names.nonlocals);
        signature["scope_complete"] = json!(!closure_names.incomplete);
        signature["scope_locals_complete"] = json!(!closure_names.locals_incomplete);
        let sig = self.text(&signature.to_string());
        let name = self.code_name(name);
        let function = self.checked(&format!("call i64 @imre_dyn_def_from_site(ptr %ctx, ptr {code}, i64 {}, i64 {name}, i64 {sig}, i64 {defaults}, i64 {qualname}, i64 {doc}, i64 {annotate}, i1 false)", self.scope));
        if let Some((source, line)) = generator {
            self.line(&format!("call void @imre_dyn_generator_body(ptr %ctx, i64 {function}, ptr {source}, i32 {line})"));
        }
        function
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
        self.raise(&error, None, false);
        self.label(&good);
        let values = self.op("dict", &[]);
        for &(name, expression) in entries {
            let key = self.code_name(name);
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
            ast::Expr::Name(n) => Place::Name(self.code_name(n.id.as_str())),
            ast::Expr::Attribute(a) => {
                let obj = self.expr(&a.value);
                Place::Attr(obj, self.code_name(a.attr.as_str()))
            }
            ast::Expr::Subscript(s) => {
                let obj = self.expr(&s.value);
                Place::Item(obj, self.expr(&s.slice))
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
        }
    }

    fn write(&mut self, place: &Place, value: &str) {
        match place {
            Place::Name(n) => self.op("assign", &[&self.scope.clone(), n, value]),
            Place::Attr(v, n) => self.op("setattr", &[v, n, value]),
            Place::Item(v, k) => self.op("put", &[v, k, value]),
        };
    }

    pub(super) fn assign(&mut self, target: &ast::Expr, value: &str) {
        self.assign_target(target, value, false);
    }

    /// Transfer unpacked items to their targets while retaining the outstanding operands.
    fn assign_target(&mut self, target: &ast::Expr, value: &str, owned: bool) {
        let elements = match target {
            ast::Expr::Tuple(t) => &t.elts,
            ast::Expr::List(l) => &l.elts,
            _ => {
                if let Some(p) = self.place(target) {
                    if owned {
                        let replaced = match &p {
                            Place::Name(name) => {
                                self.op("assign_target", &[&self.scope.clone(), name, value])
                            }
                            _ => {
                                self.write(&p, value);
                                "2".to_owned() // An attribute or item setter may replace a writer.
                            }
                        };
                        let (ptr, count) = self.live_handles();
                        self.line(&format!("call void @imre_dyn_assignment_target(ptr %ctx, ptr {ptr}, i64 {count}, i64 {replaced})"));
                    } else {
                        self.write(&p, value);
                    }
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
        if owned {
            self.live.push(sequence.clone());
        }
        for (index, target) in elements.iter().enumerate() {
            let i = self.integer(index);
            let item = self.op(if owned { "unpack_take" } else { "get" }, &[&sequence, &i]);
            let target = if let ast::Expr::Starred(s) = target {
                &s.value
            } else {
                target
            };
            self.assign_target(target, &item, owned);
        }
        if owned {
            self.live.pop();
        }
    }

    fn if_arm(&mut self, test: &ast::Expr, body: &[ast::Stmt], done: &str) {
        let body_label = self.name("b");
        let next = self.name("b");
        let condition = self.truth_expr(test);
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
        iterator: Option<&str>,
        condition: impl FnOnce(&mut Self) -> String,
        bind: impl FnOnce(&mut Self),
    ) {
        let step = self.name("b");
        let body_label = self.name("b");
        let exhausted = self.name("b");
        let done = self.name("b");
        if let Some(iterator) = iterator {
            self.live.push(iterator.to_owned());
        }
        self.jump(&step);
        self.label(&step);
        let condition = condition(self);
        self.branch(&condition, &body_label, &exhausted);
        self.label(&body_label);
        bind(self);
        self.loops.push(LoopTarget {
            step: step.clone(),
            done: done.clone(),
            iterator: iterator.map(str::to_owned),
            cleanup_depth: self.exits.len(),
        });
        self.statements(body);
        self.loops.pop();
        self.jump(&step);
        self.label(&exhausted);
        if iterator.is_some() {
            self.live.pop();
        }
        self.statements(otherwise);
        self.jump(&done);
        self.label(&done);
    }

    fn raise(&mut self, exc: &str, cause: Option<&str>, reraise: bool) {
        // Raising always transfers control to the enclosing handler.
        let op = self
            .output
            .data(if reraise { b"reraise" } else { b"raise" });
        let explicit = if cause.is_some() { 2 } else { 0 };
        let cause = cause.unwrap_or("0");
        self.line(&format!(
            "call i64 @imre_dyn_op(ptr %ctx, ptr {op}, i64 {exc}, i64 {cause}, i64 {explicit})"
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
        self.caught.push(exc.clone());
        for h in &stmt.handlers {
            let ast::ExceptHandler::ExceptHandler(h) = h;
            let next = self.name("b");
            if let Some(class) = &h.type_ {
                self.checkpoint(class.as_ref());
                let class = self.expr(class);
                let matches = self.op("matches", &[&exc, &class]);
                let matches = self.value(&format!("icmp eq i64 {matches}, 2"));
                let body = self.name("b");
                self.branch(&matches, &body, &next);
                self.label(&body);
            }
            if let Some(name) = &h.name {
                self.bind(name.as_str(), &exc);
                let failed = self.name("b");
                let outer = std::mem::replace(&mut self.error, failed.clone());
                self.exits
                    .push((Cleanup::ExceptionName(name.to_string()), outer.clone()));
                self.statements(&h.body);
                self.exits.pop();
                self.error = outer;
                if !self.terminated {
                    self.clear_exception_name(name.as_str());
                }
                self.jump(&done);
                self.label(&failed);
                let error = self.op("take_error", &[]);
                self.clear_exception_name(name.as_str());
                self.raise(&error, None, true);
            } else {
                self.statements(&h.body);
                self.jump(&done);
            }
            self.label(&next);
        }
        self.raise(&exc, None, true);
        self.caught.pop();
        self.live.pop();
        self.label(&done);
    }

    /// Remove a handler's binding even if it was changed or deleted by its body.
    fn clear_exception_name(&mut self, name: &str) {
        let name = self.code_name(name);
        self.op("clearname", &[&self.scope.clone(), &name]);
    }

    /// Run the same cleanup for normal fallthrough, a pending exception and nonlocal exits.
    fn try_finally(&mut self, stmt: &ast::StmtTry) {
        let handler = self.name("b");
        let done = self.name("b");
        let outer = std::mem::replace(&mut self.error, handler.clone());
        self.exits.push((
            Cleanup::Finally {
                body: stmt.finalbody.as_slice().into(),
                loop_depth: self.loops.len(),
                caught_depth: self.caught.len(),
            },
            outer.clone(),
        ));
        if stmt.handlers.is_empty() {
            self.statements(&stmt.body);
        } else {
            self.try_except(stmt);
        }
        self.exits.pop();
        self.error = outer;
        if !self.terminated {
            self.statements(&stmt.finalbody);
            self.jump(&done);
        }

        self.label(&handler);
        let exception = self.op("take_error", &[]);
        self.live.push(exception.clone());
        self.caught.push(exception.clone());
        self.statements(&stmt.finalbody);
        self.caught.pop();
        self.live.pop();
        if !self.terminated {
            self.raise(&exception, None, true);
        }
        self.label(&done);
    }

    /// A return, break or continue inside a finally may replace the pending exit.
    fn run_cleanups(&mut self, depth: usize) {
        let active = self.exits.clone();
        for index in (depth..active.len()).rev() {
            if self.terminated {
                break;
            }
            self.exits.truncate(index);
            let (cleanup, outer) = &active[index];
            let previous = std::mem::replace(&mut self.error, outer.clone());
            match cleanup {
                Cleanup::With { exit, caught } => {
                    self.op("exit_context", &[exit, "0", caught]);
                }
                Cleanup::ExceptionName(name) => self.clear_exception_name(name),
                Cleanup::Finally {
                    body,
                    loop_depth,
                    caught_depth,
                } => {
                    // Loops and handlers entered inside the try do not enclose its finally.
                    let protected_loops = self.loops.split_off(*loop_depth);
                    let protected_handlers = self.caught.split_off(*caught_depth);
                    self.statements(body);
                    self.caught.extend(protected_handlers);
                    self.loops.extend(protected_loops);
                }
            }
            self.error = previous;
        }
        self.exits = active;
    }

    fn with(&mut self, items: &[ast::WithItem], body: &[ast::Stmt]) {
        let Some((item, rest)) = items.split_first() else {
            self.statements(body);
            return;
        };
        let manager = self.expr(&item.context_expr);
        let enter = self.op("lookup_enter", &[&manager]);
        let exit = self.op("lookup_exit", &[&manager]);
        let value = self.invoke(&enter, &[]);
        let handler = self.name("b");
        let done = self.name("b");
        let outer = std::mem::replace(&mut self.error, handler.clone());
        let caught = self.caught.last().cloned().unwrap_or_else(|| "0".into());
        self.exits.push((
            Cleanup::With {
                exit: exit.clone(),
                caught: caught.clone(),
            },
            outer.clone(),
        ));
        self.live.push(exit.clone());
        if let Some(target) = &item.optional_vars {
            self.assign(target, &value);
        }
        self.with(rest, body);
        self.live.pop();
        self.exits.pop();
        self.error = outer;
        if !self.terminated {
            self.op("exit_context", &[&exit, "0", &caught]);
            self.jump(&done);
        }
        self.label(&handler);
        let exc = self.op("take_error", &[]);
        let result = self.op("exit_context", &[&exit, &exc]);
        let suppress = self.truth(&result);
        let propagate = self.name("b");
        self.branch(&suppress, &done, &propagate);
        self.label(&propagate);
        self.raise(&exc, None, true);
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
        Body::Statements(stmts) | Body::GeneratorFunction(stmts) => captures.visit_body(stmts),
        Body::Expression(expr) => captures.visit_expr(expr),
        Body::Generator(generators, element) => {
            for (index, generator) in generators.iter().enumerate() {
                if index != 0 {
                    captures.visit_expr(&generator.iter);
                }
                for filter in &generator.ifs {
                    captures.visit_expr(filter);
                }
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
