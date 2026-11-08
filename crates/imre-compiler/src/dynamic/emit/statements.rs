pub(super) mod class_annotations;
mod class_metadata;
pub(super) mod type_parameters;
mod worker;

use imre_ast::code_flags;
use ruff_python_ast as ast;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_text_size::{Ranged, TextSize};
use serde_json::json;

use super::{Body, Cleanup, ContextExit, DefinitionName, Entry, Function, LoopTarget};

#[derive(Default)]
struct ClosureNames {
    locals: indexmap::IndexSet<String>,
    reads: std::collections::BTreeSet<String>,
    globals: std::collections::BTreeSet<String>,
    nonlocals: std::collections::BTreeSet<String>,
    include_nested_reads: bool,
    cell_reads: std::collections::BTreeSet<String>,
    string_annotations: bool,
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
        locals: names.locals.into_iter().collect(),
        globals: names.globals,
    }
}

impl ClosureNames {
    fn finish_scope(&mut self) {
        for name in self.globals.iter().chain(&self.nonlocals) {
            self.locals.shift_remove(name);
        }
        self.reads
            .retain(|name| !self.locals.contains(name) && !self.globals.contains(name));
        self.reads.extend(self.nonlocals.iter().cloned());
    }

    /// A nested function's external reads propagate through its enclosing function.
    fn child_reads(
        &mut self,
        parameters: Option<&ast::Parameters>,
        visit: impl FnOnce(&mut Self),
    ) -> std::collections::BTreeSet<String> {
        let mut child = Self {
            include_nested_reads: true,
            string_annotations: self.string_annotations,
            ..Self::default()
        };
        visit(&mut child);
        self.incomplete |= child.incomplete;
        if let Some(parameters) = parameters {
            child
                .locals
                .extend(parameters.iter().map(|p| p.name().to_string()));
        }
        child.finish_scope();
        child.reads
    }
}

impl<'a> Visitor<'a> for ClosureNames {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        match stmt {
            ast::Stmt::FunctionDef(f) => {
                self.locals.insert(f.name.to_string());
                if self.include_nested_reads {
                    // The child body and its lazy annotations see type parameters. Only
                    // the body sees the child's parameters and local bindings.
                    let mut reads =
                        self.child_reads(Some(&f.parameters), |child| child.visit_body(&f.body));
                    let mut annotations = if self.string_annotations {
                        Vec::new()
                    } else {
                        crate::frontend::parameter_annotations(&f.parameters, f.returns.as_deref())
                    };
                    if let Some(params) = &f.type_params {
                        for param in params.iter() {
                            if let ast::TypeParam::TypeVar(v) = param
                                && let Some(bound) = v.bound.as_deref()
                            {
                                annotations.push(("", bound));
                            }
                            if let Some(default) = param.default() {
                                annotations.push(("", default));
                            }
                        }
                    }
                    reads.extend(class_metadata::annotation_reads(&annotations, None, false));
                    if let Some(params) = &f.type_params {
                        for param in params.iter() {
                            reads.remove(param.name().as_str());
                        }
                    }
                    self.cell_reads.extend(reads.iter().cloned());
                    self.reads.extend(reads);
                } else {
                    self.incomplete = true;
                }
                // Decorator expressions run here and can bind names in this frame.
                for decorator in &f.decorator_list {
                    self.visit_decorator(decorator);
                }
                for param in f.parameters.iter_non_variadic_params() {
                    if let Some(default) = &param.default {
                        // Defaults run here. Their expression walk records enclosing bindings.
                        self.visit_expr(default);
                    }
                }
            }
            // Local variable annotations never evaluate in a function's scope.
            ast::Stmt::AnnAssign(a) if self.include_nested_reads => {
                // A bare parenthesized name with no value has no binding effect.
                if imre_ast::annotation_visits_target(a) {
                    self.visit_expr(&a.target);
                }
                if let Some(value) = &a.value {
                    self.visit_expr(value);
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
                if self.include_nested_reads {
                    let reads = self.child_reads(lambda.parameters.as_deref(), |child| {
                        child.visit_expr(&lambda.body)
                    });
                    self.cell_reads.extend(reads.iter().cloned());
                    self.reads.extend(reads);
                } else {
                    self.incomplete = true;
                }
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
                let (reads, bindings, captured) =
                    class_metadata::comprehension_scope(expr, self.string_annotations);
                self.reads.extend(reads);
                self.cell_reads.extend(captured);
                self.locals.extend(bindings);
                if !self.include_nested_reads {
                    self.incomplete = true;
                    self.locals_incomplete = true;
                }
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
            self.declared_globals.insert(name.to_owned());
            let name = self.code_name(name);
            self.op("declare_global", &[&self.scope.clone(), &name]);
        }
    }

