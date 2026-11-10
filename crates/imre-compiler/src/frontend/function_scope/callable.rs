//! Parameter binding and code metadata shared by lambdas and ordinary functions.

use super::FunctionScope;
use imre_ast::code_flags;
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextSize};
use serde_json::{Value, json};

pub(crate) fn parameter_layout(parameters: &ast::Parameters, layout: &mut Value) -> Vec<String> {
    let mut defaults = Vec::new();
    for (group, args) in [
        ("posonly", &parameters.posonlyargs),
        ("positional", &parameters.args),
        ("kwonly", &parameters.kwonlyargs),
    ] {
        if !args.is_empty() {
            layout[group] = json!(
                args.iter()
                    .map(|p| p.parameter.name.as_str())
                    .collect::<Vec<_>>()
            );
            defaults.extend(
                args.iter()
                    .filter(|p| p.default.is_some())
                    .map(|p| p.parameter.name.to_string()),
            );
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

pub(crate) struct OrdinaryFunction {
    /// Original spelling used by Python function metadata.
    pub(crate) name: String,
    /// Lookup and assignment use the lowered private name in class scopes.
    pub(crate) binding: String,
    pub(crate) decorator_offsets: Vec<TextSize>,
    pub(crate) doc: Option<String>,
    pub(crate) metadata: CallableMetadata,
}

impl OrdinaryFunction {
    pub(super) fn prepare(
        def: &ast::StmtFunctionDef,
        names: &imre_ast::Names,
        lexical: &FunctionScope,
        suspension_flags: u32,
    ) -> Option<Self> {
        (suspension_flags == 0
            && def.type_params.is_none()
            && def.returns.is_none()
            && def.parameters.iter().all(|p| p.annotation().is_none()))
        .then(|| Self {
            name: names.definition(&def.name).to_owned(),
            binding: def.name.to_string(),
            decorator_offsets: def
                .decorator_list
                .iter()
                .map(|d| d.expression.start())
                .collect(),
            doc: crate::frontend::docstring(&def.body),
            metadata: CallableMetadata::new(Some(&def.parameters), lexical, false),
        })
    }
}

/// Parameter and closure metadata independent of the activation that binds the function.
pub(crate) struct CallableMetadata {
    layout: serde_json::Value,
    flags: u32,
    captures_class: bool,
    pub(crate) generator: bool,
    /// Keys for the default values evaluated in order by the enclosing expression.
    pub(crate) defaults: Vec<String>,
}

impl CallableMetadata {
    pub(crate) fn new(
        parameters: Option<&ast::Parameters>,
        scope: &FunctionScope,
        generator: bool,
    ) -> Self {
        let mut layout = json!({});
        let defaults =
            parameters.map_or_else(Vec::new, |params| parameter_layout(params, &mut layout));
        let captures_class = scope.captures_class;
        scope.write_signature(&mut layout);

        let mut flags = code_flags::function_flags(parameters, false, false);
        if generator {
            flags |= code_flags::GENERATOR;
            layout["generator_lambda"] = json!(true);
        }
        if captures_class {
            layout["class_cell"] = json!(true);
        }
        Self {
            layout,
            flags,
            generator,
            captures_class,
            defaults,
        }
    }

    pub(crate) fn parameter_names(&self) -> impl Iterator<Item = &str> {
        ["posonly", "positional", "kwonly"]
            .into_iter()
            .flat_map(|group| self.layout[group].as_array().into_iter().flatten())
            .filter_map(serde_json::Value::as_str)
            .chain(
                ["varargs", "kwargs"]
                    .into_iter()
                    .filter_map(|group| self.layout[group].as_str()),
            )
    }

    pub(crate) fn signature(
        &self,
        nested: bool,
        method: bool,
        extra_flags: u32,
    ) -> serde_json::Value {
        let mut signature = self.layout.clone();
        let mut flags = self.flags | extra_flags;
        if nested {
            flags |= code_flags::NESTED;
        }
        if method {
            flags |= code_flags::METHOD;
            signature["method_class_cell"] = json!(self.captures_class);
        }
        signature["code_flags"] = json!(flags);
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
        let facts = ScopeFacts::prepare(&body, &names, |_| {}, |_| {});
        let class = body[0].as_class_def_stmt().unwrap();
        let method = class.body[0].as_function_def_stmt().unwrap();
        let ordinary = facts.function(method).ordinary.as_ref().unwrap();
        assert_eq!(ordinary.name, "__method");
        assert_eq!(ordinary.binding, "_Owner__method");
        assert_eq!(ordinary.doc.as_deref(), Some("method doc"));
        assert_eq!(ordinary.metadata.defaults, ["value", "flag"]);
        assert_eq!(
            ordinary.decorator_offsets,
            [method.decorator_list[0].expression.start()]
        );
        let signature = ordinary
            .metadata
            .signature(false, true, code_flags::HAS_DOCSTRING);
        assert_eq!(signature["class_cell"], true);
        assert_eq!(signature["method_class_cell"], true);
        let stream = facts.function(class.body[1].as_function_def_stmt().unwrap());
        assert!(stream.ordinary.is_none());
        assert_eq!(stream.suspension_flags, code_flags::ASYNC_GENERATOR);
    }
}
