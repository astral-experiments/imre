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
    if classes.definitions[context.class].forwarder.is_some()
        && matches!(method.name.as_str(), "__str__" | "__repr__")
        && method.returns.is_none()
    {
        method.returns = Some(Box::new(
            ruff_python_parser::parse_expression("str")
                .expect("static annotation")
                .expr()
                .clone(),
        ));
    }
    let origin = classes.definitions[context.class].method_origins[context.index].module;
    if iterator_result(&method, graph, origin) {
        let Some(ast::Stmt::Expr(stmt)) = method.body.last() else {
            return Err(graph.modules[origin]
                .error(&method, "a record iterator requires a final yield from"));
        };
        let ast::Expr::YieldFrom(yielded) = &*stmt.value else {
            return Err(
                graph.modules[origin].error(stmt, "a record iterator requires a final yield from")
            );
        };
        let prelude = &method.body[..method.body.len() - 1];
        if early_generator_exit(prelude) {
            return Err(
                graph.modules[origin].error(stmt, "only one terminal yield from is supported")
            );
        }
        let range = stmt.range;
        let mut value = yielded.value.clone();
        // A local freshly accumulated mapping owns its items until the pull is
        // complete. Materialize that private view at first next, in source order.
        if let ast::Expr::Call(call) = &*value
            && let ast::Expr::Attribute(method) = &*call.func
            && method.attr.as_str() == "items"
        {
            if !private_items(prelude, &method.value, graph, origin) {
                return Err(graph.modules[origin].error(
                    stmt,
                    "yield from a mapping's items requires a fresh, unaliased local mapping",
                ));
            }
            value = Box::new(ast::Expr::Call(ast::ExprCall {
                node_index: ast::AtomicNodeIndex::NONE,
                range_start: range.start(),
                func: Box::new(ast::Expr::Name(ast::ExprName {
                    range,
                    node_index: ast::AtomicNodeIndex::NONE,
                    id: "list".into(),
                    ctx: ast::ExprContext::Load,
                })),
                arguments: ast::Arguments {
                    range,
                    node_index: ast::AtomicNodeIndex::NONE,
                    args: [*value].into_iter().collect(),
                    keywords: Default::default(),
                },
            }));
        }
        *method.body.last_mut().unwrap() = ast::Stmt::Return(ast::StmtReturn {
            range,
            node_index: ast::AtomicNodeIndex::NONE,
            value: Some(value),
        });
    } else if method.name.as_str() == "__iter__" {
        return Err(graph.modules[origin].error(
            &method,
            "__iter__ requires an Iterator[T] return annotation",
        ));
    }
    if context.kind(classes) == MethodKind::Class {
        let receiver = if method.parameters.posonlyargs.is_empty() {
            method.parameters.args.remove(0)
        } else {
            method.parameters.posonlyargs.remove(0)
        };
        let module = classes.definitions[context.class].method_origins[context.index].module;
        let source = &graph.modules[module].source;
        if receiver.default.is_some() {
            return Err(error(
                source,
                &receiver,
                "a class receiver cannot have a default",
            ));
        }
        if let Some(annotation) = receiver.annotation() {
            class_receiver_annotation(
                annotation,
                context.class,
                module,
                classes,
                graph,
                types,
                source,
            )?;
        }
    }
    Ok(method)
}

/// A generator's early return ends iteration and discards its value. Only a
/// single terminal delegation can use the ordinary native function return.
fn early_generator_exit(body: &[ast::Stmt]) -> bool {
    use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
    #[derive(Default)]
    struct Exits(bool);
    impl<'ast> Visitor<'ast> for Exits {
        fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
            match stmt {
                ast::Stmt::Return(_) => self.0 = true,
                ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_) => {}
                _ => walk_stmt(self, stmt),
            }
        }
        fn visit_expr(&mut self, expr: &'ast ast::Expr) {
            match expr {
                ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) => self.0 = true,
                ast::Expr::Lambda(_) => {}
                _ => walk_expr(self, expr),
            }
        }
    }
    let mut exits = Exits::default();
    exits.visit_body(body);
    exits.0
}

/// Snapshotting a local items view at the first pull preserves later pulls
/// only if user code cannot mutate the mapping while the generator is paused.
fn private_items(
    body: &[ast::Stmt],
    receiver: &ast::Expr,
    graph: &SourceGraph,
    module: usize,
) -> bool {
    use ruff_python_ast::visitor::{Visitor, walk_expr};
    let ast::Expr::Name(name) = receiver else {
        return false;
    };
    let Some(created) = body.iter().rposition(|stmt| {
        let ast::Stmt::Assign(assign) = stmt else { return false };
        matches!(assign.targets.as_slice(), [ast::Expr::Name(target)] if target.id == name.id)
            && (matches!(&*assign.value, ast::Expr::Dict(_))
                || matches!(&*assign.value, ast::Expr::Call(call) if call.arguments.is_empty()
                    && graph.resolve(module, &call.func) == Some(Binding::Intrinsic(Intrinsic::Counter))))
    }) else { return false };
    struct Uses<'a> {
        name: &'a str,
        escapes: bool,
    }
    impl<'ast> Visitor<'ast> for Uses<'_> {
        fn visit_expr(&mut self, value: &'ast ast::Expr) {
            let owner =
                |value: &ast::Expr| matches!(value, ast::Expr::Name(n) if n.id == self.name);
            match value {
                ast::Expr::Subscript(sub) if owner(&sub.value) => self.visit_expr(&sub.slice),
                ast::Expr::Call(call) if matches!(&*call.func, ast::Expr::Attribute(a) if a.attr.as_str() == "update" && owner(&a.value)) =>
                {
                    self.visit_arguments(&call.arguments);
                }
                _ if owner(value) => self.escapes = true,
                _ => walk_expr(self, value),
            }
        }
    }
    let mut uses = Uses {
        name: name.id.as_str(),
        escapes: false,
    };
    uses.visit_body(&body[created + 1..]);
    !uses.escapes
}

