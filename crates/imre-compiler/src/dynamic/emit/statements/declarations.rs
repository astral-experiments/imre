//! Resolve declaration and annotation code sites before emitting the checked block.
use super::super::sites::{CheckedSites, CompiledAlias, CompiledDefinition, Declaration};
use super::super::{AliasValue, Body, DefinitionName, DefinitionSite, Entry, Function};
use crate::ir::{self, Builtin, Expr, ExprKind, Statement};
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextSize};

impl<'s> Body<'s> {
    pub(super) fn function(def: &'s ast::StmtFunctionDef) -> Self {
        match (def.is_async, imre_ast::code_flags::is_generator(&def.body)) {
            (false, false) => Self::Statements(&def.body),
            (false, true) => Self::GeneratorFunction(&def.body, imre_ast::code_flags::GENERATOR),
            (true, false) => Self::GeneratorFunction(&def.body, imre_ast::code_flags::COROUTINE),
            (true, true) => {
                Self::GeneratorFunction(&def.body, imre_ast::code_flags::ASYNC_GENERATOR)
            }
        }
    }
}

impl Function<'_, '_> {
    /// Compile each source-local child once. Repeated markers can bind the same
    /// entry on independent execution paths through a module's annotation provider.
    pub(in crate::dynamic::emit) fn prepare_annotation_children(
        &mut self,
        entries: &[&crate::frontend::DeferredModuleAnnotation],
    ) {
        for entry in entries {
            let crate::frontend::AnnotationValue::Bound(child) = &entry.value else {
                continue;
            };
            if self.frame.annotation_children.contains_key(&child.id) {
                continue;
            }
            let file = self.frame.output.data(b"<string>");
            let positions = super::super::SourcePositions::new(&child.text, &file);
            let mut frame = super::super::RegisteredFrame::new(
                self.frame.output,
                &positions,
                false,
                self.source.unit,
                TextSize::default(),
            );
            super::super::RegisteredFrame::checked_body(
                &mut frame,
                &child.body,
                &CheckedSites::default(),
            );
            let code = frame.finish(false);
            self.frame.annotation_children.insert(child.id, code);
        }
    }

    pub(in crate::dynamic::emit) fn prepare_checked_value(&mut self, value: &Expr) -> CheckedSites {
        let mut sites = CheckedSites::default();
        ir::walk_operand_expressions(value, &mut |expr| self.prepare_site(expr, &mut sites));
        sites
    }

    pub(in crate::dynamic::emit) fn prepare_checked_sites(
        &mut self,
        statements: &[Statement],
    ) -> CheckedSites {
        let mut sites = CheckedSites::default();
        self.prepare_declarations(statements, &mut sites);
        sites
    }

