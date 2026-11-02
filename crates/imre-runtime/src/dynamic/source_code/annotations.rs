//! Deferred annotations of functions defined by compile() or exec().

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

impl Program {
    pub(super) fn annotation_program(
        &self,
        function: &mut ast::StmtFunctionDef,
        body: &Program,
        line: u32,
    ) -> Result<Option<Rc<Program>>> {
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
        if entries.is_empty() {
            return Ok(None);
        }
        let stringized = self
            .private_names
            .as_ref()
            .is_some_and(|names| names.string_annotations);
        for (_, expr) in &mut entries {
            validate(expr, true)?;
            if stringized {
                let range = expr.range();
                let literal = ast::StringLiteral {
                    range,
                    node_index: ast::AtomicNodeIndex::NONE,
                    value: imre_ast::annotations::spelling(expr).into_boxed_str(),
                    flags: ast::StringLiteralFlags::empty(),
                };
                *expr = ast::ExprStringLiteral {
                    range,
                    node_index: ast::AtomicNodeIndex::NONE,
                    value: ast::StringLiteralValue::single(literal),
                }
                .into();
            }
        }
        let mut provider = self.nested_program("__annotate__");
        provider.qualname = format!("{}.__annotate__", body.qualname);
        provider.first_line = line;
        provider.method_imports = self.method_imports.clone();
        provider.private_names = self.private_names.clone();
        provider.template_expressions = self.template_expressions.clone();
        let class = self.class_attributes.is_some() && !stringized;
        if class {
            provider.reads.insert("__classdict__".into());
        }
        provider
            .instructions
            .push((line, Instruction::Annotations { entries, class }));
        Ok(Some(Rc::new(provider)))
    }
}

impl Context {
    /// Method providers consult their live class dictionary before lexical or global names.
    fn source_annotation_scope(&mut self, scope: Id, program: &Program) -> Id {
        let parent = self.scope_data(scope);
        if parent.class_body.is_none() || !program.reads.contains("__classdict__") {
            return scope;
        }
        let parent_scope = parent.parent.expect("class definition scope");
        let class_cell = parent.class_cell;
        let dict_cell = self.class_annotation_dict_cell(scope);
        let mut names = vec!["__classdict__".to_owned()];
        let mut cells = vec![dict_cell];
        if let Some(cell) = class_cell
            && program.reads.contains("__class__")
        {
            names.push("__class__".to_owned());
            cells.push(cell);
        }
        self.closure_environment(parent_scope, names, &cells)
    }

    pub(super) fn source_annotator(
        &mut self,
        program: &Rc<Program>,
        function_code: Id,
        scope: Id,
    ) -> Result<Id> {
        let scope = self.source_annotation_scope(scope, program);
        // Annotation code is constant for this definition, even across calls to its factory.
        let code = if let Some((_, id)) = self
            .code_object(function_code)
            .nested
            .iter()
            .find(|(site, _)| *site == usize::MAX)
        {
            *id
        } else {
            let signature = self.text(r#"{"posonly":["format"]}"#);
            let code = self.box_source_code(program.clone(), Some(signature));
            if let Some(names) =
                self.enclosing_freevars(scope, program.reads.iter().map(String::as_str))
            {
                let Value::Code(data) = self.heap.get_mut(code) else {
                    unreachable!()
                };
                data.freevars = names;
                data.closure_supported = true;
            }
            let Value::Code(function) = self.heap.get_mut(function_code) else {
                unreachable!()
            };
            function.nested.push((usize::MAX, code));
            code
        };
        let defaults = self.dict(Vec::new());
        Ok(self.source_function(code, scope, defaults))
    }

    pub(super) fn source_annotations(
        &mut self,
        code: &Program,
        scope: Id,
        entries: &[(String, ast::Expr)],
        class: bool,
    ) -> Result<Id> {
        let format = self.scope_name(scope, "format").expect("annotation format");
        let two = self.int(2);
        let unsupported = self.op(">", format, two, NONE)?;
        if self.truth(unsupported)? {
            return Err(self.exception("NotImplementedError", &[]));
        }
        // The synthetic argument 'format' is invisible to the expressions themselves.
        let mut environment = self.scope_data(scope).parent.expect("annotation globals");
        if class {
            let dict = self.lookup(environment, "__classdict__")?;
            let globals = self.scope_data(scope).globals.clone();
            let locals = self.scope("", Some(environment));
            let Value::Scope(frame) = self.heap.get_mut(locals) else {
                unreachable!()
            };
            frame.exec_locals = Some(dict);
            frame.globals = globals;
            environment = locals;
        }
        let annotations = self.dict(Vec::new());
        for (name, expr) in entries {
            let value = self.source_expr(code, environment, expr)?;
            let key = self.heap.code_name(name);
            self.put_item(annotations, key, value)?;
        }
        Ok(annotations)
    }
}
