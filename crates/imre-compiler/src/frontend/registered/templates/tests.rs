use super::*;
use crate::llvm::registered::{Frame, Output, RegisteredFrame, SourcePositions, tests::TestFrame};

#[test]
fn template_children_use_their_eager_bindings_after_source_is_dropped() {
    let spelling = r#" [f"<{client.read(v)!r}>" for v in v]"#;
    let operand = {
        let source = String::from(
            r#"rows = [t"""{ [f"<{client.read(v)!r}>" for v in v] !a:>{f"W{client.width(v)!s}"}}""" for v in batches]"#,
        );
        let parsed = ruff_python_parser::parse_module(&source).unwrap();
        let fields = imre_ast::strings::template_fields(&source, parsed.tokens(), parsed.syntax());
        let assignment = parsed.syntax().body[0].as_assign_stmt().unwrap();
        RegisteredOperand::prepare(
            &assignment.value,
            RegisteredSource {
                python_optimization: 0,
                operand_types: None,
                text: &source,
                private_names: None,
                replacements: &Default::default(),
                template_fields: Some(&fields),
                lambdas: None,
                generators: None,
                method_imports: &Default::default(),
            },
            RegisteredScope::Module,
        )
        .unwrap()
    };

    let mut scopes = HashMap::new();
    let mut parents = HashMap::new();
    let mut captures = 0;
    operand.region().walk(&mut |node, _| match node {
        ir::Node::Statement(Statement::Assign(slot, value)) => {
            if let ExprKind::Builtin(Builtin::RegisteredNamespace(_), args) = &value.kind {
                parents.insert(*slot, args.as_slice());
            }
        }
        ir::Node::Expression(Expr {
            kind: ExprKind::Invoke { args, .. },
            ..
        }) => {
            let site = args.site.python().unwrap();
            let ir::RegisteredCallTarget::Method { name, .. } = &site.call.target else {
                panic!("expected a method call");
            };
            let [arg] = args.values.as_slice() else {
                panic!("expected a single argument");
            };
            let scope = site
                .scope
                .as_deref()
                .expect("expected a comprehension namespace");
            let ExprKind::Local(namespace) = scope.kind else {
                panic!("expected a comprehension namespace");
            };
            let ExprKind::Builtin(Builtin::RegisteredLocation(..), value) = &arg.kind else {
                panic!("expected a positioned argument");
            };
            let [
                Expr {
                    kind: ExprKind::Local(target),
                    ..
                },
            ] = value.as_slice()
            else {
                panic!("expected the comprehension target as the argument");
            };
            let binding = &operand.local_names[target];
            assert_eq!(binding.name, "v");
            assert_eq!(binding.namespace, Some(namespace));
            assert!(scopes.insert(name.as_str(), namespace).is_none());
        }
        ir::Node::Expression(Expr {
            kind:
                ExprKind::Builtin(
                    Builtin::RegisteredInterpolation {
                        expression,
                        conversion,
                    },
                    _,
                ),
            ..
        }) => {
            assert_eq!(expression, spelling);
            assert_eq!(*conversion, Some('a'));
            captures += 1;
        }
        _ => {}
    });
    assert_eq!(captures, 1);
    assert_ne!(scopes["read"], scopes["width"]);
    assert!(parents[&scopes["width"]].is_empty());
    assert!(
        matches!(parents[&scopes["read"]], [Expr { kind: ExprKind::Local(parent), .. }] if *parent == scopes["width"])
    );

    let mut output = Output::default();
    let file = output.data(b"lexical.py");
    let positions = SourcePositions::new(std::path::Path::new("lexical.py"), "", &file);
    let mut frame = RegisteredFrame::new(&mut output, &positions, false, 0, 0.into());
    Frame::evaluate_operand(
        &mut TestFrame(&mut frame, Default::default()),
        &operand.value,
        Some(&operand),
        false,
    );
    frame.finish_callable(None);
    let code = &output.functions;
    assert_eq!(
        code.matches("call i64 @imre_dyn_builtin_method_target(")
            .count(),
        2
    );
    assert_eq!(code.matches("call i64 @imre_dyn_format_parts(").count(), 3);
    assert!(code.contains("call void @imre_dyn_unwind_operands("));
    let symbol = output.strings.get_index_of(spelling.as_bytes()).unwrap();
    assert!(code.contains(&format!("ptr @str{symbol},")));
}

#[test]
fn template_async_comprehension_uses_its_enclosing_activation() {
    let text = "async def collect(rows):\n    return t'{[await load(v) async for v in rows]}'\n";
    let parsed = ruff_python_parser::parse_module(text).unwrap();
    let fields = imre_ast::strings::template_fields(text, parsed.tokens(), parsed.syntax());
    let definition = parsed.syntax().body[0].as_function_def_stmt().unwrap();
    let result = definition.body[0]
        .as_return_stmt()
        .unwrap()
        .value
        .as_deref()
        .unwrap();
    let source = RegisteredSource {
        python_optimization: 0,
        operand_types: None,
        text,
        private_names: None,
        replacements: &Default::default(),
        template_fields: Some(&fields),
        lambdas: None,
        generators: None,
        method_imports: &Default::default(),
    };
    for owner in [
        RegisteredScope::Module,
        RegisteredScope::Class,
        RegisteredScope::Function { is_async: false },
    ] {
        assert!(RegisteredOperand::prepare(result, source, owner).is_none());
    }
    let operand =
        RegisteredOperand::prepare(result, source, RegisteredScope::Function { is_async: true })
            .unwrap();
    let mut awaits = 0;
    let mut pulls = 0;
    operand.region().walk(&mut |node, _| match node {
        ir::Node::Expression(Expr {
            kind: ExprKind::Builtin(Builtin::RegisteredSuspend(ir::SuspendValue::Await, _), _),
            ..
        }) => awaits += 1,
        ir::Node::Statement(Statement::IterNext {
            kind: ir::IterNextKind::Async(_),
            ..
        }) => pulls += 1,
        _ => {}
    });
    assert_eq!((awaits, pulls), (1, 1));
}

