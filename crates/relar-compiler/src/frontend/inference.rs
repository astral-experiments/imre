//! Discovers concrete signatures without exposing unknown types to executable IR.

use std::cell::{Cell, RefCell};

use ruff_python_ast::visitor::{Visitor, walk_stmt};

use super::*;

/// Only a standalone append in this lexical scope supplies a list layout hint.
/// The ordinary checker still checks its operand and every other use of the list.
pub(super) fn first_append<'ast>(
    name: &str,
    statements: &'ast [ast::Stmt],
) -> Option<&'ast ast::Expr> {
    struct Appends<'name, 'ast> {
        name: &'name str,
        value: Option<&'ast ast::Expr>,
    }

    impl<'ast> Visitor<'ast> for Appends<'_, 'ast> {
        fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
            if self.value.is_some()
                || matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_))
            {
                return;
            }
            if let ast::Stmt::Expr(stmt) = stmt
                && let ast::Expr::Call(call) = &*stmt.value
                && let ast::Expr::Attribute(method) = &*call.func
                && method.attr.as_str() == "append"
                && matches!(&*method.value, ast::Expr::Name(name) if name.id == self.name)
                && let [value] = call.arguments.args.as_ref()
                && call.arguments.keywords.is_empty()
            {
                self.value = Some(value);
            } else {
                walk_stmt(self, stmt);
            }
        }

        fn visit_expr(&mut self, _: &'ast ast::Expr) {}
    }

    let mut appends = Appends { name, value: None };
    appends.visit_body(statements);
    appends.value
}

const DEFERRED: &str = "inference is waiting for a function return type";

struct Pending {
    params: Vec<Option<Type>>,
    returns: Option<Type>,
}

/// Each missing slot is filled once from a checked call or a fully checked body.
/// Discovery uses the ordinary checker, then discards all of its IR and owners.
/// The final pass checks every body again with complete, immutable signatures.
#[derive(Default)]
pub(super) struct Inference {
    pending: RefCell<Vec<Pending>>,
    revision: Cell<usize>,
}

impl Inference {
    pub fn register(&self, params: Vec<Option<Type>>, returns: Option<Type>) -> usize {
        let mut pending = self.pending.borrow_mut();
        let id = pending.len();
        pending.push(Pending { params, returns });
        id
    }

    fn unresolved(&self) -> bool {
        self.pending
            .borrow()
            .iter()
            .any(|value| value.returns.is_none() || value.params.iter().any(Option::is_none))
    }

    fn update(&self, signature: &mut Signature) {
        let pending = self.pending.borrow();
        let inferred = &pending[signature.id];
        for (parameter, ty) in signature.params.iter_mut().zip(&inferred.params) {
            if let Some(ty) = ty {
                parameter.ty = *ty;
            }
        }
        if let Some(returns) = inferred.returns {
            signature.returns = returns;
        }
    }

    pub(super) fn parameter(&self, id: usize, index: usize) -> Option<Type> {
        self.pending.borrow()[id].params[index]
    }

    pub(super) fn learn_parameter(&self, id: usize, index: usize, ty: Type) {
        let mut pending = self.pending.borrow_mut();
        let slot = &mut pending[id].params[index];
        if slot.is_none() {
            *slot = Some(ty);
            self.revision.set(self.revision.get() + 1);
        }
    }

