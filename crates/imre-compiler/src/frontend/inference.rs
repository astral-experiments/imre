//! Discovers concrete signatures without exposing unknown types to executable IR.

use std::cell::{Cell, RefCell};

use ruff_python_ast::visitor::{Visitor, walk_stmt};

use super::*;

/// An element inserted into a local container and its direct iteration source, when known.
struct Insertion<'a> {
    value: &'a ast::Expr,
    for_loop: Option<&'a ast::StmtFor>,
}

/// A direct element insertion in this lexical scope can supply a container layout hint.
/// The ordinary checker still checks its operand and every other use of the list.
fn first_insertion<'ast>(
    name: &str,
    method: &str,
    statements: &'ast [ast::Stmt],
) -> Option<Insertion<'ast>> {
    struct Appends<'name, 'ast> {
        name: &'name str,
        method: &'name str,
        insertion: Option<Insertion<'ast>>,
        for_loop: Option<&'ast ast::StmtFor>,
    }

    impl<'ast> Visitor<'ast> for Appends<'_, 'ast> {
        fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
            if self.insertion.is_some()
                || matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_))
            {
                return;
            }
            if let ast::Stmt::For(loop_) = stmt {
                let enclosing = self.for_loop.replace(loop_);
                self.visit_body(&loop_.body);
                self.for_loop = enclosing;
                self.visit_body(&loop_.orelse);
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
                self.insertion = Some(Insertion {
                    value,
                    for_loop: self.for_loop,
                });
            } else {
                walk_stmt(self, stmt);
            }
        }

        fn visit_expr(&mut self, _: &'ast ast::Expr) {}
    }

    let mut appends = Appends {
        name,
        method,
        insertion: None,
        for_loop: None,
    };
    appends.visit_body(statements);
    appends.insertion
}

impl Checker<'_> {
    /// A concrete insertion can determine the layout of a fresh list or set. A set
    /// filled from an exact text iterator can also use its inferred element type.
    pub(super) fn inserted_element(
        &self,
        name: &str,
        set: bool,
        later: &[ast::Stmt],
    ) -> Option<Type> {
        let insertion = first_insertion(name, if set { "add" } else { "append" }, later)?;
        if let ast::Expr::Call(call) = insertion.value
            && let Some(class) = self.class_reference(&call.func)
        {
            return Some(Type::Class(class));
        }
        if let Some(ty) = self.graph.session.scalar_type(self.module, insertion.value) {
            return Some(ty);
        }
        if !set || self.graph.set_layout_hint.get() == crate::modules::SetLayoutHint::Disabled {
            return None;
        }
        let for_loop = insertion.for_loop?;
        let item = for_loop.target.as_name_expr()?;
        let source = for_loop.iter.as_name_expr()?;
        let element = match self.locals[*self.names.get(source.id.as_str())?] {
            Type::Str => Type::Str,
            Type::Bytes | Type::ByteArray => Type::Int,
            _ => return None,
        };
        if for_loop.is_async
            || insertion.value.as_name_expr()?.id != item.id
            || narrowing::assigned_names(&for_loop.body).contains(item.id.as_str())
            || narrowing::assigned_names(later).contains(source.id.as_str())
        {
            return None;
        }
        // Later uses may need heterogeneous storage. A failed native attempt
        // must recover the ordinary diagnostics and object fallback for set().
        self.graph
            .set_layout_hint
            .set(crate::modules::SetLayoutHint::Applied);
        Some(element)
    }

    /// A later direct call can supply the same Callable context as an inline lambda.
    /// Its body and every use of the resulting local are still checked normally.
    pub(super) fn local_lambda_context(&self, local: &str, later: &[ast::Stmt]) -> Option<Type> {
        struct Calls<'a, F> {
            hint: Option<Type>,
            context: &'a F,
        }
        impl<'ast, F: Fn(&ast::ExprCall) -> Option<Type>> Visitor<'ast> for Calls<'_, F> {
            fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
                if self.hint.is_none()
                    && !matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_))
                {
                    walk_stmt(self, stmt);
                }
            }

            fn visit_expr(&mut self, expr: &'ast ast::Expr) {
                match expr {
                    ast::Expr::Call(call) if self.hint.is_none() => {
                        self.hint = (self.context)(call)
                    }
                    ast::Expr::Lambda(_)
                    | ast::Expr::Generator(_)
                    | ast::Expr::ListComp(_)
                    | ast::Expr::SetComp(_)
                    | ast::Expr::DictComp(_) => return,
                    _ => {}
                }
                if self.hint.is_none() {
                    ruff_python_ast::visitor::walk_expr(self, expr);
                }
            }
        }

        let assigned = narrowing::assigned_names(later);
        if assigned.contains(local) {
            return None;
        }
        let context = |call: &ast::ExprCall| {
            let mut root = call.func.as_ref();
            while let ast::Expr::Attribute(attr) = root {
                root = &attr.value;
            }
            let name = root.as_name_expr()?.id.as_str();
            if assigned.contains(name)
                || self.names.contains_key(name)
                || self.local_functions.contains_key(name)
            {
                return None;
            }
            let Binding::Definition(definition) = self.resolve(&call.func)? else {
                return None;
            };
            let params = &self.functions.get(&definition)?.params;
            params.iter().enumerate().find_map(|(index, param)| {
                let arg = param
                    .kind
                    .known_argument(&call.arguments, index, &param.name)?;
                let ty = self.types.payload(param.ty).unwrap_or(param.ty);
                (arg.as_name_expr()?.id == local && self.types.signature(ty).is_some())
                    .then_some(ty)
            })
        };
        let mut calls = Calls {
            hint: None,
            context: &context,
        };
        calls.visit_body(later);
        calls.hint
    }

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

