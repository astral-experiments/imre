//! Assignment expressions share the enclosing typed local scope. Flow snapshots
//! keep conditional writes from becoming unconditional initialization facts.

use super::*;
use ruff_python_ast::visitor::transformer::{self, Transformer};

/// Keep a parameter's public keyword while giving its lexical references an
/// internal spelling. Builtin fast paths can then match names without ever
/// interpreting a local `bytes`, `map`, or `len` as the builtin.
pub(super) fn alias_builtin_parameters(
    function: &mut ast::StmtFunctionDef,
    bindings: &HashMap<String, Binding>,
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
            && !bindings.contains_key(name.as_str())
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

#[derive(Clone)]
pub(super) struct LocalState {
    pub(super) assigned: HashSet<usize>,
    pub(super) narrowed: HashMap<usize, Type>,
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

    /// Restores the facts at an alternative control-flow predecessor.
    pub(super) fn restore_state(&mut self, state: LocalState) {
        self.assigned = state.assigned;
        self.narrowed = state.narrowed;
    }

    /// Keeps only facts established on every normally completed path.
    pub(super) fn join_states(&mut self, states: impl IntoIterator<Item = LocalState>) {
        let mut states = states.into_iter();
        let mut joined = states.next().expect("a checked expression has an exit");
        for state in states {
            joined.assigned.retain(|slot| state.assigned.contains(slot));
            joined
                .narrowed
                .retain(|slot, ty| state.narrowed.get(slot) == Some(ty));
        }
        self.restore_state(joined);
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
                if matches!(receiver.ty, Type::Document(_)) {
                    return Some(if matches!(&*subscript.slice, ast::Expr::Slice(_)) {
                        self.types.list(documents::ANY)
                    } else {
                        documents::ANY
                    });
                }
                self.types
                    .mapping(receiver.ty)
                    .map(|(_, value)| value)
                    .or_else(|| self.types.element(receiver.ty))
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