    pub(super) fn statements(&mut self, suite: &[ast::Stmt]) {
        for stmt in suite {
            if self.terminated {
                if self.module_annotation_sites.is_none() {
                    break;
                }
                // Unreachable statements still contribute to a module provider's site order.
                let dead = self.name("b");
                self.label(&dead);
            }
            self.checkpoint(stmt);
            self.simple_call = imre_ast::unhighlighted_call(stmt);
            match stmt {
                ast::Stmt::Expr(v) => {
                    if !matches!(&*v.value, ast::Expr::StringLiteral(_)) {
                        let value = self.expr(&v.value);
                        self.line(&format!(
                            "call void @imre_dyn_discard_expression(ptr %ctx, i64 {value})"
                        ));
                    }
                }
                ast::Stmt::Pass(_) | ast::Stmt::Global(_) | ast::Stmt::Nonlocal(_) => {}
                ast::Stmt::TypeAlias(v)
                    if self.class_locals.is_none()
                        && let Some(name) = v.name.as_name_expr() =>
                {
                    let outer = self.scope.clone();
                    let params = if let Some(params) = v.type_params.as_deref() {
                        self.scope = self.parameter_scope(&outer, params, false);
                        let key = self.code_name(".type_params");
                        self.op("lookup", &[&self.scope.clone(), &key])
                    } else {
                        "0".into()
                    };
                    let evaluator = self.define(
                        v.start(),
                        DefinitionName::Plain(name.id.as_str()),
                        None,
                        None,
                        None,
                        Body::TypeAlias(&v.value),
                    );
                    let label = self.code_name(name.id.as_str());
                    let alias = self.op("type_alias", &[&label, &evaluator, &params]);
                    self.scope = outer;
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
                        let owned = chained
                            || matches!(
                                target,
                                ast::Expr::Name(_) | ast::Expr::List(_) | ast::Expr::Tuple(_)
                            );
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
                        self.assign_target(
                            &v.target,
                            &value,
                            matches!(&*v.target, ast::Expr::Name(_)),
                        );
                    } else {
                        match &*v.target {
                            ast::Expr::Attribute(attr) => {
                                self.discard_annotation_operand(&attr.value);
                            }
                            ast::Expr::Subscript(subscript) => {
                                self.discard_annotation_operand(&subscript.value);
                                self.discard_annotation_slice(&subscript.slice);
                            }
                            _ => {}
                        }
                    }
                    if let ast::Expr::Name(name) = &*v.target
                        && v.simple
                        && (self.module || self.class_locals.is_some())
                    {
                        let key = self.code_name(name.id.as_str());
                        if self.class_locals.is_some() && !self.source.string_annotations {
                            self.remember_annotation(v, name.id.as_str());
                            let offset = v.start().to_u32();
                            self.checked(&format!("call i64 @imre_dyn_mark_class_annotation(ptr %ctx, i64 {}, i64 {key}, i32 {offset})", self.scope));
                        } else if let Some(sites) = &mut self.module_annotation_sites {
                            let index = sites.len();
                            sites.push((name.id.to_string(), (*v.annotation).clone()));
                            let index = self.integer(index);
                            self.op("mark_module_annotation", &[&self.scope.clone(), &index]);
                        } else {
                            let value =
                                self.text(&crate::frontend::annotation_spelling(&v.annotation));
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
                    let generator = code_flags::is_generator(&v.body);
                    let body = match (v.is_async, generator) {
                        (false, false) => Body::Statements(&v.body),
                        (false, true) => Body::GeneratorFunction(&v.body, code_flags::GENERATOR),
                        (true, false) => Body::GeneratorFunction(&v.body, code_flags::COROUTINE),
                        (true, true) => {
                            Body::GeneratorFunction(&v.body, code_flags::ASYNC_GENERATOR)
                        }
                    };
                    // A method's lazy annotations also need its defining class namespace.
                    let type_params =
                        if self.class_locals.is_some() && !self.source.string_annotations {
                            None
                        } else {
                            crate::dynamic::boxed_function_type_params(
                                v,
                                self.source.string_annotations,
                                !self.prefix.is_empty(),
                                Some(&self.source.private_names),
                            )
                        };
                    let function = self.define(
                        v.decorator_list.first().map_or(v.start(), Ranged::start),
                        DefinitionName::Binding(&v.name),
                        Some(&v.parameters),
                        v.returns.as_deref(),
                        type_params,
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
                        DefinitionName::Binding(&v.name),
                        None,
                        None,
                        None,
                        Body::Namespace(&v.body, v.type_params.is_some()),
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
                    self.return_value(&value);
                }
                ast::Stmt::Match(v) => self.match_statement(v),
                ast::Stmt::If(v) => {
                    let done = self.name("b");
                    let mut next = self.if_arm(&v.test, &v.body, &done);
                    for clause in &v.elif_else_clauses {
                        if !next {
                            break;
                        }
                        if let Some(test) = &clause.test {
                            next = self.if_arm(test, &clause.body, &done);
                        } else {
                            self.statements(&clause.body);
                        }
                    }
                    self.jump(&done);
                    self.label(&done);
                }
                ast::Stmt::For(v) => {
                    let iter = self.iteration_cursor(&v.iter, v.is_async);
                    self.loop_body(&v.body, &v.orelse, Some(&iter), |f, yes, no| {
                        if f.native_suspension {
                            // A retained frame exposes the header after its final pull.
                            f.set_statement_line(f.source.line(v.start()));
                        }
                        f.enter_iteration(
                            &iter,
                            &v.target,
                            v.is_async.then(|| v.iter.range()),
                            !v.orelse.is_empty(),
                            yes,
                            no,
                        );
                    });
                }
                ast::Stmt::While(v) => {
                    if self.native_suspension
                        && matches!(&*v.test, ast::Expr::BooleanLiteral(lit) if !lit.value)
                    {
                        self.statements(&v.orelse);
                        continue;
                    }
                    self.loop_body(&v.body, &v.orelse, None, |f, yes, no| {
                        f.branch_expr(&v.test, yes, no);
                        f.label(yes);
                    });
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
                        let mut module = self.import(full, "0", 0);
                        let mut parts = full.split('.');
                        let root = parts.next().unwrap();
                        if alias.asname.is_some()
                            && !self.source.private_names.imports_root(&alias.name)
                        {
                            for part in parts {
                                let member = self.code_name(part);
                                module = self.op("from", &[&module, &member]);
                            }
                        }
                        let local = alias.asname.as_ref().map_or(root, ast::Identifier::as_str);
                        self.bind(local, &module);
                    }
                }
                ast::Stmt::ImportFrom(v) => {
                    let name = v.module.as_ref().map_or("", |v| v.as_str());
                    let fromlist = self.op("list", &[]);
                    let names: Vec<_> = v
                        .names
                        .iter()
                        .map(|alias| self.code_name(alias.name.as_str()))
                        .collect();
                    for attr in &names {
                        self.op("append", &[&fromlist, attr]);
                    }
                    let fromlist = self.op("tuple", &[&fromlist]);
                    let module = self.import(name, &fromlist, v.level);
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
                ast::Stmt::With(v) if !v.is_async || self.native_suspension => {
                    self.with(&v.items, &v.body, v.is_async);
                }
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

    fn import(&mut self, name: &str, fromlist: &str, level: u32) -> String {
        let name = self.code_name(name);
        self.checked(&format!(
            "call i64 @imre_dyn_import_name(ptr %ctx, i64 {}, i64 {name}, i64 {fromlist}, i32 {level})",
            self.scope
        ))
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
            let name = self.code_name(self.source.private_names.definition(param.name()));
            let declaration = match param {
                ast::TypeParam::TypeVar(_) => name,
                ast::TypeParam::TypeVarTuple(_) => self.op("class_typevartuple", &[&name]),
                ast::TypeParam::ParamSpec(_) => self.op("class_paramspec", &[&name]),
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
        name: DefinitionName<'_>,
        params: Option<&ast::Parameters>,
        returns: Option<&ast::Expr>,
        type_params: Option<&ast::TypeParams>,
        mut body: Body<'_>,
    ) -> String {
        let (name, global) = match name {
            DefinitionName::Binding(identifier) => (
                self.source.private_names.definition(identifier),
                self.declared_globals.contains(identifier.as_str()),
            ),
            DefinitionName::Plain(name) => (name, false),
        };
        let qualname = if global {
            name.to_owned()
        } else {
            format!("{}{name}", self.prefix)
        };
        let prefix = if matches!(body, Body::Namespace(..)) {
            format!("{qualname}.")
        } else {
            format!("{qualname}.<locals>.")
        };
        let statements = match &body {
            Body::Statements(stmts) | Body::GeneratorFunction(stmts, _) => Some(*stmts),
            _ => None,
        };
        let doc = statements
            .filter(|_| self.output.keep_docstrings)
            .and_then(|stmts| self.source.docstring(&qualname, stmts));
        let flags = match &body {
            Body::Namespace(..) => None,
            _ => {
                use imre_ast::code_flags::*;
                let mut flags = function_flags(
                    params,
                    type_params.is_some() || self.in_function,
                    type_params.is_none()
                        && !self.prefix.is_empty()
                        && !self.prefix.ends_with(".<locals>."),
                );
                if self.source.string_annotations {
                    flags |= FUTURE_ANNOTATIONS;
                }
                if self.source.barry_as_flufl {
                    flags |= FUTURE_BARRY_AS_BDFL;
                }
                if doc.is_some() {
                    flags |= HAS_DOCSTRING;
                }
                Some(flags | body.generator_flags())
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
            if type_params.is_some() || self.in_function {
                annotation_flags |= NESTED;
            }
            if self.source.string_annotations {
                annotation_flags |= FUTURE_ANNOTATIONS;
            }
            if self.source.barry_as_flufl {
                annotation_flags |= FUTURE_BARRY_AS_BDFL;
            }
            let mut metadata = json!({
                "posonly": ["format"],
                "code_flags": annotation_flags,
                "scope_reads": reads,
                "scope_complete": true,
                "scope_locals_complete": true,
                "annotation_class": class.is_some(),
            });
            if !self.source.python_annotations && type_params.is_none() {
                for (slot, expr) in &entries {
                    if let Some(reference) = scalar_reference(expr) {
                        let root = reference.split('.').next().unwrap();
                        if class
                            .as_ref()
                            .is_some_and(|class| class.globals.contains(root))
                        {
                            continue;
                        }
                        metadata["annotation_references"][slot] = json!(reference);
                    }
                }
            }
            let sig = self.text(&metadata.to_string());
            let code = self.output.definition(
                self.source,
                String::new(),
                Body::Annotations(entries, class),
                params.map_or(start, Ranged::start),
                false,
                Entry::Compiled,
            );
            let aname = self.text("__annotate__");
            let aqname = self.text(&format!("{qualname}.__annotate__"));
            self.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {}, i64 {aname}, i64 {sig}, i64 0, i64 {aqname}, i64 0, i64 0, i1 true)", scope))
        };
        let qualname = self.text(&qualname);
        let mut signature =
            json!({"posonly":[],"positional":[],"kwonly":[],"varargs":null,"kwargs":null});
        if let Some(flags) = flags {
            signature["code_flags"] = json!(flags);
        }
        if matches!(body, Body::YieldingLambda(_) | Body::Expression(_))
            && flags.is_some_and(|flags| flags & code_flags::GENERATOR != 0)
        {
            signature["generator_lambda"] = json!(true);
        }
        if matches!(body, Body::Generator(..)) {
            // generator() binds the implicit .0 argument without calling the step function.
            signature["positional"] = json!([".0"]);
        }
        if let Body::Namespace(stmts, _) = &body {
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
        let class_cell = match &body {
            Body::Statements(body) | Body::GeneratorFunction(body, _) => Some(
                imre_ast::class_cell::function_needed(body, params, self.source.string_annotations),
            ),
            Body::Expression(body) | Body::YieldingLambda(body) => {
                Some(imre_ast::class_cell::expression_needed(
                    body,
                    params,
                    self.source.string_annotations,
                ))
            }
            _ => None,
        };
        if let Some(captures_class) = class_cell {
            if captures_class {
                signature["class_cell"] = json!(true);
            }
            if self.class_locals.is_some() && !self.annotation_lookup && type_params.is_none() {
                signature["method_class_cell"] = json!(captures_class);
            }
        }
        // Keep the existing staged generator when its suite cannot be lowered. Its
        // surrounding caller and any neighboring supported suites remain compiled.
        let trial = match &body {
            Body::GeneratorFunction(stmts, code_flags::GENERATOR) if !self.source.bundled => {
                Some(Body::GeneratorFunction(stmts, code_flags::GENERATOR))
            }
            Body::Generator(expr, plan, _) if !self.source.bundled && plan.outer_async => {
                Some(Body::Generator(expr, plan, Default::default()))
            }
            _ => None,
        };
        let native_generator = trial.and_then(|suite| {
            self.output.try_suspended(|output| {
                output.definition(
                    self.source,
                    prefix.clone(),
                    suite,
                    start,
                    true,
                    Entry::Suspended,
                )
            })
        });
        // Synchronous genexprs already have a compiled comprehension step.
        let entry = if native_generator.is_some()
            || (flags.is_some_and(|flags| {
                if self.source.bundled {
                    flags & code_flags::GENERATOR != 0
                } else {
                    flags & code_flags::COROUTINE != 0
                }
            }) && !matches!(body, Body::Generator(..)))
        {
            Entry::Suspended
        } else {
            match &body {
                Body::GeneratorFunction(stmts, flag) => {
                    let first = stmts[0].start();
                    let start = first.to_usize();
                    let end = stmts.last().unwrap().end().to_usize();
                    let indent =
                        imre_ast::indentation::generator_prefix(&self.source.text[..start]);
                    // Slice the suite as written so indentation inside multiline strings survives.
                    let header = if *flag == code_flags::GENERATOR {
                        "def"
                    } else {
                        "async def"
                    };
                    let text = format!("{header} _():\n{indent}{}", &self.source.text[start..end]);
                    Some((first, indent.len(), text))
                }
                Body::Generator(expr, plan, _) if plan.body_suspends() => {
                    // The same resumable interpreter that runs async def bodies
                    // consumes this expression's canonical plan, with .0 supplied
                    // by the caller when the generator is created.
                    signature["generator_expression"] = json!(true);
                    let text = &self.source.text[expr.start().to_usize()..expr.end().to_usize()];
                    Some((
                        expr.start(),
                        " return (".len(),
                        format!("async def _():\n return ({text})"),
                    ))
                }
                Body::YieldingLambda(expr) => {
                    let text = &self.source.text[expr.start().to_usize()..expr.end().to_usize()];
                    Some((
                        expr.start(),
                        " return (".len(),
                        format!("def _():\n return ({text})"),
                    ))
                }
                _ => None,
            }
            .map_or(Entry::Compiled, |(first, prefix, text)| {
                let (line, column) = self.source.line_and_column(first);
                signature["body_column_delta"] = json!(column as i64 - prefix as i64);
                if !self.source.method_imports.is_empty() {
                    signature["method_imports"] = json!(self.source.method_imports);
                }
                if let Some(owner) = self.source.private_names.suite_owner(first) {
                    signature["private_scope"] = json!([owner, self.source.string_annotations]);
                }
                Entry::Source(self.output.data(text.as_bytes()), line)
            })
        };
        let mut closure_names = ClosureNames {
            string_annotations: self.source.string_annotations,
            include_nested_reads: matches!(
                &body,
                Body::Statements(_) | Body::GeneratorFunction(..)
            ),
            ..ClosureNames::default()
        };
        match &mut body {
            Body::Statements(stmts) | Body::GeneratorFunction(stmts, _) => {
                closure_names.visit_body(stmts)
            }
            Body::Namespace(stmts, generic) => {
                let mut bindings = ClosureNames::default();
                bindings.visit_body(stmts);
                let reads =
                    class_metadata::capture(&mut bindings, stmts, self.source.string_annotations);
                signature["class_frame_reads"] = json!(reads);
                if !bindings.globals.is_empty() {
                    signature["class_globals"] = json!(bindings.globals);
                }
                closure_names.nonlocals = bindings.nonlocals;
                if !*generic
                    && let Some(reads) =
                        class_metadata::method_class_reads(stmts, self.source.string_annotations)
                {
                    closure_names.reads = reads;
                } else {
                    closure_names.incomplete = true;
                }
            }
            Body::Expression(expr) | Body::YieldingLambda(expr) | Body::TypeAlias(expr) => {
                closure_names.visit_expr(expr)
            }
            Body::Generator(_, plan, names) => {
                closure_names = class_metadata::generator_scope(
                    plan,
                    std::mem::take(names),
                    self.source.string_annotations,
                );
                if self.class_locals.is_some() && !self.annotation_lookup {
                    signature["method_class_cell"] =
                        json!(closure_names.reads.contains("__class__"));
                }
            }
            _ => closure_names.incomplete = true,
        }
        if let (Body::Statements(stmts), Some(params), None) = (&body, params, returns) {
            if let Some((names, lock_only)) = worker::parameters(stmts, params) {
                signature["thread_parameters"] = json!(names);
                if lock_only {
                    signature["thread_worker_locks"] = json!(names);
                }
            } else if let Some(names) = imre_ast::captured_thread_locks(stmts, params) {
                signature["thread_captured_locks"] = json!(names);
            } else if let Some(field) = worker::bound_lock_field(stmts, params) {
                signature["thread_bound_lock_field"] = json!(field);
            }
        }
        // A type alias thunk has an implicit default for its format parameter.
        let defaults = if matches!(body, Body::TypeAlias(_))
            || params.is_some_and(|p| {
                p.iter_non_variadic_params()
                    .any(|arg| arg.default.is_some())
            }) {
            self.op("dict", &[])
        } else {
            "0".into()
        };
        if matches!(body, Body::TypeAlias(_)) {
            signature["posonly"] = json!([".format"]);
            let format = self.text(".format");
            let value_format = self.integer(1);
            self.op("put", &[&defaults, &format, &value_format]);
        }
        let in_function = self.in_function || !matches!(body, Body::Namespace(..));
        signature["types"] = json!({});
        // A generator call returns a suspended frame, before it yields or returns a value.
        let scalar = |expr: &ast::Expr| {
            if self.source.python_annotations {
                return None;
            }
            scalar_reference(expr).filter(|name| {
                !type_params.is_some_and(|params| params.iter().any(|p| p.name().as_str() == name))
            })
        };
        if matches!(entry, Entry::Compiled) {
            signature["return_type"] = json!(returns.and_then(scalar));
        }
        let code = native_generator.unwrap_or_else(|| {
            self.output
                .definition(self.source, prefix, body, start, in_function, entry)
        });
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
                        if ty == "float"
                            && statements
                                .is_some_and(|body| crate::frontend::only_compares_zero(body, name))
                        {
                            signature["integer_sign_parameters"][name] = json!(true);
                        }
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
        closure_names.finish_scope();
        closure_names
            .cell_reads
            .retain(|name| closure_names.locals.contains(name));
        // An emitted but unresolved layout still identifies a cleared compiled frame.
        signature["scope_cellvars"] = if closure_names.incomplete {
            serde_json::Value::Null
        } else {
            json!(closure_names.cell_reads)
        };
        signature["scope_locals"] = json!(closure_names.locals.iter().collect::<Vec<_>>());
        signature["scope_reads"] = json!(closure_names.reads);
        signature["scope_nonlocals"] = json!(closure_names.nonlocals);
        signature["scope_complete"] = json!(!closure_names.incomplete);
        signature["scope_locals_complete"] = json!(!closure_names.locals_incomplete);
        let sig = self.text(&signature.to_string());
        let name = self.code_name(name);
        let function = self.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {}, i64 {name}, i64 {sig}, i64 {defaults}, i64 {qualname}, i64 {doc}, i64 {annotate}, i1 false)", scope));
        if type_params.is_some() {
            let key = self.code_name(".type_params");
            let params = self.op("lookup", &[&scope, &key]);
            let key = self.code_name("__type_params__");
            self.op("setattr", &[&function, &key, &params]);
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

    /// Publish a module's provider before its first statement. Every simple
    /// annotation, including an unconditional one, becomes visible only after
    /// execution reaches its assignment.
    pub(super) fn setup_deferred_module_annotations(&mut self, sites: &[(String, ast::Expr)]) {
        if !sites.is_empty() {
            let entries = sites
                .iter()
                .map(|(name, expr)| (name.as_str(), expr))
                .collect();
            let signature = self.text(
                &json!({
                    "posonly": ["format"], "scope_complete": true, "scope_locals_complete": true,
                })
                .to_string(),
            );
            let code = self.output.definition(
                self.source,
                String::new(),
                Body::ModuleAnnotations(entries),
                self.source.module_start,
                false,
                Entry::Compiled,
            );
            let name = self.text("__annotate__");
            let provider = self.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {}, i64 {name}, i64 {signature}, i64 0, i64 {name}, i64 0, i64 0, i1 true)", self.scope));
            self.bind("__annotate__", &provider);
        }
        self.op("setup_module_annotations", &[&self.scope.clone()]);
    }

    pub(super) fn annotations(
        &mut self,
        entries: &[(&str, &ast::Expr)],
        class: Option<ClassBindings>,
        module: bool,
    ) {
        self.check_annotation_format();
        let values = self.op("dict", &[]);
        // The public format parameter is outside the annotation expressions' lexical scope.
        self.scope = self.op("annotation_scope", &[&self.scope.clone()]);
        self.annotation_lookup = true;
        self.class_locals = class;
        for (index, &(name, expression)) in entries.iter().enumerate() {
            let after = module.then(|| {
                let active = self.lookup("__conditional_annotations__");
                let index = self.integer(index);
                let present = self.op("in", &[&index, &active]);
                let condition = self.truth(&present);
                let yes = self.name("b");
                let after = self.name("b");
                self.branch(&condition, &yes, &after);
                self.label(&yes);
                after
            });
            let key = self.code_name(name);
            let value = if self.source.string_annotations {
                self.text(&crate::frontend::annotation_spelling(expression))
            } else if let ast::Expr::Starred(starred) = expression {
                let value = self.expr(&starred.value);
                let count = self.integer(1);
                let items = self.op("unpack", &[&value, &count, "0"]);
                let index = self.integer(0);
                self.op("get", &[&items, &index])
            } else {
                self.expr(expression)
            };
            self.op("put", &[&values, &key, &value]);
            if let Some(after) = after {
                self.jump(&after);
                self.label(&after);
            }
        }
        self.end(&format!("ret i64 {values}"));
    }

    fn decorated(&mut self, name: &ast::Identifier, decorators: &[String], mut value: String) {
        for decorator in decorators.iter().rev() {
            value = self.invoke(decorator, &[&value]);
        }
        self.bind(name.as_str(), &value);
    }

    fn discard_annotation_operand(&mut self, expr: &ast::Expr) {
        let mark = self.temporary_mark();
        self.expr(expr);
        self.discard_truth(&mark);
    }

    // Without an assignment, CPython evaluates each subscript operand but
    // neither constructs an extended slice nor invokes __getitem__.
    fn discard_annotation_slice(&mut self, slice: &ast::Expr) {
        match slice {
            ast::Expr::Slice(s) => {
                for bound in [&s.lower, &s.upper, &s.step].into_iter().flatten() {
                    self.discard_annotation_operand(bound);
                }
            }
            ast::Expr::Tuple(t) => {
                for item in &t.elts {
                    self.discard_annotation_slice(item);
                }
            }
            expr => self.discard_annotation_operand(expr),
        }
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
        if !owned && let Some([left, right]) = Self::pair_target(target) {
            self.max_live = self.max_live.max(2);
            self.checked(&format!(
                "call i64 @imre_dyn_unpack_pair(ptr %ctx, i64 {value}, ptr %live)"
            ));
            self.assign_pair_slots(left, right);
            return;
        }
        let count = self.integer(elements.len());
        let star = elements
            .iter()
            .position(|v| matches!(v, ast::Expr::Starred(_)))
            .map_or_else(|| "0".into(), |i| self.integer(i));
        let operation = if owned { "unpack" } else { "unpack_read" };
        let sequence = self.op(operation, &[value, &count, &star]);
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

    /// Enter the loop body with its target bound, or branch to the exhaustion successor.
    pub(super) fn enter_iteration(
        &mut self,
        iter: &str,
        target: &ast::Expr,
        async_range: Option<ruff_text_size::TextRange>,
        has_else: bool,
        body: &str,
        exhausted: &str,
    ) {
        if let Some(range) = async_range {
            let item = self.async_pull(iter, range, body, exhausted);
            self.label(body);
            self.assign(target, &item);
            return;
        }
        let more = if Self::pair_target(target).is_some() {
            self.max_live = self.max_live.max(2);
            self.checked(&format!(
                "call i64 @imre_dyn_next_pair(ptr %ctx, i64 {iter}, ptr %live, i1 {has_else})"
            ))
        } else {
            self.op(if has_else { "more_else" } else { "more" }, &[iter])
        };
        let ready = self.value(&format!("icmp eq i64 {more}, 2"));
        self.branch(&ready, body, exhausted);
        self.label(body);
        if let Some([left, right]) = Self::pair_target(target) {
            self.assign_pair_slots(left, right);
        } else {
            let item = self.op("next", &[iter]);
            self.assign(target, &item);
        }
    }

    /// Only the implicit __anext__ call and its await can exhaust an async for.
    fn async_pull(
        &mut self,
        iter: &str,
        range: ruff_text_size::TextRange,
        body: &str,
        exhausted: &str,
    ) -> String {
        let handler = self.name("b");
        let outer = std::mem::replace(&mut self.error, handler.clone());
        let mark = self.temporary_mark();
        let next = self.op("async_next_iter", &[iter]);
        let item = self.suspend_at(&next, super::suspension::Suspension::Await, range, &mark);
        self.flush_check();
        self.error = outer;
        self.jump(body);
        self.label(&handler);
        self.discard_truth(&mark);
        let error = self.op("take_error", &[]);
        let name = self.text("StopAsyncIteration");
        let class = self.op("builtin", &[&name]);
        let matches = self.op("matches", &[&error, &class]);
        let stop = self.value(&format!("icmp eq i64 {matches}, 2"));
        let propagate = self.name("b");
        self.branch(&stop, exhausted, &propagate);
        self.label(&propagate);
        self.raise(&error, None, true);
        item
    }

    fn pair_target(target: &ast::Expr) -> Option<[&ast::Expr; 2]> {
        let elements = match target {
            ast::Expr::Tuple(t) => &t.elts,
            ast::Expr::List(l) => &l.elts,
            _ => return None,
        };
        let [left, right] = elements.as_slice() else {
            return None;
        };
        (!left.is_starred_expr() && !right.is_starred_expr()).then_some([left, right])
    }

    fn assign_pair_slots(&mut self, left: &ast::Expr, right: &ast::Expr) {
        // Target expressions may make calls that reuse the scratch slots.
        let first = self.value("load i64, ptr %live");
        let second_slot = self.value("getelementptr i64, ptr %live, i64 1");
        let second = self.value(&format!("load i64, ptr {second_slot}"));
        self.assign(left, &first);
        self.assign(right, &second);
    }

    /// Return whether control can reach a subsequent arm.
    fn if_arm(&mut self, test: &ast::Expr, body: &[ast::Stmt], done: &str) -> bool {
        if self.native_suspension
            && let ast::Expr::BooleanLiteral(lit) = test
        {
            if lit.value {
                self.statements(body);
                self.jump(done);
            }
            return !lit.value;
        }
        let body_label = self.name("b");
        let next = self.name("b");
        self.branch_expr(test, &body_label, &next);
        self.label(&body_label);
        self.statements(body);
        self.jump(done);
        self.label(&next);
        true
    }

    fn loop_body(
        &mut self,
        body: &[ast::Stmt],
        otherwise: &[ast::Stmt],
        iterator: Option<&str>,
        enter_body: impl FnOnce(&mut Self, &str, &str),
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
        enter_body(self, &body_label, &exhausted);
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
        if !self.terminated || self.module_annotation_sites.is_some() {
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
                Cleanup::With(context) => {
                    if context.is_async {
                        let slot = self
                            .live
                            .iter()
                            .rposition(|v| v == &context.callable)
                            .unwrap();
                        discarded.push((slot, self.live.remove(slot)));
                    }
                    self.exit_context(context, None);
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

    fn set_statement_line(&mut self, line: usize) {
        if !self.output.declarations.declared_statement_line {
            self.output
                .functions
                .push_str("declare void @imre_dyn_set_statement_line(ptr, i32)\n");
            self.output.declarations.declared_statement_line = true;
        }
        self.line(&format!(
            "call void @imre_dyn_set_statement_line(ptr %ctx, i32 {line})"
        ));
    }

    fn exit_context(&mut self, context: &ContextExit, exception: Option<&str>) -> String {
        let args = [
            context.callable.as_str(),
            exception.unwrap_or("0"),
            context.caught.as_str(),
        ];
        if !context.is_async {
            self.set_statement_line(self.source.line(context.range.start()));
            return self.op("exit_context", &args);
        }

        // A pending return, outer managers and the body exception survive this await.
        // The exceptional exit also retains its callable through the truth test.
        let outer_roots = self.live.len();
        self.live.extend(exception.map(String::from));
        self.live.push(context.callable.clone());
        self.caught
            .push(exception.unwrap_or(&context.caught).into());
        self.checkpoint(&context.range);
        self.caught.pop();
        self.live.truncate(outer_roots);
        self.suspension_position(context.range);
        let awaitable = self.op("async_exit_context", &args);
        let mark = if exception.is_some() {
            outer_roots + 2
        } else {
            outer_roots
        }
        .to_string();
        self.keep_operands(&mark, &awaitable, "0");
        let result = self.await_at(&awaitable, "2", context.range, &mark);
        if let Some(exc) = exception {
            self.suspension_position(context.range);
            let suppress = self.op("truth", &[&result]);
            self.keep_operands(&outer_roots.to_string(), exc, &suppress);
            self.op("restore_caught", &[&context.caught]);
            suppress
        } else {
            self.discard_truth(&mark);
            "0".into()
        }
    }

    fn with(&mut self, items: &[ast::WithItem], body: &[ast::Stmt], is_async: bool) {
        let Some((item, rest)) = items.split_first() else {
            self.statements(body);
            return;
        };
        let range = item.context_expr.range();
        let acquire = |f: &mut Self, mark: Option<&str>| {
            let manager = f.expr(&item.context_expr);
            let mode = if is_async { "2" } else { "0" };
            let lookup = |f: &mut Self, name| {
                if is_async {
                    f.suspension_position(range);
                }
                f.op(name, &[&manager, mode])
            };
            let exit = lookup(f, "lookup_exit");
            let enter = lookup(f, "lookup_enter");
            let above_exit = mark.map(|mark| f.keep_operands(mark, &exit, &enter));
            if is_async {
                f.suspension_position(range);
            }
            let mut value = f.invoke(&enter, &[]);
            if let Some(mark) = &above_exit {
                f.keep_operands(mark, &value, "0");
                value = f.await_at(&value, "1", range, mark);
            }
            (exit, value, above_exit)
        };
        let (exit, value, above_exit) = if is_async {
            let mark = self.temporary_mark();
            self.unwind_operands(&mark, |f| acquire(f, Some(&mark)))
        } else {
            acquire(self, None)
        };
        let handler = self.name("b");
        let done = self.name("b");
        let outer = std::mem::replace(&mut self.error, handler.clone());
        let caught = self.caught.last().cloned().unwrap_or_else(|| "0".into());
        let context = ContextExit {
            range,
            is_async,
            callable: exit.clone(),
            caught,
        };
        self.exits
            .push((Cleanup::With(context.clone()), outer.clone()));
        self.live.push(exit);
        if let Some(target) = &item.optional_vars {
            self.assign(target, &value);
        }
        if let Some(mark) = above_exit {
            // The assignment consumed the enter result before the next item is evaluated.
            self.discard_truth(&mark);
        }
        self.with(rest, body, is_async);
        self.live.pop();
        self.exits.pop();
        self.error = outer;
        if !self.terminated {
            self.exit_context(&context, None);
            self.jump(&done);
        }
        self.label(&handler);
        let exc = self.op("take_error", &[]);
        let result = self.exit_context(&context, Some(&exc));
        let suppress = self.truth(&result);
        let propagate = self.name("b");
        self.branch(&suppress, &done, &propagate);
        self.label(&propagate);
        self.raise(&exc, None, true);
        self.label(&done);
    }
}

/// Names that can refer directly to a type without calling Python.
fn scalar_reference(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::NoneLiteral(_) => Some("None".to_owned()),
        ast::Expr::Name(name) if !name.id.starts_with("__") => Some(name.id.to_string()),
        ast::Expr::Attribute(attr) => {
            let module = attr.value.as_name_expr()?;
            (!module.id.starts_with("__") && !attr.attr.starts_with("__"))
                .then(|| format!("{}.{}", module.id, attr.attr))
        }
        _ => None,
    }
}
