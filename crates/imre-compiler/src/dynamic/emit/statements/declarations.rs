//! Resolve declaration and annotation code sites before emitting the checked block.
use super::super::sites::{
    AnnotationString, CheckedSites, CompiledAlias, CompiledDefinition, CompiledGenerator, Declaration,
    RequiredSite,
};
use super::super::{AliasValue, Body, DefinitionName, DefinitionSite, Entry, Function, SourceInfo};
use crate::frontend::PreparedAnnotation;
use crate::ir::{Expr, Statement};
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};

impl SourceInfo<'_> {
    pub(super) fn function_body<'s>(&self, def: &'s ast::StmtFunctionDef) -> Body<'s> {
        Body::Function(&def.body, self.scopes.function(def).suspension_flags)
    }
}

impl Function<'_, '_> {
    pub(in crate::dynamic::emit) fn place_annotation_string(
        &mut self,
        site: ruff_text_size::TextSize,
    ) {
        let Some(PreparedAnnotation::String { text, body }) =
            self.source.annotation_operands.get(&site)
        else {
            return;
        };
        if self.sites.annotation_strings.contains_key(&site) {
            return;
        }
        let code = body.as_ref().map(|body| {
            let unit = self.source.unit;
            let file = self.frame.output.data(b"<string>");
            let location = super::super::SourceMap {
                info: &body.scope,
                lambda_code: Default::default(),
                definitions: None,
                unit,
                positions: &super::super::SourcePositions::new(text, &file),
            };
            location.code(
                self.frame.output,
                String::new(),
                Body::AnnotationOperand(body.code.range, &body.code.operand),
                false,
                None,
            )
        });
        self.sites.annotation_strings.insert(
            site,
            AnnotationString {
                text: text.clone(),
                code,
            },
        );
    }

    pub(in crate::dynamic::emit) fn prepare_checked_value(&mut self, value: &Expr) {
        CheckedSites::visit_operand(value, &mut |site| self.prepare_site(site));
    }

    pub(in crate::dynamic::emit) fn prepare_checked_sites(&mut self, statements: &[Statement]) {
        CheckedSites::visit(statements, &mut |site| self.prepare_site(site));
    }

