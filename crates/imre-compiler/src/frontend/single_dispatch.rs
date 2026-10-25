//! Each native single-dispatch wrapper owns its implementation registry.
use super::*;
use crate::ir::InstanceType;
use crate::types::DispatchFunction;

fn builtin(op: Builtin, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(op, args),
    }
}
fn field(frame: Expr, index: usize, ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Field(Box::new(frame), index),
    }
}
fn context(receiver: Expr, layout: DispatchFunction) -> Expr {
    builtin(Builtin::CallableContext, layout.frame, vec![receiver])
}
pub(super) fn original(receiver: Expr, layout: DispatchFunction) -> Expr {
    field(context(receiver, layout), 0, layout.callable)
}
fn conditional(test: Expr, yes: Expr, no: Expr) -> Expr {
    Expr {
        ty: yes.ty,
        kind: ExprKind::Conditional(Box::new(test), Box::new(yes), Box::new(no)),
    }
}

/// Builtin classes fall back to object; bool first inherits from int.
fn lookup(table: &Expr, name: Expr, target: Type) -> Expr {
    let get = |key: Expr, default: Expr| {
        builtin(
            Builtin::DictGetDefault,
            target,
            vec![table.clone(), key, default],
        )
    };
    let base = builtin(
        Builtin::DictGet,
        target,
        vec![table.clone(), unpacking::string("object")],
    );
    let int = || get(unpacking::string("int"), base.clone());
    let fallback = match &name.kind {
        ExprKind::Str(n) if n == "bool" => int(),
        ExprKind::Str(_) => base,
        _ => {
            let is_bool = Expr {
                ty: Type::Bool,
                kind: ExprKind::Compare(
                    vec![name.clone(), unpacking::string("bool")],
                    vec![ir::Compare::Equal],
                ),
            };
            conditional(is_bool, int(), base)
        }
    };
    get(name, fallback)
}

fn class(ty: Type) -> Option<(&'static str, InstanceType)> {
    Some(match ty {
        Type::None => ("NoneType", InstanceType::Exact(Type::None)),
        Type::Bool => ("bool", InstanceType::Exact(Type::Bool)),
        Type::Int => ("int", InstanceType::Exact(Type::Int)),
        Type::Float => ("float", InstanceType::Exact(Type::Float)),
        Type::Str | Type::StrLiteral(_) => ("str", InstanceType::Exact(Type::Str)),
        Type::Bytes => ("bytes", InstanceType::Exact(Type::Bytes)),
        Type::ListInt | Type::List(_) => ("list", InstanceType::List),
        Type::Dict(_) => ("dict", InstanceType::Dict),
        Type::Set(_) => ("set", InstanceType::Set),
        Type::Tuple(_) => ("tuple", InstanceType::Tuple),
        Type::BuiltinClass => ("type", InstanceType::Exact(Type::BuiltinClass)),
        _ => return None,
    })
}

/// One text conversion can observe an otherwise layout-independent callback argument.
pub(super) fn text_conversion(fmt: &str) -> bool {
    let mut chars = fmt.chars();
    let mut found = false;
    while chars.any(|c| c == '%') {
        match chars.next() {
            Some('%') => (),
            Some('s' | 'r' | 'a') if !found => found = true,
            _ => return false,
        }
    }
    found
}

/// Choose an exact class and value together; bool matches before its int base.
fn with_class(
    types: &Types,
    value: Expr,
    make: &impl Fn(&'static str, Expr) -> Expr,
) -> Option<Expr> {
    if let Some(inner) = types.payload(value.ty) {
        let present = Expr {
            ty: inner,
            kind: ExprKind::Unwrap(Box::new(value.clone())),
        };
        return Some(conditional(
            builtin(Builtin::IsNone, Type::Bool, vec![value]),
            with_class(
                types,
                Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                },
                make,
            )?,
            with_class(types, present, make)?,
        ));
    }
    if let Some(mut members) = types.members(value.ty) {
        members.sort_by_key(|ty| *ty != Type::Bool);
        let project = |ty| builtin(Builtin::UnionProject, ty, vec![value.clone()]);
        let mut frame = with_class(types, project(members.pop()?), make)?;
        for ty in members.into_iter().rev() {
            let (_, test) = class(ty)?;
            frame = conditional(
                builtin(
                    Builtin::IsInstance(vec![test]),
                    Type::Bool,
                    vec![value.clone()],
                ),
                with_class(types, project(ty), make)?,
                frame,
            );
        }
        return Some(frame);
    }
    let (name, _) = class(value.ty)?;
    Some(make(name, value))
}

