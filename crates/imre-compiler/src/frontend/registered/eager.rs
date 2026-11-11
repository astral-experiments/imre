//! Eager displays in prepared Python suites use the checked frontend's loop traversal.

use super::*;
use crate::frontend::comprehensions::{Collector, EagerDisplay, async_pull};
use imre_ast::comprehension::{Output, Plan};

pub(super) struct Namespace {
    pub(super) slot: usize,
    pub(super) targets: HashMap<String, usize>,
}

impl SourceScope<'_> {
    pub(super) fn creating_iterable<T>(&mut self, lower: impl FnOnce(&mut Self) -> T) -> T {
        if matches!(
            self.kind,
            RegisteredScope::Function { .. } | RegisteredScope::Lambda
        ) && self.loop_scope != LoopScope::WithCursor
        {
            self.with_suspending_operands(lower)
        } else {
            lower(self)
        }
    }

    pub(super) fn eager_comprehension(&mut self, expression: &ast::Expr) -> Option<Expr> {
        let plan = Plan::from_expr(expression)?;
        if plan.body_suspends()
            && !matches!(self.kind, RegisteredScope::Function { is_async: true })
        {
            return None;
        }

        let first_private = self.locals.len();
        // The first iterable belongs to the creating frame and runs before this cursor exists.
        let first = &plan.clauses[0];
        let mut input = self.creating_iterable(|scope| scope.input(first)).ok()?;
        let mut prefix = std::mem::take(&mut input.evaluate);
        prefix.append(&mut input.acquire);
        let namespace = self.hidden(Type::PythonObject);
        let mut names: Vec<_> = plan.target_names().into_iter().collect();
        names.sort();
        let mut targets = HashMap::new();
        for name in &names {
            let slot = self.hidden(Type::PythonObject);
            self.namespaces.insert(
                slot,
                LocalBinding {
                    name: name.clone(),
                    namespace: Some(namespace),
                },
            );
            targets.insert(name.clone(), slot);
        }
        let parent = self
            .comprehension
            .as_ref()
            .map(|scope| Expr::local(scope.slot, Type::PythonObject));
        prefix.push(Statement::Assign(
            namespace,
            operation(
                Builtin::RegisteredNamespace(names),
                Type::PythonObject,
                parent.into_iter().collect(),
            ),
        ));
        let result = self.hidden(Type::PythonObject);
        let outer = self.comprehension.replace(Namespace {
            slot: namespace,
            targets,
        });
        let body =
            self.comprehension_body(&plan, input, Collector::Display(None), result, |_, _| {});
        self.comprehension = outer;
        prefix.extend(body.ok()?.1);
        // A walrus writes into the enclosing activation. Release only the display's private slots.
        let exported: HashSet<_> = self.names.values().copied().collect();
        let clear = (first_private..self.locals.len())
            .filter(|slot| !exported.contains(slot))
            .collect();
        Some(Expr {
            ty: Type::PythonObject,
            kind: ExprKind::Scope {
                body: prefix,
                value: Box::new(Expr::local(result, Type::PythonObject)),
                clear,
            },
        })
    }
}

impl EagerDisplay for SourceScope<'_> {
    type Input = pipelines::Prepared;
    type Error = ();

    fn input(&mut self, clause: &ast::Comprehension) -> StdResult<pipelines::Prepared, ()> {
        self.iteration_input(&clause.iter, async_pull(clause))
            .ok_or(())
    }

    fn with_target(
        &mut self,
        target: &ast::Expr,
        input: Self::Input,
        output: Output<'_>,
        body: impl FnOnce(&mut Self) -> StdResult<Vec<Statement>, ()>,
    ) -> StdResult<Vec<Statement>, ()> {
        let binding = input.bind_target(self, target)?;
        let body = self.in_loop(LoopScope::WithCursor, body)?;
        Ok(match output {
            Output::Generator(item) => generators::clause(
                input,
                binding,
                body,
                item.range(),
                self.loop_scope == super::LoopScope::WithCursor,
            ),
            _ => input.for_suite(binding, body, Vec::new(), Vec::new()),
        })
    }

    fn filter(&mut self, test: &ast::Expr) -> StdResult<Expr, ()> {
        self.ordinary_operand(test).ok_or(())
    }

    fn collection_locals(&self) -> &[Type] {
        &self.locals
    }

    fn collection_key(
        &mut self,
        key: &ast::Expr,
        _: Option<Type>,
        _: Option<(usize, usize)>,
    ) -> StdResult<(usize, Expr), ()> {
        let value = self.ordinary_operand(key).ok_or(())?;
        Ok((self.hidden(Type::PythonObject), value))
    }

    fn collection_value(
        &mut self,
        output: Output<'_>,
        _: Collector,
        _: usize,
        key: Option<&Expr>,
        _: Option<(usize, usize)>,
    ) -> StdResult<Expr, ()> {
        if key.is_some() {
            self.ordinary_operand(output.value()).ok_or(())
        } else {
            values::collection_item(output.value(), |value| {
                self.ordinary_operand(value).ok_or(())
            })
        }
    }
}
