//! Parameter binding and code-object metadata for Python callables.

use indexmap::IndexSet;
use serde::{Serialize, Serializer, ser::SerializeMap};

use super::{ClosureNames, FunctionScope, LocalNames};
use crate::frontend::{NamedBody, RegisteredBody};
use imre_ast::code_flags;
use ruff_python_ast as ast;
use ruff_python_ast::visitor::Visitor as _;
use serde_json::{Value, json};

fn parameter_layout(
    parameters: &ast::Parameters,
    layout: &mut Value,
    locals: &mut IndexSet<String>,
) -> Vec<String> {
    let mut defaults = Vec::new();
    // Frame locals and the call ABI put keyword-only parameters before variadics.
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
                locals.insert(name.to_owned());
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
            locals.insert(param.name.to_string());
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
    pub(crate) doc: Option<String>,
    pub(crate) metadata: CallableMetadata,
    pub(crate) globals: std::collections::BTreeSet<String>,
    pub(super) annotation_reads: Option<Box<super::ParameterReads>>,
    pub(crate) annotations: Option<Box<crate::frontend::ParameterAnnotations>>,
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
        child: ClosureNames,
        captures_class: bool,
        suspension_flags: u32,
        annotation_reads: Option<Box<super::ParameterReads>>,
    ) -> (Self, ClosureNames) {
        let parameters = Some(def.parameters.as_ref());
        let (metadata, mut child) =
            CallableMetadata::new(parameters, child, suspension_flags, |_| captures_class);
        let header = Self {
            name: names.definition(&def.name).to_owned(),
            binding: def.name.to_string(),
            body: None,
            doc: crate::frontend::docstring(&def.body),
            metadata,
            globals: std::mem::take(&mut child.globals),
            annotation_reads,
            annotations: None,
            type_params: Vec::new(),
        };
        (header, child)
    }

    pub(crate) fn prepare_signature(
        &mut self,
        def: &ast::StmtFunctionDef,
        thread_entry: Option<imre_ast::ThreadEntry>,
        checked: Option<&crate::frontend::RegisteredFunction>,
        bound_candidate: Option<&crate::frontend::RegisteredFunction>,
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
            None => {}
        }
        if let Some((field, selector)) =
            bound_candidate.and_then(|frame| thread_projected_call(def, frame))
        {
            signature["thread_projected_call"] = json!([field, selector]);
        }
        let Some(annotations) = self
            .annotations
            .as_ref()
            .filter(|a| !a.references.is_empty())
        else {
            return;
        };
        let scalar = |name: &str| {
            let ty = annotations.references.get(name)?;
            (!self.type_params.iter().any(|param| &param.binding == ty)).then_some(ty)
        };
        if !suspended && let Some(ty) = scalar("return") {
            signature["return_type"] = json!(ty);
        }
        // A Python protocol frame reads the caller's live annotations when it binds a scalar.
        let checked = checked
            .filter(|body| !matches!(body.inputs, crate::frontend::FrameInputs::Protocol { .. }));
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

/// Borrow one checked ordered method call from a complete, ordinary bound-method body.
/// The selector is only a lookup key; native Thread admission resolves its operation.
fn thread_projected_call<'a>(
    def: &ast::StmtFunctionDef,
    frame: &'a crate::frontend::RegisteredFunction,
) -> Option<(&'a str, &'a str)> {
    use crate::ir::{self, Builtin, Expr, ExprKind, Statement};
    if def.is_async
        || def.returns.is_some()
        || def.parameters.vararg.is_some()
        || def.parameters.kwarg.is_some()
        || !def.parameters.kwonlyargs.is_empty()
    {
        return None;
    }
    let mut positional = def
        .parameters
        .posonlyargs
        .iter()
        .chain(&def.parameters.args);
    let formal = positional.next()?.name();
    if positional.next().is_some() || formal.starts_with("__") {
        return None;
    }
    let [
        Statement::Position(..),
        Statement::Expr(Expr {
            kind: ExprKind::Invoke { callable, args },
            ..
        }),
    ] = frame.function.body.as_slice()
    else {
        return None;
    };
    if !args.values.is_empty() || !args.order.is_empty() || args.exhaustive_enum_match {
        return None;
    }
    let site = args.site.python()?;
    if site.scope.is_some() || !matches!(site.call.arguments, ir::RegisteredArguments::Positional) {
        return None;
    }
    let ir::RegisteredCallTarget::Method { name: selector, .. } = &site.call.target else {
        return None;
    };
    let ExprKind::Builtin(
        Builtin::RegisteredAttribute {
            action: ir::AttributeAction::Load,
            name: field,
            ..
        },
        operands,
    ) = &callable.without_location().kind
    else {
        return None;
    };
    let [owner] = operands.as_slice() else {
        return None;
    };
    let original = match &owner.without_location().kind {
        ExprKind::Local(slot) => frame.function.params.as_slice() == [*slot],
        ExprKind::Builtin(Builtin::RegisteredName(name), operands) => {
            operands.is_empty() && name.as_str() == formal.as_str()
        }
        _ => false,
    };
    (original && !field.id.starts_with("__")).then_some((field.id.as_str(), selector.as_str()))
}

