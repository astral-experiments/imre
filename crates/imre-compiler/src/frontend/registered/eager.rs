//! Eager displays in prepared Python suites use the checked frontend's loop traversal.

use super::*;
use crate::frontend::comprehensions::EagerDisplay;
use imre_ast::comprehension::{Output, Plan};

pub(super) struct Namespace {
    pub(super) slot: usize,
    pub(super) targets: HashMap<String, usize>,
}

impl SourceScope<'_> {
    pub(super) fn creating_iterable<T>(&mut self, lower: impl FnOnce(&mut Self) -> T) -> T {
        if matches!(self.kind, RegisteredScope::Function { .. } | RegisteredScope::Lambda)
            && self.loop_scope != LoopScope::WithCursor
        {
            self.with_suspending_operands(lower)
        } else {
            lower(self)
        }
    }

    pub(super) fn eager_comprehension(&mut self, expression: &ast::Expr) -> Option<Expr> {
        let plan = Plan::from_expr(expression)?;
        if plan.body_suspends() && !matches!(self.kind, RegisteredScope::Function { is_async: true }) {
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
        let empty = match plan.output {
            Output::List(_) => ExprKind::List(Vec::new()),
            Output::Set(_) => ExprKind::Set {
                element: Type::PythonObject,
                values: Vec::new(),
            },
            Output::Dict(..) => ExprKind::Builtin(Builtin::RegisteredDict(Vec::new()), Vec::new()),
            Output::Generator(_) => return None,
        };
        let result = self.hidden(Type::PythonObject);
        prefix.push(Statement::Assign(
            result,
            Expr {
                ty: Type::PythonObject,
                kind: empty,
            },
        ));
        let outer = self.comprehension.replace(Namespace {
            slot: namespace,
            targets,
        });
        let loops = self.eager_loop(&plan, 0, None, input, (), result);
        self.comprehension = outer;
        prefix.extend(loops.ok()?);
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

pub(super) fn async_pull(clause: &ast::Comprehension) -> Option<ir::AsyncPull> {
    clause.is_async.then(|| ir::AsyncPull {
        range: clause.target.range(),
        header: None,
    })
}

impl EagerDisplay for SourceScope<'_> {
    type Input = pipelines::Prepared;
    type Destination = ();
    type Error = ();

    fn input(&mut self, clause: &ast::Comprehension) -> StdResult<pipelines::Prepared, ()> {
        self.iteration_input(&clause.iter, async_pull(clause)).ok_or(())
    }

    fn with_target(
        &mut self,
        target: &ast::Expr,
        input: Self::Input,
        output: Output<'_>,
        body: impl FnOnce(&mut Self) -> StdResult<Vec<Statement>, ()>,
    ) -> StdResult<Vec<Statement>, ()> {
        let (binding, body) = self.iteration_body(target, &input, body)?;
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

    fn invalid_edges(&self, _: &Plan<'_>) {}

    fn collect(
        &mut self,
        output: Output<'_>,
        (): (),
        result: usize,
        _: &[Expr],
    ) -> StdResult<Vec<Statement>, ()> {
        let collection = Expr::local(result, Type::PythonObject);
        Ok(match output {
            Output::List(item) | Output::Set(item) => {
                let op = if matches!(output, Output::Set(_)) {
                    Builtin::SetAdd
                } else {
                    Builtin::Append
                };
                vec![Statement::Expr(operation(
                    op,
                    Type::None,
                    vec![collection, self.ordinary_operand(item).ok_or(())?],
                ))]
            }
            Output::Dict(key, value) => {
                let key = self.ordinary_operand(key).ok_or(())?;
                let captured = self.hidden(Type::PythonObject);
                let value = self.ordinary_operand(value).ok_or(())?;
                vec![Statement::Scope {
                    body: vec![
                        Statement::Assign(captured, key),
                        Statement::SetItem(
                            collection,
                            Expr::local(captured, Type::PythonObject),
                            value,
                        ),
                    ],
                    clear: vec![captured],
                }]
            }
            Output::Generator(_) => return Err(()),
        })
    }
}
