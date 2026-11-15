//! Admit executable filenames against the same physical bodies the linker registers.
use super::{Session, is_stdlib_function_type};
use crate::frontend::phase::Diagnostic;
use ruff_python_ast::{
    self as ast,
    visitor::{self, Visitor},
};
use ruff_text_size::{Ranged, TextRange};
use std::path::Path;
use ty_python_semantic::{Db, HasType, SemanticModel, types::Type};

const REWRITE: &str = "Select a compiled callback with handlers[key](), call runpy.run_path with the literal absolute filename in that callback, and declare the file with --include-module.";

impl Session {
    pub(super) fn check_run_path_callers<'a>(
        &self,
        sources: impl Iterator<Item = (usize, crate::dynamic::RegisteredSource<'a>)> + Clone,
        has_file_body: Option<&dyn Fn(&str) -> bool>,
    ) -> Result<(), String> {
        let Some(has_file_body) = has_file_body else {
            return Ok(());
        };
        // A script named runpy.py executes as __main__. When the linker selects the SDK
        // for 'import runpy', its filename must not shadow the imported declaration in ty.
        let shadow = sources.clone().find_map(|(id, source)| {
            if source.name != "__main__" || source.path.file_name()? != "runpy.py" {
                return None;
            }
            let file = self.files[id];
            let model = SemanticModel::new(&self.db, self.db.program_file(file));
            (model.resolve_module(Some("runpy"), 0)?.file(&self.db) == Some(file)).then_some(file)
        });
        let copy = shadow
            .map(|f| self.copy_for_diagnostics(Some(f), None))
            .transpose()?;
        let session = copy.as_ref().unwrap_or(self);
        for (id, source) in sources.filter(|(_, source)| source.check_execution) {
            session.check_run_path(id, source.path, has_file_body)?;
        }
        Ok(())
    }

    fn check_run_path(
        &self,
        module: usize,
        path: &Path,
        has_file_body: &dyn Fn(&str) -> bool,
    ) -> Result<(), String> {
        let file = self.files[module];
        let program = self.db.program_file(file);
        let model = SemanticModel::new(&self.db, program);
        let index = ty_python_core::semantic_index(&self.db, program);
        // Only ask for expression types when an import or the builtin namespace
        // can supply the vendored runpy module or one of its functions.
        if !index.scope_ids().any(|scope| {
            let scope = scope.file_scope_id(&self.db);
            let places = index.place_table(scope);
            places.symbol_by_name("__import__").is_some()
                || places.symbol_by_name("__builtins__").is_some()
                || index
                    .use_def_map(scope)
                    .definitions_with_usage()
                    .any(|(_, definition, _)| definition.kind(&self.db).is_import())
        }) && model.resolve_module(Some("__builtins__"), 0).is_none()
            && model.resolve_module(Some("builtins"), 0).is_some_and(|m| {
                m.file(&self.db)
                    .is_some_and(|f| f.path(&self.db).is_vendored_path())
            })
        {
            return Ok(());
        }
        let parsed = self.parsed(file);
        let mut check = Calls {
            model,
            has_file_body,
            replaced: false,
            calls: Vec::new(),
            escapes: Vec::new(),
        };
        check.visit_body(&parsed.syntax().body);
        if check.calls.is_empty() && check.escapes.is_empty() {
            return Ok(());
        }
        let text = ruff_db::source::source_text(&self.db, file);
        let locations = check
            .calls
            .into_iter()
            .chain(check.escapes)
            .map(|(range, reason)| {
                Diagnostic::at(text.as_str(), &range, format!("{reason}."))
                    .with_path(path)
                    .to_string()
            })
            .collect::<Vec<_>>();
        Err(format!("{} {REWRITE}", locations.join("\n")))
    }
}