/// Serializes a placed callable without copying its lexical scope or parameter names.
struct PlacedSignature<'a> {
    layout: &'a serde_json::Map<String, Value>,
    locals: &'a IndexSet<String>,
    flags: u32,
    class_cell: Option<bool>,
}

impl Serialize for PlacedSignature<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut object = serializer.serialize_map(None)?;
        let mut fields = self.layout.iter().peekable();
        let flags = Value::from(self.flags);
        let class_cell = self.class_cell.map(Value::Bool);
        // The ordinary JSON object and these two replacements are in key order.
        for (key, value) in std::iter::once(("code_flags", &flags)).chain(
            class_cell
                .as_ref()
                .map(|value| ("method_class_cell", value)),
        ) {
            while let Some((name, field)) = fields.next_if(|(name, _)| name.as_str() < key) {
                object.serialize_entry(name, field)?;
            }
            fields.next_if(|(name, _)| name.as_str() == key);
            object.serialize_entry(key, value)?;
        }
        if !self.locals.is_empty() {
            while let Some((name, field)) =
                fields.next_if(|(name, _)| name.as_str() < "scope_locals")
            {
                object.serialize_entry(name, field)?;
            }
            object.serialize_entry("scope_locals", &LocalNames(self.locals))?;
        }
        for (name, field) in fields {
            object.serialize_entry(name, field)?;
        }
        object.end()
    }
}

/// Parameter and closure metadata independent of the activation that binds the function.
pub(crate) struct CallableMetadata {
    layout: serde_json::Value,
    pub(crate) locals: IndexSet<String>,
    flags: u32,
    captures_class: bool,
    /// Keys for the default values evaluated in order by the enclosing expression.
    pub(crate) defaults: Box<[String]>,
}

impl CallableMetadata {
    fn new(
        parameters: Option<&ast::Parameters>,
        mut names: ClosureNames,
        suspension_flags: u32,
        captures_class: impl FnOnce(&ClosureNames) -> bool,
    ) -> (Self, ClosureNames) {
        let mut layout = json!({});
        let defaults = parameters.map_or_else(Vec::new, |params| {
            parameter_layout(params, &mut layout, &mut names.locals)
        });
        let mut scope = FunctionScope::new(names);
        scope.captures_class = captures_class(&scope.names);
        if scope.captures_class {
            layout["class_cell"] = json!(true);
        }
        let metadata = Self::from_scope(
            layout,
            defaults,
            &mut scope,
            code_flags::function_flags(parameters, false, false) | suspension_flags,
        );
        (metadata, scope.names)
    }

    /// Generator expressions receive the iterator acquired by their enclosing frame.
    pub(crate) fn generator(scope: &mut FunctionScope, is_async: bool) -> Self {
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
        scope: &mut FunctionScope,
        flags: u32,
    ) -> Self {
        // Callable placement borrows the same ordered names for lookup and serialization.
        let locals = std::mem::take(&mut scope.names.locals);
        scope.write_signature(&mut layout);
        Self {
            layout,
            locals,
            flags,
            captures_class: scope.captures_class,
            defaults: defaults.into(),
        }
    }

