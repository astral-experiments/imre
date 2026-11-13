//! Annotations of functions and classes defined by compile() or exec().

use super::*;
use ast::visitor::{self, Visitor};

/// A nested lambda body or comprehension has its own scope. Defaults and the outer
/// iterable still execute in the annotation scope.
#[derive(Default)]
struct Restrictions {
    invalid: Option<&'static str>,
    nested_scope: bool,
}

impl<'a> Visitor<'a> for Restrictions {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if self.invalid.is_some() {
            return;
        }
        self.nested_scope |= matches!(
            expr,
            ast::Expr::Lambda(_)
                | ast::Expr::ListComp(_)
                | ast::Expr::SetComp(_)
                | ast::Expr::DictComp(_)
                | ast::Expr::Generator(_)
        );
        match expr {
            ast::Expr::Named(_) => self.invalid = Some("named expression"),
            ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) => {
                self.invalid = Some("yield expression")
            }
            ast::Expr::Await(_) => self.invalid = Some("await expression"),
            ast::Expr::Lambda(l) => {
                if let Some(params) = &l.parameters {
                    self.visit_parameters(params);
                }
            }
            ast::Expr::ListComp(c) => self.visit_expr(&c.generators[0].iter),
            ast::Expr::SetComp(c) => self.visit_expr(&c.generators[0].iter),
            ast::Expr::DictComp(c) => self.visit_expr(&c.generators[0].iter),
            ast::Expr::Generator(c) => self.visit_expr(&c.generators[0].iter),
            _ => stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                visitor::walk_expr(self, expr)
            }),
        }
    }
}

pub(super) fn validate(expr: &ast::Expr, allow_nested_scopes: bool) -> Result<()> {
    let mut restrictions = Restrictions::default();
    restrictions.visit_expr(expr);
    if let Some(kind) = restrictions.invalid {
        return Err(Error::new(
            "SyntaxError",
            format!("{kind} cannot be used within an annotation"),
        ));
    }
    // This visitor checks the outer annotation scope. An attribute or item
    // store must not accept unchecked syntax inside a lambda or comprehension.
    if restrictions.nested_scope && !allow_nested_scopes {
        return Err(unsupported(
            "nested scope in an annotated assignment target",
        ));
    }
    Ok(())
}

pub(super) fn string_literal(expr: &ast::Expr) -> ast::Expr {
    let range = expr.range();
    let literal = ast::StringLiteral {
        range,
        node_index: ast::AtomicNodeIndex::NONE,
        value: imre_ast::annotations::spelling(expr).into_boxed_str(),
        flags: ast::StringLiteralFlags::empty(),
    };
    ast::ExprStringLiteral {
        range,
        node_index: ast::AtomicNodeIndex::NONE,
        value: ast::StringLiteralValue::single(literal),
    }
    .into()
}

impl Program {
    // A bare subscript annotation evaluates the keys and bounds in order,
    // without constructing a slice or fetching the final item.
    pub(super) fn discard_annotation_slice(
        &mut self,
        slice: ast::Expr,
        function: bool,
    ) -> Result<()> {
        match slice {
            ast::Expr::Slice(slice) => {
                for bound in [slice.lower, slice.upper, slice.step].into_iter().flatten() {
                    self.record_expression(*bound, function)?;
                }
            }
            ast::Expr::Tuple(items) => {
                for item in items.elts {
                    self.discard_annotation_slice(item, function)?;
                }
            }
            ast::Expr::Starred(_) => {
                return Err(Error::new(
                    "SyntaxError",
                    "can't use starred expression here",
                ));
            }
            expr => self.record_expression(expr, function)?,
        }
        Ok(())
    }

