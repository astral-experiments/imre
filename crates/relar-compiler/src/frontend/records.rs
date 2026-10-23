//! Resolves sealed class descriptors into ordinary functions and field accesses.

use super::*;
use crate::classes::{ContainerFactory, FieldDefault, MethodKind};

#[derive(Clone, Copy)]
pub(super) struct MethodContext {
    pub class: usize,
    pub index: usize,
}

impl MethodContext {
    pub(super) fn kind(self, classes: &Classes) -> MethodKind {
        classes.definitions[self.class].method_kinds[self.index]
    }
}

/// A class method has a statically bound class, so its native function needs no
/// runtime receiver. The original declaration remains available for diagnostics.
pub(super) fn executable_method(
    method: &ast::StmtFunctionDef,
    context: MethodContext,
    classes: &Classes,
    graph: &SourceGraph,
    types: &Types,
) -> Result<ast::StmtFunctionDef, String> {
    let mut method = method.clone();
    method.decorator_list.clear();
    if context.kind(classes) == MethodKind::Class {
        let receiver = if method.parameters.posonlyargs.is_empty() {
            method.parameters.args.remove(0)
        } else {
            method.parameters.posonlyargs.remove(0)
        };
        let module = classes.definitions[context.class].definition.module;
        let source = &graph.modules[module].source;
        if receiver.default.is_some() {
            return Err(error(
                source,
                &receiver,
                "a class receiver cannot have a default",
            ));
        }
        if let Some(annotation) = receiver.annotation() {
            class_receiver_annotation(annotation, context.class, classes, graph, types, source)?;
        }
    }
    Ok(method)
}

/// Generated initializer locals must not shadow module imports or builtin
/// operations. Public keyword labels stay in `Signature`; only this executable
/// AST receives private names, so a field called `len` remains usable.
pub(super) fn private_initializer_parameters(
    method: &mut ast::StmtFunctionDef,
    module: &crate::modules::SourceModule,
) {
    let mut names = module.bindings.keys().cloned().collect::<HashSet<_>>();
    let mut renamed = HashMap::new();
    let mut next = 0;
    for parameter in method
        .parameters
        .args
        .iter_mut()
        .chain(&mut method.parameters.kwonlyargs)
    {
        let name = loop {
            let name = format!("__relar_parameter_{next}");
            next += 1;
            if names.insert(name.clone()) {
                break name;
            }
        };
        renamed.insert(parameter.name().to_string(), name.clone());
        parameter.parameter.name = ast::Identifier::new(name, parameter.parameter.name.range());
    }
    for statement in &mut method.body {
        let ast::Stmt::Assign(assignment) = statement else {
            continue;
        };
        let ast::Expr::Name(value) = &mut *assignment.value else {
            unreachable!("generated fields copy parameters")
        };
        value.id = renamed[value.id.as_str()].clone().into();
        for target in &mut assignment.targets {
            let ast::Expr::Attribute(attribute) = target else {
                unreachable!("generated assignments target fields")
            };
            let ast::Expr::Name(receiver) = &mut *attribute.value else {
                unreachable!("generated fields use their receiver")
            };
            receiver.id = renamed[receiver.id.as_str()].clone().into();
        }
    }
}

/// Class receivers use a compile-time type object, including quoted annotations.
fn class_receiver_annotation(
    expression: &ast::Expr,
    class: usize,
    classes: &Classes,
    graph: &SourceGraph,
    types: &Types,
    source: &str,
) -> Result<(), String> {
    if let ast::Expr::StringLiteral(value) = expression {
        check_string(value, source)?;
        let contents = value.value.to_str();
        let parsed = ruff_python_parser::parse_expression(contents).map_err(|_| {
            error(
                source,
                expression,
                "invalid quoted class receiver annotation",
            )
        })?;
        return class_receiver_annotation(parsed.expr(), class, classes, graph, types, contents)
            .map_err(|message| error(source, expression, &message));
    }
    let module = classes.definitions[class].definition.module;
    if let ast::Expr::Subscript(annotation) = expression
        && matches!(&*annotation.value, ast::Expr::Name(name) if name.id == "type")
        && annotation_at(
            &annotation.slice,
            graph,
            module,
            classes,
            types,
            source,
            Some(class),
        )? == classes.instance(class)
    {
        Ok(())
    } else {
        Err(error(
            source,
            expression,
            "cls must be annotated as type[Self] or type[its class]",
        ))
    }
}

