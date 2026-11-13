//! Parameter binding and code-object metadata for Python callables.

use super::FunctionScope;
use crate::frontend::{NamedBody, RegisteredBody};
use imre_ast::code_flags;
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextSize};
use serde_json::{Value, json};

fn parameter_layout(parameters: &ast::Parameters, layout: &mut Value) -> Vec<String> {
    let mut defaults = Vec::new();
    for (group, args) in [
        ("posonly", &parameters.posonlyargs),
        ("positional", &parameters.args),
        ("kwonly", &parameters.kwonlyargs),
    ] {
        if !args.is_empty() {
            let mut names = Vec::with_capacity(args.len());
            for parameter in args {
                let name = parameter.parameter.name.as_str();
                if parameter.default.is_some() {
                    defaults.push(name.to_owned());
                }
                names.push(Value::from(name));
            }
            layout[group] = Value::Array(names);
        }
    }
    for (key, param) in [
        ("varargs", &parameters.vararg),
        ("kwargs", &parameters.kwarg),
    ] {
        if let Some(param) = param {
            layout[key] = json!(param.name.as_str());
        }
    }

    defaults
}

pub(crate) struct NamedFunction<'p> {
    /// Selected after lexical children have been prepared.
    pub(crate) body: Option<NamedBody<'p>>,
    /// Original spelling used by Python function metadata.
    pub(crate) name: String,
    /// Lookup and assignment use the lowered private name in class scopes.
    pub(crate) binding: String,
    pub(crate) decorator_offsets: Vec<TextSize>,
    pub(crate) doc: Option<String>,
    pub(crate) metadata: CallableMetadata,
    pub(crate) globals: std::collections::BTreeSet<String>,
    pub(crate) annotations: Option<Box<crate::frontend::ParameterAnnotations>>,
    pub(crate) generic: bool,
    pub(crate) type_params:
        Vec<crate::ir::RegisteredTypeParameter<crate::frontend::ParameterEvaluator>>,
}

impl NamedFunction<'_> {
    pub(crate) fn body(&self) -> RegisteredBody<'_> {
        self.body
            .as_ref()
            .expect("prepared named function")
            .as_ref()
    }

    pub(super) fn prepare(
        def: &ast::StmtFunctionDef,
        names: &imre_ast::Names,
        lexical: &mut FunctionScope,
        suspension_flags: u32,
    ) -> Self {
        Self {
            name: names.definition(&def.name).to_owned(),
            binding: def.name.to_string(),
            body: None,
            decorator_offsets: def
                .decorator_list
                .iter()
                .map(|d| d.expression.start())
                .collect(),
            doc: crate::frontend::docstring(&def.body),
            metadata: CallableMetadata::new(Some(&def.parameters), lexical, suspension_flags),
            globals: std::mem::take(&mut lexical.names.globals),
            annotations: None,
            generic: def.type_params.is_some(),
            type_params: Vec::new(),
        }
    }

    pub(crate) fn prepare_signature(
        &mut self,
        def: &ast::StmtFunctionDef,
        thread_entry: Option<imre_ast::ThreadEntry>,
        python_annotations: bool,
        checked: Option<&crate::frontend::RegisteredFunction>,
    ) {
        let suspended = self.metadata.suspension_flags() != 0;
        let signature = &mut self.metadata.layout;
        match thread_entry {
            Some(imre_ast::ThreadEntry::Parameters { lock_only }) => {
                signature["thread_lock_only"] = json!(lock_only);
            }
            Some(imre_ast::ThreadEntry::Captured(names)) => {
                signature["thread_captured_locks"] = json!(names);
            }
            Some(imre_ast::ThreadEntry::Bound(field)) => {
                signature["thread_bound_lock_field"] = json!(field);
            }
            None => {}
        }
        let Some(annotations) = self.annotations.as_ref().filter(|_| !python_annotations) else {
            return;
        };
        let scalar = |name: &str| {
            let ty = annotations.references.get(name)?;
            (!self.type_params.iter().any(|param| &param.binding == ty)).then_some(ty)
        };
        if !suspended && let Some(ty) = scalar("return") {
            signature["return_type"] = json!(ty);
        }
        for (position, parameter) in def.parameters.iter_non_variadic_params().enumerate() {
            let name = parameter.parameter.name.as_str();
            if let Some(ty) = scalar(name) {
                let builtin = checked.and_then(|body| {
                    let &slot = body.function.params.get(position)?;
                    let binding = body.local_names.get(&slot)?;
                    if binding.namespace.is_some() || binding.name != name {
                        return None;
                    }
                    body.function.locals[slot].builtin_scalar_name()
                });
                signature["types"][name] = if builtin == Some(ty.as_str()) {
                    json!({ "builtin": ty })
                } else {
                    json!(ty)
                };
                if ty == "float" && crate::frontend::only_compares_zero(&def.body, name) {
                    signature["integer_sign_parameters"][name] = json!(true);
                }
            }
        }
    }
}

