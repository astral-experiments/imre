//! Prepare the hidden activation of a synchronous generator expression.

use super::*;
use crate::frontend::comprehensions::EagerDisplay;
use imre_ast::comprehension::Plan;

pub(crate) struct RegisteredGenerator {
    pub(crate) body: RegisteredFunction,
}

impl RegisteredGenerator {
    pub(super) fn prepare(
        generator: &ast::ExprGenerator,
        source: values::RegisteredSource<'_>,
    ) -> Option<Self> {
        let plan = Plan::generator(generator);
        if plan.body_suspends() {
            return None;
        }

        // The constructor installs the target declarations and the acquired cursor in
        // a fresh comprehension frame. Reads and writes now use that frame directly.
        let operations = HashMap::new();
        let sites = StatementSites::default();
        let mut scope = SourceScope::new(
            RegisteredScope::Function { is_async: false },
            source,
            &operations,
            &sites,
        );
        let input = pipelines::Prepared::cursor(
            values::name_binding(".iterator"),
            Type::PythonObject,
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
