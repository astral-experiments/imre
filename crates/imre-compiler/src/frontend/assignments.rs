//! Assignment expressions share the enclosing typed local scope. Flow snapshots
//! keep conditional writes from becoming unconditional initialization facts.

use super::*;

/// The assignment and narrowing facts at a checked control-flow edge.
#[derive(Clone)]
pub(super) struct Flow {
    assigned: Assigned,
    narrowed: Narrowed,
}

impl Flow {
    pub(super) fn read(checker: &Checker<'_>) -> Self {
        Self {
            assigned: checker.assigned.clone(),
            narrowed: checker.narrowed.clone(),
        }
    }

    pub(super) fn restore(&self, checker: &mut Checker<'_>) {
        checker.assigned.clone_from(&self.assigned);
        checker.narrowed.clone_from(&self.narrowed);
    }

    /// Resume the alternative branch, keeping the completed branch for the join.
    pub(super) fn switch_arm(&mut self, checker: &mut Checker<'_>) {
        std::mem::swap(&mut self.assigned, &mut checker.assigned);
        std::mem::swap(&mut self.narrowed, &mut checker.narrowed);
    }

    pub(super) fn join(&self, checker: &mut Checker<'_>) {
        intersect(
            &mut checker.assigned,
            &mut checker.narrowed,
            &self.assigned,
            &self.narrowed,
        );
    }

    pub(super) fn intersect(&mut self, other: &Self) {
        intersect(
            &mut self.assigned,
            &mut self.narrowed,
            &other.assigned,
            &other.narrowed,
        );
    }
}

/// Intersect the exits that short-circuit before the next operand is evaluated.
#[derive(Default)]
pub(super) struct BooleanExits {
    exit: Option<Flow>,
    stopped: bool,
}

impl BooleanExits {
    pub(super) fn operand(
        &mut self,
        checker: &mut Checker<'_>,
        operand: &ast::Expr,
        and: bool,
        known: Option<bool>,
    ) {
        if !self.stopped && known != Some(and) {
            let mut branch = Flow::read(checker);
            checker.apply_facts(operand, !and);
            branch.switch_arm(checker);
            if let Some(exit) = &mut self.exit {
                exit.intersect(&branch);
            } else {
                self.exit = Some(branch);
            }
            self.stopped = known == Some(!and);
        }
        checker.apply_facts(operand, and);
    }

    pub(super) fn finish(self, checker: &mut Checker<'_>) {
        if let Some(exit) = self.exit {
            if !self.stopped {
                exit.join(checker);
            } else {
                exit.restore(checker);
            }
        }
    }
}

// Snapshots share unchanged trie nodes with the current flow. Entering a
// nested suite only copies the paths for slots whose facts actually change.
#[derive(Clone, Default)]
pub(super) struct Assigned {
    pub(super) has_frame_writes: bool,
    alternatives: bool,
    words: rpds::HashTrieMap<usize, u64>,
    /// A value keeps its storage origin only when every incoming write agrees.
    type_sources: rpds::HashTrieMap<SourceSlot, ValueSource>,
}

#[derive(Clone, Hash, PartialEq, Eq)]
pub(super) enum SourceSlot {
    Local(usize),
    Caller(String),
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum ValueSource {
    /// A potentially live frame namespace received a write to this named slot.
    FrameBinding(std::rc::Rc<HashSet<(TextSize, Type)>>),
    /// The stored value is a writable namespace acquired by a checked frame lookup.
    FrameLocals {
        site: TextSize,
        caller: bool,
    },
    /// The selected invoke produces a live Python handle and has no alternative fixed layout.
    PythonCall,
    Declaration(TypeDeclaration),
    /// A single unbound target stored under an exact coroutine callback annotation.
    CoroutineCallback(TypeDeclaration, usize),
    InferredList(DefinitionId),
    LiteralStrings(Type),
    Function(usize),
    /// A different source method can occupy this captured slot when it is called.
    ReboundMethod,
}

impl Assigned {
    pub(super) fn value_source(&self, slot: usize) -> Option<ValueSource> {
        self.type_sources.get(&SourceSlot::Local(slot)).cloned()
    }

