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
    writes: Vec<(String, LocalAssignment)>,
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
                    let state = checker.assigned.state(checker.names.get(&name));
                    (name, state)
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
            *assigned = checker.assigned.state(checker.names.get(name));
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
            assigned.retain_common(checker.assigned.state(checker.names.get(name)));
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
                let current = checker.assigned.state(Some(&slot));
                checker.assigned.restore(slot, *assigned);
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
        for (name, mut incoming) in self.writes {
            if let Some(&slot) = checker.names.get(&name) {
                incoming.retain_common(checker.assigned.state(Some(&slot)));
                checker.assigned.restore(slot, incoming);
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
pub(super) struct Assigned {
    words: rpds::HashTrieMap<usize, u64>,
    /// A value keeps its storage origin only when every incoming write agrees.
    type_sources: rpds::HashTrieMap<usize, ValueSource>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ValueSource {
    Declaration(TypeDeclaration),
    InferredList(DefinitionId),
    Function(usize),
    /// A different source method can occupy this captured slot when it is called.
    ReboundMethod,
}

#[derive(Default, Clone, Copy)]
struct LocalAssignment {
    initialized: bool,
    source: Option<ValueSource>,
}

impl LocalAssignment {
    fn retain_common(&mut self, other: Self) {
        self.initialized &= other.initialized;
        if self.source != other.source {
            self.source = None;
        }
    }
}

impl Assigned {
    pub(super) fn value_source(&self, slot: usize) -> Option<ValueSource> {
        self.type_sources.get(&slot).copied()
    }

    pub(super) fn set_type_source(&mut self, slot: usize, source: Option<TypeDeclaration>) {
        self.set_value_source(slot, source.map(ValueSource::Declaration));
    }

    pub(super) fn set_value_source(&mut self, slot: usize, source: Option<ValueSource>) {
        if let Some(source) = source {
            self.type_sources.insert_mut(slot, source);
        } else {
            self.type_sources.remove_mut(&slot);
        }
    }

    fn state(&self, slot: Option<&usize>) -> LocalAssignment {
        LocalAssignment {
            initialized: slot.is_some_and(|slot| self.contains(slot)),
            source: slot.and_then(|&slot| self.value_source(slot)),
        }
    }

    fn restore(&mut self, slot: usize, state: LocalAssignment) {
        if state.initialized {
            self.insert(slot);
            self.set_value_source(slot, state.source);
        } else {
            self.remove(&slot);
        }
    }

    pub(super) fn contains(&self, slot: &usize) -> bool {
        let bits = self.words.get(&(slot / 64)).copied().unwrap_or(0);
        bits & (1 << (slot % 64)) != 0
    }

    pub(super) fn insert(&mut self, slot: usize) {
        let index = slot / 64;
        let mask = 1 << (slot % 64);
        let bits = self.words.get(&index).copied().unwrap_or(0);
        if bits & mask == 0 {
            self.words.insert_mut(index, bits | mask);
        }
    }

    pub(super) fn remove(&mut self, slot: &usize) {
        self.type_sources.remove_mut(slot);
        let index = slot / 64;
        if let Some(&bits) = self.words.get(&index) {
            let remaining = bits & !(1 << (slot % 64));
            if remaining != bits {
                self.set_word(index, remaining);
            }
        }
    }

    fn set_word(&mut self, index: usize, bits: u64) {
        if bits == 0 {
            self.words.remove_mut(&index);
        } else {
            self.words.insert_mut(index, bits);
        }
    }

    fn intersect_type_sources(&mut self, other: &Self) {
        if !self.type_sources.ptr_eq(&other.type_sources) {
            for (&slot, &source) in &self.type_sources.clone() {
                if other.value_source(slot) != Some(source) {
                    self.type_sources.remove_mut(&slot);
                }
            }
        }
    }

    pub(super) fn after_iteration(&mut self, mut before: Self) {
        // A loop can perform zero assignments, or finish after a different value was stored.
        before.intersect_type_sources(self);
        *self = before;
    }

    fn intersect(&mut self, other: &Self) {
        self.intersect_type_sources(other);
        if !self.words.ptr_eq(&other.words) {
            for (&index, &bits) in &self.words.clone() {
                let common = bits & other.words.get(&index).copied().unwrap_or(0);
                if common != bits {
                    self.set_word(index, common);
                }
            }
        }
    }

    pub(super) fn extend(&mut self, other: &Self) {
        if !self.type_sources.ptr_eq(&other.type_sources) {
            for (&slot, &source) in &self.type_sources.clone() {
                if other.contains(&slot) && other.value_source(slot) != Some(source) {
                    self.type_sources.remove_mut(&slot);
                }
            }
            for (&slot, &source) in &other.type_sources {
                if !self.contains(&slot) {
                    self.type_sources.insert_mut(slot, source);
                }
            }
        }
        for (&index, &bits) in &other.words {
            let old = self.words.get(&index).copied().unwrap_or(0);
            if old | bits != old {
                self.words.insert_mut(index, old | bits);
            }
        }
    }
}

pub(super) type Narrowed = rpds::HashTrieMap<usize, Type>;

/// A successful pull can replace any local written while binding its target.
pub(super) fn forget_binding_writes(facts: &mut Narrowed, binding: &[Statement]) {
    if !facts.is_empty() {
        ir::walk_writes(binding, |slot, _| {
            facts.remove_mut(&slot);
        });
    }
}

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
    function: &mut Cow<'_, ast::StmtFunctionDef>,
    graph: &SourceGraph,
    module: ModuleId,
) -> bool {
    let needs_alias = |name: &str| {
        RESERVED.contains(&name)
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && graph.modules[module]
                .bindings
                .get(name)
                .is_none_or(|&binding| {
                    graph.builtin_function_shadow(name, binding).is_some()
                        || binding == Binding::Intrinsic(Intrinsic::BuiltinsPrint)
                })
    };
    let Some(first) = function
        .parameters
        .iter_non_variadic_params()
        .position(|param| needs_alias(param.name().as_str()))
    else {
        return false;
    };
    let function = function.to_mut();
    let params = &mut function.parameters;
    for parameter in params
        .posonlyargs
        .iter_mut()
        .chain(&mut params.args)
        .chain(&mut params.kwonlyargs)
        .skip(first)
    {
        let name = &mut parameter.parameter.name.id;
        if needs_alias(name.as_str()) {
            let alias = BuiltinAlias {
                original: name.clone(),
                local: format!("$param:{name}").into(),
            };
            *name = alias.local.clone();
            alias.visit_body(&mut function.body);
        }
    }
    true
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
    last_python_call: Option<TextSize>,
    released_owner: bool,
}

impl ModuleNamespace {
    /// During signature discovery, only the initial module bindings have a
    /// concrete owner independent of the eventual registered calling convention.
    pub(super) fn is_unexposed(&self) -> bool {
        self.last_python_call.is_none()
            && !self.released_owner
            && !self.unknown_key
            && self.exposed.is_empty()
    }

    pub(super) fn join(&mut self, other: &Self) {
        self.aliases.extend(&other.aliases);
        self.keys
            .retain(|slot, key| other.keys.get(slot) == Some(key));
        self.exposed.extend(other.exposed.iter().cloned());
        self.unknown_key |= other.unknown_key;
        self.last_python_call = self.last_python_call.max(other.last_python_call);
        self.released_owner |= other.released_owner;
    }

    fn key(&self, value: &Expr) -> Option<String> {
        match &value.kind {
            ExprKind::Str(name) => Some(name.clone()),
            ExprKind::Builtin(
                Builtin::RegisteredLocation(..) | Builtin::RegisteredIndexConstant(_),
                args,
            ) => self.key(args.first()?),
            ExprKind::Local(slot) => self.keys.get(slot).cloned(),
            _ => None,
        }
    }
}

impl Checker<'_> {
    pub(super) fn live_module_binding(&self, expr: &ast::Expr) -> Option<Expr> {
        let name = &expr.as_name_expr()?.id;
        self.live_module_name(name)
            .then(|| registered::values::name_binding(name.clone()))
    }

    pub(super) fn live_module_name(&self, name: &str) -> bool {
        self.module_scope
            && (self.module_namespace.exposed.contains(name)
                || self.module_namespace.unknown_key && self.names.contains_key(name))
    }

    pub(super) fn is_module_namespace(&self, value: &Expr) -> bool {
        match &value.kind {
            ExprKind::Invoke { args, .. } => args
                .site
                .python()
                .is_some_and(|site| self.namespace_calls.contains(&site.range)),
            ExprKind::Local(slot) => self.module_namespace.aliases.contains(slot),
            _ => false,
        }
    }

    pub(super) fn closed_passive_call(&self, call: &ast::ExprCall) -> bool {
        if self.module_namespace.unknown_key
            || self.module_namespace.released_owner
            || self.live_module_binding(&call.func).is_some()
            || !call.func.is_name_expr()
            || !call.arguments.args.iter().all(ast::Expr::is_literal_expr)
            || !call
                .arguments
                .keywords
                .iter()
                .all(|keyword| keyword.arg.is_some() && keyword.value.is_literal_expr())
        {
            return false;
        }
        let Some(Binding::Definition(id)) = self.resolve(&call.func) else {
            return false;
        };
        let definition = &self.graph.modules[id.module].parsed.syntax().body[id.statement];
        id.module == self.module
            && self
                .module_namespace
                .last_python_call
                .is_none_or(|end| end <= definition.start())
            && definition
                .as_function_def_stmt()
                .is_some_and(crate::analysis::passive_function)
    }

    /// Calling Python can replace any module binding visible when control enters the
    /// callee. A later first assignment still establishes its normal checked layout.
    pub(super) fn invalidate_called_bindings(
        &mut self,
        call: &ast::Expr,
        checked: &Result<Expr, String>,
    ) {
        if checked
            .as_ref()
            .is_ok_and(|value| self.is_module_namespace(value))
        {
            return;
        }
        let can_call_python = match checked {
            Ok(value) => matches!(
                &value.kind,
                ExprKind::Call(..)
                    | ExprKind::Invoke { .. }
                    | ExprKind::Construct { .. }
                    | ExprKind::Builtin(Builtin::RegisteredCall(..), _)
            ),
            Err(error) => error.is_object(),
        };
        if can_call_python {
            self.module_namespace.last_python_call =
                self.module_namespace.last_python_call.max(Some(call.end()));
            if !self.globals.registered_initializers {
                return;
            }
            self.module_namespace.exposed.extend(
                self.names
                    .iter()
                    .filter(|(_, slot)| self.assigned.contains(slot))
                    .map(|(name, _)| name.clone()),
            );
            self.module_namespace.keys.clear();
        }
    }

    fn can_release_owner(&self, slot: usize) -> bool {
        self.assigned.contains(&slot) && self.types.stores_python_object(self.locals[slot])
    }

    // A store installs its RHS before releasing the previous owner. A Python finalizer
    // can change any binding already visible in this module before the next lookup.
    fn release_module_owner(&mut self) {
        // A finalizer can replace a function before the next direct call looks it up.
        self.module_namespace.released_owner = true;
        self.module_namespace
            .exposed
            .extend(self.names.keys().cloned());
        self.module_namespace.keys.clear();
    }

    pub(super) fn namespace_item_store(&mut self, index: &Expr, value: &Expr) {
        if let Some(name) = self.module_namespace.key(index) {
            if self.module_namespace.unknown_key
                || self.module_namespace.exposed.contains(&name)
                || self
                    .names
                    .get(&name)
                    .is_some_and(|&slot| self.can_release_owner(slot))
            {
                self.release_module_owner();
            }
            if self.types.stores_python_object(value.ty) {
                self.module_namespace.exposed.insert(name);
            }
        } else if matches!(index.ty, Type::Str | Type::PythonObject) {
            if !self.module_namespace.exposed.is_empty()
                || self.module_namespace.unknown_key
                || self
                    .names
                    .values()
                    .any(|&slot| self.can_release_owner(slot))
            {
                self.release_module_owner();
            }
            self.module_namespace.unknown_key |= self.types.stores_python_object(value.ty);
        }
    }

    pub(super) fn namespace_assignment(&mut self, slot: usize, value: &Expr) {
        let alias = self.is_module_namespace(value);
        let key = self.module_namespace.key(value);
        let releases_owner = self.can_release_owner(slot)
            || self.module_namespace.unknown_key
            || !self.module_namespace.exposed.is_empty()
                && self.names.iter().any(|(name, &local)| {
                    local == slot && self.module_namespace.exposed.contains(name)
                });
        if releases_owner {
            self.release_module_owner();
        }
        if alias {
            self.module_namespace.aliases.insert(slot);
        } else if !releases_owner {
            self.module_namespace.aliases.remove(&slot);
        }
        if let Some(key) = key.filter(|_| !releases_owner) {
            self.module_namespace.keys.insert(slot, key);
        } else {
            self.module_namespace.keys.remove(&slot);
        }
    }
}

#[derive(Clone)]
pub(super) struct LocalState {
    pub(super) assigned: Assigned,
    pub(super) narrowed: Narrowed,
    module_namespace: ModuleNamespace,
}

impl LocalState {
    pub(super) fn retain_common(&mut self, other: &Self) {
        intersect(
            &mut self.assigned,
            &mut self.narrowed,
            &other.assigned,
            &other.narrowed,
        );
        self.module_namespace.join(&other.module_namespace);
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
            module_namespace: self.module_namespace.clone(),
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
        self.module_namespace = state.module_namespace;
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
        let stored = if value.ty == storage {
            self.keep_module_list_layout(&value);
            Cow::Borrowed(&value)
        } else {
            Cow::Owned(self.coerce(value.clone(), storage, expression)?)
        };
        let slot = self.declare(name.id.as_str(), storage, expression)?;
        self.assigned_value(slot, &stored);
        if let Some(exports) = &mut self.comprehension_exports {
            exports.insert(name.id.to_string(), slot);
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
    ) -> Option<(Type, Option<TypeDeclaration>)> {
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
                    return Some((ty, None));
                }
                let receiver = Self::weak_target(probe.expr(&attribute.value).ok()?);
                // A mock accepts arbitrary effects and return values, including empty containers.
                if self.native_mock_instance(receiver.ty)
                    && matches!(attribute.attr.as_str(), "side_effect" | "return_value")
                {
                    return rhs
                        .and_then(|value| self.empty_argument_type(value))
                        .map(|ty| (ty, None));
                }
                if let Type::Class(class) = receiver.ty
                    && let Some(ty) = probe.class_data_type(class, attribute.attr.as_str())
                {
                    return Some((ty, None));
                }
                if let Type::ChainMap(id) = receiver.ty
                    && attribute.attr.as_str() == "maps"
                {
                    return Some((self.types.list(Type::Dict(id)), None));
                }
                if let Some((_, setter)) = self.writable_property(receiver.ty, attribute) {
                    return match self.inference {
                        Some(inference) => inference.parameter(setter.id, 1).map(|ty| (ty, None)),
                        None => Some((setter.params[1].ty, None)),
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
                    .map(|(_, _, ty)| (ty, None))
            }
            ast::Expr::Subscript(subscript) => {
                let receiver = probe.expr(&subscript.value).ok()?;
                self.keep_module_list_layout(&receiver);
                let declaration = probe.value_declaration(&receiver);
                if matches!(receiver.ty, Type::Document(_)) {
                    return Some((
                        if matches!(&*subscript.slice, ast::Expr::Slice(_)) {
                            self.types.list(documents::ANY)
                        } else {
                            documents::ANY
                        },
                        declaration,
                    ));
                }
                if receiver.ty == Type::JsonRoot {
                    return Some((Type::JsonValue, declaration));
                }
                self.types
                    .mapping(receiver.ty)
                    .map(|(_, value)| value)
                    .or_else(|| self.types.indexed_element(receiver.ty))
                    .map(|ty| (ty, declaration))
            }
            _ => None,
        }
    }
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
        assert!(branch.words.ptr_eq(&original.words));
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
