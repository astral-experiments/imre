//! Discovers concrete signatures without exposing unknown types to executable IR.

use std::cell::{Cell, RefCell};

use ruff_python_ast::visitor::{Visitor, walk_stmt};

use super::*;

/// A direct element insertion in this lexical scope can supply a container layout hint.
/// The ordinary checker still checks its operand and every other use of the list.
pub(super) fn first_insertion<'ast>(
    name: &str,
    method: &str,
    statements: &'ast [ast::Stmt],
) -> Option<&'ast ast::Expr> {
    struct Appends<'name, 'ast> {
        name: &'name str,
        method: &'name str,
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
                && method.attr.as_str() == self.method
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

    let mut appends = Appends {
        name,
        method,
        value: None,
    };
    appends.visit_body(statements);
    appends.value
}

impl Checker<'_> {
    /// In a truthy branch, an immediately repeated native dict lookup is present
    /// until the first possible effect. An exact local list/set receiver and
    /// a literal string key cannot execute Python code before reading the argument.
    pub(super) fn immediate_present_subscript(
        &self,
        test: &ast::Expr,
        body: &[ast::Stmt],
    ) -> Option<ruff_text_size::TextRange> {
        fn literal_lookup(expression: &ast::Expr) -> Option<(&str, &str)> {
            let ast::Expr::Subscript(sub) = expression else {
                return None;
            };
            let ast::Expr::Name(name) = &*sub.value else {
                return None;
            };
            let ast::Expr::StringLiteral(key) = &*sub.slice else {
                return None;
            };
            Some((name.id.as_str(), key.value.to_str()))
        }
        let (name, key) = literal_lookup(test)?;
        let &dict = self.names.get(name)?;
        let (key_type, value_type) = self.types.mapping(self.locals[dict])?;
        if key_type != Type::Str || self.types.payload(value_type).is_none() {
            return None;
        }
        let ast::Stmt::Expr(stmt) = body.first()? else {
            return None;
        };
        let ast::Expr::Call(call) = &*stmt.value else {
            return None;
        };
        let ast::Expr::Attribute(method) = &*call.func else {
            return None;
        };
        let ast::Expr::Name(receiver) = &*method.value else {
            return None;
        };
        let &owner = self.names.get(receiver.id.as_str())?;
        let ty = self.locals[owner];
        let intrinsic = (method.attr.as_str() == "add" && self.types.set_element(ty).is_some())
            || (method.attr.as_str() == "append" && self.types.element(ty).is_some());
        let [argument] = call.arguments.args.as_ref() else {
            return None;
        };
        (intrinsic
            && call.arguments.keywords.is_empty()
            && literal_lookup(argument) == Some((name, key)))
        .then(|| argument.range())
    }

    /// Join direct assignments across an exhaustive conditional before its first
    /// suite fixes a local's storage. This supplies a layout for empty-list arms
    /// and nullable scalar arms while retaining normal definite-assignment checks.
    pub(super) fn branch_layouts(
        &mut self,
        body: &[ast::Stmt],
        clauses: &[ast::ElifElseClause],
    ) -> Result<(), String> {
        if !clauses.last().is_some_and(|c| c.test.is_none())
            || !body.iter().any(|stmt| matches!(stmt, ast::Stmt::Assign(assign)
                if matches!(assign.targets.as_slice(), [ast::Expr::Name(name)] if !self.names.contains_key(name.id.as_str()))))
        {
            return Ok(());
        }
        let mut arms: HashMap<&str, Vec<&ast::Expr>> = HashMap::new();
        for suite in std::iter::once(body).chain(clauses.iter().map(|c| c.body.as_slice())) {
            for stmt in suite {
                if let ast::Stmt::Assign(a) = stmt
                    && let [ast::Expr::Name(n)] = a.targets.as_slice()
                    && !self.names.contains_key(n.id.as_str())
                {
                    arms.entry(n.id.as_str()).or_default().push(&a.value);
                }
            }
        }
        for (name, values) in arms {
            if values.len() != clauses.len() + 1 {
                continue;
            }
            let site = values[0];
            if values.iter().all(|e| matches!(e, ast::Expr::List(_)))
                && values
                    .iter()
                    .any(|e| matches!(e, ast::Expr::List(l) if l.elts.is_empty()))
            {
                let mut elements = values.iter().flat_map(|e| {
                    let ast::Expr::List(list) = e else {
                        unreachable!()
                    };
                    list.elts
                        .iter()
                        .map(|element| self.graph.session.scalar_type(self.module, element))
                });
                if let Some(Some(first)) = elements.next()
                    && elements.all(|ty| ty == Some(first))
                {
                    self.declare(name, self.types.list(first), site)?;
                }
            } else if values
                .iter()
                .any(|e| matches!(e, ast::Expr::NoneLiteral(_)))
            {
                let mut concrete = values
                    .iter()
                    .filter(|e| !matches!(e, ast::Expr::NoneLiteral(_)))
                    .map(|e| self.graph.session.scalar_type(self.module, e));
                if let Some(Some(first)) = concrete.next()
                    && concrete.all(|ty| ty == Some(first))
                {
                    self.declare(name, self.types.optional(first), site)?;
                }
            }
        }
        Ok(())
    }
}

