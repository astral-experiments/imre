//! Lift typed local definitions into native functions. A captured binding is
//! loaded at each direct call, after its explicit arguments, as Python would
//! read the enclosing cell on entry. No callable with captures can escape.
use std::rc::Rc;

use super::*;

#[derive(Clone)]
pub(super) struct Function {
    pub signature: Signature,
    pub definition: Rc<ast::StmtFunctionDef>,
    captures: Vec<usize>,
}

#[derive(Clone)]
pub(super) struct Binding {
    site: TextRange,
    function: Option<Function>,
    pub(super) generic: Option<Rc<ast::StmtFunctionDef>>,
    instances: HashMap<Type, Function>,
    later_writes: HashSet<String>,
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

    /// Only an unconditional definition in the enclosing function's suite can
    /// introduce a fixed call target. Reserve its spelling throughout the suite.
    pub(super) fn collect_local_functions(&mut self, stmts: &[ast::Stmt]) -> Result<(), String> {
        for (index, statement) in stmts.iter().enumerate() {
            let Some(function) = statement.as_function_def_stmt() else {
                continue;
            };
            let name = function.name.as_str();
            if reserved(name)
                || self.names.contains_key(name)
                || self.graph.modules[self.module].bindings.contains_key(name)
                || self.class_receiver.as_deref() == Some(name)
                || self.local_functions.contains_key(name)
            {
                return Err(error(
                    self.source,
                    function,
                    "local functions require a unique unshadowed name",
                ));
            }
            self.local_functions.insert(
                name.to_owned(),
                Binding {
                    site: function.range,
                    function: None,
                    generic: None,
                    instances: HashMap::new(),
                    later_writes: if function.is_async {
                        narrowing::assigned_names(&stmts[index + 1..])
                    } else {
                        HashSet::new()
                    },
                },
            );
        }
        Ok(())
    }

    pub(super) fn define_local_function(
        &mut self,
        definition: &ast::StmtFunctionDef,
    ) -> Result<(), String> {
        let name = definition.name.as_str();
        if !self
            .local_functions
            .get(name)
            .is_some_and(|binding| binding.site == definition.range && binding.function.is_none())
        {
            return Err(error(
                self.source,
                definition,
                "local definitions must appear directly in the enclosing function's suite",
            ));
        }
        if definition.returns.is_none()
            || definition
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
        if self.generic_awaitable_wrapper(definition) {
            self.local_functions.get_mut(name).unwrap().generic = Some(Rc::new(definition.clone()));
            return Ok(());
        }
        let signature = signature(
            definition,
            self.graph,
            keyword_forwarding::Defaults::local(self.module),
            &self.classes.declarations,
            self.types,
            None,
            &inference::Inference::default(),
        )?;
        let function = self.lift_local(definition, signature)?;
        self.local_functions.get_mut(name).unwrap().function = Some(function);
        Ok(())
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
        helper.returns = signature.returns;
        helper.asynchronous = signature.asynchronous;
        let mut params = Vec::new();
        for (parameter, ty) in definition
            .parameters
            .iter_non_variadic_params()
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
                let mut binding = binding.clone();
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
