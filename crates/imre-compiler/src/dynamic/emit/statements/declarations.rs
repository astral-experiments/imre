//! Resolve declaration and annotation code sites before emitting the checked block.
use super::super::sites::{
    AnnotationString, CheckedSites, CompiledAlias, CompiledDefinition, CompiledGenerator,
    Declaration, RequiredSite,
};
use super::super::{
    AnnotationScope, Body, DefinitionName, Entry, Function, Output, ReturnOperand, SourceInfo,
    SourceMap,
};
use super::CallableSites;
use crate::frontend::PreparedAnnotation;
use crate::frontend::function_scope::NamedFunction;
use crate::ir::{Expr, Statement};
use ruff_text_size::{Ranged, TextRange, TextSize};
use serde_json::json;

impl SourceInfo<'_> {
    pub(in crate::dynamic::emit) fn function_body(&self, site: TextSize) -> Body<'_> {
        let plan = &self.function_bodies[&site];
        if !plan.fully_checked()
            && let super::super::FunctionBody::Suite(suite) = &plan.body
        {
            Body::Suite(suite)
        } else {
            let globals = &self.scopes.functions[&site].globals;
            Body::Checked(plan.checked(), Some(globals))
        }
    }
}

enum Callable<'a> {
    Lambda { comprehension: bool },
    Named(&'a NamedFunction),
}

