//! Resolve declaration and annotation code sites before emitting the checked block.
use super::super::sites::{
    AnnotationString, CheckedSites, CompiledAlias, CompiledDefinition, CompiledGenerator, Declaration,
    RequiredSite,
};
use super::super::{AliasValue, Body, DefinitionName, DefinitionSite, Entry, Function, SourceInfo};
use crate::frontend::{AnnotationCode, PreparedAnnotation};
use crate::ir::{Expr, Statement};
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};

impl SourceInfo<'_> {
    pub(super) fn function_body<'s>(&self, def: &'s ast::StmtFunctionDef) -> Body<'s> {
        let suspension_flags = self.scopes.function(def).suspension_flags;
        if suspension_flags == 0 {
            Body::Statements(&def.body)
        } else {
            Body::GeneratorFunction(&def.body, suspension_flags)
        }
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
        let code = body.as_ref().and_then(|body| {
            let unit = self.source.unit;
            let file = self.frame.output.data(b"<string>");
            let location = super::super::SourceMap {
                info: &body.scope,
                lambda_code: Default::default(),
                definitions: None,
                unit,
                positions: &super::super::SourcePositions::new(text, &file),
            };
            match &body.code {
                AnnotationCode::Checked(function) => Some(location.code(
                    self.frame.output,
                    String::new(),
                    Body::CheckedExpression(function),
                    false,
                    None,
                )),
                AnnotationCode::Residual(expr) => self.frame.output.try_native_body(|output| {
                    location.code(output, String::new(), Body::Expression(expr), false, None)
                }),
            }
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
        body: Body<'_>,
        comprehension: bool,
    ) {
        if self.sites.lambdas.contains_key(&site) {
            return;
        }
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
            header: false,
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
            DefinitionName::Plain("<genexpr>"),
            None,
            None,
            None,
            Body::CheckedGenerator(site, body),
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
                self.place_lambda(site, Body::CheckedExpression(body), scoped);
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
        if !self.sites.definitions.contains_key(&start) {
            let syntax = self.source.definitions.as_ref().expect("registered scope")[&start];
            let (definition, decorators, type_params) = match syntax {
                DefinitionSite::Function(def) => {
                    let params = self.method_type_parameters(def);
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
                    None,
                ),
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
    fn forward_annotations_bind_the_compiled_child_and_preserve_residual_expressions() {
        let source = "checked: 'select(Child)'\nclosure: '(lambda: Child)()'\ngenerator: '(x for x in Child)'\nresidual: 'resolve([Child for _ in (0,)])'\ninvalid: 'Child['\n";
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
        let closure = child("'(lambda");
        assert!(matches!(
            closure.code,
            crate::frontend::AnnotationCode::Checked(_)
        ));
        assert!(closure.scope.lambdas[&1.into()].body.is_some());
        let generator = child("'(x for");
        assert!(matches!(
            generator.code,
            crate::frontend::AnnotationCode::Checked(_)
        ));
        assert_eq!(generator.scope.generators.len(), 1);
        assert!(matches!(
            child("'resolve(").code,
            crate::frontend::AnnotationCode::Residual(_)
        ));
        let mut output = Output::default();
        let filename = output.data(b"example.py");
        body.emit(&mut output, 0, &filename).unwrap();
        let children: Vec<_> = output
            .functions
            .lines()
            .filter(|line| line.contains("call void @imre_dyn_bind_expression("))
            .map(|line| line.rsplit_once("ptr ").unwrap().1.trim_end_matches(')'))
            .collect();
        assert_eq!(children.len(), 4);
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
