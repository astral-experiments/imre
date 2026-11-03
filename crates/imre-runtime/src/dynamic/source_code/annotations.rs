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

#[derive(Clone, Copy, PartialEq)]
pub(super) enum AnnotationScope {
    Function,
    Class,
    Module,
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
        let stringized = self.string_annotations();
        for (_, expr) in &mut entries {
            validate(expr, true)?;
            if stringized {
                *expr = string_literal(expr);
            }
        }
        Ok(Some(self.annotation_provider(
            entries,
            Some(&body.qualname),
            line,
        )))
    }

    pub(super) fn annotation_provider(
        &self,
        entries: Vec<(String, ast::Expr)>,
        owner: Option<&str>,
        line: u32,
    ) -> Rc<Program> {
        let mut provider = self.nested_program("__annotate__");
        provider.qualname = owner.map_or_else(
            || "__annotate__".into(),
            |name| format!("{name}.__annotate__"),
        );
        provider.first_line = line;
        provider.method_imports = self.method_imports.clone();
        provider.private_names = self.private_names.clone();
        provider.template_expressions = self.template_expressions.clone();
        let scope = if owner.is_none() {
            AnnotationScope::Module
        } else if self.class_attributes.is_some() && !self.string_annotations() {
            provider.reads.insert("__classdict__".into());
            AnnotationScope::Class
        } else {
            AnnotationScope::Function
        };
        provider
            .instructions
            .push((line, Instruction::Annotations { entries, scope }));
        Rc::new(provider)
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

    pub(super) fn annotation_code(
        &mut self,
        provider: &'a mut Rc<Program>,
        class_locals: Option<&HashSet<String>>,
        line_at: &impl Fn(usize) -> u32,
    ) -> Result<&'a BTreeSet<String>> {
        let provider = Rc::get_mut(provider).expect("new annotation code");
        if class_locals.is_some() {
            // A global declared by the class also bypasses its dictionary in the annotation scope.
            for (name, (kind, _)) in self.directives {
                if *kind == ScopeDirective::Global {
                    provider.directives.insert(name.clone(), (*kind, 0.into()));
                }
            }
        }
        let (roots, reads) = provider.prepare_scope_strings(
            self.heap,
            self.shared,
            self.spellings,
            line_at,
            class_locals,
        )?;
        if roots != NONE {
            self.keep(roots)?;
        }
        Ok(reads)
    }
}

impl Context {
    /// Method providers consult their live class dictionary before lexical or global names.
    fn source_annotation_scope(&mut self, scope: Id, defining_scope: Id, program: &Program) -> Id {
        let parent = self.scope_data(scope);
        if parent.class_body.is_none() || !program.reads.contains("__classdict__") {
            return defining_scope;
        }
        let parent_scope = if defining_scope == scope {
            parent.parent.expect("class definition scope")
        } else {
            defining_scope
        };
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
        defining_scope: Id,
    ) -> Result<Id> {
        let scope = self.source_annotation_scope(scope, defining_scope, program);
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

    /// Implicit annotation bytecode uses LOAD_NAME even for a declared global.
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

    pub(super) fn start_module_annotations(
        &mut self,
        code: &Program,
        owner: Id,
        scope: Id,
        provider: Option<&Rc<Program>>,
    ) -> Result<()> {
        let frame = self.scope_data(scope);
        let globals = if frame.exec_locals.is_some() {
            frame.parent.expect("exec globals frame")
        } else {
            scope
        };
        if let Some(provider) = provider {
            let annotate = self.source_annotator(provider, owner, globals, globals)?;
            self.bind_source_name(scope, "__annotate__", annotate)?;
        }
        self.setup_module_annotations(scope, code.string_annotations())
    }

    pub(in crate::dynamic) fn mark_module_annotation(
        &mut self,
        scope: Id,
        index: Id,
    ) -> Result<()> {
        let active = self.module_annotation_name(scope, "__conditional_annotations__")?;
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
        let key = self.heap.code_name("__conditional_annotations__");
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

    pub(super) fn source_annotations(
        &mut self,
        code: &Program,
        scope: Id,
        entries: &[(String, ast::Expr)],
        annotation_scope: AnnotationScope,
    ) -> Result<Id> {
        let format = self.scope_name(scope, "format").expect("annotation format");
        let two = self.int(2);
        let unsupported = self.op(">", format, two, NONE)?;
        if self.truth(unsupported)? {
            return Err(self.exception("NotImplementedError", &[]));
        }
        // The synthetic argument 'format' is invisible to the expressions themselves.
        let mut environment = self.scope_data(scope).parent.expect("annotation globals");
        if annotation_scope == AnnotationScope::Class {
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
        for (index, (name, expr)) in entries.iter().enumerate() {
            if annotation_scope == AnnotationScope::Module {
                let active = self.lookup(environment, "__conditional_annotations__")?;
                let index = self.int(index as i64);
                if !self.contains(index, active)? {
                    continue;
                }
            }
            let value = if let ast::Expr::Starred(starred) = expr {
                let value = self.source_expr(code, environment, &starred.value)?;
                self.unpack_items(value, 1, None)?[0]
            } else {
                self.source_expr(code, environment, expr)?
            };
            let key = self.heap.code_name(name);
            self.put_item(annotations, key, value)?;
        }
        Ok(annotations)
    }
}