    /// An exact callback boundary can supply missing formal types. The ordinary
    /// final body check still enforces every inferred parameter and return.
    pub(super) fn callable_signature(
        &self,
        signature: &mut Signature,
        skip: usize,
        expected: Option<(Vec<Type>, Type)>,
    ) -> Result<(), &'static str> {
        if let Some((params, returns)) = expected {
            if params.len() + skip != signature.params.len() {
                return Err("function value does not match its Callable parameter count");
            }
            for (index, ty) in params.into_iter().enumerate() {
                self.learn_parameter(signature.id, index + skip, ty);
            }
            let mut pending = self.pending.borrow_mut();
            if pending[signature.id].returns.is_none() {
                pending[signature.id].returns = Some(returns);
                self.revision.set(self.revision.get() + 1);
            }
        }
        let ready = {
            let pending = self.pending.borrow();
            pending[signature.id].returns.is_some()
                && pending[signature.id].params.iter().all(Option::is_some)
        };
        if !ready {
            return Err(DEFERRED);
        }
        self.update(signature);
        Ok(())
    }

    /// Stops when every signature is concrete, or no new type evidence is available.
    /// A round can only fill missing slots, so a finite call graph terminates.
    pub fn solve(
        &self,
        graph: &SourceGraph,
        definitions: &mut [(ModuleId, ast::StmtFunctionDef, Signature)],
        signatures: &mut HashMap<DefinitionId, Signature>,
        classes: &mut ClassInfo,
        types: &Types,
    ) -> Result<(), String> {
        while self.unresolved() {
            for (_, _, signature) in definitions.iter_mut() {
                self.update(signature);
            }
            for signature in signatures.values_mut() {
                self.update(signature);
            }
            for methods in &mut classes.methods {
                for signature in methods.values_mut() {
                    self.update(signature);
                }
            }
            let before = self.revision.get();
            let helpers = Helpers::new(definitions.len() + graph.initializer_modules().count());
            let globals = ModuleValues::default();
            let mut available = HashSet::new();
            let mut errors = Vec::new();
            for module in graph.initializer_modules() {
                let mut checker = Checker::new(
                    graph, module, signatures, classes, types, &helpers, &globals,
                );
                checker.inference = Some(self);
                if let Err(diagnostic) = Initialization::lower(checker, &mut available) {
                    errors.push(format!(
                        "{}:{diagnostic}",
                        graph.modules[module].path.display()
                    ));
                }
            }
            for (module, function, signature) in definitions.iter() {
                let (ready, infer_return) = {
                    let pending = self.pending.borrow();
                    let pending = &pending[signature.id];
                    (
                        pending.params.iter().all(Option::is_some),
                        pending.returns.is_none(),
                    )
                };
                if !ready {
                    continue;
                }
                // Calls earlier in this round can have supplied new parameters.
                let mut signature = signature.clone();
                self.update(&mut signature);
                let mut checker = Checker::new(
                    graph, *module, signatures, classes, types, &helpers, &globals,
                );
                checker.inference = Some(self);
                checker.returns = signature.returns;
                checker.method_context(signature.method);
                checker.inferred_returns = infer_return.then(Vec::new);
                let result = (|| {
                    for (parameter, signature) in function
                        .parameters
                        .iter_non_variadic_params()
                        .zip(&signature.params)
                    {
                        let slot =
                            checker.declare(parameter.name().as_str(), signature.ty, parameter)?;
                        checker.assigned.insert(slot);
                    }
                    checker.collect_local_functions(&function.body)?;
                    let (_, falls_through) = checker.block(&function.body)?;
                    if let Some(mut returns) = checker.inferred_returns.take() {
                        if falls_through || returns.is_empty() {
                            returns.push(Type::None);
                        }
                        let mut ty = returns[0];
                        for &other in &returns[1..] {
                            ty = types.join(ty, other).ok_or_else(|| {
                                error(
                                    checker.source,
                                    function,
                                    "inferred returns require one supported exact or nullable type",
                                )
                            })?;
                        }
                        self.pending.borrow_mut()[signature.id].returns = Some(ty);
                        self.revision.set(self.revision.get() + 1);
                    }
                    Ok::<_, String>(())
                })();
                if let Err(diagnostic) = result {
                    errors.push(format!(
                        "{}:{diagnostic}",
                        graph.modules[*module].path.display()
                    ));
                }
            }
            let module = &graph.modules[graph.entry];
            let mut checker = Checker::new(
                graph,
                graph.entry,
                signatures,
                classes,
                types,
                &helpers,
                &globals,
            );
            checker.inference = Some(self);
            checker.module_scope = true;
            let statements = module
                .executable
                .iter()
                .map(|&index| module.parsed.syntax().body[index].clone())
                .collect::<Vec<_>>();
            if let Err(diagnostic) = checker.block(&statements) {
                errors.push(format!("{}:{diagnostic}", module.path.display()));
            }
            if self.revision.get() == before {
                if let Some(diagnostic) = errors.into_iter().find(|error| !error.contains(DEFERRED))
                {
                    return Err(diagnostic);
                }
                let pending = self.pending.borrow();
                for (module, function, signature) in definitions.iter() {
                    let inferred = &pending[signature.id];
                    if let Some(index) = inferred.params.iter().position(Option::is_none) {
                        let name = &signature.params[index].name;
                        return Err(graph.modules[*module].error(
                            function,
                            &format!(
                                "cannot infer parameter {name:?}; add an annotation or a concrete direct call"
                            ),
                        ));
                    }
                    if inferred.returns.is_none() {
                        return Err(graph.modules[*module].error(
                            function,
                            "cannot infer function return type; add an annotation; recursive calls remain unsupported",
                        ));
                    }
                }
                unreachable!("an unresolved signature has a missing parameter or return");
            }
        }
        for (_, _, signature) in definitions.iter_mut() {
            self.update(signature);
        }
        for signature in signatures.values_mut() {
            self.update(signature);
        }
        for methods in &mut classes.methods {
            for signature in methods.values_mut() {
                self.update(signature);
            }
        }
        Ok(())
    }
}

impl Checker<'_> {
    /// Binds a speculative call in source order while recording missing formal types.
    /// No value from this pass reaches LLVM; final calls use arguments() normally.
    pub(super) fn infer_call(
        &mut self,
        call: &ast::ExprCall,
        signature: &Signature,
        receiver: Option<Expr>,
        skip: usize,
    ) -> Result<(Type, ir::CallArgs), String> {
        let inference = self.inference.expect("discovery call has inference state");
        let args = self.arguments_with(
            call,
            &signature.params[skip..],
            receiver,
            |checker, index, argument| {
                let index = index + skip;
                let value = if let Some(ty) = inference.parameter(signature.id, index) {
                    checker.direct_argument(argument, ty, signature.params[index].sign_only)?
                } else {
                    checker.expr(argument)?
                };
                inference.learn_parameter(signature.id, index, value.ty);
                Ok(value)
            },
        )?;
        self.calls.insert(signature.id);
        let returns = inference.pending.borrow()[signature.id]
            .returns
            .ok_or_else(|| error(self.source, call, DEFERRED))?;
        Ok((returns, args))
    }
}