/// Keep the supplied object and its concrete formatting descriptor, even for an unread argument.
fn callback_frame(types: &Types, classes: &ClassInfo, value: Expr) -> Option<Expr> {
    with_class(types, value, &|name, value| {
        let printable = Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(dataclass_values::can_format(value.ty, types, classes)),
        };
        let positional = Expr {
            ty: types.tuple(vec![value.ty]),
            kind: ExprKind::Tuple(vec![value]),
        };
        let positional = builtin(
            Builtin::RecordView,
            types.callback_positional(),
            vec![positional],
        );
        let frame = Expr {
            ty: types.tuple(vec![Type::Str, positional.ty, Type::Bool]),
            kind: ExprKind::Tuple(vec![unpacking::string(name), positional, printable]),
        };
        builtin(Builtin::RecordView, types.callback_argument(), vec![frame])
    })
}

fn value_class(types: &Types, arg: &Expr) -> Option<Expr> {
    if types.is_callback_argument(arg.ty) {
        return Some(field(arg.clone(), 0, Type::Str));
    }
    with_class(types, arg.clone(), &|name, _| unpacking::string(name))
}

/// Only formatting inspects the argument: an unread callback may safely retain opaque resources.
pub(super) fn format_argument(types: &Types, fmt: &Expr, rhs: &Expr) -> Option<Expr> {
    let ExprKind::Str(text) = &fmt.kind else {
        return None;
    };
    let ExprKind::Tuple(arguments) = &rhs.kind else {
        return None;
    };
    let [arg] = arguments.as_slice() else {
        return None;
    };
    if !text_conversion(text)
        || !types.is_callback_argument(arg.ty)
        || !matches!(arg.kind, ExprKind::Local(_))
    {
        return None;
    }
    let positional = field(arg.clone(), 1, types.callback_positional());
    let unsupported = Statement::Raise(Some(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("NotImplementedError").unwrap(),
            message: Some(Box::new(unpacking::string(
                "formatting a callback argument with opaque fields is unsupported",
            ))),
        },
    }));
    Some(Expr {
        ty: Type::Str,
        kind: ExprKind::Scope {
            body: vec![Statement::If(
                field(arg.clone(), 2, Type::Bool),
                vec![],
                vec![unsupported],
            )],
            value: Box::new(builtin(
                Builtin::PercentFormat,
                Type::Str,
                vec![fmt.clone(), positional],
            )),
            clear: vec![],
        },
    })
}

pub(super) enum LocalDecorator {
    Dispatch,
    Register(Box<Registration>),
}

/// Hold the receiver and class before evaluating the function being registered.
pub(super) struct Registration {
    scope: calls::ArgumentScope,
    table: Expr,
    key: Expr,
    callable: Type,
}

impl Registration {
    fn apply(
        mut self,
        checker: &mut Checker<'_>,
        target: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        if target.ty != self.callable {
            return Err(error(
                checker.source,
                node,
                "dispatch implementation must accept the same native positional signature",
            ));
        }
        let target = self.scope.hold(checker, target);
        Ok(self.scope.finish(Expr {
            ty: target.ty,
            kind: ExprKind::Scope {
                body: vec![Statement::SetItem(self.table, self.key, target.clone())],
                value: Box::new(target),
                clear: vec![],
            },
        }))
    }
}