    pub(super) fn function_annotations(
        &self,
        function: &mut ast::StmtFunctionDef,
    ) -> Result<Vec<(String, ast::Expr)>> {
        // CPython emits ordinary positional annotations before the positional-only group.
        let params = &mut function.parameters;
        let mut entries: Vec<_> = params
            .args
            .iter_mut()
            .chain(&mut params.posonlyargs)
            .map(|p| &mut p.parameter)
            .chain(params.vararg.as_deref_mut())
            .chain(params.kwonlyargs.iter_mut().map(|p| &mut p.parameter))
            .chain(params.kwarg.as_deref_mut())
            .filter_map(|p| p.annotation.take().map(|expr| (p.name.to_string(), *expr)))
            .chain(
                function
                    .returns
                    .take()
                    .map(|expr| ("return".to_owned(), *expr)),
            )
            .collect();
        let stringized = self.string_annotations();
        for (_, expr) in &mut entries {
            validate(expr, true)?;
            if stringized {
                *expr = string_literal(expr);
            }
        }
        Ok(entries)
    }
}

impl<'a> CompiledStrings<'a, '_> {
    pub(super) fn annotation(
        &mut self,
        name: &str,
        expr: &'a ast::Expr,
        stringized: bool,
    ) -> Result<()> {
        self.heap.code_name(name);
        // The unparser cannot recover an escaped surrogate's original spelling.
        if stringized
            && self
                .spellings
                .range(expr.start()..expr.end())
                .any(|(_, points)| points.iter().any(|&p| p != 0xfffd))
        {
            return Err(unsupported("surrogate escapes in postponed annotations"));
        }
        self.visit_expr(expr);
        Ok(())
    }

    pub(super) fn annotation_scope(
        &mut self,
        entries: &[(String, ast::Expr)],
        class_scope: bool,
        stringized: bool,
        has_named_bindings: bool,
    ) -> Result<()> {
        let mut directives = HashMap::new();
        if class_scope {
            // A class global also bypasses the class dictionary in its annotation scope.
            for (name, (kind, _)) in self.directives {
                if *kind == ScopeDirective::Global {
                    directives.insert(name.clone(), (*kind, Default::default()));
                }
            }
        }
        let mut strings = CompiledStrings::new(self.heap, self.shared, self.spellings, &directives);
        for (name, expr) in entries {
            strings.annotation(name, expr, stringized)?;
        }
        let roots = strings.finish_nested_scope(has_named_bindings)?;
        if roots != NONE {
            self.keep(roots)?;
        }
        Ok(())
    }
}

impl Context {
    /// Look up a name in the current locals mapping, then in globals and builtins.
    pub(in crate::dynamic) fn module_annotation_name(
        &mut self,
        scope: Id,
        name: &str,
    ) -> Result<Id> {
        let locals = self.frame_locals(scope)?;
        match self.load_exec_name(locals, name) {
            Err(error) if self.is_builtin_error(&error, "KeyError") => {
                self.lookup(self.root_scope(scope), name)
            }
            result => result,
        }
    }

    pub(in crate::dynamic) fn mark_module_annotation(
        &mut self,
        scope: Id,
        index: Id,
    ) -> Result<()> {
        let active = self.module_annotation_name(scope, CONDITIONAL)?;
        self.mark_annotation(active, index)
    }

    pub(in crate::dynamic) fn mark_annotation(&mut self, active: Id, index: Id) -> Result<()> {
        let set = self.scalar_value(active);
        if !matches!(self.heap.get(set), Value::Set { frozen: false, .. }) {
            return Err(Error::new("TypeError", "expected a set"));
        }
        builtins::set_add(self, set, index)
    }

    /// Initialize an annotated module's storage before executing its suite.
    pub(in crate::dynamic) fn setup_module_annotations(
        &mut self,
        scope: Id,
        string_annotations: bool,
    ) -> Result<()> {
        let active = self.op("set", NONE, NONE, NONE)?;
        let locals = self.frame_locals(scope)?;
        let key = self.heap.code_name(CONDITIONAL);
        self.put_item(locals, key, active)?;
        if string_annotations {
            match self.load_exec_name(locals, "__annotations__") {
                Err(error) if self.is_builtin_error(&error, "KeyError") => {
                    let annotations = self.dict(Vec::new());
                    let key = self.heap.code_name("__annotations__");
                    self.put_item(locals, key, annotations)?;
                }
                result => {
                    result?;
                }
            }
        }
        Ok(())
    }
}
