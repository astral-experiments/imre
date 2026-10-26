//! Typed operations on source records observed through a Python weak proxy.

use super::*;

impl Checker<'_> {
    pub(super) fn weak_proxy(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let target = match call.arguments.args.as_ref() {
            [target] | [target, ast::Expr::NoneLiteral(_)] => self.expr(target)?,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "weakref.proxy requires one fixed record and an optional None callback",
                ));
            }
        };
        let Type::Class(class) = target.ty else {
            if matches!(
                target.ty,
                Type::None
                    | Type::Bool
                    | Type::Int
                    | Type::Float
                    | Type::Str
                    | Type::Bytes
                    | Type::ListInt
                    | Type::List(_)
                    | Type::Dict(_)
                    | Type::Tuple(_)
            ) {
                let name = match target.ty {
                    Type::None => "NoneType",
                    Type::ListInt | Type::List(_) => "list",
                    Type::Dict(_) => "dict",
                    Type::Tuple(_) => "tuple",
                    ty => self.exact_argument_type_name(ty).unwrap(),
                };
                return Ok(calls::type_error(
                    Type::None,
                    vec![target],
                    format!("cannot create weak reference to '{name}' object"),
                ));
            }
            return Err(error(
                self.source,
                call,
                "weakref.proxy requires a fixed class instance",
            ));
        };
        let create = |checker: &mut Self, layout: usize, target: Expr| {
            let declaration = &checker.classes.declarations.definitions[layout];
            if checker.graph.modules[declaration.definition.module].bundled {
                return Err(error(
                    checker.source,
                    call,
                    "weak proxy support requires a source-defined class",
                ));
            }
            if std::iter::once(&layout)
                .chain(&declaration.bases)
                .all(|&id| {
                    checker.classes.declarations.definitions[id]
                        .dataclass
                        .is_some_and(|options| options.slots)
                })
            {
                return Ok(calls::type_error(
                    Type::WeakProxy(class),
                    vec![target],
                    format!(
                        "cannot create weak reference to '{}' object",
                        declaration.name
                    ),
                ));
            }
            let callable = checker.classes.methods[layout].contains_key("__call__");
            Ok(Expr {
                ty: Type::WeakProxy(class),
                kind: ExprKind::Builtin(
                    Builtin::WeakProxyNew,
                    vec![
                        target,
                        Expr {
                            ty: Type::Bool,
                            kind: ExprKind::Bool(callable),
                        },
                    ],
                ),
            })
        };
        if self.classes.has_concrete_subclasses(class) {
            self.dispatch_record(class, target, call, create)
        } else {
            create(self, class, target)
        }
    }

    pub(super) fn record_class_object(&self, class: usize) -> Expr {
        let definition = &self.classes.declarations.definitions[class];
        let module = if definition.definition.module == self.graph.entry {
            "__main__"
        } else {
            &self.graph.modules[definition.definition.module].name
        };
        annotations::class_value(&format!("{module}.{}", definition.name))
    }

    pub(super) fn weak_proxy_type_name(callable: bool) -> &'static str {
        if callable {
            "weakref.CallableProxyType"
        } else {
            "weakref.ProxyType"
        }
    }

    pub(super) fn weak_proxy_callable(proxy: Expr) -> Expr {
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::WeakProxyCallable, vec![proxy]),
        }
    }

    /// Class and hash protocols consult the proxy's own type even when its referent is gone.
    pub(super) fn weak_proxy_builtin(&self, name: &str, proxy: Expr) -> Option<Expr> {
        let result = |callable| {
            let class = Self::weak_proxy_type_name(callable);
            match name {
                "type" => annotations::class_value(class),
                "hash" => {
                    calls::type_error(Type::Int, vec![], format!("unhashable type: '{class}'"))
                }
                _ => unreachable!(),
            }
        };
        let callable = Self::weak_proxy_callable(proxy);
        if name == "callable" {
            return Some(callable);
        }
        if !matches!(name, "type" | "hash") {
            return None;
        }
        let yes = result(true);
        Some(Expr {
            ty: yes.ty,
            kind: ExprKind::Conditional(Box::new(callable), Box::new(yes), Box::new(result(false))),
        })
    }

    /// Upgrade only while performing an operation that accesses the referent.
    pub(super) fn weak_target(value: Expr) -> Expr {
        if let Type::WeakProxy(class) = value.ty {
            Expr {
                ty: Type::Class(class),
                kind: ExprKind::Builtin(Builtin::WeakProxyTarget, vec![value]),
            }
        } else {
            value
        }
    }

    /// A proxy object is evaluated before the arguments, but the referent is acquired only
    /// after every supplied argument has run. Argument code can drop the last strong owner.
    pub(super) fn weak_proxy_call(
        &mut self,
        call: &ast::ExprCall,
        proxy: Expr,
        class: usize,
    ) -> Result<Expr, String> {
        let mut scope = calls::ArgumentScope::default();
        let proxy = scope.hold(self, proxy);
        let layouts = self.classes.concrete_layouts(class);
        let Some(signature) = layouts
            .iter()
            .find_map(|&id| self.classes.methods[id].get("__call__"))
            .cloned()
        else {
            if calls::starred(call) {
                return Err(error(
                    self.source,
                    call,
                    "unpacking a call to a noncallable weak proxy is unsupported",
                ));
            }
            let mut args = call
                .arguments
                .args
                .iter()
                .chain(call.arguments.keywords.iter().map(|k| &k.value));
            let values = args
                .by_ref()
                .map(|arg| self.expr(arg))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(scope.finish(calls::type_error(
                Type::None,
                values,
                "'weakref.ProxyType' object is not callable".into(),
            )));
        };
        if layouts
            .iter()
            .filter_map(|&id| self.classes.methods[id].get("__call__"))
            .any(|other| other.id != signature.id)
            && (!call.arguments.keywords.is_empty()
                || call.arguments.args.len() + 1 != signature.params.len())
        {
            return Err(error(
                self.source,
                call,
                "calling different weak proxy overrides requires all arguments positionally",
            ));
        }
        let (ty, bound) = self.bind_source(call, &signature, None, 1, Some(&mut scope))?;
        let ty = if self.classes.contexts.contains(&signature.id)
            || self.classes.producer_items.contains_key(&signature.id)
        {
            Type::GeneratorContext
        } else if signature.asynchronous {
            self.types.coroutine(ty)
        } else {
            ty
        };
        let args: Vec<_> = bound
            .order
            .iter()
            .map(|&i| bound.values[i].clone())
            .collect();
        let invoke = |checker: &mut Self, layout: usize, target: Expr| {
            if checker.classes.methods[layout].contains_key("__call__") {
                checker.record_slot(layout, target, "__call__", &args, call)
            } else {
                Ok(calls::type_error(
                    ty,
                    vec![],
                    "'weakref.ProxyType' object is not callable".into(),
                ))
            }
        };
        let target = Self::weak_target(proxy.clone());
        let mut result = if self.classes.has_concrete_subclasses(class) {
            self.dispatch_record(class, target, call, invoke)?
        } else {
            invoke(self, class, target)?
        };
        if layouts
            .iter()
            .any(|&id| !self.classes.methods[id].contains_key("__call__"))
        {
            let error = calls::type_error(
                result.ty,
                vec![],
                "'weakref.ProxyType' object is not callable".into(),
            );
            result = Expr {
                ty: result.ty,
                kind: ExprKind::Conditional(
                    Box::new(Self::weak_proxy_callable(proxy)),
                    Box::new(result),
                    Box::new(error),
                ),
            };
        }
        Ok(scope.finish(result))
    }

    /// Operands may release each other's referents. Hold the original handles until both have evaluated.
    pub(super) fn weak_proxy_comparison(
        &mut self,
        values: Vec<Expr>,
        ops: &[ast::CmpOp],
        expression: &ast::Expr,
    ) -> Result<Expr, String> {
        let [op @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)] = ops else {
            return Err(error(
                self.source,
                expression,
                "weak record proxies support single equality comparisons",
            ));
        };
        let mut scope = calls::ArgumentScope::default();
        let inputs: Vec<_> = values
            .into_iter()
            .map(|v| Self::weak_target(scope.hold(self, v)))
            .collect();
        if !inputs
            .iter()
            .all(|v| matches!(v.ty, Type::Class(id) if !self.decimal_class(id)))
        {
            return Err(error(
                self.source,
                expression,
                "weak proxy equality requires comparable fixed records",
            ));
        }
        let op = if *op == ast::CmpOp::Eq {
            Compare::Equal
        } else {
            Compare::NotEqual
        };
        Ok(scope.finish(Expr {
            ty: Type::Bool,
            kind: ExprKind::Compare(inputs, vec![op]),
        }))
    }

    pub(super) fn weak_proxy_instance(&mut self, value: Expr, callable: bool) -> Expr {
        let mut scope = calls::ArgumentScope::default();
        let value = scope.hold(self, value);
        let yes = Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(true),
        };
        let no = Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(false),
        };
        if !matches!(value.ty, Type::WeakProxy(_)) {
            return scope.finish(no);
        }
        let actual = Self::weak_proxy_callable(value.clone());
        let other = self.evaluated_receiver(Some(Self::weak_target(value)), no);
        let (yes, no) = if callable { (yes, other) } else { (other, yes) };
        scope.finish(Expr {
            ty: Type::Bool,
            kind: ExprKind::Conditional(Box::new(actual), Box::new(yes), Box::new(no)),
        })
    }
}