    pub(super) fn set_type_source(&mut self, slot: usize, source: Option<TypeDeclaration>) {
        self.set_value_source(slot, source.map(ValueSource::Declaration));
    }

    pub(super) fn set_value_source(&mut self, slot: usize, source: Option<ValueSource>) {
        let slot = SourceSlot::Local(slot);
        if let Some(source) = source {
            self.type_sources.insert_mut(slot, source);
        } else {
            self.type_sources.remove_mut(&slot);
        }
    }

    /// A second lookup can return a different frame. Only writes through the
    /// same acquired namespace replace one another's possible value.
    pub(super) fn write_frame_local(
        &mut self,
        slot: SourceSlot,
        owner: TextSize,
        types: impl IntoIterator<Item = Type>,
    ) {
        self.has_frame_writes = true;
        let mut writes = match self.type_sources.get(&slot) {
            Some(ValueSource::FrameBinding(writes)) => writes.clone(),
            _ => Default::default(),
        };
        let possible = std::rc::Rc::make_mut(&mut writes);
        possible.retain(|(prior, _)| *prior != owner);
        possible.extend(types.into_iter().map(|ty| (owner, ty)));
        self.type_sources
            .insert_mut(slot, ValueSource::FrameBinding(writes));
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
        self.type_sources.remove_mut(&SourceSlot::Local(*slot));
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

    /// A storage origin must agree on every incoming path. A frame write can
    /// affect a later read when either incoming path reaches it.
    fn merge_type_sources(&mut self, other: &Self, include_new_slots: bool) {
        self.has_frame_writes |= other.has_frame_writes;
        self.alternatives |= other.alternatives;
        if self.type_sources.ptr_eq(&other.type_sources) {
            return;
        }
        self.alternatives |= self.has_frame_writes;
        for (slot, source) in &self.type_sources.clone() {
            if let ValueSource::FrameBinding(left) = source {
                // A shared finally also runs on interrupted paths. A possible
                // value from only one of its incoming states cannot prove a
                // normal caller return. Keep the slot live for its scalar guard.
                if include_new_slots && other.type_sources.get(slot) != Some(source) {
                    let mut agreed = left.clone();
                    std::rc::Rc::make_mut(&mut agreed).retain(|write| matches!(
                        other.type_sources.get(slot), Some(ValueSource::FrameBinding(right)) if right.contains(write)
                    ));
                    self.type_sources
                        .insert_mut(slot.clone(), ValueSource::FrameBinding(agreed));
                }
            } else if (!include_new_slots
                || matches!(slot, SourceSlot::Local(n) if other.contains(n)))
                && other.type_sources.get(slot) != Some(source)
            {
                self.type_sources.remove_mut(slot);
            }
        }
        if !include_new_slots && !other.has_frame_writes {
            return;
        }
        for (slot, source) in &other.type_sources {
            if let ValueSource::FrameBinding(right) = source {
                if include_new_slots {
                    if !matches!(
                        self.type_sources.get(slot),
                        Some(ValueSource::FrameBinding(_))
                    ) {
                        self.type_sources.insert_mut(
                            slot.clone(),
                            ValueSource::FrameBinding(Default::default()),
                        );
                    }
                    continue;
                }
                match self.type_sources.get_mut(slot) {
                    Some(ValueSource::FrameBinding(left)) if left != right => {
                        std::rc::Rc::make_mut(left).extend(right.iter().copied());
                    }
                    Some(ValueSource::FrameBinding(_)) => {}
                    _ => self.type_sources.insert_mut(slot.clone(), source.clone()),
                }
            } else if include_new_slots && matches!(slot, SourceSlot::Local(n) if !self.contains(n))
            {
                self.type_sources.insert_mut(slot.clone(), source.clone());
            }
        }
    }

    /// A new conditional suite may exclude the path that wrote the observed
    /// slot. The Python frame stays live, but its earlier value is no longer
    /// evidence that an annotated store can succeed on this path.
    pub(super) fn enter_frame_statement(&mut self, stmt: &ast::Stmt) {
        if self.alternatives && (ast::helpers::is_compound_statement(stmt) || stmt.is_assert_stmt())
        {
            self.alternatives = false;
            for (slot, source) in &self.type_sources.clone() {
                if matches!(source, ValueSource::FrameBinding(_)) {
                    self.type_sources
                        .insert_mut(slot.clone(), ValueSource::FrameBinding(Default::default()));
                }
            }
        }
    }

    pub(super) fn after_iteration(&mut self, mut before: Self) {
        // A loop can perform zero assignments, or finish after a different value was stored.
        before.merge_type_sources(self, false);
        *self = before;
    }

    fn intersect(&mut self, other: &Self) {
        self.merge_type_sources(other, false);
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
        self.merge_type_sources(other, true);
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

/// Effects on Python namespaces along the current control-flow path.
#[derive(Clone, Default)]
pub(super) struct ModuleNamespace {
    aliases: HashSet<usize>,
    keys: HashMap<usize, String>,
    exposed: HashSet<String>,
    unknown_key: bool,
    last_python_call: Option<TextSize>,
    released_owner: bool,
    /// Pairs of supplied class objects and layouts whose fields a parser can write.
    pub(super) supplied_classes: HashSet<(usize, usize)>,
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
        self.supplied_classes.extend(&other.supplied_classes);
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
    /// Resolve a direct frame lookup where the namespace object is first acquired.
    pub(super) fn frame_locals(&self, source: &ast::Expr) -> Option<ValueSource> {
        let field = source.as_attribute_expr()?;
        let call = field.value.as_call_expr()?;
        if field.attr.as_str() != "f_locals"
            || !call.arguments.keywords.is_empty()
            || !self
                .graph
                .session
                .operand_types(self.module)
                .is_original_operand(source)
            || !self
                .graph
                .session
                .is_stdlib_function(self.module, &call.func, "sys", "_getframe")
        {
            return None;
        }
        let caller = match call.arguments.args.as_ref() {
            [] => false,
            [ast::Expr::NumberLiteral(n)] if let ast::Number::Int(depth) = &n.value => {
                match depth.as_i64()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                }
            }
            _ => return None,
        };
        Some(ValueSource::FrameLocals {
            site: source.start(),
            caller,
        })
    }

    pub(super) fn record_caller_frame_return(&mut self) {
        if !self.assigned.has_frame_writes
            || !self
                .assigned
                .type_sources
                .keys()
                .any(|slot| matches!(slot, SourceSlot::Caller(_)))
        {
            return;
        }
        if let Some(prior) = &mut self.caller_frame_returns {
            prior.merge_type_sources(&self.assigned, false);
        } else {
            self.caller_frame_returns = Some(Box::new(self.assigned.clone()));
        }
    }

    /// Publish only values that reached a normal exit from this physical callee.
    pub(super) fn finish_caller_frame_stores(&mut self, signature: &Signature, falls: bool) {
        if falls {
            self.record_caller_frame_return();
        }
        let mut summaries = self.classes.caller_frame_stores.borrow_mut();
        if let Some(returned) = self.caller_frame_returns.take() {
            summaries.insert(signature.id, returned);
        } else {
            summaries.remove(&signature.id);
        }
    }

    pub(super) fn apply_caller_frame_stores(
        &mut self,
        signature: &Signature,
        call: &ast::ExprCall,
    ) {
        if self.module_scope
            || !self.reachable
            || signature.asynchronous
            || self.classes.generators.contains(&signature.id)
        {
            return;
        }
        if let Some(stores) = self.classes.caller_frame_stores.borrow().get(&signature.id) {
            for (key, value) in &stores.type_sources {
                if let (SourceSlot::Caller(name), ValueSource::FrameBinding(writes)) = (key, value)
                    && let Some(&slot) = self.names.get(name)
                    && !self.declared_slots.contains_key(&slot)
                {
                    // A return can reach the caller without writing this slot.
                    self.assigned.alternatives = true;
                    self.assigned.write_frame_local(
                        SourceSlot::Local(slot),
                        call.start(),
                        writes.iter().map(|&(_, ty)| ty),
                    );
                }
            }
        }
    }

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
                .is_some_and(|site| self.namespace_calls.contains(&site.call.range)),
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
                    | ExprKind::Builtin(Builtin::UnrepresentedCall, _)
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
        if expression.value.is_bool_op_expr() {
            // An unrolled loop can check the same source with different operands.
            self.stable_selections.remove(&expression.range());
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
        if self.registered.is_none()
            && !self.placement_only
            && value.ty.builtin_scalar_name().is_some()
            && matches!(
                value.kind,
                ExprKind::BooleanValue(..) | ExprKind::Boolean(..)
            )
            && expression.value.is_bool_op_expr()
        {
            self.stable_selections.insert(expression.range());
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
        let receiver = match target {
            ast::Expr::Attribute(attribute) => &*attribute.value,
            ast::Expr::Subscript(subscript) => &*subscript.value,
            _ => return None,
        };
        // A bound Python method returns a live object. Neither the local nor that
        // call supplies a fixed field or item type for the assignment's RHS.
        let object = if self.checked_call_contracts
            && let Some(module) = self.registered
            && let Some(call) = receiver.as_call_expr()
            && stdlib::registered_string_method(call).is_none()
            && let Some(method) = module
                .operands(self.source, self.graph.python_optimization())
                .direct_method(call)
        {
            method.value.as_ref()
        } else {
            receiver
        };
        if let Some(name) = object.as_name_expr()
            && let Some(&slot) = self.names.get(name.id.as_str())
            && self.locals[slot] == Type::PythonObject
            && !self.narrowed.contains_key(&slot)
            && !self.is_class_receiver(object)
            && self.graph.resolve(self.module, object).is_none()
        {
            return None;
        }
        let helpers = Helpers::new(0);
        let mut probe = Checker::new(
            self.graph,
            self.module,
            self.functions,
            self.classes,
            self.types,
            &helpers,
            self.globals,
        );
        probe.omitted_assert_locals = self.omitted_assert_locals.clone();
        probe.omitted_assert_free = self.omitted_assert_free.clone();
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
        let (result, lowered) = crate::frontend::caller_cache::observe_lowerings(|| {
            checked("    print(args.text if args.command == 'write' else args.label)")
        });
        result.unwrap();
        assert_eq!(lowered.iter().filter(|name| *name == "read").count(), 1);
        for expr in [
            "    print(args.text if args.command == 'write' else args.text)",
            "    print(args.command == 'write' and args.text != '')\n    print(args.text)",
        ] {
            assert!(checked(expr).unwrap_err().contains("unknown field"));
        }
    }

    #[test]
    fn an_annotated_local_requires_a_reaching_frame_write() {
        let check = |after_store, after_call| {
            let source = format!(
                r"import sys
def update(flag: bool) -> None:
    if flag:
        sys._getframe(1).f_locals['chosen'] = 17
        {after_store}
def read(flag: bool) -> int:
    chosen = 'old'
    update(flag)
    {after_call}
    alias = chosen if flag else chosen
    required: int = alias
    return required
print(read(True))
"
            );
            crate::check(&source)
        };
        check("return", "pass").unwrap();
        for (after_store, after_call) in [
            ("raise ValueError()", "pass"),
            ("return", "if flag: raise ValueError()"),
        ] {
            let error = check(after_store, after_call).unwrap_err();
            assert!(
                error.contains("11:21: Imre: expected int, got str"),
                "{error}"
            );
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
