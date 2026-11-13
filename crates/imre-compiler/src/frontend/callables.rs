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
        Type::Callable(_) | Type::ForwardRef(_) | Type::AnnotationValue(_) | Type::CmpKey(..)
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

/// Check inferred displays and annotations while visiting the function's call and frame uses.
pub(super) fn validate_storage(
    function: &ir::Function,
    types: &Types,
    mut visit: impl FnMut(&Expr),
) -> std::result::Result<(), String> {
    let mut invalid = invalid_aggregate(function.returns, types)
        || function
            .locals
            .iter()
            .any(|&ty| invalid_aggregate(ty, types));
    // A cold indirect call owns a private argument frame for its continuation.
    // Its callable cannot enter a language-visible tuple or container.
    let mut frames = HashSet::new();
    ir::walk_expressions(&function.body, &mut |value| {
        visit(value);
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

/// A constructed callable is a possible target, even when it occurs after the invocation.
pub(super) fn record_target(
    value: &Expr,
    types: &Types,
    targets: &mut HashMap<Type, HashSet<usize>>,
) {
    if let ExprKind::Callable { function, .. } = &value.kind {
        targets.entry(value.ty).or_default().insert(*function);
        if let Some(signature) = types.method_signature(value.ty) {
            targets.entry(signature).or_default().insert(*function);
        }
    }
}

pub(super) fn validate_targets(
    functions: &[ir::Function],
    types: &Types,
    targets: &HashMap<Type, HashSet<usize>>,
) -> std::result::Result<(), Diagnostic> {
    // A cold callback adapter executes an async body and returns the awaited
    // result. An eager factory returns a coroutine object through a different ABI.
    for (signature, candidates) in targets {
        let (_, returns) = types.signature(*signature).unwrap();
        if matches!(returns, Type::Coroutine(_))
            && candidates
                .iter()
                .any(|&id| functions[id].returns == returns)
        {
            return Err(
                Diagnostic::from("an Awaitable callback must be defined with async def")
                    .classified(Kind::UnsupportedRepresentation),
            );
        }
    }
    Ok(())
}

impl Signature {
    fn has_defaults_after_receiver(&self) -> bool {
        self.params
            .iter()
            .skip(1)
            .any(|param| param.default.is_some())
    }
}

impl Checker<'_> {
    /// Give a native method a positional signature and capture its receiver when present.
    pub(super) fn builtin_callable(
        &mut self,
        node: &impl Ranged,
        receiver: Option<Expr>,
        builtin: Builtin,
        parameters: Vec<Type>,
        returns: Type,
    ) -> Expr {
        let mut locals: Vec<_> = receiver.iter().map(|value| value.ty).collect();
        locals.extend(&parameters);
        let mut arguments: Vec<_> = locals
            .iter()
            .enumerate()
            .map(|(slot, &ty)| Expr::local(slot, ty))
            .collect();
        // Membership IR takes the needle first, while a saved method binds its receiver first.
        if matches!(builtin, Builtin::SetContains) {
            arguments.swap(0, 1);
        }
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
                source_frame: false,
                recursion_error: diagnostic_message(
                    self.source,
                    node,
                    "recursive native method adapter",
                ),
            },
        );
        Expr {
            ty: self.types.callable(parameters, returns),
            kind: ExprKind::Callable {
                function: id,
                receiver: receiver.map(Box::new),
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
        let separator = if context.is_some() {
            Expr {
                ty: Type::Str,
                kind: ExprKind::Field(Box::new(Expr::local(0, first)), 0),
            }
        } else {
            Expr::local(0, Type::Str)
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
                    .error(site, "recursive calls through str.join are unsupported"),
                function: ir::Function {
                    params: vec![0, 1],
                    locals: vec![first, list],
                    returns: Type::Str,
                    body: vec![Statement::Return(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Builtin(
                            Builtin::StringJoin,
                            vec![separator, Expr::local(1, list)],
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
        if signature.asynchronous {
            self.types
                .set_coroutine_target(signature.id, crate::types::CoroutineTarget::Coroutine);
        }
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
        let class_method = signature.method.is_some_and(|method| {
            method.kind(self.classes.declarations) == crate::classes::MethodKind::Class
        });
        let method_site = signature
            .method
            .filter(|_| class_method || (receiver.is_some() && !signature.asynchronous));
        let mut signature = signature.clone();
        // A saved method with defaults can infer missing formals at its later source-bound call.
        if let Some(inference) = self.inference
            && (expected.is_some()
                || method_site.is_none()
                || class_method
                || !signature.has_defaults_after_receiver())
        {
            let asynchronous = signature.asynchronous;
            inference.callable_signature(
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
                self,
                node,
            )?;
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
        let ty = if let Some(method) = method_site {
            // Inherited reads share a source binder even when they select different
            // native bodies. Keep specialized layouts and nonliteral defaults local.
            let class = &self.classes.declarations.definitions[method.class];
            let owner = self.classes.declarations.ids[&class.method_origins[method.index]];
            let method = if class_method
                && owner != method.class
                && let Some(original) =
                    self.classes.methods[owner].get(class.methods[method.index].name.as_str())
                && original.returns == signature.returns
                && original
                    .params
                    .iter()
                    .map(|p| p.ty)
                    .eq(signature.params.iter().map(|p| p.ty))
                && class.methods[method.index]
                    .parameters
                    .iter_non_variadic_params()
                    .all(|p| p.default.as_deref().is_none_or(ast::Expr::is_literal_expr))
            {
                original.method.unwrap()
            } else {
                method
            };
            self.types.source_method(
                ty,
                method.class,
                method.index,
                receiver.as_ref().map(|r| r.ty),
            )
        } else if unbound && let Some(&policy) = self.classes.cached.get(&signature.id) {
            self.types.declared_cache(ty, signature.id, policy)
        } else {
            ty
        };
        // The inherited spelling retains its checked receiver type. Its source
        // function accepts that instance through the declaring class's native ABI.
        let function = if unbound
            && !signature.asynchronous
            && let Some(method) = signature.method
            && method.kind(self.classes.declarations) == crate::classes::MethodKind::Instance
            && !self.classes.cached.contains_key(&signature.id)
        {
            let class = &self.classes.declarations.definitions[method.class];
            let owner = self.classes.declarations.ids[&class.method_origins[method.index]];
            self.classes.methods[owner]
                .get(class.methods[method.index].name.as_str())
                .filter(|original| {
                    owner != method.class
                        && !original.asynchronous
                        && !self.classes.producer_items.contains_key(&original.id)
                        && !self.classes.cached.contains_key(&original.id)
                        && original.returns == signature.returns
                        && original
                            .params
                            .first()
                            .is_some_and(|p| p.ty == Type::Class(owner))
                        && signature
                            .params
                            .first()
                            .is_some_and(|p| p.ty == Type::Class(method.class))
                        && original.params.iter().skip(1).map(|p| p.ty).eq(signature
                            .params
                            .iter()
                            .skip(1)
                            .map(|p| p.ty))
                })
                .map_or(signature.id, |original| original.id)
        } else {
            signature.id
        };
        let value = Expr {
            ty,
            kind: ExprKind::Callable {
                function,
                receiver: receiver.map(Box::new),
            },
        };
        if unbound {
            Ok(Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::StaticFunction(function), vec![value]),
            })
        } else if let Some(expected) = expected {
            self.coerce(value, expected, node)
        } else {
            Ok(value)
        }
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
        if matches!(*lambda.body, ast::Expr::Yield(_)) {
            return Err(self.object_builtin_error("generator lambda", lambda));
        }
        // A declared result still needs the fixed Callable type checks.
        let layout = if returns.is_some() {
            Kind::UnsupportedRepresentation
        } else {
            Kind::ScopedObject
        };
        let declared = lambda.parameters.as_deref();
        let object_signature = declared.is_some_and(|parameters| {
            parameters.vararg.is_some()
                || parameters.kwarg.is_some()
                || !parameters.kwonlyargs.is_empty()
                || parameters
                    .iter_non_variadic_params()
                    .any(|parameter| parameter.default.is_some())
        });
        let mut unavailable = object_signature.then(|| {
            self.classified_error(
                layout,
                lambda,
                "lambda defaults and variadic parameters require the registered binder",
            )
        });
        let names = declared
            .into_iter()
            .flat_map(|parameters| parameters.iter_non_variadic_params())
            .collect::<Vec<_>>();
        let parameters = if object_signature {
            for parameter in &names {
                if let Some(default) = &parameter.default {
                    self.expr(default)?;
                }
            }
            vec![Type::PythonObject; names.len()]
        } else if let Some(parameters) = parameters {
            parameters
        } else {
            if !names.is_empty() {
                unavailable = Some(self.classified_error(
                    Kind::ScopedObject,
                    lambda,
                    "lambda parameter layouts require the registered binder",
                ));
            }
            vec![Type::PythonObject; names.len()]
        };
        if parameters.len() != names.len() {
            return Err(error(
                self.source,
                lambda,
                "lambda parameter count does not match its Callable context",
            ));
        }
        let mut bound = narrowing::expression_assignments(&lambda.body);
        bound.extend(names.iter().map(|parameter| parameter.name().to_string()));
        let variadics = declared
            .into_iter()
            .flat_map(|params| {
                params
                    .vararg
                    .as_deref()
                    .map(|param| (param, self.types.var_tuple(Type::PythonObject)))
                    .into_iter()
                    .chain(
                        params
                            .kwarg
                            .as_deref()
                            .map(|param| (param, self.types.dict(Type::Str, Type::PythonObject))),
                    )
            })
            .collect::<Vec<_>>();
        bound.extend(variadics.iter().map(|(param, _)| param.name.to_string()));
        let mut captures = function_scope::expression_reads(&lambda.body)
            .into_iter()
            .filter(|name| !bound.contains(name))
            .filter_map(|name| self.names.get(&name).map(|&slot| (slot, name)))
            .collect::<Vec<_>>();
        if !pull && !captures.is_empty() {
            unavailable.get_or_insert_with(|| {
                self.classified_error(
                    layout,
                    lambda,
                    "an escaping lambda requires closure cells in its registered frame",
                )
            });
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
        // Keep the binder's layout failure while checking the lambda body for
        // independent source errors, just as for any other checked region.
        *helper.unrepresented.get_mut() = unavailable;
        helper.frame = LexicalFrame::Named(ast::name::Name::new_static("<lambda>"));
        helper.omitted_assert_free = omitted_free;
        helper.inference = self.inference;
        helper.class = self.class;
        helper.class_receiver = self
            .class_receiver
            .clone()
            .filter(|name| !bound.contains(name));
        let mut params = Vec::new();
        for (parameter, &ty) in names.iter().zip(&parameters) {
            let slot = helper.bind_parameter(&parameter.parameter, ty, None)?;
            params.push(slot);
        }
        for (parameter, ty) in variadics {
            params.push(helper.bind_parameter(parameter, ty, None)?);
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
            let slot = helper.declare_binding(&name, value.ty, lambda, true)?;
            helper.assigned.insert(slot);
            // An escaping lambda reads the later cell value. Only its lexical annotation survives.
            let origin = if pull {
                self.assigned.value_source(source)
            } else {
                self.declared_slots
                    .get(&source)
                    .copied()
                    .map(ValueSource::Declaration)
            };
            helper.assigned.set_value_source(slot, origin);
            params.push(slot);
            args.push(value);
        }
        let result = if let Some(returns) = returns {
            helper.expr_expected(&lambda.body, returns)
        } else {
            helper.expr(&lambda.body)
        };
        let body = helper.finish_region(result)?;
        let returns = body.ty;
        let mut body = vec![Statement::Return(body)];
        helper.fault_frame(&mut body, "<lambda>", lambda.start());
        self.helpers.finish(
            id,
            comprehensions::Helper {
                module: self.module,
                function: ir::Function {
                    params,
                    returns,
                    locals: helper.locals,
                    body,
                },
                calls: helper.calls,
                class_reads: helper.class_reads.into_inner(),
                source_frame: true,
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
        if callable.ty == Type::PythonObject {
            return self.call_with_target(call, callable);
        }
        if let Type::WeakProxy(class) = callable.ty {
            return self.weak_proxy_call(call, callable, class);
        }
        if let Type::Class(class) = callable.ty
            && self.classes.methods[class].contains_key("__call__")
        {
            return self.dispatch_record(class, callable, call, |checker, layout, receiver| {
                let signature = checker.classes.methods[layout]["__call__"].clone();
                checker.source_call(call, &signature, Some(receiver), 1)
            });
        }
        if let Type::Placeholder(kind) = callable.ty {
            return self.placeholder_call(call, callable, kind);
        }
        if let Type::CmpKey(frame, _) = callable.ty {
            return self.bind_cmp_key(call, callable, frame);
        }
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
            && self.bundled_mock_class(class, "_PatchFactory")
        {
            return Err(self.object_runtime_export("unittest.mock.patch", call));
        }
        if let Type::Class(class) = callable.ty
            && self.native_mock(class)
        {
            return self.mock_arguments(call, class, callable, "_record");
        }
        if let Some(result) = self.invoke_bound_unittest(call, &callable)? {
            return Ok(result);
        }
        if let Some((class, index, receiver)) = self.types.source_method_site(callable.ty) {
            let method = &self.classes.declarations.definitions[class].methods[index];
            let signature = &self.classes.methods[class][method.name.as_str()];
            // A required positional instance call already has its receiver-bound ABI.
            let bind_declaration = receiver.is_none_or(|ty| ty == Type::BuiltinClass)
                || !call.arguments.keywords.is_empty()
                || signature.has_defaults_after_receiver();
            if !signature.asynchronous && bind_declaration {
                let signature = signature.clone();
                let (returns, args) = self.bind_source(
                    call,
                    &signature,
                    None,
                    usize::from(receiver.is_some()),
                    None,
                )?;
                return Ok(Expr {
                    ty: returns,
                    kind: ExprKind::Invoke {
                        callable: Box::new(callable),
                        args,
                    },
                });
            }
        }
        let (params, returns) = self.types.signature(callable.ty).ok_or_else(|| {
            error(
                self.source,
                call,
                "an indirect call requires a nonoptional exact Callable value",
            )
        })?;
        self.positional_invoke(call, callable, params, returns, None)
    }

    pub(super) fn registered_callback(&mut self, expr: &ast::Expr) -> Result<Option<Expr>, String> {
        let Some(call) = expr.as_call_expr() else {
            return Ok(None);
        };
        let Some(slot) = call
            .func
            .as_name_expr()
            .and_then(|name| self.names.get(name.id.as_str()))
            .copied()
        else {
            return Ok(None);
        };
        let ty = self.locals[slot];
        if !self.assigned.contains(&slot) || !registered::values::fixed_positional_arguments(call) {
            return Ok(None);
        }
        let Some((params, returns)) = self
            .types
            .with_callback_signature(ty, |params, returns| (params.to_vec(), returns))
        else {
            return Ok(None);
        };
        self.positional_invoke(call, self.local(slot), params, returns, Some(expr))
            .map(Some)
    }

    fn positional_invoke(
        &mut self,
        call: &ast::ExprCall,
        callable: Expr,
        params: Vec<Type>,
        returns: Type,
        registered_site: Option<&ast::Expr>,
    ) -> Result<Expr, String> {
        let needs_cell_default = call.arguments.args.len() < params.len()
            && matches!(callable.kind, ExprKind::Local(slot)
                if self.assigned.value_source(slot) == Some(ValueSource::ReboundMethod));
        if !call.arguments.keywords.is_empty()
            || (call.arguments.args.len() != params.len() && !needs_cell_default)
        {
            return Err(error(
                self.source,
                call,
                "indirect calls require the full positional Callable signature; keywords and omitted defaults are unsupported",
            ));
        }
        let local = call
            .func
            .as_name_expr()
            .and_then(|name| self.names.get(name.id.as_str()));
        let saved = match &callable.kind {
            ExprKind::Local(slot) => self.saved_call_target(*slot),
            _ => None,
        };
        let local_callable = matches!(callable.kind, ExprKind::Local(slot)
            if local == Some(&slot) && self.locals[slot] == callable.ty && matches!(callable.ty, Type::Callable(_)));
        let index_callable = self.types.index_callable_signature(callable.ty).is_some();
        let values =
            call.arguments
                .args
                .iter()
                .zip(params)
                .enumerate()
                .map(|(slot, (argument, ty))| {
                    if registered_site.is_some() {
                        // A Python activation passes the source object, including its identity.
                        // A native conversion or expected display layout cannot supply that handle.
                        let value = self.expr(argument)?;
                        if value.ty != ty {
                            return Err(self
                                .scoped_builtin_error("a converted callable argument", argument));
                        }
                        return Ok(value);
                    }
                    if slot == 0 && index_callable {
                        self.integer_index(argument)
                    } else {
                        calls::Supplied::Source(argument).check_with(
                            self,
                            ty,
                            false,
                            None,
                            |checker, value, expected, site| {
                                checker.coerce(value, expected, site).map_err(|error| {
                                    error.at_callable_argument(site.range(), || {
                                        local_callable
                                            && checker.graph.session.calls_annotated_parameter(
                                                checker.module,
                                                call,
                                                site.range(),
                                                &checker.graph.modules[checker.module].parsed,
                                            )
                                    })
                                })
                            },
                        )
                    }
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
        if needs_cell_default {
            // The checked positional ABI constrains supplied arguments. The current
            // function in the shared cell owns the defaults for the remaining slots.
            self.require_registered_operation(call)?;
            return Ok(Expr {
                ty: returns,
                ..Expr::unrepresented()
            });
        }
        if let Some(expr) = registered_site {
            return Ok(self.located_callback(expr, call, callable, values));
        }
        if let Type::Coroutine(_) = returns {
            return Ok(self.indirect_coroutine(callable, values, returns));
        }
        let mut args = ir::CallArgs::positional(values);
        if let Some(id) = saved {
            args.site = ir::CallSite::SavedFunction(self.call_origin(call), id);
        }
        Ok(Expr {
            ty: returns,
            kind: ExprKind::Invoke {
                callable: Box::new(callable),
                args,
            },
        })
    }

    pub(super) fn saved_call_target(&self, slot: usize) -> Option<usize> {
        if self.tracks_called_bindings()
            && self.module_namespace.is_unexposed()
            && let Some(ValueSource::Function(id)) = self.assigned.value_source(slot)
        {
            Some(id)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stored_provider_callable_selects_its_own_unsolved_signature() {
        use crate::frontend::{lower_with_caller_cache, phase::Kind};
        use crate::modules::SourceGraph;
        use std::fs;

        let project = tempfile::tempdir().unwrap();
        let installed = tempfile::tempdir().unwrap();
        let provider = installed.path().join("support.py");
        fs::write(
            &provider,
            "def earlier(text): return text\ndef selected(exc): return exc\n",
        )
        .unwrap();
        let entry = project.path().join("app.py");
        fs::write(
            &entry,
            "import support\ntry:\n    raise KeyError('marker')\nexcept KeyError as error:\n    helper = support.selected\n    print(helper(error))\n",
        )
        .unwrap();
        let mut graph = SourceGraph::load(&entry, None, &[installed.path().to_path_buf()]).unwrap();
        let diagnostic = lower_with_caller_cache(&mut graph, None, None)
            .err()
            .unwrap();
        assert_eq!(diagnostic.kind, Kind::ScopedObject);
        assert_eq!(
            diagnostic.to_string(),
            format!(
                "{}:2:1: Imre: using this function as a value requires Python argument binding",
                provider.canonicalize().unwrap().display()
            ),
        );
    }

    #[test]
    fn registered_entry_reuses_the_typed_callback_body() {
        use crate::frontend::caller_cache::observe_lowerings;
        use crate::ir::{self, ExprKind, Type};
        use crate::modules::SourceGraph;
        use ruff_text_size::Ranged;

        let source = concat!(
            "from typing import Callable\n",
            "def report(n: int, sink: Callable[[int], None]) -> None:\n",
            "    sink(n + 1)\n",
            "def first(n): print(n)\n",
            "def second(n): print(n)\n",
            "def publish(sink): sink(3)\n",
            "def marker(n: int) -> None: print(n)\n",
            "report(7, marker)\n",
            "handlers = {'first': first, 'second': second}\n",
            "publish(handlers[input()])\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, source).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        let (program, lowered) = observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert!(matches!(program, crate::CheckedProgram::Registered(_)));
        let site = graph.modules[graph.entry].parsed.syntax().body[1].start();
        let bodies = graph.registered_functions();
        let body = &bodies[&graph.entry][&site].function;
        assert_eq!(lowered.iter().filter(|name| *name == "report").count(), 1);
        assert!(matches!(body.locals[body.params[1]], Type::Callable(_)));
        let mut invokes = 0;
        ir::walk_expressions(&body.body, &mut |expr| {
            if let ExprKind::Invoke { callable, args } = &expr.kind {
                assert!(expr.is_registered_invoke());
                assert!(matches!(callable.ty, Type::Callable(_)));
                assert_eq!(args.values[0].ty, Type::Int);
                let range = args.site.python().unwrap().call.range;
                assert_eq!(&source[range], "sink(n + 1)");
                invokes += 1;
            }
        });
        assert_eq!(invokes, 1);

        std::fs::write(&path, source.replace("sink(n + 1)", "sink('bad')")).unwrap();
        let error = crate::check_file(&path, None).unwrap_err();
        assert!(
            error.contains("app.py:3:10: Imre: expected int, got str"),
            "{error}"
        );
    }

    #[test]
    fn a_named_loop_tests_the_checked_callbacks_python_result() {
        use crate::ir;
        use crate::modules::SourceGraph;
        use ruff_text_size::Ranged;

        let source = include_str!("../../../../tests/fixtures/boxed_while_truth.py");
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app.py");
        std::fs::write(&app, "import subject\nsubject.current()\n").unwrap();
        std::fs::write(dir.path().join("subject.py"), source).unwrap();
        let mut graph = SourceGraph::load(&app, None, &[]).unwrap();
        graph.registered_startup = true;
        crate::checked_lower(&mut graph).unwrap();

        let (module, subject) = graph
            .modules
            .iter()
            .enumerate()
            .find(|(_, module)| module.name == "subject")
            .unwrap();
        let consume = subject
            .parsed
            .syntax()
            .body
            .iter()
            .find_map(|stmt| {
                stmt.as_function_def_stmt()
                    .filter(|f| f.name.as_str() == "consume")
            })
            .unwrap();
        let functions = graph.registered_functions();
        let body = &functions[&module][&consume.start()].function;
        let mut calls = Vec::new();
        ir::walk_expressions(&body.body, &mut |expr| {
            if expr.is_registered_invoke()
                && let ir::ExprKind::Invoke { args, .. } = &expr.kind
            {
                calls.push(&source[args.site.python().unwrap().call.range]);
            }
        });
        assert_eq!(
            calls.iter().filter(|&&site| site == "right()").count(),
            1,
            "{calls:?}"
        );
    }

    #[test]
    fn callable_parameter_owns_its_argument_in_a_checked_file() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.py");
        let declarations = "import ast\nfrom collections.abc import Callable\ndef take(x: str) -> str: return x\ndef apply(take: Callable[[int], int]) -> int:\n";
        std::fs::write(&main, format!("{declarations}    return take('x')\n")).unwrap();
        let error = crate::check_file(&main, None).unwrap_err();
        assert!(
            error.contains("main.py:5:17: Imre: expected int, got str"),
            "{error}"
        );

        std::fs::write(&main, format!("{declarations}    return take(1)\n")).unwrap();
        crate::check_file(&main, None).unwrap();
    }

    #[test]
    fn eager_coroutine_factories_remain_direct_calls() {
        use crate::frontend::{lower, lower_with_caller_cache, phase::Kind};
        use crate::ir::{self, Builtin, ExprKind, ProcessOperation};
        use crate::modules::SourceGraph;

        let source = "from collections.abc import Awaitable, Callable\nasync def cold(x: int) -> int:\n    return x\ndef factory(x: int) -> Awaitable[int]:\n    return cold(x)\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("callbacks.py");
        for callback in ["cold", "factory", "lambda x: cold(x)"] {
            let program = format!(
                "{source}async def run() -> int:\n    cb: Callable[[int], Awaitable[int]] = {callback}\n    return await cb(7)\n"
            );
            let mut graph = SourceGraph::single(&program).unwrap();
            if callback == "cold" {
                let module = lower(&graph).unwrap();
                let mut indirect = false;
                ir::walk_expressions(&module.functions[2].body, &mut |expr| {
                    indirect |= matches!(
                        expr.kind,
                        ExprKind::Builtin(
                            Builtin::Process(ProcessOperation::IndirectCoroutine { .. }),
                            _
                        )
                    );
                });
                assert!(indirect);
            } else {
                let Err(error) = lower_with_caller_cache(&mut graph, None, None) else {
                    panic!("eager factory cannot enter a cold callback adapter");
                };
                assert_eq!(error.kind, Kind::UnsupportedRepresentation);
                assert_eq!(
                    error.to_string(),
                    "an Awaitable callback must be defined with async def"
                );
            }
            std::fs::write(&path, &program).unwrap();
            crate::check(&program).unwrap();
            crate::check_file(&path, None).unwrap();
        }
        let direct = format!("{source}async def run() -> int:\n    return await factory(7)\n");
        let module = lower(&SourceGraph::single(&direct).unwrap()).unwrap();
        let mut calls_factory = false;
        ir::walk_expressions(&module.functions[2].body, &mut |expr| {
            calls_factory |= matches!(expr.kind, ExprKind::Call(1, _));
        });
        assert!(calls_factory);
    }
}
