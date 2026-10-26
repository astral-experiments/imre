//! Lift typed local definitions into native functions. A captured binding is
//! loaded at each direct call, after its explicit arguments, as Python would
//! read the enclosing cell on entry. No callable with captures can escape.
use ruff_python_ast::visitor::{Visitor, walk_expr};
use std::collections::VecDeque;
use std::rc::Rc;

use super::*;

#[derive(Clone)]
pub(super) struct Function {
    pub signature: Signature,
    pub definition: Rc<ast::StmtFunctionDef>,
    captures: Vec<usize>,
}

#[derive(Clone)]
struct Site {
    range: TextRange,
    value_read: bool,
    cmp_input: bool,
    later_writes: HashSet<String>,
}

impl Site {
    fn new(function: &ast::StmtFunctionDef, following: &[ast::Stmt], cmp_input: bool) -> Self {
        Self {
            range: function.range,
            cmp_input: cmp_input && cmp_keys::can_specialize(function),
            value_read: reads_function_value(following, function.name.as_str()),
            later_writes: if function.is_async {
                narrowing::assigned_names(following)
            } else {
                HashSet::new()
            },
        }
    }
}

#[derive(Clone)]
struct Comparator {
    definition: Rc<ast::StmtFunctionDef>,
    instances: HashMap<(Type, Type), Function>,
}

#[derive(Clone)]
pub(super) struct Binding {
    sites: VecDeque<Site>,
    active_comparator: Option<u32>,
    // Factories made before a new def keep using the original comparator.
    comparators: HashMap<u32, Comparator>,
    pub(super) overload_stub: bool,
    function: Option<Function>,
    pub(super) generic: Option<Rc<ast::StmtFunctionDef>>,
    instances: HashMap<Type, Function>,
    later_writes: HashSet<String>,
    pub(super) value_slot: Option<usize>,
}

/// A direct call can target a lifted helper. Every other read of the name
/// must see the same callable object for this execution of its definition.
fn reads_function_value(body: &[ast::Stmt], name: &str) -> bool {
    struct Read<'n>(&'n str, bool);
    impl<'a> Visitor<'a> for Read<'_> {
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            if self.1 {
                return;
            }
            match expr {
                ast::Expr::Call(call) if matches!(&*call.func, ast::Expr::Name(n) if n.id == self.0) =>
                {
                    self.visit_arguments(&call.arguments);
                }
                ast::Expr::Name(n) if n.id == self.0 => self.1 = true,
                _ => walk_expr(self, expr),
            }
        }
    }
    let mut read = Read(name, false);
    read.visit_body(body);
    read.1
}

/// The parameter may have different layouts at different calls only if its body
/// cannot observe the representation, including through a nested function.
fn unread_parameter(def: &ast::StmtFunctionDef) -> bool {
    let mut params = def.parameters.iter_non_variadic_params();
    let Some(param) = params.next() else {
        return false;
    };
    if def.is_async
        || params.next().is_some()
        || !def.parameters.kwonlyargs.is_empty()
        || def.parameters.vararg.is_some()
        || def.parameters.kwarg.is_some()
        || param.annotation().is_some()
        || param.default.is_some()
    {
        return false;
    }
    struct Use<'n>(&'n str, bool);
    impl<'a> Visitor<'a> for Use<'_> {
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            match expr {
                ast::Expr::Name(n)
                    if n.id == self.0
                        || matches!(n.id.as_str(), "locals" | "vars" | "eval" | "exec") =>
                {
                    self.1 = true
                }
                ast::Expr::Attribute(a)
                    if matches!(a.attr.as_str(), "_getframe" | "currentframe" | "f_locals") =>
                {
                    self.1 = true
                }
                _ if !self.1 => walk_expr(self, expr),
                _ => (),
            }
        }
    }
    let mut used = Use(param.name().as_str(), false);
    used.visit_body(&def.body);
    !used.1
}