/// Only sealed empty-container factories become call-site defaults. They run
/// after every supplied argument, allocate a fresh owner, and cannot call Python.
pub(super) fn factory_defaults(
    signature: &mut Signature,
    class: &crate::classes::Class,
    types: &Types,
    source: &str,
) -> Result<(), String> {
    if !class.generated_initializer {
        return Ok(());
    }
    for parameter in signature.params.iter_mut().skip(1) {
        let field = class
            .fields
            .iter()
            .find(|field| field.name == parameter.name)
            .unwrap();
        if matches!(field.default, Some(FieldDefault::Factory(_))) {
            parameter.default = field_default(field, parameter.ty, types, source)?;
        }
    }
    Ok(())
}

/// Class defaults are checked even when an explicit initializer hides them.
/// Nullable container fields use the same concrete factory and pointer promotion.
pub(super) fn field_default(
    field: &crate::classes::Field,
    ty: Type,
    types: &Types,
    source: &str,
) -> Result<Option<Expr>, String> {
    let Some(default) = &field.default else {
        return Ok(None);
    };
    let factory = match default {
        FieldDefault::Scalar(value) => {
            return calls::scalar_default(value, ty, types, source).map(Some);
        }
        FieldDefault::Factory(factory) => factory,
    };
    let concrete = types.payload(ty).unwrap_or(ty);
    let kind = match factory {
        ContainerFactory::List if types.element(concrete).is_some() => ExprKind::List(Vec::new()),
        ContainerFactory::Dict
            if types.mapping(concrete).is_some() && !types.is_default_list_dict(concrete) =>
        {
            let (key, value) = types.mapping(concrete).unwrap();
            ExprKind::Dict {
                key,
                value,
                entries: Vec::new(),
            }
        }
        ContainerFactory::Set if types.set_element(concrete).is_some() => ExprKind::Set {
            element: types.set_element(concrete).unwrap(),
            values: Vec::new(),
        },
        _ => {
            return Err(error(
                source,
                &field.annotation,
                "a default factory must match its exact list, dict, or set field type",
            ));
        }
    };
    let value = Expr { ty: concrete, kind };
    Ok(Some(if concrete == ty {
        value
    } else {
        Expr {
            ty,
            kind: ExprKind::Present(Box::new(value)),
        }
    }))
}

