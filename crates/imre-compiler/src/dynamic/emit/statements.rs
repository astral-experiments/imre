pub(super) mod class_annotations;
mod class_metadata;
pub(super) mod type_parameters;
mod worker;

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
    include_nested_reads: bool,
    incomplete: bool,
    locals_incomplete: bool,
}

#[derive(Clone)]
pub(super) struct ClassBindings {
    pub(super) locals: std::collections::BTreeSet<String>,
    pub(super) globals: std::collections::BTreeSet<String>,
}

pub(super) fn class_locals(body: &[ast::Stmt]) -> ClassBindings {
    let mut names = ClosureNames::default();
    names.visit_body(body);
    names
        .locals
        .retain(|name| !names.globals.contains(name) && !names.nonlocals.contains(name));
    ClassBindings {
        locals: names.locals,
        globals: names.globals,
    }
}

impl<'a> Visitor<'a> for ClosureNames {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        match stmt {
            ast::Stmt::FunctionDef(f) => {
                self.locals.insert(f.name.to_string());
                self.incomplete = true;
                if self.include_nested_reads {
                    // A name used only by a nested function can be free in this function too.
                    let mut child = Self {
                        include_nested_reads: true,
                        ..Self::default()
                    };
                    child.visit_body(&f.body);
                    child
                        .locals
                        .extend(f.parameters.iter().map(|param| param.name().to_string()));
                    self.reads.extend(child.nonlocals);
                    self.reads.extend(child.reads.into_iter().filter(|name| {
                        !child.locals.contains(name) && !child.globals.contains(name)
                    }));
                }
                // Loading a name or a literal cannot introduce a binding in the
                // defining frame. Keep the conservative fallback for other expressions.
                for decorator in &f.decorator_list {
                    self.visit_decorator(decorator);
                    self.locals_incomplete |= !decorator.expression.is_name_expr();
                }
                for param in f.parameters.iter_non_variadic_params() {
                    if let Some(default) = &param.default {
                        self.visit_expr(default);
                        self.locals_incomplete |=
                            !default.is_literal_expr() && !default.is_name_expr();
                    }
                }
            }
            ast::Stmt::ClassDef(c) => {
                self.locals.insert(c.name.to_string());
                self.incomplete = true;
                // Only class header expressions can introduce names in this enclosing function.
                for decorator in &c.decorator_list {
                    self.visit_decorator(decorator);
                }
                if let Some(args) = &c.arguments {
                    self.visit_arguments(args);
                }
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
                ast::Stmt::TypeAlias(v)
                    if self.prefix.is_empty()
                        && v.type_params.is_none()
                        && let Some(name) = v.name.as_name_expr() =>
                {
                    let evaluator = self.define(
                        v.start(),
                        name.id.as_str(),
                        None,
                        None,
                        None,
                        Body::TypeAlias(&v.value),
                    );
                    let label = self.code_name(name.id.as_str());
                    let alias = self.op("type_alias", &[&label, &evaluator]);
                    self.bind(name.id.as_str(), &alias);
                }
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
                            let value = self.output.code(
                                self.source,
                                String::new(),
                                Body::AnnotationValue(&v.annotation),
                            );
                            let code = self.output.definition(
                                self.source,
                                String::new(),
                                Body::AnnotationThunk(value.clone(), v.annotation.range()),
                                v.annotation.start(),
                            );
                            self.remember_annotation(v, name.id.as_str(), &value);
                            let name = self.text("__annotate__");
                            let sig = self.text("{}");
                            let defaults = self.op("dict", &[]);
                            let thunk = self.checked(&format!("call i64 @imre_dyn_def_from_site(ptr %ctx, ptr {code}, i64 {}, i64 {name}, i64 {sig}, i64 {defaults}, i64 {name}, i64 0, i64 0, i1 true)", self.scope));
                            let offset = v.start().to_u32();
                            self.checked(&format!("call i64 @imre_dyn_class_annotation(ptr %ctx, i64 {}, i64 {key}, i64 {thunk}, i32 {offset})", self.scope));
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
                    let generator = imre_ast::code_flags::is_generator(&v.body);
                    let body = if v.is_async && generator {
                        Body::Unsupported(stmt, "async function")
                    } else if v.is_async || generator {
                        Body::GeneratorFunction((&v.body, v.is_async))
                    } else {
                        Body::Statements(&v.body)
                    };
                    let function = self.define(
                        v.decorator_list.first().map_or(v.start(), Ranged::start),
                        self.source.private_names.definition(&v.name),
                        Some(&v.parameters),
                        v.returns.as_deref(),
                        crate::dynamic::function_type_params(v).filter(|_| self.prefix.is_empty()),
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
                    let outer = self.scope.clone();
                    let mut outer_class_locals = None;
                    let parameter_scope = v.type_params.as_deref().map(|params| {
                        let scope = self.parameter_scope(&outer, params, true);
                        self.scope = self.op("class_base_scope", &[&scope, &outer]);
                        outer_class_locals = self.class_locals.take();
                        scope
                    });
                    let bases = self.op("list", &[]);
                    let kwargs = if let Some(args) = v.arguments.as_deref() {
                        if args.keywords.iter().any(|kw| kw.arg.is_none()) {
                            self.unsupported("expanded class keyword arguments");
                        }
                        self.arguments(&bases, args, "0")
                    } else {
                        "0".into()
                    };
                    if let Some(scope) = parameter_scope.as_ref() {
                        self.scope = scope.clone();
                        let generic = self.lookup(".generic_base");
                        self.op("append", &[&bases, &generic]);
                    }
                    let init = self.define(
                        v.decorator_list.first().map_or(v.start(), Ranged::start),
                        self.source.private_names.definition(&v.name),
                        None,
                        None,
                        None,
                        Body::Namespace(&v.body),
                    );
                    if parameter_scope.is_some() {
                        self.scope = outer;
                        self.class_locals = outer_class_locals;
                    }
                    let operation = if v.type_params.is_some() {
                        "parameterized_class"
                    } else {
                        "class"
                    };
                    let class = self.op(operation, &[&kwargs, &bases, &init]);
                    self.decorated(&v.name, &decorators, class);
                }
                ast::Stmt::Return(v) => {
                    let value = v
                        .value
                        .as_deref()
                        .map_or_else(|| "0".into(), |e| self.expr(e));
                    self.live.push(value.clone());
                    self.run_cleanups(0, Some(&value));
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
                        |f, yes, no| {
                            let op = if v.orelse.is_empty() {
                                "more"
                            } else {
                                "more_else"
                            };
                            let more = f.op(op, &[&iter]);
                            let ready = f.value(&format!("icmp eq i64 {more}, 2"));
                            f.branch(&ready, yes, no);
                        },
                        |f| {
                            let item = f.op("next", &[&iter]);
                            f.assign(&v.target, &item);
                        },
                    );
                }
                ast::Stmt::While(v) => {
                    self.loop_body(
                        &v.body,
                        &v.orelse,
                        None,
                        |f, yes, no| f.branch_expr(&v.test, yes, no),
                        |_| {},
                    );
                }
                ast::Stmt::Break(_) | ast::Stmt::Continue(_) => {
                    if let Some(target) = self.loops.last() {
                        let (dest, iterator) = if matches!(stmt, ast::Stmt::Break(_)) {
                            (target.done.clone(), target.iterator.clone())
                        } else {
                            (target.step.clone(), None)
                        };
                        self.run_cleanups(target.cleanup_depth, None);
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
                    let fromlist = self.op("list", &[]);
                    let names: Vec<_> = v
                        .names
                        .iter()
                        .map(|alias| self.code_name(alias.name.as_str()))
                        .collect();
                    for attr in &names {
                        self.op("append", &[&fromlist, attr]);
                    }
                    self.op("fromlist", &[&module, &fromlist]);
                    for (alias, attr) in v.names.iter().zip(names) {
                        if alias.name.as_str() == "*" {
                            self.op("import_star", &[&self.scope.clone(), &module]);
                            continue;
                        }
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
                    let pass = self.name("b");
                    let fail = self.name("b");
                    self.branch_expr(&v.test, &pass, &fail);
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

    fn parameter_scope(&mut self, outer: &str, params: &ast::TypeParams, class: bool) -> String {
        let names = self.op("dict", &[]);
        for param in params {
            if param.default().is_some()
                || matches!(param, ast::TypeParam::TypeVar(v) if v.bound.is_some())
            {
                self.unsupported("class type parameter bounds and defaults");
            }
            let binding = self.code_name(param.name().as_str());
            let name =
                self.code_name(self.source.private_names.definition(param.name()));
            let declaration = match param {
                ast::TypeParam::TypeVar(_) => name,
                ast::TypeParam::TypeVarTuple(_) => {
                    self.op("class_typevartuple", &[&name])
                }
                ast::TypeParam::ParamSpec(_) => {
                    self.op("class_paramspec", &[&name])
                }
            };
            self.op("put", &[&names, &binding, &declaration]);
        }
        let op = if class {
            "class_parameter_scope"
        } else {
            "function_parameter_scope"
        };
        self.op(op, &[outer, &names])
    }

    pub(super) fn define(
        &mut self,
        start: TextSize,
        name: &str,
        params: Option<&ast::Parameters>,
        returns: Option<&ast::Expr>,
        type_params: Option<&ast::TypeParams>,
        body: Body<'_>,
    ) -> String {
        let qualname = format!("{}{name}", self.prefix);
        let prefix = if matches!(body, Body::Namespace(_)) {
            format!("{qualname}.")
        } else {
            format!("{qualname}.<locals>.")
        };
        let doc = match &body {
            Body::Statements(stmts) | Body::GeneratorFunction((stmts, _))
                if self.output.keep_docstrings =>
            {
                self.source.docstring(&qualname, stmts)
            }
            Body::Unsupported(ast::Stmt::FunctionDef(f), _) if self.output.keep_docstrings => {
                self.source.docstring(&qualname, &f.body)
            }
            _ => None,
        };
        let flags = match (&body, params) {
            (Body::Namespace(_) | Body::Annotations(..) | Body::ClassAnnotations(..), _) => None,
            _ => {
                use imre_ast::code_flags::*;
                let mut flags = OPTIMIZED | NEWLOCALS;
                if params.is_some_and(|p| p.vararg.is_some()) {
                    flags |= VARARGS;
                }
                if params.is_some_and(|p| p.kwarg.is_some()) {
                    flags |= VARKEYWORDS;
                }
                if type_params.is_some() || self.prefix.contains(".<locals>.") {
                    flags |= NESTED;
                }
                if !self.prefix.is_empty() && !self.prefix.ends_with(".<locals>.") {
                    flags |= METHOD;
                }
                if self.source.string_annotations {
                    flags |= FUTURE_ANNOTATIONS;
                }
                if self.source.barry_as_flufl {
                    flags |= FUTURE_BARRY_AS_BDFL;
                }
                if doc.is_some() {
                    flags |= HAS_DOCSTRING;
                }
                let kind = match &body {
                    Body::GeneratorFunction((_, true)) => COROUTINE,
                    Body::GeneratorFunction((_, false)) | Body::Generator(..) => GENERATOR,
                    Body::Expression(expr) if yields_in_expression(expr) => GENERATOR,
                    Body::Unsupported(ast::Stmt::FunctionDef(f), _) if f.is_async => {
                        if is_generator(&f.body) {
                            ASYNC_GENERATOR
                        } else {
                            COROUTINE
                        }
                    }
                    _ => 0,
                };
                Some(flags | kind)
            }
        };
        let doc = doc.map_or_else(|| "0".into(), |s| self.text(&s));
        let entries = params.map_or_else(Vec::new, |p| {
            crate::frontend::parameter_annotations(p, returns)
        });
        let outer = self.scope.clone();
        let scope = type_params
            .map(|params| self.parameter_scope(&outer, params, false))
            .unwrap_or(outer);
        let annotate = if entries.is_empty() {
            "0".into()
        } else {
            let class = self
                .class_locals
                .as_ref()
                .filter(|_| !self.source.string_annotations)
                .cloned();
            let reads = class_metadata::annotation_reads(
                &entries,
                class.as_ref(),
                self.source.string_annotations,
            );
            use imre_ast::code_flags::*;
            let mut annotation_flags = OPTIMIZED | NEWLOCALS;
            if type_params.is_some() || self.prefix.contains(".<locals>.") {
                annotation_flags |= NESTED;
            }
            if self.source.string_annotations {
                annotation_flags |= FUTURE_ANNOTATIONS;
            }
            if self.source.barry_as_flufl {
                annotation_flags |= FUTURE_BARRY_AS_BDFL;
            }
            let sig = self.text(
                &json!({
                    "posonly": ["format"],
                    "code_flags": annotation_flags,
                    "scope_reads": reads,
                    "scope_complete": true,
                    "scope_locals_complete": true,
                    "annotation_class": class.is_some(),
                })
                .to_string(),
            );
            let code = self.output.definition(
                self.source,
                String::new(),
                Body::Annotations(entries, class),
                params.map_or(start, Ranged::start),
            );
            let aname = self.text("__annotate__");
            let aqname = self.text(&format!("{qualname}.__annotate__"));
            let defaults = self.op("dict", &[]);
            self.checked(&format!("call i64 @imre_dyn_def_from_site(ptr %ctx, ptr {code}, i64 {}, i64 {aname}, i64 {sig}, i64 {defaults}, i64 {aqname}, i64 0, i64 0, i1 true)", scope))
        };
        let qualname = self.text(&qualname);
        let mut signature =
            json!({"posonly":[],"positional":[],"kwonly":[],"varargs":null,"kwargs":null});
        if let Some(flags) = flags {
            signature["code_flags"] = json!(flags);
        }
        if let Body::Namespace(stmts) = &body {
            signature["postponed_annotations"] = json!(
                self.source.string_annotations
                    && imre_ast::annotations::has_variable_annotation(stmts)
            );
            signature["needs_class_cell"] = json!(imre_ast::class_cell::needed(
                stmts,
                self.source.string_annotations
            ));
            signature["needs_dict_cell"] = json!(imre_ast::class_cell::dict_needed(
                stmts,
                self.source.string_annotations
            ));
            signature["static_attributes"] = json!(imre_ast::class_attributes::static_attributes(
                stmts,
                &self.source.private_names
            ));
        }
        if params.is_some() && uses_class_cell(&body) {
            signature["class_cell"] = json!(true);
        }
        let generator = if let Body::GeneratorFunction((stmts, is_async)) = &body {
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
            let header = if *is_async { "async def" } else { "def" };
            let text = format!("{header} _():\n{indent}{}", &self.source.text[start..end]);
            Some((self.output.data(text.as_bytes()), self.source.line(first)))
        } else {
            None
        };
        let mut closure_names = ClosureNames {
            include_nested_reads: matches!(&body, Body::Statements(_) | Body::GeneratorFunction(_)),
            ..ClosureNames::default()
        };
        match &body {
            Body::Statements(stmts) | Body::GeneratorFunction((stmts, _)) => {
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
            Body::Expression(expr) | Body::TypeAlias(expr) => closure_names.visit_expr(expr),
            _ => closure_names.incomplete = true,
        }
        if let (Body::Statements(stmts), Some(params), None) = (&body, params, returns) {
            if let Some((names, lock_only)) = worker::parameters(stmts, params) {
                signature["thread_parameters"] = json!(names);
                if lock_only {
                    signature["thread_worker_locks"] = json!(names);
                }
            } else if let Some(names) = worker::captured_locks(stmts, params) {
                signature["thread_captured_locks"] = json!(names);
            } else if let Some(field) = worker::bound_lock_field(stmts, params) {
                signature["thread_bound_lock_field"] = json!(field);
            }
        }
        let defaults = self.op("dict", &[]);
        if matches!(body, Body::TypeAlias(_)) {
            signature["posonly"] = json!([".format"]);
            let format = self.text(".format");
            let value_format = self.integer(1);
            self.op("put", &[&defaults, &format, &value_format]);
        }
        let code = self.output.definition(self.source, prefix, body, start);
        signature["types"] = json!({});
        // A generator call returns a suspended frame, before it yields or returns a value.
        let scalar = |expr: &ast::Expr| {
            scalar_annotation(expr).filter(|name| {
                !type_params.is_some_and(|params| params.iter().any(|p| p.name().as_str() == *name))
            })
        };
        if generator.is_none() {
            signature["return_type"] = json!(returns.and_then(scalar));
        }
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
                    if let Some(ty) = arg.parameter.annotation.as_deref().and_then(scalar) {
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
        let function = self.checked(&format!("call i64 @imre_dyn_def_from_site(ptr %ctx, ptr {code}, i64 {}, i64 {name}, i64 {sig}, i64 {defaults}, i64 {qualname}, i64 {doc}, i64 {annotate}, i1 false)", scope));
        if type_params.is_some() {
            let key = self.code_name(".type_params");
            let params = self.op("lookup", &[&scope, &key]);
            let key = self.code_name("__type_params__");
            self.op("setattr", &[&function, &key, &params]);
        }
        if let Some((source, line)) = generator {
            self.line(&format!("call void @imre_dyn_generator_body(ptr %ctx, i64 {function}, ptr {source}, i32 {line})"));
        }
        function
    }

    fn check_annotation_format(&mut self) {
        // CPython's compiled annotation callable accepts VALUE and VALUE_WITH_FAKE_GLOBALS.
        let format = self.lookup("format");
        let two = self.integer(2);
        let unsupported = self.op(">", &[&format, &two]);
        let bad = self.name("b");
        let good = self.name("b");
        let condition = self.truth(&unsupported);
        self.branch(&condition, &bad, &good);
        self.label(&bad);
        let name = self.code_name("NotImplementedError");
        let error = self.op("builtin", &[&name]);
        self.raise(&error, None, false);
        self.label(&good);
    }

    pub(super) fn annotations(
        &mut self,
        entries: &[(&str, &ast::Expr)],
        class: Option<ClassBindings>,
    ) {
        self.check_annotation_format();
        let values = self.op("dict", &[]);
        // The public format parameter is outside the annotation expressions' lexical scope.
        self.scope = self.op("annotation_scope", &[&self.scope.clone()]);
        self.annotation_lookup = true;
        self.class_locals = class;
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
                        self.release_replaced(&replaced);
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
        self.branch_expr(test, &body_label, &next);
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
        condition: impl FnOnce(&mut Self, &str, &str),
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
        condition(self, &body_label, &exhausted);
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
            "call i64 @imre_dyn_op_at(ptr %ctx, ptr {op}, i64 {exc}, i64 {cause}, i64 {explicit}, ptr null)"
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
        let enclosing = self.caught.last().cloned().unwrap_or_else(|| "0".into());
        self.exits.push((
            Cleanup::PendingValue {
                value: exception.clone(),
                enclosing_exception: Some(enclosing),
            },
            self.error.clone(),
        ));
        self.caught.push(exception.clone());
        self.statements(&stmt.finalbody);
        self.caught.pop();
        self.exits.pop();
        self.live.pop();
        if !self.terminated {
            self.raise(&exception, None, true);
        }
        self.label(&done);
    }

    /// Collect discarded owners while retaining the operands needed after the transfer.
    fn release_replaced(&mut self, replaced: &str) {
        let (ptr, count) = self.live_handles();
        self.line(&format!("call void @imre_dyn_assignment_target(ptr %ctx, ptr {ptr}, i64 {count}, i64 {replaced})"));
    }

    /// A return, break or continue inside a finally may replace the pending exit.
    fn run_cleanups(&mut self, depth: usize, pending: Option<&str>) {
        let active = self.exits.clone();
        let mut discarded = Vec::new();
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
                Cleanup::PendingValue {
                    value,
                    enclosing_exception,
                } => {
                    if let Some(caught) = enclosing_exception {
                        self.op("restore_caught", &[caught]);
                    }
                    let slot = self.live.iter().rposition(|live| live == value).unwrap();
                    discarded.push((slot, self.live.remove(slot)));
                    self.release_replaced("2");
                }
                Cleanup::Finally {
                    body,
                    loop_depth,
                    caught_depth,
                } => {
                    // Loops and handlers entered inside the try do not enclose its finally.
                    let protected_loops = self.loops.split_off(*loop_depth);
                    let protected_handlers = self.caught.split_off(*caught_depth);
                    if let Some(value) = pending.filter(|value| *value != "0") {
                        self.exits.push((
                            Cleanup::PendingValue {
                                value: value.into(),
                                enclosing_exception: None,
                            },
                            outer.clone(),
                        ));
                    }
                    self.statements(body);
                    self.caught.extend(protected_handlers);
                    self.loops.extend(protected_loops);
                }
            }
            self.error = previous;
        }
        for (slot, value) in discarded.into_iter().rev() {
            self.live.insert(slot, value);
        }
        self.exits = active;
    }

    fn with(&mut self, items: &[ast::WithItem], body: &[ast::Stmt]) {
        let Some((item, rest)) = items.split_first() else {
            self.statements(body);
            return;
        };
        let manager = self.expr(&item.context_expr);
        let exit = self.op("lookup_exit", &[&manager]);
        let enter = self.op("lookup_enter", &[&manager]);
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
        Body::Statements(stmts) | Body::GeneratorFunction((stmts, _)) => captures.visit_body(stmts),
        Body::Expression(expr) | Body::TypeAlias(expr) => captures.visit_expr(expr),
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
        Body::Namespace(_)
        | Body::Annotations(..)
        | Body::ClassAnnotations(..)
        | Body::AnnotationThunk(..)
        | Body::AnnotationValue(_)
        | Body::Unsupported(..) => {}
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