impl Binding {
    /// A direct call reads its captured slots at call time. A generic local
    /// function can acquire its capture list when a later call specializes it.
    pub(super) fn may_capture(&self, slot: usize, name: &str) -> bool {
        self.function
            .as_ref()
            .is_some_and(|f| f.captures.contains(&slot))
            || self
                .generic
                .as_ref()
                .is_some_and(|f| comprehensions::function_reads(f).contains(name))
    }

    pub(super) fn target(&self, source: &str, node: &impl Ranged) -> Result<&Function, String> {
        self.function.as_ref().ok_or_else(|| {
            error(
                source,
                node,
                "forward and recursive local function references are unsupported",
            )
        })
    }
}

impl Function {
    pub(super) fn callback(
        &self,
        types: &Types,
        source: &str,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        if !self.captures.is_empty()
            || self.signature.params.iter().any(|p| {
                matches!(
                    p.kind,
                    ParameterKind::KeywordOnly | ParameterKind::KeywordRest
                )
            })
        {
            return Err(error(
                source,
                node,
                "local callbacks must be noncapturing and have positional parameters",
            ));
        }
        Ok(Expr {
            ty: types.callable(
                self.signature.params.iter().map(|p| p.ty).collect(),
                if self.signature.asynchronous {
                    types.coroutine(self.signature.returns)
                } else {
                    self.signature.returns
                },
            ),
            kind: ExprKind::Callable {
                function: self.signature.id,
                receiver: None,
            },
        })
    }
}

