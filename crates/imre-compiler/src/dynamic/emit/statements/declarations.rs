//! Resolve declaration and annotation code sites before emitting the checked block.
use super::super::sites::{
    AnnotationString, CheckedSites, CompiledAlias, CompiledDefinition, CompiledGenerator,
    Declaration, RequiredSite,
};
use super::super::{
    Body, DefinitionName, Entry, Function, Output, ReturnOperand, SourceInfo, SourceMap,
};
use crate::frontend::PreparedAnnotation;
use crate::frontend::function_scope::OrdinaryFunction;
use crate::ir::{Expr, Statement};
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};

impl SourceInfo<'_> {
    pub(in crate::dynamic::emit) fn function_body<'s>(
        &'s self,
        def: &'s ast::StmtFunctionDef,
    ) -> Body<'s> {
        let start = def.start();
        let plan = &self.function_bodies[&start];
        if plan.header.is_none()
            && let super::super::FunctionBody::Suite(suite) = &plan.body
        {
            Body::Suite(suite)
        } else {
            let globals = self.scopes.functions[&start].lexical.globals();
            Body::Checked(plan.checked(), Some(globals))
        }
    }
}

enum Callable<'a> {
    Lambda { comprehension: bool },
    Ordinary(&'a OrdinaryFunction),
}

impl SourceMap<'_> {
    /// Place the checked callable prefix in order. The first richer declaration and
    /// its successors keep their original order and their existing placement service.
    pub(in crate::dynamic::emit) fn place_module_callables<'s>(
        &self,
        output: &mut Output,
        sites: &mut CheckedSites,
        required: &'s [RequiredSite],
    ) -> &'s [RequiredSite] {
        let mut callables = CallableSites {
            source: self,
            output,
            sites,
            prefix: "",
            declared_globals: &self.scopes.module_globals,
            in_function: false,
            method_scope: false,
        };
        for (index, &site) in required.iter().enumerate() {
            match site {
                RequiredSite::Lambda(site, scoped) => callables.place_checked_lambda(site, scoped),
                RequiredSite::Definition(site) if callables.prepare_ordinary_function(site) => {}
                _ => return &required[index..],
            }
        }
        &[]
    }

    pub(in crate::dynamic::emit) fn place_annotation_string(
        &self,
        output: &mut Output,
        sites: &mut CheckedSites,
        site: ruff_text_size::TextSize,
    ) {
        let Some(PreparedAnnotation::String { text, body }) = self.annotation_operands.get(&site)
        else {
            return;
        };
        if sites.annotation_strings.contains_key(&site) {
            return;
        }
        let code = body.as_ref().map(|body| {
            let unit = self.unit;
            let file = output.data(b"<string>");
            let location = super::super::SourceMap {
                info: &body.scope,
                callable_code: Default::default(),
                definitions: None,
                unit,
                positions: &super::super::SourcePositions::new(text, &file),
            };
            location.code(
                output,
                String::new(),
                Body::Operand(
                    body.code.range,
                    ReturnOperand::Prepared(&body.code.operand, None),
                ),
                false,
                None,
            )
        });
        sites.annotation_strings.insert(
            site,
            AnnotationString {
                text: text.clone(),
                code,
            },
        );
    }
}

impl Function<'_, '_> {
    pub(in crate::dynamic::emit) fn prepare_checked_value(&mut self, value: &Expr) {
        CheckedSites::visit_operand(value, &mut |site| self.prepare_site(site));
    }

    pub(in crate::dynamic::emit) fn prepare_checked_sites(&mut self, statements: &[Statement]) {
        CheckedSites::visit(statements, &mut |site| self.prepare_site(site));
    }

    pub(in crate::dynamic::emit) fn callable_sites(&mut self) -> CallableSites<'_, '_> {
        CallableSites {
            source: self.source,
            sites: &mut self.sites,
            output: self.frame.output,
            prefix: &self.prefix,
            declared_globals: &self.declared_globals,
            in_function: self.in_function,
            method_scope: self.frame.class_locals.is_some()
                && self.frame.annotation_values.is_none(),
        }
    }
}

/// The enclosing lexical scope and the existing address table used to place checked callables.
pub(in crate::dynamic::emit) struct CallableSites<'a, 'p> {
    source: &'a SourceMap<'p>,
    output: &'a mut Output,
    sites: &'a mut CheckedSites,
    prefix: &'a str,
    declared_globals: &'a std::collections::BTreeSet<String>,
    in_function: bool,
    method_scope: bool,
}

impl CallableSites<'_, '_> {
    /// Python creates a fresh callable at each execution of the binding while the
    /// underlying code object belongs to one lexical lambda.
    fn place_checked_lambda(&mut self, site: TextSize, comprehension: bool) {
        if self.sites.lambdas.contains_key(&site) {
            return;
        }
        let body = self.source.lambdas[&site]
            .body
            .as_ref()
            .expect("checked lambda body");
        let definition = self.compile_callable(
            site,
            Callable::Lambda { comprehension },
            Body::Checked(body, None),
        );
        self.sites.lambdas.insert(site, definition);
    }

