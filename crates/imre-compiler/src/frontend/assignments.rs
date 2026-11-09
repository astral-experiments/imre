//! Assignment expressions share the enclosing typed local scope. Flow snapshots
//! keep conditional writes from becoming unconditional initialization facts.

use super::*;
use ruff_python_ast::visitor::transformer::{self, Transformer};
use ruff_python_ast::visitor::{Visitor, walk_comprehension, walk_expr};

/// Previously allocated slots keep their facts throughout an expression unless
/// the expression can write the name or use it in a narrowing test. Names
/// introduced by a walrus may acquire a slot while checking the first arm.
#[derive(Clone)]
pub(super) struct ConditionalFacts<'a> {
    writes: Vec<(String, bool)>,
    narrowed: Vec<(&'a str, Option<Type>)>,
}

impl<'a> ConditionalFacts<'a> {
    pub(super) fn capture(checker: &Checker<'_>, value: &'a ast::ExprIf) -> Self {
        Self::for_expressions(
            checker,
            [&*value.test, &*value.body, &*value.orelse],
            [&*value.test],
        )
    }

    fn for_expressions(
        checker: &Checker<'_>,
        expressions: impl IntoIterator<Item = &'a ast::Expr>,
        tests: impl IntoIterator<Item = &'a ast::Expr>,
    ) -> Self {
        struct Names<'a>(HashSet<&'a str>);
        impl<'a> Names<'a> {
            /// Names that apply_facts can narrow when testing this expression.
            fn predicate(&mut self, test: &'a ast::Expr) {
                match test {
                    ast::Expr::Name(name) => {
                        self.0.insert(name.id.as_str());
                    }
                    ast::Expr::Named(node) => self.visit_expr(&node.target),
                    ast::Expr::UnaryOp(node) => self.predicate(&node.operand),
                    ast::Expr::BoolOp(node) => {
                        for value in &node.values {
                            self.predicate(value);
                        }
                    }
                    ast::Expr::Call(node) if matches!(&*node.func, ast::Expr::Name(name) if name.id == "bool" || name.id == "isinstance") => {
                        if let Some(first) = node.arguments.args.first() {
                            self.predicate(first);
                        }
                    }
                    ast::Expr::Compare(node) => {
                        for value in &node.operands {
                            if matches!(value, ast::Expr::Name(_) | ast::Expr::Named(_)) {
                                self.predicate(value);
                            } else if let ast::Expr::Call(call) = value
                                && matches!(&*call.func, ast::Expr::Name(name) if name.id == "type")
                                && let [ast::Expr::Name(name)] = call.arguments.args.as_ref()
                            {
                                // An exact type comparison can narrow the argument's slot.
                                self.0.insert(name.id.as_str());
                            } else if let ast::Expr::Attribute(field) = value
                                && let ast::Expr::Name(receiver) = &*field.value
                            {
                                // A subcommand comparison narrows its namespace receiver.
                                self.0.insert(receiver.id.as_str());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        impl<'a> Visitor<'a> for Names<'a> {
            fn visit_expr(&mut self, expr: &'a ast::Expr) {
                if let ast::Expr::Name(name) = expr {
                    self.0.insert(name.id.as_str());
                } else {
                    walk_expr(self, expr);
                }
            }
        }
        // Only truth tests, bindings, and comprehension targets can change
        // narrowing inside an expression. Plain reads in wide displays do not.
        struct FlowNames<'a> {
            names: Names<'a>,
        }
        impl<'a> Visitor<'a> for FlowNames<'a> {
            fn visit_expr(&mut self, expr: &'a ast::Expr) {
                match expr {
                    ast::Expr::If(node) => self.names.predicate(&node.test),
                    ast::Expr::Named(node) => self.names.visit_expr(&node.target),
                    ast::Expr::BoolOp(_) => self.names.predicate(expr),
                    _ => {}
                }
                walk_expr(self, expr);
            }
            fn visit_comprehension(&mut self, node: &'a ast::Comprehension) {
                self.names.visit_expr(&node.target);
                for test in &node.ifs {
                    self.names.predicate(test);
                }
                walk_comprehension(self, node);
            }
        }
        let mut flow = FlowNames {
            names: Names(HashSet::new()),
        };
        for test in tests {
            flow.names.predicate(test);
        }
        let mut writes = HashSet::new();
        for expr in expressions {
            flow.visit_expr(expr);
            writes.extend(narrowing::expression_assignments(expr));
        }
        Self {
            writes: writes
                .into_iter()
                .map(|name| {
                    let assigned = checker
                        .names
                        .get(&name)
                        .is_some_and(|s| checker.assigned.contains(s));
                    (name, assigned)
                })
                .collect(),
            narrowed: flow
                .names
                .0
                .into_iter()
                .map(|name| {
                    let ty = checker
                        .names
                        .get(name)
                        .and_then(|s| checker.narrowed.get(s))
                        .copied();
                    (name, ty)
                })
                .collect(),
        }
    }

    fn read(&mut self, checker: &Checker<'_>) {
        for (name, assigned) in &mut self.writes {
            *assigned = checker
                .names
                .get(name)
                .is_some_and(|slot| checker.assigned.contains(slot));
        }
        for (name, ty) in &mut self.narrowed {
            *ty = checker
                .names
                .get(*name)
                .and_then(|slot| checker.narrowed.get(slot))
                .copied();
        }
    }

    fn retain_common(&mut self, checker: &Checker<'_>) {
        for (name, assigned) in &mut self.writes {
            *assigned &= checker
                .names
                .get(name)
                .is_some_and(|slot| checker.assigned.contains(slot));
        }
        for (name, ty) in &mut self.narrowed {
            if checker
                .names
                .get(*name)
                .and_then(|slot| checker.narrowed.get(slot))
                .copied()
                != *ty
            {
                *ty = None;
            }
        }
    }

    /// Restore the input facts and keep the completed arm's facts for the join.
    pub(super) fn switch_arm(&mut self, checker: &mut Checker<'_>) {
        for (name, assigned) in &mut self.writes {
            if let Some(&slot) = checker.names.get(name) {
                let current = checker.assigned.contains(&slot);
                if *assigned {
                    checker.assigned.insert(slot);
                } else {
                    checker.assigned.remove(&slot);
                }
                *assigned = current;
            }
        }
        for (name, ty) in &mut self.narrowed {
            if let Some(&slot) = checker.names.get(*name) {
                let incoming = checker.narrowed.get(&slot).copied();
                match *ty {
                    Some(previous) => checker.narrowed.insert_mut(slot, previous),
                    None => {
                        checker.narrowed.remove_mut(&slot);
                    }
                }
                *ty = incoming;
            }
        }
    }

    pub(super) fn join(self, checker: &mut Checker<'_>) {
        for (name, assigned) in self.writes {
            if !assigned && let Some(slot) = checker.names.get(&name) {
                checker.assigned.remove(slot);
            }
        }
        for (name, ty) in self.narrowed {
            if let Some(slot) = checker.names.get(name)
                && checker.narrowed.get(slot).copied() != ty
            {
                checker.narrowed.remove_mut(slot);
            }
        }
    }
}

/// Each boolean operator joins the exits that skip the remaining operands.
/// Nested operators retain only the names their operands can affect.
pub(super) struct BooleanExits<'a> {
    after: ConditionalFacts<'a>,
    exit: Option<ConditionalFacts<'a>>,
}

impl<'a> BooleanExits<'a> {
    pub(super) fn new(checker: &Checker<'_>, boolean: &'a ast::ExprBoolOp) -> Self {
        Self {
            after: ConditionalFacts::for_expressions(checker, &boolean.values, &boolean.values),
            exit: None,
        }
    }

