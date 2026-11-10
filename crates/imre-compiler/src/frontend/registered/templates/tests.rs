use super::*;
use crate::llvm::registered::{Output, RegisteredFrame, SourcePositions, sites::CheckedSites};

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
                text: &source,
                private_names: None,
                replacements: &Default::default(),
                has_templates: true,
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
    let mut captures = 0;
    ir::walk_operand_expressions(&operand.value, &mut |node| match &node.kind {
        ExprKind::Builtin(Builtin::RegisteredCall(call), args) => {
            let ir::RegisteredCallTarget::Method { name, .. } = &call.target else {
                panic!("expected a method call");
            };
            let [scope, _, arg] = args.as_slice() else {
                panic!("expected a scope, receiver and single argument");
            };
            assert!(call.explicit_scope);
            let ExprKind::Local(namespace) = scope.kind else {
                panic!("expected a comprehension namespace");
            };
            let mut lookups = 0;
            ir::walk_operand_expressions(arg, &mut |child| {
                if let ExprKind::Builtin(Builtin::RegisteredName(id), owner) = &child.kind {
                    assert_eq!(id, "v");
                    assert!(
                        matches!(owner.as_slice(), [Expr {kind: ExprKind::Local(slot), ..}] if *slot == namespace)
                    );
                    lookups += 1;
                }
            });
            assert_eq!(lookups, 1);
            assert!(scopes.insert(name.as_str(), namespace).is_none());
        }
        ExprKind::Builtin(
            Builtin::RegisteredInterpolation {
                expression,
                conversion,
            },
            _,
        ) => {
            assert_eq!(expression, spelling);
            assert_eq!(*conversion, Some('a'));
            captures += 1;
        }
        _ => {}
    });
    assert_eq!(captures, 1);
    assert_ne!(scopes["read"], scopes["width"]);
    for namespace in scopes.values() {
        assert!(
            operand
                .local_names
                .values()
                .any(|binding| { binding.name == "v" && binding.namespace == Some(*namespace) })
        );
    }

    let mut output = Output::default();
    let file = output.data(b"lexical.py");
    let positions = SourcePositions::new("", &file);
    let mut frame = RegisteredFrame::new(&mut output, &positions, false, 0, 0.into());
    frame.prepared_operand(&operand, &CheckedSites::default(), false);
    frame.finish(false);
    let code = &output.functions;
    assert_eq!(
        code.matches("call i64 @imre_dyn_builtin_method_target(")
            .count(),
        2
    );
    assert_eq!(
        code.matches("call void @imre_dyn_finish_format_join(")
            .count(),
        3
    );
    assert!(code.contains("call void @imre_dyn_unwind_operands("));
    let symbol = output.strings[spelling.as_bytes()];
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
        text,
        private_names: None,
        replacements: &Default::default(),
        has_templates: true,
        template_fields: Some(&fields),
        lambdas: None,
        generators: None,
        method_imports: &Default::default(),
    };
    assert!(RegisteredOperand::prepare(result, source, RegisteredScope::Module).is_none());
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
