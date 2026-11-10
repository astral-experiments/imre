//! Compiled code identities and operands for declarations in a registered frame.
use ruff_text_size::{TextRange, TextSize};
use std::collections::HashMap;

use super::RegisteredFrame;
use crate::ir::{DefinitionSite, RegisteredTypeParameter, TypeParameterKind};

/// The body's identity is compiled once. Defaults, decorators and the defining scope
/// are the current activation's handles each time the declaration executes.
pub(crate) struct CompiledDefinition {
    pub(crate) code: String,
    pub(crate) signature: String,
    pub(crate) name: String,
    pub(crate) qualname: String,
    pub(crate) doc: Option<String>,
    pub(crate) provider: Option<CompiledProvider>,
    pub(crate) default_names: Vec<String>,
    pub(crate) type_alias: bool,
    pub(crate) header: bool,
}

pub(crate) struct CompiledProvider {
    pub(crate) code: String,
    pub(crate) signature: String,
}

pub(crate) struct DefinitionOperands {
    doc: String,
    annotate: String,
    qualname: String,
    defaults: String,
}

impl DefinitionOperands {
    pub(crate) fn default(&self, frame: &mut RegisteredFrame<'_, '_>, name: &str, value: &str) {
        let name = frame.code_name(name);
        frame.op("put", &[&self.defaults, &name, value]);
    }
}

impl CompiledDefinition {
    pub(crate) fn doc(&self, frame: &mut RegisteredFrame<'_, '_>) -> String {
        self.doc
            .as_ref()
            .map_or_else(|| "0".into(), |s| frame.text(s))
    }

    pub(crate) fn operands(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        scope: &str,
        doc: String,
    ) -> DefinitionOperands {
        let annotate = if let Some(provider) = &self.provider {
            let signature = frame.text(&provider.signature);
            let name = frame.text("__annotate__");
            let qualname = frame.text(&format!("{}.__annotate__", self.qualname));
            let code = &provider.code;
            frame.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {scope}, i64 {name}, i64 {signature}, i64 0, i64 {qualname}, i64 0, i64 0, i1 true)"))
        } else {
            "0".into()
        };
        let qualname = frame.text(&self.qualname);
        let defaults = if self.type_alias || !self.default_names.is_empty() {
            frame.op("dict", &[])
        } else {
            "0".into()
        };
        if self.type_alias {
            let format = frame.text(".format");
            let value = frame.integer(1);
            frame.op("put", &[&defaults, &format, &value]);
        }
        DefinitionOperands {
            doc,
            annotate,
            qualname,
            defaults,
        }
    }

    pub(crate) fn bind_defaults(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        scope: &str,
        defaults: &[String],
    ) -> String {
        assert_eq!(self.default_names.len(), defaults.len());
        let doc = self.doc(frame);
        let operands = self.operands(frame, scope, doc);
        for (name, value) in self.default_names.iter().zip(defaults) {
            operands.default(frame, name, value);
        }
        self.instantiate(frame, scope, operands)
    }

    pub(crate) fn instantiate(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        scope: &str,
        operands: DefinitionOperands,
    ) -> String {
        let signature = frame.text(&self.signature);
        let name = frame.code_name(&self.name);
        let DefinitionOperands {
            doc,
            annotate,
            qualname,
            defaults,
        } = operands;
        let Self { code, header, .. } = self;
        frame.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {scope}, i64 {name}, i64 {signature}, i64 {defaults}, i64 {qualname}, i64 {doc}, i64 {annotate}, i1 {header})"))
    }
}

pub(crate) struct CompiledGenerator {
    pub(crate) definition: CompiledDefinition,
    pub(crate) targets: String,
}

pub(crate) struct CompiledAlias {
    pub(crate) definition: CompiledDefinition,
    pub(crate) parameters: Option<Vec<RegisteredTypeParameter>>,
}

impl CompiledAlias {
    pub(crate) fn instantiate(&self, frame: &mut RegisteredFrame<'_, '_>) -> String {
        let outer = frame.scope.clone();
        let (scope, parameters) = if let Some(params) = &self.parameters {
            let scope = frame.checked_parameter_scope(&outer, params, false);
            let key = frame.code_name(".type_params");
            let parameters = frame.op("lookup", &[&scope, &key]);
            (scope, parameters)
        } else {
            (outer, "0".into())
        };
        let evaluator = self.definition.bind_defaults(frame, &scope, &[]);
        let name = frame.code_name(&self.definition.name);
        frame.op("type_alias", &[&name, &evaluator, &parameters])
    }
}

pub(crate) struct Declaration {
    pub(crate) definition: CompiledDefinition,
    pub(crate) decorator_lines: Vec<usize>,
    pub(crate) type_params: Vec<RegisteredTypeParameter>,
}

pub(crate) struct AnnotationString {
    pub(crate) text: String,
    pub(crate) code: Option<String>,
}

