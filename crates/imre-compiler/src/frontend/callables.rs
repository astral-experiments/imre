//! Exact function values carry no captured local environment. Indirect calls
//! retain their callee before evaluating arguments and keep a closed target set.

use super::*;

/// A callable signature is not a storage edge, but a callable value may own a
/// bound receiver. Such values cannot enter heap aggregates in this milestone.
pub(super) fn contains_callable(ty: Type, types: &Types) -> bool {
    // The declaration reader boxes only owners it has proven to be noncapturing.
    if matches!(ty, Type::AnnotationValue(_)) || types.constructor_base(ty).is_some() {
        return false;
    }
    (matches!(ty, Type::Callable(_)) && !types.is_plain_callback(ty))
        || types
            .children(ty)
            .into_iter()
            .any(|ty| contains_callable(ty, types))
}

fn invalid_aggregate(ty: Type, types: &Types) -> bool {
    if matches!(
        ty,
        Type::Callable(_) | Type::ForwardRef(_) | Type::AnnotationValue(_)
    ) || types.is_partial_frame(ty)
        || types.is_dispatch_registry(ty)
    {
        false
    } else if matches!(ty, Type::VarTuple(_)) {
        // A completed display cannot acquire a reference to itself. Its fields
        // own the same callable records as locals; mutable containers and class
        // fields still reject storing the tuple through contains_callable().
        types
            .children(ty)
            .into_iter()
            .any(|field| invalid_aggregate(field, types))
    } else if let Some(payload) = types.payload(ty) {
        invalid_aggregate(payload, types)
    } else {
        types
            .children(ty)
            .into_iter()
            .any(|ty| contains_callable(ty, types))
    }
}

/// Checks inferred displays as well as annotations, including discarded values.
pub(super) fn validate_storage(function: &ir::Function, types: &Types) -> Result<(), String> {
    let mut invalid = invalid_aggregate(function.returns, types)
        || function
            .locals
            .iter()
            .any(|&ty| invalid_aggregate(ty, types));
    // A cold indirect call owns a private argument frame for its continuation.
    // Its callable cannot enter a language-visible tuple or container.
    let mut frames = HashSet::new();
    ir::walk_expressions(&function.body, &mut |value| {
        if let ExprKind::Builtin(
            Builtin::Process(
                ir::ProcessOperation::IndirectCoroutine { .. }
                | ir::ProcessOperation::Coroutine { .. }
                | ir::ProcessOperation::ContextNew { .. },
            ),
            args,
        ) = &value.kind
        {
            frames.insert(std::ptr::from_ref(&args[0]));
        }
        if !frames.contains(&std::ptr::from_ref(value)) {
            invalid |= invalid_aggregate(value.ty, types);
        }
    });
    if invalid {
        Err("callable values cannot be stored in containers or tuples".into())
    } else {
        Ok(())
    }
}

/// Synchronous or cold indirect calls both have a closed possible target set.
pub(super) fn invoked_signature(value: &Expr) -> Option<Type> {
    match &value.kind {
        ExprKind::Invoke { callable, .. } => Some(callable.ty),
        ExprKind::Builtin(
            Builtin::Process(ir::ProcessOperation::IndirectCoroutine { signature, .. }),
            _,
        ) => Some(*signature),
        _ => None,
    }
}

/// Creation does not call a function. Every compatible constructed target is
/// nevertheless a possible edge at an indirect invocation, including dead code.
pub(super) fn targets(
    functions: &[ir::Function],
    entry: &ir::Function,
    types: &Types,
) -> Result<HashMap<Type, HashSet<usize>>, String> {
    let mut targets = HashMap::<Type, HashSet<usize>>::new();
    for function in functions.iter().chain([entry]) {
        ir::walk_expressions(&function.body, &mut |value| {
            if let ExprKind::Callable { function, .. } = &value.kind {
                targets.entry(value.ty).or_default().insert(*function);
            }
        });
    }
    // A cold callback adapter executes an async body and returns the awaited
    // result. An eager factory returns a coroutine object through a different ABI.
    for (signature, candidates) in &targets {
        let (_, returns) = types.signature(*signature).unwrap();
        if matches!(returns, Type::Coroutine(_))
            && candidates
                .iter()
                .any(|&id| functions[id].returns == returns)
        {
            return Err("an Awaitable callback must be defined with async def".into());
        }
    }
    Ok(targets)
}

