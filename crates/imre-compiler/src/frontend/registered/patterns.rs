//! Prepare value lookups and structural patterns in their declaring Python scope.
use super::*;
use ir::MatchTest;

type PatternResult<T> = std::result::Result<T, (TextRange, &'static str)>;

impl SuitePreparation<'_> {
    pub(super) fn prepare_match(
        &mut self,
        branch: &ast::StmtMatch,
        kind: RegisteredScope,
        suspended: bool,
    ) -> PatternResult<RegisteredSuite> {
        let mut cases = prepare_cases(branch, self.source)?;
        let operand = |prepare: &mut Self, expr: &ast::Expr| {
            prepare.owned_operand(expr, kind, suspended, |scope| scope.frame_value(expr))
        };
        let mut region = self.region(kind);
        let subject = region.operand(operand(self, &branch.subject), Type::PythonObject);
        let mut scope = self.source_scope(kind);
        for (case, written) in cases.iter_mut().zip(&branch.cases) {
            case.captures = scope.match_slots(&case.pattern);
            case.guard = written
                .guard
                .as_deref()
                .map(|guard| region.operand(operand(self, guard), Type::Bool));
            case.body = region.child(self.physical_suite(
                &written.body,
                kind,
                SuiteSelection::All,
                suspended,
            ));
        }
        region.checked = scope.finish(vec![
            Statement::Position(branch.range(), None),
            Statement::Match(subject, cases),
        ]);
        Ok(region)
    }
}

/// Check every pattern before preparing its guard or body. The case keeps its
/// pattern in the same vector when a caller binds the remaining operands.
pub(super) fn prepare_cases(
    branch: &ast::StmtMatch,
    source: values::RegisteredSource<'_>,
) -> PatternResult<Vec<ir::MatchCase>> {
    let mut cases = Vec::with_capacity(branch.cases.len());
    for case in &branch.cases {
        cases.push(ir::MatchCase {
            pattern: prepare_pattern(&case.pattern, source)?,
            captures: Vec::new(),
            guard: None,
            body: Vec::new(),
        });
    }
    Ok(cases)
}

fn prepare_pattern(
    pattern: &ast::Pattern,
    source: values::RegisteredSource<'_>,
) -> PatternResult<ir::MatchPattern> {
    struct Builder<'a> {
        captures: Vec<String>,
        source: values::RegisteredSource<'a>,
    }
    impl Builder<'_> {
        fn capture(&mut self, name: &ast::Identifier) -> usize {
            if let Some(index) = self
                .captures
                .iter()
                .position(|other| other == name.as_str())
            {
                return index;
            }
            let index = self.captures.len();
            self.captures.push(name.to_string());
            index
        }

        fn operand(&self, node: &ast::Expr, reason: &'static str) -> PatternResult<Expr> {
            let invalid = (node.range(), reason);
            if !matches!(
                node,
                ast::Expr::Name(_) | ast::Expr::NoneLiteral(_) | ast::Expr::BooleanLiteral(_)
            ) && imre_ast::executable::value_pattern_error(node, self.source.text).is_some()
            {
                return Err(invalid);
            }
            values::registered_value(node, self.source).ok_or(invalid)
        }

        fn lower(&mut self, node: &ast::Pattern) -> PatternResult<MatchTest> {
            Ok(match node {
                ast::Pattern::MatchValue(p) => {
                    if let Some(reason) =
                        imre_ast::executable::value_pattern_error(&p.value, self.source.text)
                    {
                        return Err((p.value.range(), reason));
                    }
                    MatchTest::Equal(self.operand(&p.value, "patterns may only match literals and attribute lookups")?)
                }
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
                        .collect::<PatternResult<_>>()?,
                ),
                ast::Pattern::MatchSequence(p) => {
                    let mut star = None;
                    let mut items = Vec::with_capacity(p.patterns.len());
                    for child in &p.patterns {
                        if child.is_match_star() {
                            star = Some(items.len());
                        }
                        items.push(self.lower(child)?);
                    }
                    MatchTest::Sequence { star, items }
                }
                ast::Pattern::MatchMapping(p) => MatchTest::Mapping(
                    p.keys
                        .iter()
                        .zip(&p.patterns)
                        .map(|(key, item)| {
                            let value = self.operand(key, "mapping pattern keys may only match literals and attribute lookups")?;
                            Ok((value, self.lower(item)?))
                        })
                        .collect::<PatternResult<_>>()?,
                    p.rest.as_ref().map(|id| self.capture(id)),
                ),
                ast::Pattern::MatchClass(p) => MatchTest::Class {
                    range: p.range(),
                    class: self.operand(&p.cls, "expected a class name in match pattern")?,
                    positional: p
                        .arguments
                        .patterns
                        .iter()
                        .map(|child| self.lower(child))
                        .collect::<PatternResult<_>>()?,
                    keywords: p
                        .arguments
                        .keywords
                        .iter()
                        .map(|kw| Ok((kw.attr.to_string(), self.lower(&kw.pattern)?)))
                        .collect::<PatternResult<_>>()?,
                },
            })
        }
    }
    let mut builder = Builder {
        captures: Vec::new(),
        source,
    };
    Ok(ir::MatchPattern {
        test: builder.lower(pattern)?,
        names: builder.captures,
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
        let [item, tail, rest] = cases[0].captures[..] else {
            panic!("expected three mapping captures");
        };
        assert_eq!(cases[1].captures, [item]);
        let mut words = vec![];
        let mut capture_reads = vec![];
        ir::walk_expressions(&suite.function.body, &mut |expr| match &expr.kind {
            ExprKind::Builtin(Builtin::RegisteredName(name), _) => words.push(name.as_str()),
            ExprKind::Local(slot) => {
                words.push(suite.local_names[slot].name.as_str());
                capture_reads.push(*slot);
            }
            _ => {}
        });
        assert_eq!(
            words,
            [
                "subject", "keys", "models", "allowed", "item", "use", "item", "tail", "rest"
            ]
        );
        assert_eq!(capture_reads, [item, item, tail, rest]);
    }
}