impl SourceMap<'_> {
    /// Place checked declarations in lexical order up to the first site that needs its frame.
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
            class_locals: None,
        };
        for (index, &site) in required.iter().enumerate() {
            match site {
                RequiredSite::Lambda(site, scoped) => callables.place_checked_lambda(site, scoped),
                RequiredSite::Alias(site) => {
                    let alias = &self.type_aliases[&site];
                    if alias.parameters.is_some() || alias.body.is_err() {
                        return &required[index..];
                    }
                    if !callables.sites.aliases.contains_key(&site) {
                        let outer_diagnostic = callables.output.diagnostic.take();
                        callables.place_alias(site, outer_diagnostic);
                    }
                }
                RequiredSite::Definition(site) => callables.place_definition(site),
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
            class_locals: self.frame.class_locals.as_ref(),
            method_scope: self.frame.class_locals.is_some()
                && self.frame.annotation_values.is_none(),
        }
    }
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
            Callable::Named(header) => {
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
        let (generic, class_cell, provider) = match callable {
            Callable::Named(header) => (
                header.generic,
                !header.generic || header.type_params.is_empty(),
                self.parameter_provider(header, &qualname),
            ),
            Callable::Lambda { .. } => (false, true, None),
        };
        let suspension = metadata.suspension_flags();
        // Several expression regions can place the same lexical lambda.
        let cache = match callable {
            Callable::Lambda { .. } => Some(&source.callable_code),
            Callable::Named(_) => None,
        };
        let existing = cache.and_then(|cache| cache.borrow().get(&site).cloned());
        let code = existing.unwrap_or_else(|| {
            let entry = match suspension {
                0 => Entry::Compiled,
                flags => Entry::Suspended(flags),
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
        let mut flags = source.function_flags(
            self.in_function || comprehension || generic,
            method && !generic,
        );
        if doc.is_some() {
            flags |= imre_ast::code_flags::HAS_DOCSTRING;
        }
        let mut signature = metadata.signature(flags, method && class_cell);
        if let Callable::Named(header) = callable {
            self.named_signature(site, header, suspension, &mut signature);
        }
        self.output
            .interned_identifiers
            .extend(metadata.parameter_names().map(str::to_owned));
        CompiledDefinition {
            doc,
            provider,
            default_names: metadata.defaults.clone(),
            ..CompiledDefinition::new(code, signature, name, qualname, false)
        }
    }

    pub(super) fn place_definition(&mut self, start: TextSize) {
        if let Some(class) = self.source.class_bodies.get(&start) {
            self.place_class(start, class);
        } else {
            self.place_named_function(start);
        }
    }

    fn place_class(&mut self, start: TextSize, class: &crate::frontend::RegisteredClass) {
        if self.sites.definitions.contains_key(&start) {
            return;
        }
        let definition = if class.header.parameters.is_some() {
            let name = format!("<generic parameters of {}>", class.name);
            let body = class.header.function.as_ref().map_or_else(
                || Body::Operand(class.name.range(), ReturnOperand::ClassArguments(start)),
                |checked| Body::Checked(checked, None),
            );
            self.compile_definition(start, DefinitionName::ClassHeader(&name), body)
        } else {
            self.compile_class_namespace(start)
        };
        let declaration = Declaration {
            definition,
            decorator_lines: class
                .decorator_offsets
                .iter()
                .map(|&offset| self.source.positions.line(offset))
                .collect(),
            type_params: Vec::new(),
        };
        self.sites.definitions.insert(start, declaration);
    }

    fn place_named_function(&mut self, site: TextSize) {
        if self.sites.definitions.contains_key(&site) {
            return;
        }
        let source = self.source;
        let header = &source.scopes.functions[&site];
        let definition =
            self.compile_callable(site, Callable::Named(header), source.function_body(site));
        let decorator_lines = header
            .decorator_offsets
            .iter()
            .map(|&offset| source.positions.line(offset))
            .collect();
        self.sites.definitions.insert(
            site,
            Declaration {
                definition,
                decorator_lines,
                type_params: header.type_params.clone(),
            },
        );
    }

    fn parameter_provider(
        &mut self,
        header: &NamedFunction,
        qualname: &str,
    ) -> Option<Box<CompiledDefinition>> {
        let source = self.source;
        let start = header.parameters;
        let annotations = source.parameter_annotations.get(&start)?;
        let class = self.class_locals.filter(|_| !source.string_annotations);
        let reads = source.scopes.annotations.parameter_reads(start, class);
        let mut metadata = json!({
            "posonly": ["format"],
            "code_flags": source.function_flags(self.in_function || header.generic, false),
            "scope_reads": reads,
            "scope_complete": true,
            "scope_locals_complete": true,
            "annotation_class": class.is_some(),
        });
        if !source.python_annotations && header.type_params.is_empty() {
            for (slot, reference) in &annotations.references {
                let root = reference.split('.').next().unwrap();
                if !class.is_some_and(|class| class.globals.contains(root)) {
                    metadata["annotation_references"][slot] = json!(reference);
                }
            }
        }
        let scope = class
            .cloned()
            .map_or(AnnotationScope::Plain, AnnotationScope::Method);
        Some(Box::new(source.annotation_provider(
            self.output,
            &annotations.body,
            scope,
            start,
            metadata,
            format!("{qualname}.__annotate__"),
        )))
    }

    fn named_signature(
        &self,
        site: TextSize,
        header: &NamedFunction,
        suspension: u32,
        signature: &mut serde_json::Value,
    ) {
        if let Some(entry) = self.source.thread_entries.get(&site) {
            match entry {
                imre_ast::ThreadEntry::Parameters { names, lock_only } => {
                    signature["thread_parameters"] = json!(names);
                    if *lock_only {
                        signature["thread_worker_locks"] = json!(names);
                    }
                }
                imre_ast::ThreadEntry::Captured(names) => {
                    signature["thread_captured_locks"] = json!(names);
                }
                imre_ast::ThreadEntry::Bound(field) => {
                    signature["thread_bound_lock_field"] = json!(field);
                }
            }
        }
        if self.source.python_annotations {
            return;
        }
        let Some(annotations) = self.source.parameter_annotations.get(&header.parameters) else {
            return;
        };
        let scalar = |slot: &str| {
            let name = annotations.references.get(slot)?;
            (!header
                .type_params
                .iter()
                .any(|param| &param.binding == name))
            .then_some(name)
        };
        if suspension == 0
            && let Some(ty) = scalar("return")
        {
            signature["return_type"] = json!(ty);
        }
        for name in header.metadata.non_variadic_parameter_names() {
            if let Some(ty) = scalar(name) {
                signature["types"][name] = json!(ty);
                if ty == "float"
                    && header
                        .integer_sign_parameters
                        .iter()
                        .any(|param| param == name)
                {
                    signature["integer_sign_parameters"][name] = json!(true);
                }
            }
        }
    }
}

impl CallableSites<'_, '_> {
    fn place_alias(&mut self, site: TextSize, outer_diagnostic: Option<String>) {
        let alias = &self.source.type_aliases[&site];
        let compiled = alias.body.as_ref().ok().map(|code| {
            let operand = ReturnOperand::Prepared(&code.operand, self.class_locals.cloned());
            CompiledAlias {
                definition: self.compile_definition(
                    site,
                    DefinitionName::TypeAlias(&alias.name),
                    Body::Operand(code.range, operand),
                ),
                parameters: alias.parameters.clone(),
            }
        });
        // A suite can exit before it binds this alias. Only the binding reports its error.
        let result = match std::mem::replace(&mut self.output.diagnostic, outer_diagnostic) {
            Some(error) => Err(error),
            None => Ok(compiled.expect("prepared alias evaluator")),
        };
        self.sites.aliases.insert(site, result);
    }
}

impl Function<'_, '_> {
    fn prepare_type_alias(&mut self, site: TextSize) {
        if self.sites.aliases.contains_key(&site) {
            return;
        }
        let alias = &self.source.type_aliases[&site];
        let outer_diagnostic = self.frame.output.diagnostic.take();
        if let Some(parameters) = &alias.parameters {
            self.check_suspended_parameters(parameters, alias.range);
        }
        if let Err(range) = &alias.body {
            self.with_expression_range(*range, |code| {
                code.build_diagnostic("type alias value has no checked expression operand")
            });
        }
        self.callable_sites().place_alias(site, outer_diagnostic);
    }

    pub(in crate::dynamic::emit) fn prepare_generator(&mut self, site: TextRange) {
        if self.sites.generators.contains_key(&site) {
            return;
        }
        let body = &self.source.generators[&site];
        let definition = self.callable_sites().compile_definition(
            site.start(),
            DefinitionName::Generator(site),
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
                self.prepare_type_alias(site);
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
                    let namespace = self.callable_sites().compile_class_namespace(start);
                    let parameters = class.header.parameters.clone().expect("generic class");
                    self.sites
                        .class_namespaces
                        .insert(start, (namespace, parameters, class.range));
                }
                return;
            }
            RequiredSite::Definition(start) => start,
        };
        self.callable_sites().place_definition(start);
    }

}