pub(super) fn iterator_result(
    method: &ast::StmtFunctionDef,
    graph: &SourceGraph,
    module: usize,
) -> bool {
    matches!(method.returns.as_deref(), Some(ast::Expr::Subscript(annotation))
        if graph.resolve(module, &annotation.value) == Some(Binding::Intrinsic(Intrinsic::Iterator)))
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
            let name = format!("__imre_parameter_{next}");
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
    module: usize,
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
        return class_receiver_annotation(
            parsed.expr(),
            class,
            module,
            classes,
            graph,
            types,
            contents,
        )
        .map_err(|message| error(source, expression, &message));
    }
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
        // The factory's initializer must be known before its native call is bound.
        FieldDefault::Record(_) => return Ok(None),
        FieldDefault::Factory(factory) => factory,
    };
    let concrete = types.payload(ty).unwrap_or(ty);
    let kind = match factory {
        ContainerFactory::Counter if types.is_counter(concrete) => {
            ExprKind::Builtin(Builtin::CounterNew, Vec::new())
        }
        ContainerFactory::List if types.element(concrete).is_some() => ExprKind::List(Vec::new()),
        ContainerFactory::Dict if concrete == documents::MAP => {
            ExprKind::Builtin(Builtin::Document(ir::DocumentOp::EmptyMap), Vec::new())
        }
        ContainerFactory::Dict if concrete == Type::JsonObject => {
            ExprKind::Builtin(Builtin::JsonEmptyObject, Vec::new())
        }
        ContainerFactory::Dict
            if types.mapping(concrete).is_some()
                && !types.is_default_list_dict(concrete)
                && !types.is_counter(concrete) =>
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
                "a default factory must match its exact list, dict, set, or Counter field type",
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

/// A zero-argument constructor has a single native thunk shared by all fields.
/// Calling it for each missing argument preserves a fresh nested record and its
/// fresh container defaults without expanding a tree of constructors in the IR.
pub(super) fn record_factories(
    classes: &mut ClassInfo,
    definitions: &mut Vec<(ModuleId, ast::StmtFunctionDef, Signature)>,
    owners: &mut Vec<DefinitionId>,
    inference: &inference::Inference,
    graph: &SourceGraph,
    types: &Types,
) -> Result<(), String> {
    let mut thunks = HashMap::new();
    for (class, record) in classes.declarations.definitions.iter().enumerate() {
        for (index, field) in record.fields.iter().enumerate() {
            let Some(FieldDefault::Record(target)) = field.default else {
                continue;
            };
            let id = classes.declarations.ids[&target];
            let declared = &classes.declarations.definitions[id];
            let concrete = Type::Class(id);
            let ty = classes.fields[class][index];
            if types.payload(ty).unwrap_or(ty) != concrete
                || !declared.generated_initializer
                || !declared.abstract_methods.is_empty()
                || classes.methods[id].contains_key("__post_init__")
            {
                return Err(graph.modules[field.module].error(&field.annotation,
                    "default_factory requires its field's exact concrete dataclass with a generated, side-effect-free initializer"));
            }
            if !record.generated_initializer {
                continue;
            }
            let function = *thunks.entry(id).or_insert_with(|| {
                let node = &declared.node;
                let range = node.name.range();
                let callee = ast::Expr::Name(ast::ExprName {
                    range,
                    node_index: ast::AtomicNodeIndex::NONE,
                    id: node.name.id.clone(),
                    ctx: ast::ExprContext::Load,
                });
                let helper = ast::StmtFunctionDef {
                    range,
                    node_index: ast::AtomicNodeIndex::NONE,
                    is_async: false,
                    decorator_list: Default::default(),
                    name: ast::Identifier::new("__imre_record_factory", range),
                    type_params: None,
                    parameters: Box::new(ast::Parameters {
                        range,
                        ..Default::default()
                    }),
                    returns: Some(Box::new(callee.clone())),
                    body: [ast::Stmt::Return(ast::StmtReturn {
                        range,
                        node_index: ast::AtomicNodeIndex::NONE,
                        value: Some(Box::new(ast::Expr::Call(ast::ExprCall {
                            range_start: range.start(),
                            node_index: ast::AtomicNodeIndex::NONE,
                            func: Box::new(callee),
                            arguments: ast::Arguments {
                                range,
                                node_index: ast::AtomicNodeIndex::NONE,
                                args: Default::default(),
                                keywords: Default::default(),
                            },
                        }))),
                    })]
                    .into_iter()
                    .collect(),
                };
                let function = inference.register(Vec::new(), Some(concrete), Vec::new());
                let signature = Signature {
                    id: function,
                    asynchronous: false,
                    params: Vec::new(),
                    returns: concrete,
                    method: None,
                };
                definitions.push((target.module, helper, signature));
                owners.push(target);
                function
            });
            let value = Expr {
                ty: concrete,
                kind: ExprKind::Call(function, ir::CallArgs::positional(Vec::new())),
            };
            let value = if ty == concrete {
                value
            } else {
                Expr {
                    ty,
                    kind: ExprKind::Present(Box::new(value)),
                }
            };
            let signature = classes.methods[class].get_mut("__init__").unwrap();
            let index = signature
                .params
                .iter()
                .position(|param| param.name == field.name)
                .unwrap();
            signature.params[index].default = Some(value.clone());
            definitions[signature.id].2.params[index].default = Some(value);
        }
    }
    Ok(())
}

/// The next initializer in the lexical class lineage, independent of the
/// concrete receiver used for inherited method specialization.
#[derive(Clone, Copy)]
pub(super) enum BaseInitializer {
    Object,
    Record(usize),
}

impl Checker<'_> {
    /// A checked type for a mutable field can become stale through an alias.
    /// Load the field once per attribute access and raise Python's AttributeError
    /// if an intervening effect cleared it.
    pub(super) fn checked_record_receiver(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
    ) -> Expr {
        let Some(payload) = self.types.payload(receiver.ty) else {
            return receiver;
        };
        match payload {
            // Handler.formatter is optional until explicit or basic configuration assigns it.
            Type::LogFormatter => {}
            Type::Str if attr.attr.as_str() == "encode" => {}
            Type::Class(_)
                if matches!(&*attr.value, ast::Expr::Attribute(_))
                    && self
                        .graph
                        .session
                        .narrowed_instance(self.module, &attr.value) => {}
            _ => return receiver,
        }
        let slot = self.hidden(receiver.ty);
        let local = self.local(slot);
        let missing = Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::IsNone, vec![local.clone()]),
        };
        let error = Statement::Raise(Some(Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("AttributeError").unwrap(),
                message: Some(Box::new(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str(format!(
                        "'NoneType' object has no attribute '{}'",
                        attr.attr
                    )),
                })),
            },
        }));
        Expr {
            ty: payload,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(slot, receiver),
                    Statement::If(missing, vec![error], vec![]),
                ],
                value: Box::new(Expr {
                    ty: payload,
                    kind: ExprKind::Unwrap(Box::new(local)),
                }),
                clear: vec![slot],
            },
        }
    }

    pub(super) fn iteration_expression(
        &mut self,
        expr: &ast::Expr,
        hint: Option<Type>,
    ) -> Result<Expr, String> {
        let previous = self.iterator_source.replace(expr.range());
        let result = self.boolean_operand(expr, hint);
        self.iterator_source = previous;
        result.map(|value| {
            if value.ty == Type::StringIterable {
                Expr {
                    ty: self.types.list(Type::Str),
                    kind: ExprKind::Builtin(Builtin::StringIterationList, vec![value]),
                }
            } else {
                self.patch_iteration(self.document_iteration(value))
            }
        })
    }

    /// Check a method at an immediate consumer after its receiver has been evaluated.
    pub(super) fn iteration_method_call(
        &mut self,
        call: &ast::ExprCall,
        method: &ast::ExprAttribute,
        class: usize,
        receiver: Expr,
    ) -> Result<Expr, String> {
        let previous = self.iterator_source.replace(call.range());
        let result = self.class_method_call(call, method, class, Some(receiver));
        self.iterator_source = previous;
        result
    }

    pub(super) fn record_generator(&self, context: MethodContext) -> bool {
        let record = &self.classes.declarations.definitions[context.class];
        iterator_result(
            &record.methods[context.index],
            self.graph,
            record.method_origins[context.index].module,
        )
    }
    /// Body checking and type discovery must see the same statically bound class
    /// and the same narrow permission to initialize a generated frozen record.
    pub(super) fn method_context(&mut self, context: Option<MethodContext>) {
        self.context_method = context.is_some_and(|context| {
            let record = &self.classes.declarations.definitions[context.class];
            super::contexts::decorated(
                &record.methods[context.index],
                self.graph,
                record.method_origins[context.index].module,
            )
        });
        self.class = context.map(|context| context.class);
        let lexical_base = context.and_then(|context| {
            let classes = &self.classes.declarations;
            let origin = classes.definitions[context.class].method_origins[context.index];
            classes.definitions[classes.ids[&origin]]
                .bases
                .first()
                .copied()
        });
        self.super_method_base = context
            .filter(|context| {
                let classes = &self.classes.declarations;
                matches!(
                    context.kind(classes),
                    MethodKind::Instance | MethodKind::Property | MethodKind::PropertySetter
                ) && classes.definitions[context.class].initializer != Some(context.index)
            })
            .and(lexical_base);
        self.exception_formatter = context.and_then(|ctx| {
            let class = &self.classes.declarations.definitions[ctx.class];
            class.forwarder.as_ref()?;
            match class.methods[ctx.index].name.as_str() {
                "__str__" => Some(false),
                "__repr__" => Some(true),
                _ => None,
            }
        });
        self.super_initializer = context.and_then(|context| {
            let record = &self.classes.declarations.definitions[context.class];
            if record.methods[context.index].name.as_str() != "__init__" {
                return None;
            }
            Some(lexical_base.map_or(BaseInitializer::Object, BaseInitializer::Record))
        });
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
        if self.unbound_assert_root(expression).is_some() {
            return None;
        }
        if let ast::Expr::Attribute(attr) = expression
            && let Some(binding) = self.fixed_class_binding(attr)
        {
            return Some(binding);
        }
        let mut root = expression;
        while let ast::Expr::Attribute(attribute) = root {
            root = &attribute.value;
        }
        if self.is_class_receiver(root)
            || root.as_name_expr().is_some_and(|n| {
                self.class_lexical.contains(n.id.as_str())
                    || (self.names.contains_key(n.id.as_str())
                        && self.mapping_class_import(n.id.as_str()))
            })
        {
            None
        } else {
            let binding = self.graph.resolve(self.module, expression);
            if let Some(Binding::Definition(definition)) = binding
                && (self.classes.declarations.ids.contains_key(&definition)
                    || self.types.namedtuple(definition).is_some())
            {
                // Static dispatch and constant folding can erase the class name from the IR.
                self.class_reads.borrow_mut().insert(definition);
            }
            binding
        }
    }

    /// Resolve a read from an initialized local or a class name. A receiver
    /// expression with effects must still be evaluated by ordinary attribute lowering.
    fn fixed_class_binding(&self, attr: &ast::ExprAttribute) -> Option<Binding> {
        if let ast::Expr::Attribute(parent) = &*attr.value {
            let Binding::IntrinsicModule(module) = self.fixed_class_binding(parent)? else {
                return None;
            };
            return module.symbol(attr.attr.as_str());
        }
        let name = attr.value.as_name_expr()?.id.as_str();
        let class = if self.is_class_receiver(&attr.value) {
            self.class?
        } else if let Some(&slot) = self.names.get(name) {
            if !self.assigned.contains(&slot) {
                return None;
            }
            let Type::Class(class) = self.locals[slot] else {
                return None;
            };
            class
        } else {
            let Binding::Definition(id) = self.graph.resolve(self.module, &attr.value)? else {
                return None;
            };
            *self.classes.declarations.ids.get(&id)?
        };
        let owner = &self.classes.declarations.definitions[class];
        let crate::classes::ClassValue::Binding(binding) =
            owner.static_bindings.get(attr.attr.as_str())?
        else {
            return None;
        };
        self.class_reads.borrow_mut().insert(owner.definition);
        Some(*binding)
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
        if let ast::Expr::Attribute(a) = expression
            && let Some(class) = self.class_reference(&a.value)
            && let Some(crate::classes::ExceptionAlias::Named(def)) =
                self.classes.declarations.definitions[class]
                    .exception_aliases
                    .get(a.attr.as_str())
        {
            return self.classes.declarations.ids.get(def).copied();
        }
        match self.resolve(expression) {
            Some(Binding::Definition(definition)) => {
                self.classes.declarations.ids.get(&definition).copied()
            }
            _ => None,
        }
    }

    /// Class-valued exception attributes bind as constructors, never as methods.
    /// An upcast receiver selects the override on its concrete record layout.
    pub(super) fn exception_alias_call(
        &mut self,
        call: &ast::ExprCall,
        attr: &ast::ExprAttribute,
        class: usize,
        receiver: Option<Expr>,
    ) -> Result<Option<Expr>, String> {
        if !self.classes.declarations.definitions[class]
            .exception_aliases
            .contains_key(attr.attr.as_str())
        {
            return Ok(None);
        }
        let direct = |checker: &mut Self, id: usize| -> Result<Expr, String> {
            match &checker.classes.declarations.definitions[id].exception_aliases
                [attr.attr.as_str()]
            {
                crate::classes::ExceptionAlias::Builtin { kind, .. } => {
                    checker.simple_exception(call, *kind)
                }
                crate::classes::ExceptionAlias::Named(def) => {
                    if let Some(&layout) = checker.classes.declarations.ids.get(def) {
                        checker.construct(call, layout)
                    } else {
                        checker.simple_exception(call, checker.classes.declarations.exceptions[def])
                    }
                }
            }
        };
        if let Some(receiver) = receiver {
            if self.classes.has_concrete_subclasses(class) {
                return self
                    .dispatch_record(class, receiver, call, |checker, id, _| {
                        let e = direct(checker, id)?;
                        checker.coerce(e, Type::Exception, call)
                    })
                    .map(Some);
            }
            let result = direct(self, class)?;
            return Ok(Some(self.evaluated_receiver(Some(receiver), result)));
        }
        direct(self, class).map(Some)
    }

    /// Instance access still evaluates the receiver for class/static methods,
    /// even though their native calls have no receiver parameter.
    pub(super) fn evaluated_receiver(&self, receiver: Option<Expr>, value: Expr) -> Expr {
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

    pub(super) fn record_text(&mut self, value: Expr, at: &impl Ranged) -> Result<Expr, String> {
        self.record_display(value, at, "__str__")
    }

    pub(super) fn record_repr(&mut self, value: Expr, at: &impl Ranged) -> Result<Expr, String> {
        self.record_display(value, at, "__repr__")
    }

    fn record_display(
        &mut self,
        value: Expr,
        at: &impl Ranged,
        method: &'static str,
    ) -> Result<Expr, String> {
        if let Type::Class(class) = value.ty
            && self.classes.has_concrete_subclasses(class)
        {
            return self.dispatch_record(class, value, at, |checker, _, receiver| {
                checker.direct_record_display(receiver, at, method)
            });
        }
        self.direct_record_display(value, at, method)
    }

    fn direct_record_display(
        &mut self,
        value: Expr,
        at: &impl Ranged,
        method_name: &str,
    ) -> Result<Expr, String> {
        let method = if let Type::Class(id) = value.ty {
            let methods = &self.classes.methods[id];
            if method_name == "__str__"
                && let Some(method) = methods.get("__str__")
            {
                Some(method.id)
            } else if self.classes.has_repr_override(id) {
                let method = methods.get("__repr__").ok_or_else(|| {
                    error(
                        self.source,
                        at,
                        "this representation requires a reachable __repr__ implementation",
                    )
                })?;
                Some(method.id)
            } else {
                None
            }
        } else {
            None
        };
        let kind = if let Some(id) = method {
            self.calls.insert(id);
            ExprKind::Call(id, ir::CallArgs::positional(vec![value]))
        } else {
            if !dataclass_values::can_format_concrete(value.ty, self.types, self.classes) {
                return Err(error(
                    self.source,
                    at,
                    "generated repr cannot format resource, callable, or custom-repr fields",
                ));
            }
            ExprKind::Builtin(Builtin::Repr, vec![value])
        };
        Ok(Expr {
            ty: Type::Str,
            kind,
        })
    }

    pub(super) fn class_attribute(
        &mut self,
        attribute: &ast::ExprAttribute,
        class: usize,
        receiver: Option<Expr>,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        if self.classes.has_concrete_subclasses(class)
            && let Some(receiver) = receiver
        {
            return self.dispatch_record(
                class,
                receiver,
                attribute,
                |checker, layout, receiver| {
                    checker.concrete_attribute(attribute, layout, Some(receiver), expected)
                },
            );
        }
        self.concrete_attribute(attribute, class, receiver, expected)
    }

    /// A subclass may supply its own plain data or a property for inherited data.
    fn concrete_attribute(
        &mut self,
        attribute: &ast::ExprAttribute,
        class: usize,
        receiver: Option<Expr>,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        use crate::classes::ClassValue;
        let decl = &self.classes.declarations.definitions[class];
        let value = match decl.static_bindings.get(attribute.attr.as_str()) {
            Some(ClassValue::Runtime { owner, ordinal }) => {
                self.class_reads.borrow_mut().insert(decl.definition);
                let value = self.globals.class_value(*owner, *ordinal).ok_or_else(|| {
                    error(
                        self.source,
                        attribute,
                        "class value is read before its declaration",
                    )
                })?;
                return self.data_attribute(class, attribute, receiver, value);
            }
            Some(ClassValue::String(text)) => unpacking::string(text.clone()),
            Some(ClassValue::Strings { owner, ordinal, .. }) => {
                self.globals.class_value(*owner, *ordinal).ok_or_else(|| {
                    error(
                        self.source,
                        attribute,
                        "class tuple is read before its declaration",
                    )
                })?
            }
            _ => return self.direct_class_attribute(attribute, class, receiver, expected),
        };
        self.class_reads.borrow_mut().insert(decl.definition);
        Ok(self.evaluated_receiver(receiver, value))
    }

    pub(super) fn direct_class_attribute(
        &mut self,
        attribute: &ast::ExprAttribute,
        class: usize,
        receiver: Option<Expr>,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        if self.mock_implementation_method(class, attribute.attr.as_str()) {
            return Err(error(
                self.source,
                attribute,
                "private native mock operations cannot be accessed directly",
            ));
        }
        if attribute.attr.starts_with("__")
            && !matches!(
                attribute.attr.as_str(),
                "__str__" | "__post_init__" | "__copy__" | "__fspath__"
            )
        {
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
                if let Some(&(ready, value)) = self
                    .classes
                    .cached_properties
                    .get(&(class, signature.method.unwrap().index))
                {
                    return Ok(self.cached_property(receiver, &signature, ready, value));
                }
                Ok(Expr {
                    ty: signature.returns,
                    kind: ExprKind::Call(signature.id, ir::CallArgs::positional(vec![receiver])),
                })
            }
            MethodKind::Instance => self.callable_value(&signature, receiver, expected, attribute),
            MethodKind::PropertySetter => unreachable!("setters are resolved only on assignment"),
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
        if self.classes.has_concrete_subclasses(class)
            && let Some(receiver) = receiver
        {
            self.dispatch_record(class, receiver, call, |checker, class, receiver| {
                checker.direct_method_call(call, attribute, class, Some(receiver))
            })
        } else {
            self.direct_method_call(call, attribute, class, receiver)
        }
    }

    fn direct_method_call(
        &mut self,
        call: &ast::ExprCall,
        attribute: &ast::ExprAttribute,
        class: usize,
        receiver: Option<Expr>,
    ) -> Result<Expr, String> {
        if self.mock_implementation_method(class, attribute.attr.as_str()) {
            return Err(error(
                self.source,
                attribute,
                "private native mock operations cannot be accessed directly",
            ));
        }
        if attribute.attr.as_str() == "get"
            && self.classes.declarations.definitions[class].mapping
            && !self.classes.methods[class].contains_key("get")
            && let Some(value) = &receiver
        {
            return self.mapping_get(call, value.clone());
        }
        if call
            .arguments
            .keywords
            .iter()
            .any(|keyword| keyword.arg.is_none())
        {
            return Err(error(
                self.source,
                call,
                "keyword argument unpacking is unsupported",
            ));
        }
        if attribute.attr.starts_with("__")
            && !matches!(
                attribute.attr.as_str(),
                "__str__" | "__post_init__" | "__copy__" | "__fspath__"
            )
            && !(self.classes.declarations.definitions[class].mapping
                && matches!(
                    attribute.attr.as_str(),
                    "__getitem__" | "__len__" | "__contains__"
                ))
        {
            return Err(error(
                self.source,
                call,
                "special methods cannot be called directly",
            ));
        }
        if attribute.attr.as_str() == "getTestCaseNames"
            && let Some(object) = &receiver
            && let Some(names) = self.unittest_loader_names(call, class, object)?
        {
            return Ok(names);
        }
        if matches!(attribute.attr.as_str(), "assertIn" | "assertNotIn")
            && let Some(receiver) = &receiver
            && let Some(checked) =
                self.unittest_membership(call, attribute.attr.as_str(), class, receiver)?
        {
            return Ok(checked);
        }
        if matches!(attribute.attr.as_str(), "assertIs" | "assertIsNot")
            && self.unittest_case(class)
            && !self.classes.methods[class].contains_key(attribute.attr.as_str())
            && let Some(receiver) = &receiver
        {
            return self.unittest_identity(
                call,
                receiver,
                attribute.attr.as_str() == "assertIsNot",
            );
        }
        if matches!(attribute.attr.as_str(), "assertEqual" | "assertListEqual")
            && let Some(receiver) = &receiver
            && let Some(checked) =
                self.unittest_equality(call, attribute.attr.as_str(), class, receiver)?
        {
            return Ok(checked);
        }
        if matches!(
            attribute.attr.as_str(),
            "assertRaises" | "assertRaisesRegex"
        ) && !self.classes.methods[class].contains_key(attribute.attr.as_str())
            && let Some(receiver) = &receiver
            && let Some(context) =
                self.unittest_assert_raises(call, class, receiver, attribute.attr.as_str())?
        {
            return Ok(context);
        }
        if attribute.attr.as_str() == "dict"
            && self.bundled_mock_class(class, "_PatchFactory")
            && let Some(object) = &receiver
        {
            return self.mock_dictionary_call(call, class, object.clone());
        }
        if self.native_mock(class)
            && let Some(object) = &receiver
        {
            match attribute.attr.as_str() {
                "assert_any_call" => {
                    return self.mock_arguments(call, class, object.clone(), "_assert_any_call");
                }
                "assert_called_with" => {
                    return self.mock_arguments(call, class, object.clone(), "_assert_called_with");
                }
                "assert_called_once_with" => {
                    return self.mock_arguments(
                        call,
                        class,
                        object.clone(),
                        "_assert_called_once_with",
                    );
                }
                name if !self.classes.methods[class].contains_key(name) => {
                    let child = self.mock_child(class, object.clone(), attribute)?;
                    return self.mock_arguments(call, class, child, "_record");
                }
                _ => {}
            }
        }
        let signature = self.classes.methods[class]
            .get(attribute.attr.as_str())
            .ok_or_else(|| error(self.source, call, "unknown class method"))?
            .clone();
        let kind = signature.method.unwrap().kind(&self.classes.declarations);
        if self.classes.keyword_copies.contains(&signature.id)
            && let Some(receiver) = receiver
        {
            if !call.arguments.args.is_empty() {
                return Err(error(
                    self.source,
                    call,
                    "asdict copy methods accept only keyword fields",
                ));
            }
            return self.copy_with_keywords(class, receiver, &call.arguments.keywords, call);
        }
        if self.record_generator(signature.method.unwrap())
            && self.iterator_source != Some(call.range())
        {
            return Err(error(
                self.source,
                call,
                "a record generator requires an immediate iterable consumer",
            ));
        }
        if kind == MethodKind::Property {
            calls::reject_unpacking(call, self.source)?;
            let value = self.class_attribute(attribute, class, receiver, None)?;
            return self.invoke(call, value);
        }
        let (receiver, discarded) = if kind == MethodKind::Instance {
            (receiver, None)
        } else {
            (None, receiver)
        };
        let skip = usize::from(receiver.is_some());
        let value = self.source_call(call, &signature, receiver, skip)?;
        Ok(self.evaluated_receiver(discarded, value))
    }

    /// Build a capture-free constructor. Each call allocates the concrete subclass
    /// and runs its initializer; storing the class never retains a handler instance.
    pub(super) fn record_constructor_value(
        &mut self,
        class: usize,
        base: usize,
        ty: Type,
        at: &ast::Expr,
    ) -> Result<Expr, String> {
        let declared = &self.classes.declarations.definitions[class];
        if (class != base && !declared.bases.contains(&base))
            || !declared.abstract_methods.is_empty()
        {
            return Err(error(
                self.source,
                at,
                "expected a concrete subclass of the declared factory class",
            ));
        }
        let initializer = self.classes.methods[class].get("__init__").ok_or_else(|| {
            error(
                self.source,
                at,
                "class factory requires an annotated initializer",
            )
        })?;
        let (params, returns) = self.types.signature(ty).unwrap();
        if !initializer
            .params
            .iter()
            .skip(1)
            .map(|p| p.ty)
            .eq(params.iter().copied())
            || initializer.params.iter().skip(1).any(|p| {
                !matches!(
                    p.kind,
                    ParameterKind::PositionalOnly | ParameterKind::PositionalOrKeyword
                )
            })
        {
            return Err(error(
                self.source,
                at,
                "subclass initializer does not match the factory signature",
            ));
        }
        let constructor = initializer.id;
        let args = params
            .iter()
            .enumerate()
            .map(|(slot, &ty)| Expr {
                ty,
                kind: ExprKind::Local(slot),
            })
            .collect();
        let result = self.coerce(
            Expr {
                ty: Type::Class(class),
                kind: ExprKind::Construct {
                    initializer: constructor,
                    args: ir::CallArgs::positional(args),
                },
            },
            returns,
            at,
        )?;
        let id = self.helpers.reserve();
        self.types.register_constructor(id, class);
        self.helpers.finish(
            id,
            comprehensions::Helper {
                module: self.module,
                function: ir::Function {
                    params: (0..params.len()).collect(),
                    locals: params,
                    returns,
                    body: vec![Statement::Return(result)],
                },
                calls: HashSet::from([constructor]),
                class_reads: HashSet::new(),
                source_frame: false,
                recursion_error: error(
                    self.source,
                    at,
                    "recursive class constructor callbacks are unsupported",
                ),
            },
        );
        Ok(Expr {
            ty,
            kind: ExprKind::Callable {
                function: id,
                receiver: None,
            },
        })
    }

    pub(super) fn construct(&mut self, call: &ast::ExprCall, class: usize) -> Result<Expr, String> {
        if self.classes.declarations.definitions[class]
            .forwarder
            .is_some()
        {
            return self.forwarding_exception(call, class);
        }
        if self.classes.declarations.definitions[class]
            .enum_members
            .as_ref()
            .is_some_and(|members| members.plain || members.http.is_some())
        {
            self.reject_builtin_keywords(call)?;
            let [value] = call.arguments.args.as_ref() else {
                return Err(error(self.source, call, "Enum lookup requires one value"));
            };
            let ty = self.classes.declarations.instance(class);
            let value = self.expr(value)?;
            if value.ty == ty {
                return Ok(value);
            }
            if self
                .types
                .members(value.ty)
                .is_some_and(|members| members == [Type::Str, ty])
            {
                let slot = self.hidden(value.ty);
                let test = Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(
                        Builtin::IsInstance(vec![ir::InstanceType::Exact(ty)]),
                        vec![self.local(slot)],
                    ),
                };
                let member = Expr {
                    ty,
                    kind: ExprKind::Builtin(Builtin::UnionProject, vec![self.local(slot)]),
                };
                let text = Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::UnionProject, vec![self.local(slot)]),
                };
                let lookup = Expr {
                    ty,
                    kind: ExprKind::Builtin(Builtin::EnumLookup(class), vec![text]),
                };
                return Ok(Expr {
                    ty,
                    kind: ExprKind::Scope {
                        body: vec![Statement::Assign(slot, value)],
                        value: Box::new(Expr {
                            ty,
                            kind: ExprKind::Conditional(
                                Box::new(test),
                                Box::new(member),
                                Box::new(lookup),
                            ),
                        }),
                        clear: vec![slot],
                    },
                });
            }
            let text = if matches!(ty, Type::IntEnum(_) | Type::StrEnum(_)) && value.ty == Type::Int
            {
                value
            } else {
                self.coerce(value, Type::Str, call)?
            };
            return Ok(Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::EnumLookup(class), vec![text]),
            });
        }
        if !self.classes.declarations.definitions[class]
            .abstract_methods
            .is_empty()
        {
            if self.classes.declarations.definitions[class].mapping {
                return self.abstract_mapping_call(class, call);
            }
            return Err(error(
                self.source,
                call,
                "cannot construct an abstract record with unimplemented methods",
            ));
        }
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
        // A generator may enter a fixed initializer whose first operation is
        // list(parameter). Only move that consumption past literal arguments.
        let materialized = self.immediate_constructor_input(call, class);
        let call = materialized.as_ref().unwrap_or(call);
        let signature = self.classes.methods[class]["__init__"].clone();
        let mut scope = calls::ArgumentScope::default();
        let normalization = self.normalized_record_input(class);
        let args = if let Some((field, wrapper, template)) = normalization {
            let params = &signature.params[1..];
            self.arguments_with(call, params, None, |checker, index, value| {
                if params[index].name == field && matches!(value, ast::Expr::Dict(_)) {
                    let mut conversion = template.clone();
                    conversion.arguments.keywords[0].value = value.clone();
                    checker.construct(&conversion, wrapper)
                } else {
                    checker.direct_argument(value, params[index].ty, params[index].sign_only)
                }
            })?
        } else if self.inference.is_some() {
            self.infer_call(call, &signature, None, 1)?.1
        } else {
            self.held_arguments(call, &signature.params[1..], None, &mut scope)?
        };
        self.calls.insert(signature.id);
        let record = scope.finish(Expr {
            ty: Type::Class(class),
            kind: ExprKind::Construct {
                initializer: signature.id,
                args,
            },
        });
        Ok(self.wrap_exception_record(class, record))
    }

    /// Attach the concrete class and display methods after the complete constructor succeeds.
    pub(super) fn wrap_exception_record(&mut self, class: usize, record: Expr) -> Expr {
        let decl = &self.classes.declarations.definitions[class];
        if let Some(&kind) = self.classes.declarations.exceptions.get(&decl.definition) {
            let text = self.classes.methods[class].get("__str__").map(|m| m.id);
            let repr = self.classes.methods[class].get("__repr__").map(|m| m.id);
            self.calls.extend(text);
            self.calls.extend(repr);
            return Expr {
                ty: Type::NamedException(class),
                kind: ExprKind::Builtin(
                    Builtin::NamedException {
                        kind,
                        message_field: decl.exception_argument_slot(),
                        tuple_args: self.exception_tuple_slot(class),
                        text,
                        repr,
                        dataclass_repr: decl.dataclass.is_some_and(|d| d.repr),
                    },
                    vec![record],
                ),
            };
        }
        record
    }

    /// Install the native message slot used by the exception's args and formatting.
    fn set_exception_message(&mut self, class: usize, message: &ast::Expr) -> Result<Expr, String> {
        let ty = self.types.optional(Type::Str);
        let message_value = self.expr_expected(message, Type::Str)?;
        let value = self.coerce(message_value, ty, message)?;
        Ok(self.store_exception_arguments(class, value))
    }

    /// Materialize all base arguments before exposing the tuple through the exception.
    fn set_exception_arguments(
        &mut self,
        class: usize,
        nodes: &[ast::Expr],
    ) -> Result<Expr, String> {
        let mut values = Vec::with_capacity(nodes.len());
        for node in nodes {
            let value = Self::string_value(self.expr(node)?);
            if !self.boxable_value(value.ty) {
                return Err(error(
                    self.source,
                    node,
                    "unsupported native exception argument type",
                ));
            }
            values.push(value);
        }
        let tuple = Expr {
            ty: self.types.tuple(values.iter().map(|v| v.ty).collect()),
            kind: ExprKind::Tuple(values),
        };
        let args = Expr {
            ty: Type::ObjectValue,
            kind: ExprKind::Builtin(Builtin::ExceptionTupleView, vec![tuple]),
        };
        Ok(self.store_exception_arguments(class, args))
    }

    fn store_exception_arguments(&self, class: usize, value: Expr) -> Expr {
        let field = self.classes.declarations.definitions[class].exception_argument_slot();
        Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body: vec![Statement::SetField(self.local(0), field, value)],
                value: Box::new(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                }),
                clear: Vec::new(),
            },
        }
    }

    pub(super) fn explicit_exception_initializer(
        &mut self,
        call: &ast::ExprCall,
    ) -> Result<Option<Expr>, String> {
        if self.super_initializer.is_some()
            && self.names.get("self") == Some(&0)
            && let Some(class) = self.class
            && self
                .classes
                .declarations
                .exceptions
                .contains_key(&self.classes.declarations.definitions[class].definition)
            && let Some((base, args)) =
                crate::classes::explicit_message_initializer(&self.graph.modules[self.module], call)
            && !self.names.contains_key(base)
        {
            return self.set_exception_arguments(class, args).map(Some);
        }
        Ok(None)
    }

    /// Start lookup after the declaring class and bind that implementation to the same object.
    pub(super) fn call_super_method(
        &mut self,
        call: &ast::ExprCall,
        method: &ast::ExprAttribute,
        base: usize,
    ) -> Result<Expr, String> {
        let signature = self.classes.methods[base]
            .get(method.attr.as_str())
            .filter(|sig| {
                sig.method.unwrap().kind(&self.classes.declarations) == MethodKind::Instance
            })
            .ok_or_else(|| {
                error(
                    self.source,
                    method,
                    "super() requires a known base instance method",
                )
            })?
            .clone();
        if !self.assigned.contains(&0) {
            return Err(error(
                self.source,
                method,
                "super() requires a bound first argument",
            ));
        }
        let receiver = self.coerce(self.local(0), Type::Class(base), method)?;
        self.source_call(call, &signature, Some(receiver), 1)
    }

    pub(super) fn call_super_initializer(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        if let Some(class) = self.class
            && !matches!(self.super_initializer, Some(BaseInitializer::Record(_)))
            && self
                .classes
                .declarations
                .exceptions
                .contains_key(&self.classes.declarations.definitions[class].definition)
            && !self.names.contains_key("super")
            && !self.graph.modules[self.module]
                .bindings
                .contains_key("super")
        {
            if self.exception_tuple_slot(class) && call.arguments.keywords.is_empty() {
                return self.set_exception_arguments(class, &call.arguments.args);
            }
            let [message] = call.arguments.args.as_ref() else {
                return Err(error(
                    self.source,
                    call,
                    "fixed exception payloads require one string argument to super().__init__",
                ));
            };
            if !call.arguments.keywords.is_empty() {
                return Err(error(
                    self.source,
                    call,
                    "exception messages do not accept keyword arguments",
                ));
            }
            return self.set_exception_message(class, message);
        }
        // An initializer defined at the root of a checked record lineage finds
        // object.__init__, including when that initializer runs on a subclass.
        if matches!(self.super_initializer, Some(BaseInitializer::Object))
            && !self.names.contains_key("super")
            && !self.graph.modules[self.module]
                .bindings
                .contains_key("super")
            && call.arguments.is_empty()
        {
            return Ok(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            });
        }
        let base = self
            .super_initializer
            .and_then(|base| match base {
                BaseInitializer::Record(base) => Some(base),
                BaseInitializer::Object => None,
            })
            .filter(|_| {
                !self.names.contains_key("super")
                    && !self.graph.modules[self.module]
                        .bindings
                        .contains_key("super")
            })
            .ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "super().__init__ requires an explicit subclass initializer",
                )
            })?;
        calls::reject_unpacking(call, self.source)?;
        let signature = self.classes.methods[base]["__init__"].clone();
        if self.classes.declarations.definitions[base].generated_initializer
            && self.classes.methods[base].contains_key("__post_init__")
        {
            return Err(error(
                self.source,
                call,
                "a base initializer with a dynamic post-init hook is unsupported",
            ));
        }
        let receiver = Expr {
            ty: Type::Class(base),
            kind: ExprKind::Builtin(Builtin::RecordView, vec![self.local(self.names["self"])]),
        };
        let mut scope = calls::ArgumentScope::default();
        let args = if self.inference.is_some() {
            self.infer_call(call, &signature, Some(receiver), 1)?.1
        } else {
            self.held_arguments(call, &signature.params[1..], Some(receiver), &mut scope)?
        };
        self.calls.insert(signature.id);
        Ok(scope.finish(Expr {
            ty: Type::None,
            kind: ExprKind::Call(signature.id, args),
        }))
    }

    fn immediate_constructor_input(
        &self,
        call: &ast::ExprCall,
        class: usize,
    ) -> Option<ast::ExprCall> {
        let class = &self.classes.declarations.definitions[class];
        let index = class.initializer?;
        let init = &class.methods[index];
        let ast::Stmt::Assign(assign) = init.body.iter().find(|stmt| !matches!(stmt, ast::Stmt::Expr(e) if matches!(&*e.value, ast::Expr::StringLiteral(_))))? else { return None };
        let ast::Expr::Call(list) = &*assign.value else {
            return None;
        };
        if !matches!(&*list.func, ast::Expr::Name(n) if n.id == "list")
            || !list.arguments.keywords.is_empty()
        {
            return None;
        }
        let [ast::Expr::Name(parameter)] = list.arguments.args.as_ref() else {
            return None;
        };
        let positional = init
            .parameters
            .iter_non_variadic_params()
            .skip(1)
            .position(|p| p.name() == parameter.id.as_str())?;
        let generator = |expr: &ast::Expr| {
            matches!(expr, ast::Expr::Generator(_))
                || matches!(expr, ast::Expr::Call(c) if self.resolve(&c.func) == Some(Binding::Intrinsic(Intrinsic::Ndiff)))
        };
        let inert = |expr: &ast::Expr| {
            matches!(
                expr,
                ast::Expr::NumberLiteral(_)
                    | ast::Expr::BooleanLiteral(_)
                    | ast::Expr::StringLiteral(_)
                    | ast::Expr::BytesLiteral(_)
                    | ast::Expr::NoneLiteral(_)
            )
        };
        let mut cloned = call.clone();
        let argument = if let Some(value) = call.arguments.args.get(positional) {
            if !generator(value)
                || !call.arguments.args[positional + 1..].iter().all(inert)
                || !call.arguments.keywords.iter().all(|k| inert(&k.value))
            {
                return None;
            }
            &mut cloned.arguments.args[positional]
        } else {
            let i = call.arguments.keywords.iter().position(|k| {
                k.arg
                    .as_ref()
                    .is_some_and(|n| n.as_str() == parameter.id.as_str())
            })?;
            if !generator(&call.arguments.keywords[i].value)
                || !call.arguments.keywords[i + 1..]
                    .iter()
                    .all(|k| inert(&k.value))
            {
                return None;
            }
            &mut cloned.arguments.keywords[i].value
        };
        let mut list = list.clone();
        list.range_start = argument.start();
        list.arguments.range = argument.range();
        list.arguments.args = vec![argument.clone()].into();
        *argument = ast::Expr::Call(list);
        Some(cloned)
    }

    /// A generated constructor immediately consuming asdict of the same frozen scalar
    /// record can read the original fields directly. Dictionary overrides still run
    /// once, in source order, including values replaced by a later key.
    pub(super) fn dataclass_copy(&mut self, call: &ast::ExprCall) -> Result<Option<Expr>, String> {
        let [keyword] = call.arguments.keywords.as_ref() else {
            return Ok(None);
        };
        if keyword.arg.is_some() || !call.arguments.args.is_empty() {
            return Ok(None);
        }
        let (spread, updates) = match &keyword.value {
            ast::Expr::Dict(dict) => {
                let Some((first, rest)) = dict.items.split_first() else {
                    return Ok(None);
                };
                if first.key.is_some() {
                    return Ok(None);
                }
                (&first.value, rest)
            }
            value => (value, &[][..]),
        };
        let ast::Expr::Call(asdict) = spread else {
            return Ok(None);
        };
        if self.resolve(&asdict.func) != Some(Binding::Intrinsic(Intrinsic::AsDict)) {
            return Ok(None);
        }
        let Some(class) = self.class_reference(&call.func) else {
            return Ok(None);
        };
        let [input] = asdict.arguments.args.as_ref() else {
            return Ok(None);
        };
        let declaration = &self.classes.declarations.definitions[class];
        if !asdict.arguments.keywords.is_empty()
            || !declaration.generated_initializer
            || !declaration.dataclass.is_some_and(|options| options.frozen)
            || !self.classes.fields[class]
                .iter()
                .all(|&ty| self.types.asdict_scalar(ty))
        {
            return Err(error(
                self.source,
                call,
                "asdict constructor expansion requires a frozen dataclass containing scalar fields",
            ));
        }
        let source = self.expr_expected(input, Type::Class(class))?;
        let slot = self.hidden(source.ty);
        let mut body = vec![Statement::Assign(slot, source)];
        let mut clear = vec![slot];
        let mut values = self.classes.fields[class]
            .iter()
            .enumerate()
            .map(|(field, &ty)| Expr {
                ty,
                kind: ExprKind::Field(Box::new(self.local(slot)), field),
            })
            .collect::<Vec<_>>();
        for item in updates {
            let Some(ast::Expr::StringLiteral(key)) = &item.key else {
                return Err(error(
                    self.source,
                    &item.value,
                    "asdict overrides require literal field names",
                ));
            };
            let field = declaration
                .fields
                .iter()
                .position(|field| field.name == key.value.to_str())
                .ok_or_else(|| {
                    error(
                        self.source,
                        key,
                        "unknown field in asdict constructor expansion",
                    )
                })?;
            let value = self.expr_expected(&item.value, values[field].ty)?;
            let temporary = self.hidden(value.ty);
            body.push(Statement::Assign(temporary, value));
            clear.push(temporary);
            values[field] = self.local(temporary);
        }
        let signature = &self.classes.methods[class]["__init__"];
        let order = signature.params[1..]
            .iter()
            .map(|parameter| {
                declaration
                    .fields
                    .iter()
                    .position(|field| field.name == parameter.name)
                    .unwrap()
            })
            .collect();
        self.calls.insert(signature.id);
        let ty = Type::Class(class);
        let value = Expr {
            ty,
            kind: ExprKind::Construct {
                initializer: signature.id,
                args: ir::CallArgs {
                    values,
                    order,
                    exhaustive_enum_match: false,
                },
            },
        };
        clear.reverse();
        Ok(Some(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(value),
                clear,
            },
        }))
    }
}