    pub(super) fn short_circuit(
        &mut self,
        checker: &mut Checker<'_>,
        operand: &ast::Expr,
        truth: bool,
    ) {
        self.after.read(checker);
        checker.apply_facts(operand, truth);
        if let Some(exit) = &mut self.exit {
            exit.retain_common(checker);
        } else {
            let mut first = self.after.clone();
            first.read(checker);
            self.exit = Some(first);
        }
        self.after.switch_arm(checker);
    }

    pub(super) fn finish(self, checker: &mut Checker<'_>, reachable: bool) {
        if let Some(mut exit) = self.exit {
            if reachable {
                exit.join(checker);
            } else {
                exit.switch_arm(checker);
            }
        }
    }
}

// Snapshots share unchanged trie nodes with the current flow. Entering a
// nested suite only copies the paths for slots whose facts actually change.
#[derive(Clone, Default)]
pub(super) struct Assigned(rpds::HashTrieMap<usize, u64>);

impl Assigned {
    pub(super) fn contains(&self, slot: &usize) -> bool {
        let bits = self.0.get(&(slot / 64)).copied().unwrap_or(0);
        bits & (1 << (slot % 64)) != 0
    }

    pub(super) fn insert(&mut self, slot: usize) {
        let index = slot / 64;
        let mask = 1 << (slot % 64);
        let bits = self.0.get(&index).copied().unwrap_or(0);
        if bits & mask == 0 {
            self.0.insert_mut(index, bits | mask);
        }
    }

