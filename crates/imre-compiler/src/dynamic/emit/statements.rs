use crate::frontend::{ExpressionInput, PhysicalOperand, SourceEffect, SourceOperand};
use crate::llvm::registered::{
    PreparedSuiteEmitter, emit_operand, emit_physical_operand, emit_prepared_suite,
};
mod declarations;
use super::sites::{CheckedSites, CompiledDefinition};

use super::{AnnotationScope, FrameEmitter, Output, RegisteredFrame, SourceMap};
use crate::llvm::registered::context::{Finally, RegionEmitter};
use ruff_text_size::TextSize;
use serde_json::json;

use super::{Body, DefinitionName, Function};

/// The enclosing lexical scope and the existing address table used to place checked callables.
pub(in crate::dynamic::emit) struct CallableSites<'a, 'p> {
    source: &'a SourceMap<'p>,
    output: &'a mut Output,
    sites: &'a mut CheckedSites,
    prefix: &'a str,
    declared_globals: &'a std::collections::BTreeSet<String>,
    in_function: bool,
    method_scope: bool,
    class_locals: Option<&'a crate::frontend::function_scope::ClassBindings>,
}

impl<'a, 'p> crate::llvm::registered::PreparedSuiteEmitter<'a, 'p> for Function<'a, 'p> {
    fn frame_and_sites(
        &mut self,
        operand: Option<&crate::ir::Expr>,
    ) -> (&mut RegisteredFrame<'a, 'p>, &CheckedSites) {
        if let Some(operand) = operand {
            self.callable_sites().place_operand(operand);
        }
        (&mut self.frame, &self.sites)
    }

    fn place_definition(&mut self, site: TextSize) {
        self.callable_sites().place_definition(site);
    }

    fn place_statements(&mut self, body: &[crate::ir::Statement]) {
        self.callable_sites().place_statements(body);
    }

    fn emit_effect(
        &mut self,
        effect: &'p SourceEffect,
    ) -> Option<(&'p crate::ir::Statement, &'p [PhysicalOperand])> {
        match effect {
            SourceEffect::Match(range, site) => {
                self.checkpoint(range);
                self.simple_call = None;
                self.prepared_match(site);
            }
            SourceEffect::Region(region) => {
                crate::llvm::registered::Frame::emit_region(self, region)
            }
            SourceEffect::Value(statement, inputs) => {
                return Some((statement, inputs));
            }
        }
        None
    }
}

impl<'a, 'p> RegionEmitter<'a, 'p> for Function<'a, 'p> {
    fn emit_finally(&mut self, suite: &Finally<'p>) {
        // Checked final suites are nested inside the checked run that owns their bindings.
        let Finally::Prepared(suite) = suite else {
            unreachable!("checked finally outside its checked run")
        };
        emit_prepared_suite(self, suite);
    }
}

impl SourceMap<'_> {
    pub(in crate::dynamic::emit) fn finish_class_annotations(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        prefix: &str,
        in_function: bool,
        plan: &crate::frontend::ClassAnnotations,
        globals: &std::collections::BTreeSet<String>,
    ) {
        let Some(body) = &plan.body else {
            return;
        };
        if frame.body.terminated {
            return;
        }
        let offsets = if plan.sites.iter().any(|site| site.index.is_some()) {
            let offsets = frame.op("list", &[]);
            for site in plan.sites.iter().filter(|site| site.index.is_some()) {
                let offset = frame.integer(site.site.to_u32());
                frame.op("append", &[&offsets, &offset]);
            }
            offsets
        } else {
            "0".into()
        };
        let signature = json!({
            "posonly": ["format"], "scope_complete": !plan.incomplete, "scope_reads": plan.reads,
            "code_flags": self.function_flags(in_function, false),
        });
        let scope = AnnotationScope::Class {
            globals: globals.clone(),
            conditional: plan.sites.iter().any(|entry| entry.index.is_some()),
        };
        let compiled = self.annotation_provider(
            frame.output,
            body,
            scope,
            plan.sites[0].site,
            signature,
            format!("{prefix}__annotate__"),
        );
        let environment = frame.op(
            "class_annotation_environment",
            &[&frame.scope.clone(), &offsets],
        );
        let function = compiled.bind_defaults(frame, &environment, &[]);
        frame.bind("__annotate_func__", &function);
    }

    fn annotation_provider(
        &self,
        output: &mut Output,
        body: &crate::frontend::AnnotationProvider,
        scope: AnnotationScope,
        start: TextSize,
        signature: serde_json::Value,
        qualname: String,
    ) -> CompiledDefinition {
        // Class annotations already have an explicit class environment. A method's provider
        // instead captures the class namespace of the function it annotates.
        let annotation_scope = !matches!(&scope, AnnotationScope::Class { .. });
        let code = self.code(
            output,
            String::new(),
            Body::CheckedProvider(body, scope),
            false,
            Some((start, 0)),
        );
        CompiledDefinition::new(code, signature, "__annotate__", qualname, annotation_scope)
    }

    /// Publish a module's provider before its first statement. Every simple
    /// annotation, including an unconditional one, becomes visible only after
    /// execution reaches its assignment.
    pub(super) fn define_module_annotations(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        sites: &[TextSize],
    ) {
        let body = self.module_annotations.module_body(sites);
        let compiled = self.annotation_provider(
            frame.output,
            &body,
            AnnotationScope::Plain,
            self.module_start,
            json!({
                "posonly": ["format"], "scope_complete": true, "scope_locals_complete": true,
                "code_flags": self.function_flags(false, false),
            }),
            "__annotate__".into(),
        );
        let scope = frame.scope.clone();
        let provider = compiled.bind_defaults(frame, &scope, &[]);
        frame.bind("__annotate__", &provider);
    }
}