impl Checker<'_> {
    pub(super) fn local_function_binding(&self, value: &ast::Expr) -> Option<&Binding> {
        self.local_functions.get(value.as_name_expr()?.id.as_str())
    }

    pub(super) fn local_cached_value(&self, expression: &ast::Expr) -> Option<Expr> {
        let slot = self.local_function_binding(expression)?.value_slot?;
        self.types
            .cached_function(self.locals[slot])
            .map(|_| self.local(slot))
    }

    pub(super) fn local_callback(
        &self,
        binding: &Binding,
        value: &ast::Expr,
    ) -> Result<Expr, String> {
        if let Some(slot) = binding.value_slot {
            Ok(self.local(slot))
        } else {
            binding
                .target(self.source, value)?
                .callback(self.types, self.source, value)
        }
    }

    /// Only an unconditional definition in the enclosing function's suite can
    /// introduce a fixed call target. Reserve its spelling throughout the suite.
    pub(super) fn collect_local_definitions(&mut self, stmts: &[ast::Stmt]) -> Result<(), String> {
        self.collect_suite_definitions(stmts, true)
    }

    pub(super) fn collect_suite_definitions(
        &mut self,
        stmts: &[ast::Stmt],
        include_classes: bool,
    ) -> Result<(), String> {
        let has_candidate = stmts
            .iter()
            .filter_map(ast::Stmt::as_function_def_stmt)
            .any(cmp_keys::can_specialize);
        let cmp_inputs = if has_candidate {
            cmp_keys::local_comparators(stmts, |callee| {
                self.resolve(callee)
                    == Some(crate::modules::Binding::Intrinsic(Intrinsic::CmpToKey))
            })
        } else {
            HashSet::new()
        };
        let mut suite_functions = HashSet::new();
        for (index, statement) in stmts.iter().enumerate() {
            let name = match statement {
                ast::Stmt::ClassDef(class) if include_classes => class.name.as_str(),
                ast::Stmt::FunctionDef(function) => function.name.as_str(),
                _ => continue,
            };
            if let ast::Stmt::FunctionDef(function) = statement
                && !suite_functions.insert(name)
            {
                let binding = self.local_functions.get_mut(name).unwrap();
                binding.sites.push_back(Site::new(
                    function,
                    &stmts[index + 1..],
                    cmp_inputs.contains(name),
                ));
                continue;
            }
            if reserved(name)
                || self.names.contains_key(name)
                || self.graph.modules[self.module].bindings.contains_key(name)
                || self.class_receiver.as_deref() == Some(name)
                || self.local_functions.contains_key(name)
                || self.local_classes.contains_key(name)
            {
                return Err(error(
                    self.source,
                    statement,
                    if statement.is_class_def_stmt() {
                        "local classes require a unique unshadowed name"
                    } else {
                        "local functions require a unique unshadowed name"
                    },
                ));
            }
            let Some(function) = statement.as_function_def_stmt() else {
                self.local_classes.insert(
                    name.to_owned(),
                    local_classes::Binding {
                        site: statement.range(),
                        value: None,
                    },
                );
                continue;
            };
            self.local_functions.insert(
                name.to_owned(),
                Binding {
                    sites: VecDeque::from([Site::new(
                        function,
                        &stmts[index + 1..],
                        cmp_inputs.contains(name),
                    )]),
                    active_comparator: None,
                    comparators: HashMap::new(),
                    overload_stub: false,
                    function: None,
                    generic: None,
                    value_slot: None,
                    instances: HashMap::new(),
                    later_writes: HashSet::new(),
                },
            );
        }
        Ok(())
    }

    pub(super) fn define_local_function(
        &mut self,
        definition: &ast::StmtFunctionDef,
    ) -> Result<Option<Statement>, String> {
        let name = definition.name.as_str();
        let Some(site) = self
            .local_functions
            .get_mut(name)
            .filter(|binding| {
                binding
                    .sites
                    .front()
                    .is_some_and(|s| s.range == definition.range)
            })
            .and_then(|binding| binding.sites.pop_front())
        else {
            return Err(error(
                self.source,
                definition,
                "local definitions must appear directly in the enclosing function's suite",
            ));
        };
        let overload = matches!(definition.decorator_list.as_slice(),[d] if self.resolve(&d.expression) == Some(crate::modules::Binding::Intrinsic(Intrinsic::Overload)));
        let plain;
        let definition = if overload {
            plain = {
                let mut def = definition.clone();
                typing::strip_overload(&mut def);
                def
            };
            &plain
        } else {
            definition
        };
        {
            let old = self.local_functions.get_mut(name).unwrap();
            old.function = None;
            old.active_comparator = None;
            old.generic = None;
            old.instances.clear();
            old.value_slot = None;
            old.overload_stub = overload;
            old.later_writes = site.later_writes;
        }
        if site.cmp_input {
            if comprehensions::function_reads(definition)
                .iter()
                .any(|n| self.names.contains_key(n) || self.local_functions.contains_key(n))
            {
                return Err(error(
                    self.source,
                    definition,
                    "deferred local comparators must be noncapturing",
                ));
            }
            let offset = u32::from(definition.range.start());
            let binding = self.local_functions.get_mut(name).unwrap();
            binding.active_comparator = Some(offset);
            binding.comparators.insert(
                offset,
                Comparator {
                    definition: Rc::new(definition.clone()),
                    instances: HashMap::new(),
                },
            );
            return Ok(None);
        }
        let unread = unread_parameter(definition);
        if !unread
            && definition
                .parameters
                .iter_non_variadic_params()
                .any(|p| p.annotation().is_none())
        {
            return Err(error(
                self.source,
                definition,
                "local functions require annotated parameters and return types",
            ));
        }
        let wraps = match definition.decorator_list.as_slice() {
            [decorator] => decorator.expression.as_call_expr().filter(|call| {
                self.resolve(&call.func)
                    == Some(crate::modules::Binding::Intrinsic(Intrinsic::Wraps))
            }),
            _ => None,
        };
        if wraps.is_none() && self.generic_awaitable_wrapper(definition) {
            self.local_functions.get_mut(name).unwrap().generic = Some(Rc::new(definition.clone()));
            return Ok(None);
        }
        let policy = self.local_cache_policy(definition)?;
        let decorator = self.local_dispatch_decorator(definition)?;
        let mut raw;
        let signature_def = if policy.is_some() || decorator.is_some() || wraps.is_some() {
            raw = definition.clone();
            raw.decorator_list.clear();
            &raw
        } else {
            definition
        };
        let mut signature = signature(
            signature_def,
            self.graph,
            keyword_forwarding::Defaults::local(self.module),
            &self.classes.declarations,
            self.types,
            None,
            &inference::Inference::default(),
        )?;
        if unread {
            signature.params[0].ty = self.types.unread_argument();
        }
        let function = self.lift_local(
            if wraps.is_some() {
                signature_def
            } else {
                definition
            },
            signature,
        )?;
        let value = if let Some(config) = wraps {
            let (scope, wrapped) = self.wraps_config(config)?;
            let wrapper = function.callback(self.types, self.source, definition)?;
            Some(scope.finish(self.attach_wrapped(wrapper, wrapped, config)?))
        } else if let Some(policy) = policy {
            let raw = function.callback(self.types, self.source, definition)?;
            let origin = self.local_annotation_origin(&function);
            Some(self.owned_cache(raw, policy, Some(origin), definition)?)
        } else if let Some(decorator) = decorator {
            let target = function.callback(self.types, self.source, definition)?;
            Some(self.decorate_dispatch(decorator, target, definition)?)
        } else if site.value_read || overload {
            function
                .callback(self.types, self.source, definition)
                .ok()
                .map(|value| {
                    if overload {
                        Expr {
                            ty: Type::OverloadStub,
                            kind: ExprKind::Builtin(
                                Builtin::Typing(ir::TypingOperation::Register),
                                vec![value],
                            ),
                        }
                    } else {
                        value
                    }
                })
        } else {
            None
        };
        let object = value.map(|value| {
            let slot = self.hidden(value.ty);
            self.assigned.insert(slot);
            self.calls.insert(function.signature.id);
            (slot, value)
        });
        if overload {
            self.graph
                .runtime_overload(self.module, definition.name.range());
        }
        if overload && object.is_none() {
            return Err(error(
                self.source,
                definition,
                "overloaded local functions must be plain noncapturing declarations",
            ));
        }
        let binding = self.local_functions.get_mut(name).unwrap();
        binding.function = Some(function);
        binding.value_slot = object.as_ref().map(|(slot, _)| *slot);
        Ok(object.map(|(slot, value)| Statement::Assign(slot, value)))
    }

    pub(super) fn deferred_comparator(&self, argument: &ast::Expr) -> Option<(usize, u32)> {
        Some((
            self.module,
            self.local_function_binding(argument)?.active_comparator?,
        ))
    }

    /// Compile the original function for the two objects that will be compared.
    pub(super) fn specialize_comparator(
        &mut self,
        site: (usize, u32),
        left: Type,
        right: Type,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let (name, comparator) = self
            .local_functions
            .iter()
            .find_map(|(name, binding)| binding.comparators.get(&site.1).map(|c| (name.clone(), c)))
            .filter(|_| site.0 == self.module)
            .ok_or_else(|| {
                error(
                    self.source,
                    node,
                    "deferred comparator used outside its defining scope",
                )
            })?;
        let key = (left, right);
        if let Some(function) = comparator.instances.get(&key) {
            return function.callback(self.types, self.source, node);
        }
        let definition = Rc::clone(&comparator.definition);
        let params = definition
            .parameters
            .iter_non_variadic_params()
            .zip([left, right])
            .enumerate()
            .map(|(i, (p, ty))| Parameter {
                name: p.name().to_string(),
                ty,
                kind: if i < definition.parameters.posonlyargs.len() {
                    ParameterKind::PositionalOnly
                } else {
                    ParameterKind::PositionalOrKeyword
                },
                default: None,
                sign_only: false,
            })
            .collect();
        let signature = Signature {
            id: 0,
            asynchronous: false,
            method: None,
            returns: Type::None,
            params,
        };
        let function = self.lift_local(&definition, signature)?;
        let value = function.callback(self.types, self.source, node)?;
        self.local_functions
            .get_mut(&name)
            .unwrap()
            .comparators
            .get_mut(&site.1)
            .unwrap()
            .instances
            .insert(key, function);
        Ok(value)
    }

    fn generic_awaitable_wrapper(&self, definition: &ast::StmtFunctionDef) -> bool {
        let [parameter] = definition.parameters.args.as_ref() else {
            return false;
        };
        let Some(result) = &definition.returns else {
            return false;
        };
        let Some(ast::Expr::Subscript(expected)) = parameter.annotation() else {
            return false;
        };
        let Some(crate::modules::Binding::Value(id)) = self.graph.resolve_type(self.module, result)
        else {
            return false;
        };
        definition.is_async
            && definition.parameters.posonlyargs.is_empty()
            && definition.parameters.kwonlyargs.is_empty()
            && definition.parameters.vararg.is_none()
            && definition.parameters.kwarg.is_none()
            && parameter.default.is_none()
            && self.graph.resolve_type(self.module, &expected.value)
                == Some(crate::modules::Binding::Intrinsic(Intrinsic::Awaitable))
            && self.graph.resolve_type(self.module, &expected.slice)
                == Some(crate::modules::Binding::Value(id))
            && type_variable(
                self.graph,
                id.module,
                &self.graph.modules[id.module].parsed.syntax().body[id.statement],
            )
    }

    pub(super) fn call_generic_awaitable(
        &mut self,
        call: &ast::ExprCall,
        definition: &ast::StmtFunctionDef,
    ) -> Result<Expr, String> {
        let [argument] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "awaitable wrapper requires one argument",
            ));
        };
        self.reject_builtin_keywords(call)?;
        let value = self.expr(argument)?;
        let result = self
            .types
            .awaited(value.ty)
            .ok_or_else(|| error(self.source, argument, "expected a typed awaitable"))?;
        let name = definition.name.as_str();
        let function = if let Some(existing) = self.local_functions[name].instances.get(&value.ty) {
            existing.clone()
        } else {
            let signature = Signature {
                id: 0,
                asynchronous: true,
                method: None,
                returns: result,
                params: vec![Parameter {
                    name: definition.parameters.args[0].name().to_string(),
                    ty: value.ty,
                    kind: ParameterKind::PositionalOrKeyword,
                    default: None,
                    sign_only: false,
                }],
            };
            let function = self.lift_local(definition, signature)?;
            self.local_functions
                .get_mut(name)
                .unwrap()
                .instances
                .insert(value.ty, function.clone());
            function
        };
        self.call_local_args(call, &function, ir::CallArgs::positional(vec![value]))
    }

    fn lift_local(
        &mut self,
        definition: &ast::StmtFunctionDef,
        mut signature: Signature,
    ) -> Result<Function, String> {
        signature.id = self.helpers.reserve();
        self.types.bind_function_site(
            self.module,
            definition.name.range().start().to_u32(),
            signature.id,
        );
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
        helper.returns = signature.returns;
        helper.inferred_returns =
            (definition.returns.is_none() && !annotations::inert_body(definition)).then(Vec::new);
        helper.asynchronous = signature.asynchronous;
        let mut params = Vec::new();
        for (parameter, ty) in closed_calls::parameters(&definition.parameters)
            .zip(signature.params.iter().map(|p| p.ty))
        {
            let slot = helper.declare(parameter.name().as_str(), ty, parameter)?;
            helper.assigned.insert(slot);
            params.push(slot);
        }

        let mut reads: Vec<_> = comprehensions::function_reads(definition)
            .into_iter()
            .collect();
        reads.sort();
        let mut captures = Vec::new();
        let mut slots = HashMap::new();
        let mut capture = |outer: usize, helper: &mut Checker<'_>| -> usize {
            *slots.entry(outer).or_insert_with(|| {
                let inner = helper.hidden(self.locals[outer]);
                helper.assigned.insert(inner);
                params.push(inner);
                captures.push(outer);
                inner
            })
        };
        for read in reads {
            if let Some(&outer) = self.names.get(&read) {
                let inner = capture(outer, &mut helper);
                helper.names.insert(read, inner);
            } else if let Some(binding) = self.local_functions.get(&read) {
                if binding.function.is_some() && !binding.sites.is_empty() {
                    return Err(error(
                        self.source,
                        definition,
                        &format!(
                            "captured local function '{read}' may be redefined before the call"
                        ),
                    ));
                }
                let mut binding = binding.clone();
                if let Some(slot) = &mut binding.value_slot {
                    *slot = capture(*slot, &mut helper);
                }
                // An earlier helper can itself close over enclosing slots. Keep
                // their outer identity even if this function shadows their names.
                if let Some(sibling) = &mut binding.function {
                    for slot in &mut sibling.captures {
                        *slot = capture(*slot, &mut helper);
                    }
                }
                helper.local_functions.insert(read, binding);
            }
        }
        // A cold coroutine can start after its caller has returned. Capturing a
        // stable binding preserves cell reads without allocating a heap cell.
        // References to mutable objects still observe changes to their contents.
        if signature.asynchronous
            && let Some(name) = self.local_functions[definition.name.as_str()]
                .later_writes
                .iter()
                .find(|name| {
                    self.names
                        .get(*name)
                        .is_some_and(|slot| captures.contains(slot))
                })
        {
            return Err(error(
                self.source,
                definition,
                &format!("asynchronous capture '{name}' may be rebound before the coroutine runs"),
            ));
        }
        let (mut body, falls) = helper.function_body(&definition.body)?;
        if let Some(mut returns) = helper.inferred_returns.take() {
            if falls || returns.is_empty() {
                returns.push(Type::None);
            }
            let result = returns[0];
            if !returns.iter().all(|&ty| ty == result) {
                return Err(error(
                    self.source,
                    definition,
                    "local functions with different return layouts require an annotated result",
                ));
            }
            signature.returns = result;
        }
        if falls && self.types.payload(signature.returns).is_some() {
            body.push(Statement::Return(helper.coerce(
                Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                },
                signature.returns,
                definition,
            )?));
        } else if falls && signature.returns != Type::None {
            return Err(error(
                self.source,
                definition,
                "not every path returns the declared type",
            ));
        }
        self.helpers.finish(
            signature.id,
            comprehensions::Helper {
                module: self.module,
                function: ir::Function {
                    params,
                    returns: signature.returns,
                    locals: helper.locals,
                    body,
                },
                calls: helper.calls,
                class_reads: helper.class_reads.into_inner(),
                source_frame: true,
                recursion_error: self.graph.modules[self.module]
                    .error(definition, "recursive local function calls are unsupported"),
            },
        );
        Ok(Function {
            signature,
            captures,
            definition: Rc::new(definition.clone()),
        })
    }

    pub(super) fn call_local_function(
        &mut self,
        call: &ast::ExprCall,
        function: &Function,
    ) -> Result<Expr, String> {
        let mut scope = calls::ArgumentScope::default();
        let args = self.held_arguments(call, &function.signature.params, None, &mut scope)?;
        Ok(scope.finish(self.call_local_args(call, function, args)?))
    }

    fn call_local_args(
        &mut self,
        call: &ast::ExprCall,
        function: &Function,
        mut args: ir::CallArgs,
    ) -> Result<Expr, String> {
        for &slot in &function.captures {
            if !self.assigned.contains(&slot) {
                return Err(error(
                    self.source,
                    call,
                    "captured local may be uninitialized at this call",
                ));
            }
            args.order.push(args.values.len());
            args.values.push(self.local(slot));
        }
        self.calls.insert(function.signature.id);
        Ok(self.call_value(&function.signature, function.signature.returns, args))
    }
}

/// A TypeVar declaration is a compile-time binding; uses as runtime values
/// are rejected by the ordinary uninitialized-global check.
pub(super) fn type_variable(graph: &SourceGraph, module: ModuleId, statement: &ast::Stmt) -> bool {
    let ast::Stmt::Assign(assign) = statement else {
        return false;
    };
    let [ast::Expr::Name(target)] = assign.targets.as_slice() else {
        return false;
    };
    let ast::Expr::Call(call) = &*assign.value else {
        return false;
    };
    graph.resolve(module, &call.func)
        == Some(crate::modules::Binding::Intrinsic(Intrinsic::TypeVar))
        && matches!(call.arguments.args.as_ref(), [ast::Expr::StringLiteral(name)] if name.value.to_str() == target.id.as_str())
        && call.arguments.keywords.is_empty()
}