    pub(super) fn remove(&mut self, slot: &usize) {
        let index = slot / 64;
        if let Some(&bits) = self.0.get(&index) {
            let remaining = bits & !(1 << (slot % 64));
            if remaining != bits {
                self.set_word(index, remaining);
            }
        }
    }

    fn set_word(&mut self, index: usize, bits: u64) {
        if bits == 0 {
            self.0.remove_mut(&index);
        } else {
            self.0.insert_mut(index, bits);
        }
    }

    fn intersect(&mut self, other: &Self) {
        if !self.0.ptr_eq(&other.0) {
            for (&index, &bits) in &self.0.clone() {
                let common = bits & other.0.get(&index).copied().unwrap_or(0);
                if common != bits {
                    self.set_word(index, common);
                }
            }
        }
    }

    pub(super) fn extend(&mut self, other: &Self) {
        for (&index, &bits) in &other.0 {
            let old = self.0.get(&index).copied().unwrap_or(0);
            if old | bits != old {
                self.0.insert_mut(index, old | bits);
            }
        }
    }
}

pub(super) type Narrowed = rpds::HashTrieMap<usize, Type>;

pub(super) fn intersect(
    assigned: &mut Assigned,
    narrowed: &mut Narrowed,
    other_assigned: &Assigned,
    other_narrowed: &Narrowed,
) {
    assigned.intersect(other_assigned);
    if !narrowed.ptr_eq(other_narrowed) {
        for (slot, ty) in &narrowed.clone() {
            if other_narrowed.get(slot) != Some(ty) {
                narrowed.remove_mut(slot);
            }
        }
    }
}

/// Keep a parameter's public keyword while giving its lexical references an
/// internal spelling. Builtin fast paths can then match names without ever
/// interpreting a local `bytes`, `map`, or `len` as the builtin.
pub(super) fn alias_builtin_parameters(
    function: &mut ast::StmtFunctionDef,
    graph: &SourceGraph,
    module: ModuleId,
) {
    let params = &mut function.parameters;
    for parameter in params
        .posonlyargs
        .iter_mut()
        .chain(&mut params.args)
        .chain(&mut params.kwonlyargs)
    {
        let name = &mut parameter.parameter.name.id;
        if RESERVED.contains(&name.as_str())
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && graph.modules[module]
                .bindings
                .get(name.as_str())
                .is_none_or(|&binding| {
                    graph
                        .builtin_function_shadow(name.as_str(), binding)
                        .is_some()
                        || binding == Binding::Intrinsic(Intrinsic::BuiltinsPrint)
                })
        {
            let alias = BuiltinAlias {
                original: name.clone(),
                local: format!("$param:{name}").into(),
            };
            *name = alias.local.clone();
            alias.visit_body(&mut function.body);
        }
    }
}

struct BuiltinAlias {
    original: ast::name::Name,
    local: ast::name::Name,
}

impl BuiltinAlias {
    fn rename(&self, name: &mut ast::name::Name) {
        if name == &self.original {
            *name = self.local.clone();
        }
    }
}

impl Transformer for BuiltinAlias {
    // Local annotations use the static type namespace, never the parameter value.
    fn visit_annotation(&self, _: &mut ast::Expr) {}

    fn visit_expr(&self, expr: &mut ast::Expr) {
        match expr {
            ast::Expr::Name(name) => self.rename(&mut name.id),
            ast::Expr::Lambda(lambda)
                if lambda
                    .parameters
                    .as_ref()
                    .is_some_and(|p| p.find(&self.original).is_some()) =>
            {
                self.visit_parameters(lambda.parameters.as_mut().unwrap());
            }
            _ => transformer::walk_expr(self, expr),
        }
    }