    pub(crate) fn lambda(
        expr: &ast::ExprLambda,
        string_annotations: bool,
        generator: bool,
    ) -> Self {
        let flags = if generator { code_flags::GENERATOR } else { 0 };
        let mut names = ClosureNames {
            include_nested_reads: true,
            implicit_class: true,
            string_annotations,
            ..ClosureNames::default()
        };
        names.visit_expr(&expr.body);
        let (mut metadata, _) = Self::new(expr.parameters.as_deref(), names, flags, |names| {
            names.reads.contains("__class__")
        });
        if generator {
            metadata.layout["generator_lambda"] = json!(true);
        }
        metadata
    }

    pub(crate) fn suspension_flags(&self) -> u32 {
        self.flags & (code_flags::GENERATOR | code_flags::COROUTINE | code_flags::ASYNC_GENERATOR)
    }

    /// Fast local stores can take effect as soon as a flat pattern has matched.
    pub(crate) fn is_fast_local(&self, name: &str) -> bool {
        self.locals.contains(name)
            && self.layout["scope_cellvars"]
                .as_array()
                .is_some_and(|cells| cells.iter().all(|cell| cell.as_str() != Some(name)))
    }

    pub(super) fn reads(&self) -> impl Iterator<Item = &str> {
        self.layout["scope_reads"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
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

    pub(crate) fn serialized_signature(
        &self,
        placement_flags: u32,
        method_class_cell: bool,
    ) -> String {
        let signature = PlacedSignature {
            layout: self
                .layout
                .as_object()
                .expect("callable signature is an object"),
            locals: &self.locals,
            flags: self.flags | placement_flags,
            class_cell: method_class_cell.then_some(self.captures_class),
        };
        serde_json::to_string(&signature).expect("callable signature serializes")
    }

    #[cfg(test)]
    pub(crate) fn signature(&self, placement_flags: u32, method_class_cell: bool) -> Value {
        serde_json::from_str(&self.serialized_signature(placement_flags, method_class_cell))
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::super::ScopeFacts;
    use imre_ast::code_flags;
    use ruff_text_size::Ranged;

    #[test]
    fn placed_signatures_preserve_the_checked_fields_between_placements() {
        for layout in [
            serde_json::json!({}),
            serde_json::json!({
                "code_flags": 100, "method_class_cell": true,
                "positional": ["x", "λ"], "scope_locals": {"x": [0, 1]}
            }),
        ] {
            let metadata = super::CallableMetadata {
                layout: layout.clone(),
                locals: Default::default(),
                flags: 2,
                captures_class: false,
                defaults: Default::default(),
            };
            for method in [true, false] {
                let mut expected = layout.clone();
                expected["code_flags"] = serde_json::json!(6);
                if method {
                    expected["method_class_cell"] = serde_json::json!(false);
                }
                assert_eq!(
                    metadata.serialized_signature(4, method),
                    expected.to_string()
                );
            }
            assert_eq!(metadata.layout, layout);
        }
    }

    #[test]
    fn class_reads_use_the_completed_method_scope() {
        let mut body = ruff_python_parser::parse_module(concat!(
            "class Owner:\n",
            "    def read[T, V: T = T](self, value: T = default) -> lambda: T:\n",
            "        local: ignored\n",
            "        return T, outside\n",
            "    def indirect(self, captured, U):\n",
            "        @decorate(D)\n",
            "        class Child[U, D](base[U]):\n",
            "            value = U, through_class, captured\n",
            "        return Child\n",
        ))
        .unwrap()
        .into_syntax()
        .body;
        let names = imre_ast::lower(&mut body);
        let (facts, _) = ScopeFacts::prepare(&body, &names, |_, _| {}, |_| {});
        let class = body[0].as_class_def_stmt().unwrap();
        let indirect = facts.function(class.body[1].as_function_def_stmt().unwrap());
        assert_eq!(indirect.metadata.layout["scope_complete"], true);
        assert_eq!(
            indirect.metadata.layout["scope_cellvars"],
            serde_json::json!(["captured"])
        );
        let reads = &facts.classes[&body[0].start()].namespace.names.reads;
        assert_eq!(
            reads.iter().map(String::as_str).collect::<Vec<_>>(),
            [
                "D",
                "base",
                "decorate",
                "default",
                "outside",
                "through_class"
            ]
        );
    }

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
