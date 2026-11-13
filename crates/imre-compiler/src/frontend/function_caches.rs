//! Native memoization for module declarations and independently constructed wrappers.

use super::*;
pub(super) use crate::types::CachePolicy as Policy;
use std::result::Result;

pub(super) fn policy(
    graph: &SourceGraph,
    module: ModuleId,
    def: &ast::StmtFunctionDef,
) -> Result<Option<Policy>, String> {
    decorator_policy(graph, module, def, |expr| graph.resolve(module, expr))
}

fn decorator_policy(
    graph: &SourceGraph,
    module: ModuleId,
    def: &ast::StmtFunctionDef,
    resolve: impl Fn(&ast::Expr) -> Option<Binding>,
) -> Result<Option<Policy>, String> {
    let [decorator] = def.decorator_list.as_slice() else {
        return Ok(None);
    };
    let expression = &decorator.expression;
    match resolve(expression) {
        Some(Binding::Intrinsic(Intrinsic::FunctionCache)) => {
            return Ok(Some(Policy {
                capacity: -1,
                typed: false,
            }));
        }
        Some(Binding::Intrinsic(Intrinsic::LruCache)) => {
            return Ok(Some(Policy {
                capacity: 128,
                typed: false,
            }));
        }
        _ => {}
    }
    let ast::Expr::Call(call) = expression else {
        return Ok(None);
    };
    if resolve(&call.func) != Some(Binding::Intrinsic(Intrinsic::LruCache)) {
        return Ok(None);
    }
    configuration(graph, module, call).map(Some)
}

fn configuration(
    graph: &SourceGraph,
    module: ModuleId,
    call: &ast::ExprCall,
) -> Result<Policy, String> {
    let invalid = || {
        graph.modules[module].error(call, "lru_cache requires a constant maxsize and typed flag")
    };
    let args = &call.arguments;
    if args.args.len() > 2 {
        return Err(invalid());
    }
    let mut size = args.args.first();
    let mut typed = args.args.get(1);
    for kw in &args.keywords {
        let slot = match kw.arg.as_deref() {
            Some("maxsize") => &mut size,
            Some("typed") => &mut typed,
            _ => return Err(invalid()),
        };
        if slot.replace(&kw.value).is_some() {
            return Err(invalid());
        }
    }
    if typed.is_some_and(|e| !matches!(e, ast::Expr::BooleanLiteral(_))) {
        return Err(invalid());
    }
    let size = match size {
        None => 128,
        Some(ast::Expr::NoneLiteral(_)) => -1,
        Some(ast::Expr::NumberLiteral(n)) if let ast::Number::Int(i) = &n.value => {
            i.as_i64().ok_or_else(invalid)?
        }
        Some(ast::Expr::UnaryOp(op))
            if op.op == ast::UnaryOp::USub
                && matches!(&*op.operand, ast::Expr::NumberLiteral(n) if matches!(n.value, ast::Number::Int(_))) =>
        {
            0
        }
        _ => return Err(invalid()),
    };
    Ok(Policy {
        capacity: size,
        typed: matches!(typed, Some(ast::Expr::BooleanLiteral(b)) if b.value),
    })
}

fn builtin(op: Builtin, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(op, args),
    }
}

pub(super) fn wrapper(
    signature: &Signature,
    raw: usize,
    types: &Types,
    policy: Policy,
) -> ir::Function {
    build_wrapper(
        types,
        &signature.params.iter().map(|p| p.ty).collect::<Vec<_>>(),
        signature.returns,
        policy,
        Backing::Module {
            id: signature.id,
            raw,
        },
    )
}

enum Backing {
    Module { id: usize, raw: usize },
    Owned(crate::types::CachedFunction),
}

fn field(owner: Expr, slot: usize, ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Field(Box::new(owner), slot),
    }
}

fn cache_table(types: &Types, params: &[Type], result: Type) -> Type {
    if let [arg] = params
        && types.is_len_argument(*arg)
    {
        return types.dict(types.len_key(), types.tuple(vec![*arg, result]));
    }
    let key = match params {
        [] => Type::Int,
        [key] => *key,
        _ => types.tuple(params.to_vec()),
    };
    types.dict(key, result)
}