    fn visit_stmt(&self, stmt: &mut ast::Stmt) {
        if let ast::Stmt::FunctionDef(nested) = stmt {
            self.rename(&mut nested.name.id);
            if nested.parameters.find(&self.original).is_some() {
                self.visit_parameters(&mut nested.parameters);
                return;
            }
        }
        transformer::walk_stmt(self, stmt);
    }

    fn visit_except_handler(&self, handler: &mut ast::ExceptHandler) {
        let ast::ExceptHandler::ExceptHandler(h) = handler;
        if let Some(name) = &mut h.name {
            self.rename(&mut name.id);
        }
        transformer::walk_except_handler(self, handler);
    }

    fn visit_pattern(&self, pattern: &mut ast::Pattern) {
        let binding = match pattern {
            ast::Pattern::MatchAs(p) => &mut p.name,
            ast::Pattern::MatchStar(p) => &mut p.name,
            ast::Pattern::MatchMapping(p) => &mut p.rest,
            _ => return transformer::walk_pattern(self, pattern),
        };
        if let Some(name) = binding {
            self.rename(&mut name.id);
        }
        transformer::walk_pattern(self, pattern);
    }
}

/// Facts about the live Python namespace, local to one module initializer.
#[derive(Clone, Default)]
pub(super) struct ModuleNamespace {
    aliases: HashSet<usize>,
    keys: HashMap<usize, String>,
    exposed: HashSet<String>,
    unknown_key: bool,
}

impl ModuleNamespace {
    pub(super) fn join(&mut self, other: &Self) {
        self.aliases.extend(&other.aliases);
        self.keys
            .retain(|slot, key| other.keys.get(slot) == Some(key));
        self.exposed.extend(other.exposed.iter().cloned());
        self.unknown_key |= other.unknown_key;
    }

    fn key(&self, value: &Expr) -> Option<String> {
        match &value.kind {
            ExprKind::Str(name) => Some(name.clone()),
            ExprKind::Local(slot) => self.keys.get(slot).cloned(),
            _ => None,
        }
    }
}

impl Checker<'_> {
    pub(super) fn live_module_binding(&self, expr: &ast::Expr) -> Option<Expr> {
        let name = expr.as_name_expr()?.id.as_str();
        (self.module_scope
            && (self.namespace.exposed.contains(name)
                || self.namespace.unknown_key && self.names.contains_key(name)))
        .then(|| registered::values::name_binding(name))
    }

    pub(super) fn is_module_namespace(&self, value: &Expr) -> bool {
        match &value.kind {
            ExprKind::Builtin(Builtin::RegisteredCall(site), _) => {
                self.namespace_calls.contains(&site.range)
            }
            ExprKind::Local(slot) => self.namespace.aliases.contains(slot),
            _ => false,
        }
    }

    fn can_release_owner(&self, slot: usize) -> bool {
        self.assigned.contains(&slot) && self.types.stores_python_object(self.locals[slot])
    }

    // A store installs its RHS before releasing the previous owner. A Python finalizer
    // can change any binding already visible in this module before the next lookup.
    fn release_module_owner(&mut self) {
        self.namespace.exposed.extend(self.names.keys().cloned());
        self.namespace.keys.clear();
    }

    pub(super) fn namespace_item_store(&mut self, index: &Expr, value: &Expr) {
        if let Some(name) = self.namespace.key(index) {
            if self.namespace.unknown_key
                || self.namespace.exposed.contains(&name)
                || self
                    .names
                    .get(&name)
                    .is_some_and(|&slot| self.can_release_owner(slot))
            {
                self.release_module_owner();
            }
            if self.types.stores_python_object(value.ty) {
                self.namespace.exposed.insert(name);
            }
        } else if matches!(index.ty, Type::Str | Type::PythonObject) {
            if !self.namespace.exposed.is_empty()
                || self.namespace.unknown_key
                || self
                    .names
                    .values()
                    .any(|&slot| self.can_release_owner(slot))
            {
                self.release_module_owner();
            }
            self.namespace.unknown_key |= self.types.stores_python_object(value.ty);
        }
    }

