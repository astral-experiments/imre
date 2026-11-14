//! Compiled code identities and operands for declarations in a registered frame.
use ruff_text_size::{TextRange, TextSize};

use super::RegisteredFrame;
use crate::ir::{RegisteredTypeParameter, TypeParameterKind};

/// The source location of a function created by an expression.
pub(crate) enum CallableSite {
    Lambda(TextSize),
    Generator(TextRange),
}

/// The body's identity is compiled once. Defaults, decorators and the defining scope
/// are the current activation's handles each time the declaration executes.
pub(crate) struct CompiledDefinition {
    pub(crate) code: String,
    pub(crate) signature: String,
    pub(crate) qualname: String,
    /// Byte offset of the unqualified name within the qualified name.
    pub(crate) name_start: usize,
    pub(crate) doc: Option<String>,
    pub(crate) provider: Option<Box<Self>>,
    pub(crate) annotation_scope: bool,
    pub(crate) default_names: std::sync::Arc<[String]>,
    pub(crate) format_evaluator: bool,
    pub(crate) class_header: Option<TextRange>,
}

impl CompiledDefinition {
    pub(crate) fn new(
        code: String,
        signature: String,
        name: &str,
        qualname: String,
        annotation_scope: bool,
    ) -> Self {
        Self {
            code,
            signature,
            name_start: qualname
                .strip_suffix(name)
                .expect("qualified name ends with its name")
                .len(),
            qualname,
            annotation_scope,
            doc: None,
            provider: None,
            default_names: Default::default(),
            format_evaluator: false,
            class_header: None,
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.qualname[self.name_start..]
    }

    pub(crate) fn instantiate_class(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        (bases, kwargs): (String, String),
        parameter_range: Option<TextRange>,
    ) -> String {
        let scope = frame.scope.clone();
        if parameter_range.is_some() {
            let base = frame.lookup(".generic_base");
            frame.op("append", &[&bases, &base]);
        }
        let namespace = self.bind_defaults(frame, &scope, &[]);
        let operation = if let Some(range) = parameter_range {
            frame.set_source_range(Some(range));
            "parameterized_class"
        } else {
            "class"
        };
        frame.op(operation, &[&kwargs, &bases, &namespace])
    }

    pub(crate) fn bind_defaults(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        scope: &str,
        defaults: &[String],
    ) -> String {
        assert_eq!(self.default_names.len(), defaults.len());
        let doc = self
            .doc
            .as_ref()
            .map_or_else(|| "0".into(), |doc| frame.text(doc));
        let annotate = self.provider.as_ref().map_or_else(
            || "0".into(),
            |provider| provider.bind_defaults(frame, scope, &[]),
        );
        let unqualified = self.name_start == 0;
        let qualname = if unqualified {
            frame.code_name(&self.qualname)
        } else {
            frame.text(&self.qualname)
        };
        let defaults_object = if self.format_evaluator || !defaults.is_empty() {
            frame.op("dict", &[])
        } else {
            "0".into()
        };
        if self.format_evaluator {
            let format = frame.text(".format");
            let value = frame.integer(1);
            frame.op("put", &[&defaults_object, &format, &value]);
        }
        for (name, value) in self.default_names.iter().zip(defaults) {
            let name = frame.code_name(name);
            frame.op("put", &[&defaults_object, &name, value]);
        }
        let signature = frame.text(&self.signature);
        let name = if unqualified {
            &qualname
        } else {
            &frame.code_name(self.name())
        };
        let code = &self.code;
        let annotation_scope = self.annotation_scope;
        frame.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {scope}, i64 {name}, i64 {signature}, i64 {defaults_object}, i64 {qualname}, i64 {doc}, i64 {annotate}, i1 {annotation_scope})"))
    }
}

pub(crate) type CompiledTypeParameters =
    Result<Vec<RegisteredTypeParameter<CompiledDefinition>>, String>;

/// The generic header evaluates its bases before it instantiates this namespace.
pub(crate) struct CompiledClassNamespace {
    pub(crate) definition: CompiledDefinition,
    pub(crate) parameters: CompiledTypeParameters,
    pub(crate) range: TextRange,
}

pub(crate) struct Declaration {
    pub(crate) definition: CompiledDefinition,
    pub(crate) type_params: Option<CompiledTypeParameters>,
}

#[derive(Clone)]
pub(crate) struct CompiledExec {
    pub(crate) entry: String,
    pub(crate) flags: u32,
    pub(crate) source: String,
}

#[derive(Default)]
pub(crate) struct CheckedSites {
    pub(crate) declarations: Vec<(TextSize, Result<Declaration, String>)>,
}