fn build_wrapper(
    types: &Types,
    parameters: &[Type],
    value: Type,
    policy: Policy,
    backing: Backing,
) -> ir::Function {
    let context = match backing {
        Backing::Module { .. } => None,
        Backing::Owned(layout) => Some(layout.frame),
    };
    let mut locals: Vec<_> = context
        .into_iter()
        .chain(parameters.iter().copied())
        .collect();
    let params: Vec<_> = (0..locals.len()).collect();
    let args: Vec<_> = parameters
        .iter()
        .enumerate()
        .map(|(i, &ty)| Expr::local(i + usize::from(context.is_some()), ty))
        .collect();
    let mut body = Vec::new();
    let len_arg = match args.as_slice() {
        [arg] if types.is_len_argument(arg.ty) => Some(arg.clone()),
        _ => None,
    };
    if policy.capacity != 0 {
        for arg in &args {
            if types.is_len_argument(arg.ty) {
                let message = field(arg.clone(), 1, Type::Str);
                body.push(Statement::If(
                    builtin(Builtin::Bool, Type::Bool, vec![message.clone()]),
                    vec![Statement::raise(Expr {
                        ty: Type::Exception,
                        kind: ExprKind::Exception {
                            kind: exception_kind("TypeError").unwrap(),
                            message: Some(Box::new(message)),
                        },
                    })],
                    vec![],
                ));
            }
        }
    }
    let key = match args.as_slice() {
        [] => Expr::integer(0),
        [arg] if types.is_len_argument(arg.ty) => field(arg.clone(), 2, types.len_key()),
        [arg] => arg.clone(),
        _ => {
            let ty = types.tuple(parameters.to_vec());
            let slot = locals.len();
            locals.push(ty);
            body.push(Statement::Assign(
                slot,
                Expr {
                    ty,
                    kind: ExprKind::Tuple(args.clone()),
                },
            ));
            Expr::local(slot, ty)
        }
    };
    let table = cache_table(types, parameters, value);
    let stored = types.mapping(table).unwrap().1;
    let state_ty = table.cache_state();
    let state_slot = locals.len();
    let table_slot = state_slot + 1;
    let result_slot = table_slot + 1;
    locals.extend([state_ty, table, value]);
    let captured = match backing {
        Backing::Module { id, .. } => builtin(Builtin::ModuleCache(id), state_ty, vec![]),
        Backing::Owned(layout) => field(Expr::local(0, layout.frame), 1, state_ty),
    };
    body.push(Statement::Assign(state_slot, captured));
    let state = || Expr::local(state_slot, state_ty);
    let cache = || builtin(Builtin::CacheTable, table, vec![state()]);
    let probe = builtin(
        Builtin::CacheProbe(policy.capacity),
        Type::Bool,
        vec![state(), key.clone()],
    );
    body.push(Statement::Assign(table_slot, cache()));
    let hit = builtin(
        Builtin::DictGet,
        stored,
        vec![Expr::local(table_slot, table), key.clone()],
    );
    let hit = if len_arg.is_some() {
        field(hit, 1, value)
    } else {
        hit
    };
    body.push(Statement::If(probe, vec![Statement::Return(hit)], vec![]));
    body.push(Statement::Clear(table_slot));
    body.push(Statement::Assign(
        result_slot,
        Expr {
            ty: value,
            kind: match backing {
                Backing::Module { raw, .. } => ExprKind::Call(raw, ir::CallArgs::positional(args)),
                Backing::Owned(layout) => ExprKind::Invoke {
                    callable: Box::new(field(Expr::local(0, layout.frame), 0, layout.callable)),
                    args: ir::CallArgs::positional(args),
                },
            },
        },
    ));
    // A cache using a normalized lookup key still owns the originally supplied object.
    let entry = match len_arg {
        Some(arg) => Expr {
            ty: stored,
            kind: ExprKind::Tuple(vec![arg, Expr::local(result_slot, value)]),
        },
        None => Expr::local(result_slot, value),
    };
    if policy.capacity < 0 {
        // A callback can clear its own cache. Use the table installed after it returns.
        body.push(Statement::Assign(table_slot, cache()));
        body.push(Statement::assign_item(
            Expr::local(table_slot, table),
            key,
            entry,
        ));
    } else if policy.capacity > 0 {
        body.push(Statement::Expr(builtin(
            Builtin::CacheInsert(policy.capacity),
            Type::None,
            vec![state(), key, entry],
        )));
    }
    body.push(Statement::Return(Expr::local(result_slot, value)));
    ir::Function {
        params,
        returns: value,
        locals,
        body,
    }
}