    pub(super) fn namespace_assignment(&mut self, slot: usize, value: &Expr) {
        let alias = self.is_module_namespace(value);
        let key = self.namespace.key(value);
        let releases_owner = self.can_release_owner(slot)
            || self.namespace.unknown_key
            || !self.namespace.exposed.is_empty()
                && self
                    .names
                    .iter()
                    .any(|(name, &local)| local == slot && self.namespace.exposed.contains(name));
        if releases_owner {
            self.release_module_owner();
        }
        if alias {
            self.namespace.aliases.insert(slot);
        } else if !releases_owner {
            self.namespace.aliases.remove(&slot);
        }
        if let Some(key) = key.filter(|_| !releases_owner) {
            self.namespace.keys.insert(slot, key);
        } else {
            self.namespace.keys.remove(&slot);
        }
    }
}

#[derive(Clone)]
pub(super) struct LocalState {
    pub(super) assigned: Assigned,
    pub(super) narrowed: Narrowed,
    namespace: ModuleNamespace,
}

impl LocalState {
    pub(super) fn retain_common(&mut self, other: &Self) {
        intersect(
            &mut self.assigned,
            &mut self.narrowed,
            &other.assigned,
            &other.narrowed,
        );
        self.namespace.join(&other.namespace);
    }
}

/// Removes storage-only optional/union promotions while preserving RHS evaluation.
pub(super) fn unpromoted(value: Expr) -> Expr {
    match value.kind {
        ExprKind::Present(inner) | ExprKind::Absent(inner) => unpromoted(*inner),
        ExprKind::Builtin(Builtin::UnionInject, mut args) => unpromoted(args.pop().unwrap()),
        _ => value,
    }
}