    fn compile_callable(
        &mut self,
        site: TextSize,
        callable: Callable<'_>,
        body: Body<'_>,
    ) -> CompiledDefinition {
        let source = self.source;
        let (name, qualname, doc, metadata, comprehension) = match callable {
            Callable::Lambda { comprehension } => (
                "<lambda>",
                format!("{}<lambda>", self.prefix),
                None,
                &source.lambdas[&site].metadata,
                comprehension,
            ),
            Callable::Ordinary(header) => {
                let qualname = if self.declared_globals.contains(&header.binding) {
                    header.name.clone()
                } else {
                    format!("{}{}", self.prefix, header.name)
                };
                let doc = if self.output.keep_docstrings {
                    header
                        .doc
                        .clone()
                        .or_else(|| source.bundled_docstring(&qualname))
                } else {
                    None
                };
                (header.name.as_str(), qualname, doc, &header.metadata, false)
            }
        };
        // A lambda can be placed by several expression regions in the same source map.
        // An ordinary definition's code address belongs to the enclosing declaration table.
        let cache = match callable {
            Callable::Lambda { .. } => Some(&source.callable_code),
            Callable::Ordinary(_) => None,
        };
        let existing = cache.and_then(|cache| cache.borrow().get(&site).cloned());
        let code = existing.unwrap_or_else(|| {
            let entry = if metadata.generator {
                Entry::Suspended(imre_ast::code_flags::GENERATOR)
            } else {
                Entry::Compiled
            };
            let code = source.definition(
                self.output,
                format!("{qualname}.<locals>."),
                body,
                site,
                true,
                entry,
            );
            if let Some(cache) = cache {
                cache.borrow_mut().insert(site, code.clone());
            }
            code
        });
        // A lambda inside a display captures the iteration namespace even in a class body.
        let method = !comprehension && self.method_scope;
        let mut flags = source.future_flags();
        if doc.is_some() {
            flags |= imre_ast::code_flags::HAS_DOCSTRING;
        }
        let signature = metadata.signature(self.in_function || comprehension, method, flags);
        self.output
            .interned_identifiers
            .extend(metadata.parameter_names().map(str::to_owned));
        CompiledDefinition {
            code,
            signature: signature.to_string(),
            name: name.to_owned(),
            qualname,
            doc,
            provider: None,
            default_names: metadata.defaults.clone(),
            type_alias: false,
            class_header: None,
        }
    }

    pub(super) fn prepare_ordinary_function(&mut self, site: TextSize) -> bool {
        let source = self.source;
        let Some((header, body)) = source.ordinary_function(site) else {
            return false;
        };
        if !self.sites.definitions.contains_key(&site) {
            let definition = self.compile_callable(site, Callable::Ordinary(header), body);
            let decorator_lines = header
                .decorator_offsets
                .iter()
                .map(|offset| source.positions.line(*offset))
                .collect();
            self.sites.definitions.insert(
                site,
                Declaration {
                    definition,
                    decorator_lines,
                    type_params: Vec::new(),
                },
            );
        }
        true
    }
}

impl Function<'_, '_> {
    fn compile_type_alias(&mut self, site: TextSize) -> Result<CompiledAlias, String> {
        let alias = &self.source.type_aliases[&site];
        // Placement can visit children of a suite that exits before the alias binds.
        let outer_diagnostic = self.frame.output.diagnostic.take();
        if let Some(parameters) = &alias.parameters {
            self.check_suspended_parameters(parameters, alias.range);
        }
        let compiled = match &alias.body {
            Ok(code) => Some(CompiledAlias {
                definition: self.compile_definition(
                    site,
                    DefinitionName::TypeAlias(&alias.name),
                    None,
                    None,
                    None,
                    Body::Operand(
                        code.range,
                        ReturnOperand::Prepared(&code.operand, self.class_locals.clone()),
                    ),
                ),
                parameters: alias.parameters.clone(),
            }),
            Err(range) => {
                self.with_expression_range(*range, |code| {
                    code.build_diagnostic("type alias value has no checked expression operand")
                });
                None
            }
        };
        match std::mem::replace(&mut self.frame.output.diagnostic, outer_diagnostic) {
            Some(error) => Err(error),
            None => Ok(compiled.expect("prepared alias evaluator")),
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
            Body::Checked(&body.body, None),
        );
        let targets = self.source.scopes.comprehension_targets[&site].clone();
        self.sites.generators.insert(
            site,
            CompiledGenerator {
                definition,
                targets,
            },
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
                    let alias = self.compile_type_alias(site);
                    self.sites.aliases.insert(site, alias);
                }
                return;
            }
            RequiredSite::Lambda(site, scoped) => {
                self.callable_sites().place_checked_lambda(site, scoped);
                return;
            }
            RequiredSite::AnnotationString(site) => {
                self.source
                    .place_annotation_string(self.frame.output, &mut self.sites, site);
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
            RequiredSite::ClassNamespace(start) => {
                if !self.sites.class_namespaces.contains_key(&start) {
                    let class = &self.source.class_bodies[&start];
                    let namespace = self.compile_class_namespace(start);
                    let parameters = class.header.parameters.clone().expect("generic class");
                    self.sites
                        .class_namespaces
                        .insert(start, (namespace, parameters, class.range));
                }
                return;
            }
            RequiredSite::Definition(start) => start,
        };
        self.prepare_definition(start);
    }

    pub(super) fn prepare_definition(&mut self, start: TextSize) {
        if self.callable_sites().prepare_ordinary_function(start)
            || self.sites.definitions.contains_key(&start)
        {
            return;
        }
        let declaration = if let Some(class) = self.source.class_bodies.get(&start) {
            let definition = if class.header.parameters.is_some() {
                let name = format!("<generic parameters of {}>", class.name);
                let body = class.header.function.as_ref().map_or_else(
                    || Body::Operand(class.name.range(), ReturnOperand::ClassArguments(start)),
                    |checked| Body::Checked(checked, None),
                );
                self.compile_definition(
                    start,
                    DefinitionName::ClassHeader(&name),
                    None,
                    None,
                    None,
                    body,
                )
            } else {
                self.compile_class_namespace(start)
            };
            Declaration {
                definition,
                decorator_lines: class
                    .decorator_offsets
                    .iter()
                    .map(|&offset| self.source.positions.line(offset))
                    .collect(),
                type_params: Vec::new(),
            }
        } else {
            let def = self
                .source
                .function_header(start)
                .or_else(|| self.source.definitions?.get(&start).copied())
                .expect("prepared function declaration");
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
            Declaration {
                definition,
                decorator_lines: def
                    .decorator_list
                    .iter()
                    .map(|d| self.source.positions.line(d.expression.start()))
                    .collect(),
                type_params: params.map_or_else(Vec::new, |params| {
                    crate::frontend::registered_type_parameters(params, &self.source.private_names)
                }),
            }
        };
        self.sites.definitions.insert(start, declaration);
    }
}