#[test]
fn template_default_and_body_suspend_in_their_defining_frames() {
    let text = concat!(
        "async def factory():\n",
        "    def inner(p=t'{await fetch()}'):\n",
        "        return t'pre{(yield p)!r:>{(yield from widths)}}post'\n",
        "    return inner\n",
    );
    let templates = {
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let fields = imre_ast::strings::template_fields(text, parsed.tokens(), parsed.syntax());
        let source = RegisteredSource {
            python_optimization: 0,
            operand_types: None,
            text,
            private_names: None,
            replacements: &Default::default(),
            template_fields: Some(&fields),
            lambdas: None,
            generators: None,
            method_imports: &Default::default(),
        };
        let mut body = parsed.into_syntax().body;
        let names = imre_ast::lower(&mut body);
        let mut sites = LexicalOperands {
            expressions: Some(Default::default()),
            ..Default::default()
        };
        crate::frontend::function_scope::ScopeFacts::prepare(
            &body,
            &names,
            |_, _| {},
            |site| sites.prepare(site, source, names.string_annotations),
        );
        sites.expressions.unwrap()
    };
    let mut sites: Vec<_> = templates.templates().collect();
    sites.sort_by_key(|(range, _)| range.start());
    assert_eq!(sites.len(), 2);

    let mut captures = Vec::new();
    let mut suspensions = Vec::new();
    let mut formats = 0;
    for (_, operand) in sites {
        let ExpressionSelection::Operand(operand) = operand else {
            panic!("expected a complete template operand");
        };
        crate::verify::registered(operand.region()).unwrap();
        ir::walk_operand_expressions(&operand.value, &mut |node| match &node.kind {
            ExprKind::Builtin(
                Builtin::RegisteredInterpolation {
                    expression,
                    conversion,
                },
                _,
            ) => {
                captures.push((expression.as_str(), *conversion));
            }
            ExprKind::Builtin(Builtin::RegisteredSuspend(kind, range), _) => {
                let op = match kind {
                    ir::SuspendValue::Await => "await",
                    ir::SuspendValue::Yield => "yield",
                    ir::SuspendValue::Delegate => "yield from",
                };
                suspensions.push((op, &text[*range]));
            }
            ExprKind::Builtin(Builtin::FormatField(_), _) => formats += 1,
            _ => {}
        });
    }
    assert_eq!(
        captures,
        [("await fetch()", None), ("(yield p)", Some('r'))]
    );
    assert_eq!(
        suspensions,
        [
            ("await", "await fetch()"),
            ("yield", "yield p"),
            ("yield from", "yield from widths"),
        ]
    );
    assert_eq!(formats, 1);
}

#[test]
fn unavailable_template_child_is_reported_at_its_own_range() {
    let text = concat!(
        "def report(rows, fetch):\n",
        "    return t'{mark()}{([lambda v=await fetch(x): v for x in rows] for _ in rows)=}', ",
        "t'{mark()}{([lambda v=await fetch(x): v for x in rows] for _ in rows)}'\n",
    );
    let parsed = ruff_python_parser::parse_module(text).unwrap();
    let module = parsed.syntax();
    let fields = imre_ast::strings::template_fields(text, parsed.tokens(), module);
    let source = RegisteredSource {
        python_optimization: 0,
        operand_types: None,
        text,
        private_names: None,
        replacements: &Default::default(),
        template_fields: Some(&fields),
        lambdas: None,
        generators: None,
        method_imports: &Default::default(),
    };
    let function = module.body[0].as_function_def_stmt().unwrap();
    let templates = function.body[0]
        .as_return_stmt()
        .unwrap()
        .value
        .as_deref()
        .unwrap()
        .as_tuple_expr()
        .unwrap();
    for complete in [false, true] {
        let mut sites = LexicalOperands {
            expressions: Some(Default::default()),
            ..Default::default()
        };
        crate::frontend::function_scope::ScopeFacts::prepare(
            &module.body,
            &Default::default(),
            |_, _| {},
            |site| {
                if complete
                    || !matches!(
                        site,
                        crate::frontend::function_scope::LexicalSite::Generator(..)
                    )
                {
                    sites.prepare(site, source, false);
                }
            },
        );
        let sites = sites.expressions.unwrap();
        for template in &templates.elts {
            let (_, result) = sites
                .templates()
                .find(|(range, _)| *range == template.range())
                .unwrap();
            let tail = template
                .as_t_string_expr()
                .unwrap()
                .value
                .iter()
                .flat_map(|part| part.elements.interpolations())
                .nth(1)
                .unwrap();
            if complete {
                let ExpressionSelection::Operand(operand) = result else {
                    panic!("expected the prepared generator");
                };
                crate::verify::registered(operand.region()).unwrap();
                let mut captures = Vec::new();
                ir::walk_operand_expressions(&operand.value, &mut |expr| {
                    if let ExprKind::Builtin(
                        Builtin::RegisteredInterpolation { expression, .. },
                        _,
                    ) = &expr.kind
                    {
                        captures.push(expression.clone());
                    }
                });
                assert_eq!(captures, ["mark()", &text[tail.expression.range()]]);
            } else {
                let ExpressionSelection::Refused(range, reason) = result else {
                    panic!("expected the unavailable generator");
                };
                assert_eq!(*range, tail.expression.range());
                assert_eq!(
                    *reason,
                    "cannot compile this generator expression ahead of time"
                );
            }
        }
    }
}