    /// Python creates a fresh callable at each execution of the binding while the
    /// underlying code object belongs to one lexical lambda.
    pub(in crate::dynamic::emit) fn place_lambda(
        &mut self,
        site: TextSize,
        body: &crate::frontend::RegisteredFunction,
        comprehension: bool,
    ) {
        if self.sites.lambdas.contains_key(&site) {
            return;
        }
        let metadata = &self.source.lambdas[&site].metadata;
        let qualname = format!("{}<lambda>", self.prefix);
        let existing = self.source.lambda_code.borrow().get(&site).cloned();
        let code = existing.unwrap_or_else(|| {
            let entry = if metadata.generator {
                Entry::Suspended(imre_ast::code_flags::GENERATOR)
            } else {
                Entry::Compiled
            };
            let code = self.source.definition(
                self.frame.output,
                format!("{qualname}.<locals>."),
                Body::Checked(body),
                site,
                true,
                entry,
            );
            self.source.lambda_code.borrow_mut().insert(site, code.clone());
            code
        });
        // A lambda bound inside a display captures its iteration namespace, including
        // when the display itself is evaluated directly in a class body.
        let method =
            !comprehension && self.class_locals.is_some() && self.annotation_values.is_none();
        let signature = metadata.signature(
            self.in_function || comprehension,
            method,
            self.source.future_flags(),
        );
        self.output
            .interned_identifiers
            .extend(metadata.parameter_names().map(str::to_owned));
        let definition = CompiledDefinition {
            code,
            signature: signature.to_string(),
            name: "<lambda>".to_owned(),
            qualname,
            doc: None,
            provider: None,
            default_names: metadata.defaults.clone(),
            type_alias: false,
            class_header: None,
        };
        self.sites.lambdas.insert(site, definition);
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

    pub(in crate::dynamic::emit) fn prepare_generator(&mut self, site: TextRange) {
        if self.sites.generators.contains_key(&site) {
            return;
        }
        let body = &self.source.generators[&site];
        let definition = self.compile_definition(
            site.start(),
            DefinitionName::Generator(site),
            None,
            None,
            None,
            Body::Checked(&body.body),
        );
        let targets = self.source.scopes.comprehension_targets[&site].clone();
        self.sites.generators.insert(
            site,
            CompiledGenerator { definition, targets },
        );
    }

    pub(in crate::dynamic::emit) fn prepare_site(&mut self, site: RequiredSite) {
        let start = match site {
            RequiredSite::Generator(site) => {
                self.prepare_generator(site);
                return;
            }
            RequiredSite::Alias(site) => {
                if !self.sites.aliases.contains_key(&site) {
                    let body = self.source.type_aliases[&site]
                        .body
                        .as_ref()
                        .expect("checked alias value");
                    let alias = self.compile_type_alias(site, AliasValue::Checked(body));
                    self.sites.aliases.insert(site, alias);
                }
                return;
            }
            RequiredSite::Lambda(site, scoped) => {
                let body = self.source.lambdas[&site].body.as_ref().expect("checked lambda body");
                self.place_lambda(site, body, scoped);
                return;
            }
            RequiredSite::AnnotationString(site) => {
                self.place_annotation_string(site);
                return;
            }
            RequiredSite::AnnotationField(site) => {
                let decl = &self.source.annotations[&site];
                self.sites
                    .annotation_fields
                    .entry(site)
                    .or_insert_with(|| (decl.name.clone(), decl.range));
                return;
            }
            RequiredSite::Definition(start) => start,
        };
        self.prepare_definition(start);
    }

    pub(super) fn prepare_definition(&mut self, start: TextSize) {
        if !self.sites.definitions.contains_key(&start) {
            let syntax = self.source.definitions.as_ref().expect("registered scope")[&start];
            let (definition, decorators, type_params) = match syntax {
                DefinitionSite::Function(def) => {
                    let params = self.namespace_type_parameters(def);
                    let definition = self.compile_definition(
                        def.decorator_list
                            .first()
                            .map_or(def.start(), Ranged::start),
                        DefinitionName::Binding(&def.name),
                        Some(&def.parameters),
                        Some(def),
                        params,
                        self.source.function_body(def),
                    );
                    (definition, &def.decorator_list, params)
                }
                DefinitionSite::Class(def) => {
                    let definition = if self.source.class(def).header.parameters.is_some() {
                        let name = format!("<generic parameters of {}>", def.name);
                        self.compile_definition(
                            def.start(),
                            DefinitionName::Plain(&name),
                            None,
                            None,
                            None,
                            Body::ClassParameters(def),
                        )
                    } else {
                        self.compile_class_namespace(def)
                    };
                    (definition, &def.decorator_list, None)
                }
            };
            let decorator_lines = decorators
                .iter()
                .map(|d| self.source.positions.line(d.expression.start()))
                .collect();
            let declaration = Declaration {
                definition,
                decorator_lines,
                type_params: type_params.map_or_else(Vec::new, |params| {
                    crate::frontend::registered_type_parameters(params, &self.source.private_names)
                }),
            };
            self.sites.definitions.insert(start, declaration);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::dynamic::emit::{ModuleOrigin, Output, PreparedBody};
    use std::path::Path;

    #[test]
    fn typing_initializer_binds_the_checked_reveal_type_definition() {
        use crate::ir::{Builtin, DefinitionSite, ExprKind};

        let source = include_str!("../../../../stdlib/typing.py");
        let start =
            ruff_text_size::TextSize::try_from(source.find("def reveal_type[T]").unwrap()).unwrap();
        PreparedBody::with_module(
            "typing",
            Path::new("<imre stdlib>/typing.py"),
            source,
            0,
            ModuleOrigin::Bundled,
            |module| {
                let initializer = module.source.initializer.as_ref().unwrap();
                assert!(initializer.fully_checked());
                let mut bindings = 0;
                crate::ir::walk_expressions(&initializer.checked.function.body, &mut |value| {
                    if let ExprKind::Builtin(
                        Builtin::RegisteredDefinition(DefinitionSite::Function(site)),
                        operands,
                    ) = &value.kind
                        && *site == start
                    {
                        assert!(operands.is_empty());
                        bindings += 1;
                    }
                });
                assert_eq!(bindings, 1);
                assert!(module.source.function_bodies[&start].fully_checked());
                Ok(())
            },
        )
        .unwrap();
    }

    #[test]
    fn forward_annotations_bind_prepared_children() {
        let source = r#"checked: 'select(Child)'
eager: '[x for x in Child]'
projected: "Child.partition(',')[1]"
width: "f'{Child!r:>{Width}}'"
closure: '(lambda: Child)()'
lazy: '(x for x in Child)'
wrapped: 'list(x for x in Child)'
nested: 'lambda: (x for x in Child)'
collected: 'lambda: [box.make(x).value for x in Child]'
bound: 'lambda: (v := box.make(9)).value'
default: 'lambda vals=[box.make(x).value for x in Child]: vals'
template: "t'{client.lookup(*args)}{[v for v in values]}'"
template_generator: "t'{client.lookup(*args)}{(v for v in values)}'"
argument: 'resolve([Child for _ in (0,)])'
invalid: 'Child['
"#;
        let body = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Provider,
        )
        .unwrap();
        let child = |quoted: &str| {
            let site = u32::try_from(source.find(quoted).unwrap()).unwrap().into();
            let Some(crate::frontend::PreparedAnnotation::String {
                body: Some(child), ..
            }) = body.source.annotation_operands.get(&site)
            else {
                panic!("missing prepared annotation {quoted}")
            };
            child
        };
        assert!(
            !child("'[x for")
                .code
                .operand
                .region()
                .private_slots()
                .is_empty()
        );
        assert!(child("'(lambda").scope.lambdas[&1.into()].body.is_some());
        for quoted in ["'(x for", "'list(x for", "'lambda:"] {
            assert_eq!(child(quoted).scope.generators.len(), 1, "{quoted}");
        }
        assert!(child("'lambda:").scope.lambdas[&0.into()].body.is_some());
        for quoted in ["'lambda: [box", "'lambda: (v", "'lambda vals="] {
            assert!(
                child(quoted).scope.lambdas[&0.into()].body.is_some(),
                "{quoted}"
            );
        }
        assert!(
            !child("\"t'")
                .code
                .operand
                .region()
                .private_slots()
                .is_empty()
        );
        assert!(
            !child("'resolve(")
                .code
                .operand
                .region()
                .private_slots()
                .is_empty()
        );
        assert_eq!(
            child("\"t'{client.lookup(*args)}{(").scope.generators.len(),
            1
        );
        let mut output = Output::default();
        let filename = output.data(b"example.py");
        body.emit(&mut output, 0, &filename).unwrap();
        let children: Vec<_> = output
            .functions
            .lines()
            .filter(|line| line.contains("call void @imre_dyn_bind_expression("))
            .map(|line| line.rsplit_once("ptr ").unwrap().1.trim_end_matches(')'))
            .collect();
        assert_eq!(children.len(), 14);
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
