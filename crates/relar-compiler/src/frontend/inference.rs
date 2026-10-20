//! Discovers concrete signatures without exposing unknown types to executable IR.

use std::cell::{Cell, RefCell};

use super::*;

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

    fn parameter(&self, id: usize, index: usize) -> Option<Type> {
        self.pending.borrow()[id].params[index]
    }

    fn learn_parameter(&self, id: usize, index: usize, ty: Type) {
        let mut pending = self.pending.borrow_mut();
        let slot = &mut pending[id].params[index];
        if slot.is_none() {
            *slot = Some(ty);
            self.revision.set(self.revision.get() + 1);
        }
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
            let helpers = Helpers::new(definitions.len() + graph.modules.len() - 1);
            let globals = ModuleValues::default();
            let mut available = HashSet::new();
            let mut errors = Vec::new();
            for &module in &graph.initialization {
                if module == graph.entry {
                    continue;
                }
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
        let params = &signature.params[skip..];
        let offset = usize::from(receiver.is_some());
        let mut values = receiver.into_iter().collect::<Vec<_>>();
        let mut order = vec![usize::MAX; params.len() + offset];
        if offset != 0 {
            order[0] = 0;
        }
        for (index, argument) in call.arguments.args.iter().enumerate() {
            params
                .get(index)
                .filter(|parameter| parameter.kind != ParameterKind::KeywordOnly)
                .ok_or_else(|| error(self.source, argument, "too many positional arguments"))?;
            let value = if let Some(ty) = inference.parameter(signature.id, index + skip) {
                self.expr_expected(argument, ty)?
            } else {
                self.expr(argument)?
            };
            inference.learn_parameter(signature.id, index + skip, value.ty);
            order[index + offset] = values.len();
            values.push(value);
        }
        for keyword in &call.arguments.keywords {
            let name = keyword
                .arg
                .as_ref()
                .expect("call lowering rejects argument unpacking")
                .as_str();
            let index = params
                .iter()
                .position(|parameter| parameter.name == name)
                .ok_or_else(|| {
                    error(
                        self.source,
                        keyword,
                        &format!("unexpected keyword argument {name:?}"),
                    )
                })?;
            if params[index].kind == ParameterKind::PositionalOnly {
                return Err(error(
                    self.source,
                    keyword,
                    &format!("parameter {name:?} is positional-only"),
                ));
            }
            if order[index + offset] != usize::MAX {
                return Err(error(
                    self.source,
                    keyword,
                    &format!("multiple values for argument {name:?}"),
                ));
            }
            let value = if let Some(ty) = inference.parameter(signature.id, index + skip) {
                self.expr_expected(&keyword.value, ty)?
            } else {
                self.expr(&keyword.value)?
            };
            inference.learn_parameter(signature.id, index + skip, value.ty);
            order[index + offset] = values.len();
            values.push(value);
        }
        for (index, parameter) in params.iter().enumerate() {
            if order[index + offset] == usize::MAX {
                let default = parameter.default.as_ref().ok_or_else(|| {
                    error(
                        self.source,
                        call,
                        &format!("missing required argument {:?}", parameter.name),
                    )
                })?;
                order[index + offset] = values.len();
                values.push(default.clone());
            }
        }
        self.calls.insert(signature.id);
        let returns = inference.pending.borrow()[signature.id]
            .returns
            .ok_or_else(|| error(self.source, call, DEFERRED))?;
        Ok((returns, ir::CallArgs { values, order }))
    }
}
