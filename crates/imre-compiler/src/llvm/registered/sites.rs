//! Compiled code identities and operands for declarations in a registered frame.
use ruff_text_size::TextSize;
use std::collections::HashMap;

use super::RegisteredFrame;
use crate::ir::{ModuleAnnotation, RegisteredTypeParameter, TypeParameterKind};

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
    pub(crate) class: bool,
}

/// Only compiled identities, physical decorator positions, and literal operands cross
/// from source preparation to the ordinary frame.
#[derive(Default)]
pub(crate) struct CheckedSites {
    pub(crate) definitions: HashMap<TextSize, Declaration>,
    pub(crate) lambdas: HashMap<TextSize, CompiledDefinition>,
    pub(crate) aliases: HashMap<TextSize, CompiledAlias>,
}

impl CheckedSites {
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
        site: TextSize,
        header: &[String],
    ) -> String {
        let Declaration {
            definition,
            decorator_lines,
            class,
        } = &self.definitions[&site];
        let (decorators, defaults) = header.split_at(decorator_lines.len());
        let bases = class.then(|| frame.op("list", &[]));
        let scope = frame.scope.clone();
        let mut value = definition.bind_defaults(frame, &scope, defaults);
        if let Some(bases) = bases {
            value = frame.op("class", &["0", &bases, &value]);
        }
        frame.decorate(decorators, decorator_lines.iter().copied(), value)
    }
}

impl RegisteredFrame<'_, '_> {
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

    pub(crate) fn module_annotation(&mut self, annotation: &ModuleAnnotation) {
        match annotation {
            ModuleAnnotation::Deferred { site, name } => {
                self.code_name(name);
                let sites = self
                    .module_annotation_sites
                    .as_mut()
                    .expect("module annotation provider");
                // A finally suite can emit the same declaration on several independent paths.
                let index = sites.len();
                sites.push(*site);
                let index = self.integer(index);
                self.op("mark_module_annotation", &[&self.scope.clone(), &index]);
            }
            ModuleAnnotation::String { name, spelling } => {
                let name = self.code_name(name);
                let value = self.text(spelling);
                self.op("annotate", &[&self.scope.clone(), &name, &value]);
            }
        }
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