struct Calls<'a> {
    model: SemanticModel<'a>,
    has_file_body: &'a dyn Fn(&str) -> bool,
    // A proven write to the module export in this straight-line suite.
    replaced: bool,
    calls: Vec<(TextRange, &'static str)>,
    escapes: Vec<(TextRange, &'static str)>,
}

#[derive(PartialEq)]
enum Selection<T = ()> {
    Other,
    RunPath(T),
    Ambiguous,
}

impl Calls<'_> {
    fn module(&self, expr: &ast::Expr, name: &str) -> bool {
        let db = self.model.db();
        matches!(expr.inferred_type(&self.model), Some(Type::ModuleLiteral(m))
            if m.module(db).name(db).as_str() == name
            && m.module(db).file(db).is_some_and(|f| f.path(db).is_vendored_path()))
    }

    fn export(&self, expr: &ast::Expr) -> bool {
        matches!(expr, ast::Expr::Attribute(a) if a.attr.as_str() == "run_path" && self.module(&a.value, "runpy"))
    }

    fn cached_module(&self, expr: &ast::Expr) -> bool {
        matches!(expr, ast::Expr::Subscript(s) if s.ctx == ast::ExprContext::Load
            && matches!(&*s.value, ast::Expr::Attribute(a) if a.attr.as_str() == "modules" && self.module(&a.value, "sys"))
            && matches!(&*s.slice, ast::Expr::StringLiteral(k) if k.value.to_str() == "runpy"))
    }

    /// Inspect effects of obtaining a reference whose value is consumed locally.
    fn read_reference(&mut self, expr: &ast::Expr) {
        if let ast::Expr::Attribute(a) = expr
            && a.value.is_name_expr()
            && self.module(&a.value, "runpy")
        {
            return;
        }
        visitor::walk_expr(self, expr);
    }

    fn replacement(&mut self, value: &ast::Expr) {
        self.visit_expr(value);
        self.replaced = (value.is_name_expr() || value.is_attribute_expr())
            && matches!(
                value.inferred_type(&self.model),
                Some(Type::FunctionLiteral(_))
            )
            && self.selection(value) == Selection::Other;
    }

    fn selection(&self, expr: &ast::Expr) -> Selection {
        self.select(expr, |_| ())
    }

    fn select<T>(
        &self,
        expr: &ast::Expr,
        selected: impl FnOnce(&ast::StmtFunctionDef) -> T,
    ) -> Selection<T> {
        let genuine = |ty| is_stdlib_function_type(&self.model, ty, "runpy", "run_path");
        match expr.inferred_type(&self.model) {
            Some(Type::Union(union))
                if union.elements(self.model.db()).iter().copied().any(genuine) =>
            {
                Selection::Ambiguous
            }
            Some(ty) => super::select_stdlib_function(&self.model, ty, "runpy", |function| {
                (function.name.as_str() == "run_path")
                    .then(|| Selection::RunPath(selected(function)))
            })
            .unwrap_or(Selection::Other),
            _ => Selection::Other,
        }
    }

    fn call(&mut self, call: &ast::ExprCall) {
        if self.replaced && self.export(&call.func) {
            return;
        }
        match self.select(&call.func, |declaration| {
            self.filename_error(call, declaration)
        }) {
            Selection::Other => {
                self.read_operand(&call.func, Some(Selection::Other));
            }
            Selection::Ambiguous => {
                self.calls.push((
                    call.func.range(),
                    "cannot prove whether the selected callable is runpy.run_path",
                ));
            }
            Selection::RunPath(error) => self.calls.extend(error),
        }
    }

    fn filename_error(
        &self,
        call: &ast::ExprCall,
        declaration: &ast::StmtFunctionDef,
    ) -> Option<(TextRange, &'static str)> {
        let invalid = || {
            (
                call.range(),
                "cannot bind the executable filename of runpy.run_path",
            )
        };
        let check = |arg: ast::ArgOrKeyword<'_>| {
            let reason = match arg.value() {
                ast::Expr::StringLiteral(s) if !arg.is_variadic() => {
                    if (self.has_file_body)(s.value.to_str()) {
                        return None;
                    }
                    "runpy.run_path requires a declared file at this exact filename"
                }
                _ => "runpy.run_path has an open executable filename",
            };
            Some((arg.value().range(), reason))
        };
        let args = &call.arguments;
        if !args.iter_source_order().any(|arg| arg.is_variadic()) {
            let keys = args.keywords.iter().map(|kw| kw.arg.as_deref().unwrap());
            let binding = match crate::frontend::bind_source(
                declaration,
                declaration.name.as_str(),
                args.args.len(),
                keys,
            ) {
                Ok(binding) => binding,
                Err(imre_ast::source_arguments::Error::Memory) => return Some(invalid()),
                // Python binding fails before entering this function.
                Err(_) => return None,
            };
            if let Some(first) = args.args.first() {
                return check(first.into());
            }
            return binding
                .keywords
                .iter()
                .position(|slot| *slot == Some(0))
                .and_then(|i| check((&args.keywords[i]).into()));
        }
        // Unpacked operands need ty's possible parameter matches.
        let Some(signature) =
            ty_python_semantic::types::ide_support::resolved_call_signature(&self.model, call)
        else {
            return Some(invalid());
        };
        args.iter_source_order()
            .zip(signature.argument_to_parameter_mapping)
            .find_map(|(arg, binding)| {
                binding
                    .parameters
                    .iter()
                    .any(|p| p.index == 0)
                    .then(|| check(arg))
                    .flatten()
            })
    }
}

