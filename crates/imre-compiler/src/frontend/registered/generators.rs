//! Prepare the hidden activation of a generator expression.

use super::*;
use crate::frontend::comprehensions::ClauseLowering;
use crate::frontend::function_scope::{CallableMetadata, FunctionScope, LexicalSite};
use imre_ast::comprehension::Plan;

pub(crate) struct RegisteredGenerator {
    pub(crate) body: RegisteredFunction,
    pub(crate) metadata: CallableMetadata,
}

#[derive(Default)]
pub(crate) struct LexicalOperands {
    pub(crate) generators: HashMap<TextRange, RegisteredGenerator>,
    pub(crate) lambdas: HashMap<TextSize, RegisteredLambda>,
    pub(crate) expressions: Option<ExpressionSites>,
}

impl LexicalOperands {
    pub(crate) fn prepare(
        &mut self,
        site: LexicalSite<'_, '_>,
        source: values::RegisteredSource<'_>,
        strings: bool,
    ) {
        let source = values::RegisteredSource {
            generators: Some(&self.generators),
            lambdas: Some(&self.lambdas),
            ..source
        };
        match site {
            LexicalSite::Generator(range, plan, is_async, scope) => {
                let child = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                    RegisteredGenerator::prepare(plan, is_async, source, scope)
                });
                if let Some(child) = child {
                    self.generators.insert(range, child);
                }
            }
            LexicalSite::Lambda(lambda, owner, yields) => {
                let scope = FunctionScope::lambda(lambda, strings);
                let plan = RegisteredLambda::prepare(lambda, source, &scope, owner, yields);
                if let Some(expressions) = &mut self.expressions {
                    expressions.register_lexical(lambda.into(), std::rc::Rc::clone(&plan.binding));
                }
                self.lambdas.insert(lambda.start(), plan);
            }
            LexicalSite::Template(template, owner) => {
                if let Some(expressions) = &mut self.expressions {
                    expressions.prepare_template(template, source, owner);
                }
            }
        }
    }
}

impl RegisteredGenerator {
    #[cfg(test)]
    pub(super) fn is_async(&self) -> bool {
        self.metadata.suspension_flags() == imre_ast::code_flags::ASYNC_GENERATOR
    }

    fn prepare(
        plan: &Plan<'_>,
        is_async: bool,
        source: values::RegisteredSource<'_>,
        scope_facts: &FunctionScope,
    ) -> Option<Self> {
        // The constructor installs the target declarations and the acquired cursor in
        // a fresh comprehension frame. Reads and writes now use that frame directly.
        let operations = HashMap::new();
        let mut scope = SourceScope::new(
            RegisteredScope::Function { is_async },
            source,
            &operations,
            false,
        );
        scope.generator_expression = true;
        let outer = imre_ast::comprehension::outer_clause(plan.clauses);
        let pull = comprehensions::async_pull(outer);
        let argument = scope.slot(".0");
        let input = cursor_input(
            Expr::local(argument, Type::PythonObject),
            Type::PythonObject,
            None,
            pull,
            |ty, cursor| scope.reserve_cursor_slot(ty, cursor),
        );
        let mut body = scope
            .comprehension_loop(plan, 0, input, &mut |scope, output, _| {
                let item = output.value();
                let yield_item = |value| {
                    Statement::Expr(operation(
                        Builtin::RegisteredSuspend(ir::SuspendValue::Yield, item.range()),
                        Type::PythonObject,
                        vec![value],
                    ))
                };
                if let ast::Expr::Starred(star) = item {
                    let input = scope.iteration_input(&star.value, None).ok_or(())?;
                    return Ok(loop_clause(
                        input,
                        item.range(),
                        true,
                        |mut input, boundary| {
                            let projection = input.pull_expression(Type::PythonObject);
                            input
                                .next
                                .extend([boundary.clone(), yield_item(projection)]);
                            input.loop_suite(Vec::new(), Vec::new())
                        },
                    ));
                }
                Ok(vec![yield_item(scope.ordinary_operand(item).ok_or(())?)])
            })
            .ok()?;
        body.push(Statement::Return(Expr::none()));
        let mut body = scope.finish(body);
        body.function.params.push(argument);
        Some(Self {
            body,
            metadata: CallableMetadata::generator(scope_facts, is_async),
        })
    }
}