#[cfg(test)]
mod tests {
    use crate::dynamic::emit::{ModuleOrigin, Output, PreparedBody};
    use std::path::Path;

    #[test]
    fn checked_module_places_class_and_following_lambda() {
        use super::{CheckedSites, RequiredSite, SourceMap};
        use crate::dynamic::emit::SourcePositions;

        let text = concat!(
            "def read[T](value, offset=1):\n    return value + offset\n",
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
        assert!(module.initializer().fully_checked());
        let mut output = Output::default();
        let file = output.data(b"children.py");
        let location = SourceMap {
            info: &module.source,
            callable_code: Default::default(),
            unit: 0,
            positions: &SourcePositions::new(text, &file),
        };
        let mut sites = CheckedSites::default();
        let required = &module.source.initializer_sites;
        let [
            RequiredSite::Definition(read),
            RequiredSite::Lambda(first, _),
            RequiredSite::Definition(class),
            RequiredSite::Lambda(last, _),
        ] = required.as_slice()
        else {
            panic!("expected the function, lambda, class and final lambda");
        };
        let rest = location.place_module_callables(&mut output, &mut sites, required);
        assert!(rest.is_empty());
        assert_eq!(sites.definitions.len(), 2);
        assert_eq!(sites.lambdas.len(), 2);
        assert_eq!(sites.definitions[class].definition.qualname, "C");
        assert_eq!(
            sites.lambdas[last].code,
            location.callable_code.borrow()[last]
        );
        assert_eq!(sites.definitions[read].definition.name, "read");
        assert_eq!(
            sites.lambdas[first].code,
            location.callable_code.borrow()[first]
        );
        let count = output.count;
        assert!(
            location
                .place_module_callables(&mut output, &mut sites, required)
                .is_empty()
        );
        assert_eq!(output.count, count);
    }

    #[test]
    fn checked_module_places_plain_alias_before_its_following_lambda() {
        use super::{CheckedSites, RequiredSite, SourceMap};
        use crate::dynamic::emit::SourcePositions;

        for (alias, generic) in [("type Count = int", false), ("type Count[T] = T", true)] {
            let text = format!("class Entry: pass\n{alias}\nlast = lambda: Entry\n");
            let module = PreparedBody::module(
                "decls",
                Path::new("decls.py"),
                &text,
                0,
                ModuleOrigin::Program(None),
            )
            .unwrap();
            assert!(module.initializer().fully_checked());
            let mut output = Output::default();
            let file = output.data(b"decls.py");
            let location = SourceMap {
                info: &module.source,
                callable_code: Default::default(),
                unit: 0,
                positions: &SourcePositions::new(&text, &file),
            };
            let mut sites = CheckedSites::default();
            let required = &module.source.initializer_sites;
            let [
                RequiredSite::Definition(class),
                RequiredSite::Alias(alias),
                RequiredSite::Lambda(last, _),
            ] = required.as_slice()
            else {
                panic!("expected the class, alias and final lambda");
            };
            assert_eq!(alias.to_u32(), 18);
            let rest = location.place_module_callables(&mut output, &mut sites, required);
            assert_eq!(sites.definitions[class].definition.code, "@code_site0");
            if generic {
                assert!(std::ptr::eq(rest, &required[1..]));
                assert!(sites.aliases.is_empty());
                assert!(sites.lambdas.is_empty());
            } else {
                assert!(rest.is_empty());
                assert_eq!(
                    sites.aliases[alias].as_ref().unwrap().definition.code,
                    "@code_site1"
                );
                assert_eq!(sites.lambdas[last].code, "@code_site2");
                let count = output.count;
                assert!(
                    location
                        .place_module_callables(&mut output, &mut sites, required)
                        .is_empty()
                );
                assert_eq!(output.count, count);
            }
        }
    }

    #[test]
    fn checked_class_site_uses_registered_metadata_after_module_erasure() {
        let mut module = PreparedBody::module_with_syntax(
            "children",
            Path::new("children.py"),
            "class C:\n    'class doc'\nlast = lambda: C\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(module.initializer().fully_checked());
        let original = module.standalone().unwrap();
        assert!(module.syntax.take().is_some());
        assert_eq!(module.standalone().unwrap(), original);

        let class = module.source.class_bodies.values_mut().next().unwrap();
        assert_eq!(class.doc.take().as_deref(), Some("class doc"));
        assert_ne!(module.standalone().unwrap(), original);
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
                let initializer = module.initializer();
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
                assert_eq!(
                    module.source.scopes.functions[&start].type_params[0].binding,
                    "T"
                );
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