    /// A block stops emitting after a transfer. Unreachable declaration bodies must
    /// not compile (or raise diagnostics) merely because their identity is in the IR.
    fn prepare_declarations(&mut self, statements: &[Statement], sites: &mut CheckedSites) -> bool {
        for stmt in statements {
            match stmt {
                Statement::If(test, yes, no) => {
                    ir::walk_operand_expressions(test, &mut |e| self.prepare_site(e, sites));
                    let yes_continues = self.prepare_declarations(yes, sites);
                    let no_continues = self.prepare_declarations(no, sites);
                    if !yes_continues && !no_continues {
                        return false;
                    }
                }
                Statement::While(test, body, otherwise) => {
                    ir::walk_operand_expressions(test, &mut |e| self.prepare_site(e, sites));
                    self.prepare_declarations(body, sites);
                    self.prepare_declarations(otherwise, sites);
                }
                Statement::Scope { body, .. } => {
                    if !self.prepare_declarations(body, sites) {
                        return false;
                    }
                }
                _ => {
                    ir::walk_expressions(std::slice::from_ref(stmt), &mut |e| {
                        self.prepare_site(e, sites)
                    });
                    if matches!(
                        stmt,
                        Statement::Return(_)
                            | Statement::Break
                            | Statement::Continue
                            | Statement::Exhaust
                    ) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Python creates a fresh callable at each execution of the binding while the
    /// underlying code object belongs to one lexical lambda.
    pub(in crate::dynamic::emit) fn compile_lambda(
        &mut self,
        site: TextSize,
        body: Body<'_>,
    ) -> CompiledDefinition {
        let metadata = &self.source.lambdas[&site].metadata;
        let qualname = format!("{}<lambda>", self.prefix);
        let existing = self.source.lambda_code.borrow().get(&site).cloned();
        let code = existing.unwrap_or_else(|| {
            let entry = if metadata.generator { Entry::Suspended } else { Entry::Compiled };
            let code = self.source.definition(
                self.frame.output,
                format!("{qualname}.<locals>."),
                body,
                site,
                true,
                entry,
            );
            self.source.lambda_code.borrow_mut().insert(site, code.clone());
            code
        });
        let method = self.class_locals.is_some() && self.annotation_values.is_none();
        let signature = metadata.signature(self.in_function, method, self.source.future_flags());
        self.output.interned_identifiers.extend(metadata.parameter_names().map(str::to_owned));
        CompiledDefinition {
            code,
            signature: signature.to_string(),
            name: "<lambda>".to_owned(),
            qualname,
            doc: None,
            provider: None,
            default_names: metadata.defaults.clone(),
            type_alias: false,
            header: false,
        }
    }

    pub(super) fn compile_type_alias(
        &mut self,
        site: TextSize,
        body: AliasValue<'_>,
    ) -> CompiledAlias {
        let alias = &self.source.type_aliases[&site];
        if let Some(parameters) = &alias.parameters {
            self.check_suspended_parameters(parameters, alias.range);
        }
        let definition = self.compile_definition(
            site,
            DefinitionName::Plain(&alias.name),
            None,
            None,
            None,
            Body::AliasEvaluator(body),
        );
        CompiledAlias {
            definition,
            parameters: alias.parameters.clone(),
        }
    }

    fn prepare_site(&mut self, expression: &Expr, sites: &mut CheckedSites) {
        if let ExprKind::Builtin(Builtin::RegisteredTypeAlias(site), _) = &expression.kind {
            sites.aliases.entry(*site).or_insert_with(|| {
                let body = self.source.type_aliases[site]
                    .body
                    .as_ref()
                    .expect("checked alias value");
                self.compile_type_alias(*site, AliasValue::Checked(body))
            });
            return;
        }
        if let ExprKind::Builtin(Builtin::RegisteredLambda(site, _), _) = &expression.kind {
            sites.lambdas.entry(*site).or_insert_with(|| {
                let body = self.source.lambdas[site].body.as_ref().expect("checked lambda body");
                self.compile_lambda(*site, Body::CheckedExpression(body))
            });
            return;
        }
        let ExprKind::Builtin(Builtin::RegisteredDefinition(site), _) = &expression.kind else {
            return;
        };
        sites.definitions.entry(*site).or_insert_with(|| {
            let syntax = self.source.definitions.as_ref().expect("registered scope")[site];
            let (definition, decorators, class) = match syntax {
                DefinitionSite::Function(def) => (
                    self.compile_definition(
                        def.decorator_list
                            .first()
                            .map_or(def.start(), Ranged::start),
                        DefinitionName::Binding(&def.name),
                        Some(&def.parameters),
                        Some(def),
                        None,
                        Body::function(def),
                    ),
                    &def.decorator_list,
                    false,
                ),
                DefinitionSite::Class(def) => (
                    self.compile_definition(
                        def.decorator_list
                            .first()
                            .map_or(def.start(), Ranged::start),
                        DefinitionName::Binding(&def.name),
                        None,
                        None,
                        None,
                        Body::Namespace(&def.body),
                    ),
                    &def.decorator_list,
                    true,
                ),
            };
            let decorator_lines = decorators
                .iter()
                .map(|d| self.source.positions.line(d.expression.start()))
                .collect();
            Declaration {
                definition,
                decorator_lines,
                class,
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::dynamic::emit::{ModuleOrigin, Output, PreparedBody};
    use std::path::Path;

    #[test]
    fn forward_annotations_bind_the_compiled_child_and_preserve_residual_expressions() {
        let source = "checked: 'select(Child)'\nresidual: '(lambda: Child)()'\ninvalid: 'Child['\n";
        let body = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Provider,
        )
        .unwrap();
        let mut output = Output::default();
        let filename = output.data(b"example.py");
        body.emit(&mut output, 0, &filename).unwrap();
        let children: Vec<_> = output
            .functions
            .lines()
            .filter(|line| line.contains("call void @imre_dyn_bind_expression("))
            .map(|line| line.rsplit_once("ptr ").unwrap().1.trim_end_matches(')'))
            .collect();
        assert_eq!(children.len(), 2);
        assert_ne!(children[0], children[1]);
        for child in children {
            assert_eq!(
                output
                    .functions
                    .matches(&format!("define internal i64 {child}("))
                    .count(),
                1
            );
        }
    }
}