impl Checker<'_> {
    /// Snapshots path-dependent facts without changing fixed slots or types.
    pub(super) fn local_state(&self) -> LocalState {
        LocalState {
            assigned: self.assigned.clone(),
            narrowed: self.narrowed.clone(),
            namespace: self.namespace.clone(),
        }
    }

    /// Intersects one normally completed branch with the exits checked so far.
    pub(super) fn merge_exit(&self, joined: &mut Option<LocalState>) {
        if let Some(exit) = joined {
            exit.retain_common(&self.local_state());
        } else {
            *joined = Some(self.local_state());
        }
    }

    /// Restores the facts at an alternative control-flow predecessor.
    pub(super) fn restore_state(&mut self, state: LocalState) {
        self.assigned = state.assigned;
        self.narrowed = state.narrowed;
        self.namespace = state.namespace;
    }

    /// A binding yields the original RHS value; optional promotion belongs to
    /// storage and must not change a concrete result into an optional result.
    pub(super) fn named_expression(
        &mut self,
        expression: &ast::ExprNamed,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        let ast::Expr::Name(name) = &*expression.target else {
            return Err(error(
                self.source,
                expression,
                "assignment expressions require a local name",
            ));
        };
        if self.comprehension_targets.contains(name.id.as_str()) {
            return Err(error(
                self.source,
                expression,
                "assignment expressions cannot rebind a comprehension iteration variable",
            ));
        }
        let existing = self.names.get(name.id.as_str()).copied();
        let value = if let Some(slot) = existing {
            self.expr_expected(&expression.value, self.locals[slot])?
        } else {
            // A new binding inherits literal context, not the consumer's type:
            // an optional guard can return a different type through `and`.
            self.boolean_operand(&expression.value, expected)?
        };
        let value = unpromoted(value);
        let storage = existing.map_or(value.ty, |slot| self.locals[slot]);
        let stored = self.coerce(value.clone(), storage, expression)?;
        let slot = self.declare(name.id.as_str(), storage, expression)?;
        self.assigned_value(slot, &stored);
        if self.comprehension_depth != 0 {
            self.comprehension_exports.insert(name.id.to_string(), slot);
        }
        Ok(Expr {
            ty: value.ty,
            kind: ExprKind::Bind(slot, Box::new(value)),
        })
    }

    /// Keep both native layouts for a local that alternates between text and
    /// bytes or a list of lines. The hint selects storage; each assignment is still
    /// checked and narrows subsequent uses to the member actually stored.
    pub(super) fn widen_reassigned_text(
        &mut self,
        name: &ast::ExprName,
        rhs: &ast::Expr,
        lowered: &mut [Statement],
        later: &[ast::Stmt],
    ) -> Result<(), String> {
        let [Statement::Assign(slot, value)] = lowered else {
            return Ok(());
        };
        let list = self.types.list(Type::Str);
        let other = match (value.ty, rhs) {
            (Type::Str, ast::Expr::StringLiteral(_)) => Type::Bytes,
            (Type::Bytes, ast::Expr::BytesLiteral(_)) => Type::Str,
            (ty, _) if ty == list => Type::Str,
            _ => return Ok(()),
        };
        // An annotation starts a new storage contract. A delete ends the
        // current binding, so neither can supply a layout for its predecessor.
        let changes_layout = later.iter().find_map(|stmt| match stmt {
            ast::Stmt::Assign(a) if matches!(a.targets.as_slice(), [ast::Expr::Name(n)] if n.id == name.id) => {
                Some(if value.ty == list {
                    self.graph.session.scalar_type(self.module, &a.value) == Some(other)
                } else {
                    matches!((other, a.value.as_ref()), (Type::Str, ast::Expr::StringLiteral(_)) | (Type::Bytes, ast::Expr::BytesLiteral(_)))
                })
            }
            ast::Stmt::AnnAssign(a) if a.target.as_name_expr().is_some_and(|n| n.id == name.id) => Some(false),
            ast::Stmt::Delete(d) if d.targets.iter().any(|t| t.as_name_expr().is_some_and(|n| n.id == name.id)) => Some(false),
            _ => None,
        });
        // A walrus in the first RHS may already have emitted reads and writes
        // of this slot at its original type.
        if changes_layout == Some(true)
            && !narrowing::expression_assignments(rhs).contains(name.id.as_str())
        {
            let union = self.types.union(&[value.ty, other]).unwrap();
            // Move the just-lowered RHS into the tagged value without cloning its IR.
            let original = std::mem::replace(value, self.local(*slot));
            *value = self.coerce(original, union, name)?;
            self.locals[*slot] = union;
            self.assigned_value(*slot, value);
        }
        Ok(())
    }

    /// Target type lookup is speculative: its expression executes only after
    /// the RHS. A private checker prevents that lookup from publishing writes
    /// or helper functions before their actual source evaluation point.
    pub(super) fn assignment_context(
        &self,
        target: &ast::Expr,
        rhs: Option<&ast::Expr>,
    ) -> Option<Type> {
        let helpers = Helpers::new(0);
        let omitted_locals = self.omitted_assert_locals.clone();
        let omitted_free = self.omitted_assert_free.clone();
        let mut probe = Checker::new(
            self.graph,
            self.module,
            self.functions,
            self.classes,
            self.types,
            &helpers,
            self.globals,
        );
        probe.omitted_assert_locals = omitted_locals;
        probe.omitted_assert_free = omitted_free;
        probe.returns = self.returns;
        probe.class = self.class;
        probe.class_receiver = self.class_receiver.clone();
        probe.initializing = self.initializing;
        probe.names = self.names.clone();
        probe.locals = self.locals.clone();
        probe.assigned = self.assigned.clone();
        probe.narrowed = self.narrowed.clone();
        probe.comprehension_targets = self.comprehension_targets.clone();
        match target {
            ast::Expr::Attribute(attribute) => {
                if let Some(class) = probe.class_reference(&attribute.value)
                    && let Some(ty) = probe.class_data_type(class, attribute.attr.as_str())
                {
                    return Some(ty);
                }
                let receiver = Self::weak_target(probe.expr(&attribute.value).ok()?);
                // A mock accepts arbitrary effects and return values, including empty containers.
                if self.native_mock_instance(receiver.ty)
                    && matches!(attribute.attr.as_str(), "side_effect" | "return_value")
                {
                    return rhs.and_then(|value| self.empty_argument_type(value));
                }
                if let Type::Class(class) = receiver.ty
                    && let Some(ty) = probe.class_data_type(class, attribute.attr.as_str())
                {
                    return Some(ty);
                }
                if let Type::ChainMap(id) = receiver.ty
                    && attribute.attr.as_str() == "maps"
                {
                    return Some(self.types.list(Type::Dict(id)));
                }
                if let Some(class) = self.writable_property(receiver.ty, attribute) {
                    let setter = &self.classes.setters[class][attribute.attr.as_str()];
                    return match self.inference {
                        Some(inference) => inference.parameter(setter.id, 1),
                        None => Some(setter.params[1].ty),
                    };
                }
                if matches!(receiver.ty, Type::ModuleSpec(_))
                    && attribute.attr.as_str() == "has_location"
                {
                    return None; // its descriptor applies bool() to the original RHS.
                }
                probe
                    .writable_field_value(attribute, receiver)
                    .ok()
                    .map(|(_, _, ty)| ty)
            }
            ast::Expr::Subscript(subscript) => {
                let receiver = probe.expr(&subscript.value).ok()?;
                if matches!(receiver.ty, Type::Document(_)) {
                    return Some(if matches!(&*subscript.slice, ast::Expr::Slice(_)) {
                        self.types.list(documents::ANY)
                    } else {
                        documents::ANY
                    });
                }
                if receiver.ty == Type::JsonRoot {
                    return Some(Type::JsonValue);
                }
                self.types
                    .mapping(receiver.ty)
                    .map(|(_, value)| value)
                    .or_else(|| self.types.indexed_element(receiver.ty))
            }
            _ => None,
        }
    }
}