impl CheckedSites {
    /// Most declarations follow the last written site. An exception handler or a
    /// replayed suite can revisit an earlier position in the same frame.
    pub(crate) fn position(&self, site: TextSize) -> Result<usize, usize> {
        match self.declarations.last() {
            Some((last, _)) if *last == site => Ok(self.declarations.len() - 1),
            Some((last, _)) if *last > site => self
                .declarations
                .binary_search_by_key(&site, |(site, _)| *site),
            _ => Err(self.declarations.len()),
        }
    }

    fn placed(&self, site: TextSize) -> &Result<Declaration, String> {
        &self.declarations[self.position(site).expect("a placed declaration")].1
    }

    pub(crate) fn declaration(&self, site: TextSize) -> &Declaration {
        self.placed(site)
            .as_ref()
            .expect("a function or class declaration")
    }

    pub(crate) fn bind_type_alias(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        site: TextSize,
    ) -> String {
        let alias = match self.placed(site) {
            Ok(alias) => alias,
            Err(error) => {
                frame.output.diagnostic.get_or_insert_with(|| error.clone());
                return "0".into();
            }
        };
        let outer = frame.scope.clone();
        let (scope, parameters) = if let Some(params) = &alias.type_params {
            let Some(scope) = frame.bind_type_parameters(&outer, params, false) else {
                return "0".into();
            };
            let key = frame.code_name(".type_params");
            let parameters = frame.op("lookup", &[&scope, &key]);
            (scope, parameters)
        } else {
            (outer, "0".into())
        };
        let evaluator = alias.definition.bind_defaults(frame, &scope, &[]);
        let name = frame.code_name(alias.definition.name());
        frame.op("type_alias", &[&name, &evaluator, &parameters])
    }

    pub(crate) fn definition(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        site: TextSize,
        decorators: &[(String, String, TextSize)],
        defaults: &[String],
        class_arguments: Option<(String, String)>,
    ) -> String {
        let Declaration {
            definition,
            type_params,
        } = self.declaration(site);
        let scope = frame.scope.clone();
        let mut value = if let Some(arguments) = class_arguments {
            definition.instantiate_class(frame, arguments, None)
        } else if let Some(range) = definition.class_header {
            let header = definition.bind_defaults(frame, &scope, &[]);
            frame.set_source_range(Some(range));
            let class = frame.invoke(&header, &[]);
            frame.set_source_range(None);
            class
        } else {
            let scope = if let Some(params) = type_params {
                let Some(scope) = frame.bind_type_parameters(&scope, params, false) else {
                    return "0".into();
                };
                scope
            } else {
                scope
            };
            let function = definition.bind_defaults(frame, &scope, defaults);
            if type_params.is_some() {
                frame.publish_type_parameters(&scope, &function);
            }
            function
        };
        for (mark, decorator, offset) in decorators.iter().rev() {
            frame.set_statement_line(frame.positions.line(*offset));
            value = frame.invoke_from_mark(decorator, &[&value], mark, None);
        }
        value
    }
}

impl RegisteredFrame<'_, '_> {
    pub(crate) fn bind_type_parameters(
        &mut self,
        outer: &str,
        parameters: &CompiledTypeParameters,
        class: bool,
    ) -> Option<String> {
        let params = match parameters {
            Ok(params) => params,
            Err(error) => {
                self.output.diagnostic.get_or_insert_with(|| error.clone());
                return None;
            }
        };
        let scope = self.parameter_scope(outer, params, class);
        for param in params.iter().filter(|param| !param.fields.is_empty()) {
            let binding = self.code_name(&param.binding);
            let parameter = self.op("lookup", &[&scope, &binding]);
            for (field, definition) in &param.fields {
                let field = self.code_name(field.as_str());
                let evaluator = definition.bind_defaults(self, &scope, &[]);
                self.op(
                    "type_parameter_evaluator",
                    &[&parameter, &field, &evaluator],
                );
            }
        }
        Some(scope)
    }

    fn parameter_scope<F>(
        &mut self,
        outer: &str,
        params: &[RegisteredTypeParameter<F>],
        class: bool,
    ) -> String {
        let names = self.op("dict", &[]);
        for param in params {
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
}

impl RegisteredFrame<'_, '_> {
    pub(crate) fn bind_annotation_expression(&mut self, text: &str, code: &str) {
        self.output.declare(
            |d| &mut d.registered_expression,
            "declare void @imre_dyn_bind_expression(ptr, i64, ptr)\n",
        );
        self.line(&format!(
            "call void @imre_dyn_bind_expression(ptr %ctx, i64 {text}, ptr {code})"
        ));
    }
}