enum SignatureUse {
    Return,
    Callable {
        skip: usize,
        expected: Option<(Vec<Type>, Type)>,
    },
}

/// Return information required the next time discovery visits this signature.
#[derive(Clone, Copy)]
enum ReturnDiscovery {
    Declared,
    Body,
    Waiting(u32),
    Forwarded,
}

struct Pending {
    used_as_value: bool,
    inferred_params: Vec<bool>,
    params: Vec<Option<Type>>,
    none_defaults: Vec<usize>,
    returns: Option<Type>,
    discovery: ReturnDiscovery,
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
            used_as_value: false,
            inferred_params: params.iter().map(Option::is_none).collect(),
            params,
            none_defaults,
            returns,
            discovery: if returns.is_some() {
                ReturnDiscovery::Declared
            } else {
                ReturnDiscovery::Body
            },
        });
        id
    }

    pub(super) fn len(&self) -> usize {
        self.pending.borrow().len()
    }

    /// The registered binder has real Python objects even when fixed callers cannot infer a layout.
    pub(super) fn checked_signature(&self, signature: &Signature) -> Option<Signature> {
        let pending = self.pending.borrow();
        let value = &pending[signature.id];
        if value.returns.is_none() || value.params.iter().any(Option::is_none) {
            return None;
        }
        let mut signature = signature.clone();
        self.update(&mut signature);
        Some(signature)
    }

    pub(super) fn registered_signature(&self, signature: &Signature) -> Option<Signature> {
        let pending = self.pending.borrow();
        let params = &pending[signature.id].params;
        if params.iter().all(Option::is_some) {
            return None;
        }
        let mut signature = signature.clone();
        for (param, inferred) in signature.params.iter_mut().zip(params) {
            param.ty = inferred.unwrap_or(Type::PythonObject);
        }
        Some(signature)
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
            if !matches!(function.discovery, ReturnDiscovery::Declared) {
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
            if !matches!(function.discovery, ReturnDiscovery::Declared)
                && let (Some(Type::Class(child)), Type::Class(parent)) = (function.returns, parent)
                && parsers.child_of(child, parent)
            {
                function.returns = None;
            }
        }
        self.widened.set(true);
        self.revision.set(self.revision.get() + 1);
    }

    /// Revisit forwarded returns after widening or before choosing a terminal diagnostic.
    fn revisit_forwarded_returns(&self) {
        for signature in self.pending.borrow_mut().iter_mut() {
            if matches!(
                signature.discovery,
                ReturnDiscovery::Waiting(_) | ReturnDiscovery::Forwarded
            ) {
                signature.discovery = ReturnDiscovery::Body;
            }
        }
    }

    /// A direct call with fixed parameter types has no other evidence to discover
    /// while its callee's return is unknown. The final checker still visits the body.
    fn forwards_declared_parameters(
        &self,
        function: &ast::StmtFunctionDef,
        signature: &Signature,
        callee: usize,
    ) -> bool {
        let [ast::Stmt::Return(ret)] = function.body.as_slice() else {
            return false;
        };
        let Some(ast::Expr::Call(call)) = ret.value.as_deref() else {
            return false;
        };
        let pending = self.pending.borrow();
        !matches!(pending[signature.id].discovery, ReturnDiscovery::Declared)
            && !pending[signature.id].inferred_params.contains(&true)
            && !pending[callee].inferred_params.contains(&true)
            && pending[callee].returns.is_none()
            && matches!(call.func.as_ref(), ast::Expr::Name(_))
            && call.arguments.keywords.is_empty()
            && call.arguments.args.iter().all(|arg| {
                matches!(arg, ast::Expr::Name(name)
                if signature.params.iter().any(|param| param.name == name.id.as_str()))
            })
    }

    /// Learn evidence from an expected callable, then answer the requested signature query.
    /// An ordinary call can use a known return while a first-class callable needs every parameter.
    fn lookup(
        &self,
        signature: &Signature,
        request: SignatureUse,
        module: ModuleId,
        range: TextRange,
    ) -> Result<Type, &'static str> {
        let needs_parameters = matches!(request, SignatureUse::Callable { .. });
        if let SignatureUse::Callable {
            skip,
            expected: Some((params, returns)),
        } = request
        {
            if params.len() + skip != signature.params.len() {
                return Err(Diagnostic::plain(
                    "function value does not match its Callable parameter count",
                )
                .into());
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
        let mut pending = self.pending.borrow_mut();
        let candidate = &mut pending[signature.id];
        candidate.used_as_value |= needs_parameters;
        candidate
            .returns
            .filter(|_| !needs_parameters || candidate.params.iter().all(Option::is_some))
            .ok_or(Failure::PendingSignature {
                signature: signature.id,
                module,
                range,
            })
    }

    pub(super) fn callable_signature(
        &self,
        signature: &mut Signature,
        skip: usize,
        expected: Option<(Vec<Type>, Type)>,
        checker: &Checker<'_>,
        site: &impl Ranged,
    ) -> Result<(), String> {
        self.lookup(
            signature,
            SignatureUse::Callable { skip, expected },
            checker.module,
            site.range(),
        )
        .map_err(|failure| failure.at(checker.source, site))?;
        self.update(signature);
        Ok(())
    }

    /// Inherited copies of one method have distinct receivers but identical
    /// source parameters. At a fixed point, a copy with no direct calls can use
    /// a parameter type on which the called copies agree. Each body still has
    /// to type-check for its own receiver, including overridden method calls.
    fn infer_inherited_parameters(
        &self,
        definitions: &[(ModuleId, ast::StmtFunctionDef, Signature)],
        classes: &ClassInfo,
    ) -> bool {
        let mut copies = HashMap::new();
        for (_, function, signature) in definitions {
            if let Some(method) = signature.method
                && method.kind(&classes.declarations) == crate::classes::MethodKind::Instance
            {
                let class = &classes.declarations.definitions[method.class];
                let origin = (
                    class.method_origins[method.index],
                    function.name.as_str(),
                    function.range(),
                );
                copies
                    .entry(origin)
                    .or_insert_with(Vec::new)
                    .push(signature.id);
            }
        }
        let mut pending = self.pending.borrow_mut();
        let mut changed = false;
        for ids in copies.values().filter(|ids| ids.len() > 1) {
            for index in 1..pending[ids[0]].params.len() {
                let mut known = ids.iter().filter_map(|&id| pending[id].params[index]);
                let Some(first) = known.next() else { continue };
                if known.any(|ty| ty != first) {
                    continue;
                }
                for &id in ids {
                    if pending[id].params[index].is_none() {
                        pending[id].params[index] = Some(first);
                        changed = true;
                    }
                }
            }
        }
        changed
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
    ) -> std::result::Result<(), Diagnostic> {
        loop {
            let widened = self.widened.replace(false);
            if !widened && !self.unresolved() {
                break;
            }
            if widened {
                self.revisit_forwarded_returns();
            }
            let mut skipped = false;
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
            let mut conflicting_returns = None;
            for (module, range) in graph.initializers(class_globals) {
                let mut checker = Checker::new(
                    graph, module, signatures, classes, types, &helpers, &globals,
                );
                checker.inference = Some(self);
                if let Err(diagnostic) = Initialization::lower(checker, &mut available, range) {
                    errors.push((module, diagnostic));
                }
            }
            for (module, function, signature) in definitions.iter() {
                if graph.modules[*module].archive.is_some() {
                    continue; // Native declarations must be fully annotated.
                }
                let (ready, infer_return, retrying) = {
                    let signatures = self.pending.borrow();
                    let pending = &signatures[signature.id];
                    let skip = match pending.discovery {
                        ReturnDiscovery::Waiting(callee) => {
                            signatures[callee as usize].returns.is_none()
                        }
                        ReturnDiscovery::Forwarded => true,
                        _ => false,
                    };
                    if skip && !self.widened.get() {
                        skipped = true;
                        continue;
                    }
                    (
                        pending.params.iter().all(Option::is_some),
                        pending.returns.is_none(),
                        matches!(pending.discovery, ReturnDiscovery::Waiting(_)),
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
                        checker.bind_parameter(parameter, signature.ty)?;
                    }
                    if let Some(keyword) = &function.parameters.kwarg {
                        let parameter = signature.params.last().expect("keyword-rest signature");
                        checker.bind_parameter(keyword, parameter.ty)?;
                    }
                    checker.collect_local_definitions(&function.body)?;
                    let (_, falls_through) = checker.block(&function.body)?;
                    if let Some(mut returns) = checker.inferred_returns.take() {
                        if falls_through || returns.is_empty() {
                            returns.push(Type::None);
                        }
                        let mut ty = returns[0];
                        for &other in &returns[1..] {
                            let Some(joined) = types
                                .join(ty, other)
                                .or_else(|| classes.parsers.common_namespace(ty, other))
                            else {
                                conflicting_returns.get_or_insert((*module, function.range()));
                                return Ok(());
                            };
                            ty = joined;
                        }
                        self.pending.borrow_mut()[signature.id].returns = Some(ty);
                        self.revision.set(self.revision.get() + 1);
                    }
                    Ok::<_, Failure>(())
                })();
                if let Err(diagnostic) = result {
                    if let Failure::PendingSignature {
                        signature: callee, ..
                    } = &diagnostic
                        && self.forwards_declared_parameters(function, &signature, *callee)
                        && let Ok(callee) = u32::try_from(*callee)
                    {
                        self.pending.borrow_mut()[signature.id].discovery =
                            ReturnDiscovery::Waiting(callee);
                    }
                    errors.push((*module, diagnostic));
                } else if retrying {
                    self.pending.borrow_mut()[signature.id].discovery = ReturnDiscovery::Forwarded;
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
                errors.push((graph.entry, diagnostic));
            }
            if self.revision.get() == before {
                // Reproduce the canonical diagnostic pass if discovery stalls.
                if skipped && self.unresolved() {
                    self.revisit_forwarded_returns();
                    continue;
                }
                let mut reports =
                    errors
                        .into_iter()
                        .filter_map(|(module, failure)| match failure {
                            Failure::Diagnostic(report) => Some(report.in_module(graph, module)),
                            Failure::PendingSignature { .. } => None,
                        });
                if let Some((module, range)) = conflicting_returns {
                    let mut report = Diagnostic::at(
                        &graph.modules[module].source,
                        &range,
                        "inferred returns require one supported exact or nullable type".to_owned(),
                    );
                    report.kind = Kind::ReturnType;
                    return Err(report.in_module(graph, module).with_following(reports));
                }
                if !self.unresolved() {
                    break;
                }
                // Only the failure that escaped the checker controls this round.
                // An alternative may have waited for a signature and been discarded.
                if let Some(primary) = reports.next() {
                    if primary.kind.is_object() {
                        registered::capture(
                            graph, signatures, classes, types, self, &globals, false,
                        );
                    }
                    return Err(primary.with_following(reports));
                }
                if self.infer_inherited_parameters(definitions, classes) {
                    continue;
                }
                let pending = self.pending.borrow();
                for (module, function, signature) in definitions.iter() {
                    let inferred = &pending[signature.id];
                    // A first-class function can bind Python values when its callers could
                    // not establish a scalar signature. Its definition is already known.
                    let unresolved = if inferred.used_as_value
                        && (inferred.returns.is_none()
                            || inferred.params.iter().any(Option::is_none))
                    {
                        Diagnostic::at(
                            &graph.modules[*module].source,
                            function,
                            "using this function as a value requires Python argument binding"
                                .to_owned(),
                        )
                        .classified(Kind::ScopedObject)
                    } else if let Some(index) = inferred.params.iter().position(Option::is_none) {
                        let name = &signature.params[index].name;
                        Diagnostic::at(
                            &graph.modules[*module].source,
                            function,
                            format!("cannot infer parameter {name:?}; add an annotation or a concrete direct call"),
                        ).classified(Kind::UnresolvedParameter(*module, function.start()))
                    } else {
                        if inferred.returns.is_none() {
                            return Err(Diagnostic::at(&graph.modules[*module].source,
                                function,
                                "cannot infer function return type; add an annotation; recursive calls remain unsupported".to_owned(),
                            ).in_module(graph, *module));
                        }
                        continue;
                    };
                    registered::capture(graph, signatures, classes, types, self, &globals, true);
                    return Err(unresolved.in_module(graph, *module));
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
                || calls::named_mapping(call, &signature.params[skip..]).is_some()
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
            &mut |checker, index, argument| {
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
                    // Keep annotations fixed. For inferred arguments, select the tagged
                    // spelling or string-list layout from both sides of the call.
                    let types = checker.types;
                    if inference.pending.borrow()[signature.id].inferred_params[index]
                        && (matches!(ty, Type::Str | Type::Bytes)
                            || types.element(ty) == Some(Type::Str))
                    {
                        let lines = types.list(Type::Str);
                        let wider = |other| {
                            if ty == other {
                                return None;
                            }
                            types.union(&[ty, other]).filter(|&union| {
                                types.members(union).is_some_and(|members| {
                                    members == [Type::Str, Type::Bytes]
                                        || members == [Type::Str, lines]
                                })
                            })
                        };
                        let value = match argument {
                            calls::Supplied::Source(node) => {
                                checker.expr_expected_if(node, ty, |_, actual| {
                                    wider(actual).is_some()
                                })?
                            }
                            calls::Supplied::Field(value, _) => value,
                        };
                        let expected = if let Some(union) = wider(value.ty) {
                            inference.widen_parameter(signature.id, index, union);
                            union
                        } else {
                            ty
                        };
                        return checker.direct_value(value, node, expected, parameter.sign_only);
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
            Some(inference) => inference
                .lookup(signature, SignatureUse::Return, self.module, call.range())
                .map_err(|failure| failure.at(self.source, call))?,
            None => signature.returns,
        };
        Ok((returns, args))
    }
}