const DEFERRED: &str = "inference is waiting for a function return type";

struct Pending {
    params: Vec<Option<Type>>,
    none_defaults: Vec<usize>,
    returns: Option<Type>,
    infer_return: bool,
}

/// Each missing slot is filled once from a checked call or a fully checked body.
/// Discovery uses the ordinary checker, then discards all of its IR and owners.
/// The final pass checks every body again with complete, immutable signatures.
#[derive(Default)]
pub(super) struct Inference {
    pending: RefCell<Vec<Pending>>,
    revision: Cell<usize>,
    widened: Cell<bool>,
}

impl Inference {
    pub fn register(
        &self,
        params: Vec<Option<Type>>,
        returns: Option<Type>,
        none_defaults: Vec<usize>,
    ) -> usize {
        // Even a fully annotated return can have an unannotated default that a
        // supplied argument must widen from None to its nullable scalar layout.
        if !none_defaults.is_empty() {
            self.widened.set(true);
        }
        let mut pending = self.pending.borrow_mut();
        let id = pending.len();
        pending.push(Pending {
            params,
            none_defaults,
            returns,
            infer_return: returns.is_none(),
        });
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
                if matches!(ty, Type::Optional(_))
                    && let Some(default) = &mut parameter.default
                    && default.ty == Type::None
                {
                    *default = Expr {
                        ty: *ty,
                        kind: ExprKind::Absent(Box::new(default.clone())),
                    };
                }
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

    /// The default None establishes absence, not the payload type of supplied arguments.
    /// An annotation or a non-None default keeps its original exact type contract.
    fn none_default(&self, id: usize, index: usize) -> bool {
        let pending = self.pending.borrow();
        pending[id].none_defaults.contains(&index) && pending[id].params[index] == Some(Type::None)
    }

    fn widen_none_default(&self, id: usize, index: usize, value: Type, types: &Types) -> Type {
        let payload = types.payload(value).unwrap_or(value);
        if !matches!(
            payload,
            Type::Str | Type::Bytes | Type::Bool | Type::Int | Type::Float
        ) {
            return Type::None;
        }
        let expected = types.optional(payload);
        self.widen_parameter(id, index, expected);
        expected
    }

    /// Discover the actual input type of closed keyword calls even when all
    /// written signatures have annotations. Python annotations do not reject None.
    pub(super) fn discover_packed_arguments(&self) {
        self.widened.set(true);
    }

    fn widen_parameter(&self, id: usize, index: usize, ty: Type) {
        let mut pending = self.pending.borrow_mut();
        pending[id].params[index] = Some(ty);
        for function in &mut *pending {
            if function.infer_return {
                function.returns = None;
            }
        }
        self.widened.set(true);
        self.revision.set(self.revision.get() + 1);
    }

    /// A later call from a different subcommand replaces a child layout with
    /// its stable parent prefix. Revisit helpers whose result may pass it on.
    fn widen_namespace(&self, id: usize, index: usize, parent: Type, parsers: &argparse::Parsers) {
        let mut pending = self.pending.borrow_mut();
        pending[id].params[index] = Some(parent);
        for function in &mut *pending {
            if function.infer_return
                && let (Some(Type::Class(child)), Type::Class(parent)) = (function.returns, parent)
                && parsers.child_of(child, parent)
            {
                function.returns = None;
            }
        }
        self.widened.set(true);
        self.revision.set(self.revision.get() + 1);
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
    /// A slot can widen from a command to its common namespace only once.
    pub fn solve(
        &self,
        graph: &SourceGraph,
        definitions: &mut [(ModuleId, ast::StmtFunctionDef, Signature)],
        signatures: &mut HashMap<DefinitionId, Signature>,
        classes: &mut ClassInfo,
        types: &Types,
    ) -> Result<(), String> {
        while self.widened.replace(false) || self.unresolved() {
            for (_, _, signature) in definitions.iter_mut() {
                self.update(signature);
            }
            for signature in signatures.values_mut() {
                self.update(signature);
            }
            for methods in classes.methods.iter_mut().chain(&mut classes.setters) {
                for signature in methods.values_mut() {
                    self.update(signature);
                }
            }
            let before = self.revision.get();
            let class_globals = classes.declarations.has_class_values(graph.entry);
            let helpers =
                Helpers::new(definitions.len() + graph.initializers(class_globals).count());
            let globals = ModuleValues::default();
            let mut available = HashSet::new();
            let mut errors = Vec::new();
            for (module, range) in graph.initializers(class_globals) {
                let mut checker = Checker::new(
                    graph, module, signatures, classes, types, &helpers, &globals,
                );
                checker.inference = Some(self);
                if let Err(diagnostic) = Initialization::lower(checker, &mut available, range) {
                    errors.push(format!(
                        "{}:{diagnostic}",
                        graph.modules[module].path.display()
                    ));
                }
            }
            for (module, function, signature) in definitions.iter() {
                if graph.modules[*module].archive.is_some() {
                    continue; // Native declarations must be fully annotated.
                }
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
                checker.asynchronous = signature.asynchronous;
                checker.method_context(signature.method);
                checker.generator_item = classes.producer_items.get(&signature.id).copied();
                checker.inferred_returns = infer_return.then(Vec::new);
                let result = (|| {
                    for (parameter, signature) in
                        closed_calls::parameters(&function.parameters).zip(&signature.params)
                    {
                        let slot =
                            checker.declare(parameter.name().as_str(), signature.ty, parameter)?;
                        checker.assigned.insert(slot);
                    }
                    if let Some(keyword) = &function.parameters.kwarg {
                        let parameter = signature.params.last().expect("keyword-rest signature");
                        let slot =
                            checker.declare(keyword.name.as_str(), parameter.ty, &**keyword)?;
                        checker.assigned.insert(slot);
                    }
                    checker.collect_local_definitions(&function.body)?;
                    let (_, falls_through) = checker.block(&function.body)?;
                    if let Some(mut returns) = checker.inferred_returns.take() {
                        if falls_through || returns.is_empty() {
                            returns.push(Type::None);
                        }
                        let mut ty = returns[0];
                        for &other in &returns[1..] {
                            ty = types.join(ty, other).or_else(|| classes.parsers.common_namespace(ty, other)).ok_or_else(|| {
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
                if !self.unresolved() {
                    break;
                }
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
        for methods in classes.methods.iter_mut().chain(&mut classes.setters) {
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
        self.bind_source(call, signature, receiver, skip, None)
    }

    pub(super) fn source_call(
        &mut self,
        call: &ast::ExprCall,
        signature: &Signature,
        receiver: Option<Expr>,
        skip: usize,
    ) -> Result<Expr, String> {
        if self.classes.cached.contains_key(&signature.id) && !call.arguments.keywords.is_empty() {
            return Err(error(
                self.source,
                call,
                "cached calls currently require a positional key",
            ));
        }
        let mut scope = calls::ArgumentScope::default();
        let (ty, args) = self.bind_source(
            call,
            signature,
            receiver,
            skip,
            (calls::starred(call)
                || signature.params.iter().any(|p| {
                    matches!(p.kind, ParameterKind::Variadic | ParameterKind::KeywordRest)
                }))
            .then_some(&mut scope),
        )?;
        Ok(scope.finish(self.call_value(signature, ty, args)))
    }

    pub(super) fn bind_source(
        &mut self,
        call: &ast::ExprCall,
        signature: &Signature,
        receiver: Option<Expr>,
        skip: usize,
        scope: Option<&mut calls::ArgumentScope>,
    ) -> Result<(Type, ir::CallArgs), String> {
        // CPython's accelerated stat functions take an unsigned mode_t through
        // the index protocol. A boolean argument must reach their integer body
        // as 0 or 1; ordinary annotated Python functions retain their own types.
        let stat_mode = matches!(self.resolve(&call.func), Some(Binding::Definition(id))
            if self.graph.modules[id.module].is_bundled("stat"));
        let selector_wait = signature.method.is_some_and(|method| {
            let class = &self.classes.declarations.definitions[method.class];
            self.graph.modules[class.definition.module].is_bundled("selectors")
                && class.methods[method.index].name.as_str() == "select"
        });
        let inference = self.inference;
        let args = self.bind_supplied(
            call,
            &signature.params[skip..],
            receiver,
            scope,
            Some((signature.id, skip)),
            |checker, index, argument| {
                let index = index + skip;
                let parameter = &signature.params[index];
                if selector_wait && parameter.name == "timeout" {
                    let node = argument.site();
                    let value = argument.value(checker)?;
                    let value = checker.selector_seconds(value);
                    return checker.coerce(value, parameter.ty, node);
                }
                if stat_mode && parameter.ty == Type::Int {
                    let node = argument.site();
                    let value = argument.value(checker)?;
                    return if value.ty == Type::Bool {
                        Ok(Expr {
                            ty: Type::Int,
                            kind: ExprKind::Builtin(Builtin::Int, vec![value]),
                        })
                    } else {
                        checker.direct_value(value, node, parameter.ty, parameter.sign_only)
                    };
                }
                let Some(inference) = inference else {
                    return argument.checked(checker, parameter.ty, parameter.sign_only);
                };
                let node = argument.site();
                let value = if let Some(ty) = inference.parameter(signature.id, index) {
                    if inference.none_default(signature.id, index) {
                        let value = argument.value(checker)?;
                        let expected = inference.widen_none_default(
                            signature.id,
                            index,
                            value.ty,
                            checker.types,
                        );
                        return checker.coerce(value, expected, node);
                    }
                    // Normalized inline keywords carry the value range; source names have their own.
                    if closed_calls::keyword_field(node)
                        || call.arguments.keywords.iter().any(|kw| {
                            kw.arg.as_ref().is_some_and(|name| {
                                name.range() == node.range() && kw.value.range() == node.range()
                            })
                        })
                    {
                        let supplied = argument.value(checker)?;
                        if supplied.ty == Type::None
                            && ty != Type::None
                            && checker.types.payload(ty).is_none()
                        {
                            let expected = checker.types.optional(ty);
                            inference.widen_parameter(signature.id, index, expected);
                            return checker.coerce(supplied, expected, node);
                        }
                        if checker.types.payload(supplied.ty) == Some(ty) {
                            inference.widen_parameter(signature.id, index, supplied.ty);
                            return Ok(supplied);
                        }
                        return checker.direct_value(supplied, node, ty, parameter.sign_only);
                    }
                    if matches!(ty, Type::Class(class) if checker.classes.parsers.namespace(class))
                    {
                        let value = argument.value(checker)?;
                        let expected = if value.ty == ty {
                            ty
                        } else if let Some(parent) =
                            checker.classes.parsers.common_namespace(ty, value.ty)
                        {
                            if parent != ty {
                                inference.widen_namespace(
                                    signature.id,
                                    index,
                                    parent,
                                    &checker.classes.parsers,
                                );
                            }
                            parent
                        } else {
                            ty
                        };
                        checker.coerce(value, expected, node)?
                    } else {
                        argument.checked(checker, ty, parameter.sign_only)?
                    }
                } else {
                    argument.value(checker)?
                };
                inference.learn_parameter(signature.id, index, value.ty);
                Ok(value)
            },
        )?;
        self.calls.insert(signature.id);
        let returns = match inference {
            Some(inference) => inference.pending.borrow()[signature.id]
                .returns
                .ok_or_else(|| error(self.source, call, DEFERRED))?,
            None => signature.returns,
        };
        Ok((returns, args))
    }
}