#[cfg(test)]
mod tests {
    use crate::dynamic::emit::{ModuleOrigin, Output, PreparedBody};
    use std::path::Path;

    #[test]
    fn checked_module_callables_stop_at_the_first_class() {
        use super::{CheckedSites, RequiredSite, SourceMap};
        use crate::dynamic::emit::SourcePositions;

        let text = concat!(
            "def read(value, offset=1):\n    return value + offset\n",
            "first = lambda: read(3)\n",
            "class C: pass\n",
            "last = lambda: 2\n",
        );
        let module = PreparedBody::module(
            "children",
            Path::new("children.py"),
            text,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(module.initializer().plan.fully_checked());
        let mut output = Output::default();
        let file = output.data(b"children.py");
        let location = SourceMap {
            info: &module.source,
            callable_code: Default::default(),
            definitions: Some(&module.linked_declarations()),
            unit: 0,
            positions: &SourcePositions::new(text, &file),
        };
        let mut sites = CheckedSites::default();
        let required = &module.source.initializer_sites;
        let [
            RequiredSite::Definition(read),
            RequiredSite::Lambda(first, _),
            ..,
        ] = required.as_slice()
        else {
            panic!("expected the ordinary function and lambda");
        };
        assert!(module.source.ordinary_function(*read).is_some());
        let rest = location.place_module_callables(&mut output, &mut sites, required);
        assert!(matches!(
            rest,
            [RequiredSite::Definition(_), RequiredSite::Lambda(_, _)]
        ));
        assert_eq!(sites.definitions.len(), 1);
        assert_eq!(sites.lambdas.len(), 1);
        assert_eq!(sites.definitions[read].definition.name, "read");
        assert_eq!(
            sites.lambdas[first].code,
            location.callable_code.borrow()[first]
        );
        let count = output.count;
        assert!(
            location
                .place_module_callables(&mut output, &mut sites, &required[..2])
                .is_empty()
        );
        assert_eq!(output.count, count);
    }

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
                let initializer = &module.initializer().plan;
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
                assert!(module.source.function_bodies[&start].header.is_some());
                Ok(())
            },
        )
        .unwrap();
    }

    #[test]
    fn suspended_alias_diagnostic_follows_the_emitted_binding() {
        for (suite, reachable) in [
            ("    raise Stop()\n    ALIAS", false),
            (
                "    try:\n        return\n        ALIAS\n    finally:\n        done()",
                false,
            ),
            (
                "    with manager:\n        raise Stop()\n        ALIAS",
                false,
            ),
            (
                "    try:\n        raise Stop()\n    except Stop:\n        ALIAS",
                true,
            ),
            (
                "    try:\n        return\n    finally:\n        ALIAS",
                true,
            ),
        ] {
            let source = format!("def stream():\n    yield 1\n{suite}\n").replace(
                "ALIAS",
                "type Dormant[T: int] = (item async for item in items)",
            );
            let module = PreparedBody::module(
                "stream",
                Path::new("stream.py"),
                &source,
                0,
                ModuleOrigin::Provider,
            )
            .unwrap();
            let outcome = module.standalone();
            if reachable {
                assert!(
                    outcome
                        .unwrap_err()
                        .contains("unsupported class type parameter bounds and defaults"),
                    "{suite}"
                );
            } else {
                outcome.unwrap();
            }
        }
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