impl Checker<'_> {
    /// Capture a native receiver once and give a saved method its positional signature.
    pub(super) fn bound_builtin(
        &mut self,
        node: &impl Ranged,
        receiver: Expr,
        builtin: Builtin,
        parameters: Vec<Type>,
        returns: Type,
    ) -> Expr {
        let mut locals = vec![receiver.ty];
        locals.extend(&parameters);
        let arguments = locals
            .iter()
            .enumerate()
            .map(|(slot, &ty)| Expr {
                ty,
                kind: ExprKind::Local(slot),
            })
            .collect();
        let id = self.helpers.reserve();
        self.helpers.finish(
            id,
            comprehensions::Helper {
                module: self.module,
                function: ir::Function {
                    params: (0..locals.len()).collect(),
                    returns,
                    locals,
                    body: vec![Statement::Return(Expr {
                        ty: returns,
                        kind: ExprKind::Builtin(builtin, arguments),
                    })],
                },
                calls: HashSet::new(),
                class_reads: HashSet::new(),
                recursion_error: error(self.source, node, "recursive native method adapter"),
            },
        );
        Expr {
            ty: self.types.callable(parameters, returns),
            kind: ExprKind::Callable {
                function: id,
                receiver: Some(Box::new(receiver)),
            },
        }
    }

    /// Expose str.join or a bound string's join as an exact native callback.
    pub(super) fn string_join_value(&mut self, receiver: Option<Expr>, site: &impl Ranged) -> Expr {
        let list = self.types.list(Type::Str);
        let context = receiver.map(|value| Expr {
            ty: self.types.tuple(vec![Type::Str]),
            kind: ExprKind::Tuple(vec![value]),
        });
        let first = context.as_ref().map_or(Type::Str, |c| c.ty);
        let local = |slot, ty| Expr {
            ty,
            kind: ExprKind::Local(slot),
        };
        let separator = if context.is_some() {
            Expr {
                ty: Type::Str,
                kind: ExprKind::Field(Box::new(local(0, first)), 0),
            }
        } else {
            local(0, Type::Str)
        };
        let id = self.helpers.reserve();
        self.helpers.finish(
            id,
            comprehensions::Helper {
                module: self.module,
                calls: HashSet::new(),
                class_reads: HashSet::new(),
                recursion_error: self.graph.modules[self.module]
                    .error(site, "recursive calls through str.join are unsupported"),
                function: ir::Function {
                    params: vec![0, 1],
                    locals: vec![first, list],
                    returns: Type::Str,
                    body: vec![Statement::Return(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Builtin(
                            Builtin::StringJoin,
                            vec![separator, local(1, list)],
                        ),
                    })],
                },
            },
        );
        Expr {
            ty: self.types.callable(
                if context.is_some() {
                    vec![list]
                } else {
                    vec![Type::Str, list]
                },
                Type::Str,
            ),
            kind: ExprKind::Callable {
                function: id,
                receiver: context.map(Box::new),
            },
        }
    }

    /// Converts a fixed definition or bound method into its exact positional ABI.
    pub(super) fn callable_value(
        &mut self,
        signature: &Signature,
        receiver: Option<Expr>,
        expected: Option<Type>,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        if self.classes.producer_items.contains_key(&signature.id) {
            return Err(error(
                self.source,
                node,
                "generator functions require direct iteration",
            ));
        }
        let skip = usize::from(receiver.is_some());
        if signature.params[skip..].iter().any(|parameter| {
            matches!(
                parameter.kind,
                ParameterKind::KeywordOnly | ParameterKind::Variadic | ParameterKind::KeywordRest
            )
        }) {
            return Err(error(
                self.source,
                node,
                "function values require fixed positional parameters",
            ));
        }
        let mut signature = signature.clone();
        if let Some(inference) = self.inference {
            let asynchronous = signature.asynchronous;
            inference
                .callable_signature(
                    &mut signature,
                    skip,
                    expected
                        .and_then(|ty| self.types.signature(ty))
                        .map(|(params, result)| {
                            let returns = if asynchronous {
                                self.types.awaited(result).unwrap_or(result)
                            } else {
                                result
                            };
                            (params, returns)
                        }),
                )
                .map_err(|message| error(self.source, node, message))?;
        }
        let ty = self.types.callable(
            signature.params[skip..]
                .iter()
                .map(|parameter| parameter.ty)
                .collect(),
            if signature.asynchronous {
                self.types.coroutine(signature.returns)
            } else {
                signature.returns
            },
        );
        let unbound = receiver.is_none();
        let ty = if unbound && let Some(&policy) = self.classes.cached.get(&signature.id) {
            self.types.declared_cache(ty, signature.id, policy)
        } else {
            ty
        };
        let value = Expr {
            ty,
            kind: ExprKind::Callable {
                function: signature.id,
                receiver: receiver.map(Box::new),
            },
        };
        Ok(if unbound {
            Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::StaticFunction(signature.id), vec![value]),
            }
        } else {
            value
        })
    }

    /// An escaping callable has no environment to store enclosing locals.
    pub(super) fn lambda_value(
        &mut self,
        lambda: &ast::ExprLambda,
        parameters: Option<Vec<Type>>,
        returns: Option<Type>,
    ) -> Result<Expr, String> {
        self.lower_lambda(lambda, parameters, returns, false)
    }

    /// An immediate iterator invokes its lambda in a fresh lexical frame on every
    /// pull. Its private arguments read enclosing slots at that point, so later
    /// rebinding is visible without creating an escaping closure.
    pub(super) fn pull_lambda(&mut self, lambda: &ast::ExprLambda) -> Result<Expr, String> {
        self.lower_lambda(lambda, Some(Vec::new()), None, true)
    }

    fn lower_lambda(
        &mut self,
        lambda: &ast::ExprLambda,
        parameters: Option<Vec<Type>>,
        returns: Option<Type>,
        pull: bool,
    ) -> Result<Expr, String> {
        let declared = lambda.parameters.as_deref();
        if declared.is_some_and(|parameters| {
            parameters.vararg.is_some()
                || parameters.kwarg.is_some()
                || !parameters.kwonlyargs.is_empty()
                || parameters
                    .iter_non_variadic_params()
                    .any(|parameter| parameter.default.is_some())
        }) {
            return Err(error(
                self.source,
                lambda,
                "lambdas require positional parameters without defaults or variadics",
            ));
        }
        let names = declared
            .into_iter()
            .flat_map(|parameters| parameters.iter_non_variadic_params())
            .collect::<Vec<_>>();
        let parameters = parameters
            .or_else(|| names.is_empty().then(Vec::new))
            .ok_or_else(|| {
                error(
                    self.source,
                    lambda,
                    "lambda parameters require an exact Callable context or a map/filter input",
                )
            })?;
        if parameters.len() != names.len() {
            return Err(error(
                self.source,
                lambda,
                "lambda parameter count does not match its Callable context",
            ));
        }
        let mut bound = narrowing::expression_assignments(&lambda.body);
        bound.extend(names.iter().map(|parameter| parameter.name().to_string()));
        let mut captures = comprehensions::free_reads(&lambda.body, bound.clone())
            .into_iter()
            .filter_map(|name| self.names.get(&name).map(|&slot| (slot, name)))
            .collect::<Vec<_>>();
        if !pull && !captures.is_empty() {
            return Err(error(
                self.source,
                lambda,
                "capturing lambdas are unsupported; only parameters and fixed module bindings may be read",
            ));
        }
        let id = self.helpers.reserve();
        let omitted_free = self.unbound_assert_captures();
        let mut helper = Checker::new(
            self.graph,
            self.module,
            self.functions,
            self.classes,
            self.types,
            self.helpers,
            self.globals,
        );
        helper.omitted_assert_free = omitted_free;
        helper.inference = self.inference;
        helper.local_iter = bound.contains("iter");
        helper.class = self.class;
        helper.class_receiver = self
            .class_receiver
            .clone()
            .filter(|name| !bound.contains(name));
        let mut params = Vec::new();
        for (parameter, &ty) in names.iter().zip(&parameters) {
            let slot = helper.declare(parameter.name().as_str(), ty, *parameter)?;
            helper.assigned.insert(slot);
            params.push(slot);
        }
        let mut args = Vec::with_capacity(captures.len());
        captures.sort_unstable_by_key(|(slot, _)| *slot);
        for (source, name) in captures {
            if !self.assigned.contains(&source) {
                return Err(error(
                    self.source,
                    lambda,
                    "captured local may be uninitialized",
                ));
            }
            // A later pull may see a different optional or union arm.
            let value = self.local(source);
            let slot = helper.declare(&name, value.ty, lambda)?;
            helper.assigned.insert(slot);
            params.push(slot);
            args.push(value);
        }
        let body = if let Some(returns) = returns {
            helper.expr_expected(&lambda.body, returns)?
        } else {
            helper.expr(&lambda.body)?
        };
        let returns = body.ty;
        self.helpers.finish(
            id,
            comprehensions::Helper {
                module: self.module,
                function: ir::Function {
                    params,
                    returns,
                    locals: helper.locals,
                    body: vec![Statement::Return(body)],
                },
                calls: helper.calls,
                class_reads: helper.class_reads.into_inner(),
                recursion_error: self.graph.modules[self.module]
                    .error(lambda, "recursive calls are unsupported in this prototype"),
            },
        );
        if pull {
            self.calls.insert(id);
            return Ok(Expr {
                ty: returns,
                kind: ExprKind::Call(id, ir::CallArgs::positional(args)),
            });
        }
        Ok(Expr {
            ty: self.types.callable(parameters, returns),
            kind: ExprKind::Callable {
                function: id,
                receiver: None,
            },
        })
    }

    /// Callable types contain positional types, not parameter names or defaults.
    pub(super) fn invoke(&mut self, call: &ast::ExprCall, callable: Expr) -> Result<Expr, String> {
        let callable = if callable.ty == Type::FunctionRef
            && call.arguments.args.is_empty()
            && call.arguments.keywords.is_empty()
        {
            typing::typed_function(callable, self.types.callable(vec![], Type::None))
        } else {
            callable
        };
        if callable.ty == Type::OverloadStub {
            if call.arguments.args.iter().any(ast::Expr::is_starred_expr)
                || call.arguments.keywords.iter().any(|kw| kw.arg.is_none())
            {
                return Err(error(
                    self.source,
                    call,
                    "expanded arguments to an overload dummy require runtime argument validation",
                ));
            }
            let mut args = call
                .arguments
                .args
                .iter()
                .chain(call.arguments.keywords.iter().map(|k| &k.value))
                .collect::<Vec<_>>();
            args.sort_by_key(|e| e.range().start());
            let mut body = vec![Statement::Expr(callable)];
            for arg in args {
                body.push(Statement::Expr(self.expr(arg)?));
            }
            // A real overload dummy accepts arbitrary arguments. Keep checks inside each operand.
            self.graph.checked_overload(self.module, call.range());
            self.graph
                .checked_missing_argument(self.module, call.range());
            self.graph.checked_keywords(
                self.module,
                call.arguments.keywords.iter().map(Ranged::range),
            );
            for arg in &call.arguments.args {
                self.graph.checked_excess_argument(self.module, arg.range());
            }
            return Ok(Expr {
                ty: Type::None,
                kind: ExprKind::Scope {
                    body,
                    clear: vec![],
                    value: Box::new(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(
                            Builtin::Typing(ir::TypingOperation::CallStub),
                            vec![],
                        ),
                    }),
                },
            });
        }
        if let Type::StrDescriptor(method) = callable.ty {
            return self.invoke_descriptor(call, callable, method);
        }
        if let Type::Class(class) = callable.ty
            && self.native_mock(class)
        {
            return self.mock_arguments(call, class, callable, "_record");
        }
        if let Some(result) = self.invoke_bound_unittest(call, &callable)? {
            return Ok(result);
        }
        let (params, returns) = self.types.signature(callable.ty).ok_or_else(|| {
            error(
                self.source,
                call,
                "an indirect call requires a nonoptional exact Callable value",
            )
        })?;
        if !call.arguments.keywords.is_empty() || call.arguments.args.len() != params.len() {
            return Err(error(
                self.source,
                call,
                "indirect calls require the full positional Callable signature; keywords and omitted defaults are unsupported",
            ));
        }
        let values = call
            .arguments
            .args
            .iter()
            .zip(params)
            .map(|(argument, ty)| self.expr_expected(argument, ty))
            .collect::<Result<Vec<_>, _>>()?;
        if let Type::Coroutine(_) = returns {
            return Ok(self.indirect_coroutine(callable, values, returns));
        }
        Ok(Expr {
            ty: returns,
            kind: ExprKind::Invoke {
                callable: Box::new(callable),
                args: ir::CallArgs::positional(values),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn eager_coroutine_factories_remain_direct_calls() {
        let source = "from collections.abc import Awaitable, Callable\nasync def cold(x: int) -> int:\n    return x\ndef factory(x: int) -> Awaitable[int]:\n    return cold(x)\n";
        for callback in ["cold", "factory", "lambda x: cold(x)"] {
            let program = format!(
                "{source}async def run() -> int:\n    cb: Callable[[int], Awaitable[int]] = {callback}\n    return await cb(7)\n"
            );
            if callback == "cold" {
                crate::check(&program).unwrap();
            } else {
                assert!(
                    crate::check(&program)
                        .unwrap_err()
                        .contains("Awaitable callback must be defined with async def")
                );
            }
        }
        crate::check(&format!(
            "{source}async def run() -> int:\n    return await factory(7)\n"
        ))
        .unwrap();
    }
}
