//! Eager displays in prepared Python suites use the checked frontend's loop traversal.

use super::*;
use crate::frontend::comprehensions::{
    ClauseLowering, Collector, EagerDisplay, acquire_in_namespace, async_pull, filter_suite,
    scoped_result,
};
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

        let first_private = self.locals.len();
        // The first iterable belongs to the creating frame and runs before this cursor exists.
        let first = &plan.clauses[0];
        let mut input = self.creating_iterable(|scope| scope.input(first)).ok()?;
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
        let parent = self.comprehension.as_ref().map(|scope| scope.slot);
        let mut prefix = acquire_in_namespace(&mut input, namespace, parent, names);
        let result = self.hidden(Type::PythonObject);
        let outer = self.comprehension.replace(Namespace {
            slot: namespace,
            targets,
        });
        let checked = self.comprehension_body(
            &plan,
            input,
            Collector::Display(None),
            result,
            Some(&mut prefix),
            |_, _| false,
        );
        self.comprehension = outer;
        checked.ok()?;
        Some(scoped_result(
            prefix,
            Expr::local(result, Type::PythonObject),
            first_private..self.locals.len(),
            self.names.values().copied(),
        ))
    }
}

impl ClauseLowering for SourceScope<'_> {
    type Input = pipelines::Prepared;
    type Error = ();
    type Body = Vec<Statement>;

    fn input(&mut self, clause: &ast::Comprehension) -> StdResult<pipelines::Prepared, ()> {
        self.iteration_input(&clause.iter, async_pull(clause))
            .ok_or(())
    }

    fn with_target(
        &mut self,
        target: &ast::Expr,
        mut input: Self::Input,
        output: Output<'_>,
        body: impl FnOnce(&mut Self) -> StdResult<(Vec<Expr>, Vec<Statement>), ()>,
    ) -> StdResult<Vec<Statement>, ()> {
        input.bind_loop_target(self, target, false)?;
        let (conditions, body) = self.in_loop(LoopScope::WithCursor, body)?;
        let body = filter_suite(conditions, body);
        Ok(match output {
            Output::Generator(item) => generators::clause(
                input,
                body,
                item.range(),
                self.loop_scope == super::LoopScope::WithCursor,
            ),
            _ => input.for_suite(body, Vec::new(), Vec::new()),
        })
    }

    fn filter(&mut self, test: &ast::Expr) -> StdResult<Expr, ()> {
        self.ordinary_operand(test).ok_or(())
    }
}

impl EagerDisplay for SourceScope<'_> {
    fn collection_locals(&self) -> &[Type] {
        &self.locals
    }

    fn collection_key(
        &mut self,
        key: &ast::Expr,
        _: Option<Type>,
        _: Option<(usize, usize)>,
    ) -> StdResult<Expr, ()> {
        self.ordinary_operand(key).ok_or(())
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