/// Reserve an optional slot when a later assignment calls a function with that
/// declared return type. The ordinary checker still enforces every assignment.
pub(super) fn nullable_assignments(
    checker: &Checker<'_>,
    body: &[ast::Stmt],
) -> HashMap<String, Type> {
    use ruff_python_ast::visitor::{Visitor, walk_stmt};
    struct Scan<'a, 'b> {
        checker: &'a Checker<'b>,
        found: HashMap<String, Type>,
    }
    impl<'a> Visitor<'a> for Scan<'_, '_> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if matches!(stmt, ast::Stmt::ClassDef(_) | ast::Stmt::FunctionDef(_)) {
                return;
            }
            if let ast::Stmt::Assign(a) = stmt
                && let [ast::Expr::Name(target)] = a.targets.as_slice()
                && let ast::Expr::Call(call) = &*a.value
                && let Some(Binding::Definition(def)) = self.checker.resolve(&call.func)
                && let Some(sig) = self.checker.functions.get(&def)
                && self.checker.types.payload(sig.returns).is_some()
            {
                self.found.insert(target.id.to_string(), sig.returns);
            }
            walk_stmt(self, stmt);
        }
    }
    let mut scan = Scan {
        checker,
        found: HashMap::new(),
    };
    for stmt in body {
        scan.visit_stmt(stmt);
    }
    scan.found
}

#[cfg(test)]
mod tests {
    use super::Assigned;

    #[test]
    fn subcommand_fields_stay_inside_their_conditional_arm() {
        let source = include_str!("../../../../tests/fixtures/argparse/commands.py");
        let checked = |expr: &str| crate::check(&source.replace("    print(args.label)", expr));
        checked("    print(args.text if args.command == 'write' else args.label)").unwrap();
        for expr in [
            "    print(args.text if args.command == 'write' else args.text)",
            "    print(args.command == 'write' and args.text != '')\n    print(args.text)",
        ] {
            assert!(checked(expr).unwrap_err().contains("unknown field"));
        }
    }

    #[test]
    fn snapshots_merge_assignments_across_word_boundaries() {
        let mut original = Assigned::default();
        for slot in [0, 63, 64, 128] {
            original.insert(slot);
        }
        let mut branch = original.clone();
        assert!(branch.0.ptr_eq(&original.0));
        branch.remove(&0);
        branch.remove(&128);
        branch.insert(65);
        assert!(original.contains(&0) && original.contains(&128));
        assert!(!original.contains(&65));

        let mut joined = original.clone();
        joined.intersect(&branch);
        assert!(joined.contains(&63) && joined.contains(&64));
        assert!(!joined.contains(&0) && !joined.contains(&65) && !joined.contains(&128));
        joined.extend(&branch);
        assert!(joined.contains(&65));
        assert!(!joined.contains(&0) && !joined.contains(&128));
    }
}
