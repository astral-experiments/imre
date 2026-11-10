//! Prepare the hidden activation of a generator expression.

use super::*;
use crate::frontend::comprehensions::EagerDisplay;
use imre_ast::comprehension::Plan;

pub(crate) struct RegisteredGenerator {
    pub(crate) body: RegisteredFunction,
    pub(crate) is_async: bool,
}

impl RegisteredGenerator {
    /// Select the iterator acquired by the defining frame before the child can escape.
    pub(crate) fn outer_operation(&self, outer: &ast::Comprehension) -> ir::IteratorOp {
        if outer.is_async {
            ir::IteratorOp::Async(outer.iter.range())
        } else if self.is_async {
            ir::IteratorOp::Get(outer.iter.range())
        } else {
            ir::IteratorOp::Loop
        }
    }

    pub(super) fn prepare(
        generator: &ast::ExprGenerator,
        source: values::RegisteredSource<'_>,
    ) -> Option<Self> {
        let plan = Plan::generator(generator);
        let is_async = plan.body_suspends();

        // The constructor installs the target declarations and the acquired cursor in
        // a fresh comprehension frame. Reads and writes now use that frame directly.
        let operations = HashMap::new();
        let sites = StatementSites::default();
        let mut scope = SourceScope::new(
            RegisteredScope::Function { is_async },
            source,
            &operations,
            &sites,
        );
        scope.generator_expression = true;
        let outer = imre_ast::comprehension::outer_clause(&generator.generators);
        let (cursor, kind) = match eager::async_pull(outer) {
            Some(pull) => (values::name_binding(".0"), ir::IterNextKind::Async(pull)),
            None if is_async => (
                operation(
                    Builtin::RegisteredIter(ir::IteratorOp::LoopCursor),
                    Type::PythonObject,
                    vec![values::name_binding(".0")],
                ),
                ir::IterNextKind::Generic,
            ),
            None => (values::name_binding(".iterator"), ir::IterNextKind::Generic),
        };
        let input = pipelines::Prepared::pull_cursor(
            cursor,
            Type::PythonObject,
            kind,
            |ty| scope.hidden(ty),
        );
        let mut body = scope
            .comprehension_loop(&plan, 0, None, input, &mut |scope, output, _| {
                let item = output.value();
                let value = scope.ordinary_operand(item).ok_or(())?;
                Ok(vec![Statement::Expr(operation(
                    Builtin::RegisteredSuspend(ir::SuspendValue::Yield, item.range()),
                    Type::PythonObject,
                    vec![value],
                ))])
            })
            .ok()?;
        body.push(Statement::Return(Expr::none()));
        Some(Self {
            body: scope.finish(body),
            is_async,
        })
    }
}

/// Emit one lazy clause using the checked iterator scope and the registered unwind edges.
pub(super) fn clause(
    mut input: pipelines::Prepared,
    binding: Vec<Statement>,
    mut body: Vec<Statement>,
    site: TextRange,
    nested: bool,
) -> Vec<Statement> {
    // Publish the acquired cursor before each pull. Release unpack temporaries
    // once targets are bound, before evaluating filters or opening another loop.
    let boundary = Statement::Expr(operation(
        Builtin::RegisteredStatement(site, None),
        Type::None,
        vec![],
    ));
    input.next.insert(0, boundary.clone());
    body.insert(0, boundary.clone());
    if !nested {
        return input.for_suite(binding, body, Vec::new(), Vec::new());
    }
    // Close and throw reach this final suite while the enclosing cursors are
    // still live. An inner cursor's last reference must go first.
    let mut finalbody: Vec<_> = input.clear.iter().copied().map(Statement::Clear).collect();
    finalbody.push(boundary);
    vec![Statement::Try {
        body: input.for_suite(binding, body, Vec::new(), Vec::new()),
        handlers: Vec::new(),
        orelse: Vec::new(),
        finalbody,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_child_pulls_the_outer_iterator_without_reacquiring_it() {
        let graph = SourceGraph::single("result = (item async for item in rows if item)\n").unwrap();
        let module = &graph.modules[graph.entry];
        let literals = values::ModuleSource::new(module);
        let (_, sites) = prepare_expression_sites(
            &module.parsed.syntax().body,
            literals.context(&module.source),
        );
        let [child] = sites.values().collect::<Vec<_>>()[..] else {
            panic!("expected one prepared generator");
        };
        assert!(child.is_async);
        let body = &child.body.function.body;
        let mut pulls = 0;
        ir::walk_statements(body, &mut |statement| {
            if matches!(statement, Statement::IterNext { kind: ir::IterNextKind::Async(_), .. }) {
                pulls += 1;
            }
        });
        let mut yields = 0;
        ir::walk_expressions(body, &mut |expr| {
            assert!(!matches!(expr.kind, ExprKind::Builtin(Builtin::RegisteredIter(ir::IteratorOp::Async(_)), _)));
            if matches!(expr.kind, ExprKind::Builtin(Builtin::RegisteredSuspend(ir::SuspendValue::Yield, _), _)) {
                yields += 1;
            }
        });
        assert_eq!((pulls, yields), (1, 1));
    }

    #[test]
    fn await_only_child_adapts_its_iterator_and_defers_filter_and_projection() {
        let graph = SourceGraph::single(
            "async def read(rows, keep, parse):\n    return (await parse(x) for x in rows if await keep(x))\n",
        )
        .unwrap();
        let module = &graph.modules[graph.entry];
        let literals = values::ModuleSource::new(module);
        let (_, sites) = prepare_expression_sites(
            &module.parsed.syntax().body,
            literals.context(&module.source),
        );
        let [child] = sites.values().collect::<Vec<_>>()[..] else {
            panic!("expected one prepared generator");
        };
        assert!(child.is_async);
        let mut awaits = 0;
        let mut yields = 0;
        let mut adapted = 0;
        ir::walk_expressions(&child.body.function.body, &mut |expr| match &expr.kind {
            ExprKind::Builtin(Builtin::RegisteredIter(ir::IteratorOp::LoopCursor), _) => {
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
        let literals = values::ModuleSource::new(module);
        let (_, sites) = prepare_expression_sites(
            &module.parsed.syntax().body,
            literals.context(&module.source),
        );
        assert_eq!(sites.len(), 2);
        let (inner, child) = sites.iter().find(|(_, child)| child.is_async).unwrap();
        let parent = sites.values().find(|child| !child.is_async).unwrap();
        let mut references = Vec::new();
        ir::walk_expressions(&parent.body.function.body, &mut |expr| {
            if let ExprKind::Builtin(Builtin::RegisteredGenerator(range, _), _) = &expr.kind {
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
        let literals = values::ModuleSource::new(module);
        let (_, sites) = prepare_expression_sites(
            &module.parsed.syntax().body,
            literals.context(&module.source),
        );
        let mut ranges: Vec<_> = sites.keys().copied().collect();
        ranges.sort_by_key(|range| range.end());
        let [inner, outer] = ranges.as_slice() else {
            panic!("both generator bodies must be prepared");
        };
        assert_eq!(inner.start(), outer.start());
        let mut references = Vec::new();
        ir::walk_expressions(&sites[outer].body.function.body, &mut |expr| {
            if let ExprKind::Builtin(Builtin::RegisteredGenerator(range, _), _) = &expr.kind {
                references.push(*range);
            }
        });
        assert_eq!(references, [*inner]);
    }
}
