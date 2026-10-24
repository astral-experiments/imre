//! Assignment expressions share the enclosing typed local scope. Flow snapshots
//! keep conditional writes from becoming unconditional initialization facts.

use super::*;
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

#[derive(Clone)]
pub(super) struct LocalState {
    pub(super) assigned: Assigned,
    pub(super) narrowed: Narrowed,
}

impl LocalState {
    pub(super) fn retain_common(&mut self, assigned: &Assigned, narrowed: &Narrowed) {
        intersect(&mut self.assigned, &mut self.narrowed, assigned, narrowed);
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
        }
    }

    /// Intersects one normally completed branch with the exits checked so far.
    pub(super) fn merge_exit(&self, joined: &mut Option<LocalState>) {
        if let Some(exit) = joined {
            exit.retain_common(&self.assigned, &self.narrowed);
        } else {
            *joined = Some(self.local_state());
        }
    }

    /// Restores the facts at an alternative control-flow predecessor.
    pub(super) fn restore_state(&mut self, state: LocalState) {
        self.assigned = state.assigned;
        self.narrowed = state.narrowed;
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

    /// Target type lookup is speculative: its expression executes only after
    /// the RHS. A private checker prevents that lookup from publishing writes
    /// or helper functions before their actual source evaluation point.
    pub(super) fn assignment_context(&self, target: &ast::Expr) -> Option<Type> {
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
            ast::Expr::Attribute(attribute) => probe.field(attribute).ok().map(|(_, _, ty)| ty),
            ast::Expr::Subscript(subscript) => {
                let receiver = probe.expr(&subscript.value).ok()?;
                self.types
                    .mapping(receiver.ty)
                    .map(|(_, value)| value)
                    .or_else(|| self.types.element(receiver.ty))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Assigned;

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