impl<'p> Function<'_, 'p> {
    pub(super) fn declare_globals(&mut self, globals: &std::collections::BTreeSet<String>) {
        for name in globals {
            self.declared_globals.insert(name.to_owned());
            self.frame.declare_global(name);
        }
    }

    fn branch_input(&mut self, input: ExpressionInput<'_>, yes: &str, no: &str) {
        let bit = self
            .test_with_temporaries(|code| emit_operand(code, SourceOperand::Residual(input), true));
        self.branch(&bit, yes, no);
    }

    pub(super) fn physical_operand(&mut self, input: &PhysicalOperand) -> String {
        emit_physical_operand(self, input)
    }

    fn prepared_match(&mut self, site: &'p crate::frontend::SourceMatch) {
        use crate::llvm::registered::patterns::MatchRegion;

        let mark = self.temporary_mark();
        let subject = self.physical_operand(&site.subject);
        let mut regions = self
            .frame
            .registered_match(&subject, &mark, site, &self.sites);
        while let Some(region) = regions.next(&mut self.frame) {
            match region {
                MatchRegion::Guard {
                    case,
                    matched,
                    failed,
                } => match site.cases[case].guard.as_ref().expect("match guard") {
                    PhysicalOperand::Checked(operand) => {
                        let (frame, sites) = self.frame_and_sites(Some(&operand.value));
                        frame.prepared_branch(operand, sites, matched, failed);
                    }
                    PhysicalOperand::Residual(input) => {
                        self.branch_input(input.as_input(), matched, failed)
                    }
                },
                MatchRegion::Body(case) => emit_prepared_suite(self, &site.cases[case].body),
            }
        }
    }
}

impl CallableSites<'_, '_> {
    pub(in crate::dynamic::emit) fn compile_class_namespace(
        &mut self,
        site: TextSize,
    ) -> CompiledDefinition {
        self.compile_definition(
            site,
            DefinitionName::Binding(&self.source.class_bodies[&site].name),
            Body::Namespace,
        )
    }

    fn compile_definition(
        &mut self,
        start: TextSize,
        name: DefinitionName<'_>,
        body: Body<'_>,
    ) -> CompiledDefinition {
        let (evaluator_scope, type_parameter_scope) = match name {
            DefinitionName::Evaluator {
                scope,
                type_parameter_scope,
                ..
            } => (Some(scope), type_parameter_scope),
            _ => (None, false),
        };
        let class = matches!(name, DefinitionName::ClassHeader(_))
            .then(|| &self.source.class_bodies[&start]);
        let (name, global) = match name {
            DefinitionName::Binding(identifier) => (
                self.source.private_names.definition(identifier),
                self.declared_globals.contains(identifier.as_str()),
            ),
            DefinitionName::ClassHeader(name) | DefinitionName::Evaluator { name, .. } => {
                (name, false)
            }
        };
        let qualname = if global {
            name.to_owned()
        } else {
            format!("{}{name}", self.prefix)
        };
        let class_header = class.map(|class| class.range);
        let prefix = if let Some(class) = class {
            if self.declared_globals.contains(class.name.as_str()) {
                String::new()
            } else {
                self.prefix.to_owned()
            }
        } else if matches!(body, Body::Namespace) {
            format!("{qualname}.")
        } else {
            format!("{qualname}.<locals>.")
        };
        let mut signature = json!({});
        if !matches!(body, Body::Namespace) {
            signature["code_flags"] = json!(
                self.source
                    .function_flags(self.in_function || type_parameter_scope, false)
            );
        }
        let scopes = &self.source.scopes;
        let scope_facts = match &body {
            Body::Namespace => {
                let class = &scopes.classes[&start];
                signature["postponed_annotations"] = json!(class.postponed_annotations);
                signature["needs_class_cell"] = json!(class.needs_class_cell);
                signature["needs_dict_cell"] = json!(class.needs_dict_cell);
                signature["static_attributes"] = json!(class.static_attributes);
                signature["class_frame_reads"] = json!(class.frame_reads);
                if !class.bindings.globals.is_empty() {
                    signature["class_globals"] = json!(class.bindings.globals);
                }
                Some(&class.namespace)
            }
            _ => evaluator_scope,
        };
        if let Some(facts) = scope_facts {
            facts.write_signature(&mut signature);
        } else {
            signature["scope_cellvars"] = serde_json::Value::Null;
            signature["scope_complete"] = json!(false);
            signature["scope_locals_complete"] = json!(true);
        }
        if evaluator_scope.is_some() {
            signature["posonly"] = json!([".format"]);
            if matches!(body, Body::Operand(_, _, Some(_))) {
                signature["annotation_class"] = json!(true);
            }
        }
        let in_function = self.in_function || !matches!(body, Body::Namespace);
        let code = self
            .source
            .code(self.output, prefix, body, in_function, Some((start, 0)));
        CompiledDefinition {
            class_header,
            format_evaluator: evaluator_scope.is_some(),
            ..CompiledDefinition::new(
                code,
                signature,
                name,
                qualname,
                class_header.is_some() || evaluator_scope.is_some(),
            )
        }
    }
}
