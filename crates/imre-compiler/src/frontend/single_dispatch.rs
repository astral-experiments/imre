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

/// Test bool before int: both match an int instance test.
fn value_class(types: &Types, arg: &Expr) -> Option<Expr> {
    if let Some(inner) = types.payload(arg.ty) {
        let present = Expr {
            ty: inner,
            kind: ExprKind::Unwrap(Box::new(arg.clone())),
        };
        return Some(conditional(
            builtin(Builtin::IsNone, Type::Bool, vec![arg.clone()]),
            unpacking::string("NoneType"),
            value_class(types, &present)?,
        ));
    }
    if let Some(mut members) = types.members(arg.ty) {
        members.sort_by_key(|ty| *ty != Type::Bool);
        let mut members = members.into_iter().map(class).collect::<Option<Vec<_>>>()?;
        let mut name = unpacking::string(members.pop()?.0);
        for (key, test) in members.into_iter().rev() {
            name = conditional(
                builtin(
                    Builtin::IsInstance(vec![test]),
                    Type::Bool,
                    vec![arg.clone()],
                ),
                unpacking::string(key),
                name,
            );
        }
        return Some(name);
    }
    Some(unpacking::string(class(arg.ty)?.0))
}

impl Checker<'_> {
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
        if !unbound {
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
        let mut scope = calls::ArgumentScope::default();
        let target = self.dispatch_target(node)?;
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
                recursion_error: self.graph.modules[self.module]
                    .error(call, "recursive indirect single dispatch is unsupported"),
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
        let frame = Expr {
            ty: layout.frame,
            kind: ExprKind::Tuple(vec![target, table]),
        };
        Ok(scope.finish(Expr {
            ty,
            kind: ExprKind::Callable {
                function: id,
                receiver: Some(Box::new(frame)),
            },
        }))
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
        let value = if let Some(node) = registration {
            let target = self.dispatch_target(node)?;
            if target.ty != layout.callable {
                return Err(error(
                    self.source,
                    node,
                    "dispatch implementation must accept the same native positional signature",
                ));
            }
            let target = scope.hold(self, target);
            Expr {
                ty: target.ty,
                kind: ExprKind::Scope {
                    body: vec![Statement::SetItem(table, key, target.clone())],
                    value: Box::new(target),
                    clear: vec![],
                },
            }
        } else {
            lookup(&table, key, layout.callable)
        };
        Ok(scope.finish(value))
    }
}
