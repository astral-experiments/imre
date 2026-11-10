//! Prepare value lookups and structural patterns in their declaring Python scope.
use super::*;
use ir::MatchTest;

pub(crate) struct SourceMatch {
    pub(crate) subject: PhysicalOperand,
    pub(crate) cases: Vec<ir::MatchCase<PhysicalOperand, PhysicalSuite, ()>>,
}

impl SuitePreparation<'_> {
    pub(super) fn prepare_match(
        &mut self,
        branch: &ast::StmtMatch,
        kind: RegisteredScope,
        suspended: bool,
    ) -> Option<SourceMatch> {
        let patterns = prepare_patterns(branch, self.source)?;
        let operand = |prepare: &mut Self, expr: &ast::Expr| {
            prepare.owned_operand(expr, kind, suspended, |scope| scope.frame_value(expr))
        };
        let subject = operand(self, &branch.subject);
        let cases = patterns
            .into_iter()
            .zip(&branch.cases)
            .map(|(pattern, case)| ir::MatchCase {
                pattern,
                captures: (),
                guard: case.guard.as_deref().map(|guard| operand(self, guard)),
                body: self.physical_suite(&case.body, kind, SuiteSelection::All, suspended),
            })
            .collect();
        Some(SourceMatch { subject, cases })
    }
}

pub(super) fn prepare_patterns(
    branch: &ast::StmtMatch,
    source: values::RegisteredSource<'_>,
) -> Option<Vec<ir::MatchPattern>> {
    branch
        .cases
        .iter()
        .map(|case| prepare_pattern(&case.pattern, source))
        .collect()
}

fn prepare_pattern(
    pattern: &ast::Pattern,
    source: values::RegisteredSource<'_>,
) -> Option<ir::MatchPattern> {
    struct Builder<'a> {
        captures: Vec<String>,
        source: values::RegisteredSource<'a>,
    }
    impl Builder<'_> {
        fn capture(&mut self, name: &ast::Identifier) -> String {
            if !self.captures.iter().any(|other| other == name.as_str()) {
                self.captures.push(name.to_string());
            }
            name.to_string()
        }

        fn operand(&self, node: &ast::Expr) -> Option<Expr> {
            match node {
                ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::USub => {
                    Some(self.source.located(node, negative_literal(&unary.operand)?))
                }
                ast::Expr::BinOp(bin)
                    if matches!(bin.op, ast::Operator::Add | ast::Operator::Sub) =>
                {
                    // Python's complex literal pattern is a signed real plus or minus an
                    // imaginary literal. Adding its signed imaginary part keeps the exact
                    // integer-to-complex conversion in the shared binary operation.
                    let left = self.operand(&bin.left)?;
                    let right = if bin.op == ast::Operator::Sub {
                        self.source
                            .located(&bin.right, negative_literal(&bin.right)?)
                    } else {
                        self.operand(&bin.right)?
                    };
                    Some(self.source.located(
                        node,
                        Expr {
                            ty: Type::PythonObject,
                            kind: ExprKind::Binary(Binary::Add, Box::new(left), Box::new(right)),
                        },
                    ))
                }
                _ => values::registered_value(node, self.source),
            }
        }

        fn lower(&mut self, node: &ast::Pattern) -> Option<MatchTest> {
            Some(match node {
                ast::Pattern::MatchValue(p) => MatchTest::Equal(self.operand(&p.value)?),
                ast::Pattern::MatchSingleton(p) => MatchTest::Singleton(match p.value {
                    ast::Singleton::None => None,
                    ast::Singleton::True => Some(true),
                    ast::Singleton::False => Some(false),
                }),
                ast::Pattern::MatchAs(p) => {
                    let inner = match &p.pattern {
                        Some(child) => self.lower(child)?,
                        None => MatchTest::Always,
                    };
                    match &p.name {
                        Some(name) => MatchTest::Capture(Box::new(inner), self.capture(name)),
                        None => inner,
                    }
                }
                ast::Pattern::MatchStar(p) => match &p.name {
                    Some(name) => {
                        MatchTest::Capture(Box::new(MatchTest::Always), self.capture(name))
                    }
                    None => MatchTest::Always,
                },
                ast::Pattern::MatchOr(p) => MatchTest::Or(
                    p.patterns
                        .iter()
                        .map(|child| self.lower(child))
                        .collect::<Option<_>>()?,
                ),
                ast::Pattern::MatchSequence(p) => MatchTest::Sequence(
                    imre_ast::sequence_pattern_plan(&p.patterns),
                    p.patterns
                        .iter()
                        .map(|child| self.lower(child))
                        .collect::<Option<_>>()?,
                ),
                ast::Pattern::MatchMapping(p) => MatchTest::Mapping(
                    p.keys
                        .iter()
                        .zip(&p.patterns)
                        .map(|(key, item)| Some((self.operand(key)?, self.lower(item)?)))
                        .collect::<Option<_>>()?,
                    p.rest.as_ref().map(|id| self.capture(id)),
                ),
                ast::Pattern::MatchClass(p) => MatchTest::Class {
                    class: self.operand(&p.cls)?,
                    positional: p
                        .arguments
                        .patterns
                        .iter()
                        .map(|child| self.lower(child))
                        .collect::<Option<_>>()?,
                    keywords: p
                        .arguments
                        .keywords
                        .iter()
                        .map(|kw| Some((kw.attr.to_string(), self.lower(&kw.pattern)?)))
                        .collect::<Option<_>>()?,
                },
            })
        }
    }
    let mut builder = Builder {
        captures: Vec::new(),
        source,
    };
    Some(ir::MatchPattern {
        test: builder.lower(pattern)?,
        names: builder.captures,
    })
}

fn negative_literal(node: &ast::Expr) -> Option<Expr> {
    let number = &node.as_number_literal_expr()?.value;
    Some(Expr {
        ty: Type::PythonObject,
        kind: values::number_literal(number, true),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_structural_cases_keep_names_operands_and_guard_order() {
        let text = concat!(
            "match subject():\n",
            "    case {keys.id: [models.Row(field=item), *tail], **rest} if allowed(item):\n",
            "        use(item, tail, rest)\n",
            "    case (1 as item) | (2 as item):\n",
            "        pass\n",
        );
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let source = values::ModuleSource::new(module);
        let suite = super::super::tests::registered_suite(
            &module.parsed.syntax().body,
            RegisteredScope::Module,
            source.context(text),
        )
        .unwrap();
        let [_, Statement::Match(_, cases)] = suite.function.body.as_slice() else {
            panic!("expected a prepared match statement");
        };
        assert_eq!(cases[0].pattern.names, ["item", "tail", "rest"]);
        assert!(matches!(cases[0].pattern.test, MatchTest::Mapping(..)));
        assert!(matches!(cases[1].pattern.test, MatchTest::Or(..)));
        assert_eq!(cases[1].captures, [cases[0].captures[0]]);
        let mut words = vec![];
        ir::walk_expressions(&suite.function.body, &mut |expr| {
            if let ExprKind::Builtin(Builtin::RegisteredName(name), _) = &expr.kind {
                words.push(name.as_str());
            }
        });
        assert_eq!(
            words,
            [
                "subject", "keys", "models", "allowed", "item", "use", "item", "tail", "rest"
            ]
        );
    }
}