impl Checker<'_> {
    /// Evaluate and root the supplied object before inspecting its class or formatting it.
    pub(super) fn pack_callback_argument(
        &mut self,
        arg: &ast::Expr,
        ty: Type,
    ) -> Result<Expr, String> {
        let value = self.expr(arg)?;
        if value.ty == ty {
            return Ok(value);
        }
        let mut scope = calls::ArgumentScope::default();
        let value = scope.hold(self, value);
        let frame = callback_frame(self.types, self.classes, value).ok_or_else(|| {
            error(
                self.source,
                arg,
                "this callback argument needs a known builtin class",
            )
        })?;
        Ok(scope.finish(frame))
    }

    /// An unbound function cannot retain its dispatcher through a captured receiver.
    fn dispatch_target(&mut self, node: &ast::Expr) -> Result<Expr, String> {
        let value = self.expr(node)?;
        let unbound = match &value.kind {
            ExprKind::Callable { receiver: None, .. }
            | ExprKind::Builtin(Builtin::StaticFunction(_), _) => true,
            // Only noncapturing local definitions have a function object slot.
            ExprKind::Local(slot) => {
                self.local_function_binding(node).and_then(|b| b.value_slot) == Some(*slot)
            }
            _ => false,
        };
        if !unbound || self.types.dispatcher(value.ty).is_some() {
            return Err(error(
                self.source,
                node,
                "single dispatch requires an unbound native function with a known signature",
            ));
        }
        Ok(value)
    }

    pub(super) fn single_dispatch(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [node] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "singledispatch needs one function",
            ));
        };
        let target = self.dispatch_target(node)?;
        self.new_dispatcher(target, self.declared_function_text(node), call)
    }

    fn new_dispatcher(
        &mut self,
        target: Expr,
        text: Option<function_text::FunctionText>,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let mut scope = calls::ArgumentScope::default();
        let target = scope.hold(self, target);
        let (parameters, returns) = self.types.signature(target.ty).unwrap();
        if matches!(returns, Type::Coroutine(_)) {
            return Err(error(
                self.source,
                node,
                "single dispatch requires a synchronous native function",
            ));
        }
        if parameters.is_empty() {
            return Err(error(
                self.source,
                node,
                "single dispatch needs a first positional parameter",
            ));
        }
        let ty = self.types.dispatch_callable(target.ty);
        let layout = self.types.dispatcher(ty).unwrap();
        let local = |slot, ty| Expr {
            ty,
            kind: ExprKind::Local(slot),
        };
        let args: Vec<_> = parameters
            .iter()
            .enumerate()
            .map(|(i, &ty)| local(i + 1, ty))
            .collect();
        let name = value_class(self.types, &args[0]).ok_or_else(|| {
            error(
                self.source,
                node,
                "single dispatch requires a builtin first argument or a finite union of builtins",
            )
        })?;
        let table = field(local(0, layout.frame), 1, layout.registry);
        let chosen = lookup(&table, name, layout.callable);
        let function = ir::Function {
            locals: std::iter::once(layout.frame)
                .chain(parameters.iter().copied())
                .collect(),
            params: (0..=parameters.len()).collect(),
            returns,
            body: vec![Statement::Return(Expr {
                ty: returns,
                kind: ExprKind::Invoke {
                    callable: Box::new(chosen),
                    args: ir::CallArgs::positional(args),
                },
            })],
        };
        let id = self.helpers.reserve();
        self.helpers.finish(
            id,
            comprehensions::Helper {
                module: self.module,
                calls: HashSet::new(),
                class_reads: HashSet::new(),
                source_frame: false,
                recursion_error: self.graph.modules[self.module]
                    .error(node, "recursive indirect single dispatch is unsupported"),
                function,
            },
        );
        let table = Expr {
            ty: layout.registry,
            kind: ExprKind::Dict {
                key: Type::Str,
                value: target.ty,
                entries: vec![(unpacking::string("object"), target.clone())],
            },
        };
        let none = || Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let metadata = if let Some(text) = text {
            let doc = text.doc.as_deref().map_or_else(none, unpacking::string);
            let doc = self.coerce(doc, self.types.optional(Type::Str), node)?;
            Expr {
                ty: self.types.payload(layout.metadata).unwrap(),
                kind: ExprKind::Tuple(vec![unpacking::string(&text.name), doc]),
            }
        } else {
            none()
        };
        let metadata = self.coerce(metadata, layout.metadata, node)?;
        let frame = Expr {
            ty: layout.frame,
            kind: ExprKind::Tuple(vec![target, table, metadata]),
        };
        Ok(scope.finish(Expr {
            ty,
            kind: ExprKind::Callable {
                function: id,
                receiver: Some(Box::new(frame)),
            },
        }))
    }

    /// Read the copy held by this dispatcher, independent of its current registry.
    pub(super) fn dispatch_attribute(
        &mut self,
        receiver: &Expr,
        layout: DispatchFunction,
        name: &str,
    ) -> Option<Expr> {
        let (index, ty) = match name {
            "__wrapped__" => return Some(original(receiver.clone(), layout)),
            "__name__" => (0, Type::Str),
            "__doc__" => (1, self.types.optional(Type::Str)),
            _ => return None,
        };
        let mut scope = calls::ArgumentScope::default();
        let metadata = scope.hold(
            self,
            field(context(receiver.clone(), layout), 2, layout.metadata),
        );
        let missing = builtin(
            Builtin::RaiseException,
            ty,
            vec![Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception {
                    kind: exception_kind("NotImplementedError").unwrap(),
                    message: Some(Box::new(unpacking::string(
                        "wrapper text is unavailable for this native callable",
                    ))),
                },
            }],
        );
        let text = Expr {
            ty: self.types.payload(layout.metadata).unwrap(),
            kind: ExprKind::Unwrap(Box::new(metadata.clone())),
        };
        Some(scope.finish(conditional(
            builtin(Builtin::IsNone, Type::Bool, vec![metadata]),
            missing,
            field(text, index, ty),
        )))
    }

    pub(super) fn single_dispatch_method(
        &mut self,
        call: &ast::ExprCall,
        attr: &ast::ExprAttribute,
        receiver: Expr,
        layout: DispatchFunction,
    ) -> Result<Expr, String> {
        let name = attr.attr.as_str();
        if name == "__wrapped__" {
            return self.invoke(call, original(receiver, layout));
        }
        self.reject_builtin_keywords(call)?;
        let (class, registration) = match (name, call.arguments.args.as_ref()) {
            ("dispatch", [class]) => (class, None),
            ("register", [class, target]) => (class, Some(target)),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "native single dispatch supports dispatch(cls) and register(cls, function)",
                ));
            }
        };
        let selection = self.dispatch_selection(receiver, layout, class)?;
        if let Some(node) = registration {
            let target = self.dispatch_target(node)?;
            selection.apply(self, target, node)
        } else {
            Ok(selection
                .scope
                .finish(lookup(&selection.table, selection.key, selection.callable)))
        }
    }

    fn dispatch_selection(
        &mut self,
        receiver: Expr,
        layout: DispatchFunction,
        class: &ast::Expr,
    ) -> Result<Registration, String> {
        let mut scope = calls::ArgumentScope::default();
        let table = scope.hold(self, field(context(receiver, layout), 1, layout.registry));
        let class_value = self.expr(class)?;
        if class_value.ty != Type::BuiltinClass {
            return Err(error(
                self.source,
                class,
                "single dispatch requires a builtin class object",
            ));
        }
        let key = scope.hold(self, annotations::type_name(class_value));
        Ok(Registration {
            scope,
            table,
            key,
            callable: layout.callable,
        })
    }

    fn registration(&mut self, expression: &ast::Expr) -> Result<Option<Registration>, String> {
        let ast::Expr::Call(call) = expression else {
            return Ok(None);
        };
        let ast::Expr::Attribute(attr) = &*call.func else {
            return Ok(None);
        };
        if attr.attr.as_str() != "register" {
            return Ok(None);
        }
        let [class] = call.arguments.args.as_ref() else {
            return Ok(None);
        };
        let receiver = self.expr(&attr.value)?;
        let Some(layout) = self.types.dispatcher(receiver.ty) else {
            return Ok(None);
        };
        self.reject_builtin_keywords(call)?;
        self.dispatch_selection(receiver, layout, class).map(Some)
    }

    pub(super) fn local_dispatch_decorator(
        &mut self,
        def: &ast::StmtFunctionDef,
    ) -> Result<Option<LocalDecorator>, String> {
        let [decorator] = def.decorator_list.as_slice() else {
            return Ok(None);
        };
        if self.resolve(&decorator.expression)
            == Some(Binding::Intrinsic(Intrinsic::SingleDispatch))
        {
            Ok(Some(LocalDecorator::Dispatch))
        } else {
            Ok(self
                .registration(&decorator.expression)?
                .map(Box::new)
                .map(LocalDecorator::Register))
        }
    }

    pub(super) fn decorate_dispatch(
        &mut self,
        decorator: LocalDecorator,
        target: Expr,
        def: &ast::StmtFunctionDef,
    ) -> Result<Expr, String> {
        match decorator {
            LocalDecorator::Dispatch => self.new_dispatcher(
                target,
                Some(function_text::FunctionText::from_definition(def)),
                def,
            ),
            LocalDecorator::Register(selection) => selection.apply(self, target, def),
        }
    }

    pub(super) fn immediate_registration(
        &mut self,
        call: &ast::ExprCall,
    ) -> Result<Option<Expr>, String> {
        let Some(selection) = self.registration(&call.func)? else {
            return Ok(None);
        };
        self.reject_builtin_keywords(call)?;
        let [implementation] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "a dispatch registration decorator needs one function",
            ));
        };
        let target = self.dispatch_target(implementation)?;
        selection.apply(self, target, implementation).map(Some)
    }
}