/// Emit one lazy clause using the checked iterator scope and the registered unwind edges.
pub(super) fn clause(
    input: pipelines::Prepared,
    mut body: Vec<Statement>,
    site: TextRange,
    nested: bool,
) -> Vec<Statement> {
    loop_clause(input, site, nested, |mut input, boundary| {
        // Publish the cursor before each pull, then publish its successful bindings.
        input.next.insert(0, boundary.clone());
        body.insert(0, boundary.clone());
        input.for_suite(body, Vec::new(), Vec::new())
    })
}

fn loop_clause(
    input: pipelines::Prepared,
    site: TextRange,
    nested: bool,
    emit: impl FnOnce(pipelines::Prepared, &Statement) -> Vec<Statement>,
) -> Vec<Statement> {
    let boundary = Statement::Position(site, None);
    if !nested {
        return emit(input, &boundary);
    }
    // Close and throw reach this final suite while the enclosing cursors are
    // still live. An inner cursor's last reference must go first.
    let mut finalbody: Vec<_> = input.clear.iter().copied().map(Statement::Clear).collect();
    let body = emit(input, &boundary);
    finalbody.push(boundary);
    vec![Statement::Try {
        body,
        handlers: Vec::new(),
        orelse: Vec::new(),
        finalbody,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(module: &crate::modules::SourceModule) -> HashMap<TextRange, RegisteredGenerator> {
        let body = &module.parsed.syntax().body;
        let literals = values::ModuleSource::new(module);
        let mut sites = LexicalOperands::default();
        crate::frontend::function_scope::ScopeFacts::prepare(
            body,
            &Default::default(),
            |_, _| {},
            |site| sites.prepare(site, literals.context(&module.source), false),
        );
        sites.generators
    }

    fn iterator_parameter(child: &RegisteredGenerator) -> usize {
        let [slot] = child.body.function.params[..] else {
            panic!("the child takes the iterator acquired by its creator");
        };
        assert_eq!(child.body.local_names[&slot].name, ".0");
        slot
    }

    #[test]
    fn async_child_pulls_the_outer_iterator_without_reacquiring_it() {
        let graph =
            SourceGraph::single("result = (item async for item in rows if item)\n").unwrap();
        let module = &graph.modules[graph.entry];
        let sites = prepared(module);
        let [child] = sites.values().collect::<Vec<_>>()[..] else {
            panic!("expected one prepared generator");
        };
        assert!(child.is_async());
        assert_eq!(child.metadata.parameter_names().collect::<Vec<_>>(), [".0"]);
        let argument = iterator_parameter(child);
        let body = &child.body.function.body;
        let mut pulls = 0;
        let mut inputs = 0;
        ir::walk_statements(body, &mut |statement| {
            if matches!(statement, Statement::Assign(_, value) if matches!(value.kind, ExprKind::Local(slot) if slot == argument))
            {
                inputs += 1;
            }
            if matches!(
                statement,
                Statement::IterNext {
                    kind: ir::IterNextKind::Async(_),
                    ..
                }
            ) {
                pulls += 1;
            }
        });
        let mut yields = 0;
        ir::walk_expressions(body, &mut |expr| {
            assert!(!matches!(
                expr.kind,
                ExprKind::Builtin(Builtin::RegisteredIter(ir::IteratorOp::Async(_)), _)
            ));
            if matches!(
                expr.kind,
                ExprKind::Builtin(Builtin::RegisteredSuspend(ir::SuspendValue::Yield, _), _)
            ) {
                yields += 1;
            }
        });
        assert_eq!((inputs, pulls, yields), (1, 1, 1));
    }

    #[test]
    fn await_only_child_adapts_its_iterator_and_defers_filter_and_projection() {
        let graph = SourceGraph::single(
            "async def read(rows, keep, parse):\n    return (await parse(x) for x in rows if await keep(x))\n",
        )
        .unwrap();
        let module = &graph.modules[graph.entry];
        let sites = prepared(module);
        let [child] = sites.values().collect::<Vec<_>>()[..] else {
            panic!("expected one prepared generator");
        };
        assert!(child.is_async());
        let mut awaits = 0;
        let mut yields = 0;
        let mut adapted = 0;
        let argument = iterator_parameter(child);
        ir::walk_expressions(&child.body.function.body, &mut |expr| match &expr.kind {
            ExprKind::Builtin(Builtin::RegisteredIter(ir::IteratorOp::LoopCursor), args) => {
                assert!(
                    matches!(args.as_slice(), [arg] if matches!(arg.kind, ExprKind::Local(slot) if slot == argument))
                );
                adapted += 1;
            }
            ExprKind::Builtin(Builtin::RegisteredSuspend(ir::SuspendValue::Await, _), _) => {
                awaits += 1;
            }
            ExprKind::Builtin(Builtin::RegisteredSuspend(ir::SuspendValue::Yield, _), _) => {
                yields += 1;
            }
            _ => {}
        });
        assert_eq!((adapted, awaits, yields), (1, 2, 1));
    }

    #[test]
    fn nested_async_child_awaits_a_lambda_default_in_its_own_frame() {
        let graph = SourceGraph::single(
            "async def make(rows, tags):\n    return ((lambda tag=tag, *, value=await fetch(x): (tag, value) async for x in rows) for tag in tags)\n",
        )
        .unwrap();
        let module = &graph.modules[graph.entry];
        let sites = prepared(module);
        assert_eq!(sites.len(), 2);
        let (inner, child) = sites.iter().find(|(_, child)| child.is_async()).unwrap();
        let parent = sites.values().find(|child| !child.is_async()).unwrap();
        let mut references = Vec::new();
        ir::walk_expressions(&parent.body.function.body, &mut |expr| {
            if let ExprKind::Builtin(Builtin::RegisteredGenerator(range, ..), _) = &expr.kind {
                references.push(range);
            }
        });
        assert_eq!(references, [inner]);

        let mut bindings = Vec::new();
        ir::walk_expressions(&child.body.function.body, &mut |expr| {
            if let ExprKind::Builtin(Builtin::RegisteredLambda(_, false), defaults) = &expr.kind {
                bindings.push(defaults.as_slice());
            }
        });
        let [[first, second]] = bindings.as_slice() else {
            panic!("expected one lambda with two defaults");
        };
        let mut operands = Vec::new();
        for default in [first, second] {
            ir::walk_operand_expressions(default, &mut |expr| match &expr.kind {
                ExprKind::Builtin(Builtin::RegisteredName(name), _) if name == "tag" => {
                    operands.push("tag");
                }
                ExprKind::Builtin(Builtin::RegisteredSuspend(ir::SuspendValue::Await, _), _) => {
                    operands.push("await");
                }
                _ => {}
            });
        }
        assert_eq!(operands, ["tag", "await"]);
    }

    #[test]
    fn nested_generators_starting_at_one_offset_reference_distinct_children() {
        let graph = SourceGraph::single("list((y for y in x) for x in rows)\n").unwrap();
        let module = &graph.modules[graph.entry];
        let sites = prepared(module);
        let mut ranges: Vec<_> = sites.keys().copied().collect();
        ranges.sort_by_key(|range| range.end());
        let [inner, outer] = ranges.as_slice() else {
            panic!("both generator bodies must be prepared");
        };
        assert_eq!(inner.start(), outer.start());
        let mut references = Vec::new();
        ir::walk_expressions(&sites[outer].body.function.body, &mut |expr| {
            if let ExprKind::Builtin(Builtin::RegisteredGenerator(range, ..), _) = &expr.kind {
                references.push(*range);
            }
        });
        assert_eq!(references, [*inner]);
    }
}