/// Parameter and closure metadata independent of the activation that binds the function.
pub(crate) struct CallableMetadata {
    layout: serde_json::Value,
    flags: u32,
    captures_class: bool,
    /// Keys for the default values evaluated in order by the enclosing expression.
    pub(crate) defaults: std::sync::Arc<[String]>,
}

impl CallableMetadata {
    pub(crate) fn new(
        parameters: Option<&ast::Parameters>,
        scope: &FunctionScope,
        suspension_flags: u32,
    ) -> Self {
        let mut layout = json!({});
        let defaults =
            parameters.map_or_else(Vec::new, |params| parameter_layout(params, &mut layout));
        if scope.captures_class {
            layout["class_cell"] = json!(true);
        }
        Self::from_scope(
            layout,
            defaults,
            scope,
            code_flags::function_flags(parameters, false, false) | suspension_flags,
        )
    }

    /// Generator expressions receive the iterator acquired by their enclosing frame.
    pub(crate) fn generator(scope: &FunctionScope, is_async: bool) -> Self {
        let flags = if is_async {
            code_flags::ASYNC_GENERATOR
        } else {
            code_flags::GENERATOR
        };
        Self::from_scope(json!({"positional": [".0"]}), Vec::new(), scope, flags)
    }

    fn from_scope(
        mut layout: Value,
        defaults: Vec<String>,
        scope: &FunctionScope,
        flags: u32,
    ) -> Self {
        scope.write_signature(&mut layout);
        Self {
            layout,
            flags,
            captures_class: scope.captures_class,
            defaults: if defaults.is_empty() {
                Default::default()
            } else {
                defaults.into()
            },
        }
    }

    pub(crate) fn lambda(
        parameters: Option<&ast::Parameters>,
        scope: &FunctionScope,
        generator: bool,
    ) -> Self {
        let flags = if generator { code_flags::GENERATOR } else { 0 };
        let mut metadata = Self::new(parameters, scope, flags);
        if generator {
            metadata.layout["generator_lambda"] = json!(true);
        }
        metadata
    }

    pub(crate) fn suspension_flags(&self) -> u32 {
        self.flags & (code_flags::GENERATOR | code_flags::COROUTINE | code_flags::ASYNC_GENERATOR)
    }

    pub(crate) fn local_names(&self) -> &[Value] {
        self.layout["scope_locals"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn non_variadic_parameter_names(&self) -> impl Iterator<Item = &str> {
        ["posonly", "positional", "kwonly"]
            .into_iter()
            .flat_map(|group| self.layout[group].as_array().into_iter().flatten())
            .filter_map(serde_json::Value::as_str)
    }

    pub(crate) fn parameter_names(&self) -> impl Iterator<Item = &str> {
        self.non_variadic_parameter_names().chain(
            ["varargs", "kwargs"]
                .into_iter()
                .filter_map(|group| self.layout[group].as_str()),
        )
    }

    pub(crate) fn signature(
        &self,
        placement_flags: u32,
        method_class_cell: bool,
    ) -> serde_json::Value {
        let mut signature = self.layout.clone();
        if method_class_cell {
            signature["method_class_cell"] = json!(self.captures_class);
        }
        signature["code_flags"] = json!(self.flags | placement_flags);
        signature
    }
}

#[cfg(test)]
mod tests {
    use super::super::ScopeFacts;
    use imre_ast::code_flags;
    use ruff_text_size::Ranged;

    #[test]
    fn private_method_keeps_its_source_name_and_class_cell() {
        let mut body = ruff_python_parser::parse_module(concat!(
            "class Owner:\n",
            "    @decorate\n",
            "    def __method(self, value=first(), /, *, flag=second()):\n",
            "        'method doc'\n",
            "        return __class__\n",
            "    async def stream(self):\n",
            "        yield self\n",
        ))
        .unwrap()
        .into_syntax()
        .body;
        let names = imre_ast::lower(&mut body);
        let (facts, _) = ScopeFacts::prepare(&body, &names, |_, _| {}, |_| {});
        let class = body[0].as_class_def_stmt().unwrap();
        let method = class.body[0].as_function_def_stmt().unwrap();
        let ordinary = facts.function(method);
        assert_eq!(ordinary.name, "__method");
        assert_eq!(ordinary.binding, "_Owner__method");
        assert_eq!(ordinary.doc.as_deref(), Some("method doc"));
        assert_eq!(ordinary.metadata.defaults.as_ref(), ["value", "flag"]);
        assert_eq!(
            ordinary.decorator_offsets,
            [method.decorator_list[0].expression.start()]
        );
        let signature = ordinary
            .metadata
            .signature(code_flags::METHOD | code_flags::HAS_DOCSTRING, true);
        assert_eq!(signature["class_cell"], true);
        assert_eq!(signature["method_class_cell"], true);
        let stream = facts.function(class.body[1].as_function_def_stmt().unwrap());
        assert_eq!(stream.name, "stream");
        assert_eq!(
            stream.metadata.suspension_flags(),
            code_flags::ASYNC_GENERATOR
        );
    }
}