impl Checker<'_> {
    /// Body checking and type discovery must see the same statically bound class
    /// and the same narrow permission to initialize a generated frozen record.
    pub(super) fn method_context(&mut self, context: Option<MethodContext>) {
        self.class = context.map(|context| context.class);
        self.class_receiver = context.and_then(|context| {
            let declaration = &self.classes.declarations.definitions[context.class];
            (context.kind(&self.classes.declarations) == MethodKind::Class).then(|| {
                declaration.methods[context.index]
                    .parameters
                    .iter_non_variadic_params()
                    .next()
                    .unwrap()
                    .name()
                    .to_string()
            })
        });
        self.initializing = context.is_some_and(|context| {
            let declaration = &self.classes.declarations.definitions[context.class];
            declaration.generated_initializer && declaration.initializer == Some(context.index)
        });
    }

    /// An erased receiver shadows module-level bindings with the same name.
    pub(super) fn is_class_receiver(&self, expression: &ast::Expr) -> bool {
        matches!(expression, ast::Expr::Name(name) if self.class_receiver.as_deref() == Some(name.id.as_str()))
    }

    /// Static receiver attributes must not accidentally resolve through an
    /// identically named module import or global binding.
    pub(super) fn resolve(&self, expression: &ast::Expr) -> Option<Binding> {
        let mut root = expression;
        while let ast::Expr::Attribute(attribute) = root {
            root = &attribute.value;
        }
        if self.is_class_receiver(root) {
            None
        } else {
            self.graph.resolve(self.module, expression)
        }
    }

    /// The class binding is usable in static dispatch, never as a runtime value.
    pub(super) fn reject_class_receiver_value(&self, expression: &ast::Expr) -> Result<(), String> {
        if self.is_class_receiver(expression) {
            return Err(error(
                self.source,
                expression,
                "a class receiver is only supported for construction, method access, and JSON schemas",
            ));
        }
        Ok(())
    }

    /// Classes exist only at compile time. `cls` can name its concrete owner in
    /// constructor calls and schema arguments without becoming a storable value.
    pub(super) fn class_reference(&self, expression: &ast::Expr) -> Option<usize> {
        if self.is_class_receiver(expression) {
            return self.class;
        }
        match self.resolve(expression) {
            Some(Binding::Definition(definition)) => {
                self.classes.declarations.ids.get(&definition).copied()
            }
            _ => None,
        }
    }

    /// Instance access still evaluates the receiver for class/static methods,
    /// even though their native calls have no receiver parameter.
    fn evaluated_receiver(&self, receiver: Option<Expr>, value: Expr) -> Expr {
        match receiver {
            None => value,
            Some(receiver) => Expr {
                ty: value.ty,
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(receiver)],
                    value: Box::new(value),
                    clear: Vec::new(),
                },
            },
        }
    }

    /// Fixed record types resolve `__str__` to an ordinary native method call.
    /// Records without an override use their existing Python representation.
    pub(super) fn record_text(&mut self, value: Expr) -> Expr {
        let kind = if let Type::Class(class) = value.ty
            && let Some(method) = self.classes.methods[class].get("__str__")
        {
            self.calls.insert(method.id);
            ExprKind::Call(method.id, ir::CallArgs::positional(vec![value]))
        } else {
            ExprKind::Builtin(Builtin::Repr, vec![value])
        };
        Expr {
            ty: Type::Str,
            kind,
        }
    }

    pub(super) fn class_attribute(
        &mut self,
        attribute: &ast::ExprAttribute,
        class: usize,
        receiver: Option<Expr>,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        if attribute.attr.as_str().starts_with("__") && attribute.attr.as_str() != "__str__" {
            return Err(error(
                self.source,
                attribute,
                "special methods cannot be used as function values",
            ));
        }
        let signature = self.classes.methods[class]
            .get(attribute.attr.as_str())
            .ok_or_else(|| error(self.source, attribute, "unknown class member"))?
            .clone();
        match signature.method.unwrap().kind(&self.classes.declarations) {
            MethodKind::Property => {
                let receiver = receiver.ok_or_else(|| {
                    error(self.source, attribute, "a property requires an instance")
                })?;
                let mut signature = signature;
                if let Some(inference) = self.inference {
                    inference
                        .callable_signature(&mut signature, 1, None)
                        .map_err(|message| error(self.source, attribute, message))?;
                }
                self.calls.insert(signature.id);
                Ok(Expr {
                    ty: signature.returns,
                    kind: ExprKind::Call(signature.id, ir::CallArgs::positional(vec![receiver])),
                })
            }
            MethodKind::Instance => self.callable_value(&signature, receiver, expected, attribute),
            MethodKind::Class | MethodKind::Static => {
                let value = self.callable_value(&signature, None, expected, attribute)?;
                Ok(self.evaluated_receiver(receiver, value))
            }
        }
    }

    /// Property calls first execute the getter, then invoke its returned value;
    /// ordinary methods use the existing source-ordered argument binder directly.
    pub(super) fn class_method_call(
        &mut self,
        call: &ast::ExprCall,
        attribute: &ast::ExprAttribute,
        class: usize,
        receiver: Option<Expr>,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        if attribute.attr.as_str().starts_with("__") && attribute.attr.as_str() != "__str__" {
            return Err(error(
                self.source,
                call,
                "special methods cannot be called directly",
            ));
        }
        let signature = self.classes.methods[class]
            .get(attribute.attr.as_str())
            .ok_or_else(|| error(self.source, call, "unknown class method"))?
            .clone();
        let kind = signature.method.unwrap().kind(&self.classes.declarations);
        if kind == MethodKind::Property {
            let value = self.class_attribute(attribute, class, receiver, None)?;
            return self.invoke(call, value);
        }
        let (receiver, discarded) = if kind == MethodKind::Instance {
            (receiver, None)
        } else {
            (None, receiver)
        };
        let skip = usize::from(receiver.is_some());
        let (ty, args) = if self.inference.is_some() {
            self.infer_call(call, &signature, receiver, skip)?
        } else {
            (
                signature.returns,
                self.arguments(call, &signature.params[skip..], receiver)?,
            )
        };
        self.calls.insert(signature.id);
        let value = Expr {
            ty,
            kind: ExprKind::Call(signature.id, args),
        };
        Ok(self.evaluated_receiver(discarded, value))
    }

    pub(super) fn construct(&mut self, call: &ast::ExprCall, class: usize) -> Result<Expr, String> {
        if self.classes.declarations.definitions[class]
            .enum_members
            .is_some()
        {
            return Err(error(
                self.source,
                call,
                "StrEnum lookup by value is unsupported; use a declared member",
            ));
        }
        let signature = self.classes.methods[class]["__init__"].clone();
        let args = if self.inference.is_some() {
            self.infer_call(call, &signature, None, 1)?.1
        } else {
            self.arguments(call, &signature.params[1..], None)?
        };
        self.calls.insert(signature.id);
        Ok(Expr {
            ty: Type::Class(class),
            kind: ExprKind::Construct {
                initializer: signature.id,
                args,
            },
        })
    }
}
