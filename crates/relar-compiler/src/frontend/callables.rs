//! Exact function values carry no captured local environment. Indirect calls
//! retain their callee before evaluating arguments and keep a closed target set.

use super::*;

/// A callable signature is not a storage edge, but a callable value may own a
/// bound receiver. Such values cannot enter heap aggregates in this milestone.
pub(super) fn contains_callable(ty: Type, types: &Types) -> bool {
    matches!(ty, Type::Callable(_))
        || types
            .children(ty)
            .into_iter()
            .any(|ty| contains_callable(ty, types))
}

fn invalid_aggregate(ty: Type, types: &Types) -> bool {
    if matches!(ty, Type::Callable(_)) {
        false
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
) -> HashMap<Type, HashSet<usize>> {
    let mut targets = HashMap::<Type, HashSet<usize>>::new();
    for function in functions.iter().chain([entry]) {
        ir::walk_expressions(&function.body, &mut |value| {
            if let ExprKind::Callable { function, .. } = &value.kind {
                targets.entry(value.ty).or_default().insert(*function);
            }
        });
    }
    targets
}

impl Checker<'_> {
    /// Converts a fixed definition or bound method into its exact positional ABI.
    pub(super) fn callable_value(
        &mut self,
        signature: &Signature,
        receiver: Option<Expr>,
        expected: Option<Type>,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let skip = usize::from(receiver.is_some());
        if signature.params[skip..]
            .iter()
            .any(|parameter| parameter.kind == ParameterKind::KeywordOnly)
        {
            return Err(error(
                self.source,
                node,
                "function values cannot have keyword-only parameters",
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
        Ok(Expr {
            ty,
            kind: ExprKind::Callable {
                function: signature.id,
                receiver: receiver.map(Box::new),
            },
        })
    }

    /// The fresh checker contains only parameters: every enclosing local read,
    /// even in an inactive branch, fails rather than silently becoming a capture.
    pub(super) fn lambda_value(
        &mut self,
        lambda: &ast::ExprLambda,
        parameters: Option<Vec<Type>>,
        returns: Option<Type>,
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
        if comprehensions::free_reads(&lambda.body, bound.clone())
            .iter()
            .any(|name| self.names.contains_key(name))
        {
            return Err(error(
                self.source,
                lambda,
                "capturing lambdas are unsupported; only parameters and fixed module bindings may be read",
            ));
        }
        let id = self.helpers.reserve();
        let mut helper = Checker::new(
            self.graph,
            self.module,
            self.functions,
            self.classes,
            self.types,
            self.helpers,
            self.globals,
        );
        helper.inference = self.inference;
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