#[derive(Default)]
pub(crate) struct CheckedSites {
    pub(crate) annotation_strings: HashMap<TextSize, AnnotationString>,
    pub(crate) definitions: HashMap<TextSize, Declaration>,
    pub(crate) lambdas: HashMap<TextSize, CompiledDefinition>,
    pub(crate) generators: HashMap<TextRange, CompiledGenerator>,
    pub(crate) aliases: HashMap<TextSize, CompiledAlias>,
    pub(crate) annotation_fields: HashMap<TextSize, (String, TextRange)>,
}

impl CheckedSites {
    pub(crate) fn annotation_string(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        site: TextSize,
    ) -> String {
        let value = &self.annotation_strings[&site];
        let text = frame.text(&value.text);
        if let Some(code) = &value.code {
            frame.bind_annotation_expression(&text, code);
        }
        text
    }

    pub(crate) fn bind_generator(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        site: TextRange,
        scope: &str,
        cursor: &str,
    ) -> String {
        let child = &self.generators[&site];
        let function = child.definition.bind_defaults(frame, scope, &[]);
        let targets = frame.text(&child.targets);
        frame.op("generator", &[&function, cursor, &targets])
    }

    pub(crate) fn bind_lambda(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        site: TextSize,
        scope: &str,
        defaults: &[String],
    ) -> String {
        self.lambdas[&site].bind_defaults(frame, scope, defaults)
    }

    pub(crate) fn definition(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        site: &DefinitionSite,
        header: &[String],
        class_arguments: Option<(String, String)>,
    ) -> String {
        let Declaration {
            definition,
            decorator_lines,
            type_params,
        } = &self.definitions[&site.start()];
        let (decorators, defaults) = header.split_at(decorator_lines.len());
        let mut scope = frame.scope.clone();
        if !type_params.is_empty() {
            scope = frame.checked_parameter_scope(&scope, type_params, false);
        }
        let mut value = definition.bind_defaults(frame, &scope, defaults);
        if !type_params.is_empty() {
            frame.publish_type_parameters(&scope, &value);
        }
        if let Some((bases, kwargs)) = class_arguments {
            value = frame.op("class", &[&kwargs, &bases, &value]);
        }
        frame.decorate(decorators, decorator_lines.iter().copied(), value)
    }
}

impl RegisteredFrame<'_, '_> {
    pub(crate) fn mark_annotation(&mut self, site: TextSize, name: &str) {
        let key = self.code_name(name);
        if let Some(sites) = &mut self.module_annotation_sites {
            let index = sites.len();
            sites.push(site);
            let index = self.integer(index);
            self.op("mark_module_annotation", &[&self.scope.clone(), &index]);
        } else {
            let offset = site.to_u32();
            self.checked(&format!("call i64 @imre_dyn_mark_class_annotation(ptr %ctx, i64 {}, i64 {key}, i32 {offset})", self.scope));
        }
    }

    pub(crate) fn checked_parameter_scope(
        &mut self,
        outer: &str,
        params: &[RegisteredTypeParameter],
        class: bool,
    ) -> String {
        let names = self.op("dict", &[]);
        for param in params {
            if param.has_bound_or_default && !self.native_suspension {
                let message = self.text("class type parameter bounds and defaults");
                self.op("unsupported", &[&message]);
            }
            let binding = self.code_name(&param.binding);
            let name = self.code_name(&param.name);
            let declaration = match param.kind {
                TypeParameterKind::TypeVar => name,
                TypeParameterKind::TypeVarTuple => self.op("class_typevartuple", &[&name]),
                TypeParameterKind::ParamSpec => self.op("class_paramspec", &[&name]),
            };
            self.op("put", &[&names, &binding, &declaration]);
        }
        let op = if class {
            "class_parameter_frame"
        } else {
            "function_parameter_scope"
        };
        self.op(op, &[outer, &names])
    }

    pub(crate) fn publish_type_parameters(&mut self, scope: &str, function: &str) {
        let key = self.code_name(".type_params");
        let params = self.op("lookup", &[scope, &key]);
        let key = self.code_name("__type_params__");
        self.op("setattr", &[function, &key, &params]);
    }

    pub(crate) fn decorate(
        &mut self,
        decorators: &[String],
        lines: impl ExactSizeIterator<Item = usize> + DoubleEndedIterator,
        mut value: String,
    ) -> String {
        for (decorator, line) in decorators.iter().zip(lines).rev() {
            self.set_statement_line(line);
            value = self.invoke(decorator, &[&value]);
        }
        value
    }
}

impl RegisteredFrame<'_, '_> {
    pub(crate) fn bind_annotation_expression(&mut self, text: &str, code: &str) {
        if !self.output.declarations.registered_expression {
            self.output
                .functions
                .push_str("declare void @imre_dyn_bind_expression(ptr, i64, ptr)\n");
            self.output.declarations.registered_expression = true;
        }
        self.line(&format!(
            "call void @imre_dyn_bind_expression(ptr %ctx, i64 {text}, ptr {code})"
        ));
    }
}