fn context_field(
    wrapper: Expr,
    layout: crate::types::CachedFunction,
    index: usize,
    ty: Type,
) -> Expr {
    field(
        builtin(Builtin::CallableContext, layout.frame, vec![wrapper]),
        index,
        ty,
    )
}

impl Checker<'_> {
    pub(super) fn local_cache_policy(
        &self,
        def: &ast::StmtFunctionDef,
    ) -> Result<Option<Policy>, String> {
        decorator_policy(self.graph, self.module, def, |expr| self.resolve(expr))
    }

    pub(super) fn local_annotation_origin(
        &self,
        function: &local_functions::Function,
    ) -> crate::types::AnnotationOrigin {
        self.helpers
            .local_annotations
            .borrow_mut()
            .entry(function.signature.id)
            .or_insert_with(|| (self.module, function.definition.clone()));
        crate::types::AnnotationOrigin::Local(function.signature.id)
    }

    /// A direct decorator or an applied constant configuration captures its actual target once.
    pub(super) fn cache_constructor(
        &mut self,
        call: &ast::ExprCall,
        binding: Option<Binding>,
    ) -> phase::Result<Option<Expr>, String> {
        let policy = match binding {
            Some(Binding::Intrinsic(Intrinsic::FunctionCache)) => Policy {
                capacity: -1,
                typed: false,
            },
            Some(Binding::Intrinsic(Intrinsic::LruCache)) => Policy {
                capacity: 128,
                typed: false,
            },
            _ => {
                let ast::Expr::Call(config) = &*call.func else {
                    return Ok(None);
                };
                if self.resolve(&config.func) != Some(Binding::Intrinsic(Intrinsic::LruCache)) {
                    return Ok(None);
                }
                configuration(self.graph, self.module, config)?
            }
        };
        let [source] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "a cache decorator needs a callable with a known signature",
            ));
        };
        self.reject_builtin_keywords(call)?;
        let callable = self.expr(source)?;
        let annotations = if let Some(binding) = self.local_function_binding(source) {
            Some(self.local_annotation_origin(binding.target(self.source, source)?))
        } else if let Some(Binding::Definition(d)) = self.resolve(source) {
            self.functions
                .contains_key(&d)
                .then_some(crate::types::AnnotationOrigin::Module(d))
        } else {
            self.types
                .cached_function(callable.ty)
                .and_then(|layout| layout.annotations)
        };
        self.owned_cache(callable, policy, annotations, call)
            .map(Some)
    }

    /// Each execution of a decorator owns its captured callable and fresh table.
    pub(super) fn owned_cache(
        &mut self,
        callable: Expr,
        policy: Policy,
        annotations: Option<crate::types::AnnotationOrigin>,
        site: &impl Ranged,
    ) -> phase::Result<Expr, String> {
        let (params, returns) = self.types.signature(callable.ty).ok_or_else(|| {
            error(
                self.source,
                site,
                "a cache decorator requires a callable with a known signature",
            )
        })?;
        if matches!(returns, Type::Coroutine(_))
            || !params
                .iter()
                .all(|&ty| self.types.hashable_key(ty) || self.types.is_len_argument(ty))
        {
            return Err(error(
                self.source,
                site,
                "cached functions require exact immutable positional keys and a synchronous result",
            ));
        }
        let table = cache_table(self.types, &params, returns);
        let frame = self
            .types
            .partial_frame(vec![callable.ty, table.cache_state()]);
        let layout = crate::types::CachedFunction {
            callable: callable.ty,
            frame,
            table,
            capacity: policy.capacity,
            annotations,
        };
        let id = self.helpers.reserve();
        self.helpers.finish(
            id,
            comprehensions::Helper {
                module: self.module,
                calls: HashSet::new(),
                class_reads: HashSet::new(),
                source_frame: false,
                recursion_error: self.graph.modules[self.module].error(
                    site,
                    "recursive indirect cache applications are unsupported",
                ),
                function: build_wrapper(
                    self.types,
                    &params,
                    returns,
                    policy,
                    Backing::Owned(layout),
                ),
            },
        );
        let state = builtin(Builtin::CacheNew, table.cache_state(), vec![]);
        let frame = Expr {
            ty: frame,
            kind: ExprKind::Tuple(vec![callable, state]),
        };
        Ok(Expr {
            ty: self.types.cached_callable(layout, id, policy),
            kind: ExprKind::Callable {
                function: id,
                receiver: Some(Box::new(frame)),
            },
        })
    }

    pub(super) fn callable_attribute(
        &mut self,
        receiver: &Expr,
        attribute: &ast::ExprAttribute,
    ) -> phase::Result<Option<Expr>, String> {
        let name = attribute.attr.as_str();
        if let Some(base) = self.types.constructor_base(receiver.ty)
            && self.classes.methods[base].get(name).is_some_and(|sig| {
                sig.method.is_some_and(|m| {
                    m.kind(self.classes.declarations) == crate::classes::MethodKind::Class
                })
            })
        {
            return Err(
                self.scoped_builtin_error("descriptor lookup on a class factory", attribute)
            );
        }
        let site = self.types.source_method_site(receiver.ty);
        if site.is_some_and(|(_, _, ty)| matches!(ty, None | Some(Type::BuiltinClass)))
            && typing::attribute_index(name).is_some()
        {
            return Err(self.scoped_builtin_error("source method metadata", attribute));
        }
        if !matches!(name, "__func__" | "__self__" | "__wrapped__")
            || !matches!(receiver.ty, Type::Callable(_))
        {
            return Ok(None);
        }
        let mut scope = calls::ArgumentScope::default();
        let owner = if let Some((class, index, Some(ty))) = site {
            if name == "__self__" {
                return Ok(matches!(ty, Type::Class(_) | Type::BuiltinClass)
                    .then(|| builtin(Builtin::CallableContext, ty, vec![receiver.clone()])));
            }
            let record = &self.classes.declarations.definitions[class];
            let method = &record.methods[index];
            let owner = if ty == Type::BuiltinClass {
                // Only a receiver-independent body accepts an arbitrary explicit cls.
                // A specialized body must resolve methods against the class supplied by the caller.
                let declaration = &method.function;
                let origin = record.method_origins[index];
                let receiver_name = declaration
                    .parameters
                    .iter_non_variadic_params()
                    .next()
                    .unwrap()
                    .name();
                if !self.graph.session.function_reads_only(
                    origin.module,
                    declaration,
                    false,
                    |name| name != receiver_name.as_str(),
                    |_| false,
                ) {
                    return Err(
                        self.scoped_builtin_error("the declaring classmethod function", attribute)
                    );
                }
                self.classes.declarations.ids[&origin]
            } else {
                class
            };
            let Some(signature) = self.classes.methods[owner]
                .get(method.name.as_str())
                .cloned()
            else {
                return Err(self.scoped_builtin_error("the declaring method function", attribute));
            };
            scope.hold(self, receiver.clone());
            self.callable_value(&signature, None, None, attribute)?
        } else if name != "__wrapped__" {
            return Ok(None);
        } else {
            receiver.clone()
        };
        if name == "__func__" {
            return Ok(Some(scope.finish(owner)));
        }
        self.wrapper_link.set(true);
        // A load spanning lines is positioned at the attribute name.
        let range =
            if self.source_line(attribute.start()) == self.source_line(attribute.attr.start()) {
                attribute.range()
            } else {
                attribute.attr.range()
            };
        let target = self.types.wrapped_link_type(owner.ty);
        Ok(Some(scope.finish(builtin(
            Builtin::FunctionWrappedGet(self.fixed_origin(range, None)),
            target,
            vec![owner],
        ))))
    }

    pub(super) fn cache_control(
        &mut self,
        call: &ast::ExprCall,
        receiver: Option<&Expr>,
    ) -> phase::Result<Option<Expr>, String> {
        let ast::Expr::Attribute(attr) = &*call.func else {
            return Ok(None);
        };
        if !matches!(
            attr.attr.as_str(),
            "cache_info" | "cache_clear" | "cache_parameters"
        ) {
            return Ok(None);
        }
        let name = attr.attr.as_str();
        let direct = match self.resolve(&attr.value) {
            Some(Binding::Definition(def)) => self
                .functions
                .get(&def)
                .filter(|sig| self.classes.cached.contains_key(&sig.id)),
            _ => None,
        };
        let (wrapper, declaration) = if let Some(sig) = direct {
            let params = sig.params.iter().map(|p| p.ty).collect();
            (None, Some((sig.id, params, sig.returns)))
        } else {
            let wrapper = match receiver {
                Some(v) => v.clone(),
                None => self.expr(&attr.value)?,
            };
            let declaration =
                self.types
                    .cache_declaration(wrapper.ty)
                    .and_then(|(callable, id)| {
                        let (params, result) = self.types.signature(callable)?;
                        Some((id, params, result))
                    });
            (Some(wrapper), declaration)
        };
        let (state, capacity) = if let Some((id, params, result)) = declaration {
            let policy = self.classes.cached[&id];
            if name == "cache_parameters" {
                return self.cache_parameters(Some(policy), wrapper, call).map(Some);
            }
            let table = cache_table(self.types, &params, result);
            let state = builtin(Builtin::ModuleCache(id), table.cache_state(), vec![]);
            (after_receiver(wrapper, state), policy.capacity)
        } else {
            let wrapper = wrapper.expect("the cache control receiver was evaluated");
            let Some(layout) = self.types.cached_function(wrapper.ty) else {
                return Ok(None);
            };
            if name == "cache_parameters" {
                return self.cache_parameters(None, Some(wrapper), call).map(Some);
            }
            (
                context_field(wrapper, layout, 1, layout.table.cache_state()),
                layout.capacity,
            )
        };
        if !call.arguments.is_empty() {
            return Err(error(
                self.source,
                call,
                "cache_info and cache_clear take no arguments",
            ));
        }
        if name == "cache_clear" {
            return Ok(Some(builtin(Builtin::CacheClear, Type::None, vec![state])));
        }
        let slot = self.hidden(state.ty);
        let snapshot = self.cache_snapshot(capacity, |field| {
            builtin(
                Builtin::CacheStat(field),
                Type::Int,
                vec![Expr::local(slot, state.ty)],
            )
        });
        Ok(Some(Expr {
            ty: snapshot.ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, state)],
                value: Box::new(snapshot),
                clear: vec![slot],
            },
        }))
    }

    /// Return a fresh ordered dictionary, preserving the Python types of both values.
    fn cache_parameters(
        &mut self,
        declaration: Option<Policy>,
        receiver: Option<Expr>,
        call: &ast::ExprCall,
    ) -> phase::Result<Expr, String> {
        if !call.arguments.is_empty() {
            return Err(error(
                self.source,
                call,
                "cache_parameters takes no arguments",
            ));
        }
        let mut body = Vec::new();
        let mut clear = Vec::new();
        let (size, typed) = if let Some(policy) = declaration {
            body.extend(receiver.map(Statement::Expr));
            let typed = Expr::boolean(policy.typed);
            (
                self.document_coerce(cache_size(policy.capacity), documents::ANY, call)?,
                self.document_coerce(typed, documents::ANY, call)?,
            )
        } else {
            let wrapper = receiver.expect("an applied cache has a native wrapper");
            let ty = wrapper.ty;
            let slot = self.hidden(ty);
            body.push(Statement::Assign(slot, wrapper));
            clear.push(slot);
            let parameter = |typed| {
                builtin(
                    Builtin::CacheParameter(typed),
                    documents::ANY,
                    vec![Expr::local(slot, ty)],
                )
            };
            (parameter(false), parameter(true))
        };
        let entries = vec![
            (unpacking::string("maxsize"), size),
            (unpacking::string("typed"), typed),
        ];
        let result = Expr {
            ty: self.types.dict(Type::Str, documents::ANY),
            kind: ExprKind::dict_pairs(entries),
        };
        Ok(Expr {
            ty: result.ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(result),
                clear,
            },
        })
    }

    fn cache_snapshot(&self, capacity: i64, stat: impl Fn(u32) -> Expr) -> Expr {
        let fields = vec![stat(0), stat(1), cache_size(capacity), stat(3)];
        Expr {
            ty: self.types.namedtuple_layout(
                crate::types::CACHE_INFO,
                fields.iter().map(|v| v.ty).collect(),
            ),
            kind: ExprKind::Tuple(fields),
        }
    }
}

fn cache_size(capacity: i64) -> Expr {
    if capacity < 0 {
        Expr::none()
    } else {
        Expr::integer(capacity)
    }
}

fn after_receiver(receiver: Option<Expr>, result: Expr) -> Expr {
    match receiver {
        Some(owner) => Expr {
            ty: result.ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Expr(owner)],
                value: Box::new(result),
                clear: vec![],
            },
        },
        None => result,
    }
}