impl<'ast> Visitor<'ast> for Calls<'_> {
    fn visit_body(&mut self, body: &'ast [ast::Stmt]) {
        self.replaced = false;
        for stmt in body {
            self.visit_stmt(stmt);
            if matches!(
                stmt,
                ast::Stmt::Return(_)
                    | ast::Stmt::Raise(_)
                    | ast::Stmt::Break(_)
                    | ast::Stmt::Continue(_)
            ) {
                break;
            }
        }
        self.replaced = false;
    }

    fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
        if let ast::Stmt::Assign(a) = stmt
            && let [target] = a.targets.as_slice()
        {
            if self.export(target) {
                self.replacement(&a.value);
                return;
            }
            if target.is_name_expr()
                && (a.value.is_name_expr() || a.value.is_attribute_expr())
                && (self.selection(&a.value) == Selection::RunPath(())
                    || self.module(&a.value, "runpy"))
            {
                // ty resolves the alias where it is called or handed to another function.
                self.read_reference(&a.value);
                self.replaced = false;
                return;
            }
            if target.is_name_expr() && a.value.is_literal_expr() {
                return;
            }
        }
        if let ast::Stmt::Expr(s) = stmt
            && let ast::Expr::Call(call) = &*s.value
            && let [owner, ast::Expr::StringLiteral(name), value] = call.arguments.args.as_ref()
            && name.value.to_str() == "run_path"
            && call.arguments.keywords.is_empty()
            && self.module(owner, "runpy")
            && super::resolves_to_stdlib_function(&self.model, &call.func, "builtins", "setattr")
        {
            self.replacement(value);
            return;
        }
        if !matches!(
            stmt,
            ast::Stmt::Expr(_) | ast::Stmt::Assign(_) | ast::Stmt::Return(_)
        ) {
            self.replaced = false;
        }
        visitor::walk_stmt(self, stmt);
        self.replaced = false;
    }

    fn visit_expr(&mut self, expr: &'ast ast::Expr) {
        self.read_operand(expr, None);
    }
}

impl Calls<'_> {
    fn read_operand(&mut self, expr: &ast::Expr, selected: Option<Selection>) {
        if let ast::Expr::Attribute(a) = expr
            && a.ctx == ast::ExprContext::Load
            && (self.cached_module(&a.value) || self.selection(&a.value) != Selection::Other)
            && expr.inferred_type(&self.model).is_some_and(|ty| {
                matches!(ty, Type::LiteralValue(_))
                    || super::concrete_scalar(
                        self.model.db(),
                        &self.model.program_environment(),
                        ty,
                    )
            })
            && matches!(
                &*a.value,
                ast::Expr::Name(_) | ast::Expr::Attribute(_) | ast::Expr::Subscript(_)
            )
        {
            // Only the scalar leaves this read. Still inspect evaluation of its receiver.
            self.read_reference(&a.value);
            self.replaced = false;
            return;
        }
        if let ast::Expr::Attribute(a) = expr
            && a.ctx == ast::ExprContext::Load
            && a.attr.as_str() == "run_path"
            && self.cached_module(&a.value)
        {
            self.escapes.push((
                a.attr.range(),
                "cannot prove which run_path callable was retrieved from sys.modules",
            ));
            return;
        }
        if self.cached_module(expr) {
            self.escapes.push((
                expr.range(),
                "the runpy module retrieved from sys.modules escapes a proven access",
            ));
            return;
        }
        if expr.is_lambda_expr() {
            self.replaced = false;
        }
        if let ast::Expr::Call(call) = expr {
            self.call(call);
            self.visit_arguments(&call.arguments);
        } else if matches!(expr, ast::Expr::Name(n) if n.ctx == ast::ExprContext::Load)
            || matches!(expr, ast::Expr::Attribute(a) if a.ctx == ast::ExprContext::Load)
        {
            if selected.unwrap_or_else(|| self.selection(expr)) != Selection::Other
                && !(self.replaced && self.export(expr))
            {
                self.escapes.push((
                    expr.range(),
                    "the selected runpy.run_path callable escapes a direct call",
                ));
                return;
            }
            if expr.is_name_expr() && self.module(expr, "runpy") {
                self.escapes.push((
                    expr.range(),
                    "the runpy module escapes direct attribute selection",
                ));
                return;
            }
            if let ast::Expr::Attribute(a) = expr
                && self.module(&a.value, "runpy")
            {
                self.replaced = false;
                if a.attr.as_str() == "__dict__" {
                    self.escapes.push((
                        expr.range(),
                        "the runpy namespace escapes direct attribute selection",
                    ));
                }
                return;
            }
            visitor::walk_expr(self, expr);
        } else if !matches!(expr, ast::Expr::Attribute(a) if self.module(&a.value, "runpy")) {
            visitor::walk_expr(self, expr);
        }
        if !expr.is_name_expr() && !expr.is_literal_expr() {
            self.replaced = false;
        }
    }
}
