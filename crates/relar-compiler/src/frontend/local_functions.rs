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
}

impl Binding {
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
            || self
                .signature
                .params
                .iter()
                .any(|p| p.kind == ParameterKind::KeywordOnly)
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
                self.signature.returns,
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
        for function in stmts.iter().filter_map(ast::Stmt::as_function_def_stmt) {
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
        let mut signature = signature(
            definition,
            self.graph,
            self.module,
            &self.classes.declarations,
            self.types,
            None,
            &inference::Inference::default(),
        )?;
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
                recursion_error: self.graph.modules[self.module]
                    .error(definition, "recursive local function calls are unsupported"),
            },
        );
        self.local_functions.get_mut(name).unwrap().function = Some(Function {
            signature,
            captures,
            definition: Rc::new(definition.clone()),
        });
        Ok(())
    }

    pub(super) fn call_local_function(
        &mut self,
        call: &ast::ExprCall,
        function: &Function,
    ) -> Result<Expr, String> {
        let mut args = self.arguments(call, &function.signature.params, None)?;
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
        Ok(Expr {
            ty: function.signature.returns,
            kind: ExprKind::Call(function.signature.id, args),
        })
    }
}
